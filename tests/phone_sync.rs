//! The phone receiver through its real HTTPS server on localhost, driven
//! the way Fennec Recorder drives it: scan the code (hello), announce,
//! upload in chunks, resume, complete, done.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fennec::config::Paths;
use fennec::store::{Source, Store};
use fennec::sync::{Receiver, ReceiverConfig, SyncEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Desktop {
    _dir: tempfile::TempDir,
    paths: Paths,
    receiver: Option<Receiver>,
    events: Arc<Mutex<Vec<SyncEvent>>>,
}

impl Desktop {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut d = Self {
            _dir: dir,
            paths,
            receiver: None,
            events: Arc::default(),
        };
        d.restart();
        d
    }

    /// Stops the receiver and starts a new one with a new code on the same
    /// data (the dialog closed and opened again).
    fn restart(&mut self) {
        self.receiver = None;
        let events = Arc::clone(&self.events);
        let receiver = Receiver::start(
            ReceiverConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                paths: self.paths.clone(),
                default_project: None,
                default_template: "notat".into(),
            },
            Arc::new(move |e| events.lock().unwrap().push(e)),
        )
        .unwrap();
        self.receiver = Some(receiver);
    }

    fn receiver(&self) -> &Receiver {
        self.receiver.as_ref().unwrap()
    }

    fn ticket(&self) -> String {
        self.receiver().offer().expect("the code is shown").ticket
    }

    fn store(&self) -> Store {
        Store::open(&self.paths.database()).unwrap()
    }

    fn phone(&self) -> Phone {
        Phone {
            base: format!("https://{}", self.receiver().local_addr()),
            client: reqwest::blocking::Client::builder()
                // The phone pins the certificate instead; checked below.
                .danger_accept_invalid_certs(true)
                .tls_info(true)
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
            session: None,
        }
    }

    fn wait_for(&self, what: &str, mut f: impl FnMut(&SyncEvent) -> bool) -> SyncEvent {
        let start = Instant::now();
        loop {
            if let Some(e) = self.events.lock().unwrap().iter().find(|e| f(e)) {
                return e.clone();
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no {what} event");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A phone that scanned the code shown now.
    fn scanned_phone(&self) -> Phone {
        let mut phone = self.phone();
        let (status, body) = phone.hello(&self.ticket(), "Pixel 8");
        assert_eq!(status, 200, "{body}");
        phone.session = Some(body["session"].as_str().unwrap().to_string());
        self.events.lock().unwrap().clear();
        phone
    }
}

#[derive(Clone)]
struct Phone {
    base: String,
    client: reqwest::blocking::Client,
    session: Option<String>,
}

impl Phone {
    fn send(&self, method: reqwest::Method, path: &str, body: Option<Vec<u8>>) -> (u16, Value) {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(s) = &self.session {
            req = req.bearer_auth(s);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req.send().unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().unwrap_or(Value::Null))
    }

    fn post_json(&self, path: &str, v: Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, path, Some(serde_json::to_vec(&v).unwrap()))
    }

    fn put_json(&self, path: &str, v: Value) -> (u16, Value) {
        self.send(reqwest::Method::PUT, path, Some(serde_json::to_vec(&v).unwrap()))
    }

    fn hello(&self, ticket: &str, name: &str) -> (u16, Value) {
        self.post_json("/v1/hello", json!({ "ticket": ticket, "device_name": name }))
    }

    fn announce(&self, id: &str, audio: &[u8], extra: Value) -> (u16, Value) {
        let mut meta = json!({
            "title": "Interview, Aarhus",
            "recorded_at": 1_791_000_000_000_i64,
            "duration_ms": 1_083_000,
            "ext": "m4a",
            "size": audio.len(),
            "sha256": sha256_hex(audio),
        });
        for (k, v) in extra.as_object().unwrap() {
            meta[k] = v.clone();
        }
        self.put_json(&format!("/v1/recordings/{id}"), meta)
    }

    fn chunk(&self, id: &str, offset: usize, bytes: &[u8]) -> (u16, Value) {
        self.send(
            reqwest::Method::PUT,
            &format!("/v1/recordings/{id}/audio?offset={offset}"),
            Some(bytes.to_vec()),
        )
    }

    fn upload(&self, id: &str, audio: &[u8], from: usize) {
        let mut offset = from;
        for piece in audio[from..].chunks(1 << 20) {
            let (status, body) = self.chunk(id, offset, piece);
            assert_eq!(status, 200, "{body}");
            offset += piece.len();
            assert_eq!(body["received"], offset);
        }
    }

    fn complete(&self, id: &str) -> (u16, Value) {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/recordings/{id}/complete"),
            None,
        )
    }
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Stand-in audio: the receiver only stores and checks bytes.
fn audio(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

const ID: &str = "3f2b9c1e-8a4d-4c6e-9b7a-1d2e3f4a5b6c";

#[test]
fn the_certificate_the_phone_sees_matches_the_pin_in_the_qr_code() {
    let desktop = Desktop::start();
    let phone = desktop.phone();
    let resp = phone
        .client
        .post(format!("{}/v1/hello", phone.base))
        .send()
        .unwrap();
    let der = resp
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|t| t.peer_certificate())
        .unwrap()
        .to_vec();
    let (_, cert) = x509_parser::parse_x509_certificate(&der).unwrap();
    let spki = cert.tbs_certificate.subject_pki.raw;
    let pin = URL_SAFE_NO_PAD.encode(Sha256::digest(spki));
    let offer = desktop.receiver().offer().unwrap();
    assert_eq!(pin, desktop.receiver().pin());
    assert!(offer.uri().starts_with("fennec://send?"), "{}", offer.uri());
    assert!(offer.uri().contains(&format!("pin={pin}&")), "{}", offer.uri());
}

#[test]
fn the_phone_that_scans_the_code_gets_a_session_and_fennecs_projects() {
    let desktop = Desktop::start();
    let p = desktop.store().create_project("Kundemøder", "#2F6F4E").unwrap();
    let phone = desktop.phone();
    let (status, body) = phone.hello(&desktop.ticket(), "  Pixel 8  ");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["session"].as_str().unwrap().len(), 43);
    assert_eq!(body["protocol"], 2);
    assert_eq!(body["id"].as_str().unwrap(), &desktop.receiver().pin()[..12]);
    assert_eq!(
        body["projects"][0],
        json!({ "id": p, "name": "Kundemøder", "color": "#2F6F4E", "default_template": null, "documents": 0 })
    );
    assert!(body["templates"].is_array());
    assert_eq!(body["default_template"], "notat");
    assert_eq!(
        desktop.wait_for("connected", |e| matches!(e, SyncEvent::Connected { .. })),
        SyncEvent::Connected {
            device_name: "Pixel 8".into()
        }
    );
    assert!(desktop.receiver().offer().is_none(), "the code is used");
}

