//! Shared state types used across the bot.
//!
//! These structs were extracted from main() to improve code organization
//! and enable reuse across modules.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Chat history entry: (timestamp, author, text).
pub type ChatHistoryEntry = (String, String, String);

/// Shared chat history buffer (thread-safe, async).
pub type SharedChatHistory = Arc<Mutex<VecDeque<ChatHistoryEntry>>>;

/// Last spoken TTS info: (text, voice, speed). Uses async Mutex.
pub type LastSpokenInfo = Arc<Mutex<Option<(String, Option<String>, Option<f32>)>>>;

/// Notify watchers: lowercase target name → vec of (requester_name, requester_uid). Uses async Mutex.
pub type NotifyWatchers = Arc<Mutex<HashMap<String, Vec<(String, String)>>>>;

/// An active poll in a TS3 channel.
pub struct ActivePoll {
    pub question: String,
    pub options: Vec<String>,
    /// One HashSet<UID> per option, tracking who voted for it.
    pub votes: Vec<HashSet<String>>,
    pub creator: String,
}

/// An active duel challenge between two users.
pub struct ActiveDuel {
    pub challenger_name: String,
    #[allow(dead_code)] // stored for potential future use (e.g. reconnect lookup)
    pub challenger_uid: String,
    pub challenger_clid: u16,
    pub target_name: String,
    pub target_uid: String,
    pub target_clid: u16,
    pub created: std::time::Instant,
}

/// A timed reminder set by a user.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Reminder {
    /// Unix timestamp in ms when the reminder fires.
    pub due_ms: u64,
    /// Creator UID.
    pub uid: String,
    /// Creator display name at time of creation.
    pub name: String,
    /// Reminder text.
    pub message: String,
    /// When it was created (Unix ms).
    pub created_ms: u64,
}

/// Serializable bot usage statistics (for persistence).
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct BotStatsData {
    pub messages_received: u64,
    pub commands_executed: u64,
    pub tts_calls: u64,
    pub voice_transcriptions: u64,
    pub greetings_sent: u64,
}

/// Thread-safe bot usage statistics with atomic counters.
pub struct BotStats {
    pub messages_received: AtomicU64,
    pub commands_executed: AtomicU64,
    pub tts_calls: AtomicU64,
    pub voice_transcriptions: AtomicU64,
    pub greetings_sent: AtomicU64,
}

impl Default for BotStats {
    fn default() -> Self {
        Self {
            messages_received: AtomicU64::new(0),
            commands_executed: AtomicU64::new(0),
            tts_calls: AtomicU64::new(0),
            voice_transcriptions: AtomicU64::new(0),
            greetings_sent: AtomicU64::new(0),
        }
    }
}

impl BotStats {
    /// Load stats from `data/stats.json`, or return zeroed stats.
    pub fn load() -> Self {
        let data: BotStatsData = std::fs::read_to_string("data/stats.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if data.messages_received > 0 || data.commands_executed > 0 {
            tracing::info!(
                "Restored stats: {} msgs, {} cmds, {} tts, {} transcriptions, {} greets",
                data.messages_received, data.commands_executed, data.tts_calls,
                data.voice_transcriptions, data.greetings_sent
            );
        }
        Self {
            messages_received: AtomicU64::new(data.messages_received),
            commands_executed: AtomicU64::new(data.commands_executed),
            tts_calls: AtomicU64::new(data.tts_calls),
            voice_transcriptions: AtomicU64::new(data.voice_transcriptions),
            greetings_sent: AtomicU64::new(data.greetings_sent),
        }
    }

    /// Persist current stats to `data/stats.json`.
    pub fn save(&self) {
        let data = BotStatsData {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            commands_executed: self.commands_executed.load(Ordering::Relaxed),
            tts_calls: self.tts_calls.load(Ordering::Relaxed),
            voice_transcriptions: self.voice_transcriptions.load(Ordering::Relaxed),
            greetings_sent: self.greetings_sent.load(Ordering::Relaxed),
        };
        let _ = std::fs::create_dir_all("data");
        if let Ok(json) = serde_json::to_string_pretty(&data) {
            let _ = std::fs::write("data/stats.json", json);
        }
    }

    pub fn inc_messages(&self) { self.messages_received.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_commands(&self) { self.commands_executed.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_tts(&self) { self.tts_calls.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_transcriptions(&self) { self.voice_transcriptions.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_greetings(&self) { self.greetings_sent.fetch_add(1, Ordering::Relaxed); }
}
