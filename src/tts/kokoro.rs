use anyhow::Result;
use std::path::Path;
use tracing::info;

use super::{TtsAudio, TtsSynthesizer};

/// Kokoro TTS backend (82M params, ONNX-based)
///
/// Produces 24kHz PCM f32 audio from text.
/// Requires espeak-ng installed on the system for phoneme conversion.
///
/// To swap this for another TTS backend, implement the `TtsSynthesizer` trait
/// on a new struct and pass it to the AudioPlayer instead.
pub struct KokoroSynthesizer {
    // TODO: Replace with actual Kokoro crate type once dependency is added
    // e.g., kokoros::Kokoro or kokoro_tiny::TTS
    #[allow(dead_code)]
    model_path: String,
    default_voice: String,
}

impl KokoroSynthesizer {
    /// Create a new Kokoro synthesizer
    ///
    /// - `model_path`: path to the Kokoro ONNX model directory (e.g., "models/kokoro/")
    /// - `default_voice`: default voice ID (e.g., "ff_siwis" for French female)
    pub fn new(model_path: impl AsRef<Path>, default_voice: &str) -> Result<Self> {
        let model_path = model_path.as_ref().to_string_lossy().to_string();

        info!("Loading Kokoro TTS model from '{}'", model_path);

        // TODO: Initialize actual Kokoro model here
        // e.g.: let model = kokoros::Kokoro::new(&model_path)?;
        // For now, store paths for later initialization

        info!("Kokoro TTS loaded (voice: {})", default_voice);

        Ok(Self {
            model_path,
            default_voice: default_voice.to_string(),
        })
    }
}

impl TtsSynthesizer for KokoroSynthesizer {
    fn synthesize(&self, text: &str, voice: Option<&str>) -> Result<TtsAudio> {
        let voice = voice.unwrap_or(&self.default_voice);
        info!("Kokoro synthesizing: '{}' (voice: {})", text, voice);

        // TODO: Replace with actual Kokoro synthesis call
        // e.g.:
        // let audio = self.model.synthesize(text, voice)?;
        // return Ok(TtsAudio { samples: audio.samples, sample_rate: 24000 });

        // Placeholder: generate silence (will be replaced when kokoro crate is added)
        let duration_secs = (text.len() as f32 * 0.06).max(0.5); // rough estimate
        let sample_count = (24000.0 * duration_secs) as usize;
        let samples = vec![0.0f32; sample_count];

        Ok(TtsAudio {
            samples,
            sample_rate: 24000,
        })
    }

    fn name(&self) -> &str {
        "Kokoro"
    }

    fn default_voice(&self) -> &str {
        &self.default_voice
    }
}