#[test]
fn a_code_works_once_for_one_phone() {
    let desktop = Desktop::start();
    let ticket = desktop.ticket();
    let first = desktop.phone();
    assert_eq!(first.hello(&ticket, "Pixel 8").0, 200);
    let (status, body) = desktop.phone().hello(&ticket, "Someone else");
    assert_eq!(status, 403);
    assert_eq!(body["error"], "used");
    assert!(body["message"].as_str().unwrap().contains("new one"));
}

#[test]
fn wrong_tickets_close_the_code() {
    let desktop = Desktop::start();
    let ticket = desktop.ticket();
    let phone = desktop.phone();
    for _ in 0..4 {
        assert_eq!(phone.hello("guess", "Guesser").1["error"], "wrong_ticket");
    }
    assert_eq!(phone.hello("guess", "Guesser").1["error"], "wrong_ticket");
    desktop.wait_for("code closed", |e| *e == SyncEvent::CodeClosed);
    assert_eq!(phone.hello(&ticket, "Pixel 8").1["error"], "no_ticket");
}

#[test]
fn without_the_session_nothing_gets_in() {
    let desktop = Desktop::start();
    let mut stranger = desktop.phone();
    let (status, body) = stranger.announce(ID, &audio(10), json!({}));
    assert_eq!(status, 401);
    assert_eq!(body["error"], "no_session");
    // Not even with the ticket as a bearer.
    stranger.session = Some(desktop.ticket());
    assert_eq!(stranger.announce(ID, &audio(10), json!({})).0, 401);
    let _phone = desktop.scanned_phone();
    stranger.session = Some("made-up".into());
    assert_eq!(stranger.announce(ID, &audio(10), json!({})).0, 401);
}

