use super::WhisperTranscriber;
use anyhow::Result;
use tracing::{debug, info};

/// Compute Levenshtein edit distance between two strings
/// Standard dynamic programming algorithm: O(n*m) time and space
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = a_chars.len();
    let m = b_chars.len();

    if n == 0 { return m; }
    if m == 0 { return n; }

    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for (i, row) in dp.iter_mut().enumerate().take(n + 1) { row[0] = i; }
    for (j, val) in dp[0].iter_mut().enumerate().take(m + 1) { *val = j; }

    for i in 1..=n {
        for j in 1..=m {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)           // deletion
                .min(dp[i][j - 1] + 1)               // insertion
                .min(dp[i - 1][j - 1] + cost);       // substitution
        }
    }

    dp[n][m]
}

/// Maximum Levenshtein distance for fuzzy wake word matching
/// Set to 3 because Whisper produces variations like "malbatt" (dist 3) and "malibut" (dist 3)
/// False positives are prevented by the first-character constraint in fuzzy_detect()
const FUZZY_MAX_DISTANCE: usize = 2;

/// Wake word detector using Whisper
pub struct WakeWordDetector {
    /// The bot name in lowercase for Levenshtein comparison
    bot_name: String,
    /// Hardcoded wake word variations (exact substring matching)
    wake_words: Vec<String>,
}

impl WakeWordDetector {
    /// Create a new wake word detector
    /// bot_name: name of the bot (e.g., "marlbot")
    pub fn new(bot_name: &str) -> Self {
        let bot_name_lower = bot_name.to_lowercase();

        // Generate variations of wake words including common Whisper misheard versions
        let wake_words = vec![
            // Exact variations
            format!("hey {}", bot_name_lower),
            format!("hé {}", bot_name_lower),
            format!("ok {}", bot_name_lower),
            bot_name_lower.clone(),
            // Common Whisper misheard variations for "marlbot"
            "malbut".to_string(),
            "malbot".to_string(),
            "marlbut".to_string(),
            "marlbatt".to_string(),
            "marlbat".to_string(),
            "marbot".to_string(),
            "marbut".to_string(),
            "marbat".to_string(),
            "marbatt".to_string(),
            "melbot".to_string(),
            "melbut".to_string(),
            "malbec".to_string(),
            "aimalbot".to_string(),
            "aimalbat".to_string(),
            "aimalbut".to_string(),
            "malibut".to_string(),
            "malbatt".to_string(),
            "malbott".to_string(),
            "mar bot".to_string(),
            "mal bot".to_string(),
            "mal but".to_string(),
            "mal batt".to_string(),
            "mar but".to_string(),
            "mel bot".to_string(),
            "mel but".to_string(),
            "marl bat".to_string(),
            "marl batt".to_string(),
        ];

        info!("Wake word detector initialized with variations: {:?}", wake_words);

        Self { bot_name: bot_name_lower, wake_words }
    }

