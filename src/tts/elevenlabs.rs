use anyhow::{Context, Result};
use tracing::{info, debug, warn};

use super::{TtsAudio, TtsSynthesizer};

/// ElevenLabs TTS backend.
pub struct ElevenLabsTtsSynthesizer {
    api_key: String,
    model_id: String,
    client: ureq::Agent,
}

impl ElevenLabsTtsSynthesizer {
    pub fn new(api_key: String, model_id: String) -> Self {
        let client = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();

        info!("ElevenLabs TTS configured: model={}", model_id);

        Self { api_key, model_id, client }
    }

    /// Decode MP3 bytes to PCM f32 samples using minimp3.
    fn decode_mp3(mp3_bytes: &[u8]) -> Result<TtsAudio> {
        let mut decoder = minimp3::Decoder::new(std::io::Cursor::new(mp3_bytes));
        let mut all_samples: Vec<f32> = Vec::new();
        let mut sample_rate = 24000u32;

        loop {
            match decoder.next_frame() {
                Ok(frame) => {
                    sample_rate = frame.sample_rate as u32;
                    let channels = frame.channels;
                    // Convert i16 to f32 and mix to mono if stereo
                    if channels == 1 {
                        all_samples.extend(frame.data.iter().map(|&s| s as f32 / 32768.0));
                    } else {
                        // Mix to mono
                        for chunk in frame.data.chunks(channels) {
                            let mono: f32 = chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / channels as f32;
                            all_samples.push(mono);
                        }
                    }
                }
                Err(minimp3::Error::Eof) => break,
                Err(minimp3::Error::InsufficientData) => break,
                Err(e) => {
                    // SkippedData is not fatal
                    warn!("MP3 decode warning: {:?}", e);
                    continue;
                }
            }
        }

        if all_samples.is_empty() {
            anyhow::bail!("MP3 decode produced no samples");
        }

        // Resample to 48kHz if needed (ElevenLabs often returns 44100Hz)
        let (final_samples, final_rate) = match sample_rate {
            48000 => (all_samples, 48000u32),
            24000 => (all_samples, 24000u32),
            other => {
                // Generic linear interpolation resample to 48kHz
                let ratio = 48000.0 / other as f64;
                let output_len = (all_samples.len() as f64 * ratio) as usize;
                let mut resampled = Vec::with_capacity(output_len);
                for i in 0..output_len {
                    let src_pos = i as f64 / ratio;
                    let src_idx = src_pos as usize;
                    let frac = src_pos - src_idx as f64;
                    let sample = if src_idx + 1 < all_samples.len() {
                        all_samples[src_idx] as f64 * (1.0 - frac) + all_samples[src_idx + 1] as f64 * frac
                    } else {
                        all_samples[src_idx.min(all_samples.len() - 1)] as f64
                    };
                    resampled.push(sample as f32);
                }
                info!("Resampled {}Hz → 48000Hz ({} → {} samples)", other, all_samples.len(), resampled.len());
                (resampled, 48000u32)
            }
        };

        Ok(TtsAudio { samples: final_samples, sample_rate: final_rate })
    }
}

impl TtsSynthesizer for ElevenLabsTtsSynthesizer {
    fn synthesize(&self, text: &str, voice: Option<&str>, _speed: Option<f32>) -> Result<TtsAudio> {
        let voice_id = voice.unwrap_or("21m00Tcm4TlvDq8ikWAM"); // default: rachel
        info!("ElevenLabs TTS request: '{}' (voice_id: {}, model: {})", text, voice_id, self.model_id);

        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", voice_id);

        let body = serde_json::json!({
            "text": text,
            "model_id": self.model_id,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.75
            }
        });

        let response = self.client
            .post(&url)
            .set("xi-api-key", &self.api_key)
            .set("Content-Type", "application/json")
            .set("Accept", "audio/mpeg")
            .send_json(body)
            .context("ElevenLabs TTS request failed")?;

        let mut mp3_bytes = Vec::new();
        response.into_reader().read_to_end(&mut mp3_bytes)
            .context("Failed to read ElevenLabs response body")?;

        if mp3_bytes.is_empty() {
            anyhow::bail!("ElevenLabs returned empty response");
        }

        if mp3_bytes.starts_with(b"{") {
            let err_text = String::from_utf8_lossy(&mp3_bytes[..mp3_bytes.len().min(500)]);
            anyhow::bail!("ElevenLabs API error: {}", err_text);
        }

        debug!("ElevenLabs response: {} bytes MP3", mp3_bytes.len());

        let audio = Self::decode_mp3(&mp3_bytes)?;

        info!(
            "ElevenLabs TTS produced {} samples at {}Hz ({:.1}s)",
            audio.samples.len(),
            audio.sample_rate,
            audio.samples.len() as f32 / audio.sample_rate as f32
        );

        Ok(audio)
    }

    fn name(&self) -> &str {
        "ElevenLabs TTS"
    }

    fn default_voice(&self) -> &str {
        "21m00Tcm4TlvDq8ikWAM"
    }
}
