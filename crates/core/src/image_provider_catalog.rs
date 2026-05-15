//! Shared image provider/model preset catalog.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageProviderPreset {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub api_style: String,
    pub base_url: String,
    pub requires_api_key: bool,
    pub description: String,
    pub models: Vec<ImageModelPreset>,
    pub size_options: Vec<ImageSizeOption>,
    pub quality_options: Vec<String>,
    pub output_formats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageModelPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSizeOption {
    pub value: String,
    pub label: String,
}

const IMAGE_PROVIDER_PRESETS_JSON: &str =
    include_str!("../../../shared/image-provider-presets.json");

pub fn load_image_provider_presets() -> Result<Vec<ImageProviderPreset>, serde_json::Error> {
    serde_json::from_str(IMAGE_PROVIDER_PRESETS_JSON)
}

pub fn find_image_provider_preset(
    provider: &str,
    api_style: Option<&str>,
    base_url: Option<&str>,
) -> Option<ImageProviderPreset> {
    let presets = load_image_provider_presets().ok()?;
    let provider = provider.trim();
    let api_style = api_style.map(str::trim).unwrap_or("");
    let normalized_base_url = normalize_base_url(base_url);

    if !normalized_base_url.is_empty() {
        if let Some(preset) = presets.iter().find(|preset| {
            preset.provider == provider
                && preset.api_style == api_style
                && normalize_base_url(Some(&preset.base_url)) == normalized_base_url
        }) {
            return Some(preset.clone());
        }
    }

    let mut provider_matches = presets
        .into_iter()
        .filter(|preset| {
            preset.provider == provider && (api_style.is_empty() || preset.api_style == api_style)
        })
        .collect::<Vec<_>>();
    if provider_matches.len() == 1 {
        provider_matches.pop()
    } else {
        None
    }
}

pub fn default_image_model(
    provider: &str,
    api_style: Option<&str>,
    base_url: Option<&str>,
) -> Option<String> {
    let preset = find_image_provider_preset(provider, api_style, base_url)?;
    preset
        .models
        .iter()
        .find(|model| model.recommended)
        .or_else(|| preset.models.first())
        .map(|model| model.id.clone())
}

pub fn default_image_base_url(provider: &str, api_style: Option<&str>) -> Option<String> {
    let presets = load_image_provider_presets().ok()?;
    let provider = provider.trim();
    let api_style = api_style.map(str::trim).unwrap_or("");
    presets
        .into_iter()
        .find(|preset| {
            preset.provider == provider && (api_style.is_empty() || preset.api_style == api_style)
        })
        .map(|preset| preset.base_url)
}

fn normalize_base_url(base_url: Option<&str>) -> String {
    base_url
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_lowercase()
}
