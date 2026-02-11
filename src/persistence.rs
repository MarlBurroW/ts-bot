//! Persistence helpers for loading/saving JSON data files.
//!
//! All bot state files live in the `data/` directory. This module provides
//! generic helpers to eliminate the repetitive read→parse→unwrap_or_default
//! and serialize→write patterns scattered throughout main.rs.

use serde::{de::DeserializeOwned, Serialize};
use tracing::{info, warn};

/// Load a JSON file, returning `Default::default()` on any error (missing file, parse failure).
/// Logs a warning on parse errors, stays silent on missing files.
pub fn load_json<T: DeserializeOwned + Default>(path: &str) -> T {
    match std::fs::read_to_string(path) {
        Ok(data) => match serde_json::from_str::<T>(&data) {
            Ok(val) => val,
            Err(e) => {
                warn!("Failed to parse {}: {}, using default", path, e);
                T::default()
            }
        },
        Err(_) => T::default(),
    }
}

/// Load a JSON file and log the count if non-empty. Works for any collection
/// that implements `Len` (HashMap, Vec, etc.).
pub fn load_json_logged<T: DeserializeOwned + Default + Len>(path: &str, label: &str) -> T {
    let val: T = load_json(path);
    let count = val.len();
    if count > 0 {
        info!("Loaded {} {} from {}", count, label, path);
    }
    val
}

/// Save a value as JSON to a file. Uses `to_string_pretty` for readability.
/// Silently ignores write errors (follows existing bot convention).
pub fn save_json<T: Serialize>(path: &str, value: &T) {
    let _ = serde_json::to_string_pretty(value)
        .ok()
        .and_then(|s| std::fs::write(path, s).ok());
}

/// Save a value as compact JSON (no pretty-printing). Use for high-frequency saves.
pub fn save_json_compact<T: Serialize>(path: &str, value: &T) {
    let _ = serde_json::to_string(value)
        .ok()
        .and_then(|s| std::fs::write(path, s).ok());
}

/// Ensure the `data/` directory exists. Called once at startup.
pub fn ensure_data_dir() {
    let _ = std::fs::create_dir_all("data");
}

/// Trait for types that have a length (HashMap, Vec, etc.)
pub trait Len {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K, V, S> Len for std::collections::HashMap<K, V, S>
where
    S: std::hash::BuildHasher,
{
    fn len(&self) -> usize {
        self.len()
    }
}

impl<T> Len for Vec<T> {
    fn len(&self) -> usize {
        self.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_load_json_missing_file() {
        let val: HashMap<String, String> = load_json("/tmp/ts3bot_test_nonexistent_12345.json");
        assert!(val.is_empty());
    }

    #[test]
    fn test_save_and_load_json() {
        let path = "/tmp/ts3bot_test_persistence.json";
        let mut map = HashMap::new();
        map.insert("key".to_string(), "value".to_string());
        save_json(path, &map);

        let loaded: HashMap<String, String> = load_json(path);
        assert_eq!(loaded.get("key").unwrap(), "value");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_load_json_invalid_data() {
        let path = "/tmp/ts3bot_test_invalid.json";
        std::fs::write(path, "not json").unwrap();

        let val: HashMap<String, String> = load_json(path);
        assert!(val.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_save_json_compact() {
        let path = "/tmp/ts3bot_test_compact.json";
        let data = vec![1, 2, 3];
        save_json_compact(path, &data);

        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "[1,2,3]"); // No whitespace

        let _ = std::fs::remove_file(path);
    }
}
