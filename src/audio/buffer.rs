use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use super::OpusDecoder;

/// Audio buffer for a single speaker
pub struct AudioBuffer {
    pub speaker_id: u64,
    pub speaker_name: String,
    pub speaker_uid: String,
    /// PCM samples at 16kHz (f32 normalized)
    samples: VecDeque<f32>,
    /// Maximum buffer size (in samples)
    max_samples: usize,
    /// Last time audio was added
    last_activity: Instant,
    /// Last time a transcription check was run
    last_check: Instant,
    /// Is this speaker currently being transcribed?
    pub is_active: bool,
    /// Per-speaker Opus decoder (Opus is stateful per-stream)
    opus_decoder: OpusDecoder,
}

impl AudioBuffer {
    /// Create a new audio buffer
    /// max_duration: maximum buffer duration in seconds
    pub fn new(
        speaker_id: u64,
        speaker_name: String,
        speaker_uid: String,
        max_duration: Duration,
    ) -> Self {
        // 16kHz * duration in seconds
        let max_samples = (16000.0 * max_duration.as_secs_f32()) as usize;

        let opus_decoder = OpusDecoder::new().unwrap_or_else(|e| {
            warn!("Failed to create per-speaker Opus decoder: {}. Using default.", e);
            OpusDecoder::default()
        });

        Self {
            speaker_id,
            speaker_name,
            speaker_uid,
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
            last_activity: Instant::now(),
            last_check: Instant::now(),
            is_active: false,
            opus_decoder,
        }
    }

    /// Decode an Opus packet using this speaker's dedicated decoder,
    /// resample to 16kHz, and push the resulting samples into the buffer.
    /// Returns the number of f32 samples pushed (0 if decode failed or empty).
    pub fn decode_and_push(&mut self, opus_data: &[u8]) -> usize {
        match self.opus_decoder.decode(opus_data) {
            Ok(pcm) => {
                if pcm.is_empty() {
                    return 0;
                }
                let samples_f32 = OpusDecoder::resample_to_16khz(&pcm);
                self.push_samples(&samples_f32);
                samples_f32.len()
            }
            Err(e) => {
                debug!("Per-speaker Opus decode error for {}: {}", self.speaker_name, e);
                0
            }
        }
    }

    /// Add audio samples to the buffer
    pub fn push_samples(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.samples.len() >= self.max_samples {
                self.samples.pop_front(); // Remove oldest sample
            }
            self.samples.push_back(sample);
        }
        // Only update activity timestamp if there's actual speech (not silence)
        let rms = if samples.is_empty() { 0.0 } else {
            let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
            (sum_sq / samples.len() as f32).sqrt()
        };
        if rms > 0.005 {
            self.last_activity = Instant::now();
        }
    }

    /// Get all buffered samples
    pub fn get_samples(&self) -> Vec<f32> {
        self.samples.iter().copied().collect()
    }

    /// Get samples from the last N seconds
    pub fn get_recent_samples(&self, duration: Duration) -> Vec<f32> {
        let sample_count = (16000.0 * duration.as_secs_f32()) as usize;
        let start_idx = self.samples.len().saturating_sub(sample_count);
        self.samples.iter().skip(start_idx).copied().collect()
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.samples.clear();
        info!("Cleared audio buffer for speaker {}", self.speaker_name);
    }

    /// Keep only the most recent N seconds of audio, discard older samples
    pub fn trim_to_recent(&mut self, duration: Duration) {
        let keep_samples = (16000.0 * duration.as_secs_f32()) as usize;
        if self.samples.len() > keep_samples {
            let discard = self.samples.len() - keep_samples;
            self.samples.drain(..discard);
            debug!("Trimmed buffer for speaker {}, kept last {:.1}s", self.speaker_name, duration.as_secs_f32());
        }
    }

    /// Get duration of buffered audio in seconds
    pub fn duration(&self) -> f32 {
        self.samples.len() as f32 / 16000.0
    }

    /// Check if the buffer has no audio samples
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Check if speaker has been silent for duration
    pub fn is_silent_for(&self, duration: Duration) -> bool {
        self.last_activity.elapsed() > duration
    }

    /// Mark that a transcription check was just run
    pub fn mark_check(&mut self) {
        self.last_check = Instant::now();
    }

    /// Activate transcription for this speaker
    pub fn activate(&mut self) {
        self.is_active = true;
        // Reset activity timer so silence timeout counts from activation, not old audio
        self.last_activity = Instant::now();
        info!("Activated transcription for speaker {}", self.speaker_name);
    }

    /// Deactivate transcription for this speaker
    pub fn deactivate(&mut self) {
        self.is_active = false;
        info!("Deactivated transcription for speaker {}", self.speaker_name);
    }
}

