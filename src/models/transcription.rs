use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionEvent {
    pub timestamp: DateTime<Utc>,
    pub speaker_id: u64,
    pub speaker_uid: String,
    pub speaker_name: String,
    pub text: String,
    pub confidence: Option<f32>,
    pub language: Option<String>,
    pub duration_ms: u64,
}
