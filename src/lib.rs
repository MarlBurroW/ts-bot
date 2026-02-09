pub mod audio;
pub mod models;
pub mod tts;
pub mod websocket;

// Re-export commonly used types
pub use models::{BotConfig, WebSocketEvent, WebSocketCommand};
pub use audio::{WakeWordPipeline, TranscriptionPipeline, TriggerWordPipeline, DetectionResult, AudioSegment, SampleExpectation, ExpectedMessage};
pub use tts::{TtsSynthesizer, TtsAudio, AudioPlayer, KokoroSynthesizer, HttpTtsSynthesizer};
