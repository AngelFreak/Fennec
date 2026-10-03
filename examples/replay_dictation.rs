//! Replays a saved dictation through the live pipeline with the real Edda
//! model (GPU) and Silero, printing each event. With a number, it plays in
//! real time and presses Stop after that many seconds, like the app does.
//!
//!   cargo run --release --features vulkan --example replay_dictation -- \
//!       ~/.local/share/fennec/audio/doc1-....wav [stop-after-seconds]
//! NOPREVIEW=1 turns the live preview off. PUNCT=1 punctuates the text after
//! each sentence and gives that to the model as context, as the app does.
use fennec::audio::{capture::PcmSource, read_wav_16k_mono};
use fennec::engine::load_engine;
use fennec::live::{LiveConfig, LiveEvent, LiveSession};
use fennec::utterance::SileroFrameVad;
use fennec::worker::EngineWorker;
use std::sync::Arc;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pcm = read_wav_16k_mono(std::path::Path::new(&a[1])).unwrap();
    let stop_after: Option<f64> = a.get(2).and_then(|s| s.parse().ok());
    let models = fennec::config::Paths::user().models();
    let model = fennec::config::Settings::default().model;
    let engine = load_engine(&models.join(&model), true).unwrap();
    let vad = SileroFrameVad::load(&models.join("ggml-silero-v6.2.0.bin")).unwrap();
    let worker = Arc::new(EngineWorker::spawn(engine));
    let t0 = std::time::Instant::now();
    let punctuator = std::env::var_os("PUNCT").map(|_| {
        Arc::new(fennec::punctuation::Punctuator::load(&models.join(fennec::punctuation::DIR)).unwrap())
    });
    let handle: Arc<std::sync::Mutex<Option<fennec::worker::Context>>> = Arc::default();
    let so_far = Arc::new(std::sync::Mutex::new(String::new()));
    let (sink_handle, sink_text) = (Arc::clone(&handle), Arc::clone(&so_far));
    let session = LiveSession::start(
        Box::new(if stop_after.is_some() {
            PcmSource::new(pcm).realtime()
        } else {
            PcmSource::new(pcm)
        }),
        Box::new(vad),
        worker,
        LiveConfig {
            show_preview: std::env::var_os("NOPREVIEW").is_none(),
            context: fennec::models::takes_context(&model).then(String::new),
            ..LiveConfig::default()
        },
        Arc::new(move |e| match e {
            LiveEvent::Level(_) => {}
            e => {
                if let (LiveEvent::Final { text, .. }, Some(p)) = (&e, &punctuator) {
                    let mut t = sink_text.lock().unwrap();
                    t.push(' ');
                    t.push_str(text);
                    let punctuated = fennec::punctuation::punctuate(&**p, t.trim(), true).unwrap();
                    if let Some(c) = &*sink_handle.lock().unwrap() {
                        c.set(&punctuated);
                    }
                }
                println!("{:6.1}s {e:?}", t0.elapsed().as_secs_f64());
            }
        }),
    );
    *handle.lock().unwrap() = session.context();
    match stop_after {
        Some(s) => {
            std::thread::sleep(std::time::Duration::from_secs_f64(s));
            println!("-- stop pressed");
            session.stop();
        }
        None => session.wait(),
    }
}
