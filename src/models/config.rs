use anyhow::Result;
use serde::Deserialize;
use std::fmt;

#[derive(Clone, Deserialize)]
pub struct BotConfig {
    #[serde(rename = "ts3_server")]
    pub ts3_server: String,
    #[serde(rename = "ts3_nickname")]
    pub ts3_nickname: String,
    #[serde(rename = "ts3_password", default)]
    pub ts3_password: Option<String>,
    #[serde(rename = "ts3_channel", default)]
    pub ts3_channel: Option<String>,

    #[serde(rename = "ws_host", default = "default_ws_host")]
    pub ws_host: String,
    #[serde(rename = "ws_port", default = "default_ws_port")]
    pub ws_port: u16,

    #[serde(rename = "log_level", default = "default_log_level")]
    pub log_level: String,
    #[serde(rename = "log_file", default)]
    pub log_file: Option<String>,

    #[serde(rename = "reconnect_max_attempts", default = "default_reconnect_max")]
    pub reconnect_max_attempts: u32,
    #[serde(rename = "reconnect_initial_delay_ms", default = "default_reconnect_initial")]
    pub reconnect_initial_delay_ms: u64,
    #[serde(rename = "reconnect_max_delay_ms", default = "default_reconnect_max_delay")]
    pub reconnect_max_delay_ms: u64,

    #[serde(rename = "tts_enabled", default)]
    pub tts_enabled: bool,
    #[serde(rename = "tts_api_url", default = "default_tts_api_url")]
    pub tts_api_url: String,
    #[serde(rename = "tts_api_key", default)]
    pub tts_api_key: Option<String>,
    #[serde(rename = "tts_model", default = "default_tts_model")]
    pub tts_model: String,
    #[serde(rename = "tts_voice", default = "default_tts_voice")]
    pub tts_voice: String,

    #[serde(rename = "elevenlabs_api_key", default)]
    pub elevenlabs_api_key: Option<String>,
    #[serde(rename = "elevenlabs_model", default = "default_elevenlabs_model")]
    pub elevenlabs_model: String,
}

fn default_ws_host() -> String { "127.0.0.1".to_string() }
fn default_ws_port() -> u16 { 8080 }
fn default_log_level() -> String { "INFO".to_string() }
fn default_reconnect_max() -> u32 { 10 }
fn default_reconnect_initial() -> u64 { 1000 }
fn default_reconnect_max_delay() -> u64 { 60000 }
fn default_tts_api_url() -> String { "https://api.openai.com/v1/audio/speech".to_string() }
fn default_tts_model() -> String { "tts-1".to_string() }
fn default_tts_voice() -> String { "nova".to_string() }
fn default_elevenlabs_model() -> String { "eleven_multilingual_v2".to_string() }

impl BotConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let config: BotConfig = envy::from_env()?;
        Ok(config)
    }
}

/// Custom Debug that redacts secrets so they never leak into logs.
/// Reveals only whether each secret is set and its length.
impl fmt::Debug for BotConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn redact(opt: &Option<String>) -> String {
            match opt {
                None => "None".to_string(),
                Some(s) if s.is_empty() => "Some(\"\")".to_string(),
                Some(s) => format!("Some(\"***[{} chars]\")", s.len()),
            }
        }
        f.debug_struct("BotConfig")
            .field("ts3_server", &self.ts3_server)
            .field("ts3_nickname", &self.ts3_nickname)
            .field("ts3_password", &redact(&self.ts3_password))
            .field("ts3_channel", &self.ts3_channel)
            .field("ws_host", &self.ws_host)
            .field("ws_port", &self.ws_port)
            .field("log_level", &self.log_level)
            .field("log_file", &self.log_file)
            .field("reconnect_max_attempts", &self.reconnect_max_attempts)
            .field("reconnect_initial_delay_ms", &self.reconnect_initial_delay_ms)
            .field("reconnect_max_delay_ms", &self.reconnect_max_delay_ms)
            .field("tts_enabled", &self.tts_enabled)
            .field("tts_api_url", &self.tts_api_url)
            .field("tts_api_key", &redact(&self.tts_api_key))
            .field("tts_model", &self.tts_model)
            .field("tts_voice", &self.tts_voice)
            .field("elevenlabs_api_key", &redact(&self.elevenlabs_api_key))
            .field("elevenlabs_model", &self.elevenlabs_model)
            .finish()
    }
}
