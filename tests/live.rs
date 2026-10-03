//! Live dictation through the real pipeline: audio source → utterance
//! builder → engine worker → events → transcript → store.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fennec::audio::capture::PcmSource;
use fennec::audio::read_wav_16k_mono;
use fennec::commands::Command;
use fennec::engine::{EngineError, Segment, TranscribeOptions, Transcriber, WhisperEngine};
use fennec::live::{LiveConfig, LiveEvent, LiveSession};
use fennec::store::{NewDocument, Store};
use fennec::transcript::Transcript;
use fennec::utterance::{EnergyVad, SileroFrameVad};
use fennec::worker::EngineWorker;

const SR: usize = 16_000;

fn tone(secs: f32) -> Vec<f32> {
    (0..(secs * SR as f32) as usize)
        .map(|i| (i as f32 * 0.07).sin() * 0.3)
        .collect()
}

fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * SR as f32) as usize]
}

/// Answers each utterance with the next scripted line, and the same
/// utterance (previews, a quick pass, the accurate one) with the same line.
struct Scripted {
    lines: Vec<&'static str>,
    last: Option<(Vec<f32>, &'static str)>,
}

#[allow(non_snake_case)]
fn Scripted(lines: Vec<&'static str>) -> Scripted {
    Scripted { lines, last: None }
}

fn same_utterance(a: &[f32], b: &[f32]) -> bool {
    let n = a.len().min(b.len());
    n > 0 && a[..n] == b[..n]
}

impl Transcriber for Scripted {
    fn transcribe(&mut self, pcm: &[f32], _: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let text = match &self.last {
            // A preview's audio is the start of its utterance's.
            Some((last, text)) if same_utterance(last, pcm) => text,
            _ => {
                let text = if self.lines.is_empty() {
                    ""
                } else {
                    self.lines.remove(0)
                };
                self.last = Some((pcm.to_vec(), text));
                text
            }
        };
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: 1,
            text: text.into(),
            low_confidence: vec![],
        }])
    }
}

fn run(
    source: PcmSource,
    vad: Box<dyn fennec::utterance::FrameVad>,
    engine: Box<dyn Transcriber>,
    cfg: LiveConfig,
) -> Vec<LiveEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let worker = Arc::new(EngineWorker::spawn(engine));
    LiveSession::start(
        Box::new(source),
        vad,
        worker,
        cfg,
        Arc::new(move |e| sink.lock().unwrap().push(e)),
    )
    .wait();
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

#[test]
fn spoken_commands_shape_paragraphs_that_save_to_the_store() {
    let mut audio = Vec::new();
    for _ in 0..3 {
        audio.extend(tone(1.0));
        audio.extend(silence(1.0));
    }
    let engine = Scripted(vec!["Første sætning.", "Nyt afsnit.", "Anden sætning."]);
    let cfg = LiveConfig {
        show_preview: false,
        ..Default::default()
    };
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(engine),
        cfg,
    );

    assert!(
        events.contains(&LiveEvent::Command(Command::NewParagraph)),
        "{events:?}"
    );
    assert_eq!(events.last(), Some(&LiveEvent::Stopped));
    let mut transcript = Transcript::new(10_000);
    for e in &events {
        match e {
            LiveEvent::Final {
                text,
                start_ms,
                end_ms,
                low_confidence,
            } => {
                transcript.add_final(text, *start_ms, *end_ms, low_confidence);
            }
            LiveEvent::Command(c) => {
                transcript.apply(*c);
            }
            _ => {}
        }
    }
    let store = Store::open_in_memory().unwrap();
    let doc = store.create_document(&NewDocument::dictation("Diktat")).unwrap();
    store.replace_paragraphs(doc, &transcript.paragraphs).unwrap();
    let saved: Vec<String> = store
        .paragraphs(doc)
        .unwrap()
        .into_iter()
        .map(|p| p.text)
        .collect();
    assert_eq!(saved, ["Første sætning.", "Anden sætning."]);
    let first = &store.paragraphs(doc).unwrap()[0];
    assert!(
        first.start_ms.unwrap() < 200 && (900..1500).contains(&first.end_ms.unwrap()),
        "{first:?}"
    );
}

#[test]
fn offset_shifts_times_for_dictation_that_continues_a_document() {
    let mut audio = tone(1.0);
    audio.extend(silence(1.0));
    let cfg = LiveConfig {
        show_preview: false,
        offset_ms: 60_000,
        ..Default::default()
    };
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(Scripted(vec!["Hej."])),
        cfg,
    );
    let start = events.iter().find_map(|e| {
        if let LiveEvent::Final { start_ms, .. } = e {
            Some(*start_ms)
        } else {
            None
        }
    });
    assert!(start.is_some_and(|s| (60_000..60_300).contains(&s)), "{events:?}");
}

