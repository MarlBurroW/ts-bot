//! Incoming microphone (push-to-talk) streaming from the mini-app to TS3.
//!
//! Symmetric counterpart of `stream.rs` (`LiveAudioStream`).
//!
//! Architecture
//! ============
//! HTTP `POST /audio/in` accepts a chunked `audio/webm; codecs=opus` body
//! produced by the browser's MediaRecorder. The body bytes are piped to a
//! long-lived `ffmpeg` subprocess that decodes WebM/Opus → s16le 48 kHz mono
//! PCM on stdout. A reader task drains stdout, accumulates samples into 20 ms
//! frames (960 samples mono), re-encodes each frame with `audiopus::Encoder`
//! (Opus, Voip application profile) and ships the resulting Opus payload to
//! TS3 via the shared `SyncConnectionHandle`, paced by a 20 ms `tokio::time`
//! interval — exactly like `TtsPlayback`.
//!
//! Only one mic-in session is active at a time. Starting a new session aborts
//! any in-flight one (via a shared cancellation flag). Starting a session also
//! interrupts any ongoing TTS playback via the shared `tts_stop_flag`.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use audiopus::coder::Encoder;
use audiopus::{Application, Channels, SampleRate};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};
use tsproto_packets::packets::{AudioData, CodecType, OutAudio};

use crate::websocket::SharedTs3Handle;

/// 20 ms @ 48 kHz mono = 960 samples per frame.
const FRAME_SAMPLES: usize = 960;
/// Max Opus encoded payload size.
const MAX_OPUS_FRAME_BYTES: usize = 1275;

/// Marker for the currently active session. A new request bumps the
/// generation and asks the running session to abort by flipping `abort`.
struct ActiveSession {
    abort: Arc<AtomicBool>,
}

/// Shared state for the mic-in feature, plugged into `AppState`.
#[derive(Clone)]
pub struct MicInState {
    /// At most one active session. Replaced atomically on each new request.
    session: Arc<Mutex<Option<ActiveSession>>>,
    /// Cloned from the main TTS stop flag — used to interrupt any in-flight
    /// TTS playback when the user starts pushing the talk button.
    pub tts_stop_flag: Option<Arc<AtomicBool>>,
}

impl MicInState {
    pub fn new(tts_stop_flag: Option<Arc<AtomicBool>>) -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            tts_stop_flag,
        }
    }
}

