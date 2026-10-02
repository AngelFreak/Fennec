//! Drives the real whisper.cpp engine on a Danish FLEURS clip.
//! Needs the tiny model: `python scripts/fetch_models.py tiny`.

use std::path::{Path, PathBuf};

use fennec::audio::read_wav_16k_mono;
use fennec::engine::{EngineError, SAMPLE_RATE, TranscribeOptions, Transcriber, WhisperEngine};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tiny_model() -> PathBuf {
    let p = fixture("models/ggml-tiny.bin");
    assert!(
        p.exists(),
        "missing {}; run `python scripts/fetch_models.py tiny`",
        p.display()
    );
    p
}

#[test]
fn transcribes_danish_clip_into_ordered_segments_within_the_audio() {
    let pcm = read_wav_16k_mono(&fixture("da_fleurs_0.wav")).unwrap();
    let duration_ms = pcm.len() as i64 * 1000 / SAMPLE_RATE as i64;
    let mut engine = WhisperEngine::load(&tiny_model(), false).unwrap();

    let segments = engine.transcribe(&pcm, &TranscribeOptions::default()).unwrap();

    assert!(!segments.is_empty(), "no segments");
    let text: String = segments.iter().map(|s| s.text.as_str()).collect();
    assert!(
        text.chars().filter(|c| c.is_alphabetic()).count() > 10,
        "too little text: {text:?}"
    );
    for pair in segments.windows(2) {
        assert!(
            pair[0].start_ms <= pair[1].start_ms,
            "segments out of order: {segments:?}"
        );
    }
    for s in &segments {
        assert!(
            s.start_ms <= s.end_ms && s.end_ms <= duration_ms + 1000,
            "bad timing: {s:?}"
        );
        for r in &s.low_confidence {
            assert!(
                s.text.is_char_boundary(r.start) && s.text.is_char_boundary(r.end),
                "{s:?}"
            );
        }
    }
}

#[test]
fn missing_model_reports_the_path() {
    let err = WhisperEngine::load(Path::new("/nonexistent/model.bin"), false)
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)));
    assert!(err.to_string().contains("/nonexistent/model.bin"));
}
