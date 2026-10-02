//! Speech-to-text engines. The rest of the app only sees [`Transcriber`].

pub mod sidecar;
mod whisper;

pub use sidecar::SidecarEngine;
pub use whisper::WhisperEngine;

use std::path::PathBuf;

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
    /// ISO 639-1 code; Fennec always passes "da".
    pub language: String,
    /// Vocabulary and preceding text that steer spelling and continuity.
    pub initial_prompt: Option<String>,
    pub threads: usize,
    /// Tokens with a probability below this are marked low-confidence.
    pub low_confidence_threshold: f32,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            language: "da".into(),
            initial_prompt: None,
            threads: default_threads(),
            low_confidence_threshold: 0.4,
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
}
