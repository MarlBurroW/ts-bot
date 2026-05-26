//! Per-speaker WAV recording.
//!
//! Captures the raw 48 kHz mono i16 PCM tapped from the Opus decoder
//! (BEFORE the 16 kHz resample used by Whisper) and appends it to a
//! permanent WAV file keyed on each speaker's stable TeamSpeak UID.
//!
//! Recording is OFF by default and toggled via WebSocket commands.
//! Each speaker's file is concatenated across sessions, so the file
//! grows over time. A 1 s zero-PCM padding is injected whenever a real
//! pause (> 500 ms without packets) is detected between two write
//! bursts — so the playback stays intelligible instead of running
//! sentences together.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use tracing::{debug, error, info, warn};

const SAMPLE_RATE_HZ: u32 = 48_000;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const PAUSE_THRESHOLD_MS: u128 = 500;
/// 1 second of silence injected between speech bursts that are separated
/// by a real pause. Sized to the native 48 kHz rate.
const SILENCE_PAD_SAMPLES: usize = SAMPLE_RATE_HZ as usize;

/// A single open recording file (one per speaker, lazy-opened on first
/// audio packet of a recording session).
struct RecordingFile {
    /// Raw TeamSpeak UID (base64), as reported by the server.
    raw_uid: String,
    speaker_name: String,
    path: PathBuf,
    /// `None` if the file failed to open — we keep a dud entry to avoid
    /// re-opening (and re-erroring) on every subsequent packet.
    writer: Option<WavWriter<File>>,
    /// Samples written to the file overall (existing + this session).
    /// Used to compute duration without re-parsing the WAV header.
    samples_written: u64,
    /// Last time we wrote real audio. `None` until the first write of
    /// this session.
    last_write_at: Option<Instant>,
    /// `true` if the file already had audio when we opened it. In that
    /// case we pad 1 s of silence before the first write of this
    /// session so the previous tail does not stick to the new head.
    had_prior_content: bool,
}

impl RecordingFile {
    fn open_or_create(raw_uid: &str, speaker_name: &str, path: PathBuf) -> Result<Self> {
        let spec = WavSpec {
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE_HZ,
            bits_per_sample: BITS_PER_SAMPLE,
            sample_format: SampleFormat::Int,
        };
        // The WAV header alone is 44 bytes. Anything bigger means there's
        // at least some PCM data already, so we should append.
        let existing_size = path.metadata().map(|m| m.len()).unwrap_or(0);
        let has_prior = existing_size > 44;

        let (writer, existing_samples): (WavWriter<File>, u64) = if has_prior {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("opening existing recording {:?}", path))?;
            let w = WavWriter::new_append(file)
                .with_context(|| format!("appending to recording {:?}", path))?;
            let existing = w.len() as u64;
            (w, existing)
        } else {
            // Defensive: ensure parent exists even though RecorderState
            // already created the recordings dir on start().
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file = File::create(&path)
                .with_context(|| format!("creating recording {:?}", path))?;
            let w = WavWriter::new(file, spec)
                .with_context(|| format!("writing WAV header for {:?}", path))?;
            (w, 0)
        };

        Ok(Self {
            raw_uid: raw_uid.to_string(),
            speaker_name: speaker_name.to_string(),
            path,
            writer: Some(writer),
            samples_written: existing_samples,
            last_write_at: None,
            had_prior_content: has_prior,
        })
    }

    fn append(&mut self, pcm_48k: &[i16]) -> Result<()> {
        if pcm_48k.is_empty() {
            return Ok(());
        }
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };

        // Lazy pause detection: gap between this write and the previous one.
        // First-write-of-session on an existing file is also treated as a
        // pause so cross-session boundaries stay audible.
        let inject_silence = match self.last_write_at {
            Some(t) => t.elapsed().as_millis() > PAUSE_THRESHOLD_MS,
            None => self.had_prior_content,
        };
        if inject_silence {
            for _ in 0..SILENCE_PAD_SAMPLES {
                writer.write_sample(0i16)?;
            }
            self.samples_written += SILENCE_PAD_SAMPLES as u64;
        }

        for &s in pcm_48k {
            writer.write_sample(s)?;
        }
        self.samples_written += pcm_48k.len() as u64;
        self.last_write_at = Some(Instant::now());
        Ok(())
    }

    fn finalize(mut self) -> Result<FinalizedFile> {
        let duration_ms = (self.samples_written * 1_000) / SAMPLE_RATE_HZ as u64;
        if let Some(writer) = self.writer.take() {
            writer
                .finalize()
                .with_context(|| format!("finalizing recording {:?}", self.path))?;
        }
        Ok(FinalizedFile {
            uid: self.raw_uid,
            speaker_name: self.speaker_name,
            path: self.path.to_string_lossy().to_string(),
            duration_ms,
        })
    }
}

