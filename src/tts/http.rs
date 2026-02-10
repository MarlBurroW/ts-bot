use std::io::Cursor;

use anyhow::{Context, Result};
use tracing::{info, debug};

use super::{TtsAudio, TtsSynthesizer};

/// HTTP-based TTS backend that calls an OpenAI-compatible `/v1/audio/speech` endpoint.
///
/// Works with any TTS service exposing this API: OpenAI, kokorox, Fish Speech, etc.
///
/// Configure in `.env`:
/// ```text
/// TTS_API_URL=https://api.openai.com/v1/audio/speech
/// TTS_API_KEY=sk-...
/// TTS_MODEL=tts-1
/// TTS_VOICE=nova
/// ```
pub struct HttpTtsSynthesizer {
    /// Full URL to the TTS endpoint
    api_url: String,
    /// Model name to pass to the API (e.g., "tts-1", "kokoro")
    model: String,
    /// Default voice ID (e.g., "nova", "ff_siwis")
    default_voice: String,
    /// Optional API key for authenticated services (OpenAI, etc.)
    api_key: Option<String>,
    /// Reusable HTTP client
    client: ureq::Agent,
}

impl HttpTtsSynthesizer {
    /// Create a new HTTP TTS synthesizer.
    ///
    /// - `api_url`: full endpoint URL (e.g., `https://api.openai.com/v1/audio/speech`)
    /// - `model`: model name to send in the request body (e.g., `"tts-1"`)
    /// - `default_voice`: voice ID used when none is specified (e.g., `"nova"`)
    /// - `api_key`: optional Bearer token for authenticated APIs (OpenAI, etc.)
    pub fn new(api_url: &str, model: &str, default_voice: &str, api_key: Option<String>) -> Self {
        let client = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();

        info!(
            "HTTP TTS configured: url={}, model={}, voice={}, auth={}",
            api_url, model, default_voice,
            if api_key.is_some() { "Bearer" } else { "none" }
        );

        Self {
            api_url: api_url.to_string(),
            model: model.to_string(),
            default_voice: default_voice.to_string(),
            api_key,
            client,
        }
    }

    /// Fix streaming WAV headers where sizes are set to 0xFFFFFFFF.
    ///
    /// OpenAI (and other streaming TTS APIs) use `transfer-encoding: chunked`
    /// and don't know the total size upfront, so they write 0xFFFFFFFF as the
    /// RIFF chunk size and data chunk size. hound rejects this.
    fn fix_streaming_wav(wav: &mut Vec<u8>) {
        if wav.len() < 44 {
            return;
        }
        // Fix RIFF chunk size (bytes 4..8): should be total_len - 8
        if wav[4..8] == [0xFF, 0xFF, 0xFF, 0xFF] {
            let riff_size = (wav.len() as u32).wrapping_sub(8);
            wav[4..8].copy_from_slice(&riff_size.to_le_bytes());
        }
        // Fix data chunk size: search for "data" marker, then fix the 4 bytes after it
        if let Some(pos) = wav.windows(4).position(|w| w == b"data") {
            if pos + 8 <= wav.len() && wav[pos + 4..pos + 8] == [0xFF, 0xFF, 0xFF, 0xFF] {
                let data_size = (wav.len() as u32).wrapping_sub(pos as u32 + 8);
                wav[pos + 4..pos + 8].copy_from_slice(&data_size.to_le_bytes());
            }
        }
    }

    /// Decode WAV bytes into PCM f32 samples + sample rate
    fn decode_wav(wav_bytes: &[u8]) -> Result<TtsAudio> {
        let cursor = Cursor::new(wav_bytes);
        let mut reader = hound::WavReader::new(cursor)
            .context("Failed to decode WAV response from TTS API")?;

        let spec = reader.spec();
        let sample_rate = spec.sample_rate;

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<Vec<f32>, _>>()
                .context("Failed to read f32 WAV samples")?,
            hound::SampleFormat::Int => {
                let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .collect::<Result<Vec<i32>, _>>()
                    .context("Failed to read int WAV samples")?
                    .into_iter()
                    .map(|s| s as f32 / max_val)
                    .collect()
            }
        };

