//! Recordings arriving from a phone: metadata first, then the file in
//! chunks that can resume after a dropped connection, then a checksum check
//! that turns it into a document.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::hex;
use super::http::Response;
use crate::store::{DeviceId, DocumentId, InboundState, NewDocument, NewInbound, Store};

/// Largest recording accepted (about 190 hours at the phone's 48 kbit/s).
pub const MAX_SIZE: u64 = 4 * 1024 * 1024 * 1024;
/// Extensions Fennec's decoder reads.
const EXTENSIONS: [&str; 8] = ["m4a", "aac", "mp3", "ogg", "opus", "wav", "flac", "webm"];

/// Where uploads go and what a recording without a project gets.
#[derive(Clone)]
pub struct Target {
    /// Partial uploads.
    pub incoming: PathBuf,
    /// Finished recordings (Fennec's audio folder).
    pub audio: PathBuf,
    pub default_project: Option<i64>,
    pub default_template: String,
}

#[derive(Deserialize)]
struct Meta {
    title: String,
    recorded_at: i64,
    duration_ms: i64,
    project_id: Option<i64>,
    template_id: Option<String>,
    ext: Option<String>,
    size: u64,
    sha256: String,
}

/// Recording ids are made by the phone; they also name files here.
pub fn valid_uuid(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn part_path(target: &Target, uuid: &str) -> PathBuf {
    target.incoming.join(format!("{uuid}.part"))
}

fn on_disk(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("phone upload: {e}");
    Response::error(500, "internal", "Fennec could not store the recording.")
}

fn state_body(
    uuid: &str,
    received: u64,
    state: InboundState,
    doc: Option<DocumentId>,
    error: Option<&str>,
) -> serde_json::Value {
    json!({
        "id": uuid,
        "received": received,
        "state": state.as_str(),
        "document_id": doc,
        "error": error,
    })
}

/// `PUT /v1/recordings/{uuid}`: announces a recording. Repeating it is
/// harmless and tells the phone how much has arrived.
pub fn announce(store: &Store, target: &Target, device: DeviceId, uuid: &str, body: &[u8]) -> Response {
    let meta: Meta = match serde_json::from_slice(body) {
        Ok(m) => m,
        Err(e) => return Response::error(400, "bad_metadata", &e.to_string()),
    };
    let sha256 = meta.sha256.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Response::error(400, "bad_metadata", "sha256 must be 64 hex digits");
    }
    if meta.size == 0 || meta.size > MAX_SIZE {
        return Response::error(400, "bad_size", "The recording is empty or larger than 4 GB.");
    }
    let ext = meta.ext.as_deref().unwrap_or("m4a").to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Response::error(400, "bad_type", "Fennec cannot read this kind of audio file.");
    }
    let title: String = meta.title.trim().chars().take(200).collect();
    let info = NewInbound {
        uuid: uuid.to_string(),
        device_id: device,
        title: if title.is_empty() {
            "Optagelse".into()
        } else {
            title
        },
        recorded_at: meta.recorded_at,
        duration_ms: meta.duration_ms.max(0),
        project_id: meta.project_id,
        template_id: meta.template_id.filter(|t| !t.trim().is_empty()),
        ext,
        size: meta.size,
        sha256,
    };

    match store.inbound(uuid) {
        Err(e) => internal(e),
        Ok(Some(existing)) if existing.info.device_id != device => {
            Response::error(409, "taken", "Another phone sent a recording with this id.")
        }
        Ok(Some(existing))
            if existing.state != InboundState::Receiving
                || (existing.info.size == info.size && existing.info.sha256 == info.sha256) =>
        {
            let received = if existing.state == InboundState::Receiving {
                on_disk(&part_path(target, uuid))
            } else {
                existing.info.size
            };
            Response::ok(state_body(
                uuid,
                received,
                existing.state,
                existing.document_id,
                existing.error.as_deref(),
            ))
        }
        Ok(existing) => {
            // New, or a different file under the same id while still arriving: start over.
            if existing.is_some() {
                let _ = std::fs::remove_file(part_path(target, uuid));
                if let Err(e) = store.delete_inbound(uuid) {
                    return internal(e);
                }
            }
            match store.add_inbound(&info) {
                Ok(()) => Response::ok(state_body(uuid, 0, InboundState::Receiving, None, None)),
                Err(e) => internal(e),
            }
        }
    }
}

/// `PUT /v1/recordings/{uuid}/audio?offset=N`: one chunk. It must start where
/// the stored part ends; otherwise 409 says where to resume.
pub fn chunk(
    store: &Store,
    target: &Target,
    device: DeviceId,
    uuid: &str,
    offset: Option<u64>,
    body: &[u8],
) -> Response {
    let rec = match store.inbound(uuid) {
        Ok(Some(r)) if r.info.device_id == device => r,
        Ok(_) => return Response::error(404, "unknown", "Announce the recording first."),
        Err(e) => return internal(e),
    };
    if rec.state != InboundState::Receiving {
        return Response::error(409, "complete", "The recording has already arrived.");
    }
    let Some(offset) = offset else {
        return Response::error(400, "no_offset", "Say where the chunk starts with ?offset=");
    };
    let part = part_path(target, uuid);
    let have = on_disk(&part);
    if offset != have {
        let mut r = Response::error(409, "offset", &format!("Resend from byte {have}."));
        r.body["received"] = json!(have);
        return r;
    }
    if have + body.len() as u64 > rec.info.size {
        return Response::error(400, "too_long", "More data than the announced size.");
    }
    let written = std::fs::create_dir_all(&target.incoming).and_then(|()| {
        let mut f = OpenOptions::new().create(true).append(true).open(&part)?;
        f.write_all(body)?;
        f.sync_data()
    });
    if let Err(e) = written {
        return internal(e);
    }
    let received = have + body.len() as u64;
    if let Err(e) = store.set_inbound_received(uuid, received) {
        return internal(e);
    }
    Response::ok(json!({ "id": uuid, "received": received }))
}

