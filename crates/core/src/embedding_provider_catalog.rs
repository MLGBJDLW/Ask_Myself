//! Shared embedding provider/model preset catalog.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProviderPreset {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub description: String,
    pub models: Vec<EmbeddingModelPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelPreset {
    pub id: String,
    pub name: String,
    pub dimensions: usize,
    pub supports_dimension_override: bool,
    #[serde(default)]
    pub recommended: bool,
}

const EMBEDDING_PROVIDER_PRESETS_JSON: &str =
    include_str!("../../../shared/embedding-provider-presets.json");

pub fn load_embedding_provider_presets() -> Result<Vec<EmbeddingProviderPreset>, serde_json::Error>
{
    serde_json::from_str(EMBEDDING_PROVIDER_PRESETS_JSON)
}

pub fn find_embedding_model(base_url: &str, model: &str) -> Option<EmbeddingModelPreset> {
    let normalized_base_url = normalize_base_url(base_url);
    let model = model.trim();
    load_embedding_provider_presets()
        .ok()?
        .into_iter()
        .find(|preset| normalize_base_url(&preset.base_url) == normalized_base_url)?
        .models
        .into_iter()
        .find(|candidate| candidate.id == model)
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_catalog_is_valid_and_has_recommended_models() {
        let presets = load_embedding_provider_presets().expect("valid embedding provider catalog");
        assert!(presets.len() >= 5);
        for preset in presets.iter().filter(|preset| preset.id != "custom") {
            assert!(preset.models.iter().any(|model| model.recommended));
        }
    }

    #[test]
    fn exact_provider_and_model_controls_dimension_override() {
        let openai = find_embedding_model("https://api.openai.com/v1/", "text-embedding-3-small")
            .expect("openai model");
        assert!(openai.supports_dimension_override);

        let mistral = find_embedding_model("https://api.mistral.ai/v1", "mistral-embed")
            .expect("mistral model");
        assert!(!mistral.supports_dimension_override);
        assert_eq!(mistral.dimensions, 1024);

        let qwen = find_embedding_model(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3.7-text-embedding",
        )
        .expect("qwen model");
        assert!(qwen.supports_dimension_override);
        assert_eq!(qwen.dimensions, 1024);
    }
}
