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
        // The sound server's default buffer is about 170 ms here, and
        // starting it blocked for 2 s; 10 ms starts in about 40 ms.
        let small = small_buffer(config.sample_rate(), config.buffer_size());
        open_stream(&device, &config, small, gain, &tx).or_else(|e| {
            if small == cpal::BufferSize::Default {
                return Err(e);
            }
            tracing::warn!("the microphone refused a 10 ms buffer ({e}); using its default");
            open_stream(&device, &config, cpal::BufferSize::Default, gain, &tx)
        })
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

/// Buffers of about 10 ms, within what the device allows.
fn small_buffer(rate: u32, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match *supported {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed((rate / 100).clamp(min, max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

/// Starts `device` delivering mono 16 kHz chunks to `tx`.
fn open_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    buffer_size: cpal::BufferSize,
    gain: f32,
    tx: &Sender<Result<Vec<f32>, CaptureError>>,
) -> Result<cpal::Stream, CaptureError> {
    let channels = config.channels() as usize;
    let mut resampler = StreamResampler::new(config.sample_rate()).map_err(CaptureError::Open)?;
    let stream_config = cpal::StreamConfig {
        buffer_size,
        ..(*config).into()
    };
    let data_tx = tx.clone();
    let err_tx = tx.clone();
    let mut cleanup = MicCleanup::default();
    let mut send = move |mut mono: Vec<f32>| {
        apply_gain(&mut mono, gain);
        let out = resampler.push(&mono);
        if out.is_empty() {
            return;
        }
        if let Some(out) = cleanup.process(out) {
            // A full channel means the consumer is gone or stalled; drop audio rather than block the callback.
            let _ = data_tx.try_send(Ok(out));
        }
    };
    let err_fn = move |e: cpal::Error| {
        let _ = err_tx.try_send(Err(CaptureError::Stream(e.to_string())));
    };
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &_| send(downmix(data, channels, |s| s)),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &_| send(downmix(data, channels, |s| f32::from(s) / 32768.0)),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_input_stream(
            stream_config,
            move |data: &[i32], _: &_| send(downmix(data, channels, |s| s as f32 / 2_147_483_648.0)),
            err_fn,
            None,
        ),
        other => return Err(CaptureError::Open(format!("unsupported sample format {other}"))),
    }
    .map_err(|e| CaptureError::Open(e.to_string()))?;
    stream.play().map_err(|e| CaptureError::Open(e.to_string()))?;
    Ok(stream)
}

/// What the microphone's audio goes through before anyone sees it.
#[derive(Debug, Default)]
pub struct MicCleanup {
    settle: Settle,
    high_pass: HighPass,
}

impl MicCleanup {
    /// The block, or `None` while the microphone is still pinned.
    pub fn process(&mut self, mut block: Vec<f32>) -> Option<Vec<f32>> {
        let open = self.settle.pass(&block);
        // Filtered from the first sample, so the filter has settled by the
        // time the gate opens.
        self.high_pass.process(&mut block);
        open.then_some(block)
    }
}

/// Holds microphone audio back while it is pinned. This laptop's
/// microphone (like many) starts at -1 for about 350 ms, then carries an
/// offset that fades over the next 800 ms; the high-pass filter removes the
/// offset, so audio can come through as soon as it stops clipping, or after
/// 1.5 s at the latest.
#[derive(Debug, Default)]
pub struct Settle {
    held: usize,
    open: bool,
}

impl Settle {
    const MAX_HELD: usize = SAMPLE_RATE as usize * 3 / 2;

    pub fn pass(&mut self, block: &[f32]) -> bool {
        if self.open {
            return true;
        }
        let peak = block.iter().fold(0f32, |a, s| a.max(s.abs()));
        self.held += block.len();
        self.open = peak < 0.9 || self.held >= Self::MAX_HELD;
        if self.open {
            tracing::debug!(held_ms = self.held / 16, "microphone settled");
        }
        self.open
    }
}

/// Second-order Butterworth high-pass at 60 Hz for 16 kHz audio: removes
/// offsets and rumble, below the lowest voices.
#[derive(Debug)]
struct HighPass {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Default for HighPass {
    fn default() -> Self {
        let k = (std::f64::consts::PI * 60.0 / f64::from(SAMPLE_RATE)).tan();
        let q = std::f64::consts::SQRT_2;
        let norm = 1.0 / (1.0 + q * k + k * k);
        Self {
            b: [norm, -2.0 * norm, norm],
            a: [2.0 * (k * k - 1.0) * norm, (1.0 - q * k + k * k) * norm],
            z: [0.0; 2],
        }
    }
}

impl HighPass {
    fn process(&mut self, block: &mut [f32]) {
        for s in block {
            let x = f64::from(*s);
            let y = self.b[0] * x + self.z[0];
            self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
            self.z[1] = self.b[2] * x - self.a[1] * y;
            *s = y as f32;
        }
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
    fn the_microphone_asks_for_ten_millisecond_buffers() {
        let any = cpal::SupportedBufferSize::Range { min: 1, max: 524_288 };
        assert_eq!(small_buffer(48_000, &any), cpal::BufferSize::Fixed(480));
        assert_eq!(small_buffer(44_100, &any), cpal::BufferSize::Fixed(441));
    }

    #[test]
    fn the_buffer_stays_inside_what_the_device_allows() {
        let range = cpal::SupportedBufferSize::Range { min: 1024, max: 4096 };
        assert_eq!(small_buffer(48_000, &range), cpal::BufferSize::Fixed(1024));
        assert_eq!(
            small_buffer(48_000, &cpal::SupportedBufferSize::Unknown),
            cpal::BufferSize::Default
        );
    }

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

    /// The first two seconds of this laptop's microphone: pinned at -1 for
    /// 350 ms, then an offset that fades over the next 800 ms.
    fn mic_start() -> Vec<f32> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/mic-start-16k.wav");
        crate::audio::read_wav_16k_mono(&path).unwrap()
    }

    /// Feeds `pcm` in 344-sample blocks, as the resampler delivers them;
    /// returns when audio started coming through, and what came through.
    fn clean(pcm: &[f32]) -> (usize, Vec<f32>) {
        let mut mic = MicCleanup::default();
        let mut first = None;
        let mut out = Vec::new();
        for (i, block) in pcm.chunks(344).enumerate() {
            if let Some(b) = mic.process(block.to_vec()) {
                first.get_or_insert(i * 344);
                out.extend(b);
            }
        }
        (first.unwrap_or(pcm.len()), out)
    }

    #[test]
    fn audio_comes_through_as_soon_as_the_microphone_stops_clipping() {
        let (first, _) = clean(&mic_start());
        assert!(first < SAMPLE_RATE as usize / 2, "after {} ms", first / 16);
    }

    #[test]
    fn the_fading_offset_does_not_reach_the_meter_or_the_voice_detector() {
        let (_, out) = clean(&mic_start());
        for (i, w) in out.as_chunks::<800>().0.iter().enumerate() {
            let rms = crate::utterance::rms(w);
            // Room noise; speech is 0.03 and up.
            assert!(rms < 0.02, "window {i}: {rms}");
        }
    }

    #[test]
    fn speech_passes_the_filter_unchanged_in_level() {
        let speech: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.1).sin() * 0.3).collect();
        let (first, out) = clean(&speech);
        assert_eq!(first, 0);
        let rms = crate::utterance::rms(&out[1600..]);
        assert!((rms - 0.3 / 2f32.sqrt()).abs() < 0.01, "{rms}");
    }

    #[test]
    fn a_microphone_that_never_settles_opens_after_a_second_and_a_half() {
        let mut gate = Settle::default();
        let pinned = vec![-1.0f32; 1600];
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