        Ok(TtsAudio {
            samples,
            sample_rate,
        })
    }
}

impl TtsSynthesizer for HttpTtsSynthesizer {
    fn synthesize(&self, text: &str, voice: Option<&str>, speed: Option<f32>) -> Result<TtsAudio> {
        let voice = voice.unwrap_or(&self.default_voice);
        let speed = speed.unwrap_or(1.15).clamp(0.25, 4.0);
        info!(
            "HTTP TTS request: '{}' (voice: {}, model: {}, speed: {})",
            text, voice, self.model, speed
        );

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": voice,
            "response_format": "wav",
            "speed": speed
        });

        let mut request = self
            .client
            .post(&self.api_url)
            .set("Content-Type", "application/json");

        if let Some(ref key) = self.api_key {
            request = request.set("Authorization", &format!("Bearer {}", key));
        }

        let response = request
            .send_json(body)
            .context("HTTP TTS request failed (is the TTS service running?)")?;

        let content_type = response.content_type().to_string();
        let status = response.status();
        debug!("TTS response: status={}, content-type={}", status, content_type);

        let mut wav_bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut wav_bytes)
            .context("Failed to read TTS response body")?;

        if wav_bytes.is_empty() {
            anyhow::bail!("TTS API returned empty response");
        }

        debug!(
            "TTS response body: {} bytes, first 16: {:?}",
            wav_bytes.len(),
            &wav_bytes[..wav_bytes.len().min(16)]
        );

        // Check if the response looks like a JSON error instead of audio
        if wav_bytes.starts_with(b"{") {
            let err_text = String::from_utf8_lossy(&wav_bytes[..wav_bytes.len().min(500)]);
            anyhow::bail!("TTS API returned error: {}", err_text);
        }

        // Fix streaming WAV headers (OpenAI sets sizes to 0xFFFFFFFF)
        Self::fix_streaming_wav(&mut wav_bytes);

        let audio = Self::decode_wav(&wav_bytes)?;

        info!(
            "HTTP TTS produced {} samples at {}Hz ({:.1}s)",
            audio.samples.len(),
            audio.sample_rate,
            audio.samples.len() as f32 / audio.sample_rate as f32
        );

        Ok(audio)
    }

    fn name(&self) -> &str {
        "HTTP TTS"
    }

    fn default_voice(&self) -> &str {
        &self.default_voice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_wav_16bit() {
        // Generate a minimal valid WAV file (16-bit PCM, mono, 24000Hz)
        let mut buf = Cursor::new(Vec::new());
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 24000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
            // Write a few samples
            writer.write_sample(0i16).unwrap();
            writer.write_sample(16384i16).unwrap(); // ~0.5
            writer.write_sample(-16384i16).unwrap(); // ~-0.5
            writer.write_sample(32767i16).unwrap(); // ~1.0
            writer.finalize().unwrap();
        }

        let audio = HttpTtsSynthesizer::decode_wav(buf.get_ref()).unwrap();
        assert_eq!(audio.sample_rate, 24000);
        assert_eq!(audio.samples.len(), 4);
        assert!((audio.samples[0]).abs() < 0.01); // ~0
        assert!((audio.samples[1] - 0.5).abs() < 0.01); // ~0.5
        assert!((audio.samples[3] - 1.0).abs() < 0.01); // ~1.0
    }

    #[test]
    fn test_decode_wav_f32() {
        let mut buf = Cursor::new(Vec::new());
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
            writer.write_sample(0.0f32).unwrap();
            writer.write_sample(0.5f32).unwrap();
            writer.write_sample(-0.5f32).unwrap();
            writer.finalize().unwrap();
        }

        let audio = HttpTtsSynthesizer::decode_wav(buf.get_ref()).unwrap();
        assert_eq!(audio.sample_rate, 48000);
        assert_eq!(audio.samples.len(), 3);
        assert!((audio.samples[1] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_decode_wav_empty_fails() {
        let result = HttpTtsSynthesizer::decode_wav(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_wav_invalid_fails() {
        let result = HttpTtsSynthesizer::decode_wav(b"not a wav file");
        assert!(result.is_err());
    }
}
