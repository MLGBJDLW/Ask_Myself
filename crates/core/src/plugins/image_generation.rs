use reqwest::Url;
use serde_json::json;

use crate::app_settings::ImageGenerationConfig;
use crate::conversation::AgentConfig;
use crate::db::Database;
use crate::error::CoreError;
use crate::image_provider_catalog::{
    default_image_base_url, default_image_model as catalog_default_image_model,
    find_image_provider_preset, load_image_provider_presets,
};

use super::{
    CapabilityCheckSeverity, CapabilityPackageView, CapabilityProviderCatalog,
    CapabilityRuntimeCheck, CapabilityRuntimeStatus, CapabilitySettingsField,
    CapabilitySettingsSchema,
};

pub(super) fn enrich_manifest(
    mut manifest: CapabilityPackageView,
    config: Option<&ImageGenerationConfig>,
) -> CapabilityPackageView {
    manifest.settings_schema = Some(settings_schema());
    manifest.provider_catalogs = vec![provider_catalog()];
    manifest.runtime_checks = runtime_checks(config);
    manifest
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageProvider {
    OpenAi,
    Xai,
    Google,
    Qwen,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedImageConfig {
    pub(crate) provider: String,
    pub(crate) api_style: Option<String>,
    pub(crate) api_key: String,
    pub(crate) base_url: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) size: Option<String>,
    pub(crate) quality: Option<String>,
    pub(crate) output_format: Option<String>,
}

impl ResolvedImageConfig {
    pub(crate) fn endpoint_base_url(&self, fallback: &str) -> String {
        self.base_url
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| default_image_base_url(&self.provider, self.api_style.as_deref()))
            .unwrap_or_else(|| fallback.to_string())
    }

    pub(crate) fn qwen_endpoint(&self) -> String {
        let default =
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
        let Some(base_url) = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return default.to_string();
        };
        if base_url.ends_with("/generation") {
            return base_url.to_string();
        }
        if base_url.contains("dashscope.aliyuncs.com/compatible-mode")
            || base_url.contains("dashscope-intl.aliyuncs.com/compatible-mode")
        {
            let host = base_url
                .split("/compatible-mode")
                .next()
                .unwrap_or("https://dashscope.aliyuncs.com");
            return format!("{host}/api/v1/services/aigc/multimodal-generation/generation");
        }
        format!(
            "{}/services/aigc/multimodal-generation/generation",
            base_url.trim_end_matches('/')
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImageGenerationRequest<'a> {
    pub(crate) provider_config_id: Option<&'a str>,
    pub(crate) provider: Option<&'a str>,
    pub(crate) api_style: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) output_format: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedImageRuntime {
    pub(crate) provider: ImageProvider,
    pub(crate) provider_name: String,
    pub(crate) model: String,
    pub(crate) output_format: &'static str,
    pub(crate) config: ResolvedImageConfig,
}

pub(crate) fn resolve_runtime(
    db: &Database,
    request: &ImageGenerationRequest<'_>,
) -> Result<ResolvedImageRuntime, CoreError> {
    let config = resolve_config(db, request)?;
    let provider = infer_provider(request, &config);
    let model = request
        .model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            config
                .model
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            catalog_default_image_model(
                &config.provider,
                config.api_style.as_deref(),
                config.base_url.as_deref(),
            )
        })
        .unwrap_or_else(|| default_model(provider).to_string());
    let output_format =
        normalize_output_format(request.output_format.or(config.output_format.as_deref()));
    let output_format = if provider == ImageProvider::Xai {
        "jpeg"
    } else {
        output_format
    };
    let provider_name = provider_artifact_name(provider, &config);

    Ok(ResolvedImageRuntime {
        provider,
        provider_name,
        model,
        output_format,
        config,
    })
}

fn resolve_config(
    db: &Database,
    request: &ImageGenerationRequest<'_>,
) -> Result<ResolvedImageConfig, CoreError> {
    if let Some(id) = request
        .provider_config_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return db.get_agent_config(id).map(agent_config_to_resolved);
    }

    let app_config = db.load_app_config()?;
    let image_config = app_config.image_generation;
    if image_config.is_configured() {
        if let Some(provider) = requested_provider_hint(request) {
            if image_config_matches_provider(&image_config, provider) {
                return Ok(image_config_to_resolved(image_config));
            }
        } else {
            return Ok(image_config_to_resolved(image_config));
        }
    }

    if let Some(provider) = requested_provider_hint(request) {
        let configs = db.list_agent_configs()?;
        if let Some(config) = configs
            .into_iter()
            .find(|config| config_matches_provider(config, provider))
        {
            return Ok(agent_config_to_resolved(config));
        }
        if provider == ImageProvider::Xai {
            return Err(CoreError::InvalidInput(
                "Configure an xAI image provider or select an xAI provider configuration first."
                    .to_string(),
            ));
        }
    }

    db.get_default_agent_config()?
        .map(agent_config_to_resolved)
        .ok_or_else(|| CoreError::InvalidInput("No default provider config is available.".into()))
}

