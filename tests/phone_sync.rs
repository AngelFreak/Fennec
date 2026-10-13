//! The phone receiver through its real HTTPS server on localhost, driven
//! the way Fennec Recorder drives it: pair, announce, upload in chunks,
//! resume, complete, ask for status, unpair.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fennec::config::Paths;
use fennec::store::{InboundState, Source, Store};
use fennec::sync::{Receiver, ReceiverConfig, SyncEvent, pairing_code};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Desktop {
    _dir: tempfile::TempDir,
    paths: Paths,
    receiver: Option<Receiver>,
    events: Arc<Mutex<Vec<SyncEvent>>>,
}

impl Desktop {
    fn start(pair_wait: Duration) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut d = Self {
            _dir: dir,
            paths,
            receiver: None,
            events: Arc::default(),
        };
        d.restart(pair_wait);
        d
    }

    /// Stops the receiver and starts a new one on the same data (Fennec
    /// closed and opened again).
    fn restart(&mut self, pair_wait: Duration) {
        self.receiver = None;
        let events = Arc::clone(&self.events);
        let receiver = Receiver::start(
            ReceiverConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                paths: self.paths.clone(),
                default_project: None,
                default_template: "notat".into(),
                advertise: false,
                pair_wait,
            },
            Arc::new(move |e| events.lock().unwrap().push(e)),
        )
        .unwrap();
        self.receiver = Some(receiver);
    }

    fn receiver(&self) -> &Receiver {
        self.receiver.as_ref().unwrap()
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
            secret: None,
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

    /// Pairs a phone the whole way: offer, token, code check, Allow.
    fn paired_phone(&self) -> Phone {
        let mut phone = self.phone();
        let offer = self.receiver().offer_pairing();
        let token = offer.token.clone();
        let p = phone.clone();
        let asking = std::thread::spawn(move || p.pair(&token, "Pixel 8", "nonce-0123456789abcdef"));
        let SyncEvent::PairRequest { request, .. } =
            self.wait_for("pair request", |e| matches!(e, SyncEvent::PairRequest { .. }))
        else {
            unreachable!()
        };
        self.receiver().decide(request, true);
        let (status, body) = asking.join().unwrap();
        assert_eq!(status, 200, "{body}");
        phone.secret = Some(body["secret"].as_str().unwrap().to_string());
        self.events.lock().unwrap().clear();
        phone
    }
}

#[derive(Clone)]
struct Phone {
    base: String,
    client: reqwest::blocking::Client,
    secret: Option<String>,
}

impl Phone {
    fn send(&self, method: reqwest::Method, path: &str, body: Option<Vec<u8>>) -> (u16, Value) {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(s) = &self.secret {
            req = req.bearer_auth(s);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req.send().unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().unwrap_or(Value::Null))
    }

    fn get(&self, path: &str) -> (u16, Value) {
        self.send(reqwest::Method::GET, path, None)
    }

    fn put_json(&self, path: &str, v: Value) -> (u16, Value) {
        self.send(reqwest::Method::PUT, path, Some(serde_json::to_vec(&v).unwrap()))
    }

    fn pair(&self, token: &str, name: &str, nonce: &str) -> (u16, Value) {
        let body = json!({ "token": token, "device_name": name, "nonce": nonce });
        self.send(
            reqwest::Method::POST,
            "/v1/pair",
            Some(serde_json::to_vec(&body).unwrap()),
        )
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
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.phone();
    let resp = phone
        .client
        .get(format!("{}/v1/info", phone.base))
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
    let offer = desktop.receiver().offer_pairing();
    assert_eq!(pin, desktop.receiver().pin());
    assert!(offer.uri().contains(&format!("pin={pin}&")), "{}", offer.uri());
}

#[test]
fn a_phone_pairs_when_allowed_and_both_sides_show_the_same_code() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.phone();
    assert_eq!(
        phone.pair("12345678", "Pixel 8", "nonce-0123456789abcdef").1["error"],
        "no_offer"
    );

    let offer = desktop.receiver().offer_pairing();
    let wrong = if offer.token == "00000000" {
        "11111111"
    } else {
        "00000000"
    };
    assert_eq!(
        phone.pair(wrong, "Pixel 8", "nonce-0123456789abcdef").1["error"],
        "wrong_token"
    );

    let nonce = "nonce-0123456789abcdef";
    let token = offer.token_display(); // typed with the space, as shown
    let p = phone.clone();
    let asking = std::thread::spawn(move || p.pair(&token, "  Pixel 8  ", nonce));
    let SyncEvent::PairRequest {
        request,
        device_name,
        code,
    } = desktop.wait_for("pair request", |e| matches!(e, SyncEvent::PairRequest { .. }))
    else {
        unreachable!()
    };
    assert_eq!(device_name, "Pixel 8");
    assert_eq!(
        code,
        pairing_code(&offer.pin, &offer.token, nonce),
        "the phone computes the same"
    );
    desktop.receiver().decide(request, true);
    let (status, body) = asking.join().unwrap();
    assert_eq!(status, 200, "{body}");
    let secret = body["secret"].as_str().unwrap();
    assert!(secret.len() >= 40);

