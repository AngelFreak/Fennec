//! File transcription: decode → find speech → transcribe chunks → paragraphs
//! in the store, reporting progress as it goes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::decode::{DecodeError, decode_file};
use crate::engine::{EngineError, SAMPLE_RATE, TranscribeOptions, Transcriber};
use crate::paragraphs::ParagraphBuilder;
use crate::store::{DocumentId, Paragraph, Store, StoreError};
use crate::vad::{SpeechDetector, VadError, chunk_speech};

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub transcribe: TranscribeOptions,
    /// Names and terms passed to the engine with every chunk.
    pub vocabulary: String,
    pub paragraph_gap_ms: i64,
    /// Whisper's window is 30 s; chunks stay below that.
    pub max_chunk_ms: i64,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            transcribe: TranscribeOptions::default(),
            vocabulary: String::new(),
            paragraph_gap_ms: 1_500,
            max_chunk_ms: 25_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestEvent {
    Decoded {
        duration_ms: i64,
    },
    /// `done_ms` of `total_ms` audio has been transcribed.
    Progress {
        done_ms: i64,
        total_ms: i64,
    },
    /// A paragraph was saved to the document.
    Paragraph(Paragraph),
    Finished {
        paragraphs: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Vad(#[from] VadError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("cancelled")]
    Cancelled,
}

/// The speech models a job runs with.
pub struct Recognizer<'a> {
    pub engine: &'a mut dyn Transcriber,
    pub vad: &'a mut dyn SpeechDetector,
}

/// Transcribes `path` into document `doc`, appending paragraphs as they are
/// finished. Checks `cancel` between chunks; paragraphs saved before a
/// cancel or error are kept.
pub fn ingest_file(
    path: &Path,
    doc: DocumentId,
    store: &Store,
    models: Recognizer<'_>,
    opts: &IngestOptions,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(IngestEvent),
) -> Result<usize, IngestError> {
    let pcm = decode_file(path)?;
    let total_ms = samples_to_ms(pcm.len());
    store.set_audio_path(doc, Some(path))?;
    on_event(IngestEvent::Decoded {
        duration_ms: total_ms,
    });

    let speech = models.vad.speech(&pcm)?;
    let max = ms_to_samples(opts.max_chunk_ms);
    let chunks = chunk_speech(&speech, max, ms_to_samples(500));

    let mut builder = ParagraphBuilder::new(opts.paragraph_gap_ms, 90_000);
    let mut saved = 0;
    let mut context = String::new();
    let save =
        |p: Paragraph, saved: &mut usize, on_event: &mut dyn FnMut(IngestEvent)| -> Result<(), IngestError> {
            let id = store.append_paragraph(doc, &p)?;
            *saved += 1;
            on_event(IngestEvent::Paragraph(Paragraph { id: Some(id), ..p }));
            Ok(())
        };

    for chunk in &chunks {
        if cancel.load(Ordering::Relaxed) {
            if let Some(p) = builder.finish() {
                save(p, &mut saved, &mut on_event)?;
            }
            return Err(IngestError::Cancelled);
        }
        let mut t = opts.transcribe.clone();
        t.initial_prompt = prompt(&opts.vocabulary, &context);
        let offset = samples_to_ms(chunk.start);
        let segments = models.engine.transcribe(&pcm[chunk.clone()], &t)?;
        // Engines may return empty segments for silence; they are not paragraphs.
        for seg in segments.into_iter().filter(|s| !s.text.trim().is_empty()) {
            context.push(' ');
            context.push_str(&seg.text);
            if let Some(done) = builder.push(&seg, offset) {
                save(done, &mut saved, &mut on_event)?;
            }
        }
        keep_tail(&mut context, 200);
        on_event(IngestEvent::Progress {
            done_ms: samples_to_ms(chunk.end),
            total_ms,
        });
    }
    if let Some(p) = builder.finish() {
        save(p, &mut saved, &mut on_event)?;
    }
    on_event(IngestEvent::Progress {
        done_ms: total_ms,
        total_ms,
    });
    on_event(IngestEvent::Finished { paragraphs: saved });
    Ok(saved)
}

/// Vocabulary first, then the end of what was just said.
pub fn prompt(vocabulary: &str, context: &str) -> Option<String> {
    let p = format!("{} {}", vocabulary.trim(), context.trim());
    let p = p.trim();
    (!p.is_empty()).then(|| p.to_string())
}

/// Keeps roughly the last `chars` characters, cut at a char boundary.
pub fn keep_tail(s: &mut String, chars: usize) {
    let count = s.chars().count();
    if count > chars {
        let cut = s.char_indices().nth(count - chars).map(|(i, _)| i).unwrap_or(0);
        s.drain(..cut);
    }
}

fn samples_to_ms(n: usize) -> i64 {
    n as i64 * 1000 / SAMPLE_RATE as i64
}

fn ms_to_samples(ms: i64) -> usize {
    (ms.max(0) as usize) * SAMPLE_RATE as usize / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_tail_cuts_on_char_boundaries() {
        let mut s = "æøå".repeat(100);
        keep_tail(&mut s, 5);
        assert_eq!(s, "øåæøå");
    }

    #[test]
    fn prompt_combines_vocabulary_and_context_or_is_none() {
        assert_eq!(
            prompt(" BBR ", "sidste sætning").as_deref(),
            Some("BBR sidste sætning")
        );
        assert_eq!(prompt("", " "), None);
    }
}