#[test]
fn the_recording_on_disk_holds_all_the_audio() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("rec.wav");
    let mut audio = tone(1.0);
    audio.extend(silence(1.5));
    let cfg = LiveConfig {
        show_preview: false,
        record_to: Some(wav.clone()),
        ..Default::default()
    };
    run(
        PcmSource::new(audio.clone()),
        Box::new(EnergyVad::default()),
        Box::new(Scripted(vec!["x"])),
        cfg,
    );
    let back = read_wav_16k_mono(&wav).unwrap();
    assert_eq!(back.len(), audio.len());
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn real_engine_and_silero_turn_danish_speech_into_a_final_event() {
    let models = fixture("models");
    let mut audio = read_wav_16k_mono(&fixture("da_fleurs_0.wav")).unwrap();
    audio.extend(silence(1.5));
    let engine = WhisperEngine::load(&models.join("ggml-tiny.bin"), false).unwrap();
    let vad = SileroFrameVad::load(&models.join("ggml-silero-v6.2.0.bin")).unwrap();
    let cfg = LiveConfig {
        transcribe: TranscribeOptions {
            threads: 4,
            ..Default::default()
        },
        ..Default::default()
    };
    let events = run(PcmSource::new(audio), Box::new(vad), Box::new(engine), cfg);

    let finals: Vec<(&String, i64)> = events
        .iter()
        .filter_map(|e| {
            if let LiveEvent::Final { text, start_ms, .. } = e {
                Some((text, *start_ms))
            } else {
                None
            }
        })
        .collect();
    assert!(!finals.is_empty(), "{events:?}");
    assert!(
        finals[0].1 < 1_500,
        "speech starts near the beginning: {finals:?}"
    );
    assert!(finals.iter().map(|f| f.0.len()).sum::<usize>() > 20, "{finals:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LiveEvent::Level(l) if *l > 0.01))
    );
}

/// Remembers the options of every call; always hears the same sentence.
struct Recording(Arc<Mutex<Vec<TranscribeOptions>>>, &'static str);

impl Transcriber for Recording {
    fn transcribe(&mut self, _: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        self.0.lock().unwrap().push(opts.clone());
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: 1,
            text: self.1.into(),
            low_confidence: vec![],
        }])
    }
}

#[test]
fn the_vocabulary_corrects_the_text_and_no_prompt_reaches_the_model() {
    // Any prompt wrecks Edda (FLEURS WER 7.8% → 87.7% with a vocabulary),
    // and feeding back what was just said made it drop and garble words.
    let mut audio = Vec::new();
    for _ in 0..3 {
        audio.extend(tone(1.0));
        audio.extend(silence(1.0));
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let cfg = LiveConfig {
        vocabulary: "Nørregade, Vicevært".into(),
        ..Default::default()
    };
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(Recording(Arc::clone(&calls), "Vi mødes på Nørregarde.")),
        cfg,
    );
    let calls = calls.lock().unwrap();
    assert!(
        calls.len() >= 6,
        "a quick and an accurate pass each: {}",
        calls.len()
    );
    assert!(calls.iter().all(|o| o.initial_prompt.is_none()));
    let shown: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Final { text, .. } | LiveEvent::Preview(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(shown.len() >= 3, "{events:?}");
    assert!(shown.iter().all(|t| *t == "Vi mødes på Nørregade."), "{shown:?}");
}

#[test]
fn previews_use_the_fast_encoder_and_finals_the_full_one() {
    let mut audio = tone(4.0);
    audio.extend(silence(1.5));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = run(
        PcmSource::new(audio).realtime(),
        Box::new(EnergyVad::default()),
        Box::new(Recording(Arc::clone(&calls), "Sætning.")),
        LiveConfig::default(),
    );
    let calls = calls.lock().unwrap();
    let finals = events
        .iter()
        .filter(|e| matches!(e, LiveEvent::Final { .. }))
        .count();
    assert!(calls.iter().any(|o| o.fast), "a preview ran: {calls:?}");
    assert_eq!(calls.iter().filter(|o| !o.fast).count(), finals);
    assert_eq!(finals, 1);
}

#[test]
fn clipped_input_warns_once_rather_than_every_chunk() {
    let clipped: Vec<f32> = (0..3 * SR)
        .map(|i| if (i / 20) % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let events = run(
        PcmSource::new(clipped),
        Box::new(EnergyVad::default()),
        Box::new(Scripted(vec![])),
        LiveConfig {
            show_preview: false,
            ..Default::default()
        },
    );
    assert_eq!(events.iter().filter(|e| **e == LiveEvent::Clipping).count(), 1);
}

#[test]
fn a_normal_level_never_warns() {
    let mut audio = tone(2.0);
    audio.extend(silence(1.0));
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(Scripted(vec!["Hej."])),
        LiveConfig::default(),
    );
    assert!(!events.contains(&LiveEvent::Clipping));
}

#[test]
fn the_level_meter_gets_a_steady_twenty_readings_a_second() {
    // PcmSource delivers 100 ms chunks; a microphone may deliver 10 ms or 170 ms.
    let mut audio = tone(1.0);
    audio.extend(silence(1.0));
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(Scripted(vec!["Hej."])),
        LiveConfig {
            show_preview: false,
            ..Default::default()
        },
    );
    let levels: Vec<f32> = events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Level(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(levels.len(), 40, "{levels:?}");
    assert!(levels[5] > 0.1 && levels[35] < 0.001, "{levels:?}");
}

/// Answers fast and full passes differently and records which it ran.
struct TwoPass {
    fast: &'static str,
    full: &'static str,
    calls: Arc<Mutex<Vec<bool>>>,
}

impl Transcriber for TwoPass {
    fn transcribe(&mut self, _: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        self.calls.lock().unwrap().push(opts.fast);
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: 1,
            text: if opts.fast { self.fast } else { self.full }.into(),
            low_confidence: vec![],
        }])
    }
}

