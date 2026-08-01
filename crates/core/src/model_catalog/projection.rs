use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    AuthStyle, CredentialKind, DiscoveryStrategy, EndpointTransport, HealthProbe, ModelAccess,
    ModelCapabilities, ModelCatalogSource, ModelDescriptor, ModelLifecycle, ModelModality,
    ProductReadiness, ProviderDescriptor, ProviderEndpoint, ReasoningCapability,
};

const TEXT_PRESETS: &str = include_str!("../../../../shared/provider-presets.json");
const IMAGE_PRESETS: &str = include_str!("../../../../shared/image-provider-presets.json");
const EMBEDDING_PRESETS: &str = include_str!("../../../../shared/embedding-provider-presets.json");
const STT_PRESETS: &str = include_str!("../../../../shared/stt-provider-presets.json");
const TTS_PRESETS: &str = include_str!("../../../../shared/tts-provider-presets.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogSurface {
    Text,
    Image,
    Embedding,
    SpeechToText,
    TextToSpeech,
}

impl CatalogSurface {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Embedding => "embedding",
            Self::SpeechToText => "speech_to_text",
            Self::TextToSpeech => "text_to_speech",
        }
    }

    fn default_api_style(self, provider_id: &str) -> &'static str {
        match self {
            Self::Text => match provider_id {
                "anthropic" => "anthropic_messages",
                "google" => "gemini_generate_content",
                "ollama" => "ollama_chat",
                _ => "openai_chat",
            },
            Self::Image => "openai_images",
            Self::Embedding => "openai_embeddings",
            Self::SpeechToText => "openai_transcription",
            Self::TextToSpeech => "openai_speech",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinModelCatalog {
    pub providers: Vec<ProviderDescriptor>,
    pub endpoints: Vec<ProviderEndpoint>,
    pub models: Vec<ModelDescriptor>,
}

pub fn load_builtin_catalog() -> Result<BuiltinModelCatalog, String> {
    let sources = [
        (CatalogSurface::Text, TEXT_PRESETS),
        (CatalogSurface::Image, IMAGE_PRESETS),
        (CatalogSurface::Embedding, EMBEDDING_PRESETS),
        (CatalogSurface::SpeechToText, STT_PRESETS),
        (CatalogSurface::TextToSpeech, TTS_PRESETS),
    ];
    let mut providers = BTreeMap::<String, ProviderDescriptor>::new();
    let mut endpoints = Vec::new();
    let mut models = Vec::new();

    for (surface, source) in sources {
        let presets = serde_json::from_str::<Vec<Value>>(source)
            .map_err(|error| format!("invalid {} catalog: {error}", surface.wire_name()))?;
        for preset in presets {
            let preset_id = required_string(&preset, "id", surface)?;
            let adapter_provider = required_string(&preset, "provider", surface)?;
            let provider_id = canonical_provider_id(&preset_id, &adapter_provider);
            let display_name = required_string(&preset, "name", surface)?;
            let base_url = optional_string(&preset, "baseUrl").unwrap_or_default();
            let api_style = optional_string(&preset, "apiStyle")
                .unwrap_or_else(|| surface.default_api_style(&provider_id).to_string());
            let requires_api_key = preset
                .get("requiresApiKey")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    !preset
                        .get("local")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                });
            let region = infer_region(&base_url);
            let endpoint_id = format!("{}:{}", surface.wire_name(), preset_id);
            let endpoint = ProviderEndpoint {
                id: endpoint_id.clone(),
                provider_id: provider_id.clone(),
                region: region.clone(),
                base_url_template: base_url,
                api_style: api_style.clone(),
                transport: infer_transport(&api_style),
                auth_style: infer_auth_style(&provider_id, requires_api_key),
                workspace_required: preset
                    .get("workspaceRequired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                discovery_strategy: infer_discovery_strategy(surface, &api_style, requires_api_key),
                health_probe: infer_health_probe(surface, requires_api_key),
            };
            endpoints.push(endpoint.clone());

            let provider =
                providers
                    .entry(provider_id.clone())
                    .or_insert_with(|| ProviderDescriptor {
                        id: provider_id.clone(),
                        display_name: canonical_provider_name(&provider_id, &display_name),
                        aliases: provider_aliases(&provider_id),
                        credential_kind: if requires_api_key {
                            CredentialKind::ApiKey
                        } else {
                            CredentialKind::None
                        },
                        documentation_ref: documentation_ref(&provider_id),
                        endpoints: Vec::new(),
                    });
            if !provider.endpoints.iter().any(|item| item.id == endpoint.id) {
                provider.endpoints.push(endpoint);
            }

            for model in preset
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                models.push(project_model(
                    surface,
                    &provider_id,
                    &endpoint_id,
                    &region,
                    &api_style,
                    &preset,
                    model,
                )?);
            }
        }
    }

    let mut providers = providers.into_values().collect::<Vec<_>>();
    for provider in &mut providers {
        provider
            .endpoints
            .sort_by(|left, right| left.id.cmp(&right.id));
    }
    endpoints.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(BuiltinModelCatalog {
        providers,
        endpoints,
        models,
    })
}

