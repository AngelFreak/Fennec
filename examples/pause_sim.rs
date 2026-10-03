//! Would merging fragments help? FLEURS clips with a 0.7 s pause put in
//! mid-sentence go through the live pipeline twice: split at the pause
//! (as before: 0.6 s ends an utterance; Edda v0.2 gets the first part as
//! context) and with the default now (a short fragment is held open).
//!   cargo run --release --features vulkan --example pause_sim -- manifest.tsv [clips]
use std::sync::{Arc, Mutex};

use fennec::audio::capture::PcmSource;
use fennec::audio::read_wav_16k_mono;
use fennec::engine::load_engine;
use fennec::eval::{ErrorCount, Punctuation, punctuation, word_errors};
use fennec::live::{LiveConfig, LiveEvent, LiveSession};
use fennec::utterance::{SileroFrameVad, UtteranceConfig};
use fennec::worker::EngineWorker;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let manifest = std::path::Path::new(&args[1]);
    let limit: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(60);
    let models = fennec::config::Paths::user().models();
    let model = fennec::config::Settings::default().model;
    let worker = Arc::new(EngineWorker::spawn(
        load_engine(&models.join(&model), true).unwrap(),
    ));
    let vad = || Box::new(SileroFrameVad::load(&models.join("ggml-silero-v6.2.0.bin")).unwrap());
    let base = manifest.parent().unwrap();
    let mut totals = [(ErrorCount::default(), Punctuation::default(), 0usize); 2];
    let text = std::fs::read_to_string(manifest).unwrap();
    for line in text.lines().take(limit) {
        let (wav, reference) = line.split_once('\t').unwrap();
        let pcm = with_pause(read_wav_16k_mono(&base.join(wav)).unwrap());
        // Before: every pause ends an utterance. Now: short fragments held.
        let before = UtteranceConfig {
            long_pause_ms: 0,
            ..Default::default()
        };
        for (k, utterance) in [before, UtteranceConfig::default()].into_iter().enumerate() {
            let finals = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&finals);
            LiveSession::start(
                // In real time, so the check at each pause can end it early.
                Box::new(PcmSource::new(pcm.clone()).realtime()),
                vad(),
                Arc::clone(&worker),
                LiveConfig {
                    utterance,
                    show_preview: false,
                    context: fennec::models::takes_context(&model).then(String::new),
                    ..Default::default()
                },
                Arc::new(move |e| {
                    if let LiveEvent::Final { text, .. } = e {
                        sink.lock().unwrap().push(text);
                    }
                }),
            )
            .wait();
            let finals = finals.lock().unwrap();
            let hyp = finals.join(" ");
            totals[k].0.add(word_errors(reference, &hyp));
            totals[k].1.add(punctuation(reference, &hyp));
            totals[k].2 += finals.len();
        }
    }
    for (name, (wer, p, n)) in ["split (before)", "held (now)"].iter().zip(totals) {
        println!(
            "{name:20} WER {:.1}%  comma F1 {:.2}  sentence-end F1 {:.2}  ({n} utterances)",
            wer.rate() * 100.0,
            p.commas.f1(),
            p.ends.f1()
        );
    }
}

/// The clip with 0.7 s of silence at its quietest 30 ms between 35 % and
/// 65 % of the way through: a pause in the middle of the sentence.
fn with_pause(pcm: Vec<f32>) -> Vec<f32> {
    let frame = 480;
    let (from, to) = (pcm.len() * 35 / 100 / frame, pcm.len() * 65 / 100 / frame);
    let quietest = (from..to.max(from + 1))
        .min_by(|&a, &b| {
            let e = |i: usize| {
                pcm[i * frame..((i + 1) * frame).min(pcm.len())]
                    .iter()
                    .map(|s| s * s)
                    .sum::<f32>()
            };
            e(a).total_cmp(&e(b))
        })
        .unwrap_or(from);
    let at = quietest * frame;
    let mut out = pcm[..at].to_vec();
    out.extend(std::iter::repeat_n(0.0, 16_000 * 7 / 10));
    out.extend_from_slice(&pcm[at..]);
    // Trailing silence so the last utterance ends.
    out.extend(std::iter::repeat_n(0.0, 16_000 * 2));
    out
}
