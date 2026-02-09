use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

use super::wake_word::WakeWordDetector;
use super::whisper::WhisperTranscriber;
use super::wav_loader;

/// Result of trigger word detection for a single audio segment
#[derive(Debug, Clone)]
pub struct DetectionResult {
    /// Whether the trigger word was detected in this segment
    pub detected: bool,
    /// Raw transcription from STT
    pub transcription: String,
    /// Extracted command after the trigger word (None if not detected)
    pub command: Option<String>,
    /// Detection confidence score (0.0 - 1.0)
    pub confidence: f32,
}

/// A segment of audio isolated by silence boundaries
#[derive(Debug, Clone)]
pub struct AudioSegment {
    /// Audio samples in f32 normalized [-1.0, 1.0] at 16kHz
    pub samples: Vec<f32>,
    /// Start position in the source audio (milliseconds)
    pub start_ms: u64,
    /// End position in the source audio (milliseconds)
    pub end_ms: u64,
    /// RMS energy of the segment
    pub rms_energy: f32,
}

/// Expected result for a single detection in a test sample
#[derive(Debug, Clone)]
pub struct ExpectedMessage {
    /// Should the trigger word be detected for this segment
    pub detected: bool,
    /// Expected command after extraction
    pub command: String,
}

/// Expected results for a test sample
#[derive(Debug, Clone)]
pub struct SampleExpectation {
    /// Sample identifier (e.g., "sample1")
    pub name: String,
    /// Human description of the audio
    pub description: String,
    /// Ordered list of expected detection results
    pub expected_messages: Vec<ExpectedMessage>,
}

/// Sample rate used by the pipeline (Whisper expects 16kHz)
const SAMPLE_RATE: u32 = 16000;

/// Segment audio by detecting silence gaps
///
/// Splits audio into segments wherever silence (RMS below threshold) lasts
/// longer than `min_silence_duration_ms`. This mimics the TS3 streaming behavior
/// where silence triggers transcription of the accumulated buffer.
pub fn segment_by_silence(
    samples: &[f32],
    sample_rate: u32,
    silence_threshold: f32,
    min_silence_duration_ms: u64,
) -> Vec<AudioSegment> {
    if samples.is_empty() {
        return Vec::new();
    }

    let window_size = (sample_rate as usize) / 20; // 50ms windows
    let min_silence_samples =
        (min_silence_duration_ms as usize * sample_rate as usize) / 1000;

    let mut segments = Vec::new();
    let mut segment_start = 0usize;
    let mut silence_start: Option<usize> = None;
    let mut in_silence = false;

    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + window_size).min(samples.len());
        let window = &samples[pos..end];
        let rms = rms_energy(window);

        if rms < silence_threshold {
            if !in_silence {
                silence_start = Some(pos);
                in_silence = true;
            }
            // Check if silence has lasted long enough to split
            if let Some(start) = silence_start {
                if pos - start >= min_silence_samples && pos > segment_start {
                    // We have a silence gap: emit the segment before the silence
                    let seg_samples = &samples[segment_start..start];
                    if !seg_samples.is_empty() {
                        let seg_rms = rms_energy(seg_samples);
                        // Only emit segments with speech energy
                        if seg_rms > silence_threshold {
                            segments.push(AudioSegment {
                                samples: seg_samples.to_vec(),
                                start_ms: (segment_start as u64 * 1000) / sample_rate as u64,
                                end_ms: (start as u64 * 1000) / sample_rate as u64,
                                rms_energy: seg_rms,
                            });
                        }
                    }
                    // Next segment starts after the silence
                    segment_start = end;
                    // Reset silence tracking to prevent re-triggering on same gap
                    silence_start = None;
                    in_silence = false;
                }
            }
        } else {
            in_silence = false;
            silence_start = None;
        }

        pos = end;
    }

    // Emit the final segment if there's remaining audio with speech energy
    if segment_start < samples.len() {
        let seg_samples = &samples[segment_start..];
        let seg_rms = rms_energy(seg_samples);
        if seg_rms > silence_threshold {
            segments.push(AudioSegment {
                samples: seg_samples.to_vec(),
                start_ms: (segment_start as u64 * 1000) / sample_rate as u64,
                end_ms: (samples.len() as u64 * 1000) / sample_rate as u64,
                rms_energy: seg_rms,
            });
        }
    }

    debug!(
        "Segmented audio into {} segments from {:.2}s of audio",
        segments.len(),
        samples.len() as f32 / sample_rate as f32
    );

    segments
}

