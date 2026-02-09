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
    }
}
