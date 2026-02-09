use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
    install_whisper_tracing_trampoline};
use anyhow::Result;
use std::path::Path;
use tracing::{debug, info};

/// Minimum RMS energy threshold to consider audio as speech (not silence/ambient)
pub const MIN_SPEECH_RMS: f32 = 0.003;

/// Whisper transcriber for audio STT
pub struct WhisperTranscriber {
    context: WhisperContext,
    /// Optional initial prompt to bias Whisper vocabulary (e.g., "Hey marlbot.")
    initial_prompt: Option<String>,
}

impl WhisperTranscriber {
    /// Create a new Whisper transcriber
    /// model_path: path to the Whisper model file (e.g., "models/ggml-tiny.bin")
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self> {
        // Redirect whisper.cpp logs to tracing (controlled by log level)
        install_whisper_tracing_trampoline();

        info!("Loading Whisper model from {:?}", model_path.as_ref());

        let context = WhisperContext::new_with_params(
            model_path.as_ref().to_str().unwrap(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {}", e))?;

        info!("Whisper model loaded successfully");

        Ok(Self { context, initial_prompt: None })
    }

    /// Set the initial prompt to bias Whisper towards specific vocabulary
    ///
    /// Whisper uses this as "previous text" context, making it more likely
    /// to recognize words present in the prompt (e.g., custom names like "marlbot").
    pub fn set_initial_prompt(&mut self, prompt: &str) {
        info!("Whisper initial_prompt set to: '{}'", prompt);
        self.initial_prompt = Some(prompt.to_string());
    }

    /// Calculate RMS energy of audio samples
    pub fn rms_energy(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    /// Check if audio has enough energy to be speech
    pub fn has_speech_energy(samples: &[f32]) -> bool {
        let rms = Self::rms_energy(samples);
        let has_energy = rms > MIN_SPEECH_RMS;
        if !has_energy {
            debug!("Audio too quiet (RMS: {:.6}), skipping transcription", rms);
        }
        has_energy
    }

    /// Detect if text is a repetition loop (e.g., "mais mais mais mais...")
    pub fn is_repetition(text: &str) -> bool {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 3 {
            return false;
        }
        // Check if the same word repeats 3+ times consecutively
        let first = words[0];
        let repeat_count = words.iter().take_while(|&&w| w == first).count();
        if repeat_count >= 3 {
            debug!("Repetition loop detected: '{}' repeated {} times", first, repeat_count);
            return true;
        }
        false
    }

    /// Apply common params to suppress output and prevent hallucinations
    fn apply_common_params(&self, params: &mut FullParams) {
        // Suppress all whisper.cpp debug output
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);

        // Prevent repetition loops and silence hallucinations
        params.set_max_tokens(50); // Hard limit on output tokens
        params.set_no_speech_thold(0.5); // Skip segments that are likely silence
        params.set_single_segment(true); // Only process one segment

        // Bias vocabulary towards expected words (e.g., bot name)
        if let Some(ref prompt) = self.initial_prompt {
            params.set_initial_prompt(prompt);
        }
    }

    /// Transcribe audio samples
    /// samples: PCM f32 samples at 16kHz normalized to [-1.0, 1.0]
    /// language: optional language code (e.g., "fr", "en") - None for auto-detect
    pub fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        // Skip very short audio (less than 0.5 seconds = 8000 samples at 16kHz)
        if samples.len() < 8000 {
            return Ok(String::new());
        }

        // Skip silent/ambient noise audio
        if !Self::has_speech_energy(samples) {
            return Ok(String::new());
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        if let Some(lang) = language {
            params.set_language(Some(lang));
        }
        params.set_translate(false);
        self.apply_common_params(&mut params);
        // For full transcription, allow more tokens but still limited
        params.set_max_tokens(100);
        params.set_single_segment(false);

        let mut state = self.context.create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create Whisper state: {}", e))?;

        // Whisper requires at least 1 second of audio (16000 samples at 16kHz)
        let min_samples = 32000;
        let padded_samples;
        let audio = if samples.len() < min_samples {
            padded_samples = {
                let mut v = samples.to_vec();
                v.resize(min_samples, 0.0);
                v
            };
            &padded_samples
        } else {
            samples
        };

        state.full(params, audio)
            .map_err(|e| anyhow::anyhow!("Transcription failed: {}", e))?;

        let num_segments = state.full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segments: {}", e))?;

        let mut full_text = String::new();

        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                if !segment.trim().is_empty() {
                    if !full_text.is_empty() {
                        full_text.push(' ');
                    }
                    full_text.push_str(segment.trim());
                }
            }
        }

        // Filter out repetition loops
        if Self::is_repetition(&full_text) {
            return Ok(String::new());
        }

        Ok(full_text)
    }