/// Resolves a legacy adapter/provider pair to the stable endpoint identity used
/// by catalog v2. Exact base URL matches take precedence because several
/// OpenAI-compatible adapters share the same legacy `provider` value.
pub fn resolve_builtin_endpoint_id(
    surface: &str,
    provider_or_alias: &str,
    base_url: Option<&str>,
) -> Option<String> {
    let catalog = load_builtin_catalog().ok()?;
    let endpoint_prefix = format!("{}:", surface.trim().to_ascii_lowercase());
    let endpoints = catalog
        .endpoints
        .iter()
        .filter(|endpoint| endpoint.id.starts_with(&endpoint_prefix))
        .collect::<Vec<_>>();
    let normalized_base_url = normalize_url(base_url);

    if !normalized_base_url.is_empty() {
        if let Some(endpoint) = endpoints.iter().find(|endpoint| {
            normalize_url(Some(&endpoint.base_url_template)) == normalized_base_url
        }) {
            return Some(endpoint.id.clone());
        }
    }

    let provider_or_alias = provider_or_alias.trim().to_ascii_lowercase();
    let provider_ids = catalog
        .providers
        .iter()
        .filter(|provider| {
            provider.id.eq_ignore_ascii_case(&provider_or_alias)
                || provider
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(&provider_or_alias))
        })
        .map(|provider| provider.id.as_str())
        .collect::<Vec<_>>();
    let matches = endpoints
        .into_iter()
        .filter(|endpoint| {
            provider_ids
                .iter()
                .any(|provider_id| endpoint.provider_id.eq_ignore_ascii_case(provider_id))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0].id.clone())
}

fn project_model(
    surface: CatalogSurface,
    provider_id: &str,
    endpoint_id: &str,
    region: &str,
    api_style: &str,
    preset: &Value,
    model: &Value,
) -> Result<ModelDescriptor, String> {
    let id = required_string(model, "id", surface)?;
    let display_name = required_string(model, "name", surface)?;
    let mut descriptor = ModelDescriptor::new(&id, provider_id, display_name);
    descriptor.family = optional_string(model, "family").unwrap_or_else(|| id.clone());
    descriptor.version = optional_string(model, "version");
    descriptor.aliases = string_array(model.get("aliases"));
    descriptor.endpoint_ids = vec![endpoint_id.to_string()];
    descriptor.endpoint_kinds = vec![surface.wire_name().to_string()];
    descriptor.regions = {
        let values = string_array(model.get("regions"));
        if values.is_empty() && !region.is_empty() {
            vec![region.to_string()]
        } else {
            values
        }
    };
    descriptor.lifecycle = lifecycle(model, &id);
    descriptor.access = access(model, descriptor.lifecycle);
    descriptor.source = source(model);
    descriptor.recommended = model
        .get("recommended")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    descriptor.last_verified_at = optional_string(model, "lastVerifiedAt");
    descriptor.release_date = optional_string(model, "releaseDate");
    descriptor.deprecation_date = optional_string(model, "deprecationDate");
    descriptor.replacement_model_id = optional_string(model, "replacementModelId");
    descriptor.pricing_ref = optional_string(model, "pricingRef");
    descriptor.product_readiness = readiness(model, &descriptor);
    descriptor.input_modalities = input_modalities(surface, model);
    descriptor.output_modalities = output_modalities(surface, model);
    descriptor.capabilities = capabilities(surface, api_style, model);
    descriptor.limits = limits(surface, preset, model);
    Ok(descriptor)
}

