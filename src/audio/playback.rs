//! Plays decoded 16 kHz mono audio at an adjustable speed, for checking a
//! transcript against its recording.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::engine::SAMPLE_RATE;

/// Where playback is and how fast it goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playhead {
    /// Position in input samples (16 kHz).
    pub pos: f64,
    pub playing: bool,
    pub speed: f64,
}

impl Default for Playhead {
    fn default() -> Self {
        Self {
            pos: 0.0,
            playing: false,
            speed: 1.0,
        }
    }
}

/// Fills `out` (interleaved, `channels` wide) from `samples`, advancing the
/// head by `speed * 16000 / out_rate` input samples per frame. Stops at the end.
pub fn render(samples: &[f32], head: &mut Playhead, out_rate: u32, channels: usize, out: &mut [f32]) {
    let step = head.speed * f64::from(SAMPLE_RATE) / f64::from(out_rate.max(1));
    for frame in out.chunks_mut(channels.max(1)) {
        let i = head.pos as usize;
        let value = if head.playing && i < samples.len() {
            let next = samples.get(i + 1).copied().unwrap_or(samples[i]);
            let t = (head.pos - i as f64) as f32;
            head.pos += step;
            samples[i] + (next - samples[i]) * t
        } else {
            if i >= samples.len() {
                head.playing = false;
            }
            0.0
        };
        frame.fill(value);
    }
}

/// Peak level per bucket, scaled so the loudest bucket is 1.
pub fn peaks(samples: &[f32], buckets: usize) -> Vec<f32> {
    if samples.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let size = samples.len().div_ceil(buckets);
    let raw: Vec<f32> = samples
        .chunks(size)
        .map(|c| c.iter().fold(0.0f32, |m, s| m.max(s.abs())))
        .collect();
    let max = raw.iter().copied().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return raw;
    }
    raw.into_iter().map(|p| p / max).collect()
}

/// Audio output for one recording. The sound device opens on first play.
pub struct Player {
    samples: Arc<Vec<f32>>,
    head: Arc<Mutex<Playhead>>,
    stop: Arc<AtomicBool>,
    opened: AtomicBool,
}

impl Player {
    pub fn new(samples: Arc<Vec<f32>>) -> Self {
        Self {
            samples,
            head: Arc::default(),
            stop: Arc::new(AtomicBool::new(false)),
            opened: AtomicBool::new(false),
        }
    }

    fn with_head<T>(&self, f: impl FnOnce(&mut Playhead) -> T) -> T {
        f(&mut self.head.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn play(&self) {
        self.with_head(|h| {
            if h.pos as usize >= self.samples.len() {
                h.pos = 0.0;
            }
            h.playing = true;
        });
        if !self.opened.swap(true, Ordering::Relaxed) {
            self.open_output();
        }
    }

    pub fn pause(&self) {
        self.with_head(|h| h.playing = false);
    }

    pub fn is_playing(&self) -> bool {
        self.with_head(|h| h.playing)
    }

    pub fn seek_ms(&self, ms: i64) {
        let pos = (ms.max(0) as f64 * f64::from(SAMPLE_RATE) / 1000.0).min(self.samples.len() as f64);
        self.with_head(|h| h.pos = pos);
    }

    pub fn position_ms(&self) -> i64 {
        self.with_head(|h| (h.pos * 1000.0 / f64::from(SAMPLE_RATE)) as i64)
    }

    pub fn duration_ms(&self) -> i64 {
        self.samples.len() as i64 * 1000 / i64::from(SAMPLE_RATE)
    }

    pub fn set_speed(&self, speed: f64) {
        self.with_head(|h| h.speed = speed);
    }

    /// Runs the output stream on its own thread until the player is dropped.
    fn open_output(&self) {
        let samples = Arc::clone(&self.samples);
        let head = Arc::clone(&self.head);
        let stop = Arc::clone(&self.stop);
        let spawned = std::thread::Builder::new()
            .name("fennec-playback".into())
            .spawn(move || {
                let stream = match output_stream(samples, Arc::clone(&head)) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("no audio output for playback: {e}");
                        head.lock().unwrap_or_else(PoisonError::into_inner).playing = false;
                        return;
                    }
                };
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
            });
        if let Err(e) = spawned {
            tracing::warn!("starting playback: {e}");
            self.pause();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn output_stream(samples: Arc<Vec<f32>>, head: Arc<Mutex<Playhead>>) -> Result<cpal::Stream, String> {
    let device = super::capture::audio_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate();
    let err_fn = |e: cpal::Error| tracing::warn!("playback stream: {e}");
    let fill = move |out: &mut [f32]| {
        let mut h = head.lock().unwrap_or_else(PoisonError::into_inner);
        render(&samples, &mut h, rate, channels, out);
    };
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config.into(),
            move |data: &mut [f32], _: &_| fill(data),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let mut buf = Vec::new();
            device.build_output_stream(
                config.into(),
                move |data: &mut [i16], _: &_| {
                    buf.resize(data.len(), 0.0);
                    fill(&mut buf);
                    for (d, s) in data.iter_mut().zip(&buf) {
                        *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_advances_by_speed() {
        let samples = vec![0.5; 16_000];
        let mut head = Playhead {
            playing: true,
            speed: 1.5,
            ..Default::default()
        };
        let mut out = vec![0.0; 200];
        render(&samples, &mut head, 16_000, 2, &mut out);
        assert!((head.pos - 150.0).abs() < 1e-9);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn render_is_silent_when_paused() {
        let samples = vec![0.5; 100];
        let mut head = Playhead::default();
        let mut out = vec![1.0; 10];
        render(&samples, &mut head, 48_000, 1, &mut out);
        assert_eq!(head.pos, 0.0);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn render_stops_at_the_end() {
        let samples = vec![0.5; 4];
        let mut head = Playhead {
            playing: true,
            ..Default::default()
        };
        let mut out = vec![0.0; 8];
        render(&samples, &mut head, 16_000, 1, &mut out);
        assert!(!head.playing);
        assert_eq!(out[7], 0.0);
    }

    #[test]
    fn peaks_are_normalised() {
        let samples = [0.1, -0.2, 0.05, 0.4];
        assert_eq!(peaks(&samples, 2), vec![0.5, 1.0]);
    }

    #[test]
    fn seek_and_position_round_trip() {
        let p = Player::new(Arc::new(vec![0.0; 32_000]));
        p.seek_ms(1500);
        assert_eq!(p.position_ms(), 1500);
        assert_eq!(p.duration_ms(), 2000);
        p.seek_ms(99_000);
        assert_eq!(p.position_ms(), 2000);
    }
}
