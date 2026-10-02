//! Voice activity detection: where in the audio someone is speaking.

use std::ops::Range;
use std::path::Path;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::engine::SAMPLE_RATE;

/// Sample ranges (16 kHz) that contain speech, in order.
pub trait SpeechDetector: Send {
    fn speech(&mut self, pcm: &[f32]) -> Result<Vec<Range<usize>>, VadError>;
}

#[derive(Debug, thiserror::Error)]
pub enum VadError {
    #[error("VAD model not found: {0}")]
    ModelMissing(std::path::PathBuf),
    #[error("voice detection failed: {0}")]
    Failed(#[from] whisper_rs::WhisperError),
}

/// Silero VAD through whisper.cpp.
pub struct SileroVad {
    ctx: WhisperVadContext,
    pub min_silence_ms: i32,
}

impl SileroVad {
    pub fn load(model: &Path, threads: usize) -> Result<Self, VadError> {
        if !model.exists() {
            return Err(VadError::ModelMissing(model.to_path_buf()));
        }
        let mut params = WhisperVadContextParams::new();
        params.set_n_threads(threads as i32);
        params.set_use_gpu(false);
        let ctx = WhisperVadContext::new(&model.to_string_lossy(), params)?;
        Ok(Self {
            ctx,
            min_silence_ms: 300,
        })
    }
}

impl SpeechDetector for SileroVad {
    fn speech(&mut self, pcm: &[f32]) -> Result<Vec<Range<usize>>, VadError> {
        let mut params = WhisperVadParams::new();
        params.set_min_silence_duration(self.min_silence_ms);
        params.set_speech_pad(100);
        let segments = self.ctx.segments_from_samples(params, pcm)?;
        // whisper.cpp reports centiseconds.
        let to_samples = |cs: f32| ((cs as f64 / 100.0) * SAMPLE_RATE as f64) as usize;
        Ok(segments
            .map(|s| to_samples(s.start).min(pcm.len())..to_samples(s.end).min(pcm.len()))
            .filter(|r| r.start < r.end)
            .collect())
    }
}

/// Treats all audio as speech, so it is cut into fixed chunks. Used when the
/// Silero model is missing.
pub struct WholeAudio;

impl SpeechDetector for WholeAudio {
    fn speech(&mut self, pcm: &[f32]) -> Result<Vec<Range<usize>>, VadError> {
        Ok(if pcm.is_empty() {
            Vec::new()
        } else {
            vec![0..pcm.len()]
        })
    }
}

/// Packs speech ranges into chunks for the engine: each chunk is at most
/// `max_len` samples, and ranges closer than `join_gap` samples share a chunk.
/// Speech longer than `max_len` is split into `max_len` pieces.
pub fn chunk_speech(speech: &[Range<usize>], max_len: usize, join_gap: usize) -> Vec<Range<usize>> {
    let mut chunks: Vec<Range<usize>> = Vec::new();
    for r in speech {
        let mut start = r.start;
        while start < r.end {
            let end = (start + max_len).min(r.end);
            match chunks.last_mut() {
                Some(last) if start.saturating_sub(last.end) <= join_gap && end - last.start <= max_len => {
                    last.end = end
                }
                _ => chunks.push(start..end),
            }
            start = end;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_ranges_join_into_one_chunk() {
        assert_eq!(chunk_speech(&[0..100, 120..200], 1000, 50), vec![0..200]);
    }

    #[test]
    fn distant_ranges_stay_apart() {
        assert_eq!(
            chunk_speech(&[0..100, 300..400], 1000, 50),
            vec![0..100, 300..400]
        );
    }

    #[test]
    fn chunks_never_exceed_the_maximum() {
        let chunks = chunk_speech(&[0..250, 260..300], 100, 50);
        assert!(chunks.iter().all(|c| c.len() <= 100), "{chunks:?}");
        assert_eq!(chunks.first().unwrap().start, 0);
        assert_eq!(chunks.last().unwrap().end, 300);
    }

    #[test]
    fn missing_model_is_reported() {
        assert!(matches!(
            SileroVad::load(Path::new("/nope/silero.bin"), 1),
            Err(VadError::ModelMissing(_))
        ));
    }
}
