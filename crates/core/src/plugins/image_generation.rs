use reqwest::Url;
use serde_json::json;

use crate::app_settings::ImageGenerationConfig;
use crate::image_provider_catalog::{
    default_image_base_url, find_image_provider_preset, load_image_provider_presets,
};

use super::{
    PluginCheckSeverity, PluginManifest, PluginProviderCatalog, PluginRuntimeCheck,
    PluginRuntimeStatus, PluginSettingsField, PluginSettingsSchema,
};

pub(super) fn enrich_manifest(
    mut manifest: PluginManifest,
    config: Option<&ImageGenerationConfig>,
) -> PluginManifest {
    manifest.settings_schema = Some(settings_schema());
    manifest.provider_catalogs = vec![provider_catalog()];
    manifest.runtime_checks = runtime_checks(config);
    manifest
}

fn provider_catalog() -> PluginProviderCatalog {
    let items = load_image_provider_presets()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|preset| serde_json::to_value(preset).ok())
        .collect();

    PluginProviderCatalog {
        id: "imageProviders".to_string(),
        label: "Image providers".to_string(),
        item_kind: "imageProviderPreset".to_string(),
        items,
    }
}

fn settings_schema() -> PluginSettingsSchema {
    let defaults = ImageGenerationConfig::default();
    PluginSettingsSchema {
        config_key: "imageGeneration".to_string(),
        fields: vec![
            field(
                "providerPreset",
                "Provider",
                "select",
                true,
                false,
                "Provider preset used to fill provider, API style, base URL, and default model.",
                Some("imageProviders"),
                None,
            ),
            field(
                "apiKey",
                "API key",
                "secret",
                true,
                true,
                "Secret used only by the image generation provider.",
                None,
                Some(json!("")),
            ),
            field(
                "baseUrl",
                "Base URL",
                "url",
                true,
                false,
                "Endpoint base URL for the selected image provider.",
                None,
                defaults.base_url.map(serde_json::Value::String),
            ),
            field(
                "model",
                "Model",
                "string",
                true,
                false,
                "Default image generation model.",
                Some("imageProviders.models"),
                Some(json!(defaults.model)),
            ),
            field(
                "size",
                "Default size",
                "select",
                false,
                false,
                "Default output size or aspect-ratio option.",
                Some("imageProviders.sizeOptions"),
                defaults.size.map(serde_json::Value::String),
            ),
            field(
                "quality",
                "Quality",
                "select",
                false,
                false,
                "Provider-specific quality hint.",
                Some("imageProviders.qualityOptions"),
                None,
            ),
            field(
                "outputFormat",
                "Output format",
                "select",
                false,
                false,
                "Preferred file format for providers that support multiple formats.",
                Some("imageProviders.outputFormats"),
                defaults.output_format.map(serde_json::Value::String),
            ),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn field(
    key: &str,
    label: &str,
    kind: &str,
    required: bool,
    secret: bool,
    description: &str,
    options_source: Option<&str>,
    default_value: Option<serde_json::Value>,
) -> PluginSettingsField {
    PluginSettingsField {
        key: key.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        required,
        secret,
        description: description.to_string(),
        options_source: options_source.map(str::to_string),
        default_value,
    }
}

fn runtime_checks(config: Option<&ImageGenerationConfig>) -> Vec<PluginRuntimeCheck> {
    let Some(config) = config else {
        return vec![check(
            "configuration",
            "Configuration",
            PluginRuntimeStatus::Unknown,
            PluginCheckSeverity::Info,
            "Image generation settings have not been loaded yet.",
        )];
    };

    vec![
        provider_check(config),
        api_key_check(config),
        base_url_check(config),
        model_check(config),
    ]
}

fn provider_check(config: &ImageGenerationConfig) -> PluginRuntimeCheck {
    let preset = find_image_provider_preset(
        &config.provider,
        Some(config.api_style.as_str()),
        config.base_url.as_deref(),
    );
    if preset.is_some() {
        check(
            "provider-preset",
            "Provider preset",
            PluginRuntimeStatus::Pass,
            PluginCheckSeverity::Info,
            "Provider, API style, and base URL match a known image provider preset.",
        )
    } else {
        check(
            "provider-preset",
            "Provider preset",
            PluginRuntimeStatus::Warning,
            PluginCheckSeverity::Warning,
            "This image provider is custom or does not match the shared provider catalog.",
        )
    }
}

fn api_key_check(config: &ImageGenerationConfig) -> PluginRuntimeCheck {
    if config.api_key.trim().is_empty() {
        check(
            "api-key",
            "API key",
            PluginRuntimeStatus::Error,
            PluginCheckSeverity::Error,
            "Image generation needs a provider API key before generate_image can run.",
        )
    } else {
        check(
            "api-key",
            "API key",
            PluginRuntimeStatus::Pass,
            PluginCheckSeverity::Info,
            "An image provider API key is configured.",
        )
    }
}

fn base_url_check(config: &ImageGenerationConfig) -> PluginRuntimeCheck {
    let base_url = config
        .base_url
        .clone()
        .or_else(|| default_image_base_url(&config.provider, Some(config.api_style.as_str())))
        .unwrap_or_default();
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return check(
            "base-url",
            "Base URL",
            PluginRuntimeStatus::Error,
            PluginCheckSeverity::Error,
            "Image generation needs a base URL for the selected provider.",
        );
    }

    match Url::parse(trimmed) {
        Ok(url) if matches!(url.scheme(), "https" | "http") => check(
            "base-url",
            "Base URL",
            PluginRuntimeStatus::Pass,
            PluginCheckSeverity::Info,
            "The image provider endpoint is a valid HTTP URL.",
        ),
        _ => check(
            "base-url",
            "Base URL",
            PluginRuntimeStatus::Error,
            PluginCheckSeverity::Error,
            "The image provider base URL is invalid.",
        ),
    }
}

fn model_check(config: &ImageGenerationConfig) -> PluginRuntimeCheck {
    let model = config.model.trim();
    if model.is_empty() {
        return check(
            "model",
            "Model",
            PluginRuntimeStatus::Error,
            PluginCheckSeverity::Error,
            "Image generation needs a default model.",
        );
    }

    let preset = find_image_provider_preset(
        &config.provider,
        Some(config.api_style.as_str()),
        config.base_url.as_deref(),
    );
    let Some(preset) = preset else {
        return check(
            "model",
            "Model",
            PluginRuntimeStatus::Warning,
            PluginCheckSeverity::Warning,
            "A custom image model is configured outside the shared provider catalog.",
        );
    };
    if preset.models.is_empty() || preset.models.iter().any(|candidate| candidate.id == model) {
        check(
            "model",
            "Model",
            PluginRuntimeStatus::Pass,
            PluginCheckSeverity::Info,
            "The configured model is available for the selected image provider.",
        )
    } else {
        check(
            "model",
            "Model",
            PluginRuntimeStatus::Warning,
            PluginCheckSeverity::Warning,
            "The configured model is custom for the selected image provider.",
        )
    }
}

fn check(
    id: &str,
    label: &str,
    status: PluginRuntimeStatus,
    severity: PluginCheckSeverity,
    message: &str,
) -> PluginRuntimeCheck {
    PluginRuntimeCheck {
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
    fn image_manifest_carries_provider_catalog_and_settings_schema() {
        let manifest = enrich_manifest(
            PluginManifest {
                id: "image-generation".to_string(),
                name: "Image Generation".to_string(),
                capability: "Image creation".to_string(),
                description: "test".to_string(),
                built_in: true,
                tools: vec!["generate_image".to_string()],
                settings_surfaces: vec!["image-generation".to_string()],
                workflows: vec!["generate-image".to_string()],
                settings_schema: None,
                provider_catalogs: Vec::new(),
                runtime_checks: Vec::new(),
            },
            Some(&ImageGenerationConfig::default()),
        );

        assert!(manifest
            .settings_schema
            .as_ref()
            .is_some_and(|schema| schema.config_key == "imageGeneration"));
        assert!(manifest
            .provider_catalogs
            .iter()
            .any(|catalog| catalog.id == "imageProviders" && !catalog.items.is_empty()));
        assert!(manifest
            .runtime_checks
            .iter()
            .any(|check| check.id == "api-key" && check.status == PluginRuntimeStatus::Error));
    }
}
