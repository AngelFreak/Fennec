//! Importing recordings from Fennec Recorder over USB, against a folder laid
//! out as a phone in File transfer mode shows it:
//! `<storage>/Download/Fennec Recorder/<id>.aac` and `<id>.json`.

use std::path::{Path, PathBuf};

use fennec::config::Paths;
use fennec::store::{InboundState, NewInbound, Store};
use fennec::sync::Target;
use fennec::sync::usb::{self, Sidecar};
use gtk::gio;

const FIXTURE: &str = include_str!("fixtures/usb/sidecar.json");

struct Phone {
    _dir: tempfile::TempDir,
    root: PathBuf,
    folder: PathBuf,
    paths: Paths,
}

fn phone() -> Phone {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("phone");
    let folder = root.join("Internal shared storage/Download/Fennec Recorder");
    std::fs::create_dir_all(&folder).unwrap();
    let paths = Paths::under(&dir.path().join("home"));
    Phone {
        _dir: dir,
        root,
        folder,
        paths,
    }
}

fn target(p: &Phone) -> Target {
    Target {
        incoming: p.paths.data_dir.join("sync/incoming"),
        audio: p.paths.audio(),
        default_project: None,
        default_template: "notat".into(),
    }
}

/// Puts a recording on the "phone" as the app does: audio, then the sidecar.
fn put(folder: &Path, json: &str, audio: &[u8]) -> Sidecar {
    let meta: Sidecar = serde_json::from_str(json).unwrap();
    std::fs::write(folder.join(format!("{}.{}", meta.id, meta.ext)), audio).unwrap();
    std::fs::write(folder.join(format!("{}.json", meta.id)), json).unwrap();
    meta
}

fn folder_of(p: &Phone) -> gio::File {
    usb::recorder_folder(&gio::File::for_path(&p.root)).expect("the app's folder is found under a storage")
}

#[test]
fn the_sidecar_the_app_writes_is_read() {
    let m: Sidecar = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(m.title, "Møde med Jensen");
    assert_eq!(
        (m.project_id, m.template_id.as_deref(), m.device_id),
        (Some(3), Some("moedereferat"), Some(7))
    );
}

#[test]
fn a_recording_is_imported_filed_and_removed_from_the_phone() {
    let p = phone();
    let store = Store::open(&p.paths.database()).unwrap();
    for name in ["a", "b", "c"] {
        store.create_project(name, "#000000").unwrap(); // the fixture's project 3 is "c"
    }
    let m = put(&p.folder, FIXTURE, b"1234");
    let waiting = usb::waiting(&folder_of(&p));
    assert_eq!(waiting.len(), 1);
    let arrived = usb::import(&store, &target(&p), &waiting[0])
        .unwrap()
        .expect("a new document");

    let d = store.document(arrived.document_id).unwrap();
    assert_eq!(d.title, "Møde med Jensen");
    assert_eq!(d.project_id, Some(3));
    assert_eq!(d.template_id.as_deref(), Some("moedereferat"));
    assert_eq!(d.created_at, m.recorded_at);
    assert_eq!(std::fs::read(&arrived.path).unwrap(), b"1234");
    assert!(arrived.path.starts_with(p.paths.audio()));
    let r = store.inbound(&m.id).unwrap().unwrap();
    assert_eq!(r.state, InboundState::Queued);
    assert_eq!(r.info.device_id, None, "no such pairing here");
    assert!(
        std::fs::read_dir(&p.folder).unwrap().next().is_none(),
        "both files are gone from the phone: that tells the app"
    );
}

#[test]
fn a_recording_fennec_already_has_is_only_removed_from_the_phone() {
    let p = phone();
    let store = Store::open(&p.paths.database()).unwrap();
    let m = put(&p.folder, FIXTURE, b"1234");
    store
        .add_inbound(&NewInbound {
            uuid: m.id.clone(),
            device_id: None,
            title: m.title.clone(),
            recorded_at: m.recorded_at,
            duration_ms: m.duration_ms,
            project_id: None,
            template_id: None,
            ext: "aac".into(),
            size: 4,
            sha256: m.sha256.clone(),
        })
        .unwrap();
    store
        .set_inbound_state(&m.id, InboundState::Done, None, None)
        .unwrap();
    let w = usb::waiting(&folder_of(&p));
    assert!(usb::known(&store, &w[0]));
    assert!(usb::import(&store, &target(&p), &w[0]).unwrap().is_none());
    assert!(
        store.documents(&Default::default()).unwrap().is_empty(),
        "no second document"
    );
    assert!(std::fs::read_dir(&p.folder).unwrap().next().is_none());
}

#[test]
fn unfinished_damaged_and_foreign_files_are_left_alone() {
    let p = phone();
    let store = Store::open(&p.paths.database()).unwrap();
    // Audio still being written: shorter than the sidecar says.
    put(&p.folder, FIXTURE, b"12");
    assert!(usb::waiting(&folder_of(&p)).is_empty());
    // Damaged on the way: right size, wrong bytes.
    put(&p.folder, FIXTURE, b"9999");
    let w = usb::waiting(&folder_of(&p));
    let err = usb::import(&store, &target(&p), &w[0]).unwrap_err();
    assert!(err.contains("did not copy correctly"), "{err}");
    assert_eq!(
        std::fs::read_dir(&p.folder).unwrap().count(),
        2,
        "kept on the phone to try again"
    );
    assert!(store.documents(&Default::default()).unwrap().is_empty());
    // Not a sidecar of ours.
    std::fs::write(p.folder.join("12345678-aaaa.json"), b"{\"hello\":1}").unwrap();
    std::fs::write(p.folder.join("notes.json"), FIXTURE).unwrap();
    assert_eq!(usb::waiting(&folder_of(&p)).len(), 1);
}

#[test]
fn a_phone_without_the_app_has_no_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Internal shared storage/Download")).unwrap();
    assert!(usb::recorder_folder(&gio::File::for_path(dir.path())).is_none());
    // Reached through a path straight to the storage.
    std::fs::create_dir_all(
        dir.path()
            .join("Internal shared storage/Download/Fennec Recorder"),
    )
    .unwrap();
    assert!(usb::recorder_folder(&gio::File::for_path(dir.path().join("Internal shared storage"))).is_some());
}

/// Against a real phone's folder, copied off it (android/e2e/run-usb.sh):
/// FENNEC_USB_PHONE names a folder with `Internal shared storage/Download/
/// Fennec Recorder/` inside. Every recording in it imports and checks out.
#[test]
fn recordings_from_a_real_phone_import() {
    let Some(root) = std::env::var_os("FENNEC_USB_PHONE") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let store = Store::open(&paths.database()).unwrap();
    let target = Target {
        incoming: paths.data_dir.join("sync/incoming"),
        audio: paths.audio(),
        default_project: None,
        default_template: "notat".into(),
    };
    let folder = usb::recorder_folder(&gio::File::for_path(&root)).expect("the app's folder");
    let waiting = usb::waiting(&folder);
    assert!(!waiting.is_empty(), "no complete recordings in the folder");
    for w in &waiting {
        let a = usb::import(&store, &target, w).unwrap().expect("a new document");
        let pcm = fennec::audio::decode::decode_file(&a.path).unwrap();
        let secs = pcm.len() as f64 / 16_000.0;
        assert!(
            (secs * 1000.0 - w.meta.duration_ms as f64).abs() < 1000.0,
            "{secs} s for {} ms",
            w.meta.duration_ms
        );
        println!("imported {} ({secs:.1} s)", w.meta.title);
    }
}
