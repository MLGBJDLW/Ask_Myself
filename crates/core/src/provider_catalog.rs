//! Shared provider/model preset catalog.
//!
//! The desktop UI and backend both read `shared/provider-presets.json` so
//! provider defaults do not drift between TypeScript and Rust.

use serde::{Deserialize, Serialize};

use crate::llm::ProviderType;
use crate::provider_registry::provider_type_from_key;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetCapability {
    pub enabled: bool,
    #[serde(default)]
    pub default_tokens: Option<u32>,
    #[serde(default)]
    pub min_tokens: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub step: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningCapability {
    #[serde(default)]
    pub effort_levels: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub thinking_budget: Option<ThinkingBudgetCapability>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(skip)]
    reasoning_declared: bool,
    #[serde(skip)]
    vision_declared: bool,
}

impl<'de> Deserialize<'de> for ProviderCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawProviderCapabilities {
            #[serde(default)]
            reasoning: Option<ReasoningCapability>,
            #[serde(default)]
            vision: Option<bool>,
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        let reasoning_declared = value.get("reasoning").is_some();
        let vision_declared = value.get("vision").is_some();
        let raw = RawProviderCapabilities::deserialize(value).map_err(serde::de::Error::custom)?;

        Ok(Self {
            reasoning: raw.reasoning,
            vision: raw.vision,
            reasoning_declared,
            vision_declared,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tag_key: Option<String>,
    #[serde(default)]
    pub recommended: Option<bool>,
    #[serde(default)]
    pub capabilities: Option<ProviderCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub models: Vec<ProviderModelPreset>,
    pub requires_api_key: bool,
    pub icon: String,
    pub description: String,
    #[serde(default)]
    pub capabilities: Option<ProviderCapabilities>,
}

const PROVIDER_PRESETS_JSON: &str = include_str!("../../../shared/provider-presets.json");

pub fn load_provider_presets() -> Result<Vec<ProviderPreset>, serde_json::Error> {
    serde_json::from_str(PROVIDER_PRESETS_JSON)
}

pub fn find_provider_preset(provider: &str, base_url: Option<&str>) -> Option<ProviderPreset> {
    let presets = load_provider_presets().ok()?;
    let provider = provider.trim();
    let normalized_base_url = normalize_base_url(base_url);

    if !normalized_base_url.is_empty() {
        if let Some(exact) = presets.iter().find(|preset| {
            preset.provider == provider
                && normalize_base_url(Some(&preset.base_url)) == normalized_base_url
        }) {
            return Some(exact.clone());
        }
    }

    let mut provider_matches = presets
        .into_iter()
        .filter(|preset| preset.provider == provider)
        .collect::<Vec<_>>();
    if provider_matches.len() == 1 {
        provider_matches.pop()
    } else {
        None
    }
}

pub fn preset_model_ids(provider: &str, base_url: Option<&str>) -> Vec<String> {
    find_provider_preset(provider, base_url)
        .map(|preset| preset.models.into_iter().map(|model| model.id).collect())
        .unwrap_or_default()
}

pub fn model_capabilities_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<ProviderCapabilities> {
    let model = normalize_model_id(model);
    if model.is_empty() {
        return None;
    }

    load_provider_presets()
        .ok()?
        .into_iter()
        .find_map(|preset| {
            let preset_provider_type = provider_type_from_key(&preset.provider)?;
            if preset_provider_type != provider_type {
                return None;
            }
            let model_preset = preset
                .models
                .iter()
                .find(|candidate| normalize_model_id(&candidate.id) == model)?;
            Some(merge_capabilities(
                preset.capabilities.as_ref(),
                model_preset.capabilities.as_ref(),
            ))
        })
}

pub fn model_supports_reasoning_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<bool> {
    model_capabilities_from_catalog(provider_type, model)
        .map(|capabilities| capabilities.reasoning.is_some())
}

pub fn model_supports_vision_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<bool> {
    model_capabilities_from_catalog(provider_type, model)
        .and_then(|capabilities| capabilities.vision)
}

fn merge_capabilities(
    provider_capabilities: Option<&ProviderCapabilities>,
    model_capabilities: Option<&ProviderCapabilities>,
) -> ProviderCapabilities {
    let mut merged = ProviderCapabilities::default();

    if let Some(capabilities) = provider_capabilities {
        if capabilities.reasoning_declared {
            merged.reasoning = capabilities.reasoning.clone();
            merged.reasoning_declared = true;
        }
        if capabilities.vision_declared {
            merged.vision = capabilities.vision;
            merged.vision_declared = true;
        }
    }

    if let Some(capabilities) = model_capabilities {
        if capabilities.reasoning_declared {
            merged.reasoning = capabilities.reasoning.clone();
            merged.reasoning_declared = true;
        }
        if capabilities.vision_declared {
            merged.vision = capabilities.vision;
            merged.vision_declared = true;
        }
    }

    merged
}

fn normalize_base_url(base_url: Option<&str>) -> String {
    base_url
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn normalize_model_id(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_catalog_uses_v4_models() {
        let deepseek = find_provider_preset("deep_seek", Some("https://api.deepseek.com/v1"))
            .expect("deepseek preset should match by provider fallback");
        let ids = deepseek
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"deepseek-v4-pro"));
        assert!(ids.contains(&"deepseek-v4-flash"));
        assert!(!ids.contains(&"deepseek-reasoner"));
        assert!(!ids.contains(&"deepseek-chat"));

        let pro = deepseek
            .models
            .iter()
            .find(|model| model.id == "deepseek-v4-pro")
            .expect("deepseek-v4-pro should be listed");
        let reasoning = pro
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("deepseek-v4-pro should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec!["high".to_string(), "max".to_string()]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn openai_catalog_defaults_to_gpt_56() {
        let openai = find_provider_preset("open_ai", Some("https://api.openai.com/v1"))
            .expect("openai preset should match");
        let ids = openai
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"gpt-5.6"));
        assert!(ids.contains(&"gpt-5.6-sol"));
        assert!(ids.contains(&"gpt-5.6-terra"));
        assert!(ids.contains(&"gpt-5.6-luna"));
        assert!(ids.contains(&"gpt-5.4"));
        assert!(ids.contains(&"gpt-5.4-mini"));
        assert!(ids.contains(&"gpt-5.4-nano"));

        let gpt_56 = openai
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6")
            .expect("gpt-5.6 should be listed");
        assert_eq!(gpt_56.recommended, Some(true));

        let reasoning = gpt_56
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("gpt-5.6 should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "none".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
                "max".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn anthropic_catalog_defaults_to_fable_5() {
        let anthropic = find_provider_preset("anthropic", Some("https://api.anthropic.com/v1"))
            .expect("anthropic preset should match");
        let ids = anthropic
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"claude-fable-5"));
        assert!(ids.contains(&"claude-mythos-5"));
        assert!(ids.contains(&"claude-sonnet-5"));
        assert!(ids.contains(&"claude-opus-4-8"));
        assert!(ids.contains(&"claude-opus-4-7"));
        assert!(ids.contains(&"claude-sonnet-4-6"));

        let fable_5 = anthropic
            .models
            .iter()
            .find(|model| model.id == "claude-fable-5")
            .expect("claude-fable-5 should be listed");
        assert_eq!(fable_5.recommended, Some(true));

        let reasoning = fable_5
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("claude-fable-5 should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
                "max".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn qwen_catalog_defaults_to_qwen37_max() {
        let qwen = find_provider_preset(
            "qwen",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        )
        .expect("qwen preset should match");
        let ids = qwen
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"qwen3.7-max"));
        assert!(ids.contains(&"qwen3.7-plus"));
        assert!(ids.contains(&"qwen3.7-max-2026-06-08"));
        assert!(ids.contains(&"qwen3.6-plus"));

        let qwen37 = qwen
            .models
            .iter()
            .find(|model| model.id == "qwen3.7-max")
            .expect("qwen3.7-max should be listed");
        assert_eq!(qwen37.recommended, Some(true));
        assert_eq!(qwen37.tag_key.as_deref(), Some("providers.tagLatest"));
        assert_eq!(
            qwen37
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.vision),
            Some(true)
        );
    }

    #[test]
    fn google_catalog_defaults_to_latest_stable_gemini_models() {
        let google = find_provider_preset(
            "google",
            Some("https://generativelanguage.googleapis.com/v1beta"),
        )
        .expect("Google preset should match");
        let ids = google
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"gemini-3.6-flash"));
        assert!(ids.contains(&"gemini-3.5-flash-lite"));
        assert!(!ids.contains(&"gemini-3-pro-preview"));
        assert!(!ids.contains(&"gemini-2.0-flash"));
        assert!(!ids.contains(&"gemini-2.0-flash-lite"));

        let latest = google
            .models
            .first()
            .expect("Gemini 3.6 Flash should be listed first");
        assert_eq!(latest.recommended, Some(true));
        let reasoning = latest
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("Gemini 3.6 Flash should expose reasoning controls");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "minimal".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("medium"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn openrouter_catalog_uses_openrouter_provider_type() {
        let openrouter = find_provider_preset("openrouter", Some("https://openrouter.ai/api/v1"))
            .expect("openrouter preset should match");
        let ids = openrouter
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"~anthropic/claude-fable-latest"));
        assert!(ids.contains(&"anthropic/claude-fable-5"));
        assert!(ids.contains(&"openai/gpt-5.6-sol"));
        assert!(ids.contains(&"x-ai/grok-4.5"));
        assert!(ids.contains(&"anthropic/claude-sonnet-5"));
        assert!(ids.contains(&"z-ai/glm-5.2"));
        assert!(ids.contains(&"moonshotai/kimi-k2.7-code"));
        assert!(ids.contains(&"qwen/qwen3.7-plus"));
        assert!(ids.contains(&"x-ai/grok-build-0.1"));
        assert!(ids.contains(&"anthropic/claude-sonnet-4.6"));
        let codex = openrouter
            .models
            .iter()
            .find(|model| model.id == "openai/gpt-5.3-codex")
            .expect("OpenRouter should include duplicate native Codex model");
        assert_eq!(codex.tag_key.as_deref(), Some("providers.tagCoding"));

        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenRouter, "x-ai/grok-4.3"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.7-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.7-max"),
            Some(false)
        );
    }

    #[test]
    fn openai_compatible_presets_match_exact_base_urls() {
        let xai = find_provider_preset("open_ai", Some("https://api.x.ai/v1/"))
            .expect("xAI preset should match its exact base URL");
        assert_eq!(
            xai.models.first().map(|model| model.id.as_str()),
            Some("grok-4.5")
        );

        let minimax = find_provider_preset("open_ai", Some("https://api.minimax.io/v1"))
            .expect("MiniMax preset should match its exact base URL");
        assert_eq!(
            minimax.models.first().map(|model| model.id.as_str()),
            Some("MiniMax-M3")
        );

        let zhipu = find_provider_preset("zhipu", Some("https://open.bigmodel.cn/api/paas/v4"))
            .expect("Zhipu preset should match");
        assert_eq!(
            zhipu.models.first().map(|model| model.id.as_str()),
            Some("glm-5.2")
        );
        let glm52_reasoning = zhipu.models[0]
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("GLM-5.2 should expose reasoning controls");
        assert!(glm52_reasoning.effort_levels.contains(&"max".to_string()));
    }

    #[test]
    fn provider_catalog_drives_vision_capabilities() {
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenAi, "gpt-5.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::DeepSeek, "deepseek-v4-pro"),
            Some(false)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::Qwen, "qwen3-vl-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::Qwen, "qwen3.6-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::Doubao, "doubao-seed-2.0-pro"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenAi, "grok-4.3"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::LmStudio, "custom-vl-model"),
            None
        );
    }

    #[test]
    fn provider_catalog_drives_reasoning_capabilities() {
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "gpt-5.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "gpt-5.5-pro"),
            Some(false)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::DeepSeek, "deepseek-v4-pro"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::Anthropic, "claude-fable-5"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::Qwen, "qwen3.6-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "grok-4.3"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(
                ProviderType::OpenAi,
                "grok-4.20-0309-non-reasoning"
            ),
            Some(false)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::LmStudio, "custom-reasoner"),
            None
        );
    }
}