fn capabilities(surface: CatalogSurface, api_style: &str, model: &Value) -> ModelCapabilities {
    let raw = model.get("capabilities").cloned().unwrap_or(Value::Null);
    let reasoning = raw
        .get("reasoning")
        .cloned()
        .and_then(|value| serde_json::from_value::<ReasoningCapability>(value).ok());
    let bool_value = |field: &str| raw.get(field).and_then(Value::as_bool).unwrap_or(false);
    let supports_tools = model
        .get("supportsTools")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| bool_value("toolCalling"));
    let supports_structured = model
        .get("supportsStructuredOutput")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| bool_value("structuredOutput"));
    let realtime = api_style.to_ascii_lowercase().contains("realtime");
    ModelCapabilities {
        reasoning,
        vision: bool_value("vision"),
        audio_input: surface == CatalogSurface::SpeechToText || bool_value("audioInput"),
        audio_output: surface == CatalogSurface::TextToSpeech || bool_value("audioOutput"),
        video_input: bool_value("videoInput"),
        video_output: bool_value("videoOutput"),
        tool_calling: supports_tools,
        parallel_tool_calling: bool_value("parallelToolCalling"),
        structured_output: supports_structured,
        image_generation: surface == CatalogSurface::Image || bool_value("imageGeneration"),
        image_editing: bool_value("imageEditing"),
        multi_reference_editing: bool_value("multiReferenceEditing"),
        realtime: realtime || bool_value("realtime"),
        prompt_cache: bool_value("promptCache"),
        async_jobs: bool_value("asyncJobs"),
        batch: bool_value("batch"),
        dimension_override: model
            .get("supportsDimensionOverride")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn limits(surface: CatalogSurface, preset: &Value, model: &Value) -> super::ModelLimits {
    super::ModelLimits {
        context_tokens: optional_u64(model, "contextTokens"),
        max_output_tokens: optional_u64(model, "maxOutputTokens"),
        max_images: optional_u64(model, "maxImages").and_then(|value| u32::try_from(value).ok()),
        max_input_bytes: optional_u64(model, "maxInputBytes"),
        max_video_seconds: optional_u64(model, "maxVideoSeconds"),
        max_audio_seconds: optional_u64(model, "maxAudioSeconds"),
        embedding_dimensions: if surface == CatalogSurface::Embedding {
            optional_u64(model, "dimensions").and_then(|value| usize::try_from(value).ok())
        } else {
            None
        },
        supported_sizes: preset
            .get("sizeOptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| optional_string(value, "value"))
            .collect(),
        output_formats: string_array(preset.get("outputFormats")),
    }
}

fn input_modalities(surface: CatalogSurface, model: &Value) -> Vec<ModelModality> {
    let explicit = modality_array(model.get("inputModalities"));
    if !explicit.is_empty() {
        return explicit;
    }
    match surface {
        CatalogSurface::Text => {
            let mut values = modality_array(model.get("modalities"));
            if values.is_empty() {
                values.push(ModelModality::Text);
            }
            values
        }
        CatalogSurface::Image => vec![ModelModality::Text],
        CatalogSurface::Embedding => vec![ModelModality::Text],
        CatalogSurface::SpeechToText => vec![ModelModality::Audio],
        CatalogSurface::TextToSpeech => vec![ModelModality::Text],
    }
}

fn output_modalities(surface: CatalogSurface, model: &Value) -> Vec<ModelModality> {
    let explicit = modality_array(model.get("outputModalities"));
    if !explicit.is_empty() {
        return explicit;
    }
    match surface {
        CatalogSurface::Text | CatalogSurface::SpeechToText => vec![ModelModality::Text],
        CatalogSurface::Image => vec![ModelModality::Image],
        CatalogSurface::Embedding => vec![ModelModality::Embedding],
        CatalogSurface::TextToSpeech => vec![ModelModality::Audio],
    }
}

fn modality_array(value: Option<&Value>) -> Vec<ModelModality> {
    string_array(value)
        .into_iter()
        .filter_map(|value| match value.to_ascii_lowercase().as_str() {
            "text" => Some(ModelModality::Text),
            "image" => Some(ModelModality::Image),
            "audio" => Some(ModelModality::Audio),
            "video" => Some(ModelModality::Video),
            "file" => Some(ModelModality::File),
            "embedding" => Some(ModelModality::Embedding),
            _ => None,
        })
        .collect()
}

fn lifecycle(model: &Value, id: &str) -> ModelLifecycle {
    match optional_string(model, "status")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "preview" => ModelLifecycle::Preview,
        "gated" => ModelLifecycle::Gated,
        "legacy" => ModelLifecycle::Legacy,
        "deprecated" => ModelLifecycle::Deprecated,
        "removed" => ModelLifecycle::Removed,
        "active" => ModelLifecycle::Active,
        _ if optional_string(model, "tagKey")
            .is_some_and(|tag| tag.to_ascii_lowercase().contains("preview"))
            || id.to_ascii_lowercase().contains("preview")
            || optional_string(model, "name")
                .is_some_and(|name| name.to_ascii_lowercase().contains("preview")) =>
        {
            ModelLifecycle::Preview
        }
        _ => ModelLifecycle::Active,
    }
}

fn access(model: &Value, lifecycle: ModelLifecycle) -> ModelAccess {
    match optional_string(model, "access")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "account_enablement" => ModelAccess::AccountEnablement,
        "application" => ModelAccess::Application,
        "private_preview" => ModelAccess::PrivatePreview,
        "public" => ModelAccess::Public,
        _ if lifecycle == ModelLifecycle::Preview => ModelAccess::Application,
        _ if lifecycle == ModelLifecycle::Gated => ModelAccess::AccountEnablement,
        _ => ModelAccess::Public,
    }
}

