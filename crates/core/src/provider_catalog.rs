//! Shared provider/model preset catalog.
//!
//! The desktop UI and backend both read `shared/provider-presets.json` so
//! provider defaults do not drift between TypeScript and Rust.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,
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

fn normalize_base_url(base_url: Option<&str>) -> String {
    base_url
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase()
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
        assert!(ids.contains(&"deepseek-reasoner"));
        assert!(ids.contains(&"deepseek-chat"));

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
    fn openai_catalog_defaults_to_gpt_55() {
        let openai = find_provider_preset("open_ai", Some("https://api.openai.com/v1"))
            .expect("openai preset should match");
        let ids = openai
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"gpt-5.5"));
        assert!(ids.contains(&"gpt-5.5-pro"));
        assert!(ids.contains(&"gpt-5.4"));

        let gpt_55 = openai
            .models
            .iter()
            .find(|model| model.id == "gpt-5.5")
            .expect("gpt-5.5 should be listed");
        assert_eq!(gpt_55.recommended, Some(true));

        let reasoning = gpt_55
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("gpt-5.5 should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "none".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("medium"));
    }
}
