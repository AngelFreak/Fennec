//! Turns a live 16 kHz stream into utterances: speech starts after a short
//! run of voiced frames (with pre-roll so first syllables survive), ends after
//! a pause, and is force-cut at the quietest point before Whisper's limit.

use std::collections::VecDeque;
use std::path::Path;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

use crate::engine::SAMPLE_RATE;
use crate::vad::VadError;

/// Samples per VAD frame (Silero's window at 16 kHz, 32 ms).
pub const FRAME: usize = 512;
const FRAME_MS: f64 = FRAME as f64 * 1000.0 / SAMPLE_RATE as f64;

/// Speech probability per frame.
pub trait FrameVad: Send {
    fn probability(&mut self, frame: &[f32]) -> f32;
}

/// Loudness-based detector: deterministic, used in tests and as a fallback
/// when the Silero model is missing.
pub struct EnergyVad {
    pub threshold_rms: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self { threshold_rms: 0.02 }
    }
}

impl FrameVad for EnergyVad {
    fn probability(&mut self, frame: &[f32]) -> f32 {
        if rms(frame) >= self.threshold_rms {
            1.0
        } else {
            0.0
        }
    }
}

/// Silero over a sliding half-second window, so the model sees context.
pub struct SileroFrameVad {
    ctx: WhisperVadContext,
    window: VecDeque<f32>,
}

impl SileroFrameVad {
    const WINDOW_FRAMES: usize = 16;

    pub fn load(model: &Path) -> Result<Self, VadError> {
        if !model.exists() {
            return Err(VadError::ModelMissing(model.to_path_buf()));
        }
        let mut params = WhisperVadContextParams::new();
        params.set_n_threads(1);
        params.set_use_gpu(false);
        let ctx = WhisperVadContext::new(&model.to_string_lossy(), params)?;
        Ok(Self {
            ctx,
            window: VecDeque::with_capacity(FRAME * Self::WINDOW_FRAMES),
        })
    }
}

