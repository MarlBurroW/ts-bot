mod ts3;

/// Truncate a string to at most `max_bytes` bytes without splitting a UTF-8 char.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

use anyhow::Result;
use ts3_bot::models::{BotConfig, MessageEvent, MessageType, WebSocketEvent, TranscriptionEvent};
use ts3_bot::websocket;
use ts3_bot::websocket::TtsRequest;
use ts3_bot::tts::{AudioPlayer, HttpTtsSynthesizer, TtsSynthesizer};
use tracing::{error, info, warn, debug};
use ts3::client::TS3Client;
use futures::prelude::*;
use tokio::sync::broadcast;
use tsproto_packets::packets::AudioData;
use base64;
use ts3_bot::audio::{
    SpeakerBufferManager,
    TranscriptionPipeline,
};
use ts3_bot::audio::whisper_api::WhisperApiTranscriber;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde_json;

// SyncConnection for bidirectional communication
use tsclientlib::prelude::*;
use tsclientlib::sync::SyncConnection;
use ts_bookkeeping::{MessageTarget, DisconnectOptions, Reason};

/// Outgoing TS3 chat message with target (channel or private)
struct OutgoingMessage {
    text: String,
    target: MessageTarget,
}

impl OutgoingMessage {
    fn channel(text: String) -> Self {
        Self { text, target: MessageTarget::Channel }
    }
    fn private(text: String, client_id: u16) -> Self {
        Self { text, target: MessageTarget::Client(tsclientlib::ClientId(client_id)) }
    }
    /// Reply to the same context as the incoming message
    fn reply(text: String, incoming_target: &MessageTarget, sender_id: u16) -> Self {
        match incoming_target {
            MessageTarget::Client(_) | MessageTarget::Poke(_) => Self::private(text, sender_id),
            _ => Self::channel(text),
        }
    }
}

/// Results from background whisper tasks
enum WhisperResult {
    Transcription {
        speaker_id: u64,
        speaker_name: String,
        speaker_uid: String,
        text: String,
        command: Option<String>,
        audio_len: usize,
        detected_language: Option<String>,
    },
}

/// Persist language preferences to disk
fn save_language_prefs(overrides: &HashMap<String, String>) -> Result<()> {
    let _ = std::fs::create_dir_all("data");
    let json = serde_json::to_string_pretty(overrides)?;
    std::fs::write("data/language_prefs.json", json)?;
    info!("Saved {} language preference(s)", overrides.len());
    Ok(())
}

