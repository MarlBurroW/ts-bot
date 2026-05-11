use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::broadcast};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};

use crate::audio::{LiveAudioStream, MicInState};
use crate::audio::mic_in;
use crate::models::{BotConfig, SharedChatHistory, WebSocketCommand, WebSocketEvent};
use crate::websocket::handlers::{handle_command, CommandAction};
use crate::audio::buffer::SpeakerBufferManager;
use crate::utils::{save_language_prefs, save_bot_state_field};

/// Shared TS3 connection handle, set once connected.
/// `None` if TS3 is not yet connected.
pub type SharedTs3Handle = Arc<tokio::sync::Mutex<Option<tsclientlib::sync::SyncConnectionHandle>>>;

/// TTS request forwarded from WebSocket to the TTS processing task
#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    pub voice: Option<String>,
    pub speed: Option<f32>,
}

/// Shared state for WebSocket server
#[derive(Clone)]
struct AppState {
    event_broadcaster: Arc<broadcast::Sender<WebSocketEvent>>,
    tts_tx: Option<tokio::sync::mpsc::Sender<TtsRequest>>,
    /// Shared TS3 connection handle for querying server state
    ts3_handle: SharedTs3Handle,
    /// Bot nickname (from config)
    bot_nickname: String,
    /// TS3 server address (from config)
    ts3_server: String,
    /// Shared flag to stop TTS playback remotely
    tts_stop_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Shared buffer manager for activating speaker listening
    buffer_manager: Option<Arc<tokio::sync::Mutex<SpeakerBufferManager>>>,
    /// Per-speaker Whisper language overrides (UID -> lang code)
    language_overrides: Option<Arc<tokio::sync::Mutex<HashMap<String, String>>>>,
    /// Shared TTS volume level (0-200, 100 = normal)
    tts_volume: Option<Arc<std::sync::atomic::AtomicU8>>,
    /// Shared default TTS voice (runtime-adjustable)
    default_voice: Option<Arc<std::sync::RwLock<String>>>,
    /// Shared default TTS speed (runtime-adjustable, 0.25-4.0)
    default_speed: Option<Arc<std::sync::RwLock<f32>>>,
    /// Shared chat history ring buffer (timestamp, author, text)
    chat_history: Option<SharedChatHistory>,
    /// TTS model name (for voice validation)
    tts_model: String,
    /// All valid voice names (from registry if available)
    all_valid_voices: Vec<String>,
    /// Live audio HTTP broadcaster (WebM/Opus mono mix)
    live_audio: Option<LiveAudioStream>,
    /// Incoming microphone (push-to-talk) shared state
    mic_in_state: MicInState,
}

/// Bundled parameters for `run_server`, avoiding a long argument list.
pub struct WebSocketServerParams {
    /// Optional TTS request channel (None if TTS disabled)
    pub tts_tx: Option<tokio::sync::mpsc::Sender<TtsRequest>>,
    /// Shared TS3 connection handle (populated after TS3 connects)
    pub ts3_handle: SharedTs3Handle,
    /// Shared flag to stop TTS playback remotely
    pub tts_stop_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Shared buffer manager for activating speaker listening
    pub buffer_manager: Option<Arc<tokio::sync::Mutex<SpeakerBufferManager>>>,
    /// Per-speaker Whisper language overrides (UID -> lang code)
    pub language_overrides: Option<Arc<tokio::sync::Mutex<HashMap<String, String>>>>,
    /// Shared TTS volume level (0-200, 100 = normal)
    pub tts_volume: Option<Arc<std::sync::atomic::AtomicU8>>,
    /// Shared default TTS voice (runtime-adjustable)
    pub default_voice: Option<Arc<std::sync::RwLock<String>>>,
    /// Shared default TTS speed (runtime-adjustable, 0.25-4.0)
    pub default_speed: Option<Arc<std::sync::RwLock<f32>>>,
    /// Shared chat history ring buffer
    pub chat_history: Option<SharedChatHistory>,
    /// All valid voice names (from registry)
    pub all_valid_voices: Option<Vec<String>>,
    /// Live audio HTTP broadcaster (WebM/Opus mono mix)
    pub live_audio: Option<LiveAudioStream>,
}

