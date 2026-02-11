use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Channel,
    Private,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub message_type: MessageType,
    pub sender_id: u64,
    pub sender_uid: String,
    pub sender_name: String,
    pub content: String,
    pub channel_id: Option<u64>,
    pub channel_name: Option<String>,
    pub timestamp: DateTime<Utc>,
}
