//! Shared text-to-speech provider/model preset catalog and live voice discovery.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app_settings::TextToSpeechConfig;
use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsProviderPreset {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub api_style: String,
    #[serde(default = "default_true")]
    pub requires_api_key: bool,
    #[serde(default)]
    pub local: bool,
    pub base_url: String,
    pub description: String,
    pub models: Vec<TtsCatalogItem>,
    pub voices: Vec<TtsCatalogItem>,
    pub output_formats: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsCatalogItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub preview_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TtsVoiceCatalogEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub recommended: bool,
    pub source: String,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub preview_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TtsVoiceCatalogSnapshot {
    pub provider: String,
    pub api_style: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    pub voices: Vec<TtsVoiceCatalogEntry>,
    pub refreshed_at: String,
    pub live_discovery_succeeded: bool,
}

const TTS_PROVIDER_PRESETS_JSON: &str = include_str!("../../../shared/tts-provider-presets.json");

pub fn load_tts_provider_presets() -> Result<Vec<TtsProviderPreset>, serde_json::Error> {
    serde_json::from_str(TTS_PROVIDER_PRESETS_JSON)
}

pub fn supports_dynamic_tts_voice_catalog(api_style: &str) -> bool {
    matches!(
        api_style.trim(),
        "elevenlabs_speech" | "azure_speech" | "dashscope_speech" | "minimax_speech"
    )
}

pub fn build_tts_voice_catalog(
    config: &TextToSpeechConfig,
    discovered: Option<Vec<TtsVoiceCatalogEntry>>,
    refreshed_at: impl Into<String>,
) -> TtsVoiceCatalogSnapshot {
    let presets = load_tts_provider_presets().unwrap_or_default();
    let curated = presets
        .iter()
        .find(|preset| preset.provider == config.provider && preset.api_style == config.api_style)
        .map(|preset| preset.voices.as_slice())
        .unwrap_or_default();
    let mut emitted = HashSet::new();
    let mut voices = Vec::new();

    if let Some(live_voices) = discovered.as_ref() {
        for voice in live_voices {
            let normalized = normalize_voice_id(&voice.id);
            if normalized.is_empty() || !voice_matches_model(voice, &config.model) {
                continue;
            }
            if emitted.insert(normalized) {
                voices.push(voice.clone());
            }
        }
    }

    for voice in curated {
        let normalized = normalize_voice_id(&voice.id);
        if normalized.is_empty()
            || !catalog_item_matches_model(voice, &config.model)
            || !emitted.insert(normalized)
        {
            continue;
        }
        voices.push(TtsVoiceCatalogEntry {
            id: voice.id.clone(),
            name: voice.name.clone(),
            recommended: voice.recommended,
            source: "curated".to_string(),
            model_ids: voice.model_ids.clone(),
            languages: voice.languages.clone(),
            gender: voice.gender.clone(),
            description: voice.description.clone(),
            preview_url: voice.preview_url.clone(),
        });
    }

    TtsVoiceCatalogSnapshot {
        provider: config.provider.trim().to_string(),
        api_style: config.api_style.trim().to_string(),
        base_url: config
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        model: config.model.trim().to_string(),
        voices,
        refreshed_at: refreshed_at.into(),
        live_discovery_succeeded: discovered.is_some(),
    }
}

pub async fn discover_tts_voices(
    config: &TextToSpeechConfig,
) -> Result<Vec<TtsVoiceCatalogEntry>, CoreError> {
    if config.api_key.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "A provider API key is required to refresh voices.".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            CoreError::InvalidInput(format!("Failed to build HTTP client: {error}"))
        })?;

    match config.api_style.trim() {
        "elevenlabs_speech" => discover_elevenlabs_voices(&client, config).await,
        "azure_speech" => discover_azure_voices(&client, config).await,
        "dashscope_speech" => discover_dashscope_voices(&client, config).await,
        "minimax_speech" => discover_minimax_voices(&client, config).await,
        value => Err(CoreError::InvalidInput(format!(
            "Provider style '{value}' does not expose a dynamic voice catalog."
        ))),
    }
}

