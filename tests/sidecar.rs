//! Models run by the Python helper (hviske-v6): the bundled helper script
//! against a stand-in model module, then the same engine through file
//! ingest and live dictation. The real model runs behind `#[ignore]`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use fennec::audio::capture::PcmSource;
use fennec::engine::sidecar::is_sidecar_model;
use fennec::engine::{EngineError, SidecarEngine, TranscribeOptions, Transcriber};
use fennec::ingest::{IngestOptions, Recognizer, ingest_file};
use fennec::live::{LiveConfig, LiveEvent, LiveSession};
use fennec::store::{NewDocument, Store};
use fennec::utterance::EnergyVad;
use fennec::vad::WholeAudio;
use fennec::worker::EngineWorker;

const SR: usize = 16_000;

/// A model directory whose `HviskeASR` says "Hej med dig." for sound and
/// nothing for silence, like the real one's interface.
fn fake_model(dir: &Path, load: &str) -> PathBuf {
    let model = dir.join("hviske-v6");
    std::fs::create_dir_all(&model).unwrap();
    std::fs::write(
        model.join("processing_whisper_qwen.py"),
        format!(
            r#"
import numpy as np
class HviskeASR:
    @classmethod
    def from_pretrained(cls, path, device=None, dtype=None, **kw):
        {load}
        return cls()
    def transcribe_batch(self, waves, cased=None, punctuated=None, **kw):
        assert all(len(w) <= 30 * 16000 for w in waves), "over 30 s"
        return ["Hej med dig." if np.abs(w).max() > 0.01 else "" for w in waves]
"#
        ),
    )
    .unwrap();
    model
}

fn tone(secs: f32) -> Vec<f32> {
    (0..(secs * SR as f32) as usize)
        .map(|i| (i as f32 * 0.07).sin() * 0.3)
        .collect()
}

fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * SR as f32) as usize]
}

fn engine(dir: &Path) -> SidecarEngine {
    SidecarEngine::hviske("python3", &fake_model(dir, "pass")).unwrap()
}

#[test]
fn hviske_model_directories_are_recognised() {
    let dir = tempfile::tempdir().unwrap();
    assert!(is_sidecar_model(&fake_model(dir.path(), "pass")));
    assert!(!is_sidecar_model(dir.path()));
}

#[test]
fn a_model_that_prompts_while_loading_cannot_take_the_request_pipe() {
    // transformers asks "[y/N]" about custom code on stdout and reads the
    // answer from stdin; that stdin is Fennec's request pipe.
    let dir = tempfile::tempdir().unwrap();
    let load = r#"print("Do you wish to run the custom code? [y/N] ", flush=True)
        try:
            input()
        except EOFError:
            pass"#;
    let model = fake_model(dir.path(), load);
    let mut e = SidecarEngine::hviske("python3", &model).unwrap();
    let text: String = e
        .transcribe(&tone(2.0), &TranscribeOptions::default())
        .unwrap()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(text.trim(), "Hej med dig.");
}

#[test]
fn the_helper_transcribes_and_gives_one_segment_per_piece() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = engine(dir.path());
    assert_eq!(e.device, "cpu");
    let segs = e.transcribe(&tone(2.0), &TranscribeOptions::default()).unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "Hej med dig.");
    assert_eq!((segs[0].start_ms, segs[0].end_ms), (0, 2_000));
    assert!(segs[0].low_confidence.is_empty());
    assert!(
        e.transcribe(&silence(1.0), &TranscribeOptions::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn long_audio_is_sent_in_pieces_of_at_most_28_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = engine(dir.path());
    let segs = e.transcribe(&tone(40.0), &TranscribeOptions::default()).unwrap();
    let spans: Vec<(i64, i64)> = segs.iter().map(|s| (s.start_ms, s.end_ms)).collect();
    assert_eq!(spans, [(0, 28_000), (28_000, 40_000)]);
}

#[test]
fn a_model_that_fails_to_load_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let model = fake_model(dir.path(), "raise RuntimeError('no weights in folder')");
    let err = SidecarEngine::hviske("python3", &model).err().unwrap();
    let msg = err.to_string();
    assert!(
        msg.contains("no weights in folder") && msg.contains("hviske-v6"),
        "{msg}"
    );
}

