use std::collections::HashMap;
use anyhow::Result;
use tracing::{info, warn};

use super::{TtsAudio, TtsSynthesizer};
use super::http::HttpTtsSynthesizer;
use super::elevenlabs::ElevenLabsTtsSynthesizer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProviderKind {
    OpenAI,
    ElevenLabs,
}

/// Maps friendly voice names to (provider, provider-specific voice ID).
/// Routes TTS requests to the correct backend automatically.
pub struct TtsRegistry {
    openai: HttpTtsSynthesizer,
    elevenlabs: Option<ElevenLabsTtsSynthesizer>,
    /// display_name (lowercase) → (provider, provider_voice_id)
    voice_map: HashMap<String, (ProviderKind, String)>,
    /// Ordered list per provider for display
    openai_voices: Vec<String>,
    elevenlabs_voices: Vec<String>,
}

/// Fetch voices from ElevenLabs API, filtered to FR voices + cloned voices.
/// Returns Vec<(friendly_name, voice_id)>.
pub fn fetch_elevenlabs_voices(api_key: &str) -> Vec<(String, String)> {
    let client = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .build();

    let response = match client
        .get("https://api.elevenlabs.io/v1/voices")
        .set("xi-api-key", api_key)
        .call()
    {
        Ok(resp) => resp,
        Err(e) => {
            warn!("Failed to fetch ElevenLabs voices: {}", e);
            return Vec::new();
        }
    };

    let body: serde_json::Value = match response.into_json() {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse ElevenLabs voices response: {}", e);
            return Vec::new();
        }
    };

    let mut voices: Vec<(String, String)> = Vec::new();
    if let Some(arr) = body.get("voices").and_then(|v| v.as_array()) {
        for voice in arr {
            let voice_id = voice.get("voice_id").and_then(|v| v.as_str()).unwrap_or_default();
            let name = voice.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let category = voice.get("category").and_then(|v| v.as_str()).unwrap_or_default();
            let labels = voice.get("labels").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
            let language = labels.get("language").and_then(|v| v.as_str()).unwrap_or_default();

            if voice_id.is_empty() || name.is_empty() {
                continue;
            }

            // Include: cloned voices + voices with French language
            let is_french = language.starts_with("fr");
            let is_cloned = category == "cloned";

            if is_french || is_cloned {
                // Build a clean friendly name: take first word, lowercase, strip non-alphanumeric
                let friendly = name
                    .split(|c: char| c == ' ' || c == '-' || c == '´')
                    .next()
                    .unwrap_or(name)
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>();

                if !friendly.is_empty() {
                    voices.push((friendly, voice_id.to_string()));
                }
            }
        }
    }

    voices.sort_by(|a, b| a.0.cmp(&b.0));
    // Deduplicate by friendly name (keep first)
    voices.dedup_by(|a, b| a.0 == b.0);

    info!("Fetched {} ElevenLabs voices (FR + clones)", voices.len());
    for (name, id) in &voices {
        info!("  ElevenLabs voice: {} → {}", name, id);
    }

    voices
}

impl TtsRegistry {
    pub fn new(
        openai: HttpTtsSynthesizer,
        elevenlabs: Option<ElevenLabsTtsSynthesizer>,
        openai_voice_names: Vec<String>,
        elevenlabs_api_key: Option<&str>,
    ) -> Self {
        let mut voice_map = HashMap::new();

        // Register OpenAI voices (name == provider voice id)
        let mut openai_voices: Vec<String> = Vec::new();
        for name in &openai_voice_names {
            let lower = name.to_lowercase();
            voice_map.insert(lower.clone(), (ProviderKind::OpenAI, lower.clone()));
            openai_voices.push(lower);
        }

        // Register ElevenLabs voices if available (dynamic fetch from API)
        let mut elevenlabs_voice_names: Vec<String> = Vec::new();
        if elevenlabs.is_some() {
            if let Some(api_key) = elevenlabs_api_key {
                let fetched = fetch_elevenlabs_voices(api_key);
                for (friendly_name, voice_id) in fetched {
                    // Skip if name conflicts with an OpenAI voice
                    if voice_map.contains_key(&friendly_name) {
                        warn!("ElevenLabs voice '{}' conflicts with OpenAI voice, skipping", friendly_name);
                        continue;
                    }
                    voice_map.insert(friendly_name.clone(), (ProviderKind::ElevenLabs, voice_id));
                    elevenlabs_voice_names.push(friendly_name);
                }
            }
        }

        info!(
            "TtsRegistry initialized: {} OpenAI voices, {} ElevenLabs voices",
            openai_voices.len(),
            elevenlabs_voice_names.len()
        );

        Self {
            openai,
            elevenlabs,
            voice_map,
            openai_voices,
            elevenlabs_voices: elevenlabs_voice_names,
        }
    }

    /// All valid voice names (sorted).
    pub fn valid_voices(&self) -> Vec<String> {
        let mut all: Vec<String> = self.voice_map.keys().cloned().collect();
        all.sort();
        all
    }

    /// Voices grouped by provider: Vec<(provider_name, sorted_voices)>.
    pub fn voices_by_provider(&self) -> Vec<(String, Vec<String>)> {
        let mut result = vec![("OpenAI".to_string(), self.openai_voices.clone())];
        if !self.elevenlabs_voices.is_empty() {
            result.push(("ElevenLabs".to_string(), self.elevenlabs_voices.clone()));
        }
        result
    }

    /// Get provider name for a voice.
    pub fn provider_for_voice(&self, name: &str) -> Option<&'static str> {
        self.voice_map.get(&name.to_lowercase()).map(|(kind, _)| match kind {
            ProviderKind::OpenAI => "OpenAI",
            ProviderKind::ElevenLabs => "ElevenLabs",
        })
    }

    /// Check if a voice name is valid.
    pub fn is_valid_voice(&self, name: &str) -> bool {
        self.voice_map.contains_key(&name.to_lowercase())
    }
}

impl TtsSynthesizer for TtsRegistry {
    fn synthesize(&self, text: &str, voice: Option<&str>, speed: Option<f32>) -> Result<TtsAudio> {
        let voice_name = voice.unwrap_or_else(|| self.openai.default_voice());
        let lower = voice_name.to_lowercase();

        if let Some((kind, provider_voice_id)) = self.voice_map.get(&lower) {
            match kind {
                ProviderKind::OpenAI => {
                    self.openai.synthesize(text, Some(provider_voice_id), speed)
                }
                ProviderKind::ElevenLabs => {
                    if let Some(ref el) = self.elevenlabs {
                        el.synthesize(text, Some(provider_voice_id), speed)
                    } else {
                        anyhow::bail!("ElevenLabs not configured but voice '{}' requested", voice_name)
                    }
                }
            }
        } else {
            // Fallback: try OpenAI with the raw voice name
            self.openai.synthesize(text, voice, speed)
        }
    }

    fn name(&self) -> &str {
        "TTS Registry"
    }

    fn default_voice(&self) -> &str {
        self.openai.default_voice()
    }
}
