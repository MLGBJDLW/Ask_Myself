use serde_json::to_value;

use crate::app_settings::TextToSpeechConfig;
use crate::tts_provider_catalog::load_tts_provider_presets;

use super::{
    CapabilityCheckSeverity, CapabilityPackageView, CapabilityProviderCatalog,
    CapabilityRuntimeCheck, CapabilityRuntimeStatus, CapabilitySettingsField,
    CapabilitySettingsSchema,
};

pub(super) fn enrich_manifest(
    mut manifest: CapabilityPackageView,
    config: Option<&TextToSpeechConfig>,
) -> CapabilityPackageView {
    manifest.settings_schema = Some(settings_schema());
    manifest.provider_catalogs = vec![provider_catalog()];
    manifest.runtime_checks = runtime_checks(config);
    manifest
}

fn provider_catalog() -> CapabilityProviderCatalog {
    let items = load_tts_provider_presets()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|preset| to_value(preset).ok())
        .collect();
    CapabilityProviderCatalog {
        id: "ttsProviders".to_string(),
        label: "Text-to-speech providers".to_string(),
        item_kind: "providerPreset".to_string(),
        items,
    }
}

fn settings_schema() -> CapabilitySettingsSchema {
    CapabilitySettingsSchema {
        config_key: "textToSpeech".to_string(),
        fields: vec![
            field(
                "provider",
                "Provider",
                "select",
                true,
                false,
                Some("ttsProviders"),
            ),
            field("apiKey", "API key", "secret", false, true, None),
            field("baseUrl", "Base URL", "url", false, false, None),
            field(
                "model",
                "Model",
                "select",
                true,
                false,
                Some("ttsProviders.models"),
            ),
            field(
                "voice",
                "Voice",
                "select",
                true,
                false,
                Some("ttsProviders.voices"),
            ),
            field("outputFormat", "Output format", "select", true, false, None),
            field("speed", "Speed", "number", false, false, None),
            field(
                "executablePath",
                "Local executable",
                "text",
                false,
                false,
                None,
            ),
            field("modelPath", "Local model", "text", false, false, None),
            field("tokensPath", "Local tokens", "text", false, false, None),
            field("voicesPath", "Local voices", "text", false, false, None),
            field(
                "dataDir",
                "Local data directory",
                "text",
                false,
                false,
                None,
            ),
            field("lexiconPath", "Local lexicon", "text", false, false, None),
            field("numThreads", "Local threads", "number", false, false, None),
        ],
    }
}

fn field(
    key: &str,
    label: &str,
    kind: &str,
    required: bool,
    secret: bool,
    options_source: Option<&str>,
) -> CapabilitySettingsField {
    CapabilitySettingsField {
        key: key.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        required,
        secret,
        description: String::new(),
        options_source: options_source.map(str::to_string),
        default_value: None,
    }
}

fn runtime_checks(config: Option<&TextToSpeechConfig>) -> Vec<CapabilityRuntimeCheck> {
    let Some(config) = config else {
        return vec![check(
            "configuration",
            "Configuration",
            CapabilityRuntimeStatus::Unknown,
            CapabilityCheckSeverity::Info,
            "Text-to-speech settings have not been loaded.",
        )];
    };
    if config.api_style == "sherpa_onnx" {
        return vec![check(
            "local-runtime",
            "Local runtime",
            if config.is_configured() {
                CapabilityRuntimeStatus::Pass
            } else {
                CapabilityRuntimeStatus::Error
            },
            CapabilityCheckSeverity::Error,
            if config.is_configured() {
                "sherpa-onnx executable and model files are configured."
            } else {
                "Set the sherpa-onnx executable, model, tokens, and any required voices file."
            },
        )];
    }

    vec![
        check(
            "api-key",
            "API key",
            if config.api_key.trim().is_empty() {
                CapabilityRuntimeStatus::Warning
            } else {
                CapabilityRuntimeStatus::Pass
            },
            CapabilityCheckSeverity::Warning,
            if config.api_key.trim().is_empty() {
                "Add a provider API key before synthesizing speech."
            } else {
                "API key is configured."
            },
        ),
        check(
            "model-and-voice",
            "Model and voice",
            if config.model.trim().is_empty() || config.voice.trim().is_empty() {
                CapabilityRuntimeStatus::Error
            } else {
                CapabilityRuntimeStatus::Pass
            },
            CapabilityCheckSeverity::Error,
            if config.model.trim().is_empty() || config.voice.trim().is_empty() {
                "Choose both a speech model and voice."
            } else {
                "Speech model and voice are selected."
            },
        ),
    ]
}

fn check(
    id: &str,
    label: &str,
    status: CapabilityRuntimeStatus,
    severity: CapabilityCheckSeverity,
    message: &str,
) -> CapabilityRuntimeCheck {
    CapabilityRuntimeCheck {
        id: id.to_string(),
        label: label.to_string(),
        status,
        severity,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_catalog_is_exposed_through_manifest_data() {
        let catalog = provider_catalog();
        assert_eq!(catalog.id, "ttsProviders");
        assert_eq!(catalog.items.len(), 8);
    }

    #[test]
    fn local_runtime_does_not_require_an_api_key() {
        let mut config = TextToSpeechConfig::default();
        config.api_style = "sherpa_onnx".to_string();
        config.model = "vits".to_string();
        config.executable_path = Some("sherpa-onnx-offline-tts".to_string());
        config.model_path = Some("model.onnx".to_string());
        config.tokens_path = Some("tokens.txt".to_string());

        let checks = runtime_checks(Some(&config));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, "local-runtime");
        assert_eq!(checks[0].status, CapabilityRuntimeStatus::Pass);
    }
}
