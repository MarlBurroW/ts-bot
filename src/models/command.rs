use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebSocketCommand {
    SendMessage {
        command_id: Option<String>,
        target: String,
        content: String,
        recipient: Option<String>,
        #[serde(default)]
        tts: Option<bool>,
    },
    MoveChannel {
        command_id: Option<String>,
        channel_id: u64,
        password: Option<String>,
    },
    GetStatus {
        command_id: Option<String>,
    },
    Speak {
        command_id: Option<String>,
        text: String,
        voice: Option<String>,
        speed: Option<f32>,
    },
    StopSpeaking {
        command_id: Option<String>,
    },
    PokeClient {
        command_id: Option<String>,
        client_id: u64,
        message: Option<String>,
    },
    KickClient {
        command_id: Option<String>,
        client_id: u64,
        reason: Option<String>,
        /// "channel" or "server" (default: "server")
        kick_type: Option<String>,
    },
    BanClient {
        command_id: Option<String>,
        client_id: u64,
        /// Ban duration in seconds (None or 0 = permanent)
        duration_seconds: Option<u64>,
        /// Optional ban reason
        reason: Option<String>,
    },
    MoveClient {
        command_id: Option<String>,
        client_id: u64,
        channel_id: u64,
        password: Option<String>,
    },
    SetNickname {
        command_id: Option<String>,
        nickname: String,
    },
    GetServerInfo {
        command_id: Option<String>,
    },
    CreateChannel {
        command_id: Option<String>,
        name: String,
        /// Parent channel ID (0 = root)
        parent_id: Option<u64>,
        /// Make it temporary (auto-delete when empty)
        temporary: Option<bool>,
        /// Optional topic
        topic: Option<String>,
        /// Optional description
        description: Option<String>,
        /// Optional password
        password: Option<String>,
    },
    SetChannelDescription {
        command_id: Option<String>,
        channel_id: u64,
        description: String,
    },
    ActivateListener {
        command_id: Option<String>,
        client_id: u64,
    },
    DeactivateListener {
        command_id: Option<String>,
        client_id: u64,
    },
    SetLanguage {
        command_id: Option<String>,
        client_id: u64,
        /// ISO 639-1 language code (e.g. "fr", "en") or "auto" to reset
        language: String,
    },
    DeleteChannel {
        command_id: Option<String>,
        channel_id: u64,
        /// If true, force-delete even if clients are in the channel (moves them to default)
        force: Option<bool>,
    },
    SetVolume {
        command_id: Option<String>,
        /// Volume level 0-200 (100 = normal, 0 = mute, 200 = 2x gain)
        volume: u8,
    },
    GetVolume {
        command_id: Option<String>,
    },
    SetVoice {
        command_id: Option<String>,
        /// Voice name (alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer, verse)
        voice: String,
    },
    GetVoice {
        command_id: Option<String>,
    },
    GetHistory {
        command_id: Option<String>,
        /// Number of entries to return (default 20, max 50)
        count: Option<u32>,
    },
    SetTimeout {
        command_id: Option<String>,
        /// Silence detection timeout in milliseconds (500-10000)
        timeout_ms: u64,
    },
    GetTimeout {
        command_id: Option<String>,
    },
    SetSpeed {
        command_id: Option<String>,
        /// TTS speed (0.25-4.0, default 1.15)
        speed: f32,
    },
    GetSpeed {
        command_id: Option<String>,
    },
}

impl WebSocketCommand {
    pub fn command_id(&self) -> Option<&str> {
        match self {
            Self::SendMessage { command_id, .. } => command_id.as_deref(),
            Self::MoveChannel { command_id, .. } => command_id.as_deref(),
            Self::GetStatus { command_id, .. } => command_id.as_deref(),
            Self::Speak { command_id, .. } => command_id.as_deref(),
            Self::StopSpeaking { command_id, .. } => command_id.as_deref(),
            Self::PokeClient { command_id, .. } => command_id.as_deref(),
            Self::KickClient { command_id, .. } => command_id.as_deref(),
            Self::BanClient { command_id, .. } => command_id.as_deref(),
            Self::MoveClient { command_id, .. } => command_id.as_deref(),
            Self::SetNickname { command_id, .. } => command_id.as_deref(),
            Self::GetServerInfo { command_id, .. } => command_id.as_deref(),
            Self::CreateChannel { command_id, .. } => command_id.as_deref(),
            Self::SetChannelDescription { command_id, .. } => command_id.as_deref(),
            Self::ActivateListener { command_id, .. } => command_id.as_deref(),
            Self::DeactivateListener { command_id, .. } => command_id.as_deref(),
            Self::SetLanguage { command_id, .. } => command_id.as_deref(),
            Self::DeleteChannel { command_id, .. } => command_id.as_deref(),
            Self::SetVolume { command_id, .. } => command_id.as_deref(),
            Self::GetVolume { command_id, .. } => command_id.as_deref(),
            Self::SetVoice { command_id, .. } => command_id.as_deref(),
            Self::GetVoice { command_id, .. } => command_id.as_deref(),
            Self::GetHistory { command_id, .. } => command_id.as_deref(),
            Self::SetTimeout { command_id, .. } => command_id.as_deref(),
            Self::GetTimeout { command_id, .. } => command_id.as_deref(),
            Self::SetSpeed { command_id, .. } => command_id.as_deref(),
            Self::GetSpeed { command_id, .. } => command_id.as_deref(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::SendMessage { content, .. } => {
                if content.is_empty() {
                    return Err("Message content cannot be empty".to_string());
                }
                Ok(())
            }
            Self::Speak { text, .. } => {
                if text.is_empty() {
                    return Err("Text cannot be empty".to_string());
                }
                Ok(())
            }
            Self::CreateChannel { name, .. } => {
                if name.is_empty() {
                    return Err("Channel name cannot be empty".to_string());
                }
                Ok(())
            }
            Self::SetChannelDescription { description, .. } => {
                if description.len() > 8192 {
                    return Err("Description too long (max 8192 chars)".to_string());
                }
                Ok(())
            }
            Self::SetLanguage { language, .. } => {
                let valid = ["fr","en","de","es","it","pt","nl","ru","ja","ko","zh","ar","pl","cs","sv","da","fi","no","tr","uk","ro","hu","el","he","th","vi","id","ms","hi","bn","auto"];
                if !valid.contains(&language.as_str()) {
                    return Err(format!("Invalid language '{}'. Use ISO 639-1 code or 'auto'.", language));
                }
                Ok(())
            }
            Self::SetVolume { volume, .. } => {
                if *volume > 200 {
                    return Err("Volume must be 0-200".to_string());
                }
                Ok(())
            }
            Self::SetVoice { voice, .. } => {
                // Only sanity-check that the voice is non-empty here.
                // The actual validation against the live voice registry
                // (OpenAI built-ins + ElevenLabs voices loaded at startup)
                // is performed by the WebSocket handler, which has access
                // to `all_valid_voices`.
                if voice.trim().is_empty() {
                    return Err("Voice name must not be empty".to_string());
                }
                Ok(())
            }
            Self::SetTimeout { timeout_ms, .. } => {
                if *timeout_ms < 500 || *timeout_ms > 10000 {
                    return Err("Timeout must be 500-10000ms".to_string());
                }
                Ok(())
            }
            Self::SetSpeed { speed, .. } => {
                if *speed < 0.25 || *speed > 4.0 {
                    return Err("Speed must be 0.25-4.0".to_string());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
