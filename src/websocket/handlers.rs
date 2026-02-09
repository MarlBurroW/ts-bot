use crate::models::{WebSocketCommand, WebSocketEvent};

/// Result of handling a command
/// Some commands (like Speak) are async and need to be forwarded to a processing task,
/// so we return both a WebSocket response and an optional action to take.
pub enum CommandAction {
    /// Just respond to the client (no side effect)
    None,
    /// Forward TTS request to the TTS pipeline
    Speak { text: String, voice: Option<String> },
    /// Stop ongoing TTS playback
    StopSpeaking,
    /// Query TS3 server state (channels + clients)
    GetServerState { command_id: Option<String> },
    /// Move bot to a different channel
    MoveChannel { command_id: Option<String>, channel_id: u64, password: Option<String> },
    /// Send a text message (channel or private)
    SendMessage { command_id: Option<String>, target: String, content: String, client_id: Option<u64> },
    /// Poke a client
    PokeClient { command_id: Option<String>, client_id: u64, message: String },
    /// Kick a client from channel or server
    KickClient { command_id: Option<String>, client_id: u64, reason: String, reason_id: u8 },
    /// Move a client to a different channel
    MoveClient { command_id: Option<String>, client_id: u64, channel_id: u64, password: Option<String> },
    /// Change the bot's nickname
    SetNickname { command_id: Option<String>, nickname: String },
    /// Query TS3 server info (name, version, platform, max_clients, etc.)
    GetServerInfo { command_id: Option<String> },
    /// Create a new channel
    CreateChannel {
        command_id: Option<String>,
        name: String,
        parent_id: Option<u64>,
        temporary: Option<bool>,
        topic: Option<String>,
        description: Option<String>,
        password: Option<String>,
    },
    /// Set channel description
    SetChannelDescription {
        command_id: Option<String>,
        channel_id: u64,
        description: String,
    },
}

/// Handle incoming WebSocket command
/// Returns a (response event, optional action) tuple
pub fn handle_command(command: WebSocketCommand) -> (WebSocketEvent, CommandAction) {
    // Validate command
    if let Err(e) = command.validate() {
        return (
            WebSocketEvent::command_error(command.command_id().map(String::from), e),
            CommandAction::None,
        );
    }

    match command {
        WebSocketCommand::SendMessage { command_id, target, content, recipient } => {
            // Parse client_id from recipient field if target is "private"
            let client_id = if target == "private" {
                recipient.as_ref().and_then(|r| r.parse::<u64>().ok())
            } else {
                None
            };
            (
                WebSocketEvent::command_success(command_id.clone(), Some("Sending message...".to_string())),
                CommandAction::SendMessage { command_id, target, content, client_id },
            )
        }
        WebSocketCommand::MoveChannel { command_id, channel_id, password } => (
            // Placeholder response — the action handler in server.rs will send the real result
            WebSocketEvent::command_success(command_id.clone(), Some("Moving channel...".to_string())),
            CommandAction::MoveChannel { command_id, channel_id, password },
        ),
        WebSocketCommand::GetStatus { command_id, .. } => (
            // Placeholder response — the action handler in server.rs will send the data
            WebSocketEvent::command_success(command_id.clone(), None),
            CommandAction::GetServerState { command_id },
        ),
        WebSocketCommand::Speak { command_id, text, voice } => (
            WebSocketEvent::command_success(
                command_id,
                Some("Speech queued".to_string()),
            ),
            CommandAction::Speak { text, voice },
        ),
        WebSocketCommand::StopSpeaking { command_id } => (
            WebSocketEvent::command_success(
                command_id,
                Some("Playback stopped".to_string()),
            ),
            CommandAction::StopSpeaking,
        ),
        WebSocketCommand::PokeClient { command_id, client_id, message } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Poking client...".to_string())),
            CommandAction::PokeClient { command_id, client_id, message: message.unwrap_or_default() },
        ),
        WebSocketCommand::KickClient { command_id, client_id, reason, kick_type } => {
            // reason_id: 5 = kick from channel, 4 = kick from server
            let reason_id = match kick_type.as_deref() {
                Some("channel") => 5u8,
                _ => 4u8, // default to server kick
            };
            (
                WebSocketEvent::command_success(command_id.clone(), Some("Kicking client...".to_string())),
                CommandAction::KickClient { command_id, client_id, reason: reason.unwrap_or_default(), reason_id },
            )
        }
        WebSocketCommand::MoveClient { command_id, client_id, channel_id, password } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Moving client...".to_string())),
            CommandAction::MoveClient { command_id, client_id, channel_id, password },
        ),
        WebSocketCommand::SetNickname { command_id, nickname } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Setting nickname...".to_string())),
            CommandAction::SetNickname { command_id, nickname },
        ),
        WebSocketCommand::GetServerInfo { command_id } => (
            WebSocketEvent::command_success(command_id.clone(), None),
            CommandAction::GetServerInfo { command_id },
        ),
        WebSocketCommand::CreateChannel { command_id, name, parent_id, temporary, topic, description, password } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Creating channel...".to_string())),
            CommandAction::CreateChannel { command_id, name, parent_id, temporary, topic, description, password },
        ),
        WebSocketCommand::SetChannelDescription { command_id, channel_id, description } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Setting description...".to_string())),
            CommandAction::SetChannelDescription { command_id, channel_id, description },
        ),
    }
}
