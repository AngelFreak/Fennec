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
