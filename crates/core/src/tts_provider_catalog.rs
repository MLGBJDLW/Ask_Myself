//! Shared text-to-speech provider/model preset catalog.

use serde::{Deserialize, Serialize};

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
}

const TTS_PROVIDER_PRESETS_JSON: &str = include_str!("../../../shared/tts-provider-presets.json");

pub fn load_tts_provider_presets() -> Result<Vec<TtsProviderPreset>, serde_json::Error> {
    serde_json::from_str(TTS_PROVIDER_PRESETS_JSON)
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
}
