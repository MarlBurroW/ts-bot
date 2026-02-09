pub mod http;
pub mod kokoro;
pub mod playback;

use anyhow::Result;

/// Audio output from TTS synthesis
pub struct TtsAudio {
    /// PCM f32 samples normalized [-1.0, 1.0]
    pub samples: Vec<f32>,
    /// Sample rate of the audio (e.g., 24000 for Kokoro)
    pub sample_rate: u32,
}

/// Trait for interchangeable TTS backends.
///
/// Implement this trait to swap TTS engines (Kokoro, Fish Speech, CosyVoice, HTTP service, etc.)
/// without changing the rest of the bot (playback, WebSocket, main.rs).
pub trait TtsSynthesizer: Send + Sync {
    /// Synthesize text to audio PCM
    ///
    /// - `text`: the text to speak
    /// - `voice`: optional voice override (backend-specific, e.g., "ff_siwis" for Kokoro French)
    ///
    /// Returns `TtsAudio` with PCM samples and their sample rate.
    /// The caller is responsible for resampling to 48kHz if needed.
    fn synthesize(&self, text: &str, voice: Option<&str>) -> Result<TtsAudio>;

    /// Get the name of this TTS backend (for logging)
    fn name(&self) -> &str;

    /// Get the default voice ID
    fn default_voice(&self) -> &str;
}

pub use http::HttpTtsSynthesizer;
pub use kokoro::KokoroSynthesizer;
pub use playback::AudioPlayer;