fn image_config_to_resolved(config: ImageGenerationConfig) -> ResolvedImageConfig {
    ResolvedImageConfig {
        provider: config.provider,
        api_style: Some(config.api_style).filter(|value| !value.trim().is_empty()),
        api_key: config.api_key,
        base_url: config.base_url.filter(|value| !value.trim().is_empty()),
        model: Some(config.model).filter(|value| !value.trim().is_empty()),
        size: config.size.filter(|value| !value.trim().is_empty()),
        quality: config.quality.filter(|value| !value.trim().is_empty()),
        output_format: config
            .output_format
            .filter(|value| !value.trim().is_empty()),
    }
}

fn agent_config_to_resolved(config: AgentConfig) -> ResolvedImageConfig {
    let image_model = config
        .image_generation_model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| is_image_generation_model(&config.model).then(|| config.model.clone()));

    ResolvedImageConfig {
        provider: config.provider,
        api_style: None,
        api_key: config.api_key,
        base_url: config.base_url.filter(|value| !value.trim().is_empty()),
        model: image_model,
        size: None,
        quality: None,
        output_format: None,
    }
}

fn requested_provider_hint(request: &ImageGenerationRequest<'_>) -> Option<ImageProvider> {
    let haystack = format!(
        "{} {}",
        request.api_style.unwrap_or(""),
        request.provider.unwrap_or("")
    )
    .to_lowercase();
    provider_hint_from_text(&haystack)
}

fn provider_hint_from_text(haystack: &str) -> Option<ImageProvider> {
    if is_xai_identity(haystack) {
        Some(ImageProvider::Xai)
    } else if haystack.contains("qwen")
        || haystack.contains("dashscope")
        || haystack.contains("aliyun")
        || haystack.contains("alibaba")
    {
        Some(ImageProvider::Qwen)
    } else if haystack.contains("google")
        || haystack.contains("gemini")
        || haystack.contains("nano_banana")
        || haystack.contains("banana")
    {
        Some(ImageProvider::Google)
    } else if haystack.contains("openai")
        || haystack.contains("openai_images")
        || haystack.contains("images_generation")
        || haystack.contains("gpt-image")
        || haystack.contains("zhipu")
        || haystack.contains("bigmodel")
        || haystack.contains("cogview")
        || haystack.contains("glm-image")
    {
        Some(ImageProvider::OpenAi)
    } else {
        None
    }
}

fn is_xai_identity(value: &str) -> bool {
    value.split_whitespace().any(|part| {
        matches!(part, "xai" | "grok" | "xai_images")
            || part.starts_with("grok-imagine-image")
            || Url::parse(part).is_ok_and(|url| url.host_str() == Some("api.x.ai"))
    })
}

fn image_config_matches_provider(config: &ImageGenerationConfig, provider: ImageProvider) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        config.provider,
        config.api_style,
        config.base_url.as_deref().unwrap_or(""),
        config.model
    )
    .to_lowercase();
    match provider {
        ImageProvider::Xai => {
            config.api_style == "xai_images"
                || config.provider == "xai"
                || config
                    .base_url
                    .as_deref()
                    .and_then(|url| Url::parse(url).ok())
                    .is_some_and(|url| url.host_str() == Some("api.x.ai"))
        }
        ImageProvider::OpenAi => {
            haystack.contains("openai")
                || haystack.contains("openai_images")
                || haystack.contains("compatible")
                || haystack.contains("gpt-image")
                || haystack.contains("zhipu")
                || haystack.contains("bigmodel")
                || haystack.contains("cogview")
                || haystack.contains("glm-image")
                || is_image_generation_model(&haystack)
        }
        ImageProvider::Google => {
            haystack.contains("google")
                || haystack.contains("gemini")
                || haystack.contains("generativelanguage.googleapis")
                || haystack.contains("gemini_generate_content")
        }
        ImageProvider::Qwen => {
            haystack.contains("qwen")
                || haystack.contains("dashscope")
                || haystack.contains("aliyun")
                || haystack.contains("alibaba")
                || haystack.contains("dashscope_multimodal")
        }
    }
}