/// Calculate RMS energy of audio samples
pub fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Parse samples.md file to extract test expectations
pub fn parse_samples_md(path: &Path) -> Result<Vec<SampleExpectation>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;

    let mut expectations = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_description = String::new();
    let mut current_messages: Vec<ExpectedMessage> = Vec::new();
    let mut in_expected = false;
    let mut pending_detected: Option<bool> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        // New sample section
        if trimmed.starts_with("## sample") {
            // Save previous sample if any
            if let Some(name) = current_name.take() {
                // Flush pending detected without command
                if let Some(det) = pending_detected.take() {
                    current_messages.push(ExpectedMessage {
                        detected: det,
                        command: String::new(),
                    });
                }
                expectations.push(SampleExpectation {
                    name,
                    description: current_description.clone(),
                    expected_messages: current_messages.clone(),
                });
            }
            current_name = Some(trimmed.trim_start_matches("## ").to_string());
            current_description.clear();
            current_messages.clear();
            in_expected = false;
            pending_detected = None;
            continue;
        }

        if current_name.is_none() {
            continue;
        }

        // Transcription line
        if trimmed.starts_with("Transcription:") {
            current_description = trimmed.trim_start_matches("Transcription:").trim().to_string();
            in_expected = false;
            continue;
        }

        // Expected section start
        if trimmed == "Expected:" {
            in_expected = true;
            continue;
        }

        if in_expected {
            // Parse detection status
            if trimmed.starts_with("- detected:") {
                // Flush any pending detected+command pair
                if let Some(det) = pending_detected.take() {
                    current_messages.push(ExpectedMessage {
                        detected: det,
                        command: String::new(),
                    });
                }
                let value = trimmed.trim_start_matches("- detected:").trim();
                pending_detected = Some(value == "yes");
            }
            // Parse command
            else if trimmed.starts_with("- command:") {
                let value = trimmed
                    .trim_start_matches("- command:")
                    .trim()
                    .trim_matches('"')
                    .to_string();
                let detected = pending_detected.take().unwrap_or(true);
                current_messages.push(ExpectedMessage {
                    detected,
                    command: value,
                });
            }
        }
    }

    // Save the last sample
    if let Some(name) = current_name {
        if let Some(det) = pending_detected.take() {
            current_messages.push(ExpectedMessage {
                detected: det,
                command: String::new(),
            });
        }
        expectations.push(SampleExpectation {
            name,
            description: current_description,
            expected_messages: current_messages,
        });
    }

    info!("Parsed {} sample expectations from {}", expectations.len(), path.display());
    Ok(expectations)
}

/// Wake word detection pipeline using a lightweight Whisper model (e.g. tiny)
///
/// Dedicated to wake word checks only — never blocked by transcription.
pub struct WakeWordPipeline {
    transcriber: WhisperTranscriber,
    detector: WakeWordDetector,
}

impl WakeWordPipeline {
    /// Create a new wake word pipeline
    ///
    /// - `model_path`: path to lightweight Whisper model (e.g., "models/ggml-tiny.bin")
    /// - `bot_name`: name of the bot for wake word detection (e.g., "marlbot")
    pub fn new(model_path: impl AsRef<Path>, bot_name: &str) -> Result<Self> {
        let mut transcriber = WhisperTranscriber::new(model_path)?;
        // Bias Whisper to recognize the bot name in audio
        transcriber.set_initial_prompt(&format!("Hey {}.", bot_name));
        let detector = WakeWordDetector::new(bot_name);

        Ok(Self {
            transcriber,
            detector,
        })
    }