/// Spawn ffmpeg with stdin=WebM/Opus, stdout=s16le 48 kHz mono.
async fn spawn_ffmpeg() -> std::io::Result<Child> {
    Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "warning",
            "-i", "pipe:0",
            "-f", "s16le",
            "-ar", "48000",
            "-ac", "1",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// Run a full mic-in session. Awaits body completion and Opus drain before
/// returning. Returns `Ok(frames_sent)` on success.
pub async fn run_session<S, E>(
    mut body_stream: S,
    state: MicInState,
    ts3_handle: SharedTs3Handle,
) -> anyhow::Result<u64>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Unpin + Send + 'static,
    E: std::fmt::Debug + Send + 'static,
{
    // 1. Interrupt any ongoing TTS playback — priority to the live mic.
    if let Some(ref flag) = state.tts_stop_flag {
        if flag.load(Ordering::Relaxed) {
            flag.store(false, Ordering::Relaxed);
            info!("Mic-in: interrupted TTS playback");
        }
    }

    // 2. Tell any running session to abort, then claim the slot.
    let my_abort = Arc::new(AtomicBool::new(false));
    {
        let mut guard = state.session.lock().await;
        if let Some(prev) = guard.take() {
            prev.abort.store(true, Ordering::Relaxed);
            info!("Mic-in: signaled previous session to abort");
        }
        *guard = Some(ActiveSession { abort: my_abort.clone() });
    }

    // 3. Spawn ffmpeg.
    let mut child = spawn_ffmpeg()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ffmpeg for mic-in: {}", e))?;
    let mut ffmpeg_stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to take ffmpeg stdin"))?;
    let mut ffmpeg_stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to take ffmpeg stdout"))?;

    info!("Mic-in: session started");

    // 4. Body → ffmpeg stdin forwarder (sequential, awaited by the handler).
    let abort_for_body = my_abort.clone();
    let body_task = tokio::spawn(async move {
        let mut total_bytes: u64 = 0;
        while let Some(chunk) = body_stream.next().await {
            if abort_for_body.load(Ordering::Relaxed) {
                debug!("Mic-in: body task observed abort flag");
                break;
            }
            match chunk {
                Ok(bytes) => {
                    if bytes.is_empty() { continue; }
                    if let Err(e) = ffmpeg_stdin.write_all(&bytes).await {
                        warn!("Mic-in: ffmpeg stdin write error: {} (client likely closed)", e);
                        break;
                    }
                    total_bytes += bytes.len() as u64;
                }
                Err(e) => {
                    debug!("Mic-in: body chunk error: {:?}", e);
                    break;
                }
            }
        }
        // Close stdin → tells ffmpeg there is no more input → it flushes
        // remaining PCM on stdout and exits, which the encoder task observes
        // via stdout EOF.
        let _ = ffmpeg_stdin.shutdown().await;
        drop(ffmpeg_stdin);
        info!("Mic-in: body stream ended ({} bytes forwarded to ffmpeg)", total_bytes);
    });

    // 5. ffmpeg stdout → Opus encode → TS3 send task, paced at 20 ms.
    let abort_for_enc = my_abort.clone();
    let ts3_handle_clone = ts3_handle.clone();
    let encoder_task = tokio::spawn(async move {
        let encoder = match Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip) {
            Ok(e) => e,
            Err(e) => {
                warn!("Mic-in: failed to create Opus encoder: {:?}", e);
                return 0u64;
            }
        };

        let mut pcm_buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 4);
        let mut read_buf = vec![0u8; 8192];
        let mut leftover_byte: Option<u8> = None;
        let mut opus_out = [0u8; MAX_OPUS_FRAME_BYTES];

        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(MissedTickBehavior::Burst);
        // Burn the immediate first tick so the first real frame waits 20 ms.
        tick.tick().await;

        let frame_count_total = AtomicU64::new(0);
        let mut eof = false;

        loop {
            if abort_for_enc.load(Ordering::Relaxed) {
                debug!("Mic-in: encoder task observed abort flag");
                break;
            }

            if pcm_buf.len() >= FRAME_SAMPLES {
                tick.tick().await;
                let peak: u16 = pcm_buf[..FRAME_SAMPLES]
                    .iter()
                    .map(|s| s.unsigned_abs())
                    .max()
                    .unwrap_or(0);

                let opus_len = match encoder.encode(&pcm_buf[..FRAME_SAMPLES], &mut opus_out) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!("Mic-in: Opus encode error: {:?}", e);
                        pcm_buf.drain(..FRAME_SAMPLES);
                        continue;
                    }
                };
                let opus_frame = opus_out[..opus_len].to_vec();
                pcm_buf.drain(..FRAME_SAMPLES);

                let mut handle_guard = ts3_handle_clone.lock().await;
                if let Some(ref mut sender) = *handle_guard {
                    let frame_data = opus_frame;
                    let _ = sender
                        .with_connection(move |con| {
                            let packet = OutAudio::new(&AudioData::C2S {
                                id: 0,
                                codec: CodecType::OpusVoice,
                                data: &frame_data,
                            });
                            let _ = con.send_audio(packet);
                        })
                        .await;
                }
                drop(handle_guard);

                let n = frame_count_total.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 50 == 0 {
                    info!("Mic-in: sent {} Opus frames (last frame peak={})", n, peak);
                }
                continue;
            }

            if eof {
                break;
            }

            match ffmpeg_stdout.read(&mut read_buf).await {
                Ok(0) => {
                    debug!("Mic-in: ffmpeg stdout EOF");
                    eof = true;
                    if !pcm_buf.is_empty() && pcm_buf.len() < FRAME_SAMPLES {
                        pcm_buf.resize(FRAME_SAMPLES, 0);
                    }
                    if pcm_buf.is_empty() {
                        break;
                    }
                }
                Ok(n) => {
                    let mut start = 0usize;
                    if let Some(b0) = leftover_byte.take() {
                        if n >= 1 {
                            let s = i16::from_le_bytes([b0, read_buf[0]]);
                            pcm_buf.push(s);
                            start = 1;
                        } else {
                            leftover_byte = Some(b0);
                        }
                    }
                    let mut i = start;
                    while i + 1 < n {
                        let s = i16::from_le_bytes([read_buf[i], read_buf[i + 1]]);
                        pcm_buf.push(s);
                        i += 2;
                    }
                    if i < n {
                        leftover_byte = Some(read_buf[i]);
                    }
                    let cap = FRAME_SAMPLES * 50; // ~1 s
                    if pcm_buf.len() > cap {
                        let drop_n = pcm_buf.len() - cap;
                        pcm_buf.drain(..drop_n);
                        warn!("Mic-in: dropped {} backlog samples to bound memory", drop_n);
                    }
                }
                Err(e) => {
                    warn!("Mic-in: ffmpeg stdout read error: {}", e);
                    break;
                }
            }
        }

        let total = frame_count_total.load(Ordering::Relaxed);
        info!("Mic-in: session ended (sent {} opus frames)", total);
        total
    });

    // 6. Await body completion, then drain the encoder.
    let _ = body_task.await;
    // Hard ceiling on encoder drain time (5 s) to keep the HTTP request
    // bounded if something is wedged.
    let drain_timeout = Duration::from_secs(5);
    let total_frames = match tokio::time::timeout(drain_timeout, encoder_task).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            warn!("Mic-in: encoder task join error: {}", e);
            0
        }
        Err(_) => {
            warn!("Mic-in: encoder drain timed out, forcing abort");
            my_abort.store(true, Ordering::Relaxed);
            0
        }
    };

    // 7. Kill ffmpeg explicitly (kill_on_drop will handle it too, but be explicit).
    let _ = child.kill().await;

    // 8. Clear the session slot if we are still the active one.
    {
        let mut guard = state.session.lock().await;
        if let Some(ref s) = *guard {
            if Arc::ptr_eq(&s.abort, &my_abort) {
                *guard = None;
            }
        }
    }

    Ok(total_frames)
}
