//! Live dictation: an audio source → utterances → the engine worker → events.
//!
//! Two threads: *capture* reads audio, runs VAD and submits jobs; *results*
//! waits for transcriptions in order and turns them into events. Audio is
//! never dropped: if the engine falls behind, finals queue up and `Lag`
//! reports by how much.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, unbounded};

use crate::audio::capture::AudioSource;
use crate::commands::{Command, CommandTable};
use crate::engine::{SAMPLE_RATE, TranscribeOptions};
use crate::ingest::prompt;
use crate::utterance::{FrameVad, Utterance, UtteranceBuilder, UtteranceConfig, UtteranceEvent};
use crate::worker::{EngineWorker, Priority, Reply};

#[derive(Debug, Clone)]
pub struct LiveConfig {
    pub utterance: UtteranceConfig,
    pub transcribe: TranscribeOptions,
    pub vocabulary: String,
    pub commands: CommandTable,
    pub show_preview: bool,
    /// Raw 16 kHz audio is written here as it arrives (crash safety).
    pub record_to: Option<PathBuf>,
    /// Added to event times, for dictation that continues a document.
    pub offset_ms: i64,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            utterance: UtteranceConfig::default(),
            transcribe: TranscribeOptions::default(),
            vocabulary: String::new(),
            commands: CommandTable::default(),
            show_preview: true,
            record_to: None,
            offset_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    /// Input loudness (RMS of the last chunk), for the level meter.
    Level(f32),
    /// The input hit the ceiling; at most one every few seconds.
    Clipping,
    SpeechStarted,
    Preview(String),
    Final {
        text: String,
        start_ms: i64,
        end_ms: i64,
        low_confidence: Vec<std::ops::Range<usize>>,
    },
    Command(Command),
    /// How far transcription trails speech, in milliseconds.
    Lag(i64),
    Error(String),
    /// The session ended (source finished, stopped, or failed).
    Stopped,
}

pub type EventSink = Arc<dyn Fn(LiveEvent) + Send + Sync>;

enum Pending {
    Final(Utterance, Receiver<Reply>),
    Preview(u64, Receiver<Reply>),
}

pub struct LiveSession {
    stop: Arc<AtomicBool>,
    capture: Option<JoinHandle<()>>,
    results: Option<JoinHandle<()>>,
}

impl LiveSession {
    pub fn start(
        mut source: Box<dyn AudioSource>,
        vad: Box<dyn FrameVad>,
        worker: Arc<EngineWorker>,
        cfg: LiveConfig,
        on_event: EventSink,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (pending_tx, pending_rx) = unbounded::<Pending>();
        let started = Instant::now();

        let capture = {
            let worker = Arc::clone(&worker);
            let stop = Arc::clone(&stop);
            let on_event = Arc::clone(&on_event);
            let cfg = cfg.clone();
            std::thread::Builder::new()
                .name("fennec-live-capture".into())
                .spawn(move || {
                    let mut builder = UtteranceBuilder::new(cfg.utterance.clone(), vad);
                    let mut recorder = cfg.record_to.as_ref().and_then(|p| match open_recording(p) {
                        Ok(w) => Some(w),
                        Err(e) => {
                            on_event(LiveEvent::Error(format!("could not save the recording: {e}")));
                            None
                        }
                    });
                    let opts_for = |fast: bool| {
                        let mut t = cfg.transcribe.clone();
                        t.initial_prompt = prompt(&cfg.vocabulary);
                        // Previews are replaced by the final text, so they trade
                        // accuracy for speed and stop holding up the finals.
                        t.fast = fast;
                        t
                    };
                    let handle = |events: Vec<UtteranceEvent>| {
                        for ev in events {
                            match ev {
                                UtteranceEvent::Started { .. } => on_event(LiveEvent::SpeechStarted),
                                UtteranceEvent::Partial(u) => {
                                    if cfg.show_preview && !worker.is_busy() {
                                        let rx =
                                            worker.submit(u.samples, opts_for(true), Priority::LivePartial);
                                        let _ = pending_tx.send(Pending::Preview(u.id, rx));
                                    }
                                }
                                UtteranceEvent::Final(u) => {
                                    let rx = worker.submit(
                                        u.samples.clone(),
                                        opts_for(false),
                                        Priority::LiveFinal,
                                    );
                                    let _ = pending_tx.send(Pending::Final(u, rx));
                                }
                            }
                        }
                    };
                    // Audio time of the last clipping warning.
                    let mut warned_at: Option<usize> = None;
                    let mut heard = 0usize;
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        match source.next_chunk() {
                            Ok(Some(chunk)) => {
                                let (peak, level) = crate::audio::level::measure(&chunk);
                                on_event(LiveEvent::Level(level));
                                heard += chunk.len();
                                if peak >= crate::audio::level::CLIP
                                    && warned_at
                                        .is_none_or(|t| heard - t >= 5 * crate::engine::SAMPLE_RATE as usize)
                                {
                                    warned_at = Some(heard);
                                    on_event(LiveEvent::Clipping);
                                }
                                if let Some(w) = &mut recorder {
                                    write_recording(w, &chunk);
                                }
                                handle(builder.push(&chunk));
                            }
                            Ok(None) => break,
                            Err(e) => {
                                on_event(LiveEvent::Error(e.to_string()));
                                break;
                            }
                        }
                    }
                    handle(builder.flush());
                    if let Some(w) = recorder
                        && let Err(e) = w.finalize()
                    {
                        on_event(LiveEvent::Error(format!("could not finish the recording: {e}")));
                    }
                    // Dropping the sender lets the results thread finish.
                })
                .expect("spawning the capture thread")
        };