/// Descriptor of a recording file at the moment it was finalized.
/// Emitted in the `recording_stopped` WebSocket event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FinalizedFile {
    pub uid: String,
    pub speaker_name: String,
    pub path: String,
    pub duration_ms: u64,
}

/// Mutable state behind the recorder. Held under a `std::sync::Mutex`
/// because the audio hot path is sync (we tap inside the Opus decoder
/// callback, not in an async context).
pub struct RecorderState {
    output_dir: PathBuf,
    writers: HashMap<String, RecordingFile>, // key = sanitized UID
    started_at: Option<Instant>,
}

/// Shared handle to the recorder.
///
/// `enabled` is an `AtomicBool` so the audio hot path can early-out
/// with a single relaxed load when recording is off — avoiding any
/// mutex acquisition on the common case (which is "no recording").
#[derive(Clone)]
pub struct RecorderHandle {
    enabled: Arc<AtomicBool>,
    state: Arc<Mutex<RecorderState>>,
}

impl RecorderHandle {
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(RecorderState {
                output_dir,
                writers: HashMap::new(),
                started_at: None,
            })),
        }
    }

    pub fn is_active(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Start a recording session.
    ///
    /// Creates the output directory if missing. Idempotent: calling
    /// `start` while already active is a no-op (returns `Ok(false)`).
    /// Returns `Ok(true)` if a new session was started.
    pub fn start(&self) -> Result<bool> {
        let mut state = self.state.lock().expect("recorder mutex poisoned");
        if self.enabled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        std::fs::create_dir_all(&state.output_dir).with_context(|| {
            format!("creating recordings directory {:?}", state.output_dir)
        })?;
        state.started_at = Some(Instant::now());
        self.enabled.store(true, Ordering::Relaxed);
        info!(
            "🎙️  Recording ENABLED — writing per-speaker WAV files to {:?}",
            state.output_dir
        );
        Ok(true)
    }

    /// Stop the recording session and finalize every open writer.
    /// Safe to call when not active (returns empty vec, no error).
    pub fn stop(&self) -> Vec<FinalizedFile> {
        if !self.enabled.swap(false, Ordering::Relaxed) {
            return Vec::new();
        }
        let mut state = self.state.lock().expect("recorder mutex poisoned");
        let mut finalized = Vec::with_capacity(state.writers.len());
        let writers: Vec<RecordingFile> = state.writers.drain().map(|(_, f)| f).collect();
        for file in writers {
            let name = file.speaker_name.clone();
            match file.finalize() {
                Ok(f) => finalized.push(f),
                Err(e) => error!("Failed to finalize recording for {}: {:?}", name, e),
            }
        }
        state.started_at = None;
        info!(
            "🎙️  Recording DISABLED — finalized {} file(s)",
            finalized.len()
        );
        finalized
    }

    /// Append a chunk of raw 48 kHz mono i16 PCM to the file associated
    /// with `raw_uid`. No-op if recording is off, if the UID is unresolved,
    /// or if the chunk is empty.
    ///
    /// Called from the audio hot path. The `enabled` atomic check
    /// short-circuits the common case without touching the mutex.
    pub fn append_pcm_48k(&self, raw_uid: &str, speaker_name: &str, pcm_48k: &[i16]) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if raw_uid.is_empty() || raw_uid == "unknown" {
            // Skip speakers without a resolved UID — we don't want to
            // mash multiple voices into a single "unknown.wav".
            return;
        }
        if pcm_48k.is_empty() {
            return;
        }

        let sanitized = sanitize_uid_for_filename(raw_uid);
        let mut state = self.state.lock().expect("recorder mutex poisoned");
        // Re-check enabled under the lock: avoids a race where stop()
        // could finalize between our atomic check and the lock.
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let path = state.output_dir.join(format!("{}.wav", sanitized));
        let entry = state.writers.entry(sanitized).or_insert_with(|| {
            match RecordingFile::open_or_create(raw_uid, speaker_name, path.clone()) {
                Ok(f) => {
                    info!(
                        "🎙️  Recording: opened file for {} (uid={}) → {:?}",
                        speaker_name, raw_uid, path
                    );
                    f
                }
                Err(e) => {
                    error!(
                        "🎙️  Recording: failed to open file for {} ({}): {:?}",
                        speaker_name, raw_uid, e
                    );
                    // Dud entry so we don't try (and fail) every packet.
                    RecordingFile {
                        raw_uid: raw_uid.to_string(),
                        speaker_name: speaker_name.to_string(),
                        path,
                        writer: None,
                        samples_written: 0,
                        last_write_at: None,
                        had_prior_content: false,
                    }
                }
            }
        });
        // Refresh the display name in case the cache was populated
        // after the writer was first opened (or the user renamed).
        if entry.speaker_name != speaker_name {
            entry.speaker_name = speaker_name.to_string();
        }
        if let Err(e) = entry.append(pcm_48k) {
            warn!(
                "🎙️  Recording: write error for {} ({}): {:?}",
                entry.speaker_name, entry.raw_uid, e
            );
        } else {
            debug!(
                "🎙️  Recording: appended {} samples to {:?}",
                pcm_48k.len(),
                entry.path
            );
        }
    }

    /// Snapshot of the current recorder state, suitable for the
    /// `get_recording_status` command response.
    pub fn status_json(&self) -> serde_json::Value {
        let state = self.state.lock().expect("recorder mutex poisoned");
        let files: Vec<_> = state
            .writers
            .values()
            .map(|f| {
                let duration_ms = (f.samples_written * 1_000) / SAMPLE_RATE_HZ as u64;
                let last_activity_ms_ago =
                    f.last_write_at.map(|t| t.elapsed().as_millis() as u64);
                serde_json::json!({
                    "uid": f.raw_uid,
                    "speaker_name": f.speaker_name,
                    "path": f.path.to_string_lossy(),
                    "duration_ms": duration_ms,
                    "last_activity_ms_ago": last_activity_ms_ago,
                })
            })
            .collect();
        serde_json::json!({
            "active": self.enabled.load(Ordering::Relaxed),
            "output_dir": state.output_dir.to_string_lossy(),
            "session_duration_ms": state.started_at.map(|t| t.elapsed().as_millis() as u64),
            "files": files,
        })
    }

    pub fn output_dir(&self) -> PathBuf {
        self.state
            .lock()
            .expect("recorder mutex poisoned")
            .output_dir
            .clone()
    }
}