    let devices = desktop.store().devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Pixel 8");
    desktop.wait_for("devices changed", |e| *e == SyncEvent::DevicesChanged);
    desktop.wait_for("pairing ended", |e| *e == SyncEvent::PairingEnded);

    // The offer is used up.
    assert_eq!(phone.pair(&offer.token, "Other", nonce).1["error"], "no_offer");
}

#[test]
fn denied_or_unanswered_pairing_saves_nothing() {
    let desktop = Desktop::start(Duration::from_millis(300));
    let phone = desktop.phone();
    let offer = desktop.receiver().offer_pairing();
    let p = phone.clone();
    let token = offer.token.clone();
    let asking = std::thread::spawn(move || p.pair(&token, "Pixel 8", "nonce-0123456789abcdef"));
    let SyncEvent::PairRequest { request, .. } =
        desktop.wait_for("pair request", |e| matches!(e, SyncEvent::PairRequest { .. }))
    else {
        unreachable!()
    };
    desktop.receiver().decide(request, false);
    assert_eq!(asking.join().unwrap().1["error"], "denied");

    let offer = desktop.receiver().offer_pairing();
    assert_eq!(
        phone.pair(&offer.token, "Pixel 8", "nonce-0123456789abcdef").1["error"],
        "timeout"
    );
    assert!(desktop.store().devices().unwrap().is_empty());
}

#[test]
fn wrong_codes_lock_the_offer() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.phone();
    let offer = desktop.receiver().offer_pairing();
    let wrong = if offer.token == "00000000" {
        "11111111"
    } else {
        "00000000"
    };
    for _ in 0..4 {
        assert_eq!(
            phone.pair(wrong, "x", "nonce-0123456789abcdef").1["error"],
            "wrong_token"
        );
    }
    assert_eq!(
        phone.pair(wrong, "x", "nonce-0123456789abcdef").1["error"],
        "locked"
    );
    assert_eq!(
        phone.pair(&offer.token, "x", "nonce-0123456789abcdef").1["error"],
        "no_offer"
    );
}

#[test]
fn only_paired_phones_get_in() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let mut stranger = desktop.phone();
    assert_eq!(stranger.get("/v1/info").0, 401);
    stranger.secret = Some("made-up".into());
    assert_eq!(stranger.get("/v1/info").0, 401);
    assert_eq!(stranger.announce(ID, &audio(10), json!({})).0, 401);
}

#[test]
fn info_lists_the_projects_a_recording_can_go_to() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let p = desktop.store().create_project("Kundemøder", "#2F6F4E").unwrap();
    let phone = desktop.paired_phone();
    let (status, info) = phone.get("/v1/info");
    assert_eq!(status, 200);
    assert_eq!(info["protocol"], 1);
    assert_eq!(
        info["projects"][0],
        json!({ "id": p, "name": "Kundemøder", "color": "#2F6F4E", "default_template": null, "documents": 0 })
    );
    assert_eq!(info["id"].as_str().unwrap(), &desktop.receiver().pin()[..12]);
}

#[test]
fn a_recording_arrives_in_chunks_and_becomes_a_queued_document() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let project = desktop.store().create_project("Kundemøder", "#2F6F4E").unwrap();
    let phone = desktop.paired_phone();
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

    let SyncEvent::Received { document_id, path } =
        desktop.wait_for("received", |e| matches!(e, SyncEvent::Received { .. }))
    else {
        unreachable!()
    };
    assert_eq!(document_id, doc);
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

    // The phone follows the Files queue.
    store
        .set_inbound_state_for_document(doc, InboundState::Done, None)
        .unwrap();
    let (_, body) = phone.get(&format!("/v1/recordings?ids={ID},not-a-known-id"));
    assert_eq!(body["recordings"][0]["state"], "done");
    assert_eq!(body["recordings"][0]["received"], data.len());
    assert_eq!(body["recordings"][1]["state"], "unknown");
    assert_eq!(desktop.store().devices().unwrap()[0].recordings, 1);
}

#[test]
fn an_upload_resumes_after_fennec_restarts() {
    let mut desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.paired_phone();
    let data = audio(3_000_000);
    phone.announce(ID, &data, json!({}));
    phone.upload(ID, &data[..1_500_000], 0);

    desktop.restart(Duration::from_secs(5));
    let mut phone2 = desktop.phone();
    phone2.secret = phone.secret.clone();
    let (_, body) = phone2.announce(ID, &data, json!({}));
    assert_eq!(body["received"], 1_500_000);
    phone2.upload(ID, &data, 1_500_000);
    assert_eq!(phone2.complete(ID).1["state"], "queued");
}

