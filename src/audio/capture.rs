//! Live audio: the microphone (cpal) and an in-memory source for tests.
//! Both yield 16 kHz mono chunks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::engine::SAMPLE_RATE;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no microphone found")]
    NoDevice,
    #[error("microphone {0:?} is not available")]
    DeviceNotFound(String),
    #[error("could not open the microphone: {0}")]
    Open(String),
    #[error("the microphone stopped: {0}")]
    Stream(String),
}

pub trait AudioSource: Send {
    /// The next chunk of 16 kHz mono samples; `Ok(None)` when the source ends.
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, CaptureError>;
}

/// Plays back samples already in memory, in 100 ms chunks.
pub struct PcmSource {
    pcm: Vec<f32>,
    pos: usize,
    realtime: bool,
}

impl PcmSource {
    pub fn new(pcm: Vec<f32>) -> Self {
        Self {
            pcm,
            pos: 0,
            realtime: false,
        }
    }

    /// Paces chunks like a real microphone.
    pub fn realtime(mut self) -> Self {
        self.realtime = true;
        self
    }
}

impl AudioSource for PcmSource {
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, CaptureError> {
        if self.pos >= self.pcm.len() {
            return Ok(None);
        }
        let end = (self.pos + SAMPLE_RATE as usize / 10).min(self.pcm.len());
        let chunk = self.pcm[self.pos..end].to_vec();
        self.pos = end;
        if self.realtime {
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Some(chunk))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// The sound server (PipeWire or PulseAudio, through the PulseAudio
/// protocol) when one runs, else raw ALSA. Raw ALSA reads the hardware with
/// its capture boost and ignores the system input volume, which clipped
/// speech badly (RMS 0.38 against 0.03 through the sound server).
pub(crate) fn audio_host() -> cpal::Host {
    match cpal::host_from_id(cpal::HostId::PulseAudio) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("no sound server ({e}); reading the microphone through ALSA");
            cpal::default_host()
        }
    }
}

/// Monitor sources record what the speakers play, not a microphone.
fn is_monitor(id: &str) -> bool {
    id.ends_with(".monitor")
}

/// Microphones the sound server offers.
pub fn input_devices() -> Vec<InputDevice> {
    let host = audio_host();
    let default_id = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            if is_monitor(&id) {
                return None;
            }
            let name = d
                .description()
                .map(|desc| desc.name().to_string())
                .unwrap_or_else(|_| id.clone());
            Some(InputDevice {
                is_default: Some(&id) == default_id.as_ref(),
                id,
                name,
            })
        })
        .collect()
}

/// The microphone. The cpal stream lives on its own thread (it is not
/// `Send`); samples arrive here already mono and at 16 kHz.
pub struct MicSource {
    rx: Receiver<Result<Vec<f32>, CaptureError>>,
    stop: Arc<AtomicBool>,
}