impl FrameVad for SileroFrameVad {
    fn probability(&mut self, frame: &[f32]) -> f32 {
        self.window.extend(frame.iter().copied());
        while self.window.len() > FRAME * Self::WINDOW_FRAMES {
            self.window.pop_front();
        }
        let samples: Vec<f32> = self.window.iter().copied().collect();
        match self.ctx.detect_speech(&samples) {
            Ok(()) => self.ctx.probabilities().last().copied().unwrap_or(0.0),
            Err(e) => {
                tracing::warn!("VAD frame failed: {e}");
                0.0
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct UtteranceConfig {
    pub threshold: f32,
    /// Voiced time needed before an utterance starts.
    pub start_ms: u32,
    /// Silence that ends an utterance.
    pub pause_ms: u32,
    /// Audio kept from before the detected start.
    pub preroll_ms: u32,
    /// Utterances are cut before this length (Whisper's window is 30 s).
    pub max_ms: u32,
    /// The forced cut lands on the quietest frame within this tail.
    pub cut_search_ms: u32,
    /// Audio in an utterance before it is first offered for a preview.
    pub first_partial_ms: u32,
    /// How often a growing utterance is offered for a preview after that.
    pub partial_every_ms: u32,
}

impl Default for UtteranceConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            start_ms: 100,
            pause_ms: 600,
            preroll_ms: 200,
            max_ms: 25_000,
            cut_search_ms: 2_000,
            // A preview of a short clip takes about 0.4 s on a laptop GPU.
            first_partial_ms: 700,
            partial_every_ms: 1_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// Position in the stream, in samples.
    pub start_sample: u64,
    pub samples: Vec<f32>,
    /// Sequence number, shared with this utterance's partials.
    pub id: u64,
}

impl Utterance {
    pub fn start_ms(&self) -> i64 {
        (self.start_sample * 1000 / SAMPLE_RATE as u64) as i64
    }
    pub fn end_ms(&self) -> i64 {
        ((self.start_sample + self.samples.len() as u64) * 1000 / SAMPLE_RATE as u64) as i64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UtteranceEvent {
    Started {
        id: u64,
    },
    /// The utterance so far, for a preview.
    Partial(Utterance),
    Final(Utterance),
}

pub struct UtteranceBuilder {
    cfg: UtteranceConfig,
    vad: Box<dyn FrameVad>,
    pending: Vec<f32>,
    consumed: u64,
    preroll: VecDeque<f32>,
    speaking: bool,
    current: Vec<f32>,
    current_start: u64,
    frame_rms: Vec<f32>,
    voiced_run: u32,
    silent_run: u32,
    /// Length of `current` at which the next preview is offered.
    next_partial: usize,
    next_id: u64,
}

impl UtteranceBuilder {
    pub fn new(cfg: UtteranceConfig, vad: Box<dyn FrameVad>) -> Self {
        Self {
            cfg,
            vad,
            pending: Vec::new(),
            consumed: 0,
            preroll: VecDeque::new(),
            speaking: false,
            current: Vec::new(),
            current_start: 0,
            frame_rms: Vec::new(),
            voiced_run: 0,
            silent_run: 0,
            next_partial: 0,
            next_id: 0,
        }
    }

    fn samples(ms: u32) -> usize {
        ms as usize * SAMPLE_RATE as usize / 1000
    }

    fn frames(ms: u32) -> u32 {
        (f64::from(ms) / FRAME_MS).ceil() as u32
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<UtteranceEvent> {
        self.pending.extend_from_slice(samples);
        let mut events = Vec::new();
        let whole = self.pending.len() / FRAME * FRAME;
        let frames: Vec<f32> = self.pending.drain(..whole).collect();
        for frame in frames.as_chunks::<FRAME>().0 {
            self.frame(frame, &mut events);
        }
        events
    }

    /// Ends the stream: an utterance in progress becomes final.
    pub fn flush(&mut self) -> Vec<UtteranceEvent> {
        let mut events = Vec::new();
        if self.speaking {
            let rest = std::mem::take(&mut self.pending);
            self.current.extend(rest);
            self.finish(&mut events);
        }
        events
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    fn frame(&mut self, frame: &[f32], events: &mut Vec<UtteranceEvent>) {
        let voiced = self.vad.probability(frame) >= self.cfg.threshold;
        self.consumed += FRAME as u64;
        if !self.speaking {
            self.preroll.extend(frame.iter().copied());
            let keep = (Self::frames(self.cfg.preroll_ms + self.cfg.start_ms) as usize) * FRAME;
            while self.preroll.len() > keep {
                self.preroll.pop_front();
            }
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= Self::frames(self.cfg.start_ms).max(1) {
                self.speaking = true;
                self.current = self.preroll.drain(..).collect();
                self.current_start = self.consumed - self.current.len() as u64;
                self.frame_rms = self.current.chunks(FRAME).map(rms).collect();
                self.silent_run = 0;
                self.next_partial = Self::samples(self.cfg.first_partial_ms);
                events.push(UtteranceEvent::Started { id: self.next_id });
            }
            return;
        }

        self.current.extend_from_slice(frame);
        self.frame_rms.push(rms(frame));
        self.silent_run = if voiced { 0 } else { self.silent_run + 1 };

        if self.silent_run >= Self::frames(self.cfg.pause_ms) {
            // Keep a little of the trailing silence; drop the rest.
            let keep_silence = Self::frames(200).min(self.silent_run) as usize;
            let drop = (self.silent_run as usize - keep_silence) * FRAME;
            self.current.truncate(self.current.len().saturating_sub(drop));
            self.finish(events);
            return;
        }
        if self.current.len() >= self.cfg.max_ms as usize * SAMPLE_RATE as usize / 1000 {
            self.force_cut(events);
            return;
        }
        // Not in a pause: the final may be on its way, and a preview
        // running then would hold it up.
        if self.current.len() >= self.next_partial && voiced {
            self.next_partial += Self::samples(self.cfg.partial_every_ms);
            if self.next_partial <= self.current.len() {
                self.next_partial = self.current.len() + Self::samples(self.cfg.partial_every_ms);
            }
            events.push(UtteranceEvent::Partial(Utterance {
                start_sample: self.current_start,
                samples: self.current.clone(),
                id: self.next_id,
            }));
        }
    }

    fn finish(&mut self, events: &mut Vec<UtteranceEvent>) {
        let samples = std::mem::take(&mut self.current);
        events.push(UtteranceEvent::Final(Utterance {
            start_sample: self.current_start,
            samples,
            id: self.next_id,
        }));
        self.next_id += 1;
        self.speaking = false;
        self.voiced_run = 0;
        self.frame_rms.clear();
        self.preroll.clear();
    }

    /// Cuts at the quietest frame in the search window and keeps speaking.
    fn force_cut(&mut self, events: &mut Vec<UtteranceEvent>) {
        let n = self.frame_rms.len();
        let search = (Self::frames(self.cfg.cut_search_ms) as usize).min(n);
        let quietest = (n - search..n)
            .min_by(|&a, &b| self.frame_rms[a].total_cmp(&self.frame_rms[b]))
            .unwrap_or(n - 1);
        let cut = ((quietest + 1) * FRAME).min(self.current.len());
        let rest = self.current.split_off(cut);
        let rest_rms = self.frame_rms.split_off(quietest + 1);
        let head = std::mem::replace(&mut self.current, rest);
        events.push(UtteranceEvent::Final(Utterance {
            start_sample: self.current_start,
            samples: head,
            id: self.next_id,
        }));
        self.next_id += 1;
        self.current_start += cut as u64;
        self.frame_rms = rest_rms;
        self.next_partial = self.current.len() + Self::samples(self.cfg.first_partial_ms);
        events.push(UtteranceEvent::Started { id: self.next_id });
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = SAMPLE_RATE as usize;

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * SR as f32) as usize)
            .map(|i| (i as f32 * 0.07).sin() * 0.3)
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * SR as f32) as usize]
    }

    fn builder(cfg: UtteranceConfig) -> UtteranceBuilder {
        UtteranceBuilder::new(cfg, Box::new(EnergyVad::default()))
    }

    fn finals(events: &[UtteranceEvent]) -> Vec<&Utterance> {
        events
            .iter()
            .filter_map(|e| {
                if let UtteranceEvent::Final(u) = e {
                    Some(u)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn speech_between_pauses_becomes_one_utterance_with_preroll() {
        let mut b = builder(UtteranceConfig::default());
        let mut ev = b.push(&silence(1.0));
        ev.extend(b.push(&tone(2.0)));
        ev.extend(b.push(&silence(1.0)));
        let f = finals(&ev);
        assert_eq!(f.len(), 1, "{ev:?}");
        // Starts just before the tone (pre-roll), ends a little after it.
        assert!(
            (750..=1000).contains(&f[0].start_ms()),
            "start {}",
            f[0].start_ms()
        );
        assert!((3000..=3300).contains(&f[0].end_ms()), "end {}", f[0].end_ms());
        assert!(matches!(ev[0], UtteranceEvent::Started { id: 0 }));
    }

    #[test]
    fn two_utterances_get_increasing_ids() {
        let mut b = builder(UtteranceConfig::default());
        let mut ev = b.push(&tone(1.0));
        ev.extend(b.push(&silence(1.0)));
        ev.extend(b.push(&tone(1.0)));
        ev.extend(b.push(&silence(1.0)));
        let ids: Vec<u64> = finals(&ev).iter().map(|u| u.id).collect();
        assert_eq!(ids, [0, 1]);
    }

    #[test]
    fn short_pauses_inside_speech_do_not_end_the_utterance() {
        let mut b = builder(UtteranceConfig::default());
        let mut ev = b.push(&tone(1.0));
        ev.extend(b.push(&silence(0.3)));
        ev.extend(b.push(&tone(1.0)));
        ev.extend(b.push(&silence(1.0)));
        assert_eq!(finals(&ev).len(), 1);
    }

    #[test]
    fn a_click_shorter_than_the_start_threshold_is_ignored() {
        let mut b = builder(UtteranceConfig {
            start_ms: 200,
            ..Default::default()
        });
        let mut ev = b.push(&tone(0.05));
        ev.extend(b.push(&silence(1.0)));
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn long_speech_is_cut_at_the_quietest_point_and_continues() {
        let cfg = UtteranceConfig {
            max_ms: 3_000,
            cut_search_ms: 1_000,
            ..Default::default()
        };
        let mut b = builder(cfg);
        let mut speech = tone(2.4);
        // A quieter (but still voiced) dip at ~2.5 s is the natural cut point.
        speech.extend(tone(0.1).iter().map(|s| s * 0.2));
        speech.extend(tone(2.0));
        let mut ev = b.push(&speech);
        ev.extend(b.push(&silence(1.0)));
        let f = finals(&ev);
        assert_eq!(f.len(), 2, "{ev:?}");
        assert!(f[0].samples.len() <= 3 * SR, "first part under the limit");
        let cut_ms = f[0].end_ms();
        assert!((2_400..=2_600).contains(&cut_ms), "cut at {cut_ms} ms");
        assert_eq!(
            f[1].start_sample,
            f[0].start_sample + f[0].samples.len() as u64,
            "no audio lost"
        );
    }

    #[test]
    fn partials_are_offered_while_speaking() {
        let mut b = builder(UtteranceConfig {
            partial_every_ms: 500,
            ..Default::default()
        });
        let ev = b.push(&tone(1.8));
        let partials = ev
            .iter()
            .filter(|e| matches!(e, UtteranceEvent::Partial(_)))
            .count();
        assert_eq!(partials, 3, "{ev:?}");
    }

    fn partials(events: &[UtteranceEvent]) -> Vec<&Utterance> {
        events
            .iter()
            .filter_map(|e| match e {
                UtteranceEvent::Partial(u) => Some(u),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_first_preview_comes_early_and_then_every_second() {
        let mut b = builder(UtteranceConfig::default());
        let ev = b.push(&tone(3.0));
        let secs: Vec<f32> = partials(&ev)
            .iter()
            .map(|u| u.samples.len() as f32 / SR as f32)
            .collect();
        assert_eq!(secs.len(), 3, "{secs:?}");
        for (got, want) in secs.iter().zip([0.7, 1.7, 2.7]) {
            assert!((got - want).abs() < 0.05, "{secs:?}");
        }
    }

    #[test]
    fn no_preview_starts_in_a_pause_where_the_final_may_be_coming() {
        let mut b = builder(UtteranceConfig::default());
        let mut audio = tone(0.5);
        audio.extend(silence(0.5));
        audio.extend(tone(1.5));
        let ev = b.push(&audio);
        let p = partials(&ev);
        assert!(!p.is_empty());
        for u in p {
            let tail = &u.samples[u.samples.len() - SR / 10..];
            assert!(
                rms(tail) > 0.01,
                "a preview ended in silence at {} samples",
                u.samples.len()
            );
        }
    }

    #[test]
    fn flush_finishes_speech_in_progress() {
        let mut b = builder(UtteranceConfig::default());
        b.push(&tone(1.0));
        assert!(b.is_speaking());
        assert_eq!(finals(&b.flush()).len(), 1);
        assert!(b.flush().is_empty());
    }
}