/// Make a TS3 UID (base64) safe for use as a filename.
///
/// TS3 UIDs use the standard base64 alphabet which includes `/`, `+`
/// and `=`. We replace them with URL-safe equivalents (and strip `=`
/// padding) so the result is portable across filesystems.
fn sanitize_uid_for_filename(uid: &str) -> String {
    let mut out = String::with_capacity(uid.len());
    for c in uid.chars() {
        match c {
            '/' => out.push('_'),
            '+' => out.push('-'),
            '=' => {} // strip padding entirely
            // Defensive: also strip path separators / null bytes.
            '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Resolve the output directory from env (`RECORDINGS_DIR`) with a
/// sensible default. Called once at startup from `main`.
pub fn resolve_output_dir(configured: Option<&str>) -> PathBuf {
    match configured.filter(|s| !s.trim().is_empty()) {
        Some(s) => Path::new(s.trim()).to_path_buf(),
        None => PathBuf::from("recordings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_handles_base64_specials() {
        assert_eq!(
            sanitize_uid_for_filename("abc+/def=="),
            "abc-_def"
        );
    }

    #[test]
    fn sanitize_strips_filesystem_hostiles() {
        assert_eq!(sanitize_uid_for_filename("a\\b:c*d?e"), "abcde");
    }

    #[test]
    fn resolve_dir_defaults_when_unset() {
        assert_eq!(resolve_output_dir(None), PathBuf::from("recordings"));
        assert_eq!(resolve_output_dir(Some("   ")), PathBuf::from("recordings"));
    }

    #[test]
    fn resolve_dir_uses_configured() {
        assert_eq!(
            resolve_output_dir(Some("/var/log/ts3rec")),
            PathBuf::from("/var/log/ts3rec")
        );
    }
}
