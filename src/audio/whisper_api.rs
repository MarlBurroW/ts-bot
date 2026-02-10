use anyhow::{Result, anyhow};
use tracing::info;
use std::time::Instant;

/// Result from Whisper API transcription, including detected language.
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
}

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
    /// Returns text + detected language. If language is None, Whisper auto-detects.
    pub fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<TranscriptionResult> {
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

        // language field (if provided, forces language; otherwise Whisper auto-detects)
        if let Some(lang) = language {
            body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
            body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
            body.extend_from_slice(lang.as_bytes());
            body.extend_from_slice(b"\r\n");
        }

        // response_format = verbose_json to get detected language
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"response_format\"\r\n\r\n");
        body.extend_from_slice(b"verbose_json");
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

        let response_text = response.into_string()
            .map_err(|e| anyhow!("WhisperAPI response read failed: {}", e))?;

        // Parse verbose_json response: {"text": "...", "language": "french", ...}
        let json: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| anyhow!("WhisperAPI JSON parse failed: {} (response: {})", e, &response_text[..response_text.len().min(200)]))?;

        let text = json["text"].as_str().unwrap_or("").trim().to_string();
        let language_detected = json["language"].as_str().map(|s| s.to_string());

        let elapsed = start.elapsed();
        info!("WhisperAPI: transcribed in {:.1}s, lang={} -> \"{}\"",
            elapsed.as_secs_f32(),
            language_detected.as_deref().unwrap_or("?"),
            text
        );

        Ok(TranscriptionResult { text, language: language_detected })
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
