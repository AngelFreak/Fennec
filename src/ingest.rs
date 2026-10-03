//! File transcription: decode → find speech → transcribe chunks → paragraphs
//! in the store, reporting progress as it goes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::decode::{DecodeError, decode_file};
use crate::engine::{EngineError, SAMPLE_RATE, TranscribeOptions, Transcriber};
use crate::paragraphs::ParagraphBuilder;
use crate::store::{DocumentId, Paragraph, Store, StoreError};
use crate::vad::{SpeechDetector, VadError, chunk_speech};
use crate::vocabulary::Vocabulary;
use crate::worker::Context;

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub transcribe: TranscribeOptions,
    /// Names and terms the transcript is corrected towards.
    pub vocabulary: String,
    pub paragraph_gap_ms: i64,
    /// Prompt each chunk with the text before it, for models trained that
    /// way (Edda v0.2); others are harmed by any prompt.
    pub context: bool,
    /// Whisper's window is 30 s; chunks stay below that.
    pub max_chunk_ms: i64,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            transcribe: TranscribeOptions::default(),
            vocabulary: String::new(),
            context: false,
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
    /// Adds punctuation to each paragraph before it is saved.
    pub punctuator: Option<&'a dyn crate::punctuation::Punctuate>,
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
    let punctuator = models.punctuator;
    let save = |mut p: Paragraph,
                saved: &mut usize,
                on_event: &mut dyn FnMut(IngestEvent)|
     -> Result<(), IngestError> {
        if let Some(m) = punctuator {
            match crate::punctuation::punctuate(m, &p.text, false) {
                Ok(text) => {
                    p.low_confidence = crate::punctuation::moved_spans(&p.text, &text, &p.low_confidence);
                    p.text = text;
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
        let id = store.append_paragraph(doc, &p)?;
        *saved += 1;
        on_event(IngestEvent::Paragraph(Paragraph { id: Some(id), ..p }));
        Ok(())
    };

    let vocabulary = Vocabulary::parse(&opts.vocabulary);
    let context = opts.context.then(|| Context::new(""));
    for chunk in &chunks {
        if cancel.load(Ordering::Relaxed) {
            if let Some(p) = builder.finish() {
                save(p, &mut saved, &mut on_event)?;
            }
            return Err(IngestError::Cancelled);
        }
        let offset = samples_to_ms(chunk.start);
        let mut t = opts.transcribe.clone();
        if let Some(c) = &context {
            t.initial_prompt = c.prompt();
        }
        let segments = models.engine.transcribe(&pcm[chunk.clone()], &t)?;
        if let Some(c) = &context {
            c.push(
                &segments
                    .iter()
                    .map(|s| s.text.trim())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        // Engines may return empty segments for silence; they are not paragraphs.
        for mut seg in segments.into_iter().filter(|s| !s.text.trim().is_empty()) {
            (seg.text, seg.low_confidence) = vocabulary.correct(&seg.text, &seg.low_confidence);
            if let Some(done) = builder.push(&seg, offset) {
                save(done, &mut saved, &mut on_event)?;
            }
        }
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

fn samples_to_ms(n: usize) -> i64 {
    n as i64 * 1000 / SAMPLE_RATE as i64
}

fn ms_to_samples(ms: i64) -> usize {
    (ms.max(0) as usize) * SAMPLE_RATE as usize / 1000
}
