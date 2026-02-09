use audiopus::coder::Encoder;
use audiopus::{Application, Channels, SampleRate};
use anyhow::Result;
use tracing::debug;

/// TS3 uses 20ms frames at 48kHz mono = 960 samples per frame
pub const OPUS_FRAME_SIZE: usize = 960;

/// Maximum Opus encoded frame size in bytes
const MAX_OPUS_FRAME_SIZE: usize = 1275;

/// Opus audio encoder for sending audio to TeamSpeak
/// Symmetric to OpusDecoder — encodes PCM f32 at 48kHz to Opus bytes
pub struct OpusEncoder {
    encoder: Encoder,
}

impl OpusEncoder {
    /// Create a new Opus encoder
    /// TS3 uses 48kHz mono with Voip application profile
    pub fn new() -> Result<Self> {
        let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;

        Ok(Self { encoder })
    }

    /// Encode a single frame of PCM f32 audio to Opus bytes
    /// Input: exactly OPUS_FRAME_SIZE (960) f32 samples at 48kHz, normalized [-1.0, 1.0]
    /// Output: Opus-encoded bytes
    pub fn encode_frame(&mut self, pcm_f32: &[f32]) -> Result<Vec<u8>> {
        assert_eq!(
            pcm_f32.len(),
            OPUS_FRAME_SIZE,
            "Frame must be exactly {} samples (20ms @ 48kHz)",
            OPUS_FRAME_SIZE
        );

        let mut opus_output = [0u8; MAX_OPUS_FRAME_SIZE];
        let len = self
            .encoder
            .encode_float(pcm_f32, &mut opus_output)
            .map_err(|e| anyhow::anyhow!("Opus encode error: {}", e))?;

        debug!("Encoded {} samples to {} Opus bytes", pcm_f32.len(), len);
        Ok(opus_output[..len].to_vec())
    }

    /// Encode a full audio buffer into a list of Opus frames
    /// Splits the input into OPUS_FRAME_SIZE chunks, zero-pads the last one if needed
    /// Input: f32 samples at 48kHz, normalized [-1.0, 1.0]
    /// Output: list of Opus-encoded byte frames (each = 20ms of audio)
    pub fn encode_all(&mut self, pcm_f32: &[f32]) -> Result<Vec<Vec<u8>>> {
        let mut frames = Vec::new();

        for chunk in pcm_f32.chunks(OPUS_FRAME_SIZE) {
            let frame = if chunk.len() == OPUS_FRAME_SIZE {
                self.encode_frame(chunk)?
            } else {
                // Zero-pad the last frame
                let mut padded = vec![0.0f32; OPUS_FRAME_SIZE];
                padded[..chunk.len()].copy_from_slice(chunk);
                self.encode_frame(&padded)?
            };
            frames.push(frame);
        }

        debug!(
            "Encoded {} samples into {} Opus frames ({:.1}s)",
            pcm_f32.len(),
            frames.len(),
            frames.len() as f32 * 0.02
        );
        Ok(frames)
    }
}

impl Default for OpusEncoder {
    fn default() -> Self {
        Self::new().expect("Failed to create Opus encoder")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_frame_sine_wave() {
        let mut encoder = OpusEncoder::new().unwrap();
        // 20ms sine wave at 440Hz
        let frame: Vec<f32> = (0..OPUS_FRAME_SIZE)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 48000.0).sin() * 0.5)
            .collect();
        let encoded = encoder.encode_frame(&frame).unwrap();
        assert!(!encoded.is_empty(), "Encoded frame should not be empty");
        assert!(
            encoded.len() <= MAX_OPUS_FRAME_SIZE,
            "Encoded frame should not exceed max size"
        );
    }

    #[test]
    fn test_encode_frame_silence() {
        let mut encoder = OpusEncoder::new().unwrap();
        let silence = vec![0.0f32; OPUS_FRAME_SIZE];
        let encoded = encoder.encode_frame(&silence).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    #[should_panic(expected = "Frame must be exactly 960 samples")]
    fn test_encode_frame_wrong_size() {
        let mut encoder = OpusEncoder::new().unwrap();
        let short = vec![0.0f32; 100];
        let _ = encoder.encode_frame(&short);
    }

    #[test]
    fn test_encode_all_exact_frames() {
        let mut encoder = OpusEncoder::new().unwrap();
        // Exactly 3 frames = 2880 samples
        let pcm = vec![0.0f32; OPUS_FRAME_SIZE * 3];
        let frames = encoder.encode_all(&pcm).unwrap();
        assert_eq!(frames.len(), 3);
    }

    #[test]
    fn test_encode_all_with_padding() {
        let mut encoder = OpusEncoder::new().unwrap();
        // 2.5 frames = 2400 samples → should produce 3 frames (last one zero-padded)
        let pcm = vec![0.0f32; OPUS_FRAME_SIZE * 2 + OPUS_FRAME_SIZE / 2];
        let frames = encoder.encode_all(&pcm).unwrap();
        assert_eq!(frames.len(), 3);
    }

    #[test]
    fn test_encode_all_empty() {
        let mut encoder = OpusEncoder::new().unwrap();
        let frames = encoder.encode_all(&[]).unwrap();
        assert!(frames.is_empty());
    }
}