        let results = {
            let on_event = Arc::clone(&on_event);
            // Keeps the engine alive until every queued result is delivered.
            let worker_alive = Arc::clone(&worker);
            let offset = cfg.offset_ms;
            let commands = cfg.commands.clone();
            std::thread::Builder::new()
                .name("fennec-live-results".into())
                .spawn(move || {
                    let mut finalized: Option<u64> = None;
                    for pending in pending_rx {
                        match pending {
                            Pending::Preview(id, rx) => {
                                // Replaced previews close their channel; stale ones are skipped.
                                if let Ok(Ok(segs)) = rx.recv()
                                    && finalized.is_none_or(|f| id > f)
                                {
                                    on_event(LiveEvent::Preview(join(&segs).0));
                                }
                            }
                            Pending::Final(u, rx) => {
                                finalized = Some(u.id);
                                match rx.recv() {
                                    Ok(Ok(segs)) => {
                                        let (text, low_confidence) = join(&segs);
                                        let lag = started.elapsed().as_millis() as i64 - u.end_ms();
                                        on_event(LiveEvent::Lag(lag.max(0)));
                                        if let Some(cmd) = commands.match_utterance(&text) {
                                            on_event(LiveEvent::Command(cmd));
                                        } else if !text.is_empty() {
                                            on_event(LiveEvent::Final {
                                                text,
                                                start_ms: u.start_ms() + offset,
                                                end_ms: u.end_ms() + offset,
                                                low_confidence,
                                            });
                                        }
                                    }
                                    Ok(Err(e)) => on_event(LiveEvent::Error(e.to_string())),
                                    Err(_) => on_event(LiveEvent::Error("the engine stopped".into())),
                                }
                            }
                        }
                    }
                    drop(worker_alive);
                    on_event(LiveEvent::Stopped);
                })
                .expect("spawning the results thread")
        };

        Self {
            stop,
            capture: Some(capture),
            results: Some(results),
        }
    }

    /// Stops listening. Speech in progress is still transcribed; this
    /// returns after the last result has been delivered.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.join();
    }

    /// Waits for the source to end on its own (tests, file playback).
    pub fn wait(mut self) {
        self.join();
    }

    fn join(&mut self) {
        if let Some(t) = self.capture.take() {
            let _ = t.join();
        }
        if let Some(t) = self.results.take() {
            let _ = t.join();
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.join();
    }
}

/// Joins segment texts with spaces, shifting low-confidence spans.
fn join(segs: &[crate::engine::Segment]) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    for s in segs {
        if !text.is_empty() {
            text.push(' ');
        }
        let shift = text.len();
        text.push_str(&s.text);
        spans.extend(s.low_confidence.iter().map(|r| r.start + shift..r.end + shift));
    }
    (text, spans)
}

type Recording = hound::WavWriter<std::io::BufWriter<std::fs::File>>;

fn open_recording(path: &std::path::Path) -> Result<Recording, hound::Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(path, spec)
}

fn write_recording(w: &mut Recording, chunk: &[f32]) {
    for s in chunk {
        let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    // Flushing keeps the header valid, so a crash still leaves a playable file.
    let _ = w.flush();
}