#[test]
fn a_damaged_upload_is_refused_and_starts_over() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.paired_phone();
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
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.paired_phone();
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
    let desktop = Desktop::start(Duration::from_secs(5));
    let store = desktop.store();
    let gone = store.create_project("Gammel", "#000000").unwrap();
    store.delete_project(gone).unwrap();
    let inbox = store.create_project("Indbakke", "#111111").unwrap();
    desktop.receiver().set_defaults(Some(inbox), "notat");
    let phone = desktop.paired_phone();
    let data = audio(1000);
    phone.announce(ID, &data, json!({ "project_id": gone }));
    phone.upload(ID, &data, 0);
    let doc = phone.complete(ID).1["document_id"].as_i64().unwrap();
    let d = store.document(doc).unwrap();
    assert_eq!(d.project_id, Some(inbox));
    assert_eq!(d.template_id.as_deref(), Some("notat"));
}

#[test]
fn an_unpaired_phone_is_shut_out() {
    let desktop = Desktop::start(Duration::from_secs(5));
    let phone = desktop.paired_phone();
    assert_eq!(phone.get("/v1/info").0, 200);
    assert_eq!(
        phone.send(reqwest::Method::DELETE, "/v1/devices/self", None).0,
        200
    );
    assert_eq!(phone.get("/v1/info").0, 401);
    assert!(desktop.store().devices().unwrap().is_empty());
}

#[test]
fn phones_can_find_fennec_by_name_on_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start(
        ReceiverConfig {
            bind: "0.0.0.0:0".parse().unwrap(),
            paths: Paths::under(dir.path()),
            default_project: None,
            default_template: "notat".into(),
            advertise: true,
            pair_wait: Duration::from_secs(5),
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let browser = mdns_sd::ServiceDaemon::new().unwrap();
    let events = browser.browse("_fennec._tcp.local.").unwrap();
    let id = &receiver.pin()[..12];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = None;
    while Instant::now() < deadline && found.is_none() {
        if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) =
            events.recv_timeout(Duration::from_millis(500))
            && info.get_property_val_str("id") == Some(id)
        {
            found = Some((
                info.get_port(),
                info.get_property_val_str("v").map(str::to_string),
            ));
        }
    }
    let _ = browser.shutdown();
    let (port, version) = found.expect("Fennec was not announced over mDNS");
    assert_eq!(port, receiver.local_addr().port());
    assert_eq!(version.as_deref(), Some("1"));
}

#[test]
fn a_phone_adds_projects_and_changes_their_name_colour_and_default_template() {
    let desktop = Desktop::start(Duration::from_secs(5));
    fennec::template::install_defaults(&desktop.paths.templates()).unwrap();
    let phone = desktop.paired_phone();

    let (_, info) = phone.get("/v1/info");
    let interview = info["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "moedereferat")
        .expect("the built-in templates are listed");
    assert!(
        !interview["fields"].as_array().unwrap().is_empty(),
        "with their fields"
    );
    assert_eq!(info["project_colors"].as_array().unwrap().len(), 5);
    assert_eq!(info["default_template"], "notat");

    let post = |v: Value| {
        phone.send(
            reqwest::Method::POST,
            "/v1/projects",
            Some(serde_json::to_vec(&v).unwrap()),
        )
    };
    let (status, p) = post(json!({ "name": "  Fra telefonen ", "default_template": "moedereferat" }));
    assert_eq!(status, 200, "{p}");
    assert_eq!(p["name"], "Fra telefonen");
    assert_eq!(p["color"], "#C2410C", "the next colour in the sidebar's order");
    assert_eq!(p["default_template"], "moedereferat");
    desktop.wait_for("projects changed", |e| *e == SyncEvent::ProjectsChanged);

    assert_eq!(post(json!({ "name": "fra TELEFONEN" })).1["error"], "exists");
    assert_eq!(post(json!({ "name": "   " })).1["error"], "bad_name");
    assert_eq!(
        post(json!({ "name": "X", "color": "red" })).1["error"],
        "bad_color"
    );
    assert_eq!(
        post(json!({ "name": "X", "default_template": "nope" })).1["error"],
        "bad_template"
    );

    let id = p["id"].as_i64().unwrap();
    let (status, p) = phone.put_json(
        &format!("/v1/projects/{id}"),
        json!({ "name": "Kundemøder", "color": "#0F766E" }),
    );
    assert_eq!(status, 200, "{p}");
    assert_eq!(p["default_template"], "moedereferat", "fields left out stay");
    let store = desktop.store();
    let saved = store
        .projects()
        .unwrap()
        .into_iter()
        .find(|x| x.id == id)
        .unwrap();
    assert_eq!(
        (saved.name.as_str(), saved.color.as_str()),
        ("Kundemøder", "#0F766E")
    );

    // A recording into it without a template of its own gets the project's.
    let data = audio(1000);
    phone.announce(ID, &data, json!({ "project_id": id }));
    phone.upload(ID, &data, 0);
    let doc = phone.complete(ID).1["document_id"].as_i64().unwrap();
    assert_eq!(
        store.document(doc).unwrap().template_id.as_deref(),
        Some("moedereferat")
    );

    let (_, p) = phone.put_json(&format!("/v1/projects/{id}"), json!({ "default_template": null }));
    assert!(p["default_template"].is_null(), "null clears it");
    assert_eq!(
        phone.put_json("/v1/projects/9999", json!({ "name": "x" })).1["error"],
        "unknown_project"
    );
}
