//! Audio input: 16 kHz WAV reading (benchmarks) and general file decoding.

pub mod capture;
pub mod decode;

use std::path::{Path, PathBuf};

use crate::engine::SAMPLE_RATE;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("could not read {path}: {source}")]
    Wav { path: PathBuf, source: hound::Error },
    #[error("{path} is {rate} Hz; expected {SAMPLE_RATE} Hz")]
    SampleRate { path: PathBuf, rate: u32 },
}

/// Reads a WAV file as mono f32 at 16 kHz, averaging channels.
pub fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, AudioError> {
    let wav_err = |source| AudioError::Wav {
        path: path.to_path_buf(),
        source,
    };
    let mut reader = hound::WavReader::open(path).map_err(wav_err)?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE {
        return Err(AudioError::SampleRate {
            path: path.to_path_buf(),
            rate: spec.sample_rate,
        });
    }
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(wav_err)?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(wav_err)?
        }
    };
    Ok(downmix(&interleaved, spec.channels as usize))
}

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.wav");
        write_wav(&p, 16_000, 2, &[16384, 0, -16384, -16384]);
        let pcm = read_wav_16k_mono(&p).unwrap();
        assert_eq!(pcm, vec![0.25, -0.5]);
    }

    #[test]
    fn wrong_sample_rate_is_rejected_with_the_rate_in_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.wav");
        write_wav(&p, 44_100, 1, &[0, 0]);
        let err = read_wav_16k_mono(&p).unwrap_err();
        assert!(err.to_string().contains("44100 Hz"), "{err}");
    }
}