    /// Quick transcription for wake word detection
    /// Uses faster settings optimized for short phrases
    pub fn transcribe_wake_word(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        // Skip silent/ambient noise audio
        if !Self::has_speech_energy(samples) {
            return Ok(String::new());
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_language(Some("fr"));
        params.set_translate(false);
        self.apply_common_params(&mut params);
        params.set_max_tokens(16); // Wake word may need more tokens

        let mut state = self.context.create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create Whisper state: {}", e))?;

        // Whisper requires at least 1 second of audio (16000 samples at 16kHz)
        let min_samples = 32000; // 2 seconds at 16kHz
        let padded_samples;
        let audio = if samples.len() < min_samples {
            padded_samples = {
                let mut v = samples.to_vec();
                v.resize(min_samples, 0.0);
                v
            };
            &padded_samples
        } else {
            samples
        };

        state.full(params, audio)
            .map_err(|e| anyhow::anyhow!("Wake word detection failed: {}", e))?;

        let num_segments = state.full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segments: {}", e))?;

        let mut text = String::new();
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        let result = text.to_lowercase();

        // Filter out repetition loops
        if Self::is_repetition(&result) {
            return Ok(String::new());
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- T020: has_speech_energy tests ---

    #[test]
    fn test_has_speech_energy_silence() {
        let silence = vec![0.0f32; 16000];
        assert!(!WhisperTranscriber::has_speech_energy(&silence));
    }

    #[test]
    fn test_has_speech_energy_loud_signal() {
        // Sine wave with amplitude 0.3 -> RMS ~0.21
        let signal: Vec<f32> = (0..16000)
            .map(|i| (i as f32 * 0.1).sin() * 0.3)
            .collect();
        assert!(WhisperTranscriber::has_speech_energy(&signal));
    }

    #[test]
    fn test_has_speech_energy_barely_above_threshold() {
        // Very quiet signal just above MIN_SPEECH_RMS (0.003)
        let signal: Vec<f32> = (0..16000)
            .map(|i| (i as f32 * 0.1).sin() * 0.005)
            .collect();
        let rms = WhisperTranscriber::rms_energy(&signal);
        assert!(rms > MIN_SPEECH_RMS, "RMS {} should be above threshold {}", rms, MIN_SPEECH_RMS);
        assert!(WhisperTranscriber::has_speech_energy(&signal));
    }

    #[test]
    fn test_has_speech_energy_barely_below_threshold() {
        // Very quiet signal below MIN_SPEECH_RMS
        let signal: Vec<f32> = (0..16000)
            .map(|i| (i as f32 * 0.1).sin() * 0.002)
            .collect();
        let rms = WhisperTranscriber::rms_energy(&signal);
        assert!(rms < MIN_SPEECH_RMS, "RMS {} should be below threshold {}", rms, MIN_SPEECH_RMS);
        assert!(!WhisperTranscriber::has_speech_energy(&signal));
    }

    #[test]
    fn test_has_speech_energy_empty() {
        assert!(!WhisperTranscriber::has_speech_energy(&[]));
    }

    // --- T020: rms_energy tests ---

    #[test]
    fn test_rms_energy_zeros() {
        assert_eq!(WhisperTranscriber::rms_energy(&[0.0; 100]), 0.0);
    }

    #[test]
    fn test_rms_energy_ones() {
        // All 1.0 -> RMS = 1.0
        let rms = WhisperTranscriber::rms_energy(&[1.0; 100]);
        assert!((rms - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_rms_energy_empty() {
        assert_eq!(WhisperTranscriber::rms_energy(&[]), 0.0);
    }

    // --- T020: is_repetition tests ---

    #[test]
    fn test_is_repetition_mais_loop() {
        assert!(WhisperTranscriber::is_repetition("mais mais mais mais mais"));
    }

    #[test]
    fn test_is_repetition_et_loop() {
        assert!(WhisperTranscriber::is_repetition("et et et et"));
    }

    #[test]
    fn test_is_repetition_normal_speech() {
        assert!(!WhisperTranscriber::is_repetition("bonjour comment ça va"));
    }

    #[test]
    fn test_is_repetition_short_text() {
        // Less than 3 words -> not a repetition
        assert!(!WhisperTranscriber::is_repetition("mais mais"));
        assert!(!WhisperTranscriber::is_repetition("oui"));
        assert!(!WhisperTranscriber::is_repetition(""));
    }

    #[test]
    fn test_is_repetition_partial_repeat() {
        // Only 2 repeats then different word -> not repetition
        assert!(!WhisperTranscriber::is_repetition("mais mais bonjour"));
    }

    #[test]
    fn test_is_repetition_exactly_three() {
        assert!(WhisperTranscriber::is_repetition("non non non"));
    }
}
