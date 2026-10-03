//! Is the microphone level right? Used by the microphone test in Settings
//! and the clipping warning during dictation.

/// Peak at or above this counts as clipping.
pub const CLIP: f32 = 0.99;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing came in at all: wrong input, or muted.
    Silent,
    TooQuiet,
    Good,
    /// The signal hit the ceiling; speech will be distorted.
    TooLoud,
}

impl Verdict {
    pub fn message(self) -> &'static str {
        match self {
            Verdict::Silent => "Nothing was heard. Check that the right microphone is chosen and not muted.",
            Verdict::TooQuiet => "Too quiet. Speak closer or raise the input volume.",
            Verdict::Good => "Good level.",
            Verdict::TooLoud => {
                "Too loud: the sound clips and speech will be distorted. Lower the input volume."
            }
        }
    }
}

/// Cuts audio into 50 ms windows, whatever size it arrives in, so the level
/// meter moves at a steady 20 readings a second.
#[derive(Debug, Default)]
pub struct LevelWindow {
    sum_sq: f64,
    n: usize,
}

impl LevelWindow {
    const LEN: usize = crate::engine::SAMPLE_RATE as usize / 20;

    /// The RMS of each window this chunk completes.
    pub fn push(&mut self, chunk: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        for s in chunk {
            self.sum_sq += f64::from(*s) * f64::from(*s);
            self.n += 1;
            if self.n == Self::LEN {
                out.push((self.sum_sq / Self::LEN as f64).sqrt() as f32);
                *self = Self::default();
            }
        }
        out
    }
}

/// Turns 50 ms levels into bar heights for the meter. Heights count
/// decibels above the room's noise, so a quiet room shows low bars and
/// speech moves them, whatever the microphone's own hiss. The noise floor
/// drops to quieter readings quickly and creeps up 1 dB a second.
#[derive(Debug, Default)]
pub struct Meter {
    floor: Option<f32>,
}

impl Meter {
    /// Decibels above the floor that fill a bar.
    const SPAN_DB: f32 = 20.0;
    /// 1 dB a second at 20 readings a second.
    const RISE: f32 = 1.005_773;

    /// Height from 0 to 1 for one level reading.
    pub fn height(&mut self, rms: f32) -> f32 {
        let rms = rms.max(1e-5);
        let floor = match self.floor {
            None => rms,
            Some(f) if rms < f => f + (rms - f) * 0.3,
            Some(f) => f * Self::RISE,
        };
        self.floor = Some(floor);
        (20.0 * (rms / floor).log10() / Self::SPAN_DB).clamp(0.0, 1.0)
    }
}

/// Peak and RMS of one block of samples.
pub fn measure(samples: &[f32]) -> (f32, f32) {
    let peak = samples.iter().fold(0f32, |a, s| a.max(s.abs()));
    (peak, crate::utterance::rms(samples))
}

/// Judges a test recording from its per-block (peak, rms) pairs. The level
/// of speech is taken from the loudest tenth of blocks, so pauses do not
/// make a good level look quiet.
pub fn assess(blocks: &[(f32, f32)]) -> Verdict {
    if blocks.is_empty() {
        return Verdict::Silent;
    }
    let clipped = blocks.iter().filter(|(p, _)| *p >= CLIP).count();
    // A single clipped block can be a cough or a knock.
    if clipped * 50 > blocks.len() || clipped >= 3 {
        return Verdict::TooLoud;
    }
    let mut rms: Vec<f32> = blocks.iter().map(|(_, r)| *r).collect();
    rms.sort_by(f32::total_cmp);
    let loud = rms[(rms.len() * 9 / 10).min(rms.len() - 1)];
    if loud < 0.001 {
        Verdict::Silent
    } else if loud < 0.02 {
        Verdict::TooQuiet
    } else {
        Verdict::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Levels measured in 50 ms windows of real dictations on a laptop:
    // the room sits near 0.012 RMS, speech at 0.03–0.1.
    fn settled() -> Meter {
        let mut m = Meter::default();
        for _ in 0..40 {
            m.height(0.012);
        }
        m
    }

    #[test]
    fn room_noise_shows_as_low_bars() {
        assert!(settled().height(0.012) < 0.1);
    }

    #[test]
    fn speech_moves_the_bars_and_loud_speech_fills_them() {
        let mut m = settled();
        let normal = m.height(0.045);
        assert!((0.4..0.75).contains(&normal), "{normal}");
        assert!(m.height(0.1) > 0.85);
    }

    #[test]
    fn a_noisier_room_settles_back_to_low_bars() {
        let mut m = settled();
        for _ in 0..20 * 30 {
            m.height(0.04);
        }
        assert!(m.height(0.04) < 0.2);
    }

    #[test]
    fn levels_come_in_fifty_millisecond_windows() {
        let mut w = LevelWindow::default();
        assert!(w.push(&[0.5; 700]).is_empty());
        let out = w.push(&[0.5; 1700]);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.5).abs() < 1e-6);
    }

    fn blocks(n: usize, peak: f32, rms: f32) -> Vec<(f32, f32)> {
        vec![(peak, rms); n]
    }

    #[test]
    fn digital_silence_is_silent() {
        assert_eq!(assess(&blocks(50, 0.0, 0.0)), Verdict::Silent);
        assert_eq!(assess(&[]), Verdict::Silent);
    }

    #[test]
    fn faint_input_is_too_quiet() {
        assert_eq!(assess(&blocks(50, 0.03, 0.008)), Verdict::TooQuiet);
    }

    #[test]
    fn speech_with_pauses_is_good() {
        let mut b = blocks(40, 0.05, 0.005);
        b.extend(blocks(10, 0.4, 0.08));
        assert_eq!(assess(&b), Verdict::Good);
    }

    #[test]
    fn repeated_clipping_is_too_loud_but_one_knock_is_not() {
        let mut b = blocks(50, 0.4, 0.08);
        b[10] = (1.0, 0.5);
        assert_eq!(assess(&b), Verdict::Good);
        b[20] = (1.0, 0.5);
        b[30] = (1.0, 0.5);
        assert_eq!(assess(&b), Verdict::TooLoud);
    }

    #[test]
    fn measure_gives_peak_and_rms() {
        let (p, r) = measure(&[0.5, -1.0, 0.5, 0.0]);
        assert_eq!(p, 1.0);
        assert!((r - 0.612).abs() < 0.01);
    }
}
