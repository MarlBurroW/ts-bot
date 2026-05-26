use serde::{Deserialize, Serialize};

use super::message::MessageEvent;
use super::transcription::TranscriptionEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebSocketEvent {
    Welcome {
        nickname: String,
        server: String,
        api_version: String,
        connection_status: String,
    },
    MessageReceived {
        #[serde(flatten)]
        event: MessageEvent,
    },
    Transcription {
        #[serde(flatten)]
        event: TranscriptionEvent,
    },
    ConnectionStatus {
        status: String,
    },
    CommandResponse {
        command_id: Option<String>,
        success: bool,
        message: Option<String>,
        data: Option<serde_json::Value>,
    },
    SpeakStarted {
        text: String,
    },
    SpeakCompleted {
        text: String,
        duration_ms: u64,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    ClientConnected {
        client_id: u64,
        client_name: String,
        channel_id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        uid: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        country_code: Option<String>,
    },
    ClientDisconnected {
        client_id: u64,
        client_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        uid: Option<String>,
    },
    ClientMoved {
        client_id: u64,
        client_name: String,
        old_channel_id: u64,
        new_channel_id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        uid: Option<String>,
        /// Name of the old channel
        #[serde(skip_serializing_if = "Option::is_none")]
        old_channel_name: Option<String>,
        /// Name of the new channel
        #[serde(skip_serializing_if = "Option::is_none")]
        new_channel_name: Option<String>,
    },
    /// Emitted when recording is enabled via `start_recording`.
    RecordingStarted {
        output_dir: String,
    },
    /// Emitted when recording is disabled via `stop_recording` or
    /// during bot shutdown. Lists every WAV file that was finalized.
    RecordingStopped {
        files: Vec<serde_json::Value>,
    },
}

impl WebSocketEvent {
    pub fn welcome(nickname: String, server: String, connection_status: String) -> Self {
        Self::Welcome {
            nickname,
            server,
            api_version: "1.0.0".to_string(),
            connection_status,
        }
    }

    pub fn message_received(event: MessageEvent) -> Self {
        Self::MessageReceived { event }
    }

    pub fn transcription(event: TranscriptionEvent) -> Self {
        Self::Transcription { event }
    }

    pub fn command_success(command_id: Option<String>, message: Option<String>) -> Self {
        Self::CommandResponse {
            command_id,
            success: true,
            message,
            data: None,
        }
    }

    pub fn command_success_with_data(command_id: Option<String>, data: serde_json::Value) -> Self {
        Self::CommandResponse {
            command_id,
            success: true,
            message: None,
            data: Some(data),
        }
    }

    pub fn command_error(command_id: Option<String>, error: String) -> Self {
        Self::CommandResponse {
            command_id,
            success: false,
            message: Some(error),
            data: None,
        }
    }

    pub fn speak_started(text: String) -> Self {
        Self::SpeakStarted { text }
    }

    pub fn speak_completed(text: String, duration_ms: u64) -> Self {
        Self::SpeakCompleted { text, duration_ms, success: true, error: None }
    }

    pub fn speak_failed(text: String, duration_ms: u64, error: String) -> Self {
        Self::SpeakCompleted { text, duration_ms, success: false, error: Some(error) }
    }
}
