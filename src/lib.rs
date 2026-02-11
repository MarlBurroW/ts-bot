pub mod audio;
pub mod models;
pub mod persistence;
pub mod tts;
pub mod utils;
pub mod websocket;

// Re-export commonly used types
pub use models::{BotConfig, WebSocketEvent, WebSocketCommand};
pub use audio::TranscriptionPipeline;
pub use tts::{TtsSynthesizer, TtsAudio, AudioPlayer, HttpTtsSynthesizer};