/// Helper: lock TS3 handle, build an OutCommand via closure, send it, and broadcast success/error.
/// Returns true if the command was sent successfully.
async fn send_ts3_cmd(
    ts3_handle: &SharedTs3Handle,
    event_tx: &Arc<broadcast::Sender<WebSocketEvent>>,
    command_id: Option<String>,
    build_cmd: impl FnOnce(&mut OutCommand),
    ts3_command: &str,
    success_msg: String,
    error_prefix: &str,
) {
    let mut handle_guard = ts3_handle.lock().await;
    if let Some(ref mut sender) = *handle_guard {
        let mut cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            ts3_command,
        );
        build_cmd(&mut cmd);
        match sender.send_command(cmd).await {
            Ok(()) => {
                info!("{}", success_msg);
                let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(success_msg)));
            }
            Err(e) => {
                let _ = event_tx.send(WebSocketEvent::command_error(
                    command_id,
                    format!("{}: {:?}", error_prefix, e),
                ));
            }
        }
    } else {
        let _ = event_tx.send(WebSocketEvent::command_error(command_id, "TS3 not connected".to_string()));
    }
}

/// Run the WebSocket server
pub async fn run_server(
    config: BotConfig,
    event_broadcaster: broadcast::Sender<WebSocketEvent>,
    params: WebSocketServerParams,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.ws_host, config.ws_port);
    let socket_addr: SocketAddr = addr.parse()?;

    // Create shared state
    let state = AppState {
        event_broadcaster: Arc::new(event_broadcaster),
        tts_tx: params.tts_tx,
        ts3_handle: params.ts3_handle,
        bot_nickname: config.ts3_nickname.clone(),
        ts3_server: config.ts3_server.clone(),
        tts_stop_flag: params.tts_stop_flag.clone(),
        buffer_manager: params.buffer_manager,
        language_overrides: params.language_overrides,
        tts_volume: params.tts_volume,
        default_voice: params.default_voice,
        default_speed: params.default_speed,
        chat_history: params.chat_history,
        tts_model: config.tts_model.clone(),
        all_valid_voices: params.all_valid_voices.unwrap_or_default(),
        live_audio: params.live_audio,
        mic_in_state: MicInState::new(params.tts_stop_flag.clone()),
    };

    // CORS: allow the mini-app (running in a browser, different origin from
    // the loopback bot) to hit `/audio/live`. Start permissive; can be
    // tightened to a specific origin allowlist later.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::HEAD, Method::OPTIONS, Method::POST])
        .allow_headers(Any);

    // Create Axum router with WebSocket + live audio HTTP endpoints
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/audio/live", get(audio_live_handler))
        .route("/audio/in", post(audio_in_handler))
        .layer(cors)
        .with_state(state);

    info!("WebSocket server listening on {}", addr);

    // Bind and serve
    let listener = TcpListener::bind(&socket_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// WebSocket upgrade handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// HTTP GET /audio/live — streams the live WebM/Opus mono mix of all speakers
/// currently audible to the bot. The encoding pipeline (ffmpeg) is only
/// spawned while at least one listener is connected; idle CPU cost is zero.
async fn audio_live_handler(State(state): State<AppState>) -> Response {
    let Some(live) = state.live_audio.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "live audio disabled").into_response();
    };

    let rx = live.subscribe();
    let init = live.init_segment();
    info!(
        "Live audio: new HTTP listener subscribed (total: {}, init_cached: {})",
        live.subscriber_count(),
        init.is_some()
    );

    // Late subscribers (everyone after the first) need the WebM init segment
    // in front of the first cluster they receive, otherwise the browser
    // rejects the stream as "EBML header parsing failed". Prepend the cached
    // init bytes (if any) before bridging to the live broadcast.
    let init_stream = futures_util::stream::iter(match init {
        Some(b) if !b.is_empty() => vec![Ok::<_, std::io::Error>(b)],
        _ => Vec::new(),
    });
    let bcast_stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            Ok(chunk) => Some(Ok::<_, std::io::Error>(chunk)),
            // Lagged: skip the missed chunks but keep streaming.
            Err(_) => None,
        }
    });
    let stream = init_stream.chain(bcast_stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/webm"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));

    (headers, Body::from_stream(stream)).into_response()
}

