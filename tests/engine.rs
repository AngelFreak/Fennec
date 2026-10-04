//! Drives the real whisper.cpp engine on a Danish FLEURS clip.
//! Needs the tiny model: `python scripts/fetch_models.py tiny`.

use std::path::{Path, PathBuf};

use fennec::audio::read_wav_16k_mono;
use fennec::engine::{EngineError, SAMPLE_RATE, TranscribeOptions, Transcriber, WhisperEngine, load_engine};

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
            0 <= s.start_ms && s.start_ms <= s.end_ms && s.end_ms <= duration_ms,
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

/// The released package is the Vulkan build, also for computers without a
/// GPU. Asked for the GPU there, the engine must still transcribe: whisper.cpp
/// uses the CPU when it finds no Vulkan device, and the app retries on the
/// CPU when loading fails (as `Deps::real` does).
#[test]
fn asking_for_the_gpu_without_one_still_transcribes() {
    let pcm = read_wav_16k_mono(&fixture("da_fleurs_0.wav")).unwrap();
    let mut engine = load_engine(&tiny_model(), true)
        .or_else(|e| {
            eprintln!("GPU engine did not load ({e}); retrying on the CPU");
            load_engine(&tiny_model(), false)
        })
        .unwrap();
    let segments = engine.transcribe(&pcm, &TranscribeOptions::default()).unwrap();
    let text: String = segments.iter().map(|s| s.text.as_str()).collect();
    assert!(
        text.chars().filter(|c| c.is_alphabetic()).count() > 10,
        "too little text: {text:?}"
    );
}

#[test]
fn missing_model_reports_the_path() {
    let err = WhisperEngine::load(Path::new("/nonexistent/model.bin"), false)
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)));
    assert!(err.to_string().contains("/nonexistent/model.bin"));
}

/// Edda on the FLEURS clip. Decoding with timestamp tokens garbled the
/// first words ("ketchupapirakanske" for "Som i alle sydafrikanske", 38% WER).
/// Needs the converted model: Settings → Speech model → Edda.
#[test]
#[ignore]
fn edda_transcribes_the_clip_from_its_first_word() {
    let model = fennec::config::Paths::user().models().join("edda-v0.1-q5_0.bin");
    let pcm = read_wav_16k_mono(&fixture("da_fleurs_0.wav")).unwrap();
    let reference = std::fs::read_to_string(fixture("da_fleurs_0.txt")).unwrap();
    let mut engine = WhisperEngine::load(&model, false).unwrap();
    let text: String = engine
        .transcribe(&pcm, &TranscribeOptions::default())
        .unwrap()
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    assert!(text.trim_start().starts_with("Som i alle"), "{text}");
    let wer = fennec::eval::word_errors(&reference, &text).rate();
    assert!(wer < 0.2, "WER {wer:.2}: {text}");
}
