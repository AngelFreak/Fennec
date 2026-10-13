//! Recordings from Fennec Recorder over a USB cable, in Android's File
//! transfer mode: the app keeps a copy of each recording not yet sent in
//! `Download/Fennec Recorder/` (`<id>.aac` and a `<id>.json` sidecar written
//! last), Fennec imports them, then deletes them from the phone. The app
//! sees its copy gone and marks the recording sent; that deletion is the
//! only way back to it, since Android shows apps only their own files.
//!
//! Files are reached through GIO, so a phone (an `mtp://` mount) and a
//! folder on disk (tests) work the same.

use std::io::Read;
use std::path::PathBuf;

use gtk::gio;
use gtk::gio::prelude::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::hex;
use super::inbound::{Arrived, Target, file_recording, valid_uuid};
use crate::store::{InboundState, NewInbound, Store};

/// Where the app keeps the copies, under a storage of the phone.
pub const FOLDER: [&str; 2] = ["Download", "Fennec Recorder"];

/// The `<id>.json` sidecar, as `UsbCopies.kt` writes it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Sidecar {
    pub fennec_recorder: u32,
    pub id: String,
    pub title: String,
    pub recorded_at: i64,
    pub duration_ms: i64,
    pub project_id: Option<i64>,
    pub template_id: Option<String>,
    pub ext: String,
    pub size: u64,
    pub sha256: String,
    /// The pairing the phone had when it recorded, if any.
    pub device_id: Option<i64>,
}

/// A recording waiting in the phone's folder.
#[derive(Debug, Clone)]
pub struct Waiting {
    pub meta: Sidecar,
    audio: gio::File,
    sidecar: gio::File,
}

/// The app's folder on a phone mounted at `root`: under one of its storages
/// (`Internal shared storage/Download/Fennec Recorder`) or right under the
/// root (a phone reached through a path, or a test).
pub fn recorder_folder(root: &gio::File) -> Option<gio::File> {
    let at = |base: &gio::File| {
        let f = FOLDER.iter().fold(base.clone(), |f, part| f.child(part));
        f.query_exists(gio::Cancellable::NONE).then_some(f)
    };
    if let Some(f) = at(root) {
        return Some(f);
    }
    let children = root
        .enumerate_children(
            "standard::name,standard::type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    children
        .filter_map(Result::ok)
        .filter(|i| i.file_type() == gio::FileType::Directory)
        .find_map(|i| at(&root.child(i.name())))
}

fn read_all(f: &gio::File) -> Result<Vec<u8>, String> {
    let stream = f.read(gio::Cancellable::NONE).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    stream
        .into_read()
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Complete recordings in the folder: a valid sidecar and audio of the size
/// it names. Others (still being written, damaged) are left alone.
pub fn waiting(folder: &gio::File) -> Vec<Waiting> {
    let Ok(children) = folder.enumerate_children(
        "standard::name,standard::size",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for info in children.filter_map(Result::ok) {
        let name = info.name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !valid_uuid(id) {
            continue;
        }
        let sidecar = folder.child(&name);
        let Some(meta) = read_all(&sidecar)
            .ok()
            .and_then(|b| serde_json::from_slice::<Sidecar>(&b).ok())
            .filter(|m| {
                m.id == id && m.fennec_recorder == 1 && m.ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
        else {
            tracing::warn!("{name} on the phone is not a Fennec Recorder sidecar");
            continue;
        };
        let audio = folder.child(format!("{id}.{}", meta.ext));
        let size = audio
            .query_info(
                "standard::size",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .map(|i| i.size() as u64);
        if size.ok() == Some(meta.size) {
            out.push(Waiting { meta, audio, sidecar });
        }
    }
    out.sort_by_key(|w| w.meta.recorded_at);
    out
}

/// Whether Fennec has this recording already (sent over Wi-Fi, or imported
/// before): then it only needs removing from the phone.
pub fn known(store: &Store, w: &Waiting) -> bool {
    store
        .inbound(&w.meta.id)
        .ok()
        .flatten()
        .is_some_and(|r| r.state != InboundState::Receiving)
}

/// Removes the copy from the phone, which tells the app Fennec has it.
pub fn remove_from_phone(w: &Waiting) -> Result<(), String> {
    // The audio first: that is what the app watches.
    w.audio
        .delete(gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?;
    let _ = w.sidecar.delete(gio::Cancellable::NONE);
    Ok(())
}

/// Imports one recording: copies it into Fennec's audio folder, checks it,
/// makes its document, then removes it from the phone. `None` when Fennec
/// already had it (it is only removed from the phone).
pub fn import(store: &Store, target: &Target, w: &Waiting) -> Result<Option<Arrived>, String> {
    if known(store, w) {
        remove_from_phone(w)?;
        return Ok(None);
    }
    let m = &w.meta;
    let path: PathBuf = target.audio.join(format!("phone-{}.{}", m.id, m.ext));
    std::fs::create_dir_all(&target.audio).map_err(|e| e.to_string())?;
    w.audio
        .copy(
            &gio::File::for_path(&path),
            gio::FileCopyFlags::OVERWRITE,
            gio::Cancellable::NONE,
            None,
        )
        .map_err(|e| e.to_string())?;
    let sum = std::fs::read(&path)
        .map(|b| hex(&Sha256::digest(&b)))
        .map_err(|e| e.to_string())?;
    if sum != m.sha256.to_ascii_lowercase() {
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "“{}” did not copy correctly from the phone. Try again.",
            m.title
        ));
    }
    let device = m
        .device_id
        .filter(|d| store.devices().is_ok_and(|ds| ds.iter().any(|x| x.id == *d)));
    let info = NewInbound {
        uuid: m.id.clone(),
        device_id: device,
        title: if m.title.trim().is_empty() {
            "Optagelse".into()
        } else {
            m.title.trim().chars().take(200).collect()
        },
        recorded_at: m.recorded_at,
        duration_ms: m.duration_ms.max(0),
        project_id: m.project_id,
        template_id: m.template_id.clone().filter(|t| !t.trim().is_empty()),
        ext: m.ext.clone(),
        size: m.size,
        sha256: sum,
    };
    // A half-finished upload over Wi-Fi is replaced by the whole file.
    if store.inbound(&m.id).map_err(|e| e.to_string())?.is_some() {
        store.delete_inbound(&m.id).map_err(|e| e.to_string())?;
    }
    store.add_inbound(&info).map_err(|e| e.to_string())?;
    let document_id = file_recording(store, target, &info, &path).map_err(|e| e.to_string())?;
    if let Err(e) = remove_from_phone(w) {
        // Imported all the same; the phone keeps showing it as waiting.
        tracing::warn!("could not remove {} from the phone: {e}", m.id);
    }
    Ok(Some(Arrived { document_id, path }))
}