    /// Known Whisper hallucination patterns (produced on silence/noise).
    /// These should never trigger wake word detection.
    const HALLUCINATION_PATTERNS: &'static [&'static str] = &[
        "[musique]",
        "[music]",
        "[applaudissements]",
        "[rires]",
        "[silence]",
        "[bruit]",
        "[bruits]",
        "merci d'avoir regardé",
        "merci d'avoir écouté",
        "sous-titres",
        "sous-titrage",
        "soustitres",
        "merci à tous",
        "à bientôt",
        "à la prochaine",
        "thank you for watching",
        "thanks for watching",
        "subscribe",
        "like and subscribe",
    ];

    /// Check if text is a known Whisper hallucination
    fn is_hallucination(text: &str) -> bool {
        let lower = text.trim().to_lowercase();
        // Remove surrounding brackets/punctuation for comparison
        let cleaned: String = lower.chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '[' || *c == ']')
            .collect();
        let cleaned = cleaned.trim();

        Self::HALLUCINATION_PATTERNS.iter().any(|pattern| {
            cleaned.contains(pattern)
        })
    }

    /// Quick wake word check on audio samples
    ///
    /// Uses the fast `transcribe_wake_word` mode (short token limit).
    /// Returns (detected, transcription_text).
    pub fn check_wake_word(&mut self, samples: &[f32]) -> Result<(bool, String)> {
        if samples.is_empty() {
            return Ok((false, String::new()));
        }

        let text = self.transcriber.transcribe_wake_word(samples)?;

        if text.is_empty() {
            return Ok((false, String::new()));
        }

        // Filter out known Whisper hallucinations before wake word matching
        if Self::is_hallucination(&text) {
            debug!("Whisper hallucination filtered: '{}'", text);
            return Ok((false, String::new()));
        }

        let detected = self.detector.detect(&text);
        Ok((detected, text))
    }
}

/// Transcription pipeline using a larger Whisper model (e.g. small)
///
/// Dedicated to full transcription after wake word is detected.
/// No wake word detection or stripping needed — audio starts after wake word.
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
    /// Returns the transcribed text. No wake word detection or stripping.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        self.transcriber.transcribe(samples, Some("fr"))
    }
}

/// The main trigger word detection pipeline (legacy, used by tests)
///
/// Assembles WhisperTranscriber + WakeWordDetector into a testable pipeline
/// that can process audio from WAV files or raw samples.
pub struct TriggerWordPipeline {
    transcriber: WhisperTranscriber,
    detector: WakeWordDetector,
    /// Silence threshold for audio segmentation
    silence_threshold: f32,
    /// Minimum silence duration (ms) to split segments
    min_silence_duration_ms: u64,
}

impl TriggerWordPipeline {
    /// Create a new pipeline
    ///
    /// - `model_path`: path to Whisper model file (e.g., "models/ggml-small.bin")
    /// - `bot_name`: name of the bot for wake word detection (e.g., "marlbot")
    pub fn new(model_path: impl AsRef<Path>, bot_name: &str) -> Result<Self> {
        let mut transcriber = WhisperTranscriber::new(model_path)?;
        // Bias Whisper to recognize the bot name in audio
        transcriber.set_initial_prompt(&format!("Hey {}.", bot_name));
        let detector = WakeWordDetector::new(bot_name);

        Ok(Self {
            transcriber,
            detector,
            silence_threshold: 0.005,
            min_silence_duration_ms: 800,
        })
    }