#[test]
fn an_older_app_is_asked_to_update() {
    let desktop = Desktop::start();
    let phone = desktop.phone();
    let (status, body) = phone.post_json(
        "/v1/pair",
        json!({ "token": "12345678", "device_name": "Old", "nonce": "nonce-0123456789abcdef" }),
    );
    assert_eq!(status, 410);
    assert_eq!(body["error"], "update_app");
}

#[test]
fn a_recording_arrives_in_chunks_and_becomes_a_queued_document() {
    let desktop = Desktop::start();
    let project = desktop.store().create_project("Kundemøder", "#2F6F4E").unwrap();
    let phone = desktop.scanned_phone();
    let data = audio(2_500_000);

    let (status, body) = phone.announce(
        ID,
        &data,
        json!({ "project_id": project, "template_id": "interview" }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["received"], 0);
    assert_eq!(body["state"], "receiving");

    // A chunk from the wrong place is refused with where to resume.
    let (status, body) = phone.chunk(ID, 100, &data[100..200]);
    assert_eq!(status, 409);
    assert_eq!(body["received"], 0);

    phone.upload(ID, &data, 0);
    let (status, body) = phone.complete(ID);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "queued");
    let doc = body["document_id"].as_i64().unwrap();

    let SyncEvent::Received {
        id,
        document_id,
        path,
    } = desktop.wait_for("received", |e| matches!(e, SyncEvent::Received { .. }))
    else {
        unreachable!()
    };
    assert_eq!((id.as_str(), document_id), (ID, doc));
    // The dialog saw it arrive, chunk by chunk.
    let seen: Vec<u64> = desktop
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Arriving {
                received,
                size,
                title,
                ..
            } => {
                assert_eq!((*size, title.as_str()), (data.len() as u64, "Interview, Aarhus"));
                Some(*received)
            }
            _ => None,
        })
        .collect();
    assert_eq!(seen, [0, 1 << 20, 2 << 20, 2_500_000]);
    assert!(path.starts_with(desktop.paths.audio()), "{}", path.display());
    assert_eq!(std::fs::read(&path).unwrap(), data);

    let store = desktop.store();
    let d = store.document(doc).unwrap();
    assert_eq!(d.title, "Interview, Aarhus");
    assert_eq!(d.source, Source::File);
    assert_eq!(d.project_id, Some(project));
    assert_eq!(d.template_id.as_deref(), Some("interview"));
    assert_eq!(d.created_at, 1_791_000_000_000);
    assert_eq!(d.audio_path.as_deref(), Some(path.as_path()));

    // Completing again (the reply was lost) gives the same document.
    let (status, body) = phone.complete(ID);
    assert_eq!(status, 200);
    assert_eq!(body["document_id"], doc);
    assert_eq!(store.documents(&Default::default()).unwrap().len(), 1);
    // Announcing again tells the phone it is all there.
    assert_eq!(phone.announce(ID, &data, json!({})).1["state"], "queued");
}

