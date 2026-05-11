//! Live audio mixing and WebM/Opus broadcasting for HTTP listeners.
//!
//! Architecture
//! ============
//! - Audio receive loop calls `LiveAudioStream::submit(speaker_id, &pcm_48k_i16)`
//!   with the *already decoded* 48 kHz mono PCM (i16) for each TS3 voice packet.
//! - A background task maintains a per-speaker ring of pending samples, ticks
//!   every 20 ms (matching a TS3 / Opus frame), pops 960 samples per speaker,
//!   sums them with int32 accumulation + i16 clipping, and feeds the result
//!   into a long-lived `ffmpeg` subprocess that re-encodes to WebM/Opus.
//! - `ffmpeg`'s stdout chunks are pushed to a `tokio::sync::broadcast::Sender`
//!   that HTTP `/audio/live` handlers subscribe to.
//! - When the broadcast has zero subscribers, the ffmpeg process is reaped and
//!   the mixing task drops queued samples (zero CPU bypass). It restarts on
//!   demand the first time a client subscribes.
//!
//! Rationale for ffmpeg-subprocess vs pure Rust
//! --------------------------------------------
//! WebM/Matroska muxing of streaming Opus is non-trivial. No mature crate in
//! the project's existing dep tree exposes a streaming WebM muxer that pairs
//! cleanly with `audiopus`. ffmpeg is already installed on the host, has a
//! battle-tested WebM/Opus pipeline, and lets us turn the feature on/off
//! without rebuilding ffmpeg-rs bindings. Cost: ~30 MB RSS + ~1 % CPU while
//! at least one listener is connected. Bypass when nobody is listening keeps
//! idle cost at zero.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc};
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

/// Broadcast channel capacity (number of WebM chunks buffered per subscriber).
const BROADCAST_CAPACITY: usize = 256;

/// Maximum per-speaker pending sample queue (in samples @ 48 kHz mono i16).
/// 2 seconds = 96 000 samples. Drops oldest to bound memory if a speaker
/// bursts faster than the 20 ms mixing tick can drain.
const MAX_SPEAKER_QUEUE: usize = 48_000 * 2;

/// Frame size for the 20 ms tick: 48 kHz * 0.020 = 960 samples.
const FRAME_SAMPLES: usize = 960;

#[derive(Clone)]
pub struct LiveAudioStream {
    inner: Arc<LiveAudioInner>,
}

struct LiveAudioInner {
    /// Outbound broadcast of WebM/Opus chunks.
    broadcast_tx: broadcast::Sender<Bytes>,
    /// Per-speaker decoded PCM (48 kHz mono i16) ingress.
    audio_in_tx: mpsc::UnboundedSender<(u64, Vec<i16>)>,
}

impl LiveAudioStream {
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (audio_in_tx, audio_in_rx) = mpsc::unbounded_channel();

        let bcast_for_task = broadcast_tx.clone();
        tokio::spawn(async move {
            run_pipeline(audio_in_rx, bcast_for_task).await;
        });

        Self {
            inner: Arc::new(LiveAudioInner {
                broadcast_tx,
                audio_in_tx,
            }),
        }
    }

    /// Are there currently any HTTP listeners subscribed?
    pub fn has_listeners(&self) -> bool {
        self.inner.broadcast_tx.receiver_count() > 0
    }

    /// Current number of subscribed HTTP listeners.
    pub fn subscriber_count(&self) -> usize {
        self.inner.broadcast_tx.receiver_count()
    }

    /// Submit a decoded 48 kHz mono i16 PCM slice for a given TS3 speaker.
    /// No-op when no listeners are connected (zero-CPU bypass).
    pub fn submit(&self, speaker_id: u64, pcm_48k_i16: &[i16]) {
        if pcm_48k_i16.is_empty() {
            return;
        }
        if !self.has_listeners() {
            return;
        }
        // Bounded `unbounded_channel` in practice — only "unbounded" name.
        let _ = self.inner.audio_in_tx.send((speaker_id, pcm_48k_i16.to_vec()));
    }

    /// Subscribe to the WebM/Opus broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<Bytes> {
        self.inner.broadcast_tx.subscribe()
    }
}

