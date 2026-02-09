use anyhow::{bail, Context, Result};
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;
use tracing::{debug, info, warn};

// --- FFI bindings to libpv_porcupine.so ---

#[repr(C)]
#[allow(dead_code)]
enum PvStatus {
    Success = 0,
    OutOfMemory,
    IoError,
    InvalidArgument,
    StopIteration,
    KeyError,
    InvalidState,
    RuntimeError,
    ActivationError,
    ActivationLimitReached,
    ActivationThrottled,
    ActivationRefused,
}

impl PvStatus {
    fn is_success(&self) -> bool {
        matches!(self, PvStatus::Success)
    }

    fn as_str(&self) -> &'static str {
        match self {
            PvStatus::Success => "SUCCESS",
            PvStatus::OutOfMemory => "OUT_OF_MEMORY",
            PvStatus::IoError => "IO_ERROR",
            PvStatus::InvalidArgument => "INVALID_ARGUMENT",
            PvStatus::StopIteration => "STOP_ITERATION",
            PvStatus::KeyError => "KEY_ERROR",
            PvStatus::InvalidState => "INVALID_STATE",
            PvStatus::RuntimeError => "RUNTIME_ERROR",
            PvStatus::ActivationError => "ACTIVATION_ERROR",
            PvStatus::ActivationLimitReached => "ACTIVATION_LIMIT_REACHED",
            PvStatus::ActivationThrottled => "ACTIVATION_THROTTLED",
            PvStatus::ActivationRefused => "ACTIVATION_REFUSED",
        }
    }
}

#[repr(C)]
struct PvPorcupine {
    _private: [u8; 0],
}

extern "C" {
    fn pv_porcupine_init(
        access_key: *const c_char,
        model_path: *const c_char,
        device: *const c_char,
        num_keywords: i32,
        keyword_paths: *const *const c_char,
        sensitivities: *const f32,
        object: *mut *mut PvPorcupine,
    ) -> PvStatus;

    fn pv_porcupine_delete(object: *mut PvPorcupine);

    fn pv_porcupine_process(
        object: *mut PvPorcupine,
        pcm: *const i16,
        keyword_index: *mut i32,
    ) -> PvStatus;

    fn pv_porcupine_frame_length() -> i32;

    fn pv_sample_rate() -> i32;

    fn pv_porcupine_version() -> *const c_char;
}

// --- Safe Rust wrapper ---

/// Porcupine-based wake word detector
///
/// Uses Picovoice's Porcupine engine via FFI for reliable wake word detection.
/// Much more reliable than Whisper for keyword spotting:
/// - Trained specifically for one keyword
/// - Processes 512-sample frames (32ms) instead of 5-second buffers
/// - No hallucinations on silence/noise
pub struct PorcupineWakeWord {
    handle: *mut PvPorcupine,
    frame_length: usize,
    sample_rate: u32,
}

// Safety: PvPorcupine is thread-safe for process() calls with different audio
unsafe impl Send for PorcupineWakeWord {}
unsafe impl Sync for PorcupineWakeWord {}

impl PorcupineWakeWord {
    /// Create a new Porcupine wake word detector
    ///
    /// - `access_key`: Picovoice AccessKey from console.picovoice.ai
    /// - `model_path`: Path to porcupine_params_fr.pv (language model)
    /// - `keyword_path`: Path to the .ppn model file (trained for "marlbot")
    /// - `sensitivity`: Detection sensitivity 0.0 (strict) to 1.0 (lenient)
    pub fn new(access_key: &str, model_path: &str, keyword_path: &str, sensitivity: f32) -> Result<Self> {
        info!("Initializing Porcupine wake word detector");
        info!("  model: {}", model_path);
        info!("  keyword: {}", keyword_path);
        info!("  sensitivity: {}", sensitivity);

        let c_access_key = CString::new(access_key).context("Invalid access key")?;
        let c_model_path = CString::new(model_path).context("Invalid model path")?;
        let c_keyword_path = CString::new(keyword_path).context("Invalid keyword path")?;
        let c_device = CString::new("best").unwrap();

        let keyword_paths = [c_keyword_path.as_ptr()];
        let sensitivities = [sensitivity];

        let mut handle: *mut PvPorcupine = ptr::null_mut();

        let status = unsafe {
            pv_porcupine_init(
                c_access_key.as_ptr(),
                c_model_path.as_ptr(),
                c_device.as_ptr(),
                1,
                keyword_paths.as_ptr(),
                sensitivities.as_ptr(),
                &mut handle,
            )
        };

        if !status.is_success() || handle.is_null() {
            bail!("Porcupine init failed: {}", status.as_str());
        }

        let frame_length = unsafe { pv_porcupine_frame_length() } as usize;
        let sample_rate = unsafe { pv_sample_rate() } as u32;

        let version = unsafe {
            let v = pv_porcupine_version();
            if v.is_null() {
                "unknown".to_string()
            } else {
                std::ffi::CStr::from_ptr(v).to_string_lossy().into_owned()
            }
        };

        info!(
            "Porcupine v{} initialized: frame_length={}, sample_rate={}Hz",
            version, frame_length, sample_rate
        );

        Ok(Self {
            handle,
            frame_length,
            sample_rate,
        })
    }

    /// Check if audio samples contain the wake word
    ///
    /// Accepts f32 samples at 16kHz (same as Whisper pipeline).
    /// Converts to i16 internally and processes frame by frame.
    /// Returns true if wake word detected anywhere in the buffer.
    pub fn detect(&self, samples: &[f32]) -> Result<bool> {
        if samples.len() < self.frame_length {
            return Ok(false);
        }

        // Convert f32 [-1.0, 1.0] to i16
        let pcm_i16: Vec<i16> = samples.iter()
            .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // Process frame by frame
        let mut offset = 0;
        while offset + self.frame_length <= pcm_i16.len() {
            let frame = &pcm_i16[offset..offset + self.frame_length];
            let mut keyword_index: i32 = -1;

            let status = unsafe {
                pv_porcupine_process(self.handle, frame.as_ptr(), &mut keyword_index)
            };

            if !status.is_success() {
                warn!("Porcupine process error: {}", status.as_str());
                continue;
            }

            if keyword_index >= 0 {
                let time_ms = (offset as f64 / self.sample_rate as f64) * 1000.0;
                info!("🎯 Porcupine: wake word detected at {:.0}ms (frame offset {})", time_ms, offset);
                return Ok(true);
            }

            offset += self.frame_length;
        }

        debug!("Porcupine: no wake word in {} samples ({} frames)",
            samples.len(), samples.len() / self.frame_length);
        Ok(false)
    }

    /// Get the required frame length in samples
    pub fn frame_length(&self) -> usize {
        self.frame_length
    }

    /// Get the expected sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Drop for PorcupineWakeWord {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            info!("Destroying Porcupine instance");
            unsafe { pv_porcupine_delete(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}