#[test]
fn an_upload_resumes_with_the_next_code() {
    let mut desktop = Desktop::start();
    let phone = desktop.scanned_phone();
    let data = audio(3_000_000);
    phone.announce(ID, &data, json!({}));
    phone.upload(ID, &data[..1_500_000], 0);

    desktop.restart();
    let phone2 = desktop.scanned_phone();
    let (_, body) = phone2.announce(ID, &data, json!({}));
    assert_eq!(body["received"], 1_500_000);
    phone2.upload(ID, &data, 1_500_000);
    assert_eq!(phone2.complete(ID).1["state"], "queued");
}

#[test]
fn a_damaged_upload_is_refused_and_starts_over() {
    let desktop = Desktop::start();
    let phone = desktop.scanned_phone();
    let data = audio(300_000);
    phone.announce(ID, &data, json!({}));
    let mut damaged = data.clone();
    damaged[1000] ^= 0xff;
    phone.upload(ID, &damaged, 0);
    let (status, body) = phone.complete(ID);
    assert_eq!(status, 422);
    assert_eq!(body["error"], "checksum");
    assert_eq!(body["received"], 0);
    assert!(desktop.store().documents(&Default::default()).unwrap().is_empty());

    phone.upload(ID, &data, 0);
    assert_eq!(phone.complete(ID).1["state"], "queued");
}

#[test]
fn bad_announcements_are_refused() {
    let desktop = Desktop::start();
    let phone = desktop.scanned_phone();
    let data = audio(1000);
    assert_eq!(
        phone.announce(ID, &data, json!({ "size": 5_000_000_000_u64 })).1["error"],
        "bad_size"
    );
    assert_eq!(
        phone.announce(ID, &data, json!({ "ext": "exe" })).1["error"],
        "bad_type"
    );
    assert_eq!(
        phone.announce(ID, &data, json!({ "sha256": "abc" })).1["error"],
        "bad_metadata"
    );
    // Ids name files; an escaped path is not an id.
    assert_eq!(phone.announce("..%2F..%2Fetc%2Fpasswd", &data, json!({})).0, 404);
    // Incomplete uploads cannot complete; more data than announced is refused.
    phone.announce(ID, &data, json!({}));
    assert_eq!(phone.complete(ID).1["error"], "incomplete");
    assert_eq!(phone.chunk(ID, 0, &audio(2000)).1["error"], "too_long");
}

#[test]
fn a_recording_for_a_deleted_project_goes_to_the_default_one() {
    let desktop = Desktop::start();
    let store = desktop.store();
    let gone = store.create_project("Gammel", "#000000").unwrap();
    store.delete_project(gone).unwrap();
    let inbox = store.create_project("Indbakke", "#111111").unwrap();
    desktop.receiver().set_defaults(Some(inbox), "notat");
    let phone = desktop.scanned_phone();
    let data = audio(1000);
    phone.announce(ID, &data, json!({ "project_id": gone }));
    phone.upload(ID, &data, 0);
    let doc = phone.complete(ID).1["document_id"].as_i64().unwrap();
    let d = store.document(doc).unwrap();
    assert_eq!(d.project_id, Some(inbox));
    assert_eq!(d.template_id.as_deref(), Some("notat"));
}

#[test]
fn done_ends_the_transfer() {
    let desktop = Desktop::start();
    let phone = desktop.scanned_phone();
    assert_eq!(phone.post_json("/v1/done", json!({})).0, 200);
    desktop.wait_for("finished", |e| *e == SyncEvent::Finished);
    let (status, body) = phone.announce(ID, &audio(10), json!({}));
    assert_eq!(status, 401);
    assert_eq!(body["error"], "no_session");
}

#[test]
fn nothing_listens_once_the_receiver_stops() {
    let mut desktop = Desktop::start();
    let phone = desktop.scanned_phone();
    desktop.receiver = None;
    let reached = phone.client.post(format!("{}/v1/done", phone.base)).send();
    assert!(reached.is_err(), "still listening");
}
