pub mod client;

use serde::{Deserialize, Serialize};

/// TS3 connection state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

/// Full connection state tracking
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TS3ConnectionState {
    pub state: ConnectionState,
    pub server_address: String,
    pub current_channel_id: Option<u64>,
    pub current_channel_name: Option<String>,
    pub reconnect_attempts: u32,
    pub last_error: Option<String>,
}

impl TS3ConnectionState {
    pub fn new(server_address: String) -> Self {
        Self {
            state: ConnectionState::Disconnected,
            server_address,
            current_channel_id: None,
            current_channel_name: None,
            reconnect_attempts: 0,
            last_error: None,
        }
    }
}
