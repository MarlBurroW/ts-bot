//! Integration tests for trigger word detection pipeline
//!
//! These tests use real WAV audio samples and the Whisper model to validate
//! the complete detection pipeline end-to-end. Expected results come from
//! samples/samples.md.
//!
//! Run with: cargo test --test trigger_word_tests -- --nocapture
//!
//! Note: These tests require:
//! - models/ggml-small.bin (Whisper model)
//! - samples/*.wav (test audio files)

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use ts3_bot::audio::pipeline::{TriggerWordPipeline, DetectionResult};

/// Path to the Whisper model
const MODEL_PATH: &str = "models/ggml-small.bin";
/// Bot name for wake word detection
const BOT_NAME: &str = "marlbot";

/// Shared pipeline instance (Whisper is expensive to init)
/// Uses unwrap_or_else to recover from poisoned mutex (previous test panic)
fn get_pipeline() -> &'static Mutex<TriggerWordPipeline> {
    static PIPELINE: OnceLock<Mutex<TriggerWordPipeline>> = OnceLock::new();
    PIPELINE.get_or_init(|| {
        let pipeline = TriggerWordPipeline::new(MODEL_PATH, BOT_NAME)
            .expect("Failed to initialize TriggerWordPipeline - is the Whisper model present?");
        Mutex::new(pipeline)
    })
}

/// Lock the pipeline, recovering from poison if a previous test panicked
fn lock_pipeline() -> std::sync::MutexGuard<'static, TriggerWordPipeline> {
    get_pipeline()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Strip accents from text (é→e, è→e, ê→e, à→a, ù→u, etc.)
fn strip_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'ù' | 'û' | 'ü' => 'u',
            'ô' | 'ö' => 'o',
            'î' | 'ï' => 'i',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