async fn discover_elevenlabs_voices(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
) -> Result<Vec<TtsVoiceCatalogEntry>, CoreError> {
    let root = configured_base_url(config, "https://api.elevenlabs.io/v1");
    let root = root.trim_end_matches('/').trim_end_matches("/v1");
    let endpoint = format!("{root}/v2/voices?page_size=100&include_total_count=false&sort=name");
    let value = read_json_response(
        client
            .get(endpoint)
            .header("xi-api-key", config.api_key.trim())
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| {
                CoreError::TransientLlm(format!("ElevenLabs voice discovery failed: {error}"))
            })?,
        "ElevenLabs",
    )
    .await?;

    Ok(value
        .get("voices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|voice| {
            let id = voice.get("voice_id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let mut languages = voice
                .get("verified_languages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|language| {
                    language
                        .get("locale")
                        .or_else(|| language.get("language"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            languages.sort();
            languages.dedup();
            Some(TtsVoiceCatalogEntry {
                id: id.to_string(),
                name: voice
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                recommended: false,
                source: "discovered".to_string(),
                // ElevenLabs exposes high-quality recommendations rather than
                // a complete compatibility matrix; treating it as a hard
                // filter would hide valid account voices.
                model_ids: Vec::new(),
                languages,
                gender: voice
                    .pointer("/labels/gender")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: voice
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                preview_url: voice
                    .get("preview_url")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect())
}

async fn discover_azure_voices(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
) -> Result<Vec<TtsVoiceCatalogEntry>, CoreError> {
    let base = configured_base_url(
        config,
        "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1",
    );
    let endpoint = if base.contains(".tts.speech.microsoft.com/cognitiveservices/v1") {
        base.replace("/cognitiveservices/v1", "/cognitiveservices/voices/list")
    } else if base.contains("/cognitiveservices/v1") {
        base.replace(
            "/cognitiveservices/v1",
            "/tts/cognitiveservices/voices/list",
        )
    } else {
        format!(
            "{}/tts/cognitiveservices/voices/list",
            base.trim_end_matches('/')
        )
    };
    let value = read_json_response(
        client
            .get(endpoint)
            .header("Ocp-Apim-Subscription-Key", config.api_key.trim())
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| {
                CoreError::TransientLlm(format!("Azure voice discovery failed: {error}"))
            })?,
        "Azure Speech",
    )
    .await?;

    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|voice| {
            let id = voice
                .get("ShortName")
                .or_else(|| voice.get("Name"))?
                .as_str()?
                .trim();
            if id.is_empty() {
                return None;
            }
            Some(TtsVoiceCatalogEntry {
                id: id.to_string(),
                name: voice
                    .get("DisplayName")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                recommended: false,
                source: "discovered".to_string(),
                model_ids: Vec::new(),
                languages: voice
                    .get("Locale")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                gender: voice
                    .get("Gender")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: voice
                    .pointer("/VoiceTag/VoicePersonalities/0")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                preview_url: None,
            })
        })
        .collect())
}

async fn discover_minimax_voices(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
) -> Result<Vec<TtsVoiceCatalogEntry>, CoreError> {
    let base = configured_base_url(config, "https://api.minimax.io/v1");
    let endpoint = format!("{}/get_voice", base.trim_end_matches('/'));
    let value = read_json_response(
        client
            .post(endpoint)
            .bearer_auth(config.api_key.trim())
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({ "voice_type": "all" }))
            .send()
            .await
            .map_err(|error| {
                CoreError::TransientLlm(format!("MiniMax voice discovery failed: {error}"))
            })?,
        "MiniMax",
    )
    .await?;

    let mut voices = Vec::new();
    for key in ["system_voice", "voice_cloning", "voice_generation"] {
        for voice in value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = voice.get("voice_id").and_then(Value::as_str) else {
                continue;
            };
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            voices.push(TtsVoiceCatalogEntry {
                id: id.to_string(),
                name: voice
                    .get("voice_name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                recommended: false,
                source: "discovered".to_string(),
                model_ids: Vec::new(),
                languages: Vec::new(),
                gender: None,
                description: voice
                    .get("description")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(Value::as_str)
                    .map(str::to_string),
                preview_url: None,
            });
        }
    }
    Ok(voices)
}

async fn discover_dashscope_voices(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
) -> Result<Vec<TtsVoiceCatalogEntry>, CoreError> {
    let base = configured_base_url(
        config,
        "https://dashscope.aliyuncs.com/api/v1/services/audio/tts",
    );
    let endpoint = if base.trim_end_matches('/').ends_with("/tts") {
        format!("{}/customization", base.trim_end_matches('/'))
    } else {
        base
    };
    let qwen = config.model.to_ascii_lowercase().contains("qwen");
    let (model, action) = if qwen {
        ("qwen-voice-design", "list")
    } else {
        ("voice-enrollment", "list_voice")
    };
    let value = read_json_response(
        client
            .post(endpoint)
            .bearer_auth(config.api_key.trim())
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({
                "model": model,
                "input": { "action": action, "page_size": 100, "page_index": 0 }
            }))
            .send()
            .await
            .map_err(|error| {
                CoreError::TransientLlm(format!("DashScope voice discovery failed: {error}"))
            })?,
        "DashScope",
    )
    .await?;

    Ok(value
        .pointer("/output/voice_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|voice| {
            if voice
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| !status.eq_ignore_ascii_case("ok"))
            {
                return None;
            }
            let id = voice
                .get("voice_id")
                .or_else(|| voice.get("voice"))?
                .as_str()?
                .trim();
            if id.is_empty() {
                return None;
            }
            Some(TtsVoiceCatalogEntry {
                id: id.to_string(),
                name: voice
                    .get("preferred_name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                recommended: false,
                source: "discovered".to_string(),
                model_ids: voice
                    .get("target_model")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                languages: voice
                    .get("language")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                gender: None,
                description: voice
                    .get("voice_prompt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                preview_url: None,
            })
        })
        .collect())
}

async fn read_json_response(
    mut response: reqwest::Response,
    provider: &str,
) -> Result<Value, CoreError> {
    const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        CoreError::Llm(format!("Failed to read {provider} voice catalog: {error}"))
    })? {
        if bytes.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(CoreError::Llm(format!(
                "{provider} voice catalog exceeds the {MAX_CATALOG_BYTES}-byte safety limit."
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(300).collect());
        return Err(CoreError::Llm(format!(
            "{provider} voice discovery returned HTTP {status}: {message}"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        CoreError::Llm(format!(
            "{provider} returned invalid voice metadata: {error}"
        ))
    })
}

fn configured_base_url(config: &TextToSpeechConfig, fallback: &str) -> String {
    config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn normalize_voice_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn voice_matches_model(voice: &TtsVoiceCatalogEntry, model: &str) -> bool {
    voice.model_ids.is_empty()
        || voice
            .model_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model.trim()))
}

fn catalog_item_matches_model(voice: &TtsCatalogItem, model: &str) -> bool {
    voice.model_ids.is_empty()
        || voice
            .model_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_catalog_has_fast_defaults_and_voices() {
        let presets = load_tts_provider_presets().expect("valid tts provider catalog");
        assert_eq!(presets.len(), 8);
        for preset in presets {
            assert!(preset.models.iter().any(|model| model.recommended));
            assert!(preset.voices.iter().any(|voice| voice.recommended));
        }
        let local = load_tts_provider_presets()
            .expect("valid tts provider catalog")
            .into_iter()
            .find(|preset| preset.id == "sherpa-onnx")
            .expect("sherpa-onnx preset");
        assert!(local.local);
        assert!(!local.requires_api_key);
        let siliconflow = load_tts_provider_presets()
            .expect("valid tts provider catalog")
            .into_iter()
            .find(|preset| preset.id == "siliconflow")
            .expect("SiliconFlow preset");
        assert_eq!(siliconflow.models[0].id, "fnlp/MOSS-TTSD-v0.5");
        assert_eq!(siliconflow.voices[0].id, "fnlp/MOSS-TTSD-v0.5:alex");
    }

    #[test]
    fn catalog_filters_curated_voices_by_selected_model() {
        let mut config = TextToSpeechConfig::default();
        config.provider = "groq".into();
        config.api_style = "openai_speech".into();
        config.base_url = Some("https://api.groq.com/openai/v1".into());
        config.model = "canopylabs/orpheus-arabic-saudi".into();

        let snapshot = build_tts_voice_catalog(&config, None, "2026-07-31T09:00:00Z");
        assert!(!snapshot.live_discovery_succeeded);
        assert!(snapshot.voices.iter().any(|voice| voice.id == "fahad"));
        assert!(snapshot.voices.iter().any(|voice| voice.id == "lulwa"));
        assert!(!snapshot.voices.iter().any(|voice| voice.id == "hannah"));
    }

    #[test]
    fn live_account_voice_precedes_curated_fallbacks() {
        let mut config = TextToSpeechConfig::default();
        config.provider = "minimax".into();
        config.api_style = "minimax_speech".into();
        config.base_url = Some("https://api.minimax.io/v1".into());
        config.model = "speech-2.8-turbo".into();
        let live = TtsVoiceCatalogEntry {
            id: "private-voice".into(),
            name: "Private Voice".into(),
            recommended: false,
            source: "discovered".into(),
            model_ids: Vec::new(),
            languages: vec!["zh-CN".into()],
            gender: None,
            description: None,
            preview_url: None,
        };

        let snapshot = build_tts_voice_catalog(&config, Some(vec![live]), "2026-07-31T09:00:00Z");
        assert!(snapshot.live_discovery_succeeded);
        assert_eq!(snapshot.voices[0].id, "private-voice");
        assert!(snapshot
            .voices
            .iter()
            .any(|voice| voice.id == "male-qn-qingse"));
        assert!(supports_dynamic_tts_voice_catalog("minimax_speech"));
        assert!(!supports_dynamic_tts_voice_catalog("openai_speech"));
    }
}