    /// Normalize text by stripping punctuation for fuzzy matching
    pub fn normalize(text: &str) -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Check if the transcribed text contains a wake word
    ///
    /// Uses four strategies in order:
    /// 1. Exact substring matching against hardcoded variations
    /// 2. Adjacent word pair concatenation (catches Whisper splitting: "aimal bat" → "aimalbat")
    /// 3. Levenshtein fuzzy matching on individual words (max distance 3, first char must match)
    /// 4. Levenshtein fuzzy matching on concatenated adjacent pairs (stricter: max distance 2)
    pub fn detect(&self, transcribed_text: &str) -> bool {
        let text_normalized = Self::normalize(transcribed_text);

        // Strategy 1: exact substring match
        let exact_match = self.wake_words.iter().any(|wake_word| {
            text_normalized.contains(wake_word)
        });

        if exact_match {
            info!("Wake word detected (exact) in: '{}'", transcribed_text);
            return true;
        }

        // Strategy 2: Adjacent word pair matching
        // Whisper sometimes splits the wake word across words (e.g., "aimal bat" → "aimalbat")
        let words: Vec<&str> = text_normalized.split_whitespace().collect();
        for pair in words.windows(2) {
            let concatenated = format!("{}{}", pair[0], pair[1]);

            // Pair exact: check against wake words (space-stripped)
            let pair_exact = self.wake_words.iter().any(|ww| {
                let ww_nospace: String = ww.chars().filter(|c| !c.is_whitespace()).collect();
                concatenated == ww_nospace
            });
            if pair_exact {
                info!("Wake word detected (pair-exact '{}') in: '{}'", concatenated, transcribed_text);
                return true;
            }

            // Pair fuzzy: Levenshtein on concatenated pair (stricter: max 2 to avoid false positives)
            let bot_first_char = self.bot_name.chars().next().unwrap_or('m');
            if let Some(first) = concatenated.chars().next() {
                if first == bot_first_char {
                    let dist = levenshtein_distance(&concatenated, &self.bot_name);
                    if dist <= FUZZY_MAX_DISTANCE.saturating_sub(1) {
                        info!("Wake word detected (pair-fuzzy '{}', dist={}) in: '{}'", concatenated, dist, transcribed_text);
                        return true;
                    }
                }
            }
        }

        // Strategy 3: Levenshtein fuzzy on individual words
        if self.fuzzy_detect(transcribed_text, FUZZY_MAX_DISTANCE).is_some() {
            info!("Wake word detected (fuzzy) in: '{}'", transcribed_text);
            return true;
        }

        debug!("No wake word in: '{}' (normalized: '{}')", transcribed_text, text_normalized);
        false
    }

    /// Fuzzy wake word detection using Levenshtein distance
    ///
    /// Tokenizes the text and checks each word against the bot name.
    /// Returns the matched word and a confidence score (1.0 for exact, lower for fuzzy).
    ///
    /// Constraint: the first character of the candidate must match the bot name's
    /// first character to avoid false positives like "carlbot" matching "marlbot".
    pub fn fuzzy_detect(&self, text: &str, max_distance: usize) -> Option<(String, f32)> {
        let text_normalized = Self::normalize(text);
        let bot_first_char = self.bot_name.chars().next()?;

        let bot_len = self.bot_name.chars().count();

        for word in text_normalized.split_whitespace() {
            // Quick reject: first char must match bot name's first char
            if let Some(first_char) = word.chars().next() {
                if first_char != bot_first_char {
                    continue;
                }
            }

            // Quick reject: word length must be within 2 chars of bot name length
            let word_len = word.chars().count();
            if word_len + 2 < bot_len || word_len > bot_len + 2 {
                continue;
            }

            let dist = levenshtein_distance(word, &self.bot_name);
            if dist == 0 {
                return Some((word.to_string(), 1.0));
            }
            if dist <= max_distance {
                let confidence = 1.0 - (dist as f32 / (max_distance as f32 + 1.0));
                debug!("Fuzzy match: '{}' ~ '{}' (dist={}, conf={:.2})", word, self.bot_name, dist, confidence);
                return Some((word.to_string(), confidence));
            }
        }

        None
    }

    /// Detect wake word in audio samples
    pub fn detect_in_audio(
        &self,
        whisper: &mut WhisperTranscriber,
        samples: &[f32],
    ) -> Result<bool> {
        // Transcribe the audio
        let text = whisper.transcribe_wake_word(samples)?;

        // Check for wake word
        Ok(self.detect(&text))
    }

    /// Extract the command after the wake word
    /// Returns the text after the wake word, or None if no wake word found
    pub fn extract_command(&self, transcribed_text: &str) -> Option<String> {
        let text_lower = transcribed_text.to_lowercase();

        for wake_word in &self.wake_words {
            if let Some(pos) = text_lower.find(wake_word) {
                let command_start = pos + wake_word.len();
                let command = transcribed_text[command_start..].trim();

                if !command.is_empty() {
                    info!("Extracted command: '{}'", command);
                    return Some(command.to_string());
                }
            }
        }

        None
    }