impl Default for LiveAudioStream {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn the ffmpeg subprocess for raw s16le → WebM/Opus.
async fn spawn_ffmpeg() -> std::io::Result<Child> {
    Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "warning",
            "-f", "s16le",
            "-ar", "48000",
            "-ac", "1",
            "-i", "pipe:0",
            "-c:a", "libopus",
            "-b:a", "64k",
            "-application", "audio",
            "-frame_duration", "20",
            "-vbr", "on",
            "-f", "webm",
            "-flush_packets", "1",
            "-cluster_size_limit", "8192",
            "-cluster_time_limit", "100",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// Background task: ticks every 20 ms, mixes per-speaker queues, feeds ffmpeg.
async fn run_pipeline(
    mut audio_in: mpsc::UnboundedReceiver<(u64, Vec<i16>)>,
    bcast: broadcast::Sender<Bytes>,
) {
    let mut speaker_queues: HashMap<u64, VecDeque<i16>> = HashMap::new();
    let mut active = false;
    let mut ffmpeg: Option<Child> = None;
    let mut stdin: Option<ChildStdin> = None;
    let mut stdout_task: Option<tokio::task::JoinHandle<()>> = None;

    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            maybe_pkt = audio_in.recv() => {
                let Some((sid, samples)) = maybe_pkt else { break; };
                if !active {
                    // No listeners — drop incoming audio.
                    continue;
                }
                let q = speaker_queues.entry(sid).or_default();
                q.extend(samples.iter().copied());
                if q.len() > MAX_SPEAKER_QUEUE {
                    let to_drop = q.len() - MAX_SPEAKER_QUEUE;
                    q.drain(..to_drop);
                    debug!("Live stream: dropped {} backlog samples for speaker {}", to_drop, sid);
                }
            }

            _ = tick.tick() => {
                let want_active = bcast.receiver_count() > 0;

                // State transitions.
                if want_active && !active {
                    info!("Live audio: first listener connected — spawning ffmpeg");
                    match spawn_ffmpeg().await {
                        Ok(mut child) => {
                            stdin = child.stdin.take();
                            if let Some(stdout) = child.stdout.take() {
                                let bcast_clone = bcast.clone();
                                stdout_task = Some(tokio::spawn(read_ffmpeg_stdout(stdout, bcast_clone)));
                            }
                            ffmpeg = Some(child);
                            active = true;
                        }
                        Err(e) => {
                            warn!("Failed to spawn ffmpeg for live audio: {}", e);
                            // Sleep a beat so we don't hot-loop on persistent failure.
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                } else if !want_active && active {
                    info!("Live audio: last listener gone — tearing down ffmpeg");
                    stdin.take(); // close stdin → EOF to ffmpeg
                    if let Some(mut c) = ffmpeg.take() {
                        let _ = c.kill().await;
                    }
                    if let Some(h) = stdout_task.take() {
                        h.abort();
                    }
                    speaker_queues.clear();
                    active = false;
                    continue;
                }

                if !active {
                    continue;
                }

                // Mix one 20 ms frame from each speaker queue.
                let mut mixed = [0i32; FRAME_SAMPLES];
                for q in speaker_queues.values_mut() {
                    let take = q.len().min(FRAME_SAMPLES);
                    for (i, s) in q.drain(..take).enumerate() {
                        mixed[i] += s as i32;
                    }
                }
                speaker_queues.retain(|_, q| !q.is_empty());

                // Clip + serialize little-endian s16.
                let mut bytes = Vec::with_capacity(FRAME_SAMPLES * 2);
                for s in mixed.iter() {
                    let clipped = (*s).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    bytes.extend_from_slice(&clipped.to_le_bytes());
                }

                if let Some(ref mut sin) = stdin {
                    if let Err(e) = sin.write_all(&bytes).await {
                        warn!("ffmpeg stdin write error: {} — restarting pipeline", e);
                        stdin.take();
                        if let Some(mut c) = ffmpeg.take() { let _ = c.kill().await; }
                        if let Some(h) = stdout_task.take() { h.abort(); }
                        speaker_queues.clear();
                        active = false;
                    }
                }
            }
        }
    }
}

async fn read_ffmpeg_stdout(
    mut stdout: tokio::process::ChildStdout,
    bcast: broadcast::Sender<Bytes>,
) {
    let mut buf = vec![0u8; 8192];
    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => {
                debug!("ffmpeg stdout EOF");
                break;
            }
            Ok(n) => {
                let chunk = Bytes::copy_from_slice(&buf[..n]);
                // Ignore send errors — they just mean all receivers were dropped.
                let _ = bcast.send(chunk);
            }
            Err(e) => {
                warn!("ffmpeg stdout read error: {}", e);
                break;
            }
        }
    }
}