fn source(model: &Value) -> ModelCatalogSource {
    match optional_string(model, "source")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "official" => ModelCatalogSource::Official,
        "discovered" => ModelCatalogSource::Discovered,
        _ => ModelCatalogSource::Curated,
    }
}

fn readiness(model: &Value, descriptor: &ModelDescriptor) -> ProductReadiness {
    match optional_string(model, "productReadiness")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "discoverable" => ProductReadiness::Discoverable,
        "callable" => ProductReadiness::Callable,
        "product_ready" => ProductReadiness::ProductReady,
        "known" => ProductReadiness::Known,
        _ if descriptor.source == ModelCatalogSource::Discovered => ProductReadiness::Discoverable,
        _ if descriptor.recommended
            && descriptor.lifecycle == ModelLifecycle::Active
            && descriptor.access == ModelAccess::Public =>
        {
            ProductReadiness::ProductReady
        }
        _ => ProductReadiness::Known,
    }
}

fn canonical_provider_id(preset_id: &str, adapter_provider: &str) -> String {
    let id = preset_id.to_ascii_lowercase();
    if id == "openai" || id == "openai-live" {
        "openai".into()
    } else if id.starts_with("google") {
        "google".into()
    } else if id.starts_with("qwen") || id.starts_with("alibaba") || id.starts_with("dashscope") {
        "alibaba_model_studio".into()
    } else if id.starts_with("custom") {
        "custom".into()
    } else if id.starts_with("sherpa") {
        "sherpa_onnx".into()
    } else {
        match adapter_provider {
            "deep_seek" => "deepseek".into(),
            "lm_studio" => "lmstudio".into(),
            "open_ai" => id.replace("-", "_"),
            value => value.to_string(),
        }
    }
}

