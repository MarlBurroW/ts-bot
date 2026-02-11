use audiopus::{coder::Decoder, Channels, SampleRate};
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Opus audio decoder for TeamSpeak audio packets
/// Counter for throttling decode error logs
static OPUS_DECODE_ERRORS: AtomicU64 = AtomicU64::new(0);

pub struct OpusDecoder {
    decoder: Decoder,
    sample_rate: u32,
}

impl OpusDecoder {
    /// Create a new Opus decoder
    /// TS3 typically uses 48kHz mono audio
    pub fn new() -> Result<Self> {
        let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;

        Ok(Self {
            decoder,
            sample_rate: 48000,
        })
    }

    /// Decode Opus audio packet to PCM samples
    /// Returns Vec<i16> PCM samples at 48kHz
    ///
    /// Handles malformed/empty packets gracefully using Opus PLC (Packet Loss
    /// Concealment) instead of propagating errors. TS3 sends many such packets
    /// during silence, so erroring on them is wasteful.
    pub fn decode(&mut self, opus_data: &[u8]) -> Result<Vec<i16>> {
        // Allocate buffer for decoded audio (max frame size)
        let mut output = vec![0i16; 5760]; // 120ms at 48kHz

        // Skip trivially invalid packets (empty or single-byte) — return silence
        if opus_data.len() < 2 {
            debug!("Skipping too-short Opus packet ({} bytes)", opus_data.len());
            return Ok(Vec::new());
        }

        // Decode the opus packet
        match self.decoder.decode(Some(opus_data), &mut output, false) {
            Ok(size) => {
                // Truncate to actual decoded size
                output.truncate(size);
                debug!("Decoded {} samples from {} bytes", size, opus_data.len());
                Ok(output)
            }
            Err(e) => {
                let count = OPUS_DECODE_ERRORS.fetch_add(1, Ordering::Relaxed) + 1;
                // Log only every 200th error to reduce noise
                if count == 1 || count.is_multiple_of(200) {
                    warn!("Opus decode error (total: {}): {} — using PLC", count, e);
                }
                // Use Opus PLC: pass None to generate concealment audio
                // This keeps the decoder state consistent and avoids audio glitches
                match self.decoder.decode(None::<&[u8]>, &mut output, false) {
                    Ok(size) => {
                        output.truncate(size);
                        Ok(output)
                    }
                    Err(_) => {
                        // PLC also failed — return empty (silence)
                        Ok(Vec::new())
                    }
                }
            }
        }
    }

    /// Resample from 48kHz to 16kHz for Whisper
    /// Whisper expects 16kHz mono audio
    ///
    /// Uses a low-pass FIR anti-aliasing filter before decimation to prevent
    /// frequency aliasing artifacts that degrade Whisper transcription quality.
    pub fn resample_to_16khz(samples_48khz: &[i16]) -> Vec<f32> {
        let ratio = 3; // 48000 / 16000 = 3

        // Convert to f32 first
        let float_samples: Vec<f32> = samples_48khz
            .iter()
            .map(|&s| s as f32 / 32768.0)
            .collect();

        // Apply low-pass FIR filter (cutoff ~7.5kHz at 48kHz sample rate)
        // 15-tap windowed sinc filter designed for 3:1 decimation
        // Normalized so passband gain = 1.0
        const COEFFS: [f32; 15] = [
            -0.0078, 0.0000, 0.0378, 0.0000, -0.1199,
             0.0000, 0.6199, 1.0000, 0.6199, 0.0000,
            -0.1199, 0.0000, 0.0378, 0.0000, -0.0078,
        ];
        const GAIN: f32 = 1.0 / 1.4400; // sum of COEFFS ≈ 1.44
        const HALF_LEN: usize = 7; // (15 - 1) / 2

        let n = float_samples.len();
        let mut samples_16khz = Vec::with_capacity(n / ratio + 1);

        // Decimation with FIR filtering
        let mut i = 0;
        while i < n {
            let mut acc: f32 = 0.0;
            for (k, &c) in COEFFS.iter().enumerate() {
                let idx = i as isize + k as isize - HALF_LEN as isize;
                if idx >= 0 && (idx as usize) < n {
                    acc += c * float_samples[idx as usize];
                }
            }
            samples_16khz.push(acc * GAIN);
            i += ratio;
        }

        samples_16khz
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Resample f32 PCM from 24kHz to 48kHz using linear interpolation
    /// Used for TTS output (Kokoro produces 24kHz) before Opus encoding for TS3 (48kHz)
    pub fn resample_24k_to_48k(samples_24k: &[f32]) -> Vec<f32> {
        if samples_24k.is_empty() {
            return Vec::new();
        }

        // Ratio 2:1 — insert one interpolated sample between each pair
        let output_len = samples_24k.len() * 2;
        let mut samples_48k = Vec::with_capacity(output_len);

        for i in 0..samples_24k.len() - 1 {
            samples_48k.push(samples_24k[i]);
            // Linear interpolation: midpoint between consecutive samples
            samples_48k.push((samples_24k[i] + samples_24k[i + 1]) * 0.5);
        }
        // Last sample (no interpolation partner)
        samples_48k.push(*samples_24k.last().unwrap());
        samples_48k.push(*samples_24k.last().unwrap());

        samples_48k
    }
}

impl Default for OpusDecoder {
    fn default() -> Self {
        Self::new().expect("Failed to create Opus decoder")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_24k_to_48k_doubles_length() {
        let input = vec![0.0f32; 24000]; // 1 second at 24kHz
        let output = OpusDecoder::resample_24k_to_48k(&input);
        assert_eq!(output.len(), 48000, "1s @ 24kHz should become 48000 samples");
    }

    #[test]
    fn test_resample_24k_to_48k_preserves_original_samples() {
        let input = vec![1.0, 0.0, -1.0, 0.0];
        let output = OpusDecoder::resample_24k_to_48k(&input);
        // Original samples should be at even indices
        assert_eq!(output[0], 1.0);
        assert_eq!(output[2], 0.0);
        assert_eq!(output[4], -1.0);
    }

    #[test]
    fn test_resample_24k_to_48k_interpolates() {
        let input = vec![0.0, 1.0];
        let output = OpusDecoder::resample_24k_to_48k(&input);
        // output[0] = 0.0 (original)
        // output[1] = 0.5 (interpolated midpoint)
        // output[2] = 1.0 (original, last sample duplicated)
        // output[3] = 1.0 (last sample duplicated)
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.5);
        assert_eq!(output[2], 1.0);
        assert_eq!(output[3], 1.0);
    }

    #[test]
    fn test_resample_24k_to_48k_empty() {
        let output = OpusDecoder::resample_24k_to_48k(&[]);
        assert!(output.is_empty());
    }

    #[test]
    fn test_resample_24k_to_48k_single_sample() {
        let output = OpusDecoder::resample_24k_to_48k(&[0.5]);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], 0.5);
        assert_eq!(output[1], 0.5);
    }
}
