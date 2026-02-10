use anyhow::Result;
use std::path::Path;

use super::whisper::WhisperTranscriber;

/// Transcription pipeline using a larger Whisper model (e.g. small)
///
/// Dedicated to full transcription after activation.
/// Used as fallback when no Whisper API key is configured.
pub struct TranscriptionPipeline {
    transcriber: WhisperTranscriber,
}

impl TranscriptionPipeline {
    /// Create a new transcription pipeline
    ///
    /// - `model_path`: path to Whisper model (e.g., "models/ggml-small.bin")
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self> {
        let transcriber = WhisperTranscriber::new(model_path)?;
        Ok(Self { transcriber })
    }

    /// Transcribe audio samples to text
    ///
    /// Returns the transcribed text.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        self.transcriber.transcribe(samples, Some("fr"))
    }
}