/// Update bot nickname to reflect listening state (e.g. "Marlbot 🎤" when listening)
async fn update_bot_nickname(
    sender: &mut tsclientlib::sync::SyncConnectionHandle,
    is_listening: bool,
) {
    use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
    let nickname = if is_listening { "Marlbot \u{1F3A4}" } else { "Marlbot" };
    let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "clientupdate");
    cmd.write_arg("client_nickname", &nickname);
    match sender.send_command(cmd).await {
        Ok(()) => debug!("Nickname updated to '{}'", nickname),
        Err(e) => debug!("Nickname update failed: {:?}", e),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration first (before logging so we can use LOG_LEVEL)
    let config = BotConfig::from_env()?;

    // Initialize logging with configured level
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    // Suppress verbose whisper.cpp internal logs
                    tracing_subscriber::EnvFilter::new(
                        format!("{},whisper_rs=warn,tsproto=warn,tsclientlib=warn", config.log_level.to_lowercase())
                    )
                })
        )
        .init();

    info!(?config, "TS3 Bot starting with configuration");

    let start_time = std::time::Instant::now();

    // Create broadcast channel for TS3 events -> WebSocket clients
    // Capacity of 100 messages in buffer
    let (event_tx, _event_rx) = broadcast::channel::<WebSocketEvent>(100);
    let event_tx_clone = event_tx.clone();

    // Create TS3 client
    let ts3_client = TS3Client::new(config.clone());

    // Shutdown signal for graceful TS3 disconnect
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Channel for TTS requests from WebSocket → TTS processing task
    let (tts_tx, mut tts_rx) = tokio::sync::mpsc::channel::<TtsRequest>(10);

    // Shared TS3 connection handle (populated once TS3 connects)
    let shared_ts3_handle: websocket::SharedTs3Handle =
        Arc::new(tokio::sync::Mutex::new(None));
    let shared_ts3_handle_for_ws = shared_ts3_handle.clone();

    // Clone config for WebSocket server BEFORE TS3 spawn consumes it
    let ws_config = config.clone();
    let tts_enabled = config.tts_enabled;

    // Shared TTS stop flag — allows WS server to interrupt playback
    let tts_stop_flag: Option<Arc<std::sync::atomic::AtomicBool>> = if tts_enabled {
        Some(Arc::new(std::sync::atomic::AtomicBool::new(false)))
    } else {
        None
    };
    let tts_stop_flag_for_ts3 = tts_stop_flag.clone();

    // Shared buffer manager — accessible from both TS3 and WS tasks
    let buffer_manager = Arc::new(Mutex::new(SpeakerBufferManager::new()));
    let buffer_manager_for_ws = Some(buffer_manager.clone());

    // Per-user language overrides for Whisper transcription (UID -> ISO 639-1 code)
    // Persisted to data/language_prefs.json across restarts
    let language_overrides: Arc<Mutex<HashMap<String, String>>> = {
        let prefs_path = "data/language_prefs.json";
        let map = if let Ok(data) = std::fs::read_to_string(prefs_path) {
            match serde_json::from_str::<HashMap<String, String>>(&data) {
                Ok(m) => {
                    info!("Loaded {} language preference(s) from {}", m.len(), prefs_path);
                    m
                }
                Err(e) => {
                    warn!("Failed to parse {}: {}, starting fresh", prefs_path, e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        Arc::new(Mutex::new(map))
    };
    let language_overrides_for_ws = Some(language_overrides.clone());

    // Load persisted bot state (mute + volume)
    let (persisted_muted, persisted_volume, persisted_voice) = {
        std::fs::read_to_string("data/bot_state.json")
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| {
                let muted = v.get("muted").and_then(|m| m.as_bool()).unwrap_or(false);
                let vol = v.get("volume").and_then(|v| v.as_u64()).unwrap_or(100) as u8;
                let voice = v.get("voice").and_then(|v| v.as_str()).map(|s| s.to_string());
                (muted, vol, voice)
            })
            .unwrap_or((false, 100, None))
    };
    if persisted_muted || persisted_volume != 100 || persisted_voice.is_some() {
        info!("Restored bot state: muted={}, volume={}%, voice={}", persisted_muted, persisted_volume, persisted_voice.as_deref().unwrap_or("config default"));
    }

    // Shared TTS mute flag (when true, agent TTS is skipped but text echo still sent)
    let tts_muted = Arc::new(std::sync::atomic::AtomicBool::new(persisted_muted));
    let tts_muted_for_tts = tts_muted.clone();

    // Shared TTS volume (0-200, default 100%)
    let tts_volume = Arc::new(std::sync::atomic::AtomicU8::new(persisted_volume));
    let tts_volume_for_ws = Some(tts_volume.clone());
    let tts_volume_for_ts3 = Some(tts_volume.clone());

    // Shared default TTS voice (overridable at runtime via !voice, persisted)
    let default_voice: Arc<std::sync::RwLock<String>> = Arc::new(std::sync::RwLock::new(
        persisted_voice.unwrap_or_else(|| config.tts_voice.clone())
    ));
    let default_voice_for_tts = default_voice.clone();
    let default_voice_for_ws = Some(default_voice.clone());

    // Last spoken text for !replay (text, voice, speed)
    let last_spoken: Arc<Mutex<Option<(String, Option<String>, Option<f32>)>>> = Arc::new(Mutex::new(None));
    let last_spoken_for_ws = last_spoken.clone();
    let last_spoken_for_ts3 = last_spoken.clone();

    // Greeting feature: greet users who join the bot's channel
    let greet_enabled = Arc::new(std::sync::atomic::AtomicBool::new({
        std::fs::read_to_string("data/greet.json")
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
            .unwrap_or(true) // enabled by default
    }));
    // Cooldown: don't greet the same user within 10 minutes (keyed by client_id)
    let greet_cooldowns: Arc<Mutex<HashMap<u64, std::time::Instant>>> = Arc::new(Mutex::new(HashMap::new()));

    // Chat history ring buffer (last N messages for !history)
    let chat_history: Arc<Mutex<std::collections::VecDeque<(String, String, String)>>> =
        Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(50)));
    // Each entry: (timestamp, author, text) — max 50 entries
    let chat_history_for_ws = Some(chat_history.clone());

    // TTS rate limiter: max 5 uses per user per 60 seconds (keyed by client_id)
    let tts_rate_limits: Arc<Mutex<HashMap<u64, Vec<std::time::Instant>>>> = Arc::new(Mutex::new(HashMap::new()));

    // Last-seen tracker: UID -> (name, ISO timestamp) — persisted to data/seen.json
    let seen_data: Arc<Mutex<HashMap<String, (String, String)>>> = {
        let map = std::fs::read_to_string("data/seen.json")
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, (String, String)>>(&s).ok())
            .unwrap_or_default();
        if !map.is_empty() {
            info!("Loaded {} seen record(s)", map.len());
        }
        Arc::new(Mutex::new(map))
    };

    // Spawn TS3 client connection task
    let mut ts3_handle = tokio::spawn(async move {
        info!("Starting TS3 client connection");

        // Initialize audio processing components
        info!("Initializing audio processing components");

        // Initialize Whisper API transcriber (uses same key as TTS)
        let whisper_api: Option<Arc<WhisperApiTranscriber>> = config.tts_api_key.as_ref()
            .filter(|k| !k.is_empty())
            .map(|key| {
                info!("WhisperAPI transcriber initialized (using TTS_API_KEY)");
                Arc::new(WhisperApiTranscriber::new(key.clone()))
            });

        // Only load local Whisper model as fallback if API is not available
        let transcription_pipeline = if whisper_api.is_some() {
            info!("Whisper API available — skipping local ggml-small.bin model (~500MB RAM saved)");
            None
        } else {
            match TranscriptionPipeline::new("models/ggml-small.bin") {
                Ok(p) => {
                    info!("TranscriptionPipeline (small) initialized as fallback (no API key)");
                    Some(Arc::new(Mutex::new(p)))
                }
                Err(e) => {
                    warn!("Failed to initialize TranscriptionPipeline: {}. Transcription disabled.", e);
                    None
                }
            }
        };

        // Attempt connection with retry loop (uses reconnect_* config)
        let max_attempts = config.reconnect_max_attempts;
        let initial_delay_ms = config.reconnect_initial_delay_ms;
        let max_delay_ms = config.reconnect_max_delay_ms;

        let mut connection_opt = None;
        for attempt in 1..=max_attempts {
            match ts3_client.connect().await {
                Ok(c) => {
                    connection_opt = Some(c);
                    break;
                }
                Err(e) => {
                    if attempt == max_attempts {
                        error!("Failed to connect to TS3 server after {} attempts: {}", max_attempts, e);
                    } else {
                        let delay_ms = (initial_delay_ms * 2u64.saturating_pow(attempt - 1)).min(max_delay_ms);
                        warn!("TS3 connection attempt {}/{} failed: {}. Retrying in {}ms...", attempt, max_attempts, e, delay_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        if let Some(connection) = connection_opt {
                info!("TS3 client connected successfully");

                // Convert to SyncConnection for bidirectional communication
                let sync_con = SyncConnection::from(connection);
                let ts3_sender = sync_con.get_handle();

                // Store the handle so WebSocket server can query TS3 state
                {
                    let mut handle = shared_ts3_handle.lock().await;
                    *handle = Some(ts3_sender.clone());
                }

                // Restore last channel from .last_channel (if no TS3_CHANNEL config)
                // Spawned as a task because with_connection/send_command need
                // sync_con to be polled (in the select! loop below).
                let has_config_channel = config.ts3_channel.as_ref()
                    .map_or(false, |c| !c.is_empty());
                if !has_config_channel {
                    if let Ok(content) = std::fs::read_to_string(".last_channel") {
                        if let Ok(channel_id) = content.trim().parse::<u64>() {
                            info!("Will restore last channel (id: {})", channel_id);
                            let mut restore_sender = ts3_sender.clone();
                            tokio::spawn(async move {
                                // Small delay to let the event loop start polling
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                let move_result = restore_sender.with_connection(move |con| {
                                    con.get_state().ok().map(|s| s.own_client.0)
                                }).await;
                                if let Ok(Some(own_id)) = move_result {
                                    use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                                    let mut cmd = OutCommand::new(
                                        Direction::C2S, Flags::empty(),
                                        PacketType::Command, "clientmove",
                                    );
                                    cmd.write_arg("clid", &own_id);
                                    cmd.write_arg("cid", &channel_id);
                                    match restore_sender.send_command(cmd).await {
                                        Ok(()) => info!("Restored to channel {}", channel_id),
                                        Err(e) => warn!("Failed to restore channel {}: {:?}", channel_id, e),
                                    }
                                }
                            });
                        }
                    }
                }

                // Rebind as mutable for use in select! loop
                let mut shutdown_rx = shutdown_rx;

                info!("Starting event loop to keep connection alive");

                // Resolve own client ID for self-message filtering
                let own_client_id: Arc<std::sync::RwLock<Option<u16>>> =
                    Arc::new(std::sync::RwLock::new(None));
                {
                    let own_id_ref = own_client_id.clone();
                    let mut own_id_sender = ts3_sender.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if let Ok(Some(id)) = own_id_sender.with_connection(move |con| {
                            con.get_state().ok().map(|s| s.own_client.0)
                        }).await {
                            if let Ok(mut w) = own_id_ref.write() {
                                *w = Some(id);
                            }
                            info!("Own client ID resolved: {}", id);
                        }
                    });
                }

                // Subscribe to ALL channels so we can see all clients on the server
                // (by default, tsclientlib only sees clients in the bot's current channel)
                {
                    let mut sub_sender = ts3_sender.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                        let cmd = OutCommand::new(
                            Direction::C2S, Flags::empty(),
                            PacketType::Command, "channelsubscribeall",
                        );
                        match sub_sender.send_command(cmd).await {
                            Ok(()) => info!("Subscribed to all channels (full client visibility)"),
                            Err(e) => warn!("Failed to subscribe to all channels: {:?}", e),
                        }
                    });
                }

                // Fix "Marlbot1" clone nickname: after restart, the old connection
                // takes ~30-60s to timeout on the TS3 server. During that time, our new
                // connection gets suffixed with "1". We retry until we can claim our name.
                {
                    let mut nick_sender = ts3_sender.clone();
                    let configured_name = config.ts3_nickname.clone();
                    tokio::spawn(async move {
                        use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                        // Start trying after 30s, retry every 10s up to 2 minutes
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        for attempt in 1..=6 {
                            let mut cmd = OutCommand::new(
                                Direction::C2S, Flags::empty(),
                                PacketType::Command, "clientupdate",
                            );
                            cmd.write_arg("client_nickname", &configured_name);
                            match nick_sender.send_command(cmd).await {
                                Ok(()) => {
                                    info!("Nickname corrected to '{}' (attempt {})", configured_name, attempt);
                                    return;
                                }
                                Err(e) => {
                                    let err_str = format!("{:?}", e);
                                    // ClientNicknameInuse when setting our own name = already correct
                                    // (TS3 auto-renames us when the old clone disconnects)
                                    if err_str.contains("ClientNicknameInuse") {
                                        info!("Nickname already '{}' — no correction needed", configured_name);
                                        return;
                                    }
                                    if attempt < 6 {
                                        debug!("Nickname correction attempt {} failed: {:?}, retrying in 10s", attempt, e);
                                        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                    } else {
                                        warn!("Nickname correction failed after {} attempts: {:?}", attempt, e);
                                    }
                                }
                            }
                        }
                    });
                }

                // NOTE: clientupdate unmute removed — was corrupting event stream

                // Channel for queuing outgoing TS3 chat messages
                let (ts3_msg_tx, mut ts3_msg_rx) = tokio::sync::mpsc::channel::<OutgoingMessage>(10);

                // Channel for receiving whisper results from background tasks
                let (whisper_tx, mut whisper_rx) = tokio::sync::mpsc::channel::<WhisperResult>(10);

                // Shared cache: speaker_id -> (name, uid) resolved from TS3 connection state
                let client_names: Arc<std::sync::RwLock<HashMap<u64, (String, String)>>> =
                    Arc::new(std::sync::RwLock::new(HashMap::new()));

                // Spawn task to send TS3 messages via the SyncConnection handle
                let mut sender_clone = ts3_sender.clone();
                tokio::spawn(async move {
                    while let Some(outgoing) = ts3_msg_rx.recv().await {
                        let text = outgoing.text.clone();
                        let target = outgoing.target;
                        match sender_clone.with_connection(move |con| {
                            if let Ok(state) = con.get_state() {
                                let _ = state.send_message(
                                    target,
                                    &text
                                ).send(con);
                            }
                        }).await {
                            Ok(_) => info!("TS3 chat ({}): {}", match outgoing.target { MessageTarget::Channel => "channel", MessageTarget::Server => "server", _ => "private" }, outgoing.text),
                            Err(e) => warn!("Failed to send TS3 message: {:?}", e),
                        }
                    }
                });

                // Spawn task to periodically refresh client names from TS3 connection state
                let names_for_refresh = client_names.clone();
                let mut refresh_sender = ts3_sender.clone();
                tokio::spawn(async move {
                    // Small delay to let the connection fully establish
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
                    loop {
                        let names_ref = names_for_refresh.clone();
                        match refresh_sender.with_connection(move |con| {
                            if let Ok(state) = con.get_state() {
                                let mut new_names = HashMap::new();
                                for (id, client) in &state.clients {
                                    let uid = client.uid.as_ref()
                                        .map(|u| base64::encode(&u.0))
                                        .unwrap_or_else(|| "unknown".to_string());
                                    new_names.insert(id.0 as u64, (client.name.clone(), uid));
                                }
                                if let Ok(mut cache) = names_ref.write() {
                                    *cache = new_names;
                                }
                            }
                        }).await {
                            Ok(_) => debug!("Refreshed TS3 client name cache"),
                            Err(e) => debug!("Failed to refresh client names: {:?}", e),
                        }
                        interval.tick().await;
                    }
                });

                // Initialize TTS audio player (if TTS enabled)
                // AudioPlayer needs ts3_sender to send audio packets to TS3
                let audio_player: Option<Arc<AudioPlayer>> = if config.tts_enabled {
                    info!("Initializing TTS audio player");
                    Some(Arc::new(AudioPlayer::with_options(
                        ts3_sender.clone(),
                        event_tx_clone.clone(),
                        tts_stop_flag_for_ts3.clone(),
                        tts_volume_for_ts3.clone(),
                    )))
                } else {
                    info!("TTS is disabled");
                    None
                };

                // Initialize TTS synthesizer (HTTP-based, calls external TTS service)
                let tts_synth: Option<Arc<dyn TtsSynthesizer>> = if config.tts_enabled {
                    let synth = HttpTtsSynthesizer::new(
                        &config.tts_api_url,
                        &config.tts_model,
                        &config.tts_voice,
                        config.tts_api_key.clone(),
                    );
                    info!("HttpTtsSynthesizer initialized (url: {}, voice: {})", config.tts_api_url, config.tts_voice);
                    Some(Arc::new(synth))
                } else {
                    None
                };

                // Pre-generate cached wake word confirmation audio
                let wake_confirmation_frames: Option<Arc<Vec<Vec<u8>>>> = if let Some(ref synth) = tts_synth {
                    let synth_clone = synth.clone();
                    match tokio::task::spawn_blocking(move || -> Result<Vec<Vec<u8>>> {
                        use ts3_bot::audio::decoder::OpusDecoder;
                        use ts3_bot::audio::encoder::OpusEncoder;
                        let tts_audio = synth_clone.synthesize("Oui ?", None, None)?;
                        let samples_48k = match tts_audio.sample_rate {
                            48000 => tts_audio.samples,
                            24000 => OpusDecoder::resample_24k_to_48k(&tts_audio.samples),
                            other => anyhow::bail!("Unsupported sample rate: {}", other),
                        };
                        let mut encoder = OpusEncoder::new()?;
                        encoder.encode_all(&samples_48k)
                    }).await {
                        Ok(Ok(frames)) => {
                            info!("Cached wake word confirmation audio: {} frames ({:.1}s)", frames.len(), frames.len() as f32 * 0.02);
                            Some(Arc::new(frames))
                        }
                        Ok(Err(e)) => {
                            warn!("Failed to pre-generate wake confirmation audio: {}. Skipping voice confirmation.", e);
                            None
                        }
                        Err(e) => {
                            warn!("Wake confirmation task panicked: {}. Skipping voice confirmation.", e);
                            None
                        }
                    }
                } else {
                    None
                };

                // Spawn TTS processing task (receives requests from WebSocket)
                if let (Some(ref player), Some(ref synth)) = (&audio_player, &tts_synth) {
                    let player_clone = player.clone();
                    let synth_clone = synth.clone();
                    let tts_chat_tx = ts3_msg_tx.clone();
                    let tts_event_tx = event_tx_clone.clone();
                    let last_spoken_ws = last_spoken_for_ws.clone();
                    let tts_muted_clone = tts_muted_for_tts.clone();
                    let tts_chat_history = chat_history.clone();
                    tokio::spawn(async move {
                        while let Some(request) = tts_rx.recv().await {
                            info!("TTS request: '{}'", request.text);

                            // Clone all state needed by the sub-task
                            let last_spoken_sub = last_spoken_ws.clone();
                            let tts_chat_tx_sub = tts_chat_tx.clone();
                            let tts_chat_history_sub = tts_chat_history.clone();
                            let tts_muted_sub = tts_muted_clone.clone();
                            let tts_event_tx_sub = tts_event_tx.clone();
                            let default_voice_sub = default_voice_for_tts.clone();
                            let player_sub = player_clone.clone();
                            let synth_sub = synth_clone.clone();

                            // Spawn each TTS request as a sub-task for panic isolation.
                            // If the sub-task panics, the JoinHandle returns Err instead of
                            // killing this receiver loop (which would close the channel and
                            // permanently break ALL TTS — see UTF-8 panic incident 2026-02-10).
                            let handle = tokio::spawn(async move {
                                // Save for !replay
                                {
                                    let mut ls = last_spoken_sub.lock().await;
                                    *ls = Some((request.text.clone(), request.voice.clone(), request.speed));
                                }
                                // Echo TTS text to TS3 channel chat so muted users can read it
                                let display_text = if request.text.len() > 300 {
                                    format!("🤖 {}...", truncate_str(&request.text, 300))
                                } else {
                                    format!("🤖 {}", request.text)
                                };
                                let _ = tts_chat_tx_sub.try_send(OutgoingMessage::channel(display_text));

                                // Record bot response to chat history
                                {
                                    let mut hist = tts_chat_history_sub.lock().await;
                                    let ts = chrono::Utc::now().format("%H:%M").to_string();
                                    let truncated = if request.text.len() > 200 {
                                        format!("{}...", truncate_str(&request.text, 200))
                                    } else {
                                        request.text.clone()
                                    };
                                    hist.push_back((ts, "🤖 Marlbot".to_string(), truncated));
                                    if hist.len() > 50 { hist.pop_front(); }
                                }

                                // If TTS is muted, skip speech but still emit events
                                if tts_muted_sub.load(std::sync::atomic::Ordering::Relaxed) {
                                    info!("TTS muted — skipping speech for: '{}'", truncate_str(&request.text, 50));
                                    let _ = tts_event_tx_sub.send(WebSocketEvent::speak_completed(request.text, 0));
                                    return;
                                }

                                // Emit speak_started event
                                let _ = tts_event_tx_sub.send(WebSocketEvent::speak_started(request.text.clone()));
                                let start = std::time::Instant::now();
                                // Use default voice override if no explicit voice in request
                                let effective_voice = request.voice.or_else(|| {
                                    Some(default_voice_sub.read().unwrap().clone())
                                });
                                let speak_result = player_sub
                                    .speak(request.text.clone(), effective_voice, request.speed, synth_sub.clone())
                                    .await;
                                let duration_ms = start.elapsed().as_millis() as u64;
                                match speak_result {
                                    Ok(_) => {
                                        let _ = tts_event_tx_sub.send(WebSocketEvent::speak_completed(request.text, duration_ms));
                                    }
                                    Err(e) => {
                                        warn!("TTS speak failed: {}", e);
                                        let _ = tts_event_tx_sub.send(WebSocketEvent::speak_failed(request.text, duration_ms, format!("{}", e)));
                                    }
                                }
                            });

                            // Await the sub-task — if it panicked, log and continue
                            // (the receiver loop stays alive for the next request)
                            match handle.await {
                                Ok(()) => {},
                                Err(e) => {
                                    error!("TTS sub-task panicked: {:?} — receiver loop continues", e);
                                }
                            }
                        }
                    });
                }

                // Process events from the SyncConnection stream
                tokio::pin!(sync_con);
                let mut silence_check_interval = tokio::time::interval(std::time::Duration::from_millis(500));
                let mut buffer_cleanup_interval = tokio::time::interval(std::time::Duration::from_secs(60));
                let mut shutting_down = false;

                loop {
                    tokio::select! {
                        // Process results from background whisper tasks
                        Some(result) = whisper_rx.recv() => {
                            match result {
                                WhisperResult::Transcription { speaker_id, speaker_name, speaker_uid, text, command, audio_len, detected_language } => {
                                    // command field is unused in new architecture (no wake word stripping needed)
                                    let _ = command;
                                    let command_text = text.clone();

                                    if !command_text.trim().is_empty() {
                                        info!("Transcription from {} [{}]: '{}'", speaker_name, detected_language.as_deref().unwrap_or("?"), command_text);

                                        // Record transcription to chat history
                                        {
                                            let mut hist = chat_history.lock().await;
                                            let ts = chrono::Utc::now().format("%H:%M").to_string();
                                            hist.push_back((ts, format!("🎤{}", speaker_name), command_text.clone()));
                                            if hist.len() > 50 { hist.pop_front(); }
                                        }

                                        // Send transcription to TS3 chat
                                        let _ = ts3_msg_tx.try_send(
                                            OutgoingMessage::channel(format!("{}: {}", speaker_name, command_text))
                                        );

                                        let transcription_event = TranscriptionEvent {
                                            timestamp: chrono::Utc::now(),
                                            speaker_id,
                                            speaker_uid,
                                            speaker_name,
                                            text: command_text,
                                            confidence: None,
                                            language: detected_language,
                                            duration_ms: (audio_len as u64 * 1000) / 16000,
                                        };

                                        let ws_event = WebSocketEvent::transcription(transcription_event);
                                        if let Err(e) = event_tx_clone.send(ws_event) {
                                            warn!("Failed to broadcast transcription: {}", e);
                                        }
                                    }

                                    // Reset wake check timer for re-detection
                                    let mut bm = buffer_manager.lock().await;
                                    if let Some(buf) = bm.get_buffer_mut(speaker_id) {
                                        buf.mark_wake_check();
                                    }
                                }
                            }
                        }

                        // Periodic silence check for active speakers
                        // NOT gated by whisper_busy: silence detection must work even
                        // when Whisper is busy checking wake words for other speakers.
                        // The pipeline mutex (blocking_lock) handles serialization.
                        _ = silence_check_interval.tick() => {
                            if transcription_pipeline.is_some() || whisper_api.is_some() {
                                let tp_arc = transcription_pipeline.clone();
                                let mut bm = buffer_manager.lock().await;
                                let timed_out = bm.check_silence_timeouts();

                                for speaker_id in timed_out {
                                    if let Some(buffer) = bm.get_buffer_mut(speaker_id) {
                                        info!("Silence timeout for speaker {} - transcribing", speaker_id);
                                        let full_audio = buffer.get_samples();
                                        let speaker_name = buffer.speaker_name.clone();
                                        let speaker_uid = buffer.speaker_uid.clone();
                                        buffer.clear();
                                        drop(bm);

                                        // Send "Arrêt de l'écoute" message in TS3 chat
                                        let _ = ts3_msg_tx.try_send(
                                            OutgoingMessage::channel(format!("Arret de l'ecoute, {}.", speaker_name))
                                        );

                                        // Update nickname if no more active listeners
                                        {
                                            let bm_check = buffer_manager.lock().await;
                                            let still_listening = !bm_check.get_active_speakers().is_empty();
                                            drop(bm_check);
                                            if !still_listening {
                                                let mut nick_sender = ts3_sender.clone();
                                                tokio::spawn(async move { update_bot_nickname(&mut nick_sender, false).await; });
                                            }
                                        }

                                        if !full_audio.is_empty() {
                                            // RMS energy gate: skip transcription if audio is mostly silence
                                            let rms_energy = {
                                                let sum_sq: f32 = full_audio.iter().map(|s| s * s).sum();
                                                (sum_sq / full_audio.len() as f32).sqrt()
                                            };

                                            if rms_energy < 0.005 {
                                                info!("Skipping transcription for {} — audio too quiet (RMS: {:.4})", speaker_name, rms_energy);
                                                bm = buffer_manager.lock().await;
                                                if let Some(buf) = bm.get_buffer_mut(speaker_id) {
                                                    buf.mark_wake_check();
                                                }
                                                break;
                                            }

                                            let tp_clone = tp_arc.clone();
                                            let whisper_tx_clone = whisper_tx.clone();
                                            let api_clone = whisper_api.clone();

                                            // Resolve per-user language override (by UID) before spawn_blocking
                                            let lang_override = {
                                                let overrides = language_overrides.lock().await;
                                                overrides.get(&speaker_uid).cloned()
                                            };

                                            tokio::task::spawn_blocking(move || {
                                                let audio_len = full_audio.len();

                                                // Try API transcription (with language override if set), fall back to local
                                                let (text_result, detected_lang) = if let Some(ref api) = api_clone {
                                                    match api.transcribe(&full_audio, lang_override.as_deref()) {
                                                        Ok(tr) if !tr.text.is_empty() => {
                                                            info!("WhisperAPI transcription succeeded (lang: {})", tr.language.as_deref().unwrap_or("?"));
                                                            (Ok(tr.text), tr.language)
                                                        }
                                                        Ok(_) => {
                                                            warn!("WhisperAPI returned empty");
                                                            if let Some(ref tp) = tp_clone {
                                                                warn!("Falling back to local Whisper");
                                                                let mut lock = tp.blocking_lock();
                                                                let r = lock.transcribe(&full_audio);
                                                                drop(lock);
                                                                (r, None)
                                                            } else {
                                                                (Ok(String::new()), None)
                                                            }
                                                        }
                                                        Err(e) => {
                                                            warn!("WhisperAPI failed: {}", e);
                                                            if let Some(ref tp) = tp_clone {
                                                                warn!("Falling back to local Whisper");
                                                                let mut lock = tp.blocking_lock();
                                                                let r = lock.transcribe(&full_audio);
                                                                drop(lock);
                                                                (r, None)
                                                            } else {
                                                                (Err(anyhow::anyhow!("WhisperAPI failed and no local model: {}", e)), None)
                                                            }
                                                        }
                                                    }
                                                } else if let Some(ref tp) = tp_clone {
                                                    let mut lock = tp.blocking_lock();
                                                    let r = lock.transcribe(&full_audio);
                                                    drop(lock);
                                                    (r, None)
                                                } else {
                                                    (Err(anyhow::anyhow!("No transcription backend available")), None)
                                                };

                                                // Filter Whisper hallucinations before forwarding
                                                fn is_hallucination(text: &str) -> bool {
                                                    let t = text.trim().to_lowercase();
                                                    if t.is_empty() { return true; }
                                                    let patterns = [
                                                        "[musique]", "[music]", "[applaudissements]", "[rires]",
                                                        "[silence]", "[bruit]", "[bruits]", "[applause]", "[laughter]",
                                                        "merci d'avoir regardé", "merci d'avoir écouté",
                                                        "sous-titres", "sous-titrage", "merci à tous",
                                                        "à bientôt", "à la prochaine",
                                                        "thank you for watching", "thanks for watching",
                                                        "subscribe", "like and subscribe",
                                                        "...", "you", "bye.",
                                                    ];
                                                    patterns.iter().any(|p| t.contains(p))
                                                }

                                                match text_result {
                                                    Ok(text) if !is_hallucination(&text) => {
                                                        let _ = whisper_tx_clone.blocking_send(WhisperResult::Transcription {
                                                            speaker_id,
                                                            speaker_name,
                                                            speaker_uid,
                                                            text,
                                                            command: None,
                                                            audio_len,
                                                            detected_language: detected_lang,
                                                        });
                                                    }
                                                    Ok(text) => {
                                                        info!("Filtered Whisper hallucination: '{}'", text.trim());
                                                        // Don't forward — send empty so pipeline resets cleanly
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("Transcription failed: {}", e);
                                                        let _ = whisper_tx_clone.blocking_send(WhisperResult::Transcription {
                                                            speaker_id,
                                                            speaker_name,
                                                            speaker_uid,
                                                            text: String::new(),
                                                            command: None,
                                                            audio_len,
                                                            detected_language: None,
                                                        });
                                                    }
                                                }
                                            });
                                        } else {
                                            // Reset wake check timer even if no audio
                                            bm = buffer_manager.lock().await;
                                            if let Some(buf) = bm.get_buffer_mut(speaker_id) {
                                                buf.mark_wake_check();
                                            }
                                        }

                                        break; // Only process one timeout per tick
                                    }
                                }
                            }
                        }

                        // Periodic cleanup of old inactive speaker buffers
                        _ = buffer_cleanup_interval.tick() => {
                            let mut bm = buffer_manager.lock().await;
                            bm.cleanup_old_buffers(std::time::Duration::from_secs(300));
                        }

                        // Process TS3 events
                        event = sync_con.next() => {
                    let event = match event {
                        Some(e) => e,
                        None => {
                            if shutting_down {
                                info!("TS3 stream closed after disconnect");
                            }
                            break;
                        }
                    };
                    match event {
                        Ok(item) => {
                            use tsclientlib::sync::SyncStreamItem;
                            match item {
                                SyncStreamItem::BookEvents(book_events) => {
                                    use ts_bookkeeping::events::{Event, PropertyId, PropertyValue};
                                    for event in book_events {
                                        match event {
                                            Event::Message { target, invoker, message } => {
                                                info!("TS3 Message from {}: {}", invoker.name, message);

                                                // Ignore our own messages to prevent infinite loops
                                                // Check 1: by client ID (if resolved)
                                                if let Ok(guard) = own_client_id.read() {
                                                    if *guard == Some(invoker.id.0) {
                                                        debug!("Ignoring own message (by client ID)");
                                                        continue;
                                                    }
                                                }
                                                // Check 2: by UID (bot's UID is always the same)
                                                if let Some(ref uid) = invoker.uid {
                                                    let uid_b64 = base64::encode(&uid.0);
                                                    if uid_b64 == "NlViljH4cvfmHfMg6CUT4PGuqEM=" {
                                                        debug!("Ignoring own message (by UID)");
                                                        continue;
                                                    }
                                                }

                                                // Record to chat history (skip bot commands)
                                                if !message.starts_with('!') {
                                                    let mut hist = chat_history.lock().await;
                                                    let ts = chrono::Utc::now().format("%H:%M").to_string();
                                                    hist.push_back((ts, invoker.name.to_string(), message.to_string()));
                                                    if hist.len() > 50 { hist.pop_front(); }
                                                }

                                                let (message_type, channel_id) = match target {
                                                    MessageTarget::Channel => (MessageType::Channel, None),
                                                    MessageTarget::Server => (MessageType::Channel, None),
                                                    MessageTarget::Client(_) | MessageTarget::Poke(_) => (MessageType::Private, None),
                                                };

                                                let sender_uid = invoker.uid
                                                    .as_ref()
                                                    .map(|uid| base64::encode(&uid.0))
                                                    .unwrap_or_else(|| "unknown".to_string());

                                                let msg_event = MessageEvent {
                                                    message_type,
                                                    sender_id: invoker.id.0 as u64,
                                                    sender_uid: sender_uid.clone(),
                                                    sender_name: invoker.name.to_string(),
                                                    content: message.to_string(),
                                                    channel_id,
                                                    channel_name: None,
                                                    timestamp: chrono::Utc::now(),
                                                };

                                                // Only forward non-command messages to OpenClaw via WS
                                                // Bot commands (!listen, !stop, !help, !status) are handled locally
                                                if !message.starts_with('!') {
                                                    let ws_event = WebSocketEvent::message_received(msg_event);
                                                    if let Err(e) = event_tx_clone.send(ws_event) {
                                                        warn!("Failed to broadcast message event: {}", e);
                                                    }
                                                }

                                                // Chat trigger: !listen or !marlbot activates listening for the sender
                                                let msg_lower = message.to_lowercase();
                                                let reply_target = target;
                                                let reply_sender_id = invoker.id.0;
                                                if msg_lower.starts_with("!help") {
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        "📋 Commandes disponibles :\n\
                                                         • [b]!listen[/b] / [b]!marlbot[/b] — activer l'écoute vocale\n\
                                                         • [b]!stop[/b] — arrêter l'écoute + couper la parole\n\
                                                         • [b]!lang[/b] <code> — forcer la langue (fr, en, de...) ou [b]!lang auto[/b]\n\
                                                         • [b]!who[/b] — qui est dans ton channel ?\n\
                                                         • [b]!channels[/b] — lister tous les channels du serveur\n\
                                                         • [b]!tts[/b] [voice:X] [speed:X] <texte> — TTS (voix: alloy/echo/fable/nova/onyx/shimmer...)\n\
                                                         • [b]!move[/b] <channel> — déplacer le bot vers un channel\n\
                                                         • [b]!come[/b] / [b]!viens[/b] — le bot vient dans ton channel\n\
                                                         • [b]!replay[/b] — rejouer le dernier message TTS\n\
                                                         • [b]!voice[/b] [nom] — changer la voix par défaut (alloy/echo/nova/onyx...)\n\
                                                         • [b]!volume[/b] [0-200] — régler le volume TTS (100 = normal)\n\
                                                         • [b]!mute[/b] / [b]!unmute[/b] — couper/rétablir la voix (le bot écoute toujours)\n\
                                                         • [b]!greet[/b] [on|off] — activer/désactiver les salutations auto\n\
                                                         • [b]!timeout[/b] [ms] — régler le délai de silence (500-10000ms, défaut 2000)\n\
                                                         • [b]!roll[/b] [NdS+M] — lancer des dés (ex: 2d6, d20+3, 100)\n\
                                                         • [b]!quote[/b] [add|list|count|del] — livre de quotes mémorables\n\
                                                         • [b]!history[/b] [N] — derniers messages (défaut 10, max 50)\n\
                                                         • [b]!seen[/b] <nom> — quand un utilisateur a été vu pour la dernière fois\n\
                                                         • [b]!ping[/b] — latence vers le serveur TS3\n\
                                                         • [b]!status[/b] — afficher l'état du bot\n\
                                                         • [b]!help[/b] — afficher cette aide".to_string(),
                                                        &reply_target, reply_sender_id
                                                    ));
                                                } else if msg_lower.starts_with("!status") {
                                                    // Build status report (async to query TS3 state for channel info)
                                                    let bm = buffer_manager.lock().await;
                                                    let active_speakers = bm.get_active_speakers();
                                                    let active_names: Vec<String> = active_speakers.iter().filter_map(|id| {
                                                        bm.get_buffer(*id).map(|b| b.speaker_name.clone())
                                                    }).collect();
                                                    drop(bm);

                                                    let speaking = if let Some(ref player) = audio_player {
                                                        player.is_speaking()
                                                    } else { false };

                                                    let listen_str = if active_names.is_empty() {
                                                        "Personne".to_string()
                                                    } else {
                                                        active_names.join(", ")
                                                    };
                                                    let speak_str = if speaking { "Oui 🔊" } else { "Non" };

                                                    let uptime = start_time.elapsed();
                                                    let uptime_secs = uptime.as_secs();
                                                    let uptime_str = if uptime_secs < 60 {
                                                        format!("{}s", uptime_secs)
                                                    } else if uptime_secs < 3600 {
                                                        format!("{}m {}s", uptime_secs / 60, uptime_secs % 60)
                                                    } else if uptime_secs < 86400 {
                                                        format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60)
                                                    } else {
                                                        format!("{}j {}h {}m", uptime_secs / 86400, (uptime_secs % 86400) / 3600, (uptime_secs % 3600) / 60)
                                                    };

                                                    let vol = audio_player.as_ref().map(|p| p.volume()).unwrap_or(100);
                                                    let is_muted = tts_muted.load(std::sync::atomic::Ordering::Relaxed);
                                                    let tts_str = if !config.tts_enabled { "Désactivé ❌" } else if is_muted { "Muté 🔇" } else { "Activé ✅" };
                                                    let whisper_str = if whisper_api.is_some() { "API ✅" } else if transcription_pipeline.is_some() { "Local" } else { "Désactivé ❌" };
                                                    let greet_str = if greet_enabled.load(std::sync::atomic::Ordering::Relaxed) { "Activé 👋" } else { "Désactivé" };

                                                    // Query TS3 state for channel info
                                                    let mut sender_for_status = ts3_sender.clone();
                                                    let tx_status = ts3_msg_tx.clone();
                                                    let rt_status = reply_target;
                                                    let rs_status = reply_sender_id;
                                                    let bm_status = buffer_manager.clone();
                                                    let default_voice_status = default_voice.clone();
                                                    tokio::spawn(async move {
                                                        let channel_info = sender_for_status.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
                                                                let bot_client = state.clients.get(&state.own_client);
                                                                if let Some(bot) = bot_client {
                                                                    let ch_id = bot.channel.0 as u64;
                                                                    let ch_name = state.channels.get(&bot.channel)
                                                                        .map(|c| c.name.clone())
                                                                        .unwrap_or_else(|| format!("#{}", ch_id));
                                                                    // Count users in bot's channel
                                                                    let user_count = state.clients.values()
                                                                        .filter(|c| c.channel == bot.channel)
                                                                        .count();
                                                                    Some((ch_name, user_count))
                                                                } else { None }
                                                            } else { None }
                                                        }).await;

                                                        let channel_str = match channel_info {
                                                            Ok(Some((name, count))) => format!("{} ({} 👤)", name, count),
                                                            _ => "Inconnu".to_string(),
                                                        };

                                                        let voice_str = default_voice_status.read().unwrap().clone();
                                                        let silence_ms = {
                                                            let bm = bm_status.lock().await;
                                                            bm.silence_timeout_ms()
                                                        };
                                                        let _ = tx_status.try_send(OutgoingMessage::reply(format!(
                                                            "📊 [b]Status Marlbot[/b]\n\
                                                             • Channel : {}\n\
                                                             • Uptime : {}\n\
                                                             • Écoute : {}\n\
                                                             • Parle : {}\n\
                                                             • Volume : {}%\n\
                                                             • Voix : {}\n\
                                                             • TTS : {}\n\
                                                             • Whisper : {}\n\
                                                             • Greetings : {}\n\
                                                             • Silence timeout : {}ms",
                                                            channel_str,
                                                            uptime_str,
                                                            listen_str,
                                                            speak_str,
                                                            vol,
                                                            voice_str,
                                                            tts_str,
                                                            whisper_str,
                                                            greet_str,
                                                            silence_ms,
                                                        ), &rt_status, rs_status));
                                                    });
                                                } else if msg_lower.starts_with("!who") {
                                                    // Show who's in the same channel as the sender
                                                    let sender_id = invoker.id.0 as u64;
                                                    let sender_name_who = invoker.name.to_string();
                                                    let mut sender_for_who = ts3_sender.clone();
                                                    let tx_who = ts3_msg_tx.clone();
                                                    let rt_who = reply_target;
                                                    let rs_who = reply_sender_id;
                                                    tokio::spawn(async move {
                                                        let result = sender_for_who.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
                                                                // Find sender's channel (try ClientId first, then name fallback)
                                                                let sender_cid = tsclientlib::ClientId(sender_id as u16);
                                                                let channel_id = state.clients.get(&sender_cid)
                                                                    .map(|c| c.channel)
                                                                    .or_else(|| {
                                                                        state.clients.values()
                                                                            .find(|c| c.name == sender_name_who)
                                                                            .map(|c| c.channel)
                                                                    });
                                                                if let Some(ch_id) = channel_id {
                                                                    let ch_name = state.channels.get(&ch_id)
                                                                        .map(|c| c.name.clone())
                                                                        .unwrap_or_else(|| format!("Channel #{}", ch_id.0));
                                                                    let mut lines: Vec<String> = Vec::new();
                                                                    for c in state.clients.values() {
                                                                        if c.channel != ch_id { continue; }
                                                                        let mut flags = Vec::new();
                                                                        if c.input_muted { flags.push("🔇mic"); }
                                                                        if c.output_muted { flags.push("🔇son"); }
                                                                        if c.away_message.as_ref().map_or(false, |m| !m.is_empty()) {
                                                                            flags.push("💤away");
                                                                        }
                                                                        let flag_str = if flags.is_empty() {
                                                                            String::new()
                                                                        } else {
                                                                            format!(" ({})", flags.join(", "))
                                                                        };
                                                                        lines.push(format!("• {}{}", c.name, flag_str));
                                                                    }
                                                                    let count = lines.len();
                                                                    Some(format!(
                                                                        "👥 [b]{}[/b] — {} personne{}\n{}",
                                                                        ch_name,
                                                                        count,
                                                                        if count > 1 { "s" } else { "" },
                                                                        lines.join("\n")
                                                                    ))
                                                                } else {
                                                                    Some("❌ Impossible de trouver ton channel.".to_string())
                                                                }
                                                            } else {
                                                                Some("❌ État TS3 indisponible.".to_string())
                                                            }
                                                        }).await;
                                                        match result {
                                                            Ok(Some(msg)) => { let _ = tx_who.try_send(OutgoingMessage::reply(msg, &rt_who, rs_who)); }
                                                            Ok(None) => { let _ = tx_who.try_send(OutgoingMessage::reply("❌ Erreur interne.".to_string(), &rt_who, rs_who)); }
                                                            Err(e) => { let _ = tx_who.try_send(OutgoingMessage::reply(format!("❌ Erreur: {}", e), &rt_who, rs_who)); }
                                                        }
                                                    });
                                                } else if msg_lower.starts_with("!move") {
                                                    // Move the bot to a channel by name
                                                    let query = message.trim()[5..].trim().to_string();
                                                    if query.is_empty() {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Usage: !move <nom du channel>".to_string(), &reply_target, reply_sender_id));
                                                    } else {
                                                        let mut sender_for_move = ts3_sender.clone();
                                                        let tx_move = ts3_msg_tx.clone();
                                                        let rt_move = reply_target;
                                                        let rs_move = reply_sender_id;
                                                        let query_lower = query.to_lowercase();
                                                        tokio::spawn(async move {
                                                            let result = sender_for_move.with_connection(move |con| {
                                                                if let Ok(state) = con.get_state() {
                                                                    let own_id = state.own_client.0;
                                                                    // Find channel by case-insensitive partial match
                                                                    let mut best_match: Option<(u64, String)> = None;
                                                                    for (id, ch) in state.channels.iter() {
                                                                        let ch_name_lower = ch.name.to_lowercase();
                                                                        if ch_name_lower == query_lower {
                                                                            // Exact match — use immediately
                                                                            best_match = Some((id.0 as u64, ch.name.clone()));
                                                                            break;
                                                                        } else if ch_name_lower.contains(&query_lower) && best_match.is_none() {
                                                                            best_match = Some((id.0 as u64, ch.name.clone()));
                                                                        }
                                                                    }
                                                                    best_match.map(|(ch_id, ch_name)| (own_id, ch_id, ch_name))
                                                                } else {
                                                                    None
                                                                }
                                                            }).await;

                                                            match result {
                                                                Ok(Some((own_id, channel_id, channel_name))) => {
                                                                    use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                                                                    let mut cmd = OutCommand::new(
                                                                        Direction::C2S, Flags::empty(),
                                                                        PacketType::Command, "clientmove",
                                                                    );
                                                                    cmd.write_arg("clid", &own_id);
                                                                    cmd.write_arg("cid", &channel_id);
                                                                    match sender_for_move.send_command(cmd).await {
                                                                        Ok(()) => {
                                                                            // Save channel for restore on restart
                                                                            let _ = std::fs::write(".last_channel", channel_id.to_string());
                                                                            let _ = tx_move.try_send(OutgoingMessage::reply(format!("✅ Déplacé vers [b]{}[/b]", channel_name), &rt_move, rs_move));
                                                                        }
                                                                        Err(e) => {
                                                                            let _ = tx_move.try_send(OutgoingMessage::reply(format!("❌ Impossible de bouger: {:?}", e), &rt_move, rs_move));
                                                                        }
                                                                    }
                                                                }
                                                                Ok(None) => {
                                                                    let _ = tx_move.try_send(OutgoingMessage::reply(format!("❌ Aucun channel trouvé pour \"{}\"", query), &rt_move, rs_move));
                                                                }
                                                                Err(e) => {
                                                                    let _ = tx_move.try_send(OutgoingMessage::reply(format!("❌ Erreur: {}", e), &rt_move, rs_move));
                                                                }
                                                            }
                                                        });
                                                    }
                                                } else if msg_lower.starts_with("!come") || msg_lower.starts_with("!viens") {
                                                    // Move the bot to the sender's channel
                                                    let sender_id = invoker.id.0 as u64;
                                                    let sender_name_come = invoker.name.to_string();
                                                    let mut sender_for_come = ts3_sender.clone();
                                                    let tx_come = ts3_msg_tx.clone();
                                                    let rt_come = reply_target;
                                                    let rs_come = reply_sender_id;
                                                    tokio::spawn(async move {
                                                        let result = sender_for_come.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
                                                                let sender_cid = tsclientlib::ClientId(sender_id as u16);
                                                                let bot_channel = state.clients.get(&state.own_client).map(|c| c.channel);
                                                                // Try direct ClientId lookup first, then fallback to name search
                                                                let sender_channel = state.clients.get(&sender_cid)
                                                                    .map(|c| c.channel)
                                                                    .or_else(|| {
                                                                        // Fallback: search by name (handles reconnect with new ClientId)
                                                                        state.clients.values()
                                                                            .find(|c| c.name == sender_name_come)
                                                                            .map(|c| c.channel)
                                                                    });
                                                                match (bot_channel, sender_channel) {
                                                                    (Some(bot_ch), Some(sender_ch)) if bot_ch == sender_ch => {
                                                                        let ch_name = state.channels.get(&bot_ch).map(|c| c.name.clone()).unwrap_or_default();
                                                                        Some(Err(format!("Je suis déjà dans [b]{}[/b] 😏", ch_name)))
                                                                    }
                                                                    (_, Some(sender_ch)) => {
                                                                        let ch_name = state.channels.get(&sender_ch).map(|c| c.name.clone()).unwrap_or_default();
                                                                        let own_id = state.own_client.0;
                                                                        Some(Ok((own_id, sender_ch.0 as u64, ch_name)))
                                                                    }
                                                                    _ => Some(Err("❌ Impossible de trouver ton channel.".to_string()))
                                                                }
                                                            } else {
                                                                Some(Err("❌ État TS3 indisponible.".to_string()))
                                                            }
                                                        }).await;

                                                        match result {
                                                            Ok(Some(Ok((own_id, channel_id, channel_name)))) => {
                                                                use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                                                                let mut cmd = OutCommand::new(
                                                                    Direction::C2S, Flags::empty(),
                                                                    PacketType::Command, "clientmove",
                                                                );
                                                                cmd.write_arg("clid", &own_id);
                                                                cmd.write_arg("cid", &channel_id);
                                                                match sender_for_come.send_command(cmd).await {
                                                                    Ok(()) => {
                                                                        let _ = std::fs::write(".last_channel", channel_id.to_string());
                                                                        let _ = tx_come.try_send(OutgoingMessage::reply(format!("✅ J'arrive dans [b]{}[/b] !", channel_name), &rt_come, rs_come));
                                                                    }
                                                                    Err(e) => {
                                                                        let _ = tx_come.try_send(OutgoingMessage::reply(format!("❌ Impossible de bouger: {:?}", e), &rt_come, rs_come));
                                                                    }
                                                                }
                                                            }
                                                            Ok(Some(Err(msg))) => {
                                                                let _ = tx_come.try_send(OutgoingMessage::reply(msg, &rt_come, rs_come));
                                                            }
                                                            _ => {
                                                                let _ = tx_come.try_send(OutgoingMessage::reply("❌ Erreur interne.".to_string(), &rt_come, rs_come));
                                                            }
                                                        }
                                                    });
                                                } else if msg_lower.starts_with("!channels") {
                                                    // Show all server channels with user counts
                                                    let mut sender_for_ch = ts3_sender.clone();
                                                    let tx_ch = ts3_msg_tx.clone();
                                                    let rt_ch = reply_target;
                                                    let rs_ch = reply_sender_id;
                                                    tokio::spawn(async move {
                                                        let result = sender_for_ch.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
                                                                // Count clients per channel
                                                                let mut client_counts: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
                                                                for c in state.clients.values() {
                                                                    *client_counts.entry(c.channel.0 as u64).or_insert(0) += 1;
                                                                }

                                                                // Build channel tree (root channels sorted by order, then sub-channels)
                                                                let mut lines: Vec<String> = Vec::new();

                                                                // Collect and sort channels by parent, then order
                                                                let mut channels: Vec<_> = state.channels.iter().collect();
                                                                channels.sort_by_key(|(_, ch)| (ch.parent.0, ch.order.0));

                                                                // Simple flat list with indentation for sub-channels
                                                                for (id, ch) in &channels {
                                                                    let count = client_counts.get(&(id.0 as u64)).copied().unwrap_or(0);
                                                                    let indent = if ch.parent.0 == 0 { "" } else { "  " };
                                                                    let users = if count > 0 {
                                                                        format!(" [b]({})[/b]", count)
                                                                    } else {
                                                                        String::new()
                                                                    };
                                                                    lines.push(format!("{}• {}{}", indent, ch.name, users));
                                                                }

                                                                let total_channels = channels.len();
                                                                let total_clients: usize = client_counts.values().sum();
                                                                Some(format!(
                                                                    "📡 [b]Channels[/b] — {} channels, {} utilisateurs\n{}",
                                                                    total_channels,
                                                                    total_clients,
                                                                    lines.join("\n")
                                                                ))
                                                            } else {
                                                                Some("❌ État TS3 indisponible.".to_string())
                                                            }
                                                        }).await;
                                                        match result {
                                                            Ok(Some(msg)) => { let _ = tx_ch.try_send(OutgoingMessage::reply(msg, &rt_ch, rs_ch)); }
                                                            Ok(None) => { let _ = tx_ch.try_send(OutgoingMessage::reply("❌ Erreur interne.".to_string(), &rt_ch, rs_ch)); }
                                                            Err(e) => { let _ = tx_ch.try_send(OutgoingMessage::reply(format!("❌ Erreur: {}", e), &rt_ch, rs_ch)); }
                                                        }
                                                    });
                                                } else if msg_lower.starts_with("!volume") || msg_lower.starts_with("!vol") {
                                                    let parts: Vec<&str> = message.split_whitespace().collect();
                                                    if parts.len() < 2 {
                                                        // Show current volume (default 100 if TTS disabled)
                                                        let vol = audio_player.as_ref().map(|p| p.volume()).unwrap_or(100);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("🔊 Volume actuel : {}%", vol),
                                                            &reply_target, reply_sender_id,
                                                        ));
                                                    } else if let Ok(vol) = parts[1].trim_end_matches('%').parse::<u8>() {
                                                        if vol > 200 {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                "❌ Volume entre 0 et 200 (100 = normal)".to_string(),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        } else {
                                                            if let Some(ref player) = audio_player {
                                                                player.set_volume(vol);
                                                            }
                                                            // Persist volume
                                                            let _ = std::fs::create_dir_all("data");
                                                            let muted_val = tts_muted.load(std::sync::atomic::Ordering::Relaxed);
                                                            let voice_val = default_voice.read().unwrap().clone();
                                                            let _ = std::fs::write("data/bot_state.json", format!(r#"{{"muted":{},"volume":{},"voice":"{}"}}"#, muted_val, vol, voice_val));
                                                            let emoji = if vol == 0 { "🔇" } else if vol < 50 { "🔈" } else if vol <= 100 { "🔉" } else { "🔊" };
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                format!("{} Volume réglé à {}%", emoji, vol),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        }
                                                    } else {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            "❌ Usage: !volume [0-200]".to_string(),
                                                            &reply_target, reply_sender_id,
                                                        ));
                                                    }
                                                } else if msg_lower.starts_with("!voice") {
                                                    let valid_voices = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"];
                                                    let parts: Vec<&str> = message.split_whitespace().collect();
                                                    if parts.len() < 2 {
                                                        // Show current default voice
                                                        let current = default_voice.read().unwrap().clone();
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("🎙️ Voix par défaut : [b]{}[/b]\nVoix disponibles : {}", current, valid_voices.join(", ")),
                                                            &reply_target, reply_sender_id,
                                                        ));
                                                    } else {
                                                        let requested = parts[1].to_lowercase();
                                                        if valid_voices.contains(&requested.as_str()) {
                                                            *default_voice.write().unwrap() = requested.clone();
                                                            // Persist
                                                            let _ = std::fs::create_dir_all("data");
                                                            let muted_val = tts_muted.load(std::sync::atomic::Ordering::Relaxed);
                                                            let vol_val = tts_volume.load(std::sync::atomic::Ordering::Relaxed);
                                                            let _ = std::fs::write("data/bot_state.json", format!(r#"{{"muted":{},"volume":{},"voice":"{}"}}"#, muted_val, vol_val, requested));
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                format!("🎙️ Voix par défaut changée en [b]{}[/b]", requested),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                format!("❌ Voix inconnue : \"{}\"\nVoix disponibles : {}", parts[1], valid_voices.join(", ")),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        }
                                                    }
                                                } else if msg_lower == "!mute" {
                                                    tts_muted.store(true, std::sync::atomic::Ordering::Relaxed);
                                                    // Persist mute state
                                                    let _ = std::fs::create_dir_all("data");
                                                    let vol_val = tts_volume.load(std::sync::atomic::Ordering::Relaxed);
                                                    let voice_val = default_voice.read().unwrap().clone();
                                                    let _ = std::fs::write("data/bot_state.json", format!(r#"{{"muted":true,"volume":{},"voice":"{}"}}"#, vol_val, voice_val));
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        "🔇 TTS muté — je reste à l'écoute mais ne parlerai pas.".to_string(),
                                                        &reply_target, reply_sender_id,
                                                    ));
                                                } else if msg_lower == "!unmute" {
                                                    tts_muted.store(false, std::sync::atomic::Ordering::Relaxed);
                                                    // Persist unmute state
                                                    let _ = std::fs::create_dir_all("data");
                                                    let vol_val = tts_volume.load(std::sync::atomic::Ordering::Relaxed);
                                                    let voice_val = default_voice.read().unwrap().clone();
                                                    let _ = std::fs::write("data/bot_state.json", format!(r#"{{"muted":false,"volume":{},"voice":"{}"}}"#, vol_val, voice_val));
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        "🔊 TTS réactivé — je parle à nouveau !".to_string(),
                                                        &reply_target, reply_sender_id,
                                                    ));
                                                } else if msg_lower.starts_with("!greet") {
                                                    let parts: Vec<&str> = msg_lower.split_whitespace().collect();
                                                    if parts.len() >= 2 {
                                                        match parts[1] {
                                                            "on" => {
                                                                greet_enabled.store(true, std::sync::atomic::Ordering::Relaxed);
                                                                let _ = std::fs::create_dir_all("data");
                                                                let _ = std::fs::write("data/greet.json", r#"{"enabled":true}"#);
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                    "👋 Greetings activés — je saluerai les arrivants !".to_string(),
                                                                    &reply_target, reply_sender_id,
                                                                ));
                                                            }
                                                            "off" => {
                                                                greet_enabled.store(false, std::sync::atomic::Ordering::Relaxed);
                                                                let _ = std::fs::create_dir_all("data");
                                                                let _ = std::fs::write("data/greet.json", r#"{"enabled":false}"#);
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                    "🔕 Greetings désactivés.".to_string(),
                                                                    &reply_target, reply_sender_id,
                                                                ));
                                                            }
                                                            _ => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                    "❌ Usage: !greet on|off".to_string(),
                                                                    &reply_target, reply_sender_id,
                                                                ));
                                                            }
                                                        }
                                                    } else {
                                                        let status = if greet_enabled.load(std::sync::atomic::Ordering::Relaxed) { "activés ✅" } else { "désactivés ❌" };
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("👋 Greetings : {} — !greet on|off pour changer", status),
                                                            &reply_target, reply_sender_id,
                                                        ));
                                                    }
                                                } else if msg_lower.starts_with("!timeout") {
                                                    let parts: Vec<&str> = msg_lower.split_whitespace().collect();
                                                    if parts.len() >= 2 {
                                                        if let Ok(ms) = parts[1].parse::<u64>() {
                                                            let clamped = ms.clamp(500, 10000);
                                                            let mut bm = buffer_manager.lock().await;
                                                            bm.set_silence_timeout_ms(clamped);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                format!("⏱️ Silence timeout : {}ms", clamped),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                "❌ Usage: !timeout <ms> (500-10000)".to_string(),
                                                                &reply_target, reply_sender_id,
                                                            ));
                                                        }
                                                    } else {
                                                        let bm = buffer_manager.lock().await;
                                                        let current = bm.silence_timeout_ms();
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("⏱️ Silence timeout : {}ms — !timeout <ms> pour changer (500-10000)", current),
                                                            &reply_target, reply_sender_id,
                                                        ));
                                                    }
                                                } else if msg_lower.starts_with("!roll") || msg_lower.starts_with("!dice") {
                                                    // Dice roller: !roll [NdS[+/-M]] — default 1d6
                                                    let args = message.split_whitespace().skip(1).collect::<Vec<&str>>().join(" ");
                                                    let dice_str = if args.trim().is_empty() { "1d6" } else { args.trim() };

                                                    // Parse dice notation: NdS+M or NdS-M
                                                    let result = (|| -> std::result::Result<String, String> {
                                                        let s = dice_str.to_lowercase();

                                                        // Check for simple number (e.g., !roll 20 = random 1-20)
                                                        if let Ok(max) = s.parse::<i64>() {
                                                            if max < 1 || max > 1000000 { return Err("Nombre entre 1 et 1000000 svp".to_string()); }
                                                            use rand::Rng;
                                                            let val = rand::thread_rng().gen_range(1..=max);
                                                            return Ok(format!("🎲 1-{} → [b]{}[/b]", max, val));
                                                        }

                                                        // Parse NdS[+/-M]
                                                        let d_pos = s.find('d').ok_or("Format: NdS, NdS+M, NdS-M (ex: 2d6, 1d20+3)")?;
                                                        let count_str = &s[..d_pos];
                                                        let count: u32 = if count_str.is_empty() { 1 } else {
                                                            count_str.parse().map_err(|_| "Nombre de dés invalide")?
                                                        };
                                                        if count < 1 || count > 100 { return Err("1 à 100 dés max".to_string()); }

                                                        let rest = &s[d_pos+1..];
                                                        // Split on + or -
                                                        let (sides_str, modifier) = if let Some(pos) = rest.find('+') {
                                                            (&rest[..pos], rest[pos+1..].parse::<i64>().map_err(|_| "Modificateur invalide")?)
                                                        } else if let Some(pos) = rest[1..].find('-') {
                                                            let pos = pos + 1; // offset because we started searching at index 1
                                                            (&rest[..pos], -(rest[pos+1..].parse::<i64>().map_err(|_| "Modificateur invalide")?))
                                                        } else {
                                                            (rest, 0i64)
                                                        };
                                                        let sides: u32 = sides_str.parse().map_err(|_| "Nombre de faces invalide")?;
                                                        if sides < 2 || sides > 1000 { return Err("2 à 1000 faces".to_string()); }

                                                        use rand::Rng;
                                                        let mut rng = rand::thread_rng();
                                                        let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
                                                        let sum: i64 = rolls.iter().map(|&r| r as i64).sum::<i64>() + modifier;

                                                        if count == 1 && modifier == 0 {
                                                            Ok(format!("🎲 d{} → [b]{}[/b]", sides, rolls[0]))
                                                        } else if count <= 20 {
                                                            let details: Vec<String> = rolls.iter().map(|r| r.to_string()).collect();
                                                            let mod_str = if modifier > 0 { format!("+{}", modifier) } else if modifier < 0 { format!("{}", modifier) } else { String::new() };
                                                            Ok(format!("🎲 {}d{}{} → ({}) = [b]{}[/b]", count, sides, mod_str, details.join("+"), sum))
                                                        } else {
                                                            let mod_str = if modifier > 0 { format!("+{}", modifier) } else if modifier < 0 { format!("{}", modifier) } else { String::new() };
                                                            Ok(format!("🎲 {}d{}{} → [b]{}[/b]", count, sides, mod_str, sum))
                                                        }
                                                    })();

                                                    let response = match result {
                                                        Ok(s) => s,
                                                        Err(e) => format!("❌ {}", e),
                                                    };
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower.starts_with("!quote") {
                                                    // Quote book: save and recall memorable quotes
                                                    let args = message.get(6..).unwrap_or("").trim();
                                                    let quotes_path = "data/quotes.json";

                                                    // Load quotes from file
                                                    let mut quotes: Vec<serde_json::Value> = std::fs::read_to_string(quotes_path)
                                                        .ok()
                                                        .and_then(|s| serde_json::from_str(&s).ok())
                                                        .unwrap_or_default();

                                                    let response = if args.starts_with("add ") || args.starts_with("add\t") {
                                                        let quote_text = args[4..].trim();
                                                        if quote_text.is_empty() {
                                                            "❌ Usage: !quote add <texte>".to_string()
                                                        } else if quote_text.len() > 500 {
                                                            "❌ Quote trop longue (max 500 caractères)".to_string()
                                                        } else {
                                                            let entry = serde_json::json!({
                                                                "text": quote_text,
                                                                "author": invoker.name.to_string(),
                                                                "date": chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string(),
                                                            });
                                                            quotes.push(entry);
                                                            let _ = std::fs::create_dir_all("data");
                                                            let _ = std::fs::write(quotes_path, serde_json::to_string_pretty(&quotes).unwrap_or_default());
                                                            format!("💬 Quote #{} sauvegardée !", quotes.len())
                                                        }
                                                    } else if args == "list" {
                                                        if quotes.is_empty() {
                                                            "📖 Aucune quote sauvegardée. Utilise [b]!quote add <texte>[/b]".to_string()
                                                        } else {
                                                            let start = if quotes.len() > 5 { quotes.len() - 5 } else { 0 };
                                                            let mut lines = vec![format!("📖 Dernières quotes ({}/{}) :", quotes.len() - start, quotes.len())];
                                                            for (i, q) in quotes[start..].iter().enumerate() {
                                                                let num = start + i + 1;
                                                                let text = q.get("text").and_then(|v| v.as_str()).unwrap_or("?");
                                                                let author = q.get("author").and_then(|v| v.as_str()).unwrap_or("?");
                                                                lines.push(format!("#{} — \"{}\" — {}", num, text, author));
                                                            }
                                                            lines.join("\n")
                                                        }
                                                    } else if args == "count" {
                                                        format!("📖 {} quote(s) sauvegardée(s)", quotes.len())
                                                    } else if args.starts_with("del ") || args.starts_with("delete ") {
                                                        let num_str = args.split_whitespace().nth(1).unwrap_or("");
                                                        if let Ok(num) = num_str.parse::<usize>() {
                                                            if num >= 1 && num <= quotes.len() {
                                                                let removed = quotes.remove(num - 1);
                                                                let _ = std::fs::write(quotes_path, serde_json::to_string_pretty(&quotes).unwrap_or_default());
                                                                let text = removed.get("text").and_then(|v| v.as_str()).unwrap_or("?");
                                                                format!("🗑️ Quote #{} supprimée : \"{}\"", num, text)
                                                            } else {
                                                                format!("❌ Numéro invalide (1-{})", quotes.len())
                                                            }
                                                        } else {
                                                            "❌ Usage: !quote del <numéro>".to_string()
                                                        }
                                                    } else if args.is_empty() {
                                                        // Random quote
                                                        if quotes.is_empty() {
                                                            "📖 Aucune quote sauvegardée. Utilise [b]!quote add <texte>[/b]".to_string()
                                                        } else {
                                                            use rand::Rng;
                                                            let idx = rand::thread_rng().gen_range(0..quotes.len());
                                                            let q = &quotes[idx];
                                                            let text = q.get("text").and_then(|v| v.as_str()).unwrap_or("?");
                                                            let author = q.get("author").and_then(|v| v.as_str()).unwrap_or("?");
                                                            let date = q.get("date").and_then(|v| v.as_str()).unwrap_or("");
                                                            format!("💬 #{}/{} — \"{}\" — {} ({})", idx + 1, quotes.len(), text, author, date)
                                                        }
                                                    } else {
                                                        "❌ Usage: !quote [add <texte>|list|count|del <n>]".to_string()
                                                    };
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower.starts_with("!history") {
                                                    let args = message.get(8..).unwrap_or("").trim();
                                                    let count: usize = args.parse().unwrap_or(10).min(50).max(1);
                                                    let hist = chat_history.lock().await;
                                                    if hist.is_empty() {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("📜 Aucun historique.".to_string(), &reply_target, reply_sender_id));
                                                    } else {
                                                        let start = if hist.len() > count { hist.len() - count } else { 0 };
                                                        let mut lines = vec![format!("📜 Derniers {} message(s) :", hist.len() - start)];
                                                        for (ts, author, text) in hist.iter().skip(start) {
                                                            let truncated = if text.len() > 100 {
                                                                format!("{}...", truncate_str(text, 100))
                                                            } else {
                                                                text.clone()
                                                            };
                                                            lines.push(format!("[{}] {} : {}", ts, author, truncated));
                                                        }
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(lines.join("\n"), &reply_target, reply_sender_id));
                                                    }
                                                    drop(hist);
                                                } else if msg_lower == "!ping" {
                                                    // Respond with pong + uptime info (no TS3 command needed)
                                                    let uptime = start_time.elapsed();
                                                    let uptime_secs = uptime.as_secs();
                                                    let uptime_str = if uptime_secs < 60 {
                                                        format!("{}s", uptime_secs)
                                                    } else if uptime_secs < 3600 {
                                                        format!("{}m {}s", uptime_secs / 60, uptime_secs % 60)
                                                    } else {
                                                        format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60)
                                                    };
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        format!("🏓 Pong ! (uptime: {})", uptime_str),
                                                        &reply_target, reply_sender_id
                                                    ));
                                                } else if msg_lower.starts_with("!seen") {
                                                    let query = message.get(5..).unwrap_or("").trim();
                                                    let seen = seen_data.lock().await;
                                                    if query.is_empty() {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("👁️ {} utilisateur(s) trackés — !seen <nom> pour chercher", seen.len()),
                                                            &reply_target, reply_sender_id
                                                        ));
                                                    } else {
                                                        let query_lower = query.to_lowercase();
                                                        let matches: Vec<_> = seen.values()
                                                            .filter(|(name, _)| name.to_lowercase().contains(&query_lower))
                                                            .collect();
                                                        let response = if matches.is_empty() {
                                                            format!("❌ Aucun résultat pour \"{}\"", query)
                                                        } else if matches.len() == 1 {
                                                            let (name, ts) = &matches[0];
                                                            format!("👁️ {} — dernière déconnexion : {}", name, ts)
                                                        } else {
                                                            let mut lines = vec![format!("👁️ {} résultats pour \"{}\" :", matches.len(), query)];
                                                            for (name, ts) in matches.iter().take(5) {
                                                                lines.push(format!("• {} — {}", name, ts));
                                                            }
                                                            lines.join("\n")
                                                        };
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                    }
                                                    drop(seen);
                                                } else if msg_lower.starts_with("!lang") {
                                                    let parts: Vec<&str> = message.split_whitespace().collect();
                                                    if parts.len() < 2 || parts[1] == "auto" {
                                                        // Reset to auto-detect
                                                        let mut overrides = language_overrides.lock().await;
                                                        overrides.remove(&sender_uid);
                                                        let _ = save_language_prefs(&overrides);
                                                        drop(overrides);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("🌍 Langue : auto-détection".to_string(), &reply_target, reply_sender_id));
                                                    } else {
                                                        let lang_code = parts[1].to_lowercase();
                                                        // Validate: must be 2-letter ISO 639-1
                                                        let valid_langs = ["fr", "en", "de", "es", "it", "pt", "nl", "ru", "ja", "ko", "zh", "ar", "pl", "cs", "sv", "da", "fi", "no", "tr", "uk", "ro", "hu", "el", "he", "th", "vi", "id", "ms", "hi", "bn"];
                                                        if valid_langs.contains(&lang_code.as_str()) {
                                                            let mut overrides = language_overrides.lock().await;
                                                            overrides.insert(sender_uid.clone(), lang_code.clone());
                                                            let _ = save_language_prefs(&overrides);
                                                            drop(overrides);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(format!("🌍 Langue forcée : [b]{}[/b]", lang_code), &reply_target, reply_sender_id));
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(format!("❌ Langue inconnue : {}. Ex: !lang fr, !lang en, !lang auto", lang_code), &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!tts ") {
                                                    // Parse optional voice:XX and speed:XX prefixes
                                                    let raw_text = message[5..].trim();
                                                    let valid_voices = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"];
                                                    let mut tts_voice: Option<String> = None;
                                                    let mut tts_speed: Option<f32> = None;
                                                    let mut remaining = raw_text;
                                                    // Extract options from the start of the text
                                                    loop {
                                                        let trimmed = remaining.trim_start();
                                                        if let Some(rest) = trimmed.strip_prefix("voice:") {
                                                            let end = rest.find(' ').unwrap_or(rest.len());
                                                            let v = &rest[..end];
                                                            if valid_voices.contains(&v.to_lowercase().as_str()) {
                                                                tts_voice = Some(v.to_lowercase());
                                                                remaining = &rest[end..];
                                                                continue;
                                                            }
                                                        }
                                                        if let Some(rest) = trimmed.strip_prefix("speed:") {
                                                            let end = rest.find(' ').unwrap_or(rest.len());
                                                            if let Ok(s) = rest[..end].parse::<f32>() {
                                                                if (0.25..=4.0).contains(&s) {
                                                                    tts_speed = Some(s);
                                                                    remaining = &rest[end..];
                                                                    continue;
                                                                }
                                                            }
                                                        }
                                                        break;
                                                    }
                                                    let tts_text = remaining.trim().to_string();
                                                    if tts_text.is_empty() {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            "❌ Usage: !tts [voice:nova] [speed:1.5] <texte>\nVoix: alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer, verse".to_string(),
                                                            &reply_target, reply_sender_id));
                                                    } else if tts_text.len() > 500 {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Texte trop long (max 500 caractères)".to_string(), &reply_target, reply_sender_id));
                                                    } else {
                                                        // Rate limit: max 5 TTS per user per 60s
                                                        let rate_ok = {
                                                            let mut limits = tts_rate_limits.lock().await;
                                                            let now = std::time::Instant::now();
                                                            let entries = limits.entry(invoker.id.0 as u64).or_insert_with(Vec::new);
                                                            entries.retain(|t| now.duration_since(*t).as_secs() < 60);
                                                            if entries.len() >= 5 {
                                                                false
                                                            } else {
                                                                entries.push(now);
                                                                true
                                                            }
                                                        };
                                                        if !rate_ok {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("⏳ Rate limit : max 5 TTS par minute. Attends un peu !".to_string(), &reply_target, reply_sender_id));
                                                        } else if let (Some(ref player), Some(ref synth)) = (&audio_player, &tts_synth) {
                                                        let sender_name = invoker.name.to_string();
                                                        let voice_label = tts_voice.as_deref().unwrap_or("default");
                                                        let speed_label = tts_speed.map_or("default".to_string(), |s| format!("{:.1}x", s));
                                                        info!("🔊 TTS request from {} (voice: {}, speed: {}): '{}'", sender_name, voice_label, speed_label, tts_text);
                                                        // Save for !replay
                                                        {
                                                            let mut ls = last_spoken_for_ts3.lock().await;
                                                            *ls = Some((tts_text.clone(), tts_voice.clone(), tts_speed));
                                                        }
                                                        let player_ref = player.clone();
                                                        let synth_ref = synth.clone();
                                                        let tx_tts = ts3_msg_tx.clone();
                                                        let rt_tts = reply_target.clone();
                                                        let rs_tts = reply_sender_id;
                                                        let evt_tts = event_tx_clone.clone();
                                                        tokio::spawn(async move {
                                                            let _ = evt_tts.send(WebSocketEvent::speak_started(tts_text.clone()));
                                                            let start = std::time::Instant::now();
                                                            let speak_result = player_ref.speak(tts_text.clone(), tts_voice, tts_speed, synth_ref).await;
                                                            let duration_ms = start.elapsed().as_millis() as u64;
                                                            match speak_result {
                                                                Ok(_) => {
                                                                    let _ = evt_tts.send(WebSocketEvent::speak_completed(tts_text, duration_ms));
                                                                }
                                                                Err(e) => {
                                                                    let _ = tx_tts.try_send(OutgoingMessage::reply(format!("❌ TTS error: {}", e), &rt_tts, rs_tts));
                                                                    let _ = evt_tts.send(WebSocketEvent::speak_failed(tts_text, duration_ms, format!("{}", e)));
                                                                }
                                                            }
                                                        });
                                                    } else {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ TTS désactivé".to_string(), &reply_target, reply_sender_id));
                                                    }
                                                    } // close rate limit else block
                                                } else if msg_lower.starts_with("!replay") {
                                                    let ls = last_spoken_for_ts3.lock().await;
                                                    if let Some((ref text, ref voice, speed)) = *ls {
                                                        if let (Some(ref player), Some(ref synth)) = (&audio_player, &tts_synth) {
                                                            info!("🔁 Replay requested by {}", invoker.name);
                                                            let player_ref = player.clone();
                                                            let synth_ref = synth.clone();
                                                            let replay_text = text.clone();
                                                            let replay_voice = voice.clone();
                                                            let replay_speed = speed;
                                                            let evt_replay = event_tx_clone.clone();
                                                            drop(ls);
                                                            tokio::spawn(async move {
                                                                let _ = evt_replay.send(WebSocketEvent::speak_started(replay_text.clone()));
                                                                let start = std::time::Instant::now();
                                                                let result = player_ref.speak(replay_text.clone(), replay_voice, replay_speed, synth_ref).await;
                                                                let duration_ms = start.elapsed().as_millis() as u64;
                                                                match result {
                                                                    Ok(_) => { let _ = evt_replay.send(WebSocketEvent::speak_completed(replay_text, duration_ms)); }
                                                                    Err(e) => { let _ = evt_replay.send(WebSocketEvent::speak_failed(replay_text, duration_ms, format!("{}", e))); }
                                                                }
                                                            });
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("🔁 Replay...".to_string(), &reply_target, reply_sender_id));
                                                        } else {
                                                            drop(ls);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ TTS désactivé".to_string(), &reply_target, reply_sender_id));
                                                        }
                                                    } else {
                                                        drop(ls);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Rien à rejouer".to_string(), &reply_target, reply_sender_id));
                                                    }
                                                } else if msg_lower.starts_with("!stop") {
                                                    let sender_id = invoker.id.0 as u64;
                                                    let sender_name = invoker.name.to_string();
                                                    // Stop TTS playback
                                                    if let Some(ref player) = audio_player {
                                                        if player.is_speaking() { player.stop(); }
                                                    }
                                                    // Deactivate listening for this user
                                                    let mut bm = buffer_manager.lock().await;
                                                    let was_active = if let Some(buffer) = bm.get_buffer_mut(sender_id) {
                                                        let active = buffer.is_active;
                                                        if active { buffer.deactivate(); }
                                                        active
                                                    } else { false };
                                                    let still_listening = !bm.get_active_speakers().is_empty();
                                                    drop(bm);
                                                    if was_active {
                                                        info!("🛑 Stop trigger from {} (id: {})", sender_name, sender_id);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(format!("🛑 OK {}, j'arrête.", sender_name), &reply_target, reply_sender_id));
                                                        // Update nickname if no more active listeners
                                                        let mut nick_sender = ts3_sender.clone();
                                                        tokio::spawn(async move { update_bot_nickname(&mut nick_sender, still_listening).await; });
                                                    } else {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("🔇 Rien à arrêter.".to_string(), &reply_target, reply_sender_id));
                                                    }
                                                } else if msg_lower.starts_with("!listen") || msg_lower.starts_with("!marlbot") {
                                                    let sender_id = invoker.id.0 as u64;
                                                    let sender_name = invoker.name.to_string();
                                                    info!("🎤 Chat trigger from {} (id: {})", sender_name, sender_id);

                                                    // Stop current speech if any
                                                    if let Some(ref player) = audio_player {
                                                        if player.is_speaking() { player.stop(); }
                                                    }

                                                    let mut bm = buffer_manager.lock().await;
                                                    let buffer = bm.get_or_create_buffer(sender_id, sender_name.clone(), sender_uid.clone());
                                                    if !buffer.is_active {
                                                        buffer.activate();
                                                        buffer.clear();
                                                        drop(bm);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(format!("🎤 J'écoute, {} !", sender_name), &reply_target, reply_sender_id));
                                                        // Update nickname to show listening state
                                                        let mut nick_sender = ts3_sender.clone();
                                                        tokio::spawn(async move { update_bot_nickname(&mut nick_sender, true).await; });
                                                        // Play confirmation audio if available
                                                        if let (Some(ref player), Some(ref cached)) = (&audio_player, &wake_confirmation_frames) {
                                                            let frames = (**cached).clone();
                                                            let player_ref = player.clone();
                                                            tokio::spawn(async move {
                                                                if let Err(e) = player_ref.play_cached(frames, "Oui ?".to_string()).await {
                                                                    warn!("Failed to play confirmation: {}", e);
                                                                }
                                                            });
                                                        }
                                                    } else {
                                                        drop(bm);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(format!("Je t'écoute déjà, {} 😉", sender_name), &reply_target, reply_sender_id));
                                                    }
                                                }
                                            }
                                            Event::PropertyAdded { id, .. } => {
                                                if let PropertyId::Client(client_id) = id {
                                                    let client_id_u64 = client_id.0 as u64;
                                                    let mut sender = ts3_sender.clone();
                                                    let tx = event_tx_clone.clone();
                                                    tokio::spawn(async move {
                                                        let result = sender.with_connection(move |con| {
                                                            con.get_state().ok().and_then(|state| {
                                                                state.clients.get(&client_id).map(|c| {
                                                                    let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                    let cc = if c.country_code.is_empty() { None } else { Some(c.country_code.clone()) };
                                                                    (c.name.clone(), c.channel.0 as u64, uid, cc)
                                                                })
                                                            })
                                                        }).await;
                                                        if let Ok(Some((name, channel_id, uid, country_code))) = result {
                                                            info!("Client connected: {} (id: {}, uid: {:?})", name, client_id_u64, uid);
                                                            let _ = tx.send(WebSocketEvent::ClientConnected {
                                                                client_id: client_id_u64,
                                                                client_name: name,
                                                                channel_id,
                                                                uid,
                                                                country_code,
                                                            });
                                                        }
                                                    });
                                                }
                                            }
                                            Event::PropertyRemoved { id, old, .. } => {
                                                if let (PropertyId::Client(client_id), PropertyValue::Client(client)) = (id, old) {
                                                    let uid = client.uid.as_ref().map(|u| base64::encode(&u.0));
                                                    let client_id_u64 = client_id.0 as u64;
                                                    info!("Client disconnected: {} (id: {}, uid: {:?})", client.name, client_id_u64, uid);
                                                    // Record last-seen time
                                                    if let Some(ref uid_str) = uid {
                                                        let mut seen = seen_data.lock().await;
                                                        seen.insert(uid_str.clone(), (client.name.clone(), chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string()));
                                                        let _ = std::fs::create_dir_all("data");
                                                        let _ = std::fs::write("data/seen.json", serde_json::to_string_pretty(&*seen).unwrap_or_default());
                                                        drop(seen);
                                                    }
                                                    // Deactivate listening if this user was being listened to
                                                    {
                                                        let mut bm = buffer_manager.lock().await;
                                                        if let Some(buffer) = bm.get_buffer_mut(client_id_u64) {
                                                            if buffer.is_active {
                                                                buffer.deactivate();
                                                                info!("🔇 Auto-deactivated listening for disconnected user {} (id: {})", client.name, client_id_u64);
                                                            }
                                                        }
                                                        let still_listening = !bm.get_active_speakers().is_empty();
                                                        drop(bm);
                                                        if !still_listening {
                                                            let mut nick_sender = ts3_sender.clone();
                                                            tokio::spawn(async move { update_bot_nickname(&mut nick_sender, false).await; });
                                                        }
                                                    }
                                                    let _ = event_tx_clone.send(WebSocketEvent::ClientDisconnected {
                                                        client_id: client_id_u64,
                                                        client_name: client.name.clone(),
                                                        uid,
                                                    });
                                                }
                                            }
                                            Event::PropertyChanged { id, old, .. } => {
                                                if let (PropertyId::ClientChannel(client_id), PropertyValue::ChannelId(old_channel)) = (id, old) {
                                                    let old_channel_id = old_channel.0 as u64;
                                                    let client_id_u64 = client_id.0 as u64;
                                                    let mut sender = ts3_sender.clone();
                                                    let tx = event_tx_clone.clone();
                                                    let bm_for_move = buffer_manager.clone();
                                                    let greet_enabled_move = greet_enabled.clone();
                                                    let greet_cooldowns_move = greet_cooldowns.clone();
                                                    let greet_msg_tx = ts3_msg_tx.clone();
                                                    tokio::spawn(async move {
                                                        let result = sender.with_connection(move |con| {
                                                            con.get_state().ok().and_then(|state| {
                                                                let is_self = state.own_client == client_id;
                                                                let bot_channel = state.clients.get(&state.own_client).map(|c| c.channel.0 as u64);
                                                                state.clients.get(&client_id).map(|c| {
                                                                    let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                    let old_ch_name = state.channels.get(&tsclientlib::ChannelId(old_channel.0)).map(|ch| ch.name.clone());
                                                                    let new_ch_name = state.channels.get(&c.channel).map(|ch| ch.name.clone());
                                                                    (c.name.clone(), c.channel.0 as u64, is_self, uid, old_ch_name, new_ch_name, bot_channel)
                                                                })
                                                            })
                                                        }).await;
                                                        if let Ok(Some((name, new_channel_id, is_self, uid, old_ch_name, new_ch_name, bot_channel))) = result {
                                                            // Deactivate listening if user moved away from the bot's channel
                                                            if !is_self {
                                                                if let Some(bot_ch) = bot_channel {
                                                                    if new_channel_id != bot_ch {
                                                                        let mut bm = bm_for_move.lock().await;
                                                                        if let Some(buffer) = bm.get_buffer_mut(client_id_u64) {
                                                                            if buffer.is_active {
                                                                                buffer.deactivate();
                                                                                info!("🔇 Auto-deactivated listening for {} (moved to different channel)", name);
                                                                            }
                                                                        }
                                                                        let still_listening = !bm.get_active_speakers().is_empty();
                                                                        drop(bm);
                                                                        if !still_listening {
                                                                            let mut nick_sender = sender.clone();
                                                                            tokio::spawn(async move { update_bot_nickname(&mut nick_sender, false).await; });
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            info!("Client moved: {} ({} -> {})", name, old_channel_id, new_channel_id);
                                                            let _ = tx.send(WebSocketEvent::ClientMoved {
                                                                client_id: client_id_u64,
                                                                client_name: name.clone(),
                                                                old_channel_id,
                                                                new_channel_id,
                                                                uid,
                                                                old_channel_name: old_ch_name,
                                                                new_channel_name: new_ch_name,
                                                            });
                                                            // Greet user if they joined the bot's channel
                                                            if !is_self {
                                                                if let Some(bot_ch) = bot_channel {
                                                                    if new_channel_id == bot_ch && greet_enabled_move.load(std::sync::atomic::Ordering::Relaxed) {
                                                                        let now = std::time::Instant::now();
                                                                        let should_greet = {
                                                                            let mut cooldowns = greet_cooldowns_move.lock().await;
                                                                            // Clean old entries (>10min)
                                                                            cooldowns.retain(|_, t| now.duration_since(*t).as_secs() < 600);
                                                                            if let Some(last) = cooldowns.get(&client_id_u64) {
                                                                                now.duration_since(*last).as_secs() >= 600
                                                                            } else {
                                                                                true
                                                                            }
                                                                        };
                                                                        if should_greet {
                                                                            greet_cooldowns_move.lock().await.insert(client_id_u64, now);
                                                                            let greetings = [
                                                                                format!("👋 Salut {} !", name),
                                                                                format!("Hey {} ! 🙌", name),
                                                                                format!("Yo {} 👊", name),
                                                                                format!("Bienvenue {} ! 😄", name),
                                                                                format!("{} est dans la place ! 🎉", name),
                                                                            ];
                                                                            let idx = (now.elapsed().subsec_nanos() as usize) % greetings.len();
                                                                            let greeting = &greetings[idx];
                                                                            info!("Greeting {} in bot channel", name);
                                                                            let _ = greet_msg_tx.try_send(OutgoingMessage::channel(greeting.clone()));
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            // Persist channel ID when the bot itself moves
                                                            if is_self {
                                                                info!("Saving last channel: {}", new_channel_id);
                                                                if let Err(e) = std::fs::write(".last_channel", new_channel_id.to_string()) {
                                                                    warn!("Failed to save .last_channel: {}", e);
                                                                }
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                                SyncStreamItem::Audio(audio_data) => {
                                    if transcription_pipeline.is_some() || whisper_api.is_some() {
                                        let audio_inner = audio_data.data();
                                        let data = audio_inner.data();

                                        let (speaker_id, codec_data) = match data {
                                            AudioData::S2C { from, data: audio_bytes, .. } => {
                                                (*from as u64, *audio_bytes)
                                            }
                                            AudioData::S2CWhisper { from, data: audio_bytes, .. } => {
                                                (*from as u64, *audio_bytes)
                                            }
                                            _ => continue,
                                        };

                                        // Resolve real TS3 client name from cache
                                        let (speaker_name, speaker_uid) = {
                                            let names = client_names.read().unwrap_or_else(|e| e.into_inner());
                                            names.get(&speaker_id)
                                                .cloned()
                                                .unwrap_or_else(|| (format!("Speaker_{}", speaker_id), "unknown".to_string()))
                                        };

                                        // Skip audio from bot instances (avoid listening to ourselves)
                                        {
                                            let name_lower = speaker_name.to_lowercase();
                                            if name_lower.starts_with("marlbot") {
                                                continue;
                                            }
                                        }

                                        // Decode Opus + resample via per-speaker decoder
                                        let mut bm = buffer_manager.lock().await;
                                        let buffer = bm.get_or_create_buffer(
                                            speaker_id,
                                            speaker_name.clone(),
                                            speaker_uid.clone(),
                                        );
                                        // Update name in case cache was populated after buffer creation
                                        buffer.speaker_name = speaker_name;
                                        buffer.speaker_uid = speaker_uid;
                                        if buffer.decode_and_push(codec_data) == 0 {
                                            drop(bm);
                                            continue;
                                        }

                                        // Audio wake word detection is DISABLED.
                                        // Wake word is now triggered via text chat messages
                                        // containing "marlbot" (handled by OpenClaw plugin).
                                        // Audio is still buffered for transcription after activation.
                                        if !buffer.is_active {
                                            drop(bm);
                                        }
                                        // Active speaker: audio is just buffered
                                        // Silence detection is handled by the periodic timer
                                    }
                                }
                                _ => {} // Other event types (NetworkStats, etc.)
                            }
                        }
                        Err(e) => {
                            error!("Event stream error: {:?}", e);
                            break;
                        }
                    }
                    } // close event = sync_con.next() arm

                        // Graceful shutdown signal
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() && !shutting_down {
                                shutting_down = true;
                                info!("Graceful shutdown: disconnecting from TS3...");
                                // Spawn disconnect in a separate task because
                                // disconnect() needs the SyncConnection stream to be
                                // polled (via sync_con.next()) to process the command.
                                // We must NOT break here - the loop must continue so
                                // the stream can process the disconnect packet.
                                let mut handle = ts3_sender.clone();
                                let options = DisconnectOptions::new()
                                    .reason(Reason::Clientdisconnect)
                                    .message("Bot shutting down");
                                tokio::spawn(async move {
                                    if let Err(e) = handle.disconnect(options).await {
                                        warn!("TS3 disconnect failed: {:?}", e);
                                    }
                                });
                            }
                        }
                    } // close tokio::select!
                } // close loop

                info!("Event loop ended");
        } // close if let Some(connection)
    });

    // Spawn WebSocket server task (with TTS channel if enabled)
    let tts_tx_for_ws = if tts_enabled { Some(tts_tx.clone()) } else { None };
    let tts_stop_flag_for_ws = tts_stop_flag.clone();
    let ws_handle = tokio::spawn(async move {
        if let Err(e) = websocket::run_server(ws_config, event_tx, tts_tx_for_ws, shared_ts3_handle_for_ws, tts_stop_flag_for_ws, buffer_manager_for_ws, language_overrides_for_ws, tts_volume_for_ws, default_voice_for_ws, chat_history_for_ws).await {
            error!("WebSocket server error: {}", e);
        }
    });

    info!("TS3 Bot ready (TS3 + WebSocket servers running)");

    // Keep running until Ctrl+C or any task fails
    let mut ws_handle = ws_handle;
    let shutdown_requested = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
            true
        }
        _ = &mut ts3_handle => {
            error!("TS3 client task ended unexpectedly");
            false
        }
        _ = &mut ws_handle => {
            error!("WebSocket server task ended unexpectedly");
            false
        }
    };

    if shutdown_requested {
        // Signal the TS3 event loop to disconnect gracefully
        let _ = shutdown_tx.send(true);
        // Wait for the TS3 task to finish (with timeout)
        match tokio::time::timeout(std::time::Duration::from_secs(5), ts3_handle).await {
            Ok(_) => info!("TS3 client shut down cleanly"),
            Err(_) => warn!("TS3 shutdown timed out after 5s, forcing exit"),
        }
    }

    info!("Goodbye!");
    Ok(())
}
