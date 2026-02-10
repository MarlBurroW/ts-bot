use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use tsproto_packets::packets::{AudioData, CodecType, OutAudio};

use super::TtsSynthesizer;
use crate::audio::encoder::OpusEncoder;
use crate::audio::decoder::OpusDecoder;

/// Request sent to the playback task
struct PlaybackRequest {
    /// Pre-encoded Opus frames ready to send to TS3
    opus_frames: Vec<Vec<u8>>,
    /// Original text (for event reporting)
    text: String,
}

/// Audio player that sends TTS audio to TeamSpeak 3.
///
/// Architecture:
/// - `play()` synthesizes text → resamples → encodes Opus → sends to playback task
/// - The playback task paces frames at 20ms intervals via `with_connection(send_audio)`
/// - `stop()` interrupts playback immediately (e.g., when wake word detected)
pub struct AudioPlayer {
    /// Channel to send encoded frames to the playback task
    request_tx: mpsc::Sender<PlaybackRequest>,
    /// Shared flag to interrupt playback — can be shared externally for remote stop
    is_speaking: Arc<AtomicBool>,
}

impl AudioPlayer {
    /// Get a clone of the is_speaking flag (for external stop control)
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.is_speaking.clone()
    }

    /// Create a new AudioPlayer and spawn the background playback task.
    ///
    /// - `ts3_sender`: handle to the TS3 connection for sending audio packets
    /// - `event_tx`: broadcast channel to notify WebSocket clients of speak events
    pub fn new(
        ts3_sender: tsclientlib::sync::SyncConnectionHandle,
        event_tx: tokio::sync::broadcast::Sender<crate::models::WebSocketEvent>,
    ) -> Self {
        Self::with_stop_flag(ts3_sender, event_tx, None)
    }

    /// Create a new AudioPlayer with an optional external stop flag.
    /// If provided, the flag is shared so external code can stop playback.
    pub fn with_stop_flag(
        mut ts3_sender: tsclientlib::sync::SyncConnectionHandle,
        event_tx: tokio::sync::broadcast::Sender<crate::models::WebSocketEvent>,
        external_flag: Option<Arc<AtomicBool>>,
    ) -> Self {
        let (request_tx, mut request_rx) = mpsc::channel::<PlaybackRequest>(16);
        let is_speaking = external_flag.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let is_speaking_clone = is_speaking.clone();

        // Spawn the playback task
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let frame_count = request.opus_frames.len();
                info!(
                    "Playing {} Opus frames ({:.1}s) for: '{}'",
                    frame_count,
                    frame_count as f32 * 0.02,
                    request.text
                );

                // Notify: speak started
                let _ = event_tx.send(
                    crate::models::WebSocketEvent::speak_started(request.text.clone()),
                );

                is_speaking_clone.store(true, Ordering::Relaxed);
                let start = std::time::Instant::now();

                // Use interval instead of sleep to maintain constant 20ms pacing.
                // sleep(20ms) after each send accumulates drift (send_time + 20ms per frame).
                // interval(20ms) compensates: if send takes 5ms, it waits only 15ms more.
                let mut interval = tokio::time::interval(Duration::from_millis(20));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

                for (i, frame) in request.opus_frames.iter().enumerate() {
                    interval.tick().await;

                    // Check for interruption
                    if !is_speaking_clone.load(Ordering::Relaxed) {
                        info!("Playback interrupted at frame {}/{}", i, frame_count);
                        break;
                    }

                    let frame_data = frame.clone();
                    match ts3_sender
                        .with_connection(move |con| {
                            let packet = OutAudio::new(&AudioData::C2S {
                                id: 0,
                                codec: CodecType::OpusVoice,
                                data: &frame_data,
                            });
                            let _ = con.send_audio(packet);
                        })
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Failed to send audio frame {}: {:?}", i, e);
                            break;
                        }
                    }
                }

                let elapsed = start.elapsed();
                is_speaking_clone.store(false, Ordering::Relaxed);

                // Notify: speak completed
                let _ = event_tx.send(
                    crate::models::WebSocketEvent::speak_completed(
                        request.text,
                        elapsed.as_millis() as u64,
                    ),
                );

                info!("Playback finished in {:.1}s", elapsed.as_secs_f32());
            }
        });

        Self {
            request_tx,
            is_speaking,
        }
    }

    /// Synthesize text and play it on TS3.
    ///
    /// This is a non-blocking call — the actual playback happens in the background task.
    /// Uses `spawn_blocking` for the CPU-bound TTS synthesis.
    pub async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        synthesizer: Arc<dyn TtsSynthesizer>,
    ) -> Result<()> {
        let text_for_synth = text.clone();

        // CPU-bound: synthesize + resample + encode in a blocking task
        let opus_frames = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<u8>>> {
            // 1. Synthesize text → PCM (TTS-specific sample rate)
            let tts_audio = synthesizer.synthesize(&text_for_synth, voice.as_deref())?;
            info!(
                "{} produced {} samples at {}Hz ({:.1}s)",
                synthesizer.name(),
                tts_audio.samples.len(),
                tts_audio.sample_rate,
                tts_audio.samples.len() as f32 / tts_audio.sample_rate as f32
            );

            // 2. Resample to 48kHz for TS3
            let samples_48k = match tts_audio.sample_rate {
                48000 => tts_audio.samples,
                24000 => OpusDecoder::resample_24k_to_48k(&tts_audio.samples),
                other => {
                    anyhow::bail!(
                        "Unsupported TTS sample rate: {}Hz (expected 24000 or 48000)",
                        other
                    );
                }
            };

            // 3. Encode to Opus frames
            let mut encoder = OpusEncoder::new()?;
            let frames = encoder.encode_all(&samples_48k)?;

            Ok(frames)
        })
        .await??;

        if opus_frames.is_empty() {
            debug!("No audio frames to play");
            return Ok(());
        }

        // Send to the playback task
        self.request_tx
            .send(PlaybackRequest {
                opus_frames,
                text,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Playback task is not running"))?;

        Ok(())
    }

    /// Synthesize text and play it on TS3, interrupting any ongoing playback first.
    ///
    /// Use this when you want to replace the current playback instead of queuing.
    pub async fn speak_or_replace(
        &self,
        text: String,
        voice: Option<String>,
        synthesizer: Arc<dyn TtsSynthesizer>,
    ) -> Result<()> {
        self.stop();
        self.speak(text, voice, synthesizer).await
    }

    /// Play pre-encoded cached opus frames directly (skipping TTS synthesis).
    pub async fn play_cached(&self, frames: Vec<Vec<u8>>, label: String) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        self.request_tx
            .send(PlaybackRequest {
                opus_frames: frames,
                text: label,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Playback task is not running"))?;
        Ok(())
    }

    /// Stop any ongoing playback immediately
    pub fn stop(&self) {
        if self.is_speaking.load(Ordering::Relaxed) {
            info!("Stopping TTS playback");
            self.is_speaking.store(false, Ordering::Relaxed);
        }
    }

    /// Check if the bot is currently speaking
    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::Relaxed)
    }
}
