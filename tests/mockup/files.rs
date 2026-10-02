//! Files: three recordings in the queue, the interview still transcribing.

use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use fennec::ui::Nav;
use fennec::vad::{SpeechDetector, VadError};

use crate::{Scene, pump_until};

/// Peak level of the interview recording; the scene engine tells files apart by it.
pub const INTERVIEW_LEVEL: f32 = 0.3;

pub const INTERVIEW_LINES: [&str; 5] = [
    "Vi startede med at kortlægge, hvordan sagerne faktisk flyttede sig mellem afdelingerne.",
    "Det viste sig, at meget af tiden gik med at vente på svar fra andre.",
    "Så vi besluttede at samle det hele i ét system i stedet for at sende mails frem og tilbage.",
    "Den første måned var svær. Folk var vant til deres egne regneark, og det tog tid at vænne sig til den nye måde at arbejde på.",
    "Men efter et par uger kunne vi begynde at se, hvor sagerne hobede sig op. Og det var egentlig dér, det gav mening for de fleste.",
];

/// One second of speech in every three: each a piece of its own, and the
/// two-second pauses start new paragraphs.
pub struct EverySecond;
impl SpeechDetector for EverySecond {
    fn speech(&mut self, pcm: &[f32]) -> Result<Vec<Range<usize>>, VadError> {
        Ok((0..pcm.len() / 48_000)
            .map(|i| i * 48_000..i * 48_000 + 16_000)
            .collect())
    }
}

fn recording(path: &Path, pieces: usize, level: f32) {
    let mut pcm = Vec::new();
    for _ in 0..pieces {
        pcm.extend((0..16_000).map(|i| (i as f32 * 0.07).sin() * level));
        pcm.extend(std::iter::repeat_n(0.0, 32_000));
    }
    fennec::audio::write_wav_16k_mono(path, &pcm).unwrap();
}

pub fn capture(s: &Scene, root: &Path) {
    let dir = root.join("recordings");
    std::fs::create_dir_all(&dir).unwrap();
    let car = dir.join("diktat_bil.wav");
    let interview = dir.join("interview_afdelingsleder.wav");
    let meeting = dir.join("moede_2026-09-29.wav");
    recording(&car, 2, 0.2);
    // It stalls after five of eight pieces, as the mockup catches it.
    recording(&interview, 8, INTERVIEW_LEVEL);
    recording(&meeting, 3, 0.25);

    s.w.sidebar.go(Nav::Files);
    s.w.files.add_paths(&[car, interview, meeting]);
    let stalled = pump_until(Duration::from_secs(20), || {
        s.w.files.statuses().iter().any(|(_, st)| st.ends_with('%'))
    });
    if !stalled {
        println!(
            "FAILED the interview did not stall mid-way: {:?}",
            s.w.files.statuses()
        );
    }
    s.w.files.select_file("interview_afdelingsleder.wav");
    s.shot("files");
}