fn config_matches_provider(config: &AgentConfig, provider: ImageProvider) -> bool {
    let haystack = format!(
        "{} {} {}",
        config.provider,
        config.base_url.as_deref().unwrap_or(""),
        config.model
    )
    .to_lowercase();
    match provider {
        ImageProvider::Xai => config
            .base_url
            .as_deref()
            .and_then(|url| Url::parse(url).ok())
            .is_some_and(|url| url.host_str() == Some("api.x.ai")),
        ImageProvider::OpenAi => {
            haystack.contains("openai")
                || haystack.contains("compatible")
                || haystack.contains("gpt-image")
                || haystack.contains("api.openai.com")
                || haystack.contains("zhipu")
                || haystack.contains("bigmodel")
                || haystack.contains("cogview")
                || haystack.contains("glm-image")
                || is_image_generation_model(&haystack)
        }
        ImageProvider::Google => {
            haystack.contains("google")
                || haystack.contains("gemini")
                || haystack.contains("generativelanguage.googleapis")
        }
        ImageProvider::Qwen => {
            haystack.contains("qwen")
                || haystack.contains("dashscope")
                || haystack.contains("aliyun")
                || haystack.contains("alibaba")
        }
    }
}

fn infer_provider(
    request: &ImageGenerationRequest<'_>,
    config: &ResolvedImageConfig,
) -> ImageProvider {
    if let Some(provider) = requested_provider_hint(request) {
        return provider;
    }

    let configured = format!(
        "{} {} {}",
        config.api_style.as_deref().unwrap_or(""),
        config.provider,
        config.base_url.as_deref().unwrap_or("")
    )
    .to_lowercase();
    if let Some(provider) = provider_hint_from_text(&configured) {
        return provider;
    }

    let model = request
        .model
        .or(config.model.as_deref())
        .unwrap_or("")
        .to_lowercase();
    provider_hint_from_text(&model).unwrap_or(ImageProvider::OpenAi)
}

fn is_image_generation_model(model: &str) -> bool {
    let model = model.to_lowercase();
    [
        "gpt-image",
        "grok-imagine-image",
        "chatgpt-image",
        "dall-e",
        "gemini-2.5-flash-image",
        "gemini-3-pro-image",
        "nano-banana",
        "nano_banana",
        "qwen-image",
        "glm-image",
        "cogview",
        "imagen",
        "flux",
        "seedream",
        "ideogram",
        "stable-image",
        "sdxl",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}

fn default_model(provider: ImageProvider) -> &'static str {
    match provider {
        ImageProvider::OpenAi => "gpt-image-2.5-flare",
        ImageProvider::Xai => "grok-imagine-image-2.0",
        ImageProvider::Google => "gemini-3-pro-image-preview",
        ImageProvider::Qwen => "qwen-image-2.0-pro",
    }
}

fn provider_artifact_name(provider: ImageProvider, config: &ResolvedImageConfig) -> String {
    if provider == ImageProvider::Xai {
        return "xai".to_string();
    }
    if !config.provider.trim().is_empty() {
        return config.provider.trim().to_string();
    }

    match provider {
        ImageProvider::OpenAi => "openai".to_string(),
        ImageProvider::Xai => "xai".to_string(),
        ImageProvider::Google => "google".to_string(),
        ImageProvider::Qwen => "qwen".to_string(),
    }
}

fn normalize_output_format(value: Option<&str>) -> &'static str {
    match value.unwrap_or("png").trim().to_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg",
        "webp" => "webp",
        _ => "png",
    }
}

fn provider_catalog() -> CapabilityProviderCatalog {
    let items = load_image_provider_presets()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|preset| serde_json::to_value(preset).ok())
        .collect();

    CapabilityProviderCatalog {
        id: "imageProviders".to_string(),
        label: "Image providers".to_string(),
        item_kind: "imageProviderPreset".to_string(),
        items,
    }
}

