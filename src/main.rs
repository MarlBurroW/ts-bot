mod ts3;

use anyhow::Result;
use ts3_bot::models::{BotConfig, MessageEvent, MessageType, WebSocketEvent, TranscriptionEvent, ActiveDuel, ActivePoll, BotStats, LastSpokenInfo, NotifyWatchers, Reminder, SharedChatHistory};
use ts3_bot::utils::{truncate_str, format_uptime, save_language_prefs, save_bot_state_field, record_history, load_chat_history, update_bot_nickname, valid_voices_for_model, spawn_delayed_channel_kick};
use ts3_bot::persistence::{load_json, load_json_logged, save_json, save_json_compact, ensure_data_dir};
use ts3_bot::commands;
use ts3_bot::websocket;
use ts3_bot::websocket::TtsRequest;
use ts3_bot::tts::{AudioPlayer, HttpTtsSynthesizer, TtsSynthesizer};
use tracing::{error, info, warn, debug};
use ts3::client::TS3Client;
use futures::prelude::*;
use tokio::sync::broadcast;
use tsproto_packets::packets::AudioData;
use ts3_bot::audio::{
    SpeakerBufferManager,
    TranscriptionPipeline,
};
use ts3_bot::audio::whisper_api::WhisperApiTranscriber;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

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

    // Ensure data directory exists (once at startup, not scattered everywhere)
    ensure_data_dir();

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
    let (tts_tx, tts_rx) = tokio::sync::mpsc::channel::<TtsRequest>(10);
    let mut tts_rx_opt: Option<tokio::sync::mpsc::Receiver<TtsRequest>> = Some(tts_rx);

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
    let language_overrides: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(load_json_logged("data/language_prefs.json", "language preference(s)")));
    let language_overrides_for_ws = Some(language_overrides.clone());

    // Load persisted bot state (mute + volume + voice)
    let (persisted_muted, persisted_volume, persisted_voice, persisted_speed) = {
        let v: serde_json::Value = load_json("data/bot_state.json");
        let muted = v.get("muted").and_then(|m| m.as_bool()).unwrap_or(false);
        let vol = v.get("volume").and_then(|v| v.as_u64()).unwrap_or(100) as u8;
        let voice = v.get("voice").and_then(|v| v.as_str()).map(|s| s.to_string());
        let speed = v.get("speed").and_then(|s| s.as_f64()).map(|s| s as f32);
        (muted, vol, voice, speed)
    };
    if persisted_muted || persisted_volume != 100 || persisted_voice.is_some() || persisted_speed.is_some() {
        info!("Restored bot state: muted={}, volume={}%, voice={}, speed={}", persisted_muted, persisted_volume, persisted_voice.as_deref().unwrap_or("config default"), persisted_speed.map_or("default".to_string(), |s| format!("{:.2}", s)));
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

    // Shared default TTS speed (overridable at runtime via !speed, persisted)
    let default_speed: Arc<std::sync::RwLock<f32>> = Arc::new(std::sync::RwLock::new(
        persisted_speed.unwrap_or(1.15)
    ));
    let default_speed_for_tts = default_speed.clone();
    let default_speed_for_ws = Some(default_speed.clone());

    // Last spoken text for !replay (text, voice, speed)
    let last_spoken: LastSpokenInfo = Arc::new(Mutex::new(None));
    let last_spoken_for_ws = last_spoken.clone();
    let last_spoken_for_ts3 = last_spoken.clone();

    // Greeting feature: greet users who join the bot's channel
    let greet_enabled = Arc::new(std::sync::atomic::AtomicBool::new({
        let val: serde_json::Value = load_json("data/greet.json");
        val.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true)
    }));
    // Cooldown: don't greet the same user within 10 minutes (keyed by client_id)
    let greet_cooldowns: Arc<Mutex<HashMap<u64, std::time::Instant>>> = Arc::new(Mutex::new(HashMap::new()));

    // Chat history ring buffer (last 200 messages for !history) — persisted to data/chat_history.jsonl
    let chat_history: SharedChatHistory = Arc::new(Mutex::new(load_chat_history()));
    let chat_history_for_ws = Some(chat_history.clone());

    // TTS rate limiter: max 5 uses per user per 60 seconds (keyed by client_id)
    let tts_rate_limits: Arc<Mutex<HashMap<u64, Vec<std::time::Instant>>>> = Arc::new(Mutex::new(HashMap::new()));

    // Last-seen tracker: UID -> (name, ISO timestamp) — persisted to data/seen.json
    let seen_data: Arc<Mutex<HashMap<String, (String, String)>>> =
        Arc::new(Mutex::new(load_json_logged("data/seen.json", "seen record(s)")));

    // Notify-on-connect watchers: lowercase_target_name -> Vec<(requester_name, requester_uid)>
    // When a client connects whose lowercase name contains the key, poke all requesters
    // Persisted to data/notify.json
    let notify_watchers: NotifyWatchers = {
        let map: HashMap<String, Vec<(String, String)>> = load_json("data/notify.json");
        if !map.is_empty() {
            let total: usize = map.values().map(|v| v.len()).sum();
            info!("Restored {} notify watchers ({} targets) from disk", total, map.len());
        }
        Arc::new(Mutex::new(map))
    };

    // Connect time tracking: UID -> Instant when they were first seen (for !who duration display)
    let connect_times: Arc<Mutex<HashMap<String, std::time::Instant>>> = Arc::new(Mutex::new(HashMap::new()));

    // AFK status: UID -> (username, message). Persisted to data/afk.json
    let afk_status: Arc<Mutex<HashMap<String, (String, String)>>> =
        Arc::new(Mutex::new(load_json_logged("data/afk.json", "AFK entries")));

    let active_poll: Arc<Mutex<Option<ActivePoll>>> = Arc::new(Mutex::new(None));


    let active_duel: Arc<Mutex<Option<ActiveDuel>>> = Arc::new(Mutex::new(None));
    // Reminders: list of (due_timestamp_ms, creator_uid, creator_name, message)
    // Persisted to data/reminders.json
    let reminders: Arc<Mutex<Vec<Reminder>>> =
        Arc::new(Mutex::new(load_json_logged("data/reminders.json", "reminders")));

    // Bot usage statistics — persisted to data/stats.json
    let bot_stats = Arc::new(BotStats::load());

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

        // Reconnection config
        let max_attempts = config.reconnect_max_attempts;
        let initial_delay_ms = config.reconnect_initial_delay_ms;
        let max_delay_ms = config.reconnect_max_delay_ms;
        let mut reconnect_count: u32 = 0;

        // Outer reconnection loop — reconnects on unexpected TS3 disconnects
        'reconnect: loop {

        // Attempt connection with retry loop (uses reconnect_* config)
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

        let connection = match connection_opt {
            Some(c) => c,
            None => {
                if reconnect_count > 0 {
                    // During reconnection, wait and retry the outer loop
                    let delay_ms = (initial_delay_ms * 2u64.saturating_pow(reconnect_count.min(10) - 1)).min(max_delay_ms);
                    warn!("Reconnection cycle {} failed after {} attempts. Retrying in {}ms...", reconnect_count, max_attempts, delay_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    reconnect_count += 1;
                    continue 'reconnect;
                } else {
                    error!("Initial connection failed after {} attempts. Exiting.", max_attempts);
                    return;
                }
            }
        };

        {
                if reconnect_count > 0 {
                    info!("🔄 TS3 reconnected successfully (attempt #{})", reconnect_count);
                    reconnect_count = 0; // Reset on successful connection
                } else {
                    info!("TS3 client connected successfully");
                }

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
                    .is_some_and(|c| !c.is_empty());
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
                let mut shutdown_rx = shutdown_rx.clone();

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

                // Spawn reminder checker task — fires every 5 seconds, sends channel messages for due reminders
                {
                    let reminders_check = reminders.clone();
                    let reminder_tx = ts3_msg_tx.clone();
                    let mut reminder_sender = ts3_sender.clone();
                    tokio::spawn(async move {
                        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                        loop {
                            interval.tick().await;
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            let mut reminders_lock = reminders_check.lock().await;
                            let mut fired = Vec::new();
                            let mut remaining = Vec::new();
                            for r in reminders_lock.drain(..) {
                                if now_ms >= r.due_ms {
                                    fired.push(r);
                                } else {
                                    remaining.push(r);
                                }
                            }
                            *reminders_lock = remaining;
                            let need_save = !fired.is_empty();
                            drop(reminders_lock);

                            for r in &fired {
                                // Send channel message
                                let msg = format!("⏰ Rappel pour [b]{}[/b] : {}", r.name, r.message);
                                let _ = reminder_tx.try_send(OutgoingMessage::channel(msg));

                                // Also try to poke the user if they're online
                                let uid_target = r.uid.clone();
                                let poke_msg = format!("⏰ Rappel : {}", if r.message.len() > 80 { &r.message[..r.message.char_indices().take_while(|&(i, _)| i < 80).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(80)] } else { &r.message });
                                let poke_result = reminder_sender.with_connection(move |con| {
                                    let mut target_clid = None;
                                    if let Ok(state) = con.get_state() {
                                        for client in state.clients.values() {
                                            let uid_str = client.uid.as_ref().map(|u| base64::encode(&u.0)).unwrap_or_default();
                                            if uid_str == uid_target {
                                                target_clid = Some(client.id.0);
                                                break;
                                            }
                                        }
                                    }
                                    target_clid
                                }).await;
                                if let Ok(Some(clid)) = poke_result {
                                    use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                                    let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "clientpoke");
                                    cmd.write_arg("clid", &clid);
                                    cmd.write_arg("msg", &poke_msg);
                                    let _ = reminder_sender.send_command(cmd).await;
                                }
                                info!("Reminder fired for {}: {}", r.name, r.message);
                            }

                            if need_save {
                                let reminders_lock = reminders_check.lock().await;
                                save_json_compact("data/reminders.json", &*reminders_lock);
                            }
                        }
                    });
                }

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
                // Only spawn once (first connection) — tts_rx is consumed by move
                if let Some(tts_rx_taken) = tts_rx_opt.take() {
                if let (Some(ref player), Some(ref synth)) = (&audio_player, &tts_synth) {
                    let player_clone = player.clone();
                    let synth_clone = synth.clone();
                    let tts_chat_tx = ts3_msg_tx.clone();
                    let tts_event_tx = event_tx_clone.clone();
                    let last_spoken_ws = last_spoken_for_ws.clone();
                    let tts_muted_clone = tts_muted_for_tts.clone();
                    let tts_chat_history = chat_history.clone();
                    let bot_stats_tts = bot_stats.clone();
                    let default_voice_clone = default_voice_for_tts.clone();
                    let default_speed_clone = default_speed_for_tts.clone();
                    let mut tts_rx = tts_rx_taken;
                    tokio::spawn(async move {
                        while let Some(request) = tts_rx.recv().await {
                            info!("TTS request: '{}'", request.text);
                            bot_stats_tts.inc_tts();

                            // Clone all state needed by the sub-task
                            let last_spoken_sub = last_spoken_ws.clone();
                            let tts_chat_tx_sub = tts_chat_tx.clone();
                            let tts_chat_history_sub = tts_chat_history.clone();
                            let tts_muted_sub = tts_muted_clone.clone();
                            let tts_event_tx_sub = tts_event_tx.clone();
                            let default_voice_sub = default_voice_clone.clone();
                            let default_speed_for_tts_sub = default_speed_clone.clone();
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

                                // Record bot response to chat history (persisted)
                                {
                                    let truncated = if request.text.len() > 200 {
                                        format!("{}...", truncate_str(&request.text, 200))
                                    } else {
                                        request.text.clone()
                                    };
                                    record_history(&tts_chat_history_sub, "🤖 Marlbot".to_string(), truncated).await;
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
                                // Use default voice/speed overrides if not explicit in request
                                let effective_voice = request.voice.or_else(|| {
                                    Some(default_voice_sub.read().unwrap().clone())
                                });
                                let effective_speed = request.speed.or_else(|| {
                                    Some(*default_speed_for_tts_sub.read().unwrap())
                                });
                                let speak_result = player_sub
                                    .speak(request.text.clone(), effective_voice, effective_speed, synth_sub.clone())
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
                } // close if let Some(tts_rx_taken)

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
                                        bot_stats.inc_transcriptions();

                                        // Record transcription to chat history (persisted)
                                        record_history(&chat_history, format!("🎤{}", speaker_name), command_text.clone()).await;

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
                                        buf.mark_check();
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
                                                    buf.mark_check();
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
                                                buf.mark_check();
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
                            // Persist stats to disk every 60s
                            bot_stats.save();
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
                                                bot_stats.inc_messages();

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
                                                    record_history(&chat_history, invoker.name.to_string(), message.to_string()).await;
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

                                                let sender_name = invoker.name.to_string();

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

                                                // Auto-clear AFK when an AFK user sends any message (except !afk itself)
                                                if !msg_lower.starts_with("!afk") {
                                                    let mut afk = afk_status.lock().await;
                                                    if afk.remove(&sender_uid).is_some() {
                                                        save_json("data/afk.json", &*afk);
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("👋 {} n'est plus AFK", sender_name),
                                                            &reply_target, reply_sender_id
                                                        ));
                                                    }
                                                    drop(afk);
                                                }

                                                // AFK mention detection: if non-command message mentions an AFK user, notify
                                                if !message.starts_with('!') {
                                                    let afk_map = afk_status.lock().await;
                                                    if let Some((afk_name, afk_msg)) = commands::afk_check_mentions(&message, &sender_uid, &afk_map) {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                            format!("💤 {} est AFK : {}", afk_name, afk_msg),
                                                            &reply_target, reply_sender_id
                                                        ));
                                                    }
                                                    drop(afk_map);
                                                }

                                                if msg_lower.starts_with("!") {
                                                    bot_stats.inc_commands();
                                                }

                                                if msg_lower.starts_with("!help") {
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        commands::help_text(),
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
                                                    let uptime_str = format_uptime(uptime.as_secs(), true);

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
                                                    let default_speed_status = default_speed.clone();
                                                    tokio::spawn(async move {
                                                        let channel_info = sender_for_status.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
                                                                let bot_client = state.clients.get(&state.own_client);
                                                                if let Some(bot) = bot_client {
                                                                    let ch_id = bot.channel.0;
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
                                                        let speed_val = *default_speed_status.read().unwrap();
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
                                                             • Vitesse : {:.2}x\n\
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
                                                            speed_val,
                                                            tts_str,
                                                            whisper_str,
                                                            greet_str,
                                                            silence_ms,
                                                        ), &rt_status, rs_status));
                                                    });
                                                } else if msg_lower.starts_with("!who") {
                                                    // Show who's in the same channel as the sender (with connection duration)
                                                    let sender_id = invoker.id.0 as u64;
                                                    let sender_name_who = invoker.name.to_string();
                                                    let mut sender_for_who = ts3_sender.clone();
                                                    let tx_who = ts3_msg_tx.clone();
                                                    let rt_who = reply_target;
                                                    let rs_who = reply_sender_id;
                                                    let connect_times_who = connect_times.clone();
                                                    tokio::spawn(async move {
                                                        // Step 1: Get channel data + client info (name, flags, uid) from TS3 state
                                                        let result = sender_for_who.with_connection(move |con| {
                                                            if let Ok(state) = con.get_state() {
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
                                                                    let mut clients_data: Vec<(String, String, Option<String>)> = Vec::new();
                                                                    for c in state.clients.values() {
                                                                        if c.channel != ch_id { continue; }
                                                                        let mut flags = Vec::new();
                                                                        if c.input_muted { flags.push("🔇mic"); }
                                                                        if c.output_muted { flags.push("🔇son"); }
                                                                        if c.away_message.as_ref().is_some_and(|m| !m.is_empty()) {
                                                                            flags.push("💤away");
                                                                        }
                                                                        let flag_str = if flags.is_empty() {
                                                                            String::new()
                                                                        } else {
                                                                            format!(" ({})", flags.join(", "))
                                                                        };
                                                                        let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                        clients_data.push((c.name.clone(), flag_str, uid));
                                                                    }
                                                                    Some((ch_name, clients_data))
                                                                } else {
                                                                    None
                                                                }
                                                            } else {
                                                                None
                                                            }
                                                        }).await;
                                                        // Step 2: Format with connect durations (async lock)
                                                        let msg = match result {
                                                            Ok(Some((ch_name, clients_data))) => {
                                                                let ct = connect_times_who.lock().await;
                                                                let now = std::time::Instant::now();
                                                                let resolved: Vec<(String, String, Option<u64>)> = clients_data.iter()
                                                                    .map(|(name, flag_str, uid)| {
                                                                        let secs = uid.as_ref()
                                                                            .and_then(|u| ct.get(u))
                                                                            .map(|since| now.duration_since(*since).as_secs());
                                                                        (name.clone(), flag_str.clone(), secs)
                                                                    })
                                                                    .collect();
                                                                commands::who_format(&ch_name, &resolved)
                                                            }
                                                            Ok(None) => "❌ Impossible de trouver ton channel.".to_string(),
                                                            Err(e) => format!("❌ Erreur: {}", e),
                                                        };
                                                        let _ = tx_who.try_send(OutgoingMessage::reply(msg, &rt_who, rs_who));
                                                    });
                                                } else if msg_lower.starts_with("!find") {
                                                    // Find a user on the server by partial name
                                                    let query = message.trim()[5..].trim().to_string();
                                                    if query.is_empty() {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Usage: !find <nom>".to_string(), &reply_target, reply_sender_id));
                                                    } else {
                                                        let mut sender_for_find = ts3_sender.clone();
                                                        let tx_find = ts3_msg_tx.clone();
                                                        let rt_find = reply_target;
                                                        let rs_find = reply_sender_id;
                                                        let query_lower = query.to_lowercase();
                                                        let connect_times_find = connect_times.clone();
                                                        tokio::spawn(async move {
                                                            let result = sender_for_find.with_connection(move |con| {
                                                                if let Ok(state) = con.get_state() {
                                                                    let mut matches: Vec<(String, String, Option<String>)> = Vec::new(); // (name, channel_name, uid)
                                                                    for c in state.clients.values() {
                                                                        if c.name.to_lowercase().contains(&query_lower) {
                                                                            let ch_name = state.channels.get(&c.channel)
                                                                                .map(|ch| ch.name.clone())
                                                                                .unwrap_or_else(|| format!("Channel #{}", c.channel.0));
                                                                            let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                            matches.push((c.name.clone(), ch_name, uid));
                                                                        }
                                                                    }
                                                                    Some(matches)
                                                                } else {
                                                                    None
                                                                }
                                                            }).await;

                                                            let msg = match result {
                                                                Ok(Some(matches)) => {
                                                                    let ct = connect_times_find.lock().await;
                                                                    let now = std::time::Instant::now();
                                                                    let resolved: Vec<(String, String, Option<u64>)> = matches.iter()
                                                                        .map(|(name, ch_name, uid)| {
                                                                            let secs = uid.as_ref()
                                                                                .and_then(|u| ct.get(u))
                                                                                .map(|since| now.duration_since(*since).as_secs());
                                                                            (name.clone(), ch_name.clone(), secs)
                                                                        })
                                                                        .collect();
                                                                    commands::find_format(&query, &resolved)
                                                                }
                                                                Ok(None) => "❌ État du serveur indisponible".to_string(),
                                                                Err(e) => format!("❌ Erreur: {}", e),
                                                            };
                                                            let _ = tx_find.try_send(OutgoingMessage::reply(msg, &rt_find, rs_find));
                                                        });
                                                    }
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
                                                                            best_match = Some((id.0, ch.name.clone()));
                                                                            break;
                                                                        } else if ch_name_lower.contains(&query_lower) && best_match.is_none() {
                                                                            best_match = Some((id.0, ch.name.clone()));
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
                                                                        Some(Ok((own_id, sender_ch.0, ch_name)))
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
                                                                let mut client_counts: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
                                                                for c in state.clients.values() {
                                                                    *client_counts.entry(c.channel.0).or_insert(0) += 1;
                                                                }
                                                                let channels_data: Vec<(String, u64, u64, usize)> = state.channels.iter()
                                                                    .map(|(id, ch)| {
                                                                        let count = client_counts.get(&id.0).copied().unwrap_or(0);
                                                                        (ch.name.clone(), ch.parent.0, ch.order.0, count)
                                                                    })
                                                                    .collect();
                                                                Some(commands::channels_format(&channels_data))
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
                                                    let arg = message.split_whitespace().nth(1).unwrap_or("");
                                                    let current_vol = audio_player.as_ref().map(|p| p.volume()).unwrap_or(100);
                                                    match commands::volume_command(arg, current_vol) {
                                                        commands::VolumeResult::Show(msg) | commands::VolumeResult::Invalid(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::VolumeResult::Set { message: msg, value } => {
                                                            if let Some(ref player) = audio_player {
                                                                player.set_volume(value);
                                                            }
                                                            save_bot_state_field("volume", &value);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!voice") {
                                                    let valid_voices = valid_voices_for_model(&config.tts_model);
                                                    let arg = message.split_whitespace().nth(1).unwrap_or("");
                                                    let current = default_voice.read().unwrap().clone();
                                                    match commands::voice_command(arg, &current, &valid_voices) {
                                                        commands::VoiceResult::Show(msg) | commands::VoiceResult::Invalid(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::VoiceResult::Set { message: msg, voice } => {
                                                            *default_voice.write().unwrap() = voice.clone();
                                                            save_bot_state_field("voice", &voice);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!speed") {
                                                    let arg = message.split_whitespace().nth(1).unwrap_or("");
                                                    let current = *default_speed.read().unwrap();
                                                    match commands::speed_command(arg, current) {
                                                        commands::SpeedResult::Show(msg) | commands::SpeedResult::Invalid(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::SpeedResult::Set { message: msg, value } => {
                                                            *default_speed.write().unwrap() = value;
                                                            save_bot_state_field("speed", &value);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower == "!mute" {
                                                    tts_muted.store(true, std::sync::atomic::Ordering::Relaxed);
                                                    // Persist mute state
                                                    save_bot_state_field("muted", &true);
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        "🔇 TTS muté — je reste à l'écoute mais ne parlerai pas.".to_string(),
                                                        &reply_target, reply_sender_id,
                                                    ));
                                                } else if msg_lower == "!unmute" {
                                                    tts_muted.store(false, std::sync::atomic::Ordering::Relaxed);
                                                    // Persist unmute state
                                                    save_bot_state_field("muted", &false);
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        "🔊 TTS réactivé — je parle à nouveau !".to_string(),
                                                        &reply_target, reply_sender_id,
                                                    ));
                                                } else if msg_lower.starts_with("!greet") {
                                                    let arg = msg_lower.split_whitespace().nth(1).unwrap_or("");
                                                    let current = greet_enabled.load(std::sync::atomic::Ordering::Relaxed);
                                                    match commands::greet_command(arg, current) {
                                                        commands::GreetResult::SetEnabled { message, enabled } => {
                                                            greet_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
                                                            save_json_compact("data/greet.json", &serde_json::json!({"enabled": enabled}));
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(message, &reply_target, reply_sender_id));
                                                        }
                                                        commands::GreetResult::Status(msg) | commands::GreetResult::Invalid(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!timeout") {
                                                    let arg = msg_lower.split_whitespace().nth(1).unwrap_or("");
                                                    let current_ms = {
                                                        let bm = buffer_manager.lock().await;
                                                        bm.silence_timeout_ms()
                                                    };
                                                    match commands::timeout_command(arg, current_ms) {
                                                        commands::TimeoutResult::Set { message, value_ms } => {
                                                            let mut bm = buffer_manager.lock().await;
                                                            bm.set_silence_timeout_ms(value_ms);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(message, &reply_target, reply_sender_id));
                                                        }
                                                        commands::TimeoutResult::Show(msg) | commands::TimeoutResult::Invalid(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!roll") || msg_lower.starts_with("!dice") {
                                                    let args = message.split_whitespace().skip(1).collect::<Vec<&str>>().join(" ");
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        commands::roll_dice(&args), &reply_target, reply_sender_id
                                                    ));
                                                } else if msg_lower.starts_with("!8ball") || msg_lower.starts_with("!8b") || msg_lower.starts_with("!boule") {
                                                    let question = if msg_lower.starts_with("!8ball") {
                                                        message.get(6..).unwrap_or("").trim()
                                                    } else if msg_lower.starts_with("!8b") {
                                                        message.get(3..).unwrap_or("").trim()
                                                    } else {
                                                        message.get(6..).unwrap_or("").trim()
                                                    };
                                                    let response = match commands::eight_ball(&invoker.name, question) {
                                                        Some(s) => s,
                                                        None => "🎱 Pose une question ! (ex: !8ball Est-ce que je vais gagner ?)".to_string(),
                                                    };
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower.starts_with("!roulette") {
                                                    match commands::roulette_command(&invoker.name) {
                                                        commands::RouletteResult::Bang(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            spawn_delayed_channel_kick(ts3_sender.clone(), reply_sender_id, "💀 Roulette russe !", 1500);
                                                        }
                                                        commands::RouletteResult::Survived(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!duel") {
                                                    let args = message.trim()[5..].trim().to_string();
                                                    let args_lower = args.to_lowercase();
                                                    let duel_ref = active_duel.clone();

                                                    if args_lower == "accept" || args_lower == "ok" || args_lower == "oui" {
                                                        let mut duel_guard = duel_ref.lock().await;
                                                        if let Some(duel) = duel_guard.as_ref() {
                                                            match commands::duel_accept(duel, &sender_uid) {
                                                                commands::DuelAcceptResult::NotYourDuel => {
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Ce duel ne te concerne pas !".to_string(), &reply_target, reply_sender_id));
                                                                }
                                                                commands::DuelAcceptResult::Expired => {
                                                                    *duel_guard = None;
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("⏰ Le duel a expiré !".to_string(), &reply_target, reply_sender_id));
                                                                }
                                                                commands::DuelAcceptResult::Resolved { message: msg, loser_clid } => {
                                                                    *duel_guard = None;
                                                                    drop(duel_guard);
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                                    if let Some(loser_clid) = loser_clid {
                                                                        spawn_delayed_channel_kick(ts3_sender.clone(), loser_clid, "💀 Perdu au duel !", 1500);
                                                                    }
                                                                }
                                                            }
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Aucun duel en attente.".to_string(), &reply_target, reply_sender_id));
                                                        }
                                                    } else if args_lower == "decline" || args_lower == "non" || args_lower == "refuse" {
                                                        let mut duel_guard = duel_ref.lock().await;
                                                        if let Some(duel) = duel_guard.as_ref() {
                                                            match commands::duel_decline(duel, &sender_uid) {
                                                                commands::DuelDeclineResult::NotYourDuel => {
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Ce duel ne te concerne pas !".to_string(), &reply_target, reply_sender_id));
                                                                }
                                                                commands::DuelDeclineResult::Declined(msg) => {
                                                                    *duel_guard = None;
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                                }
                                                                commands::DuelDeclineResult::NoDuel => {
                                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Aucun duel en attente.".to_string(), &reply_target, reply_sender_id));
                                                                }
                                                            }
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Aucun duel en attente.".to_string(), &reply_target, reply_sender_id));
                                                        }
                                                    } else if args.is_empty() {
                                                        let duel_guard = duel_ref.lock().await;
                                                        match commands::duel_status(duel_guard.as_ref()) {
                                                            commands::DuelStatusResult::Pending(msg) => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            }
                                                            commands::DuelStatusResult::ExpiredOrNone(msg) => {
                                                                drop(duel_guard);
                                                                let mut dg = duel_ref.lock().await;
                                                                *dg = None;
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            }
                                                        }
                                                    } else {
                                                        // Challenge someone: find target by partial name
                                                        let duel_guard = duel_ref.lock().await;
                                                        if let Some(msg) = commands::duel_check_active(duel_guard.as_ref()) {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            continue;
                                                        }
                                                        drop(duel_guard);

                                                        let mut sender_for_duel = ts3_sender.clone();
                                                        let tx_duel = ts3_msg_tx.clone();
                                                        let rt_duel = reply_target;
                                                        let rs_duel = reply_sender_id;
                                                        let target_query = args.to_lowercase();
                                                        let target_query_display = target_query.clone();
                                                        let challenger_name_duel = invoker.name.clone();
                                                        let challenger_uid_duel = sender_uid.clone();
                                                        let challenger_clid_duel = reply_sender_id;
                                                        let duel_ref2 = active_duel.clone();
                                                        tokio::spawn(async move {
                                                            let result = sender_for_duel.with_connection(move |con| {
                                                                if let Ok(state) = con.get_state() {
                                                                    let mut matches: Vec<(String, String, u16)> = Vec::new();
                                                                    for c in state.clients.values() {
                                                                        if c.name.to_lowercase().contains(&target_query) {
                                                                            let uid = c.uid.as_ref().map(|u| base64::encode(&u.0)).unwrap_or_default();
                                                                            matches.push((c.name.clone(), uid, c.id.0));
                                                                        }
                                                                    }
                                                                    Some(matches)
                                                                } else {
                                                                    None
                                                                }
                                                            }).await;

                                                            match result {
                                                                Ok(Some(matches)) => {
                                                                    let filtered: Vec<_> = matches.into_iter()
                                                                        .filter(|(_, uid, _)| uid != &challenger_uid_duel && !uid.is_empty())
                                                                        .collect();
                                                                    if filtered.is_empty() {
                                                                        let _ = tx_duel.try_send(OutgoingMessage::reply(
                                                                            format!("❌ Aucun adversaire trouvé pour \"{}\"", target_query_display),
                                                                            &rt_duel, rs_duel
                                                                        ));
                                                                    } else if filtered.len() > 1 {
                                                                        let names: Vec<_> = filtered.iter().map(|(n, _, _)| n.as_str()).collect();
                                                                        let _ = tx_duel.try_send(OutgoingMessage::reply(
                                                                            format!("❌ Trop de résultats : {}. Précise le nom.", names.join(", ")),
                                                                            &rt_duel, rs_duel
                                                                        ));
                                                                    } else {
                                                                        let (target_name, target_uid, target_clid) = &filtered[0];
                                                                        let mut dg = duel_ref2.lock().await;
                                                                        *dg = Some(ActiveDuel {
                                                                            challenger_name: challenger_name_duel.clone(),
                                                                            challenger_uid: challenger_uid_duel.clone(),
                                                                            challenger_clid: challenger_clid_duel,
                                                                            target_name: target_name.clone(),
                                                                            target_uid: target_uid.clone(),
                                                                            target_clid: *target_clid,
                                                                            created: std::time::Instant::now(),
                                                                        });
                                                                        let _ = tx_duel.try_send(OutgoingMessage::reply(
                                                                            format!("⚔️ {} défie {} en duel ! 🎲 2d6, le perdant est kick.\n{} a 30 secondes pour taper [b]!duel accept[/b] ou [b]!duel non[/b]", challenger_name_duel, target_name, target_name),
                                                                            &rt_duel, rs_duel
                                                                        ));
                                                                    }
                                                                }
                                                                _ => {
                                                                    let _ = tx_duel.try_send(OutgoingMessage::reply(
                                                                        "❌ Impossible de chercher les utilisateurs.".to_string(),
                                                                        &rt_duel, rs_duel
                                                                    ));
                                                                }
                                                            }
                                                        });
                                                    }

                                                } else if msg_lower.starts_with("!quote") {
                                                    let args = message.get(6..).unwrap_or("").trim();
                                                    let commands::QuoteAction::Response(response) = commands::quote_command(args, &invoker.name, "data/quotes.json");
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower.starts_with("!history") {
                                                    let args = message.get(8..).unwrap_or("").trim();
                                                    let hist = chat_history.lock().await;
                                                    let response = match commands::history_response(&hist, args) {
                                                        Some(formatted) => formatted,
                                                        None => "📜 Aucun historique.".to_string(),
                                                    };
                                                    drop(hist);
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower == "!ping" {
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        commands::ping_response(start_time.elapsed().as_secs()),
                                                        &reply_target, reply_sender_id
                                                    ));
                                                } else if msg_lower == "!stats" {
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                        commands::stats_response(start_time.elapsed().as_secs(), &bot_stats),
                                                        &reply_target, reply_sender_id
                                                    ));
                                                } else if msg_lower.starts_with("!seen") {
                                                    let query = message.get(5..).unwrap_or("").trim();
                                                    let seen = seen_data.lock().await;
                                                    let response = commands::seen_response(&seen, query);
                                                    drop(seen);
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(response, &reply_target, reply_sender_id));
                                                } else if msg_lower.starts_with("!notify") {
                                                    let arg = message.get(7..).unwrap_or("").trim();
                                                    let mut watchers = notify_watchers.lock().await;
                                                    let commands::NotifyResult::Response { message: resp, changed } =
                                                        commands::notify_command(&mut watchers, arg, &sender_uid, &invoker.name);
                                                    if changed {
                                                        save_json("data/notify.json", &*watchers);
                                                    }
                                                    let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(resp, &reply_target, reply_sender_id));
                                                    drop(watchers);
                                                } else if msg_lower.starts_with("!afk") {
                                                    let arg = message.get(4..).unwrap_or("").trim();
                                                    match commands::afk_command(arg, &sender_uid, &sender_name) {
                                                        commands::AfkResult::Clear { message_if_was_afk, message_if_not_afk, .. } => {
                                                            let mut afk = afk_status.lock().await;
                                                            let msg = if afk.remove(&sender_uid).is_some() {
                                                                save_json("data/afk.json", &*afk);
                                                                message_if_was_afk
                                                            } else {
                                                                message_if_not_afk
                                                            };
                                                            drop(afk);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::AfkResult::Set { message: msg, afk_entry } => {
                                                            let mut afk = afk_status.lock().await;
                                                            afk.insert(sender_uid.clone(), afk_entry);
                                                            save_json("data/afk.json", &*afk);
                                                            drop(afk);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!poll") {
                                                    let arg = message.get(5..).unwrap_or("").trim();
                                                    let poll_ref = active_poll.clone();

                                                    if arg.is_empty() || arg == "help" {
                                                        let poll = poll_ref.lock().await;
                                                        match commands::poll_show(poll.as_ref()) {
                                                            commands::PollResponse::Help => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(
                                                                    "📊 Aucun sondage en cours.\n\
                                                                     Créer : [b]!poll Question ? | Option 1 | Option 2 | ...[/b]\n\
                                                                     Voter : [b]!vote <n>[/b]\n\
                                                                     Résultats : [b]!poll[/b]\n\
                                                                     Terminer : [b]!poll end[/b]".to_string(),
                                                                    &reply_target, reply_sender_id
                                                                ));
                                                            }
                                                            commands::PollResponse::Message(msg) => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            }
                                                        }
                                                    } else if arg == "end" || arg == "stop" || arg == "close" {
                                                        let mut poll = poll_ref.lock().await;
                                                        if let Some(p) = poll.take() {
                                                            let msg = commands::poll_end(&p);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::channel(msg));
                                                        } else {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Aucun sondage en cours.".to_string(), &reply_target, reply_sender_id));
                                                        }
                                                    } else {
                                                        match commands::poll_create(arg, &sender_name) {
                                                            Some((new_poll, announcement)) => {
                                                                let mut poll = poll_ref.lock().await;
                                                                *poll = Some(new_poll);
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::channel(announcement));
                                                            }
                                                            None => {
                                                                let parts_count = arg.split('|').map(|s| s.trim()).filter(|s| !s.is_empty()).count();
                                                                let err = if parts_count > 11 {
                                                                    "❌ Maximum 10 options."
                                                                } else {
                                                                    "❌ Minimum 1 question + 2 options. Format : [b]!poll Question ? | Opt1 | Opt2[/b]"
                                                                };
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(err.to_string(), &reply_target, reply_sender_id));
                                                            }
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!vote") {
                                                    let arg = message.get(5..).unwrap_or("").trim();
                                                    let poll_ref = active_poll.clone();
                                                    let mut poll = poll_ref.lock().await;
                                                    if let Some(ref mut p) = *poll {
                                                        match commands::vote(p, arg, &sender_uid, &sender_name) {
                                                            commands::VoteResult::Voted(msg) => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::channel(msg));
                                                            }
                                                            commands::VoteResult::Error(msg) => {
                                                                let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                            }
                                                        }
                                                    } else {
                                                        let _ = ts3_msg_tx.try_send(OutgoingMessage::reply("❌ Aucun sondage en cours. Crée-en un avec [b]!poll[/b]".to_string(), &reply_target, reply_sender_id));
                                                    }
                                                } else if msg_lower.starts_with("!remind") || msg_lower.starts_with("!rappel") {
                                                    let arg = message.split_once(' ').map(|x| x.1).unwrap_or("").trim();
                                                    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                                                    let reminders_lock = reminders.lock().await;
                                                    let result = commands::remind_command(&reminders_lock, arg, &sender_uid, &sender_name, now_ms);
                                                    match result {
                                                        commands::RemindResult::Response(msg) => {
                                                            drop(reminders_lock);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::RemindResult::Clear { message: msg, .. } => {
                                                            drop(reminders_lock);
                                                            let mut reminders_lock = reminders.lock().await;
                                                            reminders_lock.retain(|r| r.uid != sender_uid);
                                                            save_json_compact("data/reminders.json", &*reminders_lock);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::RemindResult::Add { message: msg, reminder } => {
                                                            drop(reminders_lock);
                                                            let mut reminders_lock = reminders.lock().await;
                                                            reminders_lock.push(reminder);
                                                            save_json_compact("data/reminders.json", &*reminders_lock);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!lang") {
                                                    let arg = message.split_whitespace().nth(1).unwrap_or("");
                                                    match commands::lang_command(arg) {
                                                        commands::LangResult::Reset { message: msg } => {
                                                            let mut overrides = language_overrides.lock().await;
                                                            overrides.remove(&sender_uid);
                                                            let _ = save_language_prefs(&overrides);
                                                            drop(overrides);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::LangResult::Set { message: msg, code } => {
                                                            let mut overrides = language_overrides.lock().await;
                                                            overrides.insert(sender_uid.clone(), code);
                                                            let _ = save_language_prefs(&overrides);
                                                            drop(overrides);
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::LangResult::Invalid { message: msg } => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                    }
                                                } else if msg_lower.starts_with("!tts ") {
                                                    let raw_text = message[5..].trim();
                                                    let valid_voices = valid_voices_for_model(&config.tts_model);
                                                    let parsed = commands::tts_parse_options(raw_text, &valid_voices);
                                                    match commands::tts_validate(&parsed) {
                                                        commands::TtsValidation::Empty(msg) | commands::TtsValidation::TooLong(msg) => {
                                                            let _ = ts3_msg_tx.try_send(OutgoingMessage::reply(msg, &reply_target, reply_sender_id));
                                                        }
                                                        commands::TtsValidation::Ok => {
                                                    let tts_text = parsed.text;
                                                    let tts_voice = parsed.voice;
                                                    let tts_speed = parsed.speed;
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
                                                        // Resolve voice/speed: explicit > runtime default (from !voice/!speed / set_voice/set_speed)
                                                        let tts_voice = tts_voice.or_else(|| Some(default_voice.read().unwrap().clone()));
                                                        let tts_speed = tts_speed.or_else(|| Some(*default_speed.read().unwrap()));
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
                                                        let rt_tts = reply_target;
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
                                                    } // close TtsValidation::Ok
                                                    } // close match tts_validate
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
                                                    let notify_watchers_clone = notify_watchers.clone();
                                                    let mut sender_for_poke = ts3_sender.clone();
                                                    let connect_times_clone = connect_times.clone();
                                                    tokio::spawn(async move {
                                                        let result = sender.with_connection(move |con| {
                                                            con.get_state().ok().and_then(|state| {
                                                                state.clients.get(&client_id).map(|c| {
                                                                    let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                    let cc = if c.country_code.is_empty() { None } else { Some(c.country_code.clone()) };
                                                                    (c.name.clone(), c.channel.0, uid, cc)
                                                                })
                                                            })
                                                        }).await;
                                                        if let Ok(Some((name, channel_id, uid, country_code))) = result {
                                                            info!("Client connected: {} (id: {}, uid: {:?})", name, client_id_u64, uid);
                                                            // Track connect time
                                                            if let Some(ref uid_str) = uid {
                                                                connect_times_clone.lock().await.entry(uid_str.clone()).or_insert_with(std::time::Instant::now);
                                                            }
                                                            let _ = tx.send(WebSocketEvent::ClientConnected {
                                                                client_id: client_id_u64,
                                                                client_name: name.clone(),
                                                                channel_id,
                                                                uid,
                                                                country_code,
                                                            });

                                                            // Check notify watchers
                                                            let name_lower = name.to_lowercase();
                                                            let watchers = notify_watchers_clone.lock().await;
                                                            let mut to_poke: Vec<(String, String)> = Vec::new(); // (requester_name, requester_uid)
                                                            for (target, requesters) in watchers.iter() {
                                                                if name_lower.contains(target) {
                                                                    for r in requesters {
                                                                        to_poke.push(r.clone());
                                                                    }
                                                                }
                                                            }
                                                            drop(watchers);

                                                            if !to_poke.is_empty() {
                                                                // Find requester client IDs from current state
                                                                let poke_targets = sender_for_poke.with_connection(move |con| {
                                                                    con.get_state().ok().map(|state| {
                                                                        let mut targets = Vec::new();
                                                                        for client in state.clients.values() {
                                                                            let cuid = client.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                            if let Some(ref cuid_str) = cuid {
                                                                                for (_, req_uid) in &to_poke {
                                                                                    if cuid_str == req_uid {
                                                                                        targets.push(client.id.0);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                        targets
                                                                    }).unwrap_or_default()
                                                                }).await;

                                                                if let Ok(targets) = poke_targets {
                                                                    for clid in targets {
                                                                        use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
                                                                        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "clientpoke");
                                                                        cmd.write_arg("clid", &clid);
                                                                        let msg = format!("🔔 {} vient de se connecter !", name);
                                                                        cmd.write_arg("msg", &msg);
                                                                        if let Err(e) = sender_for_poke.send_command(cmd).await {
                                                                            warn!("Failed to poke for notify: {:?}", e);
                                                                        } else {
                                                                            info!("Notified (poke) about {} connecting", name);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                            Event::PropertyRemoved { id, old, .. } => {
                                                if let (PropertyId::Client(client_id), PropertyValue::Client(client)) = (id, old) {
                                                    let uid = client.uid.as_ref().map(|u| base64::encode(&u.0));
                                                    let client_id_u64 = client_id.0 as u64;
                                                    info!("Client disconnected: {} (id: {}, uid: {:?})", client.name, client_id_u64, uid);
                                                    // Remove connect time tracking
                                                    if let Some(ref uid_str) = uid {
                                                        connect_times.lock().await.remove(uid_str);
                                                    }
                                                    // Record last-seen time
                                                    if let Some(ref uid_str) = uid {
                                                        let mut seen = seen_data.lock().await;
                                                        seen.insert(uid_str.clone(), (client.name.clone(), chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string()));
                                                        save_json("data/seen.json", &*seen);
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
                                                    let old_channel_id = old_channel.0;
                                                    let client_id_u64 = client_id.0 as u64;
                                                    let mut sender = ts3_sender.clone();
                                                    let tx = event_tx_clone.clone();
                                                    let bm_for_move = buffer_manager.clone();
                                                    let greet_enabled_move = greet_enabled.clone();
                                                    let greet_cooldowns_move = greet_cooldowns.clone();
                                                    let greet_msg_tx = ts3_msg_tx.clone();
                                                    let bot_stats_greet = bot_stats.clone();
                                                    tokio::spawn(async move {
                                                        let result = sender.with_connection(move |con| {
                                                            con.get_state().ok().and_then(|state| {
                                                                let is_self = state.own_client == client_id;
                                                                let bot_channel = state.clients.get(&state.own_client).map(|c| c.channel.0);
                                                                state.clients.get(&client_id).map(|c| {
                                                                    let uid = c.uid.as_ref().map(|u| base64::encode(&u.0));
                                                                    let old_ch_name = state.channels.get(&tsclientlib::ChannelId(old_channel.0)).map(|ch| ch.name.clone());
                                                                    let new_ch_name = state.channels.get(&c.channel).map(|ch| ch.name.clone());
                                                                    (c.name.clone(), c.channel.0, is_self, uid, old_ch_name, new_ch_name, bot_channel)
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
                                                                            bot_stats_greet.inc_greetings();
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

                // Clear the shared TS3 handle (connection is dead)
                {
                    let mut handle = shared_ts3_handle.lock().await;
                    *handle = None;
                }

                if shutting_down {
                    info!("Graceful shutdown complete, exiting reconnect loop");
                    break 'reconnect;
                }

                // Unexpected disconnect — reconnect with backoff
                reconnect_count += 1;
                let delay_ms = (initial_delay_ms * 2u64.saturating_pow(reconnect_count.min(10) - 1)).min(max_delay_ms);
                warn!("⚡ TS3 connection lost unexpectedly. Reconnecting in {}ms (attempt #{})...", delay_ms, reconnect_count);
                // Wait for old clone to timeout on TS3 server
                let wait_ms = delay_ms.max(35_000); // at least 35s to avoid clone conflicts
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

        } // close connection block
        } // close 'reconnect loop
    });

    // Spawn WebSocket server task (with TTS channel if enabled)
    let tts_tx_for_ws = if tts_enabled { Some(tts_tx.clone()) } else { None };
    let tts_stop_flag_for_ws = tts_stop_flag.clone();
    let ws_handle = tokio::spawn(async move {
        let ws_params = websocket::WebSocketServerParams {
            tts_tx: tts_tx_for_ws,
            ts3_handle: shared_ts3_handle_for_ws,
            tts_stop_flag: tts_stop_flag_for_ws,
            buffer_manager: buffer_manager_for_ws,
            language_overrides: language_overrides_for_ws,
            tts_volume: tts_volume_for_ws,
            default_voice: default_voice_for_ws,
            default_speed: default_speed_for_ws,
            chat_history: chat_history_for_ws,
        };
        if let Err(e) = websocket::run_server(ws_config, event_tx, ws_params).await {
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
