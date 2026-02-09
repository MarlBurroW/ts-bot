//! Rustpotter-based wake word detector.
//!
//! Replaces the Whisper-based wake word pipeline with a purpose-built
//! wakeword spotter that runs on raw audio samples directly.

use anyhow::Result;
use rustpotter::{Rustpotter, RustpotterConfig, SampleFormat, AudioFmt, Endianness};
use tracing::{info, debug};

/// Wake word detector backed by Rustpotter.
///
/// Expects 16kHz mono f32 audio samples (same as the rest of the pipeline).
/// Call `process_samples` with chunks of `samples_per_frame()` samples.
pub struct RustpotterWakeWord {
    detector: Rustpotter,
    frame_size: usize,
    /// Internal buffer for accumulating samples between calls
    buffer: Vec<f32>,
    /// Frame counter for periodic logging
    frame_count: u64,
}

impl RustpotterWakeWord {
    /// Create a new detector from a `.rpw` wakeword model file.
    pub fn new(model_path: &str) -> Result<Self> {
        let mut config = RustpotterConfig::default();
        // Our audio pipeline delivers 16kHz mono f32
        config.fmt = AudioFmt {
            sample_rate: 16000,
            sample_format: SampleFormat::F32,
            channels: 1,
            endianness: Endianness::Native,
        };
        // Tuning: lower thresholds = more sensitive, higher = fewer false positives
        // Using very low thresholds since model was trained with TTS samples only
        config.detector.avg_threshold = 0.15;
        config.detector.threshold = 0.15;
        config.detector.min_scores = 2;
        config.detector.eager = true;

        // Enable gain normalization for consistent detection across volume levels
        config.filters.gain_normalizer.enabled = true;
        config.filters.gain_normalizer.min_gain = 0.5;
        config.filters.gain_normalizer.max_gain = 2.0;

        let mut detector = Rustpotter::new(&config)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        detector.add_wakeword_from_file("marlbot", model_path)
            .map_err(|e| anyhow::anyhow!("Failed to load wakeword model: {}", e))?;

        let frame_size = detector.get_samples_per_frame();
        info!(
            "Rustpotter wake word detector initialized (frame_size={} samples, {:.1}ms)",
            frame_size,
            frame_size as f32 / 16.0
        );

        Ok(Self {
            detector,
            frame_size,
            buffer: Vec::with_capacity(frame_size * 2),
            frame_count: 0,
        })
    }

    /// Number of samples needed per frame.
    pub fn samples_per_frame(&self) -> usize {
        self.frame_size
    }

    /// Feed audio samples and check for wake word detection.
    ///
    /// Accepts any number of f32 samples. Internally buffers and processes
    /// complete frames. Returns `true` if the wake word was detected in
    /// any of the processed frames.
    pub fn process_samples(&mut self, samples: &[f32]) -> bool {
        self.buffer.extend_from_slice(samples);

        let mut detected = false;
        self.frame_count += 1;
        while self.buffer.len() >= self.frame_size {
            let frame: Vec<f32> = self.buffer.drain(..self.frame_size).collect();
            // Log RMS every ~2 seconds (100 frames at 20ms) to verify audio flows
            if self.frame_count % 100 == 0 {
                let rms: f32 = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
                if rms > 0.005 {
                    info!("Rustpotter audio flowing (rms={:.4}, frames={})", rms, self.frame_count);
                }
            }
            if let Some(detection) = self.detector.process_samples(frame) {
                info!(
                    "🎯 Rustpotter detection: name='{}', score={:.3}, avg_score={:.3}",
                    detection.name, detection.score, detection.avg_score
                );
                detected = true;
            }
        }

        detected
    }

    /// Clear internal state (call after detection to avoid re-triggers).
    pub fn reset(&mut self) {
        self.buffer.clear();
        // Reset by clearing the internal buffer only
        // Rustpotter doesn't have a public clean/reset method in this version
        debug!("Rustpotter detector state reset");
    }

    /// Get the RMS level of the last processed frame.
    pub fn last_rms(&self) -> f32 {
        self.detector.get_rms_level()
    }
}
