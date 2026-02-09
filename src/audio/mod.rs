pub mod decoder;
pub mod encoder;
pub mod buffer;
pub mod whisper;
pub mod wake_word;
pub mod pipeline;
pub mod wav_loader;
pub mod whisper_api;
pub mod rustpotter_detector;
pub mod porcupine_wake;

pub use decoder::OpusDecoder;
pub use encoder::OpusEncoder;
pub use buffer::{AudioBuffer, SpeakerBufferManager};
pub use whisper::WhisperTranscriber;
pub use wake_word::WakeWordDetector;
pub use pipeline::{WakeWordPipeline, TranscriptionPipeline, TriggerWordPipeline, DetectionResult, AudioSegment, SampleExpectation, ExpectedMessage, rms_energy};
pub use rustpotter_detector::RustpotterWakeWord;