fn short_utterance(fast: &'static str, full: &'static str) -> (Vec<LiveEvent>, Vec<bool>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut audio = tone(0.5);
    audio.extend(silence(1.0));
    let events = run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(TwoPass {
            fast,
            full,
            calls: Arc::clone(&calls),
        }),
        LiveConfig {
            show_preview: false,
            ..Default::default()
        },
    );
    let calls = calls.lock().unwrap().clone();
    (events, calls)
}

#[test]
fn a_short_command_acts_on_the_quick_pass_alone() {
    let (events, calls) = short_utterance("Punktum.", "Punktum.");
    assert!(
        events.contains(&LiveEvent::Command(Command::Punctuate('.'))),
        "{events:?}"
    );
    assert_eq!(calls, [true], "no slow pass for a command");
}

#[test]
fn a_short_phrase_shows_the_quick_text_then_the_accurate_one() {
    let (events, calls) = short_utterance("Hej med dig", "Hej med dig.");
    let shown: Vec<&LiveEvent> = events
        .iter()
        .filter(|e| matches!(e, LiveEvent::Preview(_) | LiveEvent::Final { .. }))
        .collect();
    assert!(
        matches!(shown.as_slice(), [LiveEvent::Preview(p), LiveEvent::Final { text, .. }]
            if p == "Hej med dig" && text == "Hej med dig."),
        "{shown:?}"
    );
    assert_eq!(calls, [true, false]);
}

#[test]
fn a_command_the_quick_pass_missed_is_still_caught() {
    let (events, _) = short_utterance("Bunktum", "Punktum.");
    assert!(
        events.contains(&LiveEvent::Command(Command::Punctuate('.'))),
        "{events:?}"
    );
}

#[test]
fn long_utterances_go_straight_to_the_accurate_pass() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut audio = tone(4.0);
    audio.extend(silence(1.0));
    run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(TwoPass {
            fast: "x",
            full: "y",
            calls: Arc::clone(&calls),
        }),
        LiveConfig {
            show_preview: false,
            ..Default::default()
        },
    );
    assert_eq!(*calls.lock().unwrap(), [false]);
}

#[test]
fn a_model_that_takes_context_hears_the_sentence_before_each_utterance() {
    let mut audio = Vec::new();
    for _ in 0..2 {
        audio.extend(tone(1.0));
        audio.extend(silence(1.0));
    }
    audio.extend(tone(4.0));
    audio.extend(silence(1.0));
    let calls = Arc::new(Mutex::new(Vec::new()));
    run(
        PcmSource::new(audio),
        Box::new(EnergyVad::default()),
        Box::new(Recording(Arc::clone(&calls), "Det regner.")),
        LiveConfig {
            context: Some("Sagen gælder Nørregade 14.".into()),
            ..Default::default()
        },
    );
    let finals: Vec<Option<String>> = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|o| !o.fast)
        .map(|o| o.initial_prompt.clone())
        .collect();
    assert_eq!(
        finals,
        [
            Some("Sagen gælder Nørregade 14.".to_string()),
            Some("Sagen gælder Nørregade 14. Det regner.".into()),
            // The long one (no quick pass) too.
            Some("Sagen gælder Nørregade 14. Det regner. Det regner.".into()),
        ]
    );
}

#[test]
fn the_app_can_replace_the_context_with_punctuated_text() {
    let mut audio = Vec::new();
    for _ in 0..2 {
        audio.extend(tone(1.0));
        audio.extend(silence(1.5));
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let session = LiveSession::start(
        Box::new(PcmSource::new(audio).realtime()),
        Box::new(EnergyVad::default()),
        Arc::new(EngineWorker::spawn(Box::new(Recording(
            Arc::clone(&calls),
            "det regner",
        )))),
        LiveConfig {
            show_preview: false,
            context: Some(String::new()),
            ..Default::default()
        },
        Arc::new(move |e| {
            if matches!(e, LiveEvent::Final { .. }) {
                let _ = tx.send(());
            }
        }),
    );
    let context = session.context().expect("a model that takes context");
    rx.recv().unwrap();
    // What the punctuation pass made of it.
    context.set("Det regner.");
    session.wait();
    let last = calls.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.initial_prompt.as_deref(), Some("Det regner."));
}
