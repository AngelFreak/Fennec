//! The desktop half of the phone end-to-end test (android/e2e/run-e2e.sh):
//! the real Fennec window with receiving on, a pairing offer open and
//! someone pressing Allow, so Fennec Recorder in the emulator can pair, send
//! recordings and see them transcribed. The speech engine is a stand-in that
//! answers every stretch of audio with one Danish sentence; the decoding of
//! the phone's audio is real.
//!
//! Runs only when FENNEC_PHONE_E2E_DIR names a folder. Writes there:
//!   pair-uri      the QR code's text, with the address the emulator uses (10.0.2.2)
//!   results.json  every recording that arrived, rewritten as they change
//!   *.png         the window when the phone asks to pair and when recordings are in
//! and stops when a file named `stop` appears there (or after 15 minutes).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fennec::config::{Paths, Settings};
use fennec::engine::{EngineError, Segment, TranscribeOptions, Transcriber};
use fennec::store::Store;
use fennec::ui::{self, Deps};
use fennec::vad::{SpeechDetector, WholeAudio};
use gtk::glib;
use gtk::prelude::*;

/// Samples the engine was given, in all.
static HEARD: AtomicUsize = AtomicUsize::new(0);

struct Sentence;

impl Transcriber for Sentence {
    fn transcribe(&mut self, pcm: &[f32], _: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        HEARD.fetch_add(pcm.len(), Ordering::Relaxed);
        Ok(vec![Segment {
            text: "Hej fra telefonen.".into(),
            start_ms: 0,
            end_ms: (pcm.len() as i64) * 1000 / 16_000,
            low_confidence: Vec::new(),
        }])
    }
}

fn pump(d: Duration) {
    let end = Instant::now() + d;
    let ctx = glib::MainContext::default();
    while Instant::now() < end {
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn screenshot(widget: &impl IsA<gtk::Widget>, path: &Path) {
    let widget = widget.as_ref();
    pump(Duration::from_millis(400));
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, widget.width() as f64, widget.height() as f64);
    if let (Some(node), Some(renderer)) = (snapshot.to_node(), widget.native().and_then(|n| n.renderer())) {
        renderer.render_texture(&node, None).save_to_png(path).unwrap();
    }
}

fn deps(root: &Path, port: u16) -> Deps {
    let mut d = Deps::real(Paths::under(root), Settings::default());
    {
        let mut s = d.settings.borrow_mut();
        s.phone.enabled = true;
        s.phone.port = port;
        s.punctuate = false;
    }
    let models = Paths::under(root).models();
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join(&Settings::default().model), b"stand-in").unwrap();
    std::fs::write(models.join(fennec::models::VAD_FILE), b"stand-in").unwrap();
    d.engine = Arc::new(|_, _| Ok(Box::new(Sentence) as Box<dyn Transcriber>));
    d.file_vad = Arc::new(|_, _| Box::new(WholeAudio) as Box<dyn SpeechDetector>);
    d.secrets = Arc::new(fennec::ai::MemorySecrets::default());
    d.confirm_cloud = std::rc::Rc::new(|_, _, answer| answer(true));
    // 127.0.0.1 only, no mDNS: the emulator reaches it as 10.0.2.2.
    d.phone_local_only = true;
    d
}

fn results(store: &Store, root: &Path) -> serde_json::Value {
    let db = rusqlite::Connection::open(Paths::under(root).database()).unwrap();
    let mut stmt = db
        .prepare("SELECT uuid FROM inbound_recordings ORDER BY updated_at")
        .unwrap();
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = Vec::new();
    for id in ids {
        let Some(r) = store.inbound(&id).unwrap() else {
            continue;
        };
        let doc = r.document_id.and_then(|d| store.document(d).ok());
        let audio = doc.as_ref().and_then(|d| d.audio_path.clone());
        let decoded_ms = audio
            .as_ref()
            .and_then(|p| fennec::audio::decode::decode_file(p).ok())
            .map(|pcm| pcm.len() as i64 * 1000 / 16_000);
        let paragraphs: Vec<String> = r
            .document_id
            .and_then(|d| store.paragraphs(d).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.text)
            .collect();
        out.push(serde_json::json!({
            "id": id,
            "state": r.state.as_str(),
            "error": r.error,
            "title": doc.as_ref().map(|d| d.title.clone()),
            "project_id": doc.as_ref().and_then(|d| d.project_id),
            "template_id": doc.as_ref().and_then(|d| d.template_id.clone()),
            "created_at": doc.as_ref().map(|d| d.created_at),
            "recorded_at": r.info.recorded_at,
            "ext": r.info.ext,
            "size": r.info.size,
            "duration_ms": r.info.duration_ms,
            "decoded_ms": decoded_ms,
            "audio": audio.map(|p| p.display().to_string()),
            "paragraphs": paragraphs,
        }));
    }
    serde_json::json!({ "recordings": out, "devices": store.devices().unwrap().len(), "samples_heard": HEARD.load(Ordering::Relaxed) })
}

fn main() {
    let Some(out) = std::env::var_os("FENNEC_PHONE_E2E_DIR").map(PathBuf::from) else {
        println!("phone_e2e: skipped (set FENNEC_PHONE_E2E_DIR; android/e2e/run-e2e.sh does)");
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let _ = std::fs::remove_file(out.join("stop"));
    let root = out.join("home");
    let _ = std::fs::remove_dir_all(&root);
    let port: u16 = std::env::var("FENNEC_PHONE_E2E_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(47131);

    adw::init().expect("a display is available");
    ui::load_css();
    let store = Store::open(&Paths::under(&root).database()).unwrap();
    let project = store.create_project("Kundemøder", "#2F6F4E").unwrap();
    println!("project {project}");
    let w = ui::build_window(deps(&root, port));
    w.window.present();
    pump(Duration::from_millis(500));
    w.sidebar.go(ui::Nav::Settings);
    w.settings.show_section("phone");
    w.settings.phone.start_pairing();
    let Some(offer) = w.phone.offer() else {
        panic!("receiving did not start: {:?}", w.phone.state());
    };
    let uri = offer.uri().replace(&offer.address, &format!("10.0.2.2:{port}"));
    std::fs::write(out.join("pair-uri"), &uri).unwrap();
    println!("pair-uri {uri}");
    screenshot(&w.window, &out.join("desktop-pairing.png"));

    let start = Instant::now();
    let mut last = serde_json::Value::Null;
    let mut shot_files = false;
    while start.elapsed() < Duration::from_secs(900) && !out.join("stop").exists() {
        pump(Duration::from_millis(200));
        if let Some(p) = w.phone.pending() {
            println!("pair request from {} with code {}", p.device_name, p.code);
            std::fs::write(out.join("pair-code"), &p.code).unwrap();
            screenshot(&w.window, &out.join("desktop-allow.png"));
            w.phone.decide(true);
        }
        let now = results(&store, &root);
        if now != last {
            std::fs::write(
                out.join("results.json"),
                serde_json::to_string_pretty(&now).unwrap(),
            )
            .unwrap();
            println!("{}", serde_json::to_string(&now["recordings"]).unwrap());
            let all_done = now["recordings"]
                .as_array()
                .is_some_and(|r| !r.is_empty() && r.iter().all(|x| x["state"] == "done"));
            if all_done && !shot_files {
                w.sidebar.go(ui::Nav::Files);
                screenshot(&w.window, &out.join("desktop-files.png"));
                w.sidebar.go(ui::Nav::Settings);
                screenshot(&w.window, &out.join("desktop-phone-settings.png"));
                shot_files = true;
            }
            last = now;
        }
    }
    println!("phone_e2e: stopped");
}