/// A finished recording, ready for the Files queue.
pub struct Arrived {
    pub document_id: DocumentId,
    pub path: PathBuf,
}

/// `POST /v1/recordings/{uuid}/complete`: checks the file and makes its
/// document. Repeating it returns the same document.
pub fn complete(store: &Store, target: &Target, device: DeviceId, uuid: &str) -> (Response, Option<Arrived>) {
    let rec = match store.inbound(uuid) {
        Ok(Some(r)) if r.info.device_id == device => r,
        Ok(_) => {
            return (
                Response::error(404, "unknown", "Announce the recording first."),
                None,
            );
        }
        Err(e) => return (internal(e), None),
    };
    if rec.state != InboundState::Receiving {
        let body = state_body(
            uuid,
            rec.info.size,
            rec.state,
            rec.document_id,
            rec.error.as_deref(),
        );
        return (Response::ok(body), None);
    }
    let part = part_path(target, uuid);
    let have = on_disk(&part);
    if have != rec.info.size {
        let mut r = Response::error(
            409,
            "incomplete",
            &format!("Only {have} of {} bytes arrived.", rec.info.size),
        );
        r.body["received"] = json!(have);
        return (r, None);
    }
    match file_sha256(&part) {
        Ok(sum) if sum == rec.info.sha256 => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&part);
            let _ = store.set_inbound_received(uuid, 0);
            let mut r = Response::error(
                422,
                "checksum",
                "The file arrived damaged. Send it again from the start.",
            );
            r.body["received"] = json!(0);
            return (r, None);
        }
        Err(e) => return (internal(e), None),
    }

    let path = target.audio.join(format!("phone-{uuid}.{}", rec.info.ext));
    if let Err(e) = std::fs::create_dir_all(&target.audio).and_then(|()| std::fs::rename(&part, &path)) {
        return (internal(e), None);
    }
    let projects = store.projects().unwrap_or_default();
    let exists = |p: Option<i64>| p.filter(|id| projects.iter().any(|x| x.id == *id));
    let project_id = exists(rec.info.project_id).or_else(|| exists(target.default_project));
    // The phone's choice, else the project's default, else Fennec's (as in dictation).
    let template = rec
        .info
        .template_id
        .clone()
        .or_else(|| {
            projects
                .iter()
                .find(|p| Some(p.id) == project_id)?
                .default_template
                .clone()
        })
        .unwrap_or_else(|| target.default_template.clone());
    let doc = store
        .create_document(&NewDocument {
            project_id,
            template_id: Some(template),
            ..NewDocument::file(&rec.info.title)
        })
        .and_then(|doc| {
            store.set_document_created_at(doc, rec.info.recorded_at)?;
            store.set_audio_path(doc, Some(&path))?;
            store.set_inbound_received(uuid, rec.info.size)?;
            store.set_inbound_state(uuid, InboundState::Queued, Some(doc), None)?;
            Ok(doc)
        });
    match doc {
        Ok(document_id) => (
            Response::ok(state_body(
                uuid,
                rec.info.size,
                InboundState::Queued,
                Some(document_id),
                None,
            )),
            Some(Arrived { document_id, path }),
        ),
        Err(e) => (internal(e), None),
    }
}

/// `GET /v1/recordings?ids=a,b`: where each recording is.
pub fn statuses(store: &Store, target: &Target, device: DeviceId, ids: &str) -> Response {
    let mut out = Vec::new();
    for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()).take(200) {
        match store.inbound(id) {
            Ok(Some(r)) if r.info.device_id == device => {
                let received = if r.state == InboundState::Receiving {
                    on_disk(&part_path(target, id))
                } else {
                    r.info.size
                };
                out.push(state_body(
                    id,
                    received,
                    r.state,
                    r.document_id,
                    r.error.as_deref(),
                ));
            }
            Ok(_) => out.push(json!({ "id": id, "state": "unknown" })),
            Err(e) => return internal(e),
        }
    }
    Response::ok(json!({ "recordings": out }))
}

/// Deletes uploads that stopped arriving more than `max_age_ms` ago.
pub fn sweep_stale(store: &Store, target: &Target, now_ms: i64, max_age_ms: i64) -> usize {
    let Ok(stale) = store.stale_inbound(now_ms - max_age_ms) else {
        return 0;
    };
    for uuid in &stale {
        let _ = std::fs::remove_file(part_path(target, uuid));
        if let Err(e) = store.delete_inbound(uuid) {
            tracing::warn!("removing unfinished upload {uuid}: {e}");
        }
    }
    stale.len()
}

fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}
