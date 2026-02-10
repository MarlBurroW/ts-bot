use anyhow::{Result, anyhow};
use tracing::info;
use std::time::Instant;

/// Transcriber that uses the OpenAI Whisper API instead of local whisper.
#[derive(Clone)]
pub struct WhisperApiTranscriber {
    api_url: String,
    api_key: String,
    model: String,
}

impl WhisperApiTranscriber {
    pub fn new(api_key: String) -> Self {
        Self {
            api_url: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key,
            model: "whisper-1".to_string(),
        }
    }

    /// Transcribe f32 audio samples (16kHz mono) via the OpenAI Whisper API.
    pub fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<String> {
        let start = Instant::now();
        let wav_data = samples_to_wav(samples);
        let duration_secs = samples.len() as f32 / 16000.0;
        info!("WhisperAPI: sending {:.1}s of audio ({} bytes WAV)", duration_secs, wav_data.len());

        // Build multipart form data manually for ureq
        let boundary = "----WhisperApiBoundary9876543210";
        let mut body: Vec<u8> = Vec::new();

        // file field
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n");
        body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
        body.extend_from_slice(&wav_data);
        body.extend_from_slice(b"\r\n");

        // model field
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
        body.extend_from_slice(self.model.as_bytes());
        body.extend_from_slice(b"\r\n");

        // language field
        if let Some(lang) = language {
            body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
            body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
            body.extend_from_slice(lang.as_bytes());
            body.extend_from_slice(b"\r\n");
        }

        // response_format field
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"response_format\"\r\n\r\n");
        body.extend_from_slice(b"text");
        body.extend_from_slice(b"\r\n");

        // closing boundary
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let content_type = format!("multipart/form-data; boundary={}", boundary);

        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .build();

        let response = agent
            .post(&self.api_url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", &content_type)
            .send_bytes(&body)
            .map_err(|e| anyhow!("WhisperAPI request failed: {}", e))?;

        let text = response.into_string()
            .map_err(|e| anyhow!("WhisperAPI response read failed: {}", e))?;

        let elapsed = start.elapsed();
        let trimmed = text.trim().to_string();
        info!("WhisperAPI: transcribed in {:.1}s -> \"{}\"", elapsed.as_secs_f32(), trimmed);

        Ok(trimmed)
    }
}

fn samples_to_wav(samples: &[f32]) -> Vec<u8> {
    let num_samples = samples.len();
    let data_size = num_samples * 2;
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size);
    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(file_size as u32).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());  // PCM
    buf.extend_from_slice(&1u16.to_le_bytes());  // mono
    buf.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
    buf.extend_from_slice(&32000u32.to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes());  // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(data_size as u32).to_le_bytes());
    for &s in samples {
        let i = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        buf.extend_from_slice(&i.to_le_bytes());
    }
    buf
}
