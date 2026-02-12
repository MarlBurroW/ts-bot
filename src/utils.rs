//! Utility functions shared across the bot.

use anyhow::Result;
use std::collections::HashMap;
use std::collections::VecDeque;
use tracing::info;

/// All voices supported by tts-1 (the classic 6).
const TTS1_VOICES: [&str; 6] = ["alloy", "echo", "fable", "nova", "onyx", "shimmer"];
/// Additional voices only supported by gpt-4o-mini-tts.
const GPT4O_MINI_EXTRA: [&str; 5] = ["ash", "ballad", "coral", "sage", "verse"];

/// Returns the list of valid voice names for a given TTS model.
/// - `tts-1` / `tts-1-hd`: only the classic 6 voices
/// - `gpt-4o-mini-tts` or unknown: all 11 voices (permissive)
pub fn valid_voices_for_model(model: &str) -> Vec<String> {
    let base: Vec<String> = TTS1_VOICES.iter().map(|s| s.to_string()).collect();
    if model == "tts-1" || model == "tts-1-hd" {
        base
    } else {
        let mut all = base;
        all.extend(GPT4O_MINI_EXTRA.iter().map(|s| s.to_string()));
        all
    }
}

/// Truncate a string to at most `max_bytes` bytes without splitting a UTF-8 char.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Format an uptime duration (seconds) into a compact human-readable string.
///
/// - `detailed=true`: includes seconds for short uptimes (e.g. "2m 34s")
/// - `detailed=false`: omits seconds (e.g. "2m"), suitable for stats display
pub fn format_uptime(secs: u64, detailed: bool) -> String {
    if detailed {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else if secs < 86400 {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        } else {
            format!("{}j {}h {}m", secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60)
        }
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}j {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Parse a duration string like "30m", "2h", "1h30m", "90s", "1d", "1j" into milliseconds.
/// Bare number without unit is treated as minutes.
/// Format a connection duration (seconds) into a compact string with ⏱ prefix.
///
/// Examples: `⏱45s`, `⏱12m`, `⏱2h34m`, `⏱1j5h`
pub fn format_connection_duration(secs: u64) -> String {
    if secs < 60 {
        format!("⏱{}s", secs)
    } else if secs < 3600 {
        format!("⏱{}m", secs / 60)
    } else if secs < 86400 {
        format!("⏱{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("⏱{}j{}h", secs / 86400, (secs % 86400) / 3600)
    }
}

pub fn parse_duration_str(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let mut total_ms: u64 = 0;
    let mut num_buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else {
            let n: u64 = num_buf.parse().ok()?;
            num_buf.clear();
            match ch {
                's' => total_ms += n * 1000,
                'm' => total_ms += n * 60 * 1000,
                'h' => total_ms += n * 3600 * 1000,
                'd' | 'j' => total_ms += n * 86400 * 1000,
                _ => return None,
            }
        }
    }
    // Bare number without unit → treat as minutes
    if !num_buf.is_empty() {
        let n: u64 = num_buf.parse().ok()?;
        if total_ms == 0 {
            total_ms += n * 60 * 1000;
        } else {
            return None; // trailing digits without unit after a valid part
        }
    }
    if total_ms > 0 { Some(total_ms) } else { None }
}

/// Format a duration in milliseconds to a human-readable string (e.g. "2h34m").
pub fn format_duration_ms(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 { format!("{}h{}m", h, m) } else { format!("{}h", h) }
    } else {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        if h > 0 { format!("{}j{}h", d, h) } else { format!("{}j", d) }
    }
}

/// Persist a single field in `data/bot_state.json` using read-modify-write.
/// Preserves all other fields in the JSON object.
pub fn save_bot_state_field<V: serde::Serialize>(key: &str, value: &V) {
    let _ = std::fs::create_dir_all("data");
    let mut state: serde_json::Value = std::fs::read_to_string("data/bot_state.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    state[key] = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    if let Ok(json) = serde_json::to_string_pretty(&state) {
        let _ = std::fs::write("data/bot_state.json", json);
    }
}

/// Persist language preferences to disk.
pub fn save_language_prefs(overrides: &HashMap<String, String>) -> Result<()> {
    let _ = std::fs::create_dir_all("data");
    let json = serde_json::to_string_pretty(overrides)?;
    std::fs::write("data/language_prefs.json", json)?;
    info!("Saved {} language preference(s)", overrides.len());
    Ok(())
}

/// Record a chat history entry: push to ring buffer + append to JSONL file.
pub async fn record_history(
    history: &tokio::sync::Mutex<VecDeque<(String, String, String)>>,
    author: String,
    text: String,
) {
    let ts = chrono::Utc::now().format("%H:%M").to_string();
    let full_ts = chrono::Utc::now().to_rfc3339();
    {
        let mut hist = history.lock().await;
        hist.push_back((ts.clone(), author.clone(), text.clone()));
        if hist.len() > 200 { hist.pop_front(); }
    }
    // Append to JSONL file (best-effort, don't block on errors)
    let line = serde_json::json!({
        "t": full_ts,
        "a": author,
        "m": text,
    });
    if let Ok(mut f) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("data/chat_history.jsonl")
        .await
    {
        use tokio::io::AsyncWriteExt;
        let _ = f.write_all(format!("{}\n", line).as_bytes()).await;
    }
}

/// Load chat history from JSONL file (last 200 entries), trim file if >500 lines.
pub fn load_chat_history() -> VecDeque<(String, String, String)> {
    let mut history = VecDeque::with_capacity(200);
    let path = "data/chat_history.jsonl";
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        // Trim file if over 500 lines (keep last 300)
        if lines.len() > 500 {
            let trimmed: Vec<&str> = lines[lines.len() - 300..].to_vec();
            let _ = std::fs::write(path, trimmed.join("\n") + "\n");
            info!("Trimmed chat_history.jsonl from {} to 300 lines", lines.len());
        }
        // Load last 200 into memory
        let start = if lines.len() > 200 { lines.len() - 200 } else { 0 };
        for line in &lines[start..] {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let full_ts = v.get("t").and_then(|t| t.as_str()).unwrap_or("");
                let ts = if full_ts.len() >= 16 {
                    full_ts[11..16].to_string()
                } else {
                    full_ts.to_string()
                };
                let author = v.get("a").and_then(|a| a.as_str()).unwrap_or("").to_string();
                let text = v.get("m").and_then(|m| m.as_str()).unwrap_or("").to_string();
                if !author.is_empty() {
                    history.push_back((ts, author, text));
                }
            }
        }
        if !history.is_empty() {
            info!("Loaded {} chat history entries from disk", history.len());
        }
    }
    history
}