    /// Process raw audio samples and return detection results
    pub fn process_audio(&mut self, samples: &[f32]) -> Result<Vec<DetectionResult>> {
        let segments = segment_by_silence(
            samples,
            SAMPLE_RATE,
            self.silence_threshold,
            self.min_silence_duration_ms,
        );

        info!(
            "Processing {} audio segments ({:.2}s total)",
            segments.len(),
            samples.len() as f32 / SAMPLE_RATE as f32
        );

        let mut transcriptions: Vec<String> = Vec::new();
        for (i, segment) in segments.iter().enumerate() {
            debug!(
                "Segment {}: {:.2}s - {:.2}s (RMS: {:.4})",
                i,
                segment.start_ms as f32 / 1000.0,
                segment.end_ms as f32 / 1000.0,
                segment.rms_energy
            );

            let transcription = self
                .transcriber
                .transcribe(&segment.samples, Some("fr"))?;

            debug!("Segment {}: transcribed as '{}'", i, transcription);
            transcriptions.push(transcription);
        }

        let mut results = Vec::new();
        let mut pending_wake_word: Option<String> = None;

        for (i, transcription) in transcriptions.iter().enumerate() {
            if transcription.is_empty() {
                continue;
            }

            if let Some(wake_transcription) = pending_wake_word.take() {
                info!(
                    "Segment {}: treated as command for previous wake word",
                    i
                );
                results.push(DetectionResult {
                    detected: true,
                    transcription: format!("{} {}", wake_transcription, transcription),
                    command: Some(transcription.trim().to_string()),
                    confidence: 1.0,
                });
                continue;
            }

            let detected = self.detector.detect(transcription);

            if detected {
                let command = self.detector.extract_command_clean(transcription);

                match command {
                    Some(cmd) if cmd.split_whitespace().count() > 1 => {
                        info!("Segment {}: wake word + command: '{}'", i, cmd);
                        results.push(DetectionResult {
                            detected: true,
                            transcription: transcription.clone(),
                            command: Some(cmd),
                            confidence: 1.0,
                        });
                    }
                    _ => {
                        if i + 1 < transcriptions.len() {
                            info!(
                                "Segment {}: wake word only, deferring command to next segment",
                                i
                            );
                            pending_wake_word = Some(transcription.clone());
                        } else {
                            results.push(DetectionResult {
                                detected: true,
                                transcription: transcription.clone(),
                                command,
                                confidence: 1.0,
                            });
                        }
                    }
                }
            } else {
                debug!("Segment {}: no wake word detected", i);
                results.push(DetectionResult {
                    detected: false,
                    transcription: transcription.clone(),
                    command: None,
                    confidence: 0.0,
                });
            }
        }

        if let Some(wake_transcription) = pending_wake_word {
            results.push(DetectionResult {
                detected: true,
                transcription: wake_transcription,
                command: None,
                confidence: 1.0,
            });
        }

        Ok(results)
    }

    /// Process a WAV file and return detection results
    pub fn process_wav_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<DetectionResult>> {
        let path = path.as_ref();
        info!("Processing WAV file: {}", path.display());

        let samples = wav_loader::load_wav(path)?;
        self.process_audio(&samples)
    }

    /// Transcribe a pre-segmented audio buffer and check for wake word
    pub fn transcribe_and_detect(&mut self, samples: &[f32]) -> Result<DetectionResult> {
        if samples.is_empty() {
            return Ok(DetectionResult {
                detected: false,
                transcription: String::new(),
                command: None,
                confidence: 0.0,
            });
        }

        let transcription = self.transcriber.transcribe(samples, Some("fr"))?;

        if transcription.is_empty() {
            return Ok(DetectionResult {
                detected: false,
                transcription: String::new(),
                command: None,
                confidence: 0.0,
            });
        }

        let detected = self.detector.detect(&transcription);
        let command = if detected {
            self.detector.extract_command_clean(&transcription)
        } else {
            None
        };

        Ok(DetectionResult {
            detected,
            transcription,
            command,
            confidence: if detected { 1.0 } else { 0.0 },
        })
    }

