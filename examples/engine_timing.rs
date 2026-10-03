//! How long the engine takes per call on this machine, by audio length.
use fennec::engine::{TranscribeOptions, load_engine};
use std::time::Instant;
fn main() {
    let pcm =
        fennec::audio::read_wav_16k_mono(std::path::Path::new(&std::env::args().nth(1).unwrap())).unwrap();
    let models = fennec::config::Paths::user().models();
    let mut e = load_engine(&models.join("edda-v0.1-q5_0.bin"), true).unwrap();
    let start = 9 * 16000; // speech starts about here
    for fast in [true, false] {
        for secs in [1.0f32, 1.5, 3.0, 6.0, 10.0] {
            let clip = &pcm[start..start + (secs * 16000.0) as usize];
            let opts = TranscribeOptions {
                fast,
                ..Default::default()
            };
            e.transcribe(clip, &opts).unwrap(); // warm this window size
            let t = Instant::now();
            let s = e.transcribe(clip, &opts).unwrap();
            let text: String = s.iter().map(|s| s.text.as_str()).collect();
            println!(
                "fast={fast} {secs:>4}s audio: {:>5} ms  {}",
                t.elapsed().as_millis(),
                text.chars().take(60).collect::<String>()
            );
        }
    }
}
