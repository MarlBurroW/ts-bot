use anyhow::{Context, Result};
use hound::WavReader;
use std::path::Path;
use tracing::debug;

const TARGET_SAMPLE_RATE: u32 = 16000;

/// Load a WAV file and convert to mono f32 samples at 16kHz
/// Returns normalized samples in [-1.0, 1.0]
pub fn load_wav(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let path = path.as_ref();
    let reader = WavReader::open(path)
        .with_context(|| format!("Failed to open WAV file: {}", path.display()))?;

    let spec = reader.spec();
    let channels = spec.channels as usize;
    let source_rate = spec.sample_rate;

    debug!(
        "Loading WAV: {} ({}Hz, {} channels, {:?})",
        path.display(),
        source_rate,
        channels,
        spec.sample_format
    );

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            let max_val = (1u32 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_val))
                .collect::<Result<Vec<f32>, _>>()
                .context("Failed to read integer WAV samples")?
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<f32>, _>>()
            .context("Failed to read float WAV samples")?,
    };

    // Convert to mono by averaging channels
    let mono: Vec<f32> = if channels > 1 {
        samples_f32
            .chunks(channels)
            .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples_f32
    };

    // Resample to 16kHz if needed
    let resampled = if source_rate != TARGET_SAMPLE_RATE {
        resample(&mono, source_rate, TARGET_SAMPLE_RATE)
    } else {
        mono
    };

    debug!(
        "Loaded {} samples ({:.2}s at {}Hz)",
        resampled.len(),
        resampled.len() as f32 / TARGET_SAMPLE_RATE as f32,
        TARGET_SAMPLE_RATE
    );

    Ok(resampled)
}

/// Simple linear interpolation resampling
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < samples.len() {
            samples[src_idx] as f64 * (1.0 - frac) + samples[src_idx + 1] as f64 * frac
        } else {
            samples[src_idx.min(samples.len() - 1)] as f64
        };

        output.push(sample as f32);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_same_rate() {
        let input = vec![1.0, 0.5, -0.5, -1.0];
        let output = resample(&input, 16000, 16000);
        assert_eq!(input, output);
    }

    #[test]
    fn test_resample_downsample() {
        // 48kHz -> 16kHz should produce ~1/3 samples
        let input: Vec<f32> = (0..480).map(|i| (i as f32 / 480.0) * 2.0 - 1.0).collect();
        let output = resample(&input, 48000, 16000);
        assert_eq!(output.len(), 160);
    }

    #[test]
    fn test_load_wav_missing_file() {
        let result = load_wav("nonexistent.wav");
        assert!(result.is_err());
    }
}