/// Update bot nickname to reflect listening state (e.g. "Marlbot 🎤" when listening).
pub async fn update_bot_nickname(
    sender: &mut tsclientlib::sync::SyncConnectionHandle,
    is_listening: bool,
) {
    use tsproto_packets::packets::{Direction, Flags, OutCommand, PacketType};
    let nickname = if is_listening { "Marlbot \u{1F3A4}" } else { "Marlbot" };
    let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "clientupdate");
    cmd.write_arg("client_nickname", &nickname);
    match sender.send_command(cmd).await {
        Ok(()) => tracing::debug!("Nickname updated to '{}'", nickname),
        Err(e) => tracing::debug!("Nickname update failed: {:?}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 3), "hel");
    }

    #[test]
    fn test_truncate_str_utf8() {
        let s = "café";
        // 'é' is 2 bytes, so "café" = 5 bytes
        assert_eq!(truncate_str(s, 4), "caf"); // can't split é
        assert_eq!(truncate_str(s, 5), "café");
    }

    #[test]
    fn test_parse_duration_str() {
        assert_eq!(parse_duration_str("30m"), Some(30 * 60 * 1000));
        assert_eq!(parse_duration_str("2h"), Some(2 * 3600 * 1000));
        assert_eq!(parse_duration_str("1h30m"), Some(90 * 60 * 1000));
        assert_eq!(parse_duration_str("90s"), Some(90 * 1000));
        assert_eq!(parse_duration_str("1d"), Some(86400 * 1000));
        assert_eq!(parse_duration_str("1j"), Some(86400 * 1000));
        assert_eq!(parse_duration_str("5"), Some(5 * 60 * 1000)); // bare = minutes
        assert_eq!(parse_duration_str(""), None);
    }

    #[test]
    fn test_format_uptime_detailed() {
        assert_eq!(format_uptime(45, true), "45s");
        assert_eq!(format_uptime(150, true), "2m 30s");
        assert_eq!(format_uptime(3661, true), "1h 1m");
        assert_eq!(format_uptime(90061, true), "1j 1h 1m");
    }

    #[test]
    fn test_format_uptime_compact() {
        assert_eq!(format_uptime(150, false), "2m");
        assert_eq!(format_uptime(3661, false), "1h 1m");
        assert_eq!(format_uptime(90000, false), "1j 1h");
    }

    #[test]
    fn test_format_duration_ms() {
        assert_eq!(format_duration_ms(30_000), "30s");
        assert_eq!(format_duration_ms(60_000), "1m");
        assert_eq!(format_duration_ms(5_400_000), "1h30m");
        assert_eq!(format_duration_ms(86_400_000), "1j");
        assert_eq!(format_duration_ms(90_000_000), "1j1h");
    }

    // Voices supported by tts-1
    const TTS1_VOICES: &[&str] = &["alloy", "echo", "fable", "nova", "onyx", "shimmer"];
    // Additional voices only in gpt-4o-mini-tts
    const GPT4O_MINI_ONLY: &[&str] = &["ash", "ballad", "coral", "sage", "verse"];

    #[test]
    fn test_valid_voices_tts1() {
        for v in TTS1_VOICES {
            assert!(super::valid_voices_for_model("tts-1").contains(&v.to_string()));
        }
        for v in GPT4O_MINI_ONLY {
            assert!(!super::valid_voices_for_model("tts-1").contains(&v.to_string()));
        }
    }

    #[test]
    fn test_valid_voices_gpt4o_mini_tts() {
        let voices = super::valid_voices_for_model("gpt-4o-mini-tts");
        for v in TTS1_VOICES.iter().chain(GPT4O_MINI_ONLY.iter()) {
            assert!(voices.contains(&v.to_string()));
        }
    }

    #[test]
    fn test_valid_voices_unknown_model() {
        // Unknown models get all voices (permissive)
        assert_eq!(super::valid_voices_for_model("kokoro").len(), 11);
    }
}