    /// Strip ALL wake word occurrences from text, returning clean command
    /// Handles "Hey Marlbot, hey Marlbot, comment ça va?" -> "comment ça va?"
    ///
    /// Uses both exact matching (hardcoded list) and fuzzy matching (Levenshtein)
    /// to strip wake word variants from the text.
    pub fn extract_command_clean(&self, transcribed_text: &str) -> Option<String> {
        let mut result = transcribed_text.to_string();
        let mut changed = true;

        // Phase 1: Repeatedly strip exact wake words until none remain
        while changed {
            changed = false;
            let result_lower = result.to_lowercase();
            let result_normalized = Self::normalize(&result);

            for wake_word in &self.wake_words {
                // Try normalized match first
                if result_normalized.contains(wake_word) {
                    // Find approximate position in original text
                    let lower = result_lower.as_str();
                    if let Some(pos) = lower.find(wake_word) {
                        let end = pos + wake_word.len();
                        // Remove the wake word and surrounding punctuation/spaces
                        let before = result[..pos].trim_end_matches([',', '.', ' ', '!', '?']);
                        let after = result[end..].trim_start_matches([',', '.', ' ', '!', '?']);
                        result = if before.is_empty() {
                            after.to_string()
                        } else {
                            format!("{} {}", before, after)
                        };
                        changed = true;
                        break; // Restart loop after modification
                    }
                }
            }
        }

        // Phase 2: Strip fuzzy-matched wake words (Levenshtein)
        changed = true;
        while changed {
            changed = false;
            let normalized = Self::normalize(&result);
            let bot_first_char = self.bot_name.chars().next().unwrap_or('m');

            let words: Vec<&str> = normalized.split_whitespace().collect();
            for word in &words {
                if let Some(first) = word.chars().next() {
                    if first == bot_first_char {
                        let dist = levenshtein_distance(word, &self.bot_name);
                        if dist > 0 && dist <= FUZZY_MAX_DISTANCE {
                            // Found a fuzzy match — strip it from original text
                            let lower = result.to_lowercase();
                            if let Some(pos) = lower.find(word) {
                                let end = pos + word.len();
                                let before = result[..pos].trim_end_matches([',', '.', ' ', '!', '?']);
                                let after = result[end..].trim_start_matches([',', '.', ' ', '!', '?']);
                                result = if before.is_empty() {
                                    after.to_string()
                                } else {
                                    format!("{} {}", before, after)
                                };
                                changed = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Also strip common trigger prefixes ("hey", "hé", "ok")
        let trimmed = result.trim().to_string();
        let stripped = Self::strip_trigger_prefix(&trimmed);

        if stripped.is_empty() || stripped == transcribed_text.trim() {
            None
        } else {
            info!("Cleaned transcription: '{}' -> '{}'", transcribed_text, stripped);
            Some(stripped)
        }
    }

    /// Strip common trigger prefixes like "hey", "hé", "ok" from start of text
    fn strip_trigger_prefix(text: &str) -> String {
        let prefixes = ["hey", "hé", "he", "ok"];
        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();

        for prefix in &prefixes {
            if lower.starts_with(prefix) {
                let rest = trimmed[prefix.len()..].trim_start_matches([',', '.', ' ', '!', '?']);
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
        }

        trimmed.to_string()
    }
}

impl Default for WakeWordDetector {
    fn default() -> Self {
        Self::new("marlbot")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Basic detection ---

    #[test]
    fn test_wake_word_detection() {
        let detector = WakeWordDetector::new("marlbot");

        assert!(detector.detect("hey marlbot comment ça va"));
        assert!(detector.detect("Hey Marlbot, dis moi"));
        assert!(detector.detect("hé marlbot"));
        assert!(detector.detect("ok marlbot"));

        assert!(!detector.detect("bonjour tout le monde"));
        assert!(!detector.detect("hey"));
    }

    #[test]
    fn test_command_extraction() {
        let detector = WakeWordDetector::new("marlbot");

        assert_eq!(
            detector.extract_command("hey marlbot comment ça va"),
            Some("comment ça va".to_string())
        );

        assert_eq!(
            detector.extract_command("bonjour"),
            None
        );
    }

    // --- T019: Normalization edge cases ---

    #[test]
    fn test_normalize_strips_punctuation() {
        assert_eq!(WakeWordDetector::normalize("Hey, Marlbot!"), "hey marlbot");
        assert_eq!(WakeWordDetector::normalize("...Marlbot???"), "marlbot");
        assert_eq!(WakeWordDetector::normalize("Hey Marlbot."), "hey marlbot");
    }

    #[test]
    fn test_normalize_collapses_whitespace() {
        assert_eq!(WakeWordDetector::normalize("hey   marlbot   comment"), "hey marlbot comment");
        assert_eq!(WakeWordDetector::normalize("  hey marlbot  "), "hey marlbot");
    }

    #[test]
    fn test_normalize_mixed_case() {
        assert_eq!(WakeWordDetector::normalize("HEY MARLBOT"), "hey marlbot");
        assert_eq!(WakeWordDetector::normalize("HeY mArLbOt"), "hey marlbot");
    }

    // --- T019: Whisper misheard variations ---

    #[test]
    fn test_detect_whisper_variations() {
        let detector = WakeWordDetector::new("marlbot");

        // All known Whisper misheard variations should trigger
        assert!(detector.detect("malbut comment ça va"));
        assert!(detector.detect("malbot dis moi"));
        assert!(detector.detect("marlbut qu'est-ce que tu fais"));
        assert!(detector.detect("marbot bonjour"));
        assert!(detector.detect("marbut allume la lumière"));
        assert!(detector.detect("melbot éteins tout"));
        assert!(detector.detect("melbut c'est quoi"));
        assert!(detector.detect("malbec fais un truc"));
        assert!(detector.detect("aimalbot aide moi"));

        // Space-separated variants
        assert!(detector.detect("mar bot dis moi"));
        assert!(detector.detect("mal bot bonjour"));
        assert!(detector.detect("mel bot salut"));
        assert!(detector.detect("marl bat oui"));
    }

    #[test]
    fn test_detect_rejects_distant_names() {
        let detector = WakeWordDetector::new("marlbot");

        // These names should NOT trigger detection
        assert!(!detector.detect("carlbot comment ça va"));
        assert!(!detector.detect("yarlbot dis moi"));
        assert!(!detector.detect("sarlbot bonjour"));
        assert!(!detector.detect("hello world"));
        assert!(!detector.detect("robot fais quelque chose"));
    }

    // --- T019: extract_command_clean with multi-trigger ---

    #[test]
    fn test_extract_command_clean_single() {
        let detector = WakeWordDetector::new("marlbot");

        let cmd = detector.extract_command_clean("Hey marlbot, comment ça va ?");
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert!(cmd.contains("comment"), "Expected 'comment' in: '{}'", cmd);
    }

    #[test]
    fn test_extract_command_clean_double_trigger() {
        let detector = WakeWordDetector::new("marlbot");

        // Double wake word should be fully stripped
        let cmd = detector.extract_command_clean("Hey marlbot, hey marlbot, comment ça va?");
        assert!(cmd.is_some(), "Should extract a command from double trigger");
        let cmd = cmd.unwrap();
        // The command should not contain "marlbot"
        assert!(
            !cmd.to_lowercase().contains("marlbot"),
            "Command should not contain 'marlbot', got: '{}'",
            cmd
        );
    }

    #[test]
    fn test_extract_command_clean_only_trigger() {
        let detector = WakeWordDetector::new("marlbot");

        // Only wake word, no command
        let cmd = detector.extract_command_clean("Hey marlbot");
        // Should return None (no command after stripping)
        assert!(cmd.is_none(), "Only wake word should return None, got: {:?}", cmd);
    }

    #[test]
    fn test_extract_command_clean_variant() {
        let detector = WakeWordDetector::new("marlbot");

        // Using a Whisper misheard variant
        let cmd = detector.extract_command_clean("malbot dis moi l'heure");
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert!(cmd.contains("heure"), "Expected 'heure' in: '{}'", cmd);
    }

    // --- T019: Edge cases ---

    #[test]
    fn test_detect_empty_string() {
        let detector = WakeWordDetector::new("marlbot");
        assert!(!detector.detect(""));
    }

    #[test]
    fn test_extract_command_empty() {
        let detector = WakeWordDetector::new("marlbot");
        assert_eq!(detector.extract_command(""), None);
    }

    #[test]
    fn test_custom_bot_name() {
        let detector = WakeWordDetector::new("testbot");
        assert!(detector.detect("hey testbot bonjour"));
        assert!(detector.detect("testbot"));
        // Should not match marlbot variations
        assert!(!detector.detect("marlbot"));
    }

    // --- T027: Levenshtein distance tests ---

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein_distance("marlbot", "marlbot"), 0);
    }

    #[test]
    fn test_levenshtein_one_substitution() {
        // malbot: 'r' replaced by nothing -> distance 1 (deletion)
        assert_eq!(levenshtein_distance("marlbot", "malbot"), 1);
    }

    #[test]
    fn test_levenshtein_two_changes() {
        // marbut: 'l'->'nothing' and 'o'->'u' -> distance 2
        assert_eq!(levenshtein_distance("marlbot", "marbut"), 2);
    }

    #[test]
    fn test_levenshtein_carlbot() {
        // carlbot: m->c (1 substitution, distance 1)
        // But fuzzy_detect should reject it because first char differs
        assert_eq!(levenshtein_distance("marlbot", "carlbot"), 1);
    }

    #[test]
    fn test_levenshtein_yarlbot() {
        // yarlbot: m->y (1 substitution, distance 1)
        // But fuzzy_detect should reject it because first char differs
        assert_eq!(levenshtein_distance("marlbot", "yarlbot"), 1);
    }

    #[test]
    fn test_levenshtein_sarlbot() {
        // sarlbot: m->s (1 substitution, distance 1)
        // But fuzzy_detect should reject it because first char differs
        assert_eq!(levenshtein_distance("marlbot", "sarlbot"), 1);
    }

    #[test]
    fn test_levenshtein_completely_different() {
        assert!(levenshtein_distance("marlbot", "hello") > 3);
    }

    #[test]
    fn test_levenshtein_empty() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    // --- T028: Combined matching (exact + fuzzy) ---

    #[test]
    fn test_fuzzy_detect_close_match() {
        let detector = WakeWordDetector::new("marlbot");
        // "marlbut" is distance 1 from "marlbot" and starts with 'm' - should fuzzy match
        // (even though it's also in the hardcoded list, testing fuzzy path)
        let result = detector.fuzzy_detect("hey merlbot comment ça va", 2);
        assert!(result.is_some(), "merlbot should fuzzy-match marlbot (dist 1, starts with m)");
        let (matched, confidence) = result.unwrap();
        assert!(!matched.is_empty());
        assert!(confidence > 0.0);
    }

    #[test]
    fn test_fuzzy_detect_too_distant() {
        let detector = WakeWordDetector::new("marlbot");
        // "robot" is very different from "marlbot"
        let result = detector.fuzzy_detect("hey robot comment ça va", 2);
        assert!(result.is_none(), "robot should NOT fuzzy-match marlbot");
    }

    #[test]
    fn test_fuzzy_detect_rejects_different_first_char() {
        let detector = WakeWordDetector::new("marlbot");
        // "carlbot" is distance 1 but starts with 'c', not 'm' -> rejected
        let result = detector.fuzzy_detect("hey carlbot comment ça va", 2);
        assert!(result.is_none(), "carlbot should NOT fuzzy-match (different first char)");

        let result = detector.fuzzy_detect("hey yarlbot", 2);
        assert!(result.is_none(), "yarlbot should NOT fuzzy-match (different first char)");

        let result = detector.fuzzy_detect("hey sarlbot", 2);
        assert!(result.is_none(), "sarlbot should NOT fuzzy-match (different first char)");
    }

    #[test]
    fn test_fuzzy_detect_exact_match_higher_confidence() {
        let detector = WakeWordDetector::new("marlbot");
        // Exact match gives confidence 1.0
        let exact_result = detector.fuzzy_detect("hey marlbot", 2);
        // Close match (merlbot, dist 1, starts with 'm') gives lower confidence
        let fuzzy_result = detector.fuzzy_detect("hey merlbot", 2);

        assert!(exact_result.is_some());
        assert!(fuzzy_result.is_some());

        let exact_conf = exact_result.unwrap().1;
        let fuzzy_conf = fuzzy_result.unwrap().1;
        assert!(
            exact_conf >= fuzzy_conf,
            "Exact match confidence ({}) should be >= fuzzy ({})",
            exact_conf,
            fuzzy_conf
        );
    }

    #[test]
    fn test_detect_with_fuzzy_fallback() {
        let detector = WakeWordDetector::new("marlbot");
        // "merlbot" isn't in the hardcoded list, but Levenshtein dist 1 + starts with 'm' should catch it
        assert!(detector.detect("hey merlbot comment ça va"));
        // "morlbot" dist 1, starts with 'm' - novel variation
        assert!(detector.detect("hey morlbot dis moi"));
    }

    #[test]
    fn test_detect_fuzzy_rejects_distant() {
        let detector = WakeWordDetector::new("marlbot");
        // Words with different first char should be rejected
        assert!(!detector.detect("hello world"));
        assert!(!detector.detect("robot fais quelque chose"));
        // carlbot, yarlbot, sarlbot should still be rejected (different first char)
        assert!(!detector.detect("carlbot comment ça va"));
        assert!(!detector.detect("yarlbot dis moi"));
        assert!(!detector.detect("sarlbot bonjour"));
    }

    #[test]
    fn test_extract_command_clean_fuzzy_match() {
        let detector = WakeWordDetector::new("marlbot");
        // "merlbot" should be stripped by fuzzy matching (starts with 'm', dist 1)
        let cmd = detector.extract_command_clean("Hey merlbot, comment ça va ?");
        assert!(cmd.is_some(), "Should extract command from fuzzy-matched wake word");
        let cmd = cmd.unwrap();
        assert!(
            !cmd.to_lowercase().contains("merlbot"),
            "Command should not contain 'merlbot', got: '{}'",
            cmd
        );
    }

    // --- Adjacent word pair detection (Whisper word-splitting) ---

    #[test]
    fn test_detect_pair_exact_split_wake_word() {
        let detector = WakeWordDetector::new("marlbot");
        // Whisper splits "aimalbat" into "aimal bat" — pair concatenation should catch it
        assert!(detector.detect("aimal bat comment ça va"));
        // Also test "aimal bot" → "aimalbot" which is in the list
        assert!(detector.detect("hey aimal bot"));
    }

    #[test]
    fn test_detect_pair_fuzzy_split_wake_word() {
        let detector = WakeWordDetector::new("marlbot");
        // "ma rebot" → "marebot" = dist 1 from "marlbot", starts with 'm'
        assert!(detector.detect("il ma rebot comment ça va"));
        // "mar ebot" → "marebot" = dist 1
        assert!(detector.detect("hey mar ebot dis moi"));
    }

    #[test]
    fn test_detect_pair_rejects_false_positives() {
        let detector = WakeWordDetector::new("marlbot");
        // "ma robe" → "marobe" = dist 3 from "marlbot" — pair fuzzy uses max 2, should reject
        assert!(!detector.detect("ma robe est belle"));
        // Normal speech should not trigger
        assert!(!detector.detect("comme même ici"));
        assert!(!detector.detect("ma lecture est finie"));
    }
}