impl MicSource {
    /// Opens `device_id`, or the default microphone when `None`, amplified
    /// by `gain_db` on top of the system input volume.
    pub fn open(device_id: Option<&str>, gain_db: f32) -> Result<Self, CaptureError> {
        let (tx, rx) = bounded(64);
        let (ready_tx, ready_rx) = bounded(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let wanted = device_id.map(str::to_string);
        std::thread::Builder::new()
            .name("fennec-mic".into())
            .spawn(move || mic_thread(wanted, db_to_gain(gain_db), tx, ready_tx, stop_thread))
            .map_err(|e| CaptureError::Open(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { rx, stop }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(CaptureError::Open("the audio system did not respond".into())),
        }
    }
}

impl AudioSource for MicSource {
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, CaptureError> {
        loop {
            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(chunk) => return chunk.map(Some),
                Err(RecvTimeoutError::Timeout) if !self.stop.load(Ordering::Relaxed) => continue,
                Err(_) => return Ok(None),
            }
        }
    }
}

impl Drop for MicSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn mic_thread(
    wanted: Option<String>,
    gain: f32,
    tx: Sender<Result<Vec<f32>, CaptureError>>,
    ready: Sender<Result<(), CaptureError>>,
    stop: Arc<AtomicBool>,
) {
    let opened = (|| -> Result<cpal::Stream, CaptureError> {
        let host = audio_host();
        let device = match &wanted {
            Some(id) => host
                .input_devices()
                .map_err(|e| CaptureError::Open(e.to_string()))?
                .find(|d| d.id().map(|i| i.to_string()).as_deref() == Ok(id.as_str()))
                .ok_or_else(|| CaptureError::DeviceNotFound(id.clone()))?,
            None => host.default_input_device().ok_or(CaptureError::NoDevice)?,
        };
        let config = device
            .default_input_config()
            .map_err(|e| CaptureError::Open(e.to_string()))?;
        let channels = config.channels() as usize;
        let rate = config.sample_rate();
        let mut resampler = StreamResampler::new(rate).map_err(CaptureError::Open)?;
        let data_tx = tx.clone();
        let err_tx = tx.clone();
        let mut settle = Settle::default();
        let mut send = move |mut mono: Vec<f32>| {
            apply_gain(&mut mono, gain);
            let out = resampler.push(&mono);
            if !out.is_empty() && settle.pass(&out) {
                // A full channel means the consumer is gone or stalled; drop audio rather than block the callback.
                let _ = data_tx.try_send(Ok(out));
            }
        };
        let err_fn = move |e: cpal::Error| {
            let _ = err_tx.try_send(Err(CaptureError::Stream(e.to_string())));
        };
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config.into(),
                move |data: &[f32], _: &_| send(downmix(data, channels, |s| s)),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config.into(),
                move |data: &[i16], _: &_| send(downmix(data, channels, |s| f32::from(s) / 32768.0)),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_input_stream(
                config.into(),
                move |data: &[i32], _: &_| send(downmix(data, channels, |s| s as f32 / 2_147_483_648.0)),
                err_fn,
                None,
            ),
            other => return Err(CaptureError::Open(format!("unsupported sample format {other}"))),
        }
        .map_err(|e| CaptureError::Open(e.to_string()))?;
        stream.play().map_err(|e| CaptureError::Open(e.to_string()))?;
        Ok(stream)
    })();
    match opened {
        Ok(stream) => {
            let _ = ready.send(Ok(()));
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(stream);
        }
        Err(e) => {
            let _ = ready.send(Err(e));
        }
    }
}

/// Holds microphone audio back until the input settles. This laptop's
/// microphone (like many) starts pinned near -1 for half a second and
/// drifts back to zero over the next; that read as clipping and as speech.
/// The gate opens on the first 100 ms that averages near zero without
/// clipping, or after 1.5 s at the latest.
#[derive(Debug, Default)]
pub struct Settle {
    held: usize,
    open: bool,
}

impl Settle {
    const MAX_HELD: usize = SAMPLE_RATE as usize * 3 / 2;

    /// Whether `block` (16 kHz mono) should be used.
    pub fn pass(&mut self, block: &[f32]) -> bool {
        if self.open {
            return true;
        }
        let n = block.len().max(1) as f32;
        let mean = block.iter().sum::<f32>() / n;
        let peak = block.iter().fold(0f32, |a, s| a.max(s.abs()));
        self.held += block.len();
        self.open = (mean.abs() < 0.02 && peak < 0.98) || self.held >= Self::MAX_HELD;
        if self.open {
            tracing::debug!(held_ms = self.held / 16, "microphone settled");
        }
        self.open
    }
}