#[test]
fn a_missing_model_directory_is_reported() {
    let err = SidecarEngine::hviske("python3", Path::new("/nowhere/hviske-v6"))
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)), "{err}");
}

#[test]
fn a_helper_that_dies_mid_request_is_an_error_not_a_hang() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dies.py");
    std::fs::write(
        &script,
        "import sys, json\nprint('loading weights...')\nprint(json.dumps({'ready': True, 'device': 'cpu'}), flush=True)\nsys.stdin.buffer.readline()\nsys.exit(3)\n",
    )
    .unwrap();
    let mut cmd = Command::new("python3");
    cmd.arg(&script);
    let mut e = SidecarEngine::spawn(cmd, Path::new("fake")).unwrap();
    let err = e
        .transcribe(&tone(0.5), &TranscribeOptions::default())
        .unwrap_err();
    assert!(err.to_string().contains("stopped"), "{err}");
}

#[test]
fn file_ingest_runs_through_the_helper_with_vad_chunk_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("to_ytringer.wav");
    let mut audio = tone(3.0);
    audio.extend(silence(3.0));
    audio.extend(tone(2.0));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for s in &audio {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
    let store = Store::open(&dir.path().join("f.db")).unwrap();
    let doc = store.create_document(&NewDocument::file("Fil")).unwrap();
    let mut e = engine(dir.path());

    ingest_file(
        &wav,
        doc,
        &store,
        Recognizer {
            engine: &mut e,
            vad: &mut WholeAudio,
            punctuator: None,
        },
        &IngestOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    let paragraphs = store.paragraphs(doc).unwrap();
    assert!(!paragraphs.is_empty());
    assert!(paragraphs.iter().all(|p| p.text.contains("Hej med dig.")));
    assert_eq!(paragraphs[0].start_ms, Some(0));
    assert!(paragraphs.last().unwrap().end_ms.unwrap() <= 8_000);
}

#[test]
fn live_dictation_runs_through_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let mut audio = tone(1.5);
    audio.extend(silence(1.5));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let worker = Arc::new(EngineWorker::spawn(Box::new(engine(dir.path()))));
    LiveSession::start(
        Box::new(PcmSource::new(audio)),
        Box::new(EnergyVad::default()),
        worker,
        LiveConfig::default(),
        Arc::new(move |e| sink.lock().unwrap().push(e)),
    )
    .wait();
    let finals: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(finals, ["Hej med dig."]);
}

#[test]
fn the_app_engine_factory_runs_a_hviske_model_folder_through_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let paths = fennec::config::Paths::under(dir.path());
    std::fs::create_dir_all(paths.models()).unwrap();
    std::fs::rename(fake_model(dir.path(), "pass"), paths.models().join("hviske-v6")).unwrap();
    let settings = fennec::config::Settings {
        model: "hviske-v6".into(),
        ..Default::default()
    };
    let deps = fennec::ui::Deps::real(paths.clone(), settings.clone());
    let mut engine = (deps.engine)(&settings, &paths).unwrap();
    let segs = engine
        .transcribe(&tone(1.0), &TranscribeOptions::default())
        .unwrap();
    assert_eq!(segs[0].text, "Hej med dig.");
}

/// The real hviske-v6 (2.8 GB) on the FLEURS clip:
/// `python scripts/fetch_models.py hviske6` puts it in the models folder;
/// FENNEC_PYTHON must have torch and transformers.
#[test]
#[ignore]
fn real_hviske_v6_transcribes_danish() {
    let model = fennec::config::Paths::user().models().join("hviske-v6");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let pcm = fennec::audio::read_wav_16k_mono(&fixture.join("da_fleurs_0.wav")).unwrap();
    let reference = std::fs::read_to_string(fixture.join("da_fleurs_0.txt")).unwrap();
    let mut e = SidecarEngine::hviske(&fennec::models::python(), &model).unwrap();
    let text: String = e
        .transcribe(&pcm, &TranscribeOptions::default())
        .unwrap()
        .into_iter()
        .map(|s| s.text)
        .collect();
    let wer = fennec::eval::word_errors(&reference, &text).rate();
    assert!(wer < 0.25, "WER {wer:.2}: {text}");
}
