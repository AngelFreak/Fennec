//! Needs a real microphone, so ignored by default:
//!   cargo test --test microphone -- --ignored

use std::time::{Duration, Instant};

use fennec::audio::capture::{AudioSource, MicSource};

#[test]
#[ignore = "needs a microphone"]
fn the_microphone_starts_quickly_and_delivers_small_chunks() {
    let t = Instant::now();
    let mut mic = MicSource::open(None, 0.0).expect("a microphone");
    let opened = t.elapsed();
    assert!(opened < Duration::from_millis(500), "opening took {opened:?}");
    // Skip the settling gate, then look at steady chunks.
    mic.next_chunk().unwrap().unwrap();
    let sizes: Vec<usize> = (0..10)
        .map(|_| mic.next_chunk().unwrap().unwrap().len())
        .collect();
    // 30 ms at 16 kHz: small enough for a lively level meter.
    assert!(sizes.iter().all(|n| *n <= 480), "{sizes:?}");
}