/// HTTP POST /audio/in — accepts a chunked audio/webm body from the browser
/// and forwards it (decoded → re-encoded as 20 ms Opus frames) to TS3 as
/// AudioData::C2S. Symmetric to /audio/live. Used by the mini-app's
/// push-to-talk button. Only one session is active at a time; a new request
/// aborts any in-flight one. Also interrupts any ongoing TTS playback.
async fn audio_in_handler(State(state): State<AppState>, body: Body) -> Response {
    let stream = body.into_data_stream();
    match mic_in::run_session(stream, state.mic_in_state.clone(), state.ts3_handle.clone()).await {
        Ok(frames) => (StatusCode::OK, format!("{} frames sent", frames)).into_response(),
        Err(e) => {
            warn!("Mic-in handler error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("mic-in error: {}", e)).into_response()
        }
    }
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState) {
    info!("WebSocket client connected");

    // Split socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to event broadcasts
    let mut event_rx = state.event_broadcaster.subscribe();

    // Send welcome message with real TS3 connection state
    let connection_status = if state.ts3_handle.lock().await.is_some() {
        "connected".to_string()
    } else {
        "disconnected".to_string()
    };
    let welcome = WebSocketEvent::welcome(
        state.bot_nickname.clone(),
        state.ts3_server.clone(),
        connection_status,
    );

    if let Ok(json) = serde_json::to_string(&welcome) {
        if let Err(e) = sender.send(Message::Text(json)).await {
            warn!("Failed to send welcome message: {}", e);
            return;
        }
    }

    // Spawn task to forward broadcast events to WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            // Serialize event to JSON
            let json = match serde_json::to_string(&event) {
                Ok(json) => json,
                Err(e) => {
                    error!("Failed to serialize event: {}", e);
                    continue;
                }
            };

            // Send to WebSocket client
            if sender.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages from WebSocket
    let event_tx = state.event_broadcaster.clone();
    let tts_tx = state.tts_tx.clone();
    let tts_stop_flag = state.tts_stop_flag.clone();
    let tts_volume = state.tts_volume.clone();
    let default_voice = state.default_voice.clone();
    let default_speed = state.default_speed.clone();
    let chat_history = state.chat_history.clone();
    let ts3_handle = state.ts3_handle.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    info!("Received command from WebSocket: {}", text);

                    // Parse command
                    let command: WebSocketCommand = match serde_json::from_str(&text) {
                        Ok(cmd) => cmd,
                        Err(e) => {
                            warn!("Failed to parse WebSocket command: {}", e);
                            let err = WebSocketEvent::command_error(
                                None,
                                format!("Invalid command: {}", e),
                            );
                            let _ = event_tx.send(err);
                            continue;
                        }
                    };

                    // Handle command
                    let (response, action) = handle_command(command);

                    // Send response via broadcast
                    let _ = event_tx.send(response);

                    // Execute side effects
                    match action {
                        CommandAction::None => {}
                        CommandAction::Speak { text, voice, speed } => {
                            if let Some(ref tx) = tts_tx {
                                if let Err(e) = tx.send(TtsRequest { text, voice, speed }).await {
                                    warn!("Failed to queue TTS request: {}", e);
                                    let _ = event_tx.send(WebSocketEvent::command_error(
                                        None,
                                        "TTS pipeline not available".to_string(),
                                    ));
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    None,
                                    "TTS is disabled".to_string(),
                                ));
                            }
                        }
                        CommandAction::StopSpeaking => {
                            if let Some(ref flag) = tts_stop_flag {
                                let flag: &std::sync::atomic::AtomicBool = flag.as_ref();
                                flag.store(false, std::sync::atomic::Ordering::Relaxed);
                                info!("TTS playback stopped via WebSocket command");
                            }
                        }
                        CommandAction::SetVolume { command_id, volume } => {
                            if let Some(ref vol) = tts_volume {
                                vol.store(volume.min(200), std::sync::atomic::Ordering::Relaxed);
                                info!("TTS volume set to {}% via WebSocket", volume.min(200));
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(format!("Volume set to {}%", volume.min(200))),
                                ));
                            }
                        }
                        CommandAction::GetVolume { command_id } => {
                            let vol = tts_volume.as_ref()
                                .map(|v| v.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(100);
                            let _ = event_tx.send(WebSocketEvent::command_success(
                                command_id,
                                Some(serde_json::json!({ "volume": vol }).to_string()),
                            ));
                        }
                        CommandAction::SetVoice { command_id, voice } => {
                            let valid = if !state.all_valid_voices.is_empty() {
                                state.all_valid_voices.clone()
                            } else {
                                crate::utils::valid_voices_for_model(&state.tts_model)
                            };
                            if !valid.contains(&voice.to_lowercase()) {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    command_id,
                                    format!("Invalid voice '{}'. Valid: {}", voice, valid.join(", ")),
                                ));
                            } else if let Some(ref dv) = default_voice {
                                let voice_lower = voice.to_lowercase();
                                *dv.write().unwrap() = voice_lower.clone();
                                save_bot_state_field("voice", &voice_lower);
                                info!("Default TTS voice set to '{}' via WebSocket", voice_lower);
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(format!("Default voice set to '{}'", voice_lower)),
                                ));
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    command_id,
                                    "TTS not enabled".to_string(),
                                ));
                            }
                        }
                        CommandAction::GetVoice { command_id } => {
                            let voice = default_voice.as_ref()
                                .map(|dv| dv.read().unwrap().clone())
                                .unwrap_or_else(|| "onyx".to_string());
                            let _ = event_tx.send(WebSocketEvent::command_success(
                                command_id,
                                Some(serde_json::json!({ "voice": voice }).to_string()),
                            ));
                        }
                        CommandAction::SetSpeed { command_id, speed } => {
                            if let Some(ref ds) = default_speed {
                                *ds.write().unwrap() = speed;
                                save_bot_state_field("speed", &speed);
                                info!("Default TTS speed set to {:.2} via WebSocket", speed);
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(format!("Default speed set to {:.2}", speed)),
                                ));
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    command_id,
                                    "TTS not enabled".to_string(),
                                ));
                            }
                        }
                        CommandAction::GetSpeed { command_id } => {
                            let speed: f32 = default_speed.as_ref()
                                .map(|ds| *ds.read().unwrap())
                                .unwrap_or(1.15);
                            let _ = event_tx.send(WebSocketEvent::command_success(
                                command_id,
                                Some(serde_json::json!({ "speed": speed }).to_string()),
                            ));
                        }
                        CommandAction::GetHistory { command_id, count } => {
                            if let Some(ref hist) = chat_history {
                                let history: tokio::sync::MutexGuard<'_, std::collections::VecDeque<(String, String, String)>> = hist.lock().await;
                                let entries: Vec<serde_json::Value> = history.iter()
                                    .rev()
                                    .take(count as usize)
                                    .map(|(ts, author, text)| {
                                        serde_json::json!({
                                            "timestamp": ts,
                                            "author": author,
                                            "text": text
                                        })
                                    })
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect();
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(serde_json::json!({
                                        "history": entries,
                                        "count": entries.len()
                                    }).to_string()),
                                ));
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    command_id,
                                    "Chat history not available".to_string(),
                                ));
                            }
                        }
                        CommandAction::SetTimeout { command_id, timeout_ms } => {
                            if let Some(ref bm) = state.buffer_manager {
                                let mut bm = bm.lock().await;
                                bm.set_silence_timeout_ms(timeout_ms);
                                info!("Silence timeout set to {}ms via WebSocket", timeout_ms);
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(format!("Silence timeout set to {}ms", timeout_ms)),
                                ));
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(
                                    command_id,
                                    "Buffer manager not available".to_string(),
                                ));
                            }
                        }
                        CommandAction::GetTimeout { command_id } => {
                            if let Some(ref bm) = state.buffer_manager {
                                let bm = bm.lock().await;
                                let timeout = bm.silence_timeout_ms();
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(serde_json::json!({ "timeout_ms": timeout }).to_string()),
                                ));
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_success(
                                    command_id,
                                    Some(serde_json::json!({ "timeout_ms": 2000 }).to_string()),
                                ));
                            }
                        }
                        CommandAction::GetServerState { command_id } => {
                            let mut handle_guard = ts3_handle.lock().await;
                            if let Some(ref mut sender) = *handle_guard {
                                match sender.with_connection(|con| {
                                    con.get_state().ok().map(|state| {
                                        let own_client_id = state.own_client.0;
                                        let channels: Vec<serde_json::Value> = state.channels.iter().map(|(id, ch)| {
                                            let mut ch_json = serde_json::json!({
                                                "id": id.0,
                                                "name": ch.name,
                                                "parent_id": ch.parent.0,
                                                "order": ch.order.0,
                                                "codec": format!("{:?}", ch.codec),
                                                "forced_silence": ch.forced_silence,
                                                "subscribed": ch.subscribed
                                            });
                                            if let Some(ref topic) = ch.topic {
                                                ch_json["topic"] = serde_json::json!(topic);
                                            }
                                            if let Some(ref max_clients) = ch.max_clients {
                                                ch_json["max_clients"] = serde_json::json!(format!("{:?}", max_clients));
                                            }
                                            if let Some(has_pw) = ch.has_password {
                                                ch_json["has_password"] = serde_json::json!(has_pw);
                                            }
                                            if let Some(talk_power) = ch.needed_talk_power {
                                                ch_json["needed_talk_power"] = serde_json::json!(talk_power);
                                            }
                                            if let Some(ref codec_quality) = ch.codec_quality {
                                                ch_json["codec_quality"] = serde_json::json!(codec_quality);
                                            }
                                            ch_json
                                        }).collect();
                                        let clients: Vec<serde_json::Value> = state.clients.iter().map(|(id, cl)| {
                                            let mut client_json = serde_json::json!({
                                                "id": id.0,
                                                "name": cl.name,
                                                "channel_id": cl.channel.0,
                                                "input_muted": cl.input_muted,
                                                "output_muted": cl.output_muted,
                                                "is_recording": cl.is_recording,
                                                "talk_power": cl.talk_power,
                                                "is_priority_speaker": cl.is_priority_speaker
                                            });
                                            if let Some(ref uid) = cl.uid {
                                                client_json["uid"] = serde_json::json!(uid.0);
                                            }
                                            if let Some(ref away_msg) = cl.away_message {
                                                client_json["away_message"] = serde_json::json!(away_msg);
                                            }
                                            if !cl.description.is_empty() {
                                                client_json["description"] = serde_json::json!(cl.description);
                                            }
                                            client_json["database_id"] = serde_json::json!(cl.database_id.0);
                                            if !cl.country_code.is_empty() {
                                                client_json["country_code"] = serde_json::json!(cl.country_code);
                                            }
                                            client_json["channel_group"] = serde_json::json!(cl.channel_group.0);
                                            let sg: Vec<u64> = cl.server_groups.iter().map(|g| g.0).collect();
                                            if !sg.is_empty() {
                                                client_json["server_groups"] = serde_json::json!(sg);
                                            }
                                            client_json["is_channel_commander"] = serde_json::json!(cl.is_channel_commander);
                                            if cl.talk_power_granted {
                                                client_json["talk_power_granted"] = serde_json::json!(true);
                                            }
                                            client_json
                                        }).collect();
                                        serde_json::json!({
                                            "own_client_id": own_client_id,
                                            "channels": channels,
                                            "clients": clients
                                        })
                                    })
                                }).await {
                                    Ok(Some(data)) => {
                                        let _ = event_tx.send(WebSocketEvent::command_success_with_data(command_id, data));
                                    }
                                    Ok(None) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Failed to read TS3 state".to_string()));
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("TS3 connection error: {:?}", e)));
                                    }
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "TS3 not connected".to_string()));
                            }
                        }
                        CommandAction::SendMessage { command_id, target, content, client_id, tts: speak_tts } => {
                            let mut handle_guard = ts3_handle.lock().await;
                            if let Some(ref mut sender) = *handle_guard {
                                let msg_target = if target == "private" {
                                    if let Some(cid) = client_id {
                                        tsclientlib::MessageTarget::Client(tsclientlib::ClientId(cid as u16))
                                    } else {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Private message requires recipient client_id".to_string()));
                                        continue;
                                    }
                                } else {
                                    tsclientlib::MessageTarget::Channel
                                };

                                let content_clone = content.clone();
                                match sender.with_connection(move |con| {
                                    use tsclientlib::OutCommandExt;
                                    if let Ok(state) = con.get_state() {
                                        let _ = state.send_message(msg_target, &content_clone).send(con);
                                    }
                                }).await {
                                    Ok(_) => {
                                        info!("Sent {} message: {}", target, &content[..content.len().min(60)]);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Message sent ({})", target))));

                                        // Also speak via TTS if requested
                                        if speak_tts {
                                            if let Some(ref tx) = tts_tx {
                                                info!("TTS for send_message: '{}'", &content[..content.len().min(60)]);
                                                if let Err(e) = tx.send(TtsRequest { text: content, voice: None, speed: None }).await {
                                                    warn!("Failed to queue TTS for send_message: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("Send failed: {:?}", e)));
                                    }
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "TS3 not connected".to_string()));
                            }
                        }
                        CommandAction::MoveChannel { command_id, channel_id, password } => {
                            let mut handle_guard = ts3_handle.lock().await;
                            if let Some(ref mut sender) = *handle_guard {
                                // Step 1: Get own client ID
                                let own_id = match sender.with_connection(|con| {
                                    con.get_state().ok().map(|s| s.own_client.0)
                                }).await {
                                    Ok(Some(id)) => id,
                                    _ => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Failed to get own client ID".to_string()));
                                        continue;
                                    }
                                };

                                // Step 2: Build and send clientmove command
                                let mut cmd = OutCommand::new(
                                    Direction::C2S,
                                    Flags::empty(),
                                    PacketType::Command,
                                    "clientmove",
                                );
                                cmd.write_arg("clid", &own_id);
                                cmd.write_arg("cid", &channel_id);
                                if let Some(ref p) = password {
                                    if !p.is_empty() {
                                        cmd.write_arg("cpw", p);
                                    }
                                }

                                match sender.send_command(cmd).await {
                                    Ok(()) => {
                                        info!("Bot moved to channel {}", channel_id);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Moved to channel {}", channel_id))));
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("Move failed: {:?}", e)));
                                    }
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "TS3 not connected".to_string()));
                            }
                        }
                        CommandAction::PokeClient { command_id, client_id, message } => {
                            let msg_preview = message[..message.len().min(60)].to_string();
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| { cmd.write_arg("clid", &(client_id as u16)); cmd.write_arg("msg", &message); },
                                "clientpoke",
                                format!("Poked client {} with message: {}", client_id, msg_preview),
                                "Poke failed",
                            ).await;
                        }
                        CommandAction::KickClient { command_id, client_id, reason, reason_id } => {
                            let kick_type_str = if reason_id == 4 { "channel" } else { "server" };
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| {
                                    cmd.write_arg("clid", &(client_id as u16));
                                    cmd.write_arg("reasonid", &reason_id);
                                    if !reason.is_empty() { cmd.write_arg("reasonmsg", &reason); }
                                },
                                "clientkick",
                                format!("Kicked client {} from {}", client_id, kick_type_str),
                                "Kick failed",
                            ).await;
                        }
                        CommandAction::BanClient { command_id, client_id, duration_seconds, reason } => {
                            let duration_label = if duration_seconds == 0 {
                                "permanent".to_string()
                            } else {
                                format!("{}s", duration_seconds)
                            };
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| {
                                    cmd.write_arg("clid", &(client_id as u16));
                                    cmd.write_arg("time", &duration_seconds);
                                    if !reason.is_empty() { cmd.write_arg("banreason", &reason); }
                                },
                                "banclient",
                                format!("Banned client {} ({})", client_id, duration_label),
                                "Ban failed",
                            ).await;
                        }
                        CommandAction::MoveClient { command_id, client_id, channel_id, password } => {
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| {
                                    cmd.write_arg("clid", &(client_id as u16));
                                    cmd.write_arg("cid", &channel_id);
                                    if let Some(ref p) = password {
                                        if !p.is_empty() { cmd.write_arg("cpw", p); }
                                    }
                                },
                                "clientmove",
                                format!("Moved client {} to channel {}", client_id, channel_id),
                                "Move client failed",
                            ).await;
                        }
                        CommandAction::GetServerInfo { command_id } => {
                            let mut handle_guard = ts3_handle.lock().await;
                            if let Some(ref mut sender) = *handle_guard {
                                match sender.with_connection(|con| {
                                    con.get_state().ok().map(|state| {
                                        let server = &state.server;
                                        let client_count = state.clients.len();
                                        let channel_count = state.channels.len();
                                        serde_json::json!({
                                            "name": server.name,
                                            "version": server.version,
                                            "platform": server.platform,
                                            "max_clients": server.max_clients,
                                            "created": server.created.to_string(),
                                            "welcome_message": server.welcome_message,
                                            "server_id": server.id,
                                            "current_clients": client_count,
                                            "current_channels": channel_count,
                                            "codec_encryption_mode": format!("{:?}", server.codec_encryption_mode),
                                        })
                                    })
                                }).await {
                                    Ok(Some(data)) => {
                                        let _ = event_tx.send(WebSocketEvent::command_success_with_data(command_id, data));
                                    }
                                    Ok(None) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Failed to read TS3 server info".to_string()));
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("TS3 connection error: {:?}", e)));
                                    }
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "TS3 not connected".to_string()));
                            }
                        }
                        CommandAction::SetNickname { command_id, nickname } => {
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| { cmd.write_arg("client_nickname", &nickname); },
                                "clientupdate",
                                format!("Nickname changed to '{}'", nickname),
                                "Set nickname failed",
                            ).await;
                        }
                        CommandAction::CreateChannel { command_id, name, parent_id, temporary, topic, description, password } => {
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| {
                                    cmd.write_arg("channel_name", &name);
                                    if let Some(pid) = parent_id { cmd.write_arg("cpid", &pid); }
                                    if temporary.unwrap_or(false) { cmd.write_arg("channel_flag_temporary", &1u8); }
                                    if let Some(ref t) = topic { cmd.write_arg("channel_topic", t); }
                                    if let Some(ref d) = description { cmd.write_arg("channel_description", d); }
                                    if let Some(ref p) = password {
                                        if !p.is_empty() { cmd.write_arg("channel_password", p); }
                                    }
                                },
                                "channelcreate",
                                format!("Channel '{}' created", name),
                                "Create channel failed",
                            ).await;
                        }
                        CommandAction::ActivateListener { command_id, client_id } => {
                            if let Some(ref bm) = state.buffer_manager {
                                let mut bm = bm.lock().await;
                                if let Some(buf) = bm.get_buffer_mut(client_id) {
                                    if !buf.is_active {
                                        buf.activate();
                                        buf.clear();
                                        info!("🎤 Activated listening for client {} via WS command", client_id);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Listening activated for client {}", client_id))));
                                        // Update nickname to show listening state
                                        let mut handle_guard = ts3_handle.lock().await;
                                        if let Some(ref mut sender) = *handle_guard {
                                            let mut cmd = tsproto_packets::packets::OutCommand::new(
                                                tsproto_packets::packets::Direction::C2S,
                                                tsproto_packets::packets::Flags::empty(),
                                                tsproto_packets::packets::PacketType::Command,
                                                "clientupdate",
                                            );
                                            cmd.write_arg("client_nickname", &"Marlbot \u{1F3A4}");
                                            let _ = sender.send_command(cmd).await;
                                        }
                                    } else {
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Client {} already being listened to", client_id))));
                                    }
                                } else {
                                    let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("No audio buffer for client {}", client_id)));
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Buffer manager not available".to_string()));
                            }
                        }
                        CommandAction::DeactivateListener { command_id, client_id } => {
                            if let Some(ref bm) = state.buffer_manager {
                                let mut bm = bm.lock().await;
                                if let Some(buf) = bm.get_buffer_mut(client_id) {
                                    if buf.is_active {
                                        buf.deactivate();
                                        info!("🔇 Deactivated listening for client {} via WS command", client_id);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Listening deactivated for client {}", client_id))));
                                        // Update nickname if no more active listeners
                                        let still_listening = !bm.get_active_speakers().is_empty();
                                        if !still_listening {
                                            let mut handle_guard = ts3_handle.lock().await;
                                            if let Some(ref mut sender) = *handle_guard {
                                                let mut cmd = tsproto_packets::packets::OutCommand::new(
                                                    tsproto_packets::packets::Direction::C2S,
                                                    tsproto_packets::packets::Flags::empty(),
                                                    tsproto_packets::packets::PacketType::Command,
                                                    "clientupdate",
                                                );
                                                cmd.write_arg("client_nickname", &"Marlbot");
                                                let _ = sender.send_command(cmd).await;
                                            }
                                        }
                                    } else {
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Client {} was not being listened to", client_id))));
                                    }
                                } else {
                                    let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("No audio buffer for client {}", client_id)));
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Buffer manager not available".to_string()));
                            }
                        }
                        CommandAction::SetLanguage { command_id, client_id, language } => {
                            if let Some(ref lo) = state.language_overrides {
                                // Resolve UID from client_id via TS3 state
                                let mut handle_guard = ts3_handle.lock().await;
                                let uid_opt = if let Some(ref mut sender) = *handle_guard {
                                    sender.with_connection(move |con| {
                                        con.get_state().ok().and_then(|state| {
                                            state.clients.iter()
                                                .find(|(_, c)| c.id.0 == client_id as u16)
                                                .filter(|(_, c)| c.uid.is_some())
                                                .map(|(_, c)| base64::encode(&c.uid.as_ref().unwrap().0))
                                        })
                                    }).await.ok().flatten()
                                } else {
                                    None
                                };
                                drop(handle_guard);

                                if let Some(uid) = uid_opt {
                                    let mut overrides = lo.lock().await;
                                    if language == "auto" {
                                        overrides.remove(&uid);
                                        info!("Language override removed for {} (client {}) via WS", uid, client_id);
                                        let _ = save_language_prefs(&overrides);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Language reset to auto-detect for client {}", client_id))));
                                    } else {
                                        overrides.insert(uid.clone(), language.clone());
                                        info!("Language override set to '{}' for {} (client {}) via WS", language, uid, client_id);
                                        let _ = save_language_prefs(&overrides);
                                        let _ = event_tx.send(WebSocketEvent::command_success(command_id, Some(format!("Language set to '{}' for client {}", language, client_id))));
                                    }
                                } else {
                                    let _ = event_tx.send(WebSocketEvent::command_error(command_id, format!("Could not resolve UID for client {}", client_id)));
                                }
                            } else {
                                let _ = event_tx.send(WebSocketEvent::command_error(command_id, "Language overrides not available".to_string()));
                            }
                        }
                        CommandAction::SetChannelDescription { command_id, channel_id, description } => {
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| { cmd.write_arg("cid", &channel_id); cmd.write_arg("channel_description", &description); },
                                "channeledit",
                                format!("Channel {} description updated", channel_id),
                                "Set description failed",
                            ).await;
                        }
                        CommandAction::DeleteChannel { command_id, channel_id, force } => {
                            send_ts3_cmd(
                                &ts3_handle, &event_tx, command_id,
                                |cmd| {
                                    cmd.write_arg("cid", &channel_id);
                                    cmd.write_arg("force", &(if force { 1u32 } else { 0u32 }));
                                },
                                "channeldelete",
                                format!("Channel {} deleted (force={})", channel_id, force),
                                "Delete channel failed",
                            ).await;
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket client closed connection");
                    break;
                }
                Ok(Message::Ping(_)) => {
                    // Axum handles pong automatically
                }
                Err(e) => {
                    warn!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete (means connection is closing)
    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
        }
        _ = (&mut recv_task) => {
            send_task.abort();
        }
    }

    info!("WebSocket client disconnected");
}
