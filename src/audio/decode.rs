//! Any audio or video file → 16 kHz mono f32.
//! symphonia handles common audio formats; ffmpeg is the fallback for
//! everything else (video containers, Opus in WebM, …).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::engine::SAMPLE_RATE;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not open {path}: {source}")]
    Open { path: PathBuf, source: std::io::Error },
    #[error("{path}: no audio track could be decoded ({symphonia}; ffmpeg: {ffmpeg})")]
    Unsupported {
        path: PathBuf,
        symphonia: String,
        ffmpeg: String,
    },
    #[error("resampling failed: {0}")]
    Resample(String),
}

/// Decodes `path` to 16 kHz mono samples.
pub fn decode_file(path: &Path) -> Result<Vec<f32>, DecodeError> {
    match decode_with_symphonia(path) {
        Ok((pcm, rate)) => resample_to_16k(&pcm, rate),
        Err(DecodeAttempt::Io(source)) => Err(DecodeError::Open {
            path: path.to_path_buf(),
            source,
        }),
        Err(DecodeAttempt::Unsupported(symphonia)) => {
            decode_with_ffmpeg(path).map_err(|ffmpeg| DecodeError::Unsupported {
                path: path.to_path_buf(),
                symphonia,
                ffmpeg,
            })
        }
    }
}

enum DecodeAttempt {
    Io(std::io::Error),
    Unsupported(String),
}

/// Returns mono samples at the file's own rate.
fn decode_with_symphonia(path: &Path) -> Result<(Vec<f32>, u32), DecodeAttempt> {
    let file = std::fs::File::open(path).map_err(DecodeAttempt::Io)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let unsupported = |e: &dyn std::fmt::Display| DecodeAttempt::Unsupported(e.to_string());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| unsupported(&e))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| DecodeAttempt::Unsupported("no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| DecodeAttempt::Unsupported("no audio codec parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| unsupported(&e))?;

    let mut mono = Vec::new();
    let mut rate = 0;
    let mut scratch: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // A truncated tail ends the stream; keep what decoded.
            Err(SymError::IoError(_)) if !mono.is_empty() => break,
            Err(e) => return Err(unsupported(&e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            // Corrupt packets are skipped, as players do.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(unsupported(&e)),
        };
        let spec = buf.spec();
        rate = spec.rate();
        let channels = spec.channels().count().max(1);
        scratch.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut scratch);
        mono.extend(
            scratch
                .chunks_exact(channels)
                .map(|f| f.iter().sum::<f32>() / channels as f32),
        );
    }
    if mono.is_empty() || rate == 0 {
        return Err(DecodeAttempt::Unsupported("no audio decoded".into()));
    }
    Ok((mono, rate))
}

fn decode_with_ffmpeg(path: &Path) -> Result<Vec<f32>, String> {
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .expect("piped")
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if pcm.is_empty() {
        return Err("no audio stream".into());
    }
    Ok(pcm)
}

/// Resamples mono audio to 16 kHz.
pub fn resample_to_16k(pcm: &[f32], rate: u32) -> Result<Vec<f32>, DecodeError> {
    if rate == SAMPLE_RATE {
        return Ok(pcm.to_vec());
    }
    let err = |e: &dyn std::fmt::Display| DecodeError::Resample(e.to_string());
    let mut resampler = Fft::<f32>::new(rate as usize, SAMPLE_RATE as usize, 1024, 1, FixedSync::Input)
        .map_err(|e| err(&e))?;
    let input = InterleavedOwned::new_from(pcm.to_vec(), 1, pcm.len()).map_err(|e| err(&e))?;
    let needed = resampler.process_all_needed_output_len(pcm.len());
    let mut output = InterleavedOwned::new(0.0f32, 1, needed);
    let (_, produced) = resampler
        .process_all_into_buffer(&input, &mut output, pcm.len(), None)
        .map_err(|e| err(&e))?;
    let mut out = output.take_data();
    out.truncate(produced);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f32, hz: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    #[test]
    fn resampling_keeps_duration_and_frequency() {
        let out = resample_to_16k(&tone(44_100, 1.0, 440.0), 44_100).unwrap();
        assert!((out.len() as i64 - 16_000).abs() < 50, "{} samples", out.len());
        // A 440 Hz tone crosses zero ~880 times a second.
        let crossings = out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((860..=900).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn audio_already_at_16k_is_untouched() {
        let t = tone(16_000, 0.1, 300.0);
        assert_eq!(resample_to_16k(&t, 16_000).unwrap(), t);
    }

    /// Fennec Recorder records raw AAC (ADTS); `tests/fixtures/phone_recorder.aac`
    /// came from the Android emulator.
    #[test]
    fn phone_recordings_decode_and_a_cut_short_one_still_does() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phone_recorder.aac");
        let whole = decode_file(&fixture).unwrap();
        let secs = whole.len() as f64 / 16_000.0;
        assert!((secs - 15.8).abs() < 0.5, "{secs} s");

        // The app killed mid-recording: the file just stops.
        let dir = tempfile::tempdir().unwrap();
        let cut = dir.path().join("cut.aac");
        let bytes = std::fs::read(&fixture).unwrap();
        std::fs::write(&cut, &bytes[..bytes.len() / 2 + 333]).unwrap();
        let part = decode_file(&cut).unwrap();
        let ratio = part.len() as f64 / whole.len() as f64;
        assert!((0.4..0.6).contains(&ratio), "{ratio}");
    }

    #[test]
    fn missing_file_is_an_open_error_naming_the_path() {
        let err = decode_file(Path::new("/nope/lyd.mp3")).unwrap_err();
        assert!(matches!(err, DecodeError::Open { .. }));
        assert!(err.to_string().contains("/nope/lyd.mp3"));
    }

    #[test]
    fn a_file_that_is_not_audio_explains_both_decoders_failed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("noter.txt");
        std::fs::write(&p, "ikke lyd").unwrap();
        let err = decode_file(&p).unwrap_err();
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err}");
    }
}
