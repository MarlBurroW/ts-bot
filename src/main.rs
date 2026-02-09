mod ts3;

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
    OpusDecoder,
    SpeakerBufferManager,
    TriggerWordPipeline,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// SyncConnection for bidirectional communication
use tsclientlib::prelude::*;
use tsclientlib::sync::SyncConnection;
use ts_bookkeeping::{MessageTarget, DisconnectOptions, Reason};

/// Results from background whisper tasks
enum WhisperResult {
    WakeWordCheck { speaker_id: u64, detected: bool, text: String },
    Transcription {
        speaker_id: u64,
        speaker_name: String,
        speaker_uid: String,
        text: String,
        command: Option<String>,
        audio_len: usize,
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

    // Spawn TS3 client connection task
    let mut ts3_handle = tokio::spawn(async move {
        info!("Starting TS3 client connection");

        // Initialize audio processing components
        info!("Initializing audio processing components");

        // TODO: Make model path configurable via BotConfig
        let whisper_model_path = "models/ggml-small.bin";
        let pipeline = match TriggerWordPipeline::new(whisper_model_path, "marlbot") {
            Ok(p) => {
                info!("TriggerWordPipeline initialized successfully");
                Some(Arc::new(Mutex::new(p)))
            }
            Err(e) => {
                warn!("Failed to initialize TriggerWordPipeline: {}. Audio transcription disabled.", e);
                warn!("To enable audio transcription:");
                warn!("  1. Create a 'models' directory in the project root");
                warn!("  2. Download a Whisper model (e.g., ggml-small.bin) to models/");
                warn!("  3. Download from: https://huggingface.co/ggerganov/whisper.cpp/tree/main");
                None
            }
        };

        let decoder = match OpusDecoder::new() {
            Ok(d) => Arc::new(Mutex::new(d)),
            Err(e) => {
                error!("Failed to initialize Opus decoder: {}. Audio will not work.", e);
                return;
            }
        };

        let buffer_manager = Arc::new(Mutex::new(SpeakerBufferManager::new()));

        // Attempt initial connection
        match ts3_client.connect().await {
            Ok(connection) => {
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

                // Channel for queuing outgoing TS3 chat messages
                let (ts3_msg_tx, mut ts3_msg_rx) = tokio::sync::mpsc::channel::<String>(10);

                // Channel for receiving whisper results from background tasks
                let (whisper_tx, mut whisper_rx) = tokio::sync::mpsc::channel::<WhisperResult>(10);

                // Track if a whisper task is currently running (to avoid stacking)
                let whisper_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));

                // Shared cache: speaker_id -> (name, uid) resolved from TS3 connection state
                let client_names: Arc<std::sync::RwLock<HashMap<u64, (String, String)>>> =
                    Arc::new(std::sync::RwLock::new(HashMap::new()));

                // Spawn task to send TS3 messages via the SyncConnection handle
                let mut sender_clone = ts3_sender.clone();
                tokio::spawn(async move {
                    while let Some(msg) = ts3_msg_rx.recv().await {
                        let msg_clone = msg.clone();
                        match sender_clone.with_connection(move |con| {
                            if let Ok(state) = con.get_state() {
                                let _ = state.send_message(
                                    MessageTarget::Channel,
                                    &msg_clone
                                ).send(con);
                            }
                        }).await {
                            Ok(_) => info!("TS3 chat: {}", msg),
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
                    Some(Arc::new(AudioPlayer::new(
                        ts3_sender.clone(),
                        event_tx_clone.clone(),
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

                // Spawn TTS processing task (receives requests from WebSocket)
                if let (Some(ref player), Some(ref synth)) = (&audio_player, &tts_synth) {
                    let player_clone = player.clone();
                    let synth_clone = synth.clone();
                    tokio::spawn(async move {
                        while let Some(request) = tts_rx.recv().await {
                            info!("TTS request: '{}'", request.text);
                            if let Err(e) = player_clone
                                .speak(request.text, request.voice, synth_clone.clone())
                                .await
                            {
                                warn!("TTS speak failed: {}", e);
                            }
                        }
                    });
                }

                // Process events from the SyncConnection stream
                tokio::pin!(sync_con);
                let mut silence_check_interval = tokio::time::interval(std::time::Duration::from_millis(500));
                let mut shutting_down = false;

                loop {
                    tokio::select! {
                        // Process results from background whisper tasks
                        Some(result) = whisper_rx.recv() => {
                            match result {
                                WhisperResult::WakeWordCheck { speaker_id, detected, text } => {
                                    whisper_busy.store(false, std::sync::atomic::Ordering::Relaxed);

                                    if text.is_empty() {
                                        info!("Wake word check for speaker {}: (silence/empty)", speaker_id);
                                    } else if detected {
                                        info!("Wake word detected from speaker {}!", speaker_id);

                                        // Interrupt TTS playback if bot is speaking
                                        if let Some(ref player) = audio_player {
                                            if player.is_speaking() {
                                                info!("Interrupting TTS playback (wake word)");
                                                player.stop();
                                            }
                                        }

                                        let mut bm = buffer_manager.lock().await;
                                        if let Some(buf) = bm.get_buffer_mut(speaker_id) {
                                            buf.activate();
                                            // Keep last 3s of audio (may contain command after wake word)
                                            buf.trim_to_recent(std::time::Duration::from_secs(3));
                                            let name = buf.speaker_name.clone();
                                            drop(bm);
                                            let _ = ts3_msg_tx.try_send(
                                                format!("J'ecoute, {} ?", name)
                                            );
                                        }
                                    } else {
                                        info!("Wake word check for speaker {}: '{}'", speaker_id, text);
                                    }
                                }
                                WhisperResult::Transcription { speaker_id, speaker_name, speaker_uid, text, command, audio_len } => {
                                    whisper_busy.store(false, std::sync::atomic::Ordering::Relaxed);

                                    // Use the pre-extracted command from pipeline, or fall back to raw text
                                    let command_text = command.unwrap_or_else(|| text.clone());

                                    if !command_text.trim().is_empty() {
                                        info!("Transcription from {}: '{}' (raw: '{}')", speaker_name, command_text, text);

                                        // Send transcription to TS3 chat
                                        let _ = ts3_msg_tx.try_send(
                                            format!("{}: {}", speaker_name, command_text)
                                        );

                                        let transcription_event = TranscriptionEvent {
                                            timestamp: chrono::Utc::now(),
                                            speaker_id,
                                            speaker_uid,
                                            speaker_name,
                                            text: command_text,
                                            confidence: None,
                                            language: Some("fr".to_string()),
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
                            if let Some(ref pipeline_arc) = pipeline {
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
                                            format!("Arret de l'ecoute, {}.", speaker_name)
                                        );

                                        if !full_audio.is_empty() {
                                            // Spawn transcription via pipeline in background
                                            // blocking_lock() will wait if Whisper is busy with another task
                                            let pipeline_clone = pipeline_arc.clone();
                                            let whisper_tx_clone = whisper_tx.clone();
                                            whisper_busy.store(true, std::sync::atomic::Ordering::Relaxed);

                                            tokio::task::spawn_blocking(move || {
                                                let audio_len = full_audio.len();
                                                let mut lock = pipeline_clone.blocking_lock();
                                                let result = lock.transcribe_and_detect(&full_audio);
                                                drop(lock);

                                                match result {
                                                    Ok(det) => {
                                                        let _ = whisper_tx_clone.blocking_send(WhisperResult::Transcription {
                                                            speaker_id,
                                                            speaker_name,
                                                            speaker_uid,
                                                            text: det.transcription,
                                                            command: det.command,
                                                            audio_len,
                                                        });
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("Pipeline transcription failed: {}", e);
                                                        let _ = whisper_tx_clone.blocking_send(WhisperResult::Transcription {
                                                            speaker_id,
                                                            speaker_name,
                                                            speaker_uid,
                                                            text: String::new(),
                                                            command: None,
                                                            audio_len,
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
                                                    sender_uid,
                                                    sender_name: invoker.name.to_string(),
                                                    content: message.to_string(),
                                                    channel_id,
                                                    channel_name: None,
                                                    timestamp: chrono::Utc::now(),
                                                };

                                                let ws_event = WebSocketEvent::message_received(msg_event);
                                                if let Err(e) = event_tx_clone.send(ws_event) {
                                                    warn!("Failed to broadcast message event: {}", e);
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
                                                                    (c.name.clone(), c.channel.0 as u64)
                                                                })
                                                            })
                                                        }).await;
                                                        if let Ok(Some((name, channel_id))) = result {
                                                            info!("Client connected: {} (id: {})", name, client_id_u64);
                                                            let _ = tx.send(WebSocketEvent::ClientConnected {
                                                                client_id: client_id_u64,
                                                                client_name: name,
                                                                channel_id,
                                                            });
                                                        }
                                                    });
                                                }
                                            }
                                            Event::PropertyRemoved { id, old, .. } => {
                                                if let (PropertyId::Client(client_id), PropertyValue::Client(client)) = (id, old) {
                                                    info!("Client disconnected: {} (id: {})", client.name, client_id.0);
                                                    let _ = event_tx_clone.send(WebSocketEvent::ClientDisconnected {
                                                        client_id: client_id.0 as u64,
                                                        client_name: client.name.clone(),
                                                    });
                                                }
                                            }
                                            Event::PropertyChanged { id, old, .. } => {
                                                if let (PropertyId::ClientChannel(client_id), PropertyValue::ChannelId(old_channel)) = (id, old) {
                                                    let old_channel_id = old_channel.0 as u64;
                                                    let client_id_u64 = client_id.0 as u64;
                                                    let mut sender = ts3_sender.clone();
                                                    let tx = event_tx_clone.clone();
                                                    tokio::spawn(async move {
                                                        let result = sender.with_connection(move |con| {
                                                            con.get_state().ok().and_then(|state| {
                                                                let is_self = state.own_client == client_id;
                                                                state.clients.get(&client_id).map(|c| {
                                                                    (c.name.clone(), c.channel.0 as u64, is_self)
                                                                })
                                                            })
                                                        }).await;
                                                        if let Ok(Some((name, new_channel_id, is_self))) = result {
                                                            info!("Client moved: {} ({} -> {})", name, old_channel_id, new_channel_id);
                                                            let _ = tx.send(WebSocketEvent::ClientMoved {
                                                                client_id: client_id_u64,
                                                                client_name: name,
                                                                old_channel_id,
                                                                new_channel_id,
                                                            });
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
                                    if let Some(ref pipeline_arc) = pipeline {
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

                                        // Decode Opus audio to PCM
                                        let mut decoder_lock = decoder.lock().await;
                                        let pcm_samples = match decoder_lock.decode(codec_data) {
                                            Ok(samples) => samples,
                                            Err(e) => {
                                                debug!("Failed to decode audio from speaker {}: {}", speaker_id, e);
                                                continue;
                                            }
                                        };
                                        drop(decoder_lock);

                                        // Resample to 16kHz for Whisper
                                        let samples_f32 = OpusDecoder::resample_to_16khz(&pcm_samples);

                                        // Resolve real TS3 client name from cache
                                        let (speaker_name, speaker_uid) = {
                                            let names = client_names.read().unwrap_or_else(|e| e.into_inner());
                                            names.get(&speaker_id)
                                                .cloned()
                                                .unwrap_or_else(|| (format!("Speaker_{}", speaker_id), "unknown".to_string()))
                                        };

                                        // Add to speaker's buffer
                                        let mut bm = buffer_manager.lock().await;
                                        let buffer = bm.get_or_create_buffer(
                                            speaker_id,
                                            speaker_name.clone(),
                                            speaker_uid.clone(),
                                        );
                                        // Update name in case cache was populated after buffer creation
                                        buffer.speaker_name = speaker_name;
                                        buffer.speaker_uid = speaker_uid;
                                        buffer.push_samples(&samples_f32);

                                        let is_active = buffer.is_active;
                                        let wake_check_interval = std::time::Duration::from_millis(1500);

                                        if !is_active {
                                            // Only check wake word if not already busy with whisper
                                            let is_busy = whisper_busy.load(std::sync::atomic::Ordering::Relaxed);
                                            if !is_busy && buffer.should_check_wake_word(wake_check_interval) {
                                                buffer.mark_wake_check();
                                                let recent_audio = buffer.get_recent_samples(std::time::Duration::from_secs(3));
                                                drop(bm);

                                                // Spawn wake word check via pipeline in background
                                                let pipeline_clone = pipeline_arc.clone();
                                                let whisper_tx_clone = whisper_tx.clone();
                                                whisper_busy.store(true, std::sync::atomic::Ordering::Relaxed);

                                                tokio::task::spawn_blocking(move || {
                                                    let mut lock = pipeline_clone.blocking_lock();
                                                    let (detected, text) = lock.check_wake_word(&recent_audio)
                                                        .unwrap_or_default();
                                                    drop(lock);
                                                    let _ = whisper_tx_clone.blocking_send(WhisperResult::WakeWordCheck {
                                                        speaker_id,
                                                        detected,
                                                        text,
                                                    });
                                                });
                                            }
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
            }
            Err(e) => {
                error!("Failed to connect to TS3 server: {}", e);
            }
        }
    });

    // Spawn WebSocket server task (with TTS channel if enabled)
    let tts_tx_for_ws = if tts_enabled { Some(tts_tx.clone()) } else { None };
    let ws_handle = tokio::spawn(async move {
        if let Err(e) = websocket::run_server(ws_config, event_tx, tts_tx_for_ws, shared_ts3_handle_for_ws).await {
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