    /// Quick wake word check on audio samples
    pub fn check_wake_word(&mut self, samples: &[f32]) -> Result<(bool, String)> {
        if samples.is_empty() {
            return Ok((false, String::new()));
        }

        let text = self.transcriber.transcribe_wake_word(samples)?;

        if text.is_empty() {
            return Ok((false, String::new()));
        }

        let detected = self.detector.detect(&text);
        Ok((detected, text))
    }

    /// Get a reference to the wake word detector (for testing)
    pub fn detector(&self) -> &WakeWordDetector {
        &self.detector
    }

    /// Get a mutable reference to the transcriber (for testing)
    pub fn transcriber_mut(&mut self) -> &mut WhisperTranscriber {
        &mut self.transcriber
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rms_energy_silence() {
        let silence = vec![0.0f32; 1600];
        assert_eq!(rms_energy(&silence), 0.0);
    }

    #[test]
    fn test_rms_energy_signal() {
        let signal: Vec<f32> = (0..1600)
            .map(|i| (i as f32 * 0.1).sin() * 0.5)
            .collect();
        let rms = rms_energy(&signal);
        assert!(rms > 0.0);
        assert!(rms < 1.0);
    }

    #[test]
    fn test_rms_energy_empty() {
        assert_eq!(rms_energy(&[]), 0.0);
    }

    #[test]
    fn test_segment_by_silence_no_audio() {
        let segments = segment_by_silence(&[], 16000, 0.005, 500);
        assert!(segments.is_empty());
    }

    #[test]
    fn test_segment_by_silence_continuous_speech() {
        // Simulated speech: constant tone (no silence gaps)
        let speech: Vec<f32> = (0..32000)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect();
        let segments = segment_by_silence(&speech, 16000, 0.005, 500);
        // Should produce exactly 1 segment (no silence to split on)
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn test_segment_by_silence_with_gap() {
        // Speech (1s) + Silence (1s) + Speech (1s)
        let mut audio = Vec::new();

        // 1 second of "speech" (sine wave)
        for i in 0..16000 {
            audio.push((i as f32 * 0.1).sin() * 0.3);
        }
        // 1 second of silence
        audio.extend(std::iter::repeat_n(0.0f32, 16000));
        // 1 second of "speech"
        for i in 0..16000 {
            audio.push((i as f32 * 0.1).sin() * 0.3);
        }

        let segments = segment_by_silence(&audio, 16000, 0.005, 500);
        assert_eq!(segments.len(), 2, "Expected 2 segments separated by silence");
        assert!(segments[0].rms_energy > 0.005);
        assert!(segments[1].rms_energy > 0.005);
    }

    #[test]
    fn test_parse_samples_md() {
        let path = Path::new("samples/samples.md");
        if !path.exists() {
            eprintln!("Skipping test: samples/samples.md not found");
            return;
        }

        let expectations = parse_samples_md(path).unwrap();
        assert!(expectations.len() >= 4, "Expected at least 4 samples, got {}", expectations.len());

        // sample1: 1 detection
        assert_eq!(expectations[0].name, "sample1");
        assert_eq!(expectations[0].expected_messages.len(), 1);
        assert!(expectations[0].expected_messages[0].detected);

        // sample2: 2 detections
        assert_eq!(expectations[1].name, "sample2");
        assert_eq!(expectations[1].expected_messages.len(), 2);

        // sample3: 1 detection
        assert_eq!(expectations[2].name, "sample3");
        assert_eq!(expectations[2].expected_messages.len(), 1);

        // sample4: 1 detection
        assert_eq!(expectations[3].name, "sample4");
        assert_eq!(expectations[3].expected_messages.len(), 1);
        assert_eq!(
            expectations[3].expected_messages[0].command,
            "comment ca va ?"
        );
    }
}
