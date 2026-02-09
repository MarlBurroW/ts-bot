use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::broadcast};
use tracing::{error, info, warn};

use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};

use crate::models::{BotConfig, WebSocketCommand, WebSocketEvent};
use crate::websocket::handlers::{handle_command, CommandAction};

/// Shared TS3 connection handle, set once connected.
/// `None` if TS3 is not yet connected.
pub type SharedTs3Handle = Arc<tokio::sync::Mutex<Option<tsclientlib::sync::SyncConnectionHandle>>>;

/// TTS request forwarded from WebSocket to the TTS processing task
#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    pub voice: Option<String>,
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
}

/// Run the WebSocket server
///
/// - `tts_tx`: optional channel to forward TTS speak requests (None if TTS disabled)
/// - `ts3_handle`: shared TS3 connection handle (populated after TS3 connects)
pub async fn run_server(
    config: BotConfig,
    event_broadcaster: broadcast::Sender<WebSocketEvent>,
    tts_tx: Option<tokio::sync::mpsc::Sender<TtsRequest>>,
    ts3_handle: SharedTs3Handle,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.ws_host, config.ws_port);
    let socket_addr: SocketAddr = addr.parse()?;

    // Create shared state
    let state = AppState {
        event_broadcaster: Arc::new(event_broadcaster),
        tts_tx,
        ts3_handle,
        bot_nickname: config.ts3_nickname.clone(),
        ts3_server: config.ts3_server.clone(),
    };

    // Create Axum router with WebSocket endpoint
    let app = Router::new()
        .route("/ws", get(ws_handler))
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
                        CommandAction::Speak { text, voice } => {
                            if let Some(ref tx) = tts_tx {
                                if let Err(e) = tx.send(TtsRequest { text, voice }).await {
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
                            // StopSpeaking is handled by the AudioPlayer in main.rs
                            // We send a special event that main.rs listens for
                            // For now, the stop is immediate via the CommandAction
                            // TODO: wire stop through a dedicated channel if needed
                        }
                        CommandAction::GetServerState { command_id } => {
                            let mut handle_guard = ts3_handle.lock().await;
                            if let Some(ref mut sender) = *handle_guard {
                                match sender.with_connection(|con| {
                                    con.get_state().ok().map(|state| {
                                        let own_client_id = state.own_client.0;
                                        let channels: Vec<serde_json::Value> = state.channels.iter().map(|(id, ch)| {
                                            serde_json::json!({
                                                "id": id.0,
                                                "name": ch.name,
                                                "parent_id": ch.parent.0
                                            })
                                        }).collect();
                                        let clients: Vec<serde_json::Value> = state.clients.iter().map(|(id, cl)| {
                                            serde_json::json!({
                                                "id": id.0,
                                                "name": cl.name,
                                                "channel_id": cl.channel.0
                                            })
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
                        CommandAction::SendMessage { command_id, target, content, client_id } => {
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