/// Linear factor for a gain in decibels.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Amplifies in place, holding samples to [-1, 1].
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if gain == 1.0 {
        return;
    }
    for s in samples {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

fn downmix<T: Copy>(data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) -> Vec<f32> {
    let channels = channels.max(1);
    data.chunks_exact(channels)
        .map(|f| f.iter().map(|&s| to_f32(s)).sum::<f32>() / channels as f32)
        .collect()
}

/// Incremental resampling to 16 kHz for audio that arrives in pieces.
pub struct StreamResampler {
    inner: Option<Fft<f32>>,
    pending: Vec<f32>,
    out: Vec<f32>,
}

impl StreamResampler {
    pub fn new(rate: u32) -> Result<Self, String> {
        let inner = if rate == SAMPLE_RATE {
            None
        } else {
            Some(
                Fft::<f32>::new(rate as usize, SAMPLE_RATE as usize, 1024, 1, FixedSync::Input)
                    .map_err(|e| e.to_string())?,
            )
        };
        Ok(Self {
            inner,
            pending: Vec::new(),
            out: Vec::new(),
        })
    }

    /// Feeds samples at the input rate; returns whatever 16 kHz audio is ready.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let Some(r) = &mut self.inner else {
            return samples.to_vec();
        };
        self.pending.extend_from_slice(samples);
        let mut ready = Vec::new();
        loop {
            let need = r.input_frames_next();
            if self.pending.len() < need {
                break;
            }
            self.out.resize(r.output_frames_max(), 0.0);
            let input = InterleavedSlice::new(&self.pending[..need], 1, need).expect("sized above");
            let mut output =
                InterleavedSlice::new_mut(&mut self.out[..], 1, r.output_frames_max()).expect("sized above");
            match r.process_into_buffer(&input, &mut output, None) {
                Ok((used, produced)) => {
                    ready.extend_from_slice(&self.out[..produced]);
                    self.pending.drain(..used);
                }
                Err(e) => {
                    tracing::warn!("resampling a microphone chunk failed: {e}");
                    self.pending.clear();
                    break;
                }
            }
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_source_yields_all_samples_in_order_then_ends() {
        let pcm: Vec<f32> = (0..4000).map(|i| i as f32).collect();
        let mut src = PcmSource::new(pcm.clone());
        let mut got = Vec::new();
        while let Some(c) = src.next_chunk().unwrap() {
            assert!(c.len() <= 1600);
            got.extend(c);
        }
        assert_eq!(got, pcm);
    }

    #[test]
    fn streaming_resampler_converts_48k_to_16k_in_pieces() {
        let mut r = StreamResampler::new(48_000).unwrap();
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin()).collect();
        let out: Vec<f32> = tone.chunks(480).flat_map(|c| r.push(c)).collect();
        // Within one FFT block of a second's worth.
        assert!((15_000..=16_000).contains(&out.len()), "{} samples", out.len());
    }

    #[test]
    fn audio_is_held_back_until_the_microphone_settles() {
        let mut gate = Settle::default();
        let pinned = vec![-0.95f32; 1600];
        let drifting = vec![-0.15f32; 1600];
        let speech: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.3).sin() * 0.4).collect();
        assert!(!gate.pass(&pinned));
        assert!(!gate.pass(&drifting));
        assert!(gate.pass(&speech));
        // Once settled it stays open, even for loud or offset audio.
        assert!(gate.pass(&pinned));
    }

    #[test]
    fn a_microphone_that_never_settles_opens_after_a_second_and_a_half() {
        let mut gate = Settle::default();
        let pinned = vec![-0.95f32; 1600];
        let opened = (0..20).position(|_| gate.pass(&pinned)).unwrap();
        assert_eq!(opened, 14, "the block that completes 1.5 s");
    }

    #[test]
    fn gain_in_decibels_scales_and_holds_the_range() {
        assert!((db_to_gain(6.0) - 1.995).abs() < 0.01);
        assert!((db_to_gain(-20.0) - 0.1).abs() < 1e-6);
        let mut s = vec![0.1, -0.6, 0.9];
        apply_gain(&mut s, 2.0);
        assert_eq!(s, vec![0.2, -1.0, 1.0]);
    }

    #[test]
    fn monitor_sources_are_not_microphones() {
        assert!(is_monitor("alsa_output.pci-0000_c1_00.6.analog-stereo.monitor"));
        assert!(!is_monitor("alsa_input.pci-0000_c1_00.6.analog-stereo"));
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0f32, 0.0, 0.5, 0.5], 2, |s| s), vec![0.5, 0.5]);
    }
}
