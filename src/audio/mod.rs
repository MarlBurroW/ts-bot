pub mod decoder;
pub mod encoder;
pub mod buffer;
pub mod whisper;
pub mod wake_word;
pub mod pipeline;
pub mod wav_loader;

pub use decoder::OpusDecoder;
pub use encoder::OpusEncoder;
pub use buffer::{AudioBuffer, SpeakerBufferManager};
pub use whisper::WhisperTranscriber;
pub use wake_word::WakeWordDetector;
pub use pipeline::{TriggerWordPipeline, DetectionResult, AudioSegment, SampleExpectation, ExpectedMessage};