fn settings_schema() -> CapabilitySettingsSchema {
    let defaults = ImageGenerationConfig::default();
    CapabilitySettingsSchema {
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
) -> CapabilitySettingsField {
    CapabilitySettingsField {
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

fn runtime_checks(config: Option<&ImageGenerationConfig>) -> Vec<CapabilityRuntimeCheck> {
    let Some(config) = config else {
        return vec![check(
            "configuration",
            "Configuration",
            CapabilityRuntimeStatus::Unknown,
            CapabilityCheckSeverity::Info,
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

fn provider_check(config: &ImageGenerationConfig) -> CapabilityRuntimeCheck {
    let preset = find_image_provider_preset(
        &config.provider,
        Some(config.api_style.as_str()),
        config.base_url.as_deref(),
    );
    if preset.is_some() {
        check(
            "provider-preset",
            "Provider preset",
            CapabilityRuntimeStatus::Pass,
            CapabilityCheckSeverity::Info,
            "Provider, API style, and base URL match a known image provider preset.",
        )
    } else {
        check(
            "provider-preset",
            "Provider preset",
            CapabilityRuntimeStatus::Warning,
            CapabilityCheckSeverity::Warning,
            "This image provider is custom or does not match the shared provider catalog.",
        )
    }
}

fn api_key_check(config: &ImageGenerationConfig) -> CapabilityRuntimeCheck {
    if config.api_key.trim().is_empty() {
        check(
            "api-key",
            "API key",
            CapabilityRuntimeStatus::Error,
            CapabilityCheckSeverity::Error,
            "Image generation needs a provider API key before generate_image can run.",
        )
    } else {
        check(
            "api-key",
            "API key",
            CapabilityRuntimeStatus::Pass,
            CapabilityCheckSeverity::Info,
            "An image provider API key is configured.",
        )
    }
}

fn base_url_check(config: &ImageGenerationConfig) -> CapabilityRuntimeCheck {
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
            CapabilityRuntimeStatus::Error,
            CapabilityCheckSeverity::Error,
            "Image generation needs a base URL for the selected provider.",
        );
    }

    match Url::parse(trimmed) {
        Ok(url) if matches!(url.scheme(), "https" | "http") => check(
            "base-url",
            "Base URL",
            CapabilityRuntimeStatus::Pass,
            CapabilityCheckSeverity::Info,
            "The image provider endpoint is a valid HTTP URL.",
        ),
        _ => check(
            "base-url",
            "Base URL",
            CapabilityRuntimeStatus::Error,
            CapabilityCheckSeverity::Error,
            "The image provider base URL is invalid.",
        ),
    }
}

fn model_check(config: &ImageGenerationConfig) -> CapabilityRuntimeCheck {
    let model = config.model.trim();
    if model.is_empty() {
        return check(
            "model",
            "Model",
            CapabilityRuntimeStatus::Error,
            CapabilityCheckSeverity::Error,
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
            CapabilityRuntimeStatus::Warning,
            CapabilityCheckSeverity::Warning,
            "A custom image model is configured outside the shared provider catalog.",
        );
    };
    if preset.models.is_empty() || preset.models.iter().any(|candidate| candidate.id == model) {
        check(
            "model",
            "Model",
            CapabilityRuntimeStatus::Pass,
            CapabilityCheckSeverity::Info,
            "The configured model is available for the selected image provider.",
        )
    } else {
        check(
            "model",
            "Model",
            CapabilityRuntimeStatus::Warning,
            CapabilityCheckSeverity::Warning,
            "The configured model is custom for the selected image provider.",
        )
    }
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
    use crate::app_settings::AppConfig;
    use crate::conversation::SaveAgentConfigInput;
    use crate::db::Database;

    #[test]
    fn image_xai_runtime_preserves_endpoint_key_model_and_adapter() {
        let db = Database::open_memory().unwrap();
        let mut config = AppConfig::default();
        config.image_generation = ImageGenerationConfig {
            provider: "open_ai".to_string(),
            api_style: "xai_images".to_string(),
            api_key: "xai-image-key".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            model: "grok-imagine-image-2.0".to_string(),
            size: Some("16:9|2k".to_string()),
            quality: Some("medium".to_string()),
            output_format: Some("jpeg".to_string()),
        };
        db.save_app_config(&config).unwrap();
        let runtime = resolve_runtime(
            &db,
            &ImageGenerationRequest {
                provider_config_id: None,
                provider: Some("xai"),
                api_style: None,
                model: None,
                output_format: None,
            },
        )
        .unwrap();
        assert_eq!(runtime.provider, ImageProvider::Xai);
        assert_eq!(runtime.model, "grok-imagine-image-2.0");
        assert_eq!(runtime.config.api_key, "xai-image-key");
        assert_eq!(
            runtime.config.endpoint_base_url("unused"),
            "https://api.x.ai/v1"
        );
        assert_eq!(runtime.output_format, "jpeg");
    }

    #[test]
    fn image_xai_override_does_not_reuse_an_openai_key_or_lookalike_host() {
        assert!(!is_xai_identity("open_ai https://api.x.ai.example.com/v1"));
        let db = Database::open_memory().unwrap();
        let mut config = AppConfig::default();
        config.image_generation.api_key = "openai-only-key".to_string();
        db.save_app_config(&config).unwrap();
        assert!(resolve_runtime(
            &db,
            &ImageGenerationRequest {
                provider_config_id: None,
                provider: Some("xai"),
                api_style: None,
                model: None,
                output_format: None
            }
        )
        .is_err());
    }

    #[test]
    fn image_manifest_carries_provider_catalog_and_settings_schema() {
        let manifest = enrich_manifest(
            CapabilityPackageView {
                id: "image-generation".to_string(),
                name: "Image Generation".to_string(),
                capability: "Image creation".to_string(),
                description: "test".to_string(),
                built_in: true,
                surface: crate::ecosystem::EcosystemSurfaceKind::Adapter,
                version: 1,
                tools: vec!["generate_image".to_string()],
                skills: Vec::new(),
                settings_surfaces: vec!["image-generation".to_string()],
                workflows: vec!["generate-image".to_string()],
                permissions: crate::capability_package::CapabilityPackagePermissions::default(),
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
            .any(|check| check.id == "api-key" && check.status == CapabilityRuntimeStatus::Error));
    }

    #[test]
    fn resolve_runtime_uses_image_plugin_config_and_normalizes_defaults() {
        let db = Database::open_memory().expect("open in-memory db");
        let mut config = AppConfig::default();
        config.image_generation = ImageGenerationConfig {
            provider: "google".to_string(),
            api_style: "gemini_generate_content".to_string(),
            api_key: "image-key".to_string(),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            model: "gemini-3-pro-image-preview".to_string(),
            size: Some("16:9|2K".to_string()),
            quality: None,
            output_format: Some("png".to_string()),
        };
        db.save_app_config(&config).expect("save app config");

        let runtime = resolve_runtime(
            &db,
            &ImageGenerationRequest {
                provider_config_id: None,
                provider: Some("google"),
                api_style: Some("gemini_generate_content"),
                model: None,
                output_format: Some("jpg"),
            },
        )
        .expect("resolve runtime");

        assert_eq!(runtime.provider, ImageProvider::Google);
        assert_eq!(runtime.provider_name, "google");
        assert_eq!(runtime.model, "gemini-3-pro-image-preview");
        assert_eq!(runtime.output_format, "jpeg");
        assert_eq!(runtime.config.api_key, "image-key");
    }

    #[test]
    fn resolve_runtime_reuses_same_provider_agent_key_when_image_config_is_unset() {
        let db = Database::open_memory().expect("open in-memory db");
        db.save_agent_config(&SaveAgentConfigInput {
            id: None,
            name: "Qwen CN".to_string(),
            provider: "qwen".to_string(),
            api_key: "qwen-key".to_string(),
            base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
            model: "qwen3.6-plus".to_string(),
            provider_endpoint_id: None,
            model_id: None,
            temperature: None,
            max_tokens: None,
            context_window: None,
            is_default: true,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            delegation_limits_v2: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
            provider_streaming: Default::default(),
        })
        .expect("save qwen config");

        let runtime = resolve_runtime(
            &db,
            &ImageGenerationRequest {
                provider_config_id: None,
                provider: Some("qwen"),
                api_style: Some("dashscope_multimodal"),
                model: None,
                output_format: None,
            },
        )
        .expect("resolve runtime");

        assert_eq!(runtime.provider, ImageProvider::Qwen);
        assert_eq!(runtime.config.api_key, "qwen-key");
        assert_eq!(runtime.model, "qwen-image-2.0-pro");
        assert_eq!(
            runtime.config.qwen_endpoint(),
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
        );
    }

    #[test]
    fn qwen_endpoint_rewrites_compatible_mode_base_url() {
        let config = ResolvedImageConfig {
            provider: "qwen".to_string(),
            api_style: Some("dashscope_multimodal".to_string()),
            api_key: "key".to_string(),
            base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
            model: Some("qwen-image-2.0-pro".to_string()),
            size: None,
            quality: None,
            output_format: None,
        };

        assert_eq!(
            config.qwen_endpoint(),
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
        );
    }
}