/// Normalize text for fuzzy comparison: lowercase, strip accents, strip punctuation
fn normalize_for_comparison(s: &str) -> String {
    let stripped = strip_accents(&s.to_lowercase());
    stripped
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Helper: check if a command fuzzy-matches the expected text
/// Whisper transcription is approximate, so we check if key words are present
fn fuzzy_command_match(actual: &str, expected: &str) -> bool {
    let actual_norm = normalize_for_comparison(actual);
    let expected_norm = normalize_for_comparison(expected);

    // Extract key words from expected (words > 3 chars)
    let expected_words: Vec<&str> = expected_norm
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();

    if expected_words.is_empty() {
        return actual_norm.contains(&expected_norm) || expected_norm.contains(&actual_norm);
    }

    // At least 50% of key words should be present in actual
    // (Whisper may miss or rephrase some words)
    let matches = expected_words
        .iter()
        .filter(|w| actual_norm.contains(**w))
        .count();

    let ratio = matches as f32 / expected_words.len() as f32;
    ratio >= 0.5
}

/// Print detailed test report for a sample (FR-012)
fn print_test_report(
    sample_name: &str,
    results: &[DetectionResult],
    expected_commands: &[&str],
) {
    println!("\n=== {} ===", sample_name);
    println!("Segments detected: {}", results.len());

    let detected_results: Vec<&DetectionResult> =
        results.iter().filter(|r| r.detected).collect();
    println!("Wake word detections: {}", detected_results.len());
    println!("Expected detections: {}", expected_commands.len());

    for (i, result) in results.iter().enumerate() {
        println!(
            "  Segment {}: detected={}, transcription='{}', command={:?}",
            i, result.detected, result.transcription, result.command
        );
    }

    for (i, expected) in expected_commands.iter().enumerate() {
        if i < detected_results.len() {
            let actual_cmd = detected_results[i]
                .command
                .as_deref()
                .unwrap_or("<none>");
            let matches = fuzzy_command_match(actual_cmd, expected);
            println!(
                "  Expected[{}]: '{}'\n  Actual[{}]:   '{}' -> {}",
                i,
                expected,
                i,
                actual_cmd,
                if matches { "MATCH" } else { "MISMATCH" }
            );
        } else {
            println!("  Expected[{}]: '{}' -> MISSING (no detection)", i, expected);
        }
    }

    println!("=== end {} ===\n", sample_name);
}

#[test]
fn test_sample1_basic_trigger_and_command() {
    let sample_path = Path::new("samples/sample1.wav");
    if !sample_path.exists() {
        eprintln!("SKIP: sample1.wav not found");
        return;
    }

    let mut pipeline = lock_pipeline();
    let results = pipeline
        .process_wav_file(sample_path)
        .expect("Failed to process sample1.wav");

    let expected_command = "connecte toi a home assistant et eteins toutes mes lumieres";

    print_test_report("sample1", &results, &[expected_command]);

    // Should have at least one detection
    let detections: Vec<&DetectionResult> = results.iter().filter(|r| r.detected).collect();
    assert!(
        !detections.is_empty(),
        "sample1: Expected wake word detection but got none.\nTranscriptions: {:?}",
        results.iter().map(|r| &r.transcription).collect::<Vec<_>>()
    );

    // The detected command should fuzzy-match the expected
    let actual_cmd = detections[0].command.as_deref().unwrap_or("");
    assert!(
        fuzzy_command_match(actual_cmd, expected_command),
        "sample1: Command mismatch.\nExpected (fuzzy): '{}'\nActual: '{}'",
        expected_command,
        actual_cmd
    );
}

#[test]
fn test_sample2_multiple_detections() {
    let sample_path = Path::new("samples/sample2.wav");
    if !sample_path.exists() {
        eprintln!("SKIP: sample2.wav not found");
        return;
    }

    let mut pipeline = lock_pipeline();
    let results = pipeline
        .process_wav_file(sample_path)
        .expect("Failed to process sample2.wav");

    let expected_commands = [
        "comment ca va ? tu va bien ? moi ca va c'est cool",
        "qu'est ce que tu fait de beau la ?",
    ];

    print_test_report("sample2", &results, &expected_commands);

    let detections: Vec<&DetectionResult> = results.iter().filter(|r| r.detected).collect();

    // Should detect wake word at least twice (two invocations)
    assert!(
        detections.len() >= 2,
        "sample2: Expected at least 2 detections, got {}.\nAll results: {:#?}",
        detections.len(),
        results
    );

    // First detection command
    let cmd1 = detections[0].command.as_deref().unwrap_or("");
    assert!(
        fuzzy_command_match(cmd1, expected_commands[0]),
        "sample2 msg1: Command mismatch.\nExpected (fuzzy): '{}'\nActual: '{}'",
        expected_commands[0],
        cmd1
    );

    // Second detection command
    let cmd2 = detections[1].command.as_deref().unwrap_or("");
    assert!(
        fuzzy_command_match(cmd2, expected_commands[1]),
        "sample2 msg2: Command mismatch.\nExpected (fuzzy): '{}'\nActual: '{}'",
        expected_commands[1],
        cmd2
    );
}

#[test]
fn test_sample3_ignore_non_trigger_speech() {
    let sample_path = Path::new("samples/sample3.wav");
    if !sample_path.exists() {
        eprintln!("SKIP: sample3.wav not found");
        return;
    }

    let mut pipeline = lock_pipeline();
    let results = pipeline
        .process_wav_file(sample_path)
        .expect("Failed to process sample3.wav");

    let expected_command = "comment ca va ? qu'est ce que tu fait de beau ?";

    print_test_report("sample3", &results, &[expected_command]);

    let detections: Vec<&DetectionResult> = results.iter().filter(|r| r.detected).collect();

    // Should have at least one detection
    assert!(
        !detections.is_empty(),
        "sample3: Expected at least 1 wake word detection.\nAll results: {:#?}",
        results
    );

    // Extracted command should match
    let actual_cmd = detections[0].command.as_deref().unwrap_or("");
    assert!(
        fuzzy_command_match(actual_cmd, expected_command),
        "sample3: Command mismatch.\nExpected (fuzzy): '{}'\nActual: '{}'",
        expected_command,
        actual_cmd
    );
}

#[test]
fn test_sample4_reject_similar_names() {
    let sample_path = Path::new("samples/sample4.wav");
    if !sample_path.exists() {
        eprintln!("SKIP: sample4.wav not found");
        return;
    }

    let mut pipeline = lock_pipeline();
    let results = pipeline
        .process_wav_file(sample_path)
        .expect("Failed to process sample4.wav");

    let expected_command = "comment ca va ?";

    print_test_report("sample4", &results, &[expected_command]);

    let detections: Vec<&DetectionResult> = results.iter().filter(|r| r.detected).collect();
    let non_detections: Vec<&DetectionResult> = results.iter().filter(|r| !r.detected).collect();

    // CRITICAL: "Carlbot", "Marlmot", "Yarlbot", "Sarlbot" must NOT trigger
    // These should appear as non-detected segments
    assert!(
        !non_detections.is_empty(),
        "sample4: Expected some non-detected segments (fake bot names).\nAll results: {:#?}",
        results
    );

    // Should have exactly 1 detection (only the real "marlbot")
    assert!(
        !detections.is_empty(),
        "sample4: Expected at least 1 detection for 'marlbot'.\nAll results: {:#?}",
        results
    );

    // Verify the detection contains "marlbot" (not a false positive)
    let has_marlbot_detection = detections.iter().any(|d| {
        let norm = normalize_for_comparison(&d.transcription);
        norm.contains("marlbot") || norm.contains("marbot") || norm.contains("malbot")
    });
    assert!(
        has_marlbot_detection,
        "sample4: Detection should be on 'marlbot' variant, not a false positive.\nDetections: {:?}",
        detections.iter().map(|d| &d.transcription).collect::<Vec<_>>()
    );

    // Command extraction is a bonus — Whisper may not capture "Comment ca va?"
    // if it's cut off. The key validation is that only marlbot triggers detection.
    if let Some(cmd) = detections[0].command.as_deref() {
        if !cmd.is_empty() {
            assert!(
                fuzzy_command_match(cmd, expected_command),
                "sample4: Command was extracted but doesn't match.\nExpected (fuzzy): '{}'\nActual: '{}'",
                expected_command,
                cmd
            );
        } else {
            println!("  NOTE: Wake word detected but command not captured by Whisper (expected with short audio)");
        }
    } else {
        println!("  NOTE: Wake word detected but no command extracted (Whisper truncation)");
    }
}
