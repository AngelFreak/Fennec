//! Speech-to-text engines. The rest of the app only sees [`Transcriber`].

mod bilingual;
pub mod sidecar;
mod whisper;

pub use bilingual::Bilingual;
pub use sidecar::SidecarEngine;
pub use whisper::WhisperEngine;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Audio sample rate every engine expects (mono f32).
pub const SAMPLE_RATE: u32 = 16_000;

/// One transcribed stretch of audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Start and end relative to the start of the audio passed in.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Byte ranges in `text` covering words the model was unsure of.
    pub low_confidence: Vec<std::ops::Range<usize>>,
}

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    /// ISO 639-1 code, used when `spoken` is `None`.
    pub language: String,
    /// The language of a whole dictation or file, chosen or detected.
    pub spoken: Option<SpokenLanguage>,
    /// Vocabulary that steers the spelling of names and terms.
    pub initial_prompt: Option<String>,
    pub threads: usize,
    /// Tokens with a probability below this are marked low-confidence.
    pub low_confidence_threshold: f32,
    /// Encode only as much audio as there is instead of a full 30 s window:
    /// about 2.5× faster, but WER rose from 7.9% to 10.6% for Edda on FLEURS.
    /// For live previews only.
    pub fast: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            language: "da".into(),
            spoken: None,
            initial_prompt: None,
            threads: default_threads(),
            low_confidence_threshold: 0.4,
            fast: false,
        }
    }
}

/// Leaves a couple of cores for the UI and audio threads.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(1))
        .unwrap_or(4)
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("model file not found: {0}")]
    ModelMissing(PathBuf),
    #[error("could not load model {path}: {source}")]
    Load {
        path: PathBuf,
        source: whisper_rs::WhisperError,
    },
    #[error("transcription failed: {0}")]
    Transcribe(#[from] whisper_rs::WhisperError),
    #[error("the transcription engine has stopped")]
    WorkerStopped,
    #[error("{0}")]
    Other(String),
}

/// Loads the engine a model path needs: a hviske-style model directory
/// runs in the Python helper, anything else is a whisper.cpp GGML file.
pub fn load_engine(path: &std::path::Path, gpu: bool) -> Result<Box<dyn Transcriber>, EngineError> {
    if sidecar::is_sidecar_model(path) {
        return Ok(Box::new(SidecarEngine::hviske(&crate::models::python(), path)?));
    }
    Ok(Box::new(WhisperEngine::load(path, gpu)?))
}

pub trait Transcriber: Send {
    /// Transcribes 16 kHz mono samples.
    fn transcribe(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError>;

    /// Which of `among` (ISO 639-1 codes) the speech is in, and how sure
    /// the model is (0–1). `None` from models that cannot tell.
    fn detect_language(
        &mut self,
        _pcm: &[f32],
        _among: &[&str],
        _threads: usize,
    ) -> Result<Option<(String, f32)>, EngineError> {
        Ok(None)
    }
}

/// The language of one dictation or file: chosen in Settings, or detected
/// from its first words and then kept, so a session never switches model
/// halfway. Clones share what was detected.
#[derive(Debug, Clone, Default)]
pub struct SpokenLanguage {
    chosen: Option<String>,
    detected: Arc<Mutex<Option<String>>>,
}

impl SpokenLanguage {
    pub fn chosen(code: &str) -> Self {
        Self {
            chosen: Some(code.to_string()),
            ..Default::default()
        }
    }

    pub fn detect() -> Self {
        Self::default()
    }

    /// The same choice for a new dictation or file, which detects afresh.
    pub fn fresh(&self) -> Self {
        Self {
            chosen: self.chosen.clone(),
            ..Default::default()
        }
    }

    /// The language, once chosen or detected.
    pub fn current(&self) -> Option<String> {
        self.chosen
            .clone()
            .or_else(|| self.detected.lock().expect("language poisoned").clone())
    }

    pub fn is_english(&self) -> bool {
        self.current().as_deref() == Some("en")
    }

    fn remember(&self, code: &str) {
        *self.detected.lock().expect("language poisoned") = Some(code.to_string());
    }
}

/// Edda now and then writes no space after a full stop ("positivt.Denne");
/// puts one where a lower-case letter, a sentence mark and a capital meet
/// (so initials such as "A.P. Møller" stay), moving unsure spans along.
pub fn space_after_sentences(
    text: &str,
    spans: &[std::ops::Range<usize>],
) -> (String, Vec<std::ops::Range<usize>>) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut inserts = Vec::new();
    for w in chars.windows(3) {
        let [(_, a), (_, b), (at, c)] = [w[0], w[1], w[2]];
        if a.is_lowercase() && matches!(b, '.' | '?' | '!') && c.is_uppercase() {
            inserts.push(at);
        }
    }
    if inserts.is_empty() {
        return (text.to_string(), spans.to_vec());
    }
    let mut out = String::with_capacity(text.len() + inserts.len());
    let mut last = 0;
    for &at in &inserts {
        out.push_str(&text[last..at]);
        out.push(' ');
        last = at;
    }
    out.push_str(&text[last..]);
    let shift = |p: usize| p + inserts.iter().filter(|&&i| i <= p).count();
    let spans: Vec<std::ops::Range<usize>> = spans
        .iter()
        .map(|r| shift(r.start)..shift(r.end.max(r.start + 1) - 1) + 1)
        .collect();
    (out, spans)
}

#[cfg(test)]
mod spacing_tests {
    use super::space_after_sentences;

    #[test]
    fn a_missing_space_after_a_full_stop_is_put_back() {
        let text = "Det er positivt.Denne graf hakker";
        let graf = text.find("graf").unwrap();
        let (out, spans) = space_after_sentences(text, &[graf..graf + 4]);
        assert_eq!(out, "Det er positivt. Denne graf hakker");
        assert_eq!(&out[spans[0].clone()], "graf");
    }

    #[test]
    fn initials_and_numbers_stay_as_they_are() {
        for t in ["A.P. Møller", "kl. 14.30", "Det er godt. Ja."] {
            assert_eq!(space_after_sentences(t, &[]).0, t);
        }
    }
}
