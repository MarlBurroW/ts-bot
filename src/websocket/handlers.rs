use crate::models::{WebSocketCommand, WebSocketEvent};

/// Result of handling a command
/// Some commands (like Speak) are async and need to be forwarded to a processing task,
/// so we return both a WebSocket response and an optional action to take.
pub enum CommandAction {
    /// Just respond to the client (no side effect)
    None,
    /// Forward TTS request to the TTS pipeline
    Speak { text: String, voice: Option<String>, speed: Option<f32> },
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
    /// Activate listening for a specific client
    ActivateListener {
        command_id: Option<String>,
        client_id: u64,
    },
    /// Deactivate listening for a specific client
    DeactivateListener {
        command_id: Option<String>,
        client_id: u64,
    },
    /// Set Whisper language override for a client
    SetLanguage {
        command_id: Option<String>,
        client_id: u64,
        language: String,
    },
    /// Delete a channel
    DeleteChannel {
        command_id: Option<String>,
        channel_id: u64,
        force: bool,
    },
    /// Set TTS volume (0-200)
    SetVolume {
        command_id: Option<String>,
        volume: u8,
    },
    /// Get current TTS volume
    GetVolume {
        command_id: Option<String>,
    },
    /// Set default TTS voice
    SetVoice {
        command_id: Option<String>,
        voice: String,
    },
    /// Get current default TTS voice
    GetVoice {
        command_id: Option<String>,
    },
    /// Get chat/transcription history
    GetHistory {
        command_id: Option<String>,
        count: u32,
    },
    /// Set silence detection timeout (ms)
    SetTimeout {
        command_id: Option<String>,
        timeout_ms: u64,
    },
    /// Get current silence detection timeout
    GetTimeout {
        command_id: Option<String>,
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
        WebSocketCommand::Speak { command_id, text, voice, speed } => (
            WebSocketEvent::command_success(
                command_id,
                Some("Speech queued".to_string()),
            ),
            CommandAction::Speak { text, voice, speed },
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
            // TS3 protocol: reasonid=4 = kick from channel, reasonid=5 = kick from server
            let reason_id = match kick_type.as_deref() {
                Some("channel") => 4u8,
                _ => 5u8, // default to server kick
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
        WebSocketCommand::ActivateListener { command_id, client_id } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Activating listener...".to_string())),
            CommandAction::ActivateListener { command_id, client_id },
        ),
        WebSocketCommand::DeactivateListener { command_id, client_id } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Deactivating listener...".to_string())),
            CommandAction::DeactivateListener { command_id, client_id },
        ),
        WebSocketCommand::SetLanguage { command_id, client_id, language } => (
            WebSocketEvent::command_success(command_id.clone(), Some(format!("Setting language to '{}'...", language))),
            CommandAction::SetLanguage { command_id, client_id, language },
        ),
        WebSocketCommand::DeleteChannel { command_id, channel_id, force } => (
            WebSocketEvent::command_success(command_id.clone(), Some("Deleting channel...".to_string())),
            CommandAction::DeleteChannel { command_id, channel_id, force: force.unwrap_or(false) },
        ),
        WebSocketCommand::SetVolume { command_id, volume } => (
            WebSocketEvent::command_success(command_id.clone(), Some(format!("Volume set to {}%", volume))),
            CommandAction::SetVolume { command_id, volume },
        ),
        WebSocketCommand::GetVolume { command_id } => (
            WebSocketEvent::command_success(command_id.clone(), None),
            CommandAction::GetVolume { command_id },
        ),
        WebSocketCommand::SetVoice { command_id, voice } => (
            WebSocketEvent::command_success(command_id.clone(), Some(format!("Voice set to '{}'", voice))),
            CommandAction::SetVoice { command_id, voice },
        ),
        WebSocketCommand::GetVoice { command_id } => (
            WebSocketEvent::command_success(command_id.clone(), None),
            CommandAction::GetVoice { command_id },
        ),
        WebSocketCommand::GetHistory { command_id, count } => {
            let count = count.unwrap_or(20).min(50);
            (
                WebSocketEvent::command_success(command_id.clone(), None),
                CommandAction::GetHistory { command_id, count },
            )
        }
        WebSocketCommand::SetTimeout { command_id, timeout_ms } => (
            WebSocketEvent::command_success(command_id.clone(), Some(format!("Timeout set to {}ms", timeout_ms))),
            CommandAction::SetTimeout { command_id, timeout_ms },
        ),
        WebSocketCommand::GetTimeout { command_id } => (
            WebSocketEvent::command_success(command_id.clone(), None),
            CommandAction::GetTimeout { command_id },
        ),
    }
}
