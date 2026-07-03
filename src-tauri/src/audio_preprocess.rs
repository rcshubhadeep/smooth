use serde::Serialize;
use std::path::Path;

pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Debug, Serialize)]
pub struct WhisperAudioInfo {
    pub source_sample_rate: u32,
    pub source_channels: u16,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u128,
    pub samples: usize,
    pub rms_db: Option<f32>,
    pub peak_db: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct WhisperAudio {
    pub samples: Vec<f32>,
    pub info: WhisperAudioInfo,
}

pub fn load_wav_for_whisper(path: &Path) -> Result<WhisperAudio, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|error| format!("Failed to open WAV: {error}"))?;
    let spec = reader.spec();
    let source_channels = spec.channels.max(1);

    let interleaved = match spec.sample_format {
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
            .samples::<i16>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i16::MAX as f32)
                    .map_err(|error| format!("Failed to read WAV sample: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Float if spec.bits_per_sample == 32 => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(|error| format!("Failed to read WAV sample: {error}")))
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(format!(
                "Unsupported WAV format: {:?}, {} bits per sample",
                spec.sample_format, spec.bits_per_sample
            ));
        }
    };

    let mono = mix_interleaved_to_mono(&interleaved, source_channels);
    let samples = resample_linear(&mono, spec.sample_rate, WHISPER_SAMPLE_RATE);
    let duration_ms = if WHISPER_SAMPLE_RATE == 0 {
        0
    } else {
        (samples.len() as u128 * 1000) / WHISPER_SAMPLE_RATE as u128
    };
    let sample_count = samples.len();
    let (rms_db, peak_db) = audio_levels(&samples);

    Ok(WhisperAudio {
        samples,
        info: WhisperAudioInfo {
            source_sample_rate: spec.sample_rate,
            source_channels,
            sample_rate: WHISPER_SAMPLE_RATE,
            channels: 1,
            duration_ms,
            samples: sample_count,
            rms_db,
            peak_db,
        },
    })
}

fn mix_interleaved_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return samples.to_vec();
    }

    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

fn resample_linear(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }

    if source_rate == target_rate {
        return samples.to_vec();
    }

    let output_len =
        ((samples.len() as u128 * target_rate as u128) / source_rate as u128).max(1) as usize;
    let ratio = source_rate as f64 / target_rate as f64;
    let mut output = Vec::with_capacity(output_len);

    for index in 0..output_len {
        let source_position = index as f64 * ratio;
        let left = source_position.floor() as usize;
        let right = (left + 1).min(samples.len() - 1);
        let fraction = (source_position - left as f64) as f32;
        output.push(samples[left] + (samples[right] - samples[left]) * fraction);
    }

    output
}

fn audio_levels(samples: &[f32]) -> (Option<f32>, Option<f32>) {
    if samples.is_empty() {
        return (None, None);
    }

    let sum_squares = samples
        .iter()
        .map(|sample| (*sample as f64) * (*sample as f64))
        .sum::<f64>();
    let rms = (sum_squares / samples.len() as f64).sqrt() as f32;
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);

    (amplitude_to_db(rms), amplitude_to_db(peak))
}

fn amplitude_to_db(amplitude: f32) -> Option<f32> {
    if amplitude <= f32::EPSILON {
        return None;
    }
    Some(20.0 * amplitude.log10())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_stereo_to_mono() {
        let mono = mix_interleaved_to_mono(&[1.0, -1.0, 0.5, 0.25], 2);
        assert_eq!(mono, vec![0.0, 0.375]);
    }

    #[test]
    fn resamples_to_expected_length() {
        let source = vec![0.0; 48_000];
        let resampled = resample_linear(&source, 48_000, WHISPER_SAMPLE_RATE);
        assert_eq!(resampled.len(), 16_000);
    }
}