fn canonical_provider_name(provider_id: &str, fallback: &str) -> String {
    match provider_id {
        "openai" => "OpenAI".into(),
        "google" => "Google".into(),
        "alibaba_model_studio" => "Alibaba Cloud Model Studio".into(),
        "sherpa_onnx" => "Sherpa ONNX".into(),
        _ => fallback.to_string(),
    }
}

fn provider_aliases(provider_id: &str) -> Vec<String> {
    match provider_id {
        "openai" => vec!["open_ai".into()],
        "deepseek" => vec!["deep_seek".into()],
        "lmstudio" => vec!["lm_studio".into()],
        "alibaba_model_studio" => vec!["qwen".into(), "dashscope".into()],
        _ => Vec::new(),
    }
}

fn normalize_url(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn documentation_ref(provider_id: &str) -> Option<String> {
    match provider_id {
        "alibaba_model_studio" => Some("https://help.aliyun.com/en/model-studio/".into()),
        "doubao" => Some("https://www.volcengine.com/docs/82379".into()),
        _ => None,
    }
}

fn infer_region(base_url: &str) -> String {
    let value = base_url.to_ascii_lowercase();
    if value.is_empty() || value.contains("localhost") || value.contains("127.0.0.1") {
        "local".into()
    } else if value.contains("dashscope-intl") {
        "ap-southeast-1".into()
    } else if value.contains("dashscope") || value.contains("cn-beijing") {
        "cn-beijing".into()
    } else if value.contains("eastus") {
        "eastus".into()
    } else {
        "global".into()
    }
}

fn infer_transport(api_style: &str) -> EndpointTransport {
    let value = api_style.to_ascii_lowercase();
    if value.contains("realtime") {
        EndpointTransport::Websocket
    } else if value.contains("async") {
        EndpointTransport::AsyncJob
    } else if value.contains("sse") {
        EndpointTransport::Sse
    } else {
        EndpointTransport::Http
    }
}

fn infer_auth_style(provider_id: &str, requires_api_key: bool) -> AuthStyle {
    if !requires_api_key {
        AuthStyle::None
    } else if provider_id == "google" {
        AuthStyle::Query
    } else if provider_id == "azure_speech" {
        AuthStyle::Header
    } else {
        AuthStyle::Bearer
    }
}

fn infer_discovery_strategy(
    surface: CatalogSurface,
    api_style: &str,
    requires_api_key: bool,
) -> DiscoveryStrategy {
    if !requires_api_key {
        return DiscoveryStrategy::Static;
    }
    if surface == CatalogSurface::Text && api_style.starts_with("openai") {
        DiscoveryStrategy::OpenAiModels
    } else if matches!(
        surface,
        CatalogSurface::TextToSpeech | CatalogSurface::SpeechToText
    ) {
        DiscoveryStrategy::ProviderCatalog
    } else {
        DiscoveryStrategy::Static
    }
}

fn infer_health_probe(surface: CatalogSurface, requires_api_key: bool) -> HealthProbe {
    if !requires_api_key {
        HealthProbe::None
    } else if surface == CatalogSurface::Text {
        HealthProbe::Models
    } else {
        HealthProbe::LightweightRequest
    }
}

fn required_string(value: &Value, field: &str, surface: CatalogSurface) -> Result<String, String> {
    optional_string(value, field).ok_or_else(|| {
        format!(
            "{} catalog entry is missing a non-empty '{field}'",
            surface.wire_name()
        )
    })
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