/// Manager for all speaker audio buffers
pub struct SpeakerBufferManager {
    buffers: HashMap<u64, AudioBuffer>,
    max_buffer_duration: Duration,
    /// Silence timeout after user has started speaking
    silence_timeout: Duration,
    /// Longer timeout waiting for user to START speaking after activation
    initial_timeout: Duration,
}

impl SpeakerBufferManager {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            max_buffer_duration: Duration::from_secs(30), // 30 seconds max
            silence_timeout: Duration::from_millis(2000), // 2s after speech started
            initial_timeout: Duration::from_secs(7),      // 7s to start speaking after "J'écoute"
        }
    }

    /// Get or create buffer for a speaker
    pub fn get_or_create_buffer(
        &mut self,
        speaker_id: u64,
        speaker_name: String,
        speaker_uid: String,
    ) -> &mut AudioBuffer {
        self.buffers.entry(speaker_id).or_insert_with(|| {
            debug!("Creating new audio buffer for speaker {}", speaker_name);
            AudioBuffer::new(
                speaker_id,
                speaker_name,
                speaker_uid,
                self.max_buffer_duration,
            )
        })
    }

    /// Get buffer for a speaker if it exists
    pub fn get_buffer(&self, speaker_id: u64) -> Option<&AudioBuffer> {
        self.buffers.get(&speaker_id)
    }

    /// Get mutable buffer for a speaker if it exists
    pub fn get_buffer_mut(&mut self, speaker_id: u64) -> Option<&mut AudioBuffer> {
        self.buffers.get_mut(&speaker_id)
    }

    /// Add audio samples for a speaker
    pub fn add_audio(
        &mut self,
        speaker_id: u64,
        speaker_name: String,
        speaker_uid: String,
        samples: &[f32],
    ) {
        let buffer = self.get_or_create_buffer(speaker_id, speaker_name, speaker_uid);
        buffer.push_samples(samples);
    }

    /// Check if any active speaker should be deactivated due to silence
    /// Uses a two-phase timeout:
    /// - initial_timeout (4s): waiting for user to start speaking after "J'écoute"
    /// - silence_timeout (1.5s): detecting end of speech once user has started talking
    pub fn check_silence_timeouts(&mut self) -> Vec<u64> {
        let mut to_deactivate = Vec::new();

        for (speaker_id, buffer) in self.buffers.iter_mut() {
            if buffer.is_active {
                // Use longer timeout if no audio received since activation
                let timeout = if buffer.is_empty() {
                    self.initial_timeout
                } else {
                    self.silence_timeout
                };
                if buffer.is_silent_for(timeout) {
                    buffer.deactivate();
                    to_deactivate.push(*speaker_id);
                }
            }
        }

        to_deactivate
    }

    /// Get all active speakers
    pub fn get_active_speakers(&self) -> Vec<u64> {
        self.buffers
            .iter()
            .filter(|(_, buffer)| buffer.is_active)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get current silence timeout in milliseconds
    pub fn silence_timeout_ms(&self) -> u64 {
        self.silence_timeout.as_millis() as u64
    }

    /// Set silence timeout (clamped to 500ms–10000ms)
    pub fn set_silence_timeout_ms(&mut self, ms: u64) {
        let clamped = ms.clamp(500, 10000);
        self.silence_timeout = Duration::from_millis(clamped);
    }

    /// Cleanup old inactive buffers
    pub fn cleanup_old_buffers(&mut self, max_age: Duration) {
        let to_remove: Vec<u64> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| !buffer.is_active && buffer.is_silent_for(max_age))
            .map(|(id, _)| *id)
            .collect();

        for speaker_id in to_remove {
            self.buffers.remove(&speaker_id);
            debug!("Removed old buffer for speaker {}", speaker_id);
        }
    }
}

impl Default for SpeakerBufferManager {
    fn default() -> Self {
        Self::new()
    }
}
