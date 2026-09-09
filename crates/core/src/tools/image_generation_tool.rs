//! GenerateImageTool — text-to-image generation through configured providers.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use reqwest::header::CONTENT_TYPE;
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::plugins::image_generation::{
    ImageGenerationRequest, ImageProvider, ResolvedImageConfig,
};

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_image.json");

pub struct GenerateImageTool;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ImagePromptMode {
    #[default]
    Verbatim,
    AgentRefined,
    ProviderEnhanced,
}

impl ImagePromptMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Verbatim => "verbatim",
            Self::AgentRefined => "agent_refined",
            Self::ProviderEnhanced => "provider_enhanced",
        }
    }

    fn provider_enhancement_enabled(self) -> bool {
        matches!(self, Self::ProviderEnhanced)
    }
}

#[derive(Debug, Deserialize)]
struct GenerateImageArgs {
    prompt: String,
    #[serde(default, alias = "promptMode")]
    prompt_mode: Option<ImagePromptMode>,
    #[serde(default, alias = "promptExtend")]
    prompt_extend: Option<bool>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default, alias = "apiStyle")]
    api_style: Option<String>,
    #[serde(default, alias = "providerConfigId")]
    provider_config_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default, alias = "aspectRatio")]
    aspect_ratio: Option<String>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default, alias = "outputCompression")]
    output_compression: Option<u8>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default, alias = "outputFormat")]
    output_format: Option<String>,
    #[serde(default, alias = "negativePrompt")]
    negative_prompt: Option<String>,
    #[serde(default)]
    watermark: Option<bool>,
    #[serde(default)]
    filename: Option<String>,
}

impl GenerateImageArgs {
    fn effective_prompt_mode(&self) -> ImagePromptMode {
        self.prompt_mode.unwrap_or_else(|| {
            if self.prompt_extend == Some(true) {
                ImagePromptMode::ProviderEnhanced
            } else {
                ImagePromptMode::Verbatim
            }
        })
    }

    fn runtime_request(&self) -> ImageGenerationRequest<'_> {
        ImageGenerationRequest {
            provider_config_id: self.provider_config_id.as_deref(),
            provider: self.provider.as_deref(),
            api_style: self.api_style.as_deref(),
            model: self.model.as_deref(),
            output_format: self.output_format.as_deref(),
        }
    }
}

#[derive(Debug)]
struct GeneratedImage {
    bytes: Vec<u8>,
    media_type: String,
    provider_image_url: Option<String>,
    usage: Option<Value>,
    revised_prompt: Option<String>,
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: GenerateImageArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_image arguments: {e}"))
        })?;

        if args.prompt.trim().is_empty() {
            return Ok(error_result(call_id, "Image prompt cannot be empty."));
        }
        if args.prompt.chars().count() > 32_000 {
            return Ok(error_result(
                call_id,
                "Image prompt is too long; keep it under 32000 characters.",
            ));
        }

        let runtime =
            crate::plugins::image_generation::resolve_runtime(db, &args.runtime_request())?;
        validate_image_options(
            &runtime.config,
            &args,
            &runtime.model,
            runtime.provider,
            runtime.output_format,
        )?;
        let prompt_mode = args.effective_prompt_mode();
        let provider_enhancement_requested = prompt_mode.provider_enhancement_enabled();
        let provider_enhancement_supported = supports_explicit_prompt_enhancement(runtime.provider);
        let provider_enhancement_applied =
            provider_enhancement_requested && provider_enhancement_supported;
        if runtime.config.api_key.trim().is_empty() {
            return Ok(error_result(
                call_id,
                "The image generation provider has no API key.",
            ));
        }

        let client = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| CoreError::InvalidInput(format!("Failed to build HTTP client: {e}")))?;

        let generated = match runtime.provider {
            ImageProvider::OpenAi | ImageProvider::Xai => {
                generate_openai_image(
                    &client,
                    &runtime.config,
                    &args,
                    &runtime.model,
                    runtime.output_format,
                    runtime.provider,
                )
                .await?
            }
            ImageProvider::Google => {
                generate_google_image(&client, &runtime.config, &args, &runtime.model).await?
            }
            ImageProvider::Qwen => {
                generate_qwen_image(&client, &runtime.config, &args, &runtime.model).await?
            }
        };

        let media_type = if generated.media_type.trim().is_empty() {
            media_type_for_format(runtime.output_format).to_string()
        } else {
            generated.media_type.clone()
        };
        let extension = extension_for_media_type(&media_type)
            .unwrap_or_else(|| extension_for_format(runtime.output_format).to_string());
        let (preview_path, suggested_filename) = resolve_preview_path(&args, &extension)?;
        if let Some(parent) = preview_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&preview_path, &generated.bytes)?;

        let size_bytes = generated.bytes.len();
        let effective_prompt = generated.revised_prompt.as_deref();
        let prompt_integrity = match effective_prompt {
            Some(effective) if effective == args.prompt => "exact",
            Some(_) => "revised",
            None => "unknown",
        };
        let mut warnings = Vec::new();
        if prompt_mode == ImagePromptMode::Verbatim && prompt_integrity == "revised" {
            warnings.push("the provider reported a revised prompt; the requested prompt remains preserved in preview metadata");
        }
        if provider_enhancement_requested && !provider_enhancement_supported {
            warnings.push("the selected provider adapter has no controllable prompt enhancer, so Nexa submitted the base prompt unchanged");
        }
        let warning_text = if warnings.is_empty() {
            String::new()
        } else {
            format!("\nWarning: {}.", warnings.join("; "))
        };
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "Generated image ready for preview. It has not been saved to the workspace.\nProvider: {}\nModel: {}\nSize: {} bytes{}",
                runtime.provider_name,
                runtime.model,
                size_bytes,
                warning_text,
            ),
            is_error: false,
            artifacts: Some(json!({
                "kind": "generatedImage",
                "provider": runtime.provider_name,
                "model": runtime.model,
                "path": preview_path.to_string_lossy(),
                "previewPath": preview_path.to_string_lossy(),
                "suggestedFilename": suggested_filename,
                "mediaType": media_type,
                "bytes": size_bytes,
                "saved": false,
                "transient": true,
                "prompt": args.prompt.as_str(),
                "requestedPrompt": args.prompt.as_str(),
                "promptMode": prompt_mode.as_str(),
                "effectivePrompt": effective_prompt,
                "providerPromptEnhancementRequested": provider_enhancement_requested,
                "providerPromptEnhancementSupported": provider_enhancement_supported,
                "providerPromptEnhanced": provider_enhancement_applied,
                "promptRewriteObservable": effective_prompt.is_some(),
                "promptIntegrity": prompt_integrity,
                "revisedPrompt": effective_prompt,
                "providerImageUrl": generated.provider_image_url,
                "usage": generated.usage,
            })),
        })
    }
}

fn error_result(call_id: &str, message: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: message.into(),
        is_error: true,
        artifacts: None,
    }
}

fn selected_text(arg: Option<&str>, configured: Option<&str>, fallback: &str) -> String {
    selected_optional(arg, configured)
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn selected_optional<'a>(arg: Option<&'a str>, configured: Option<&'a str>) -> Option<&'a str> {
    [arg, configured]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn selected_image_quality<'a>(
    config: &'a ResolvedImageConfig,
    args: &'a GenerateImageArgs,
    model: &str,
) -> Option<&'a str> {
    let configured = config
        .model
        .as_deref()
        .is_none_or(|configured| configured.trim() == model.trim())
        .then_some(config.quality.as_deref())
        .flatten();
    selected_optional(args.quality.as_deref(), configured)
}

fn selected_image_size(
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
) -> String {
    let configured = config
        .model
        .as_deref()
        .is_none_or(|configured| configured.trim() == model.trim())
        .then_some(config.size.as_deref())
        .flatten();
    selected_text(args.size.as_deref(), configured, "1024x1024")
}

fn supports_explicit_prompt_enhancement(provider: ImageProvider) -> bool {
    matches!(provider, ImageProvider::Qwen)
}

fn google_image_config(size: Option<&str>) -> Option<Value> {
    let size = size.map(str::trim).filter(|value| !value.is_empty())?;
    let mut object = serde_json::Map::new();

    for part in size
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if part.contains(':') {
            object.insert("aspectRatio".to_string(), json!(part));
        } else if matches!(part.to_ascii_uppercase().as_str(), "1K" | "2K" | "4K") {
            object.insert("imageSize".to_string(), json!(part.to_ascii_uppercase()));
        }
    }

    if object.is_empty() {
        None
    } else {
        Some(Value::Object(object))
    }
}

fn build_google_image_body(args: &GenerateImageArgs, configured_size: Option<&str>) -> Value {
    let mut generation_config = json!({
        "responseModalities": ["TEXT", "IMAGE"]
    });
    if let Some(image_config) = google_image_config(args.size.as_deref().or(configured_size)) {
        generation_config["responseFormat"] = json!({ "image": image_config });
    }
    json!({
        "contents": [{
            "parts": [{ "text": args.prompt.as_str() }]
        }],
        "generationConfig": generation_config
    })
}

async fn generate_openai_image(
    client: &reqwest::Client,
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
    output_format: &str,
    provider: ImageProvider,
) -> Result<GeneratedImage, CoreError> {
    let is_xai = provider == ImageProvider::Xai;
    let provider_label = if is_xai {
        "xAI image API"
    } else {
        "OpenAI image API"
    };
    let base_url = config.endpoint_base_url(if is_xai {
        "https://api.x.ai/v1"
    } else {
        "https://api.openai.com/v1"
    });
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/images/generations");
    let body = if is_xai {
        build_xai_images_body(config, args, model)?
    } else {
        build_openai_images_body(config, args, model, output_format)
    };

    let response = client
        .post(url)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| CoreError::TransientLlm(format!("{provider_label} request failed: {e}")))?;

    let (status, content_type, bytes) = read_provider_body(response, provider_label).await?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error_from_body(
            provider_label,
            status,
            &bytes,
        )));
    }

    if let Some(media_type) = image_media_type_from_content_type(&content_type) {
        return Ok(GeneratedImage {
            bytes,
            media_type,
            provider_image_url: None,
            usage: None,
            revised_prompt: None,
        });
    }

    let value = parse_json_body(provider_label, &bytes)?;
    let parsed = parse_openai_image_response(&value, output_format)?;
    let materialized =
        materialize_image_payload(client, &parsed.payload, &parsed.media_type, provider_label)
            .await?;
    Ok(GeneratedImage {
        bytes: materialized.bytes,
        media_type: materialized.media_type,
        provider_image_url: materialized.provider_image_url,
        usage: parsed.usage,
        revised_prompt: parsed.revised_prompt,
    })
}

fn build_openai_images_body(
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
    output_format: &str,
) -> Value {
    let mut body = json!({
        "model": model,
        "prompt": args.prompt.as_str(),
        "n": 1,
        "size": selected_image_size(config, args, model),
    });

    if should_send_openai_output_format(config, model) {
        body["output_format"] = json!(output_format);
        if let Some(compression) = args.output_compression {
            body["output_compression"] = json!(compression);
        }
    } else if is_dalle_model(model) {
        body["response_format"] = json!("b64_json");
    }

    if let Some(quality) = selected_image_quality(config, args, model) {
        body["quality"] = json!(quality);
    }
    if !is_dalle_model(model) {
        if let Some(background) = args
            .background
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["background"] = json!(background);
        }
    }
    body
}

fn is_gpt_image_25(model: &str) -> bool {
    matches!(model, "gpt-image-2.5-flare" | "gpt-image-2.5-sunburst")
}

fn validate_image_options(
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
    provider: ImageProvider,
    output_format: &str,
) -> Result<(), CoreError> {
    let invalid = |message: &str| CoreError::InvalidInput(message.to_string());
    if provider == ImageProvider::Xai {
        build_xai_images_body(config, args, model)?;
        if args
            .background
            .as_deref()
            .is_some_and(|value| value != "auto")
            || args.output_compression.is_some()
        {
            return Err(invalid(
                "xAI image generation does not accept background or output_compression.",
            ));
        }
        return Ok(());
    }
    if !model.starts_with("gpt-image-") {
        return Ok(());
    }
    if let Some(quality) = selected_image_quality(config, args, model) {
        let supported = matches!(quality, "auto" | "low" | "medium" | "high")
            || (is_gpt_image_25(model) && matches!(quality, "xhigh" | "max"));
        if !supported {
            return Err(invalid(
                "Unsupported GPT Image quality. xhigh and max require GPT Image 2.5.",
            ));
        }
    }
    if args
        .output_compression
        .is_some_and(|compression| compression > 100)
        || (args.output_compression.is_some() && !matches!(output_format, "jpeg" | "webp"))
    {
        return Err(invalid(
            "output_compression must be 0-100 and requires jpeg or webp.",
        ));
    }
    if args.background.as_deref() == Some("transparent") && output_format == "jpeg" {
        return Err(invalid("Transparent backgrounds require png or webp."));
    }
    let size = selected_image_size(config, args, model);
    if matches!(model, "gpt-image-1.5" | "gpt-image-1" | "gpt-image-1-mini")
        && !matches!(
            size.as_str(),
            "auto" | "1024x1024" | "1024x1536" | "1536x1024"
        )
    {
        return Err(invalid(
            "This GPT Image model supports only auto, 1024x1024, 1024x1536 or 1536x1024.",
        ));
    }
    if (is_gpt_image_25(model) || model == "gpt-image-2") && size != "auto" {
        let valid = size
            .split_once('x')
            .and_then(|(width, height)| {
                Some((width.parse::<u64>().ok()?, height.parse::<u64>().ok()?))
            })
            .is_some_and(|(width, height)| {
                width > 0
                    && height > 0
                    && width <= 3840
                    && height <= 3840
                    && width % 16 == 0
                    && height % 16 == 0
                    && width <= height * 3
                    && height <= width * 3
                    && (655_360..=8_294_400).contains(&(width * height))
            });
        if !valid {
            return Err(invalid("GPT Image size must use multiples of 16, at most 3840 per edge, a 1:3 to 3:1 ratio, and 655360-8294400 total pixels."));
        }
    }
    Ok(())
}

fn build_xai_images_body(
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
) -> Result<Value, CoreError> {
    let size = selected_text(args.size.as_deref(), config.size.as_deref(), "auto|1k");
    let (size_ratio, size_resolution) = size.split_once('|').unwrap_or((&size, "1k"));
    let ratio = args.aspect_ratio.as_deref().unwrap_or(size_ratio).trim();
    let resolution = args
        .resolution
        .as_deref()
        .unwrap_or(size_resolution)
        .trim()
        .to_ascii_lowercase();
    if ![
        "auto", "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "2:1", "1:2", "19.5:9",
        "9:19.5", "20:9", "9:20", "21:9", "5:2",
    ]
    .contains(&ratio)
    {
        return Err(CoreError::InvalidInput(
            "Unsupported xAI aspect ratio. Use size like 16:9|2k or aspect_ratio and resolution."
                .to_string(),
        ));
    }
    if !matches!(resolution.as_str(), "1k" | "2k") {
        return Err(CoreError::InvalidInput(
            "xAI resolution must be 1k or 2k.".to_string(),
        ));
    }
    let mut body = json!({ "model": model, "prompt": args.prompt, "n": 1, "aspect_ratio": ratio, "resolution": resolution, "response_format": "b64_json" });
    if let Some(quality) = selected_image_quality(config, args, model) {
        if model != "grok-imagine-image-2.0" || !matches!(quality, "auto" | "low" | "medium") {
            return Err(CoreError::InvalidInput(
                "xAI quality accepts auto, low or medium on grok-imagine-image-2.0 only."
                    .to_string(),
            ));
        }
        body["quality"] = json!(quality);
    }
    Ok(body)
}

fn should_send_openai_output_format(config: &ResolvedImageConfig, model: &str) -> bool {
    if is_dalle_model(model) {
        return false;
    }
    let haystack = format!(
        "{} {} {} {}",
        config.provider,
        config.api_style.as_deref().unwrap_or(""),
        config.base_url.as_deref().unwrap_or(""),
        model
    )
    .to_lowercase();
    !(haystack.contains("zhipu")
        || haystack.contains("bigmodel")
        || haystack.contains("cogview")
        || haystack.contains("glm-image"))
}

fn is_dalle_model(model: &str) -> bool {
    model.trim().to_lowercase().starts_with("dall-e")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImagePayload {
    Base64(String),
    Url(String),
}

#[derive(Debug, Clone)]
struct ParsedImageResponse {
    payload: ImagePayload,
    media_type: String,
    usage: Option<Value>,
    revised_prompt: Option<String>,
}

#[derive(Debug)]
struct MaterializedImage {
    bytes: Vec<u8>,
    media_type: String,
    provider_image_url: Option<String>,
}

fn parse_openai_image_response(
    value: &Value,
    output_format: &str,
) -> Result<ParsedImageResponse, CoreError> {
    if let Some(item) = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return parse_openai_image_item(item, value.get("usage").cloned(), value, output_format);
    }

    if let Some(item) = value
        .get("output")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("type").and_then(Value::as_str) == Some("image_generation_call")
            })
        })
    {
        return parse_responses_image_call(item, value.get("usage").cloned(), output_format);
    }

    if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
        if let Some(item) = value.get("item") {
            return parse_responses_image_call(item, value.get("usage").cloned(), output_format);
        }
    }

    if matches!(
        value.get("type").and_then(Value::as_str),
        Some("image_generation.completed") | Some("image_edit.completed")
    ) {
        if let Some(b64) = value.get("b64_json").and_then(Value::as_str) {
            return Ok(ParsedImageResponse {
                payload: ImagePayload::Base64(b64.to_string()),
                media_type: media_type_from_value(value, output_format),
                usage: value.get("usage").cloned(),
                revised_prompt: value
                    .get("revised_prompt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }

    if let Some(url) = value
        .pointer("/choices/0/message/images/0/image_url/url")
        .and_then(Value::as_str)
    {
        return Ok(ParsedImageResponse {
            payload: ImagePayload::Url(url.to_string()),
            media_type: media_type_from_data_url(url)
                .unwrap_or_else(|| media_type_for_format(output_format).to_string()),
            usage: value.get("usage").cloned(),
            revised_prompt: None,
        });
    }

    Err(CoreError::Llm(
        "OpenAI-compatible image response did not include a supported image payload. Expected Images API data[0].b64_json/url, Responses API image_generation_call.result, streaming b64_json, or chat image_url.".into(),
    ))
}

fn parse_openai_image_item(
    item: &Value,
    usage: Option<Value>,
    top_level: &Value,
    output_format: &str,
) -> Result<ParsedImageResponse, CoreError> {
    let media_type = explicit_media_type_from_value(item)
        .or_else(|| explicit_media_type_from_value(top_level))
        .unwrap_or_else(|| media_type_for_format(output_format).to_string());
    let revised_prompt = item
        .get("revised_prompt")
        .and_then(Value::as_str)
        .map(str::to_string);

    if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
        return Ok(ParsedImageResponse {
            payload: ImagePayload::Base64(b64.to_string()),
            media_type,
            usage,
            revised_prompt,
        });
    }
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        return Ok(ParsedImageResponse {
            payload: ImagePayload::Url(url.to_string()),
            media_type: media_type_from_data_url(url).unwrap_or(media_type),
            usage,
            revised_prompt,
        });
    }

    Err(CoreError::Llm(
        "OpenAI image response data[0] did not include b64_json or url.".into(),
    ))
}

fn parse_responses_image_call(
    item: &Value,
    usage: Option<Value>,
    output_format: &str,
) -> Result<ParsedImageResponse, CoreError> {
    if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
        return Err(CoreError::Llm(
            "OpenAI Responses image output item was not an image_generation_call.".into(),
        ));
    }
    let result = item.get("result").and_then(Value::as_str).ok_or_else(|| {
        CoreError::Llm("OpenAI Responses image_generation_call did not include result.".into())
    })?;
    Ok(ParsedImageResponse {
        payload: ImagePayload::Base64(result.to_string()),
        media_type: media_type_from_value(item, output_format),
        usage,
        revised_prompt: item
            .get("revised_prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn media_type_from_value(value: &Value, output_format: &str) -> String {
    explicit_media_type_from_value(value)
        .unwrap_or_else(|| media_type_for_format(output_format).to_string())
}

fn explicit_media_type_from_value(value: &Value) -> Option<String> {
    value
        .get("mime_type")
        .or_else(|| value.get("media_type"))
        .and_then(Value::as_str)
        .filter(|media_type| media_type.starts_with("image/"))
        .map(str::to_string)
        .or_else(|| {
            value
                .get("output_format")
                .and_then(Value::as_str)
                .map(media_type_for_format)
                .map(str::to_string)
        })
}

async fn materialize_image_payload(
    client: &reqwest::Client,
    payload: &ImagePayload,
    fallback_media_type: &str,
    provider: &str,
) -> Result<MaterializedImage, CoreError> {
    match payload {
        ImagePayload::Base64(b64) => {
            let media_type =
                media_type_from_data_url(b64).unwrap_or_else(|| fallback_media_type.to_string());
            Ok(MaterializedImage {
                bytes: decode_base64_image(b64, provider)?,
                media_type,
                provider_image_url: None,
            })
        }
        ImagePayload::Url(url) => {
            if let Some((media_type, b64)) = parse_image_data_url(url) {
                return Ok(MaterializedImage {
                    bytes: decode_base64_image(b64, provider)?,
                    media_type,
                    provider_image_url: None,
                });
            }
            Ok(MaterializedImage {
                bytes: download_image(client, url).await?,
                media_type: fallback_media_type.to_string(),
                provider_image_url: Some(url.to_string()),
            })
        }
    }
}

async fn generate_google_image(
    client: &reqwest::Client,
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
) -> Result<GeneratedImage, CoreError> {
    let base_url = config.endpoint_base_url("https://generativelanguage.googleapis.com/v1beta");
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/models/{model}:generateContent");
    let body = build_google_image_body(args, config.size.as_deref());

    let response = client
        .post(url)
        .header("x-goog-api-key", config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| CoreError::TransientLlm(format!("Google image request failed: {e}")))?;
    let value = read_provider_json(response, "Google Gemini image API").await?;

    let parts = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoreError::Llm(
                "Google image response did not include candidates[0].content.parts.".into(),
            )
        })?;

    for part in parts {
        let inline = part.get("inlineData").or_else(|| part.get("inline_data"));
        if let Some(inline) = inline {
            let data = inline.get("data").and_then(Value::as_str).ok_or_else(|| {
                CoreError::Llm("Google image inlineData was missing data.".into())
            })?;
            let media_type = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            return Ok(GeneratedImage {
                bytes: decode_base64_image(data, "Google")?,
                media_type,
                provider_image_url: None,
                usage: value.get("usageMetadata").cloned(),
                revised_prompt: None,
            });
        }
    }

    Err(CoreError::Llm(
        "Google image response did not include inline image data.".into(),
    ))
}

async fn generate_qwen_image(
    client: &reqwest::Client,
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
) -> Result<GeneratedImage, CoreError> {
    let url = config.qwen_endpoint();
    let body = build_qwen_image_body(config, args, model);

    let response = client
        .post(url)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| CoreError::TransientLlm(format!("Qwen image request failed: {e}")))?;
    let value = read_provider_json(response, "Qwen image API").await?;

    let image_url = value
        .get("output")
        .and_then(|output| output.get("choices"))
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|item| item.get("image"))
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("output")
                .and_then(|output| output.get("results"))
                .and_then(Value::as_array)
                .and_then(|results| results.first())
                .and_then(|item| item.get("url").or_else(|| item.get("image")))
                .and_then(Value::as_str)
        })
        .ok_or_else(|| {
            CoreError::Llm(
                "Qwen image response did not include output.choices[0].message.content[0].image or output.results[0].url."
                    .into(),
            )
        })?;

    Ok(GeneratedImage {
        bytes: download_image(client, image_url).await?,
        media_type: "image/png".to_string(),
        provider_image_url: Some(image_url.to_string()),
        usage: value.get("usage").cloned(),
        revised_prompt: value
            .pointer("/output/actual_prompt")
            .or_else(|| value.pointer("/output/results/0/orig_prompt"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn build_qwen_image_body(
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
) -> Value {
    let mut parameters = json!({
        "prompt_extend": args.effective_prompt_mode().provider_enhancement_enabled(),
        "watermark": args.watermark.unwrap_or(false),
    });
    if let Some(size) = selected_optional(args.size.as_deref(), config.size.as_deref()) {
        parameters["size"] = json!(size.replace('x', "*"));
    }
    if let Some(negative) = args
        .negative_prompt
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parameters["negative_prompt"] = json!(negative);
    }

    json!({
        "model": model,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{ "text": args.prompt.as_str() }]
            }]
        },
        "parameters": parameters
    })
}

async fn download_image(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, CoreError> {
    let parsed = Url::parse(url)
        .map_err(|e| CoreError::Llm(format!("Provider returned an invalid image URL: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CoreError::Llm(format!(
                "Provider returned unsupported image URL scheme '{other}'."
            )))
        }
    }
    let response =
        client.get(parsed).send().await.map_err(|e| {
            CoreError::TransientLlm(format!("Failed to download generated image: {e}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(CoreError::Llm(format!(
            "Generated image download failed with HTTP {status}."
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| CoreError::Llm(format!("Failed to read generated image bytes: {e}")))?;
    Ok(bytes.to_vec())
}

fn decode_base64_image(b64: &str, provider: &str) -> Result<Vec<u8>, CoreError> {
    let data = parse_image_data_url(b64)
        .map(|(_, data)| data)
        .unwrap_or(b64)
        .trim();
    STANDARD
        .decode(data)
        .or_else(|_| STANDARD_NO_PAD.decode(data))
        .or_else(|_| URL_SAFE.decode(data))
        .or_else(|_| URL_SAFE_NO_PAD.decode(data))
        .map_err(|e| {
            CoreError::Llm(format!(
                "{provider} returned invalid base64 image data: {e}"
            ))
        })
}

async fn read_provider_body(
    response: reqwest::Response,
    provider_name: &str,
) -> Result<(reqwest::StatusCode, String, Vec<u8>), CoreError> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = response.bytes().await.map_err(|e| {
        CoreError::Llm(format!("Failed to read {provider_name} response body: {e}"))
    })?;
    Ok((status, content_type, bytes.to_vec()))
}

async fn read_provider_json(
    response: reqwest::Response,
    provider_name: &str,
) -> Result<Value, CoreError> {
    let (status, _content_type, bytes) = read_provider_body(response, provider_name).await?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error_from_body(
            provider_name,
            status,
            &bytes,
        )));
    }
    parse_json_body(provider_name, &bytes)
}

fn parse_json_body(provider_name: &str, bytes: &[u8]) -> Result<Value, CoreError> {
    serde_json::from_slice(bytes).map_err(|e| {
        CoreError::Llm(format!(
            "{provider_name} returned a non-JSON response: {e}. Body preview: {}",
            response_body_preview(bytes)
        ))
    })
}

fn provider_error_from_body(name: &str, status: reqwest::StatusCode, bytes: &[u8]) -> String {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => provider_error(name, status, &value),
        Err(_) => format!(
            "{name} returned HTTP {status} with a non-JSON body: {}",
            response_body_preview(bytes)
        ),
    }
}

fn provider_error(name: &str, status: reqwest::StatusCode, value: &Value) -> String {
    let message = [
        value.pointer("/error/message"),
        value.pointer("/error/code"),
        value.get("message"),
        value.get("Message"),
        value.get("msg"),
        value.get("error"),
        value.get("code"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .unwrap_or("no error message returned");
    format!("{name} returned HTTP {status}: {message}")
}

fn response_body_preview(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "<empty>".to_string();
    }
    let text = String::from_utf8_lossy(bytes);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview: String = collapsed.chars().take(500).collect();
    if collapsed.chars().count() > 500 {
        preview.push_str("...");
    }
    preview
}

fn image_media_type_from_content_type(content_type: &str) -> Option<String> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if media_type.starts_with("image/") {
        Some(media_type)
    } else {
        None
    }
}

fn parse_image_data_url(data_url: &str) -> Option<(String, &str)> {
    let rest = data_url.trim().strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    if !header.to_ascii_lowercase().contains(";base64") {
        return None;
    }
    let media_type = header.split(';').next()?.trim().to_lowercase();
    if !media_type.starts_with("image/") {
        return None;
    }
    Some((media_type, data))
}

fn media_type_from_data_url(data_url: &str) -> Option<String> {
    parse_image_data_url(data_url).map(|(media_type, _)| media_type)
}

fn media_type_for_format(format: &str) -> &'static str {
    match format {
        "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn extension_for_format(format: &str) -> &'static str {
    match format {
        "jpeg" => "jpg",
        "webp" => "webp",
        _ => "png",
    }
}

fn extension_for_media_type(media_type: &str) -> Option<String> {
    match media_type.to_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg".into()),
        "image/webp" => Some("webp".into()),
        "image/gif" => Some("gif".into()),
        "image/png" => Some("png".into()),
        _ => None,
    }
}

fn resolve_preview_path(
    args: &GenerateImageArgs,
    extension: &str,
) -> Result<(PathBuf, String), CoreError> {
    let suggested_filename = resolve_filename(args.filename.as_deref(), extension);
    let preview_filename = preview_cache_filename(&suggested_filename, extension);
    let preview_dir = std::env::temp_dir()
        .join("nexa")
        .join("generated-image-previews");
    Ok((preview_dir.join(preview_filename), suggested_filename))
}

fn resolve_filename(filename: Option<&str>, extension: &str) -> String {
    let extension = extension.trim_start_matches('.');
    let raw = filename
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("generated-image-{}.{}", Uuid::new_v4(), extension));
    let mut safe: String = raw
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => ch,
        })
        .collect();
    match Path::new(&safe)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some(current_extension) if current_extension.eq_ignore_ascii_case(extension) => {}
        _ => {
            let mut path = PathBuf::from(&safe);
            path.set_extension(extension);
            safe = path.to_string_lossy().to_string();
        }
    }
    safe
}

fn preview_cache_filename(suggested_filename: &str, extension: &str) -> String {
    let stem = Path::new(suggested_filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generated-image");
    format!("{stem}-{}.{}", Uuid::new_v4(), extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args() -> GenerateImageArgs {
        GenerateImageArgs {
            prompt: "a precise product photo".to_string(),
            prompt_mode: Some(ImagePromptMode::Verbatim),
            prompt_extend: None,
            provider: None,
            api_style: None,
            provider_config_id: None,
            model: None,
            size: None,
            quality: None,
            aspect_ratio: None,
            resolution: None,
            output_compression: None,
            background: None,
            output_format: None,
            negative_prompt: None,
            watermark: None,
            filename: None,
        }
    }

    fn test_config(provider: &str, base_url: Option<&str>) -> ResolvedImageConfig {
        ResolvedImageConfig {
            provider: provider.to_string(),
            api_style: Some("openai_images".to_string()),
            api_key: "test-key".to_string(),
            base_url: base_url.map(str::to_string),
            model: None,
            size: None,
            quality: None,
            output_format: None,
        }
    }

    #[test]
    fn image_25_transmits_extended_quality_and_transparent_output() {
        let config = test_config("open_ai", Some("https://api.openai.com/v1"));
        let mut args = test_args();
        args.size = Some("3840x2160".to_string());
        args.quality = Some("max".to_string());
        args.background = Some("transparent".to_string());
        args.output_compression = Some(85);
        for model in ["gpt-image-2.5-flare", "gpt-image-2.5-sunburst"] {
            validate_image_options(&config, &args, model, ImageProvider::OpenAi, "webp").unwrap();
            let body = build_openai_images_body(&config, &args, model, "webp");
            assert_eq!(body["model"], model);
            assert_eq!(body["quality"], "max");
            assert_eq!(body["size"], "3840x2160");
            assert_eq!(body["background"], "transparent");
            assert_eq!(body["output_compression"], 85);
        }
        assert!(validate_image_options(
            &config,
            &args,
            "gpt-image-2",
            ImageProvider::OpenAi,
            "webp"
        )
        .is_err());
        assert!(validate_image_options(
            &config,
            &args,
            "gpt-image-2.5-flare",
            ImageProvider::OpenAi,
            "jpeg"
        )
        .is_err());
        args.size = Some("3840x3840".to_string());
        assert!(validate_image_options(
            &config,
            &args,
            "gpt-image-2.5-flare",
            ImageProvider::OpenAi,
            "webp"
        )
        .is_err());
    }

    #[test]
    fn xai_image_2_uses_its_own_wire_contract() {
        let config = test_config("open_ai", Some("https://api.x.ai/v1"));
        let mut args = test_args();
        args.size = Some("16:9|2k".to_string());
        args.quality = Some("medium".to_string());
        let body = build_xai_images_body(&config, &args, "grok-imagine-image-2.0").unwrap();
        assert_eq!(
            body,
            json!({ "model":"grok-imagine-image-2.0", "prompt":args.prompt, "n":1, "aspect_ratio":"16:9", "resolution":"2k", "quality":"medium", "response_format":"b64_json" })
        );
        args.quality = Some("high".to_string());
        assert!(build_xai_images_body(&config, &args, "grok-imagine-image-2.0").is_err());
        args.quality = None;
        args.aspect_ratio = Some("9:20".to_string());
        args.resolution = Some("1k".to_string());
        let body = build_xai_images_body(&config, &args, "grok-imagine-image").unwrap();
        assert_eq!(body["aspect_ratio"], "9:20");
        assert_eq!(body["resolution"], "1k");
        assert!(body.get("quality").is_none());
        args.resolution = Some("4k".to_string());
        assert!(build_xai_images_body(&config, &args, "grok-imagine-image-2.0").is_err());
    }

    #[test]
    fn image_model_override_does_not_inherit_another_models_options() {
        let mut config = test_config("open_ai", Some("https://api.x.ai/v1"));
        config.model = Some("grok-imagine-image-2.0".to_string());
        config.quality = Some("medium".to_string());
        let mut args = test_args();
        let legacy = build_xai_images_body(&config, &args, "grok-imagine-image").unwrap();
        assert!(legacy.get("quality").is_none());
        assert_eq!(
            build_xai_images_body(&config, &args, "grok-imagine-image-2.0").unwrap()["quality"],
            "medium"
        );
        args.quality = Some("medium".to_string());
        assert!(build_xai_images_body(&config, &args, "grok-imagine-image").is_err());

        args.quality = None;
        config.model = Some("gpt-image-2.5-flare".to_string());
        config.quality = Some("max".to_string());
        validate_image_options(&config, &args, "gpt-image-2", ImageProvider::OpenAi, "png")
            .unwrap();
        assert!(
            build_openai_images_body(&config, &args, "gpt-image-2", "png")
                .get("quality")
                .is_none()
        );
        assert_eq!(
            build_openai_images_body(&config, &args, "gpt-image-2.5-flare", "png")["quality"],
            "max"
        );
        config.size = Some("3840x2160".to_string());
        assert_eq!(
            build_openai_images_body(&config, &args, "gpt-image-2.5-flare", "png")["size"],
            "3840x2160"
        );
        for model in ["gpt-image-1.5", "gpt-image-1", "gpt-image-1-mini"] {
            assert_eq!(
                build_openai_images_body(&config, &args, model, "png")["size"],
                "1024x1024"
            );
            validate_image_options(&config, &args, model, ImageProvider::OpenAi, "png").unwrap();
            args.size = Some("3840x2160".to_string());
            assert!(
                validate_image_options(&config, &args, model, ImageProvider::OpenAi, "png")
                    .is_err()
            );
            args.size = Some("1536x1024".to_string());
            validate_image_options(&config, &args, model, ImageProvider::OpenAi, "png").unwrap();
            assert_eq!(
                build_openai_images_body(&config, &args, model, "png")["size"],
                "1536x1024"
            );
            args.size = None;
        }
    }

    #[tokio::test]
    async fn image_adapters_send_the_selected_model_and_decode_the_http_result() {
        use std::io::{Read, Write};
        for (provider, model, quality) in [
            (ImageProvider::OpenAi, "gpt-image-2.5-flare", "xhigh"),
            (ImageProvider::OpenAi, "gpt-image-2.5-sunburst", "max"),
            (ImageProvider::Xai, "grok-imagine-image-2.0", "medium"),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0u8; 2048];
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(offset) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&request[..offset]).to_lowercase();
                        assert!(headers.starts_with("post /v1/images/generations "));
                        assert!(headers.contains("authorization: bearer test-key"));
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (offset + 4, length);
                    }
                };
                while request.len() < header_end + length {
                    let mut chunk = [0u8; 2048];
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                }
                let body: Value =
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                let response =
                    json!({ "data": [{ "b64_json": STANDARD.encode(b"test-image-bytes") }] })
                        .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
                body
            });
            let config = test_config("open_ai", Some(&format!("http://{address}/v1")));
            let mut args = test_args();
            args.quality = Some(quality.to_string());
            if provider == ImageProvider::Xai {
                args.size = Some("9:16|2k".to_string());
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let result = generate_openai_image(&client, &config, &args, model, "jpeg", provider)
                .await
                .unwrap();
            assert_eq!(result.bytes, b"test-image-bytes");
            assert_eq!(result.media_type, "image/jpeg");
            let body = server.join().unwrap();
            assert_eq!(body["model"], model);
            assert_eq!(body["quality"], quality);
            if provider == ImageProvider::Xai {
                assert_eq!(body["aspect_ratio"], "9:16");
                assert!(body.get("size").is_none());
                assert!(body.get("output_format").is_none());
            } else {
                assert_eq!(body["output_format"], "jpeg");
            }
        }
    }

    #[test]
    fn tool_contract_exposes_three_auditable_prompt_policies() {
        let definition: Value = serde_json::from_str(DEF_JSON).expect("valid tool definition");
        let prompt_mode = &definition["parameters"]["properties"]["prompt_mode"];

        assert_eq!(
            prompt_mode["enum"],
            json!(["verbatim", "agent_refined", "provider_enhanced"])
        );
        assert_eq!(prompt_mode["default"], "verbatim");
        assert!(!definition["parameters"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("prompt_mode"))));
        assert!(prompt_mode["description"]
            .as_str()
            .is_some_and(|description| description.contains("verbatim")));
        let legacy_prompt_extend = &definition["parameters"]["properties"]["prompt_extend"];
        assert_eq!(legacy_prompt_extend["type"], "boolean");
        assert!(legacy_prompt_extend["description"]
            .as_str()
            .is_some_and(|description| description.contains("Deprecated compatibility")));
    }

    #[test]
    fn prompt_mode_defaults_to_verbatim_for_legacy_callers() {
        let args: GenerateImageArgs = serde_json::from_value(json!({
            "prompt": "  保留标点：猫。\r\n--style raw  "
        }))
        .expect("legacy arguments parse");

        assert_eq!(args.effective_prompt_mode(), ImagePromptMode::Verbatim);
        assert_eq!(args.prompt, "  保留标点：猫。\r\n--style raw  ");
    }

    #[test]
    fn legacy_prompt_extend_preserves_explicit_qwen_enhancement() {
        let config = test_config("qwen", None);
        for field in ["prompt_extend", "promptExtend"] {
            let mut value = json!({
                "prompt": "A legacy prompt that requested enhancement"
            });
            value[field] = json!(true);
            let args: GenerateImageArgs =
                serde_json::from_value(value).expect("legacy prompt enhancement arguments parse");

            assert_eq!(
                args.effective_prompt_mode(),
                ImagePromptMode::ProviderEnhanced
            );
            assert_eq!(
                build_qwen_image_body(&config, &args, "qwen-image-plus")["parameters"]
                    ["prompt_extend"],
                true
            );
        }

        let args: GenerateImageArgs = serde_json::from_value(json!({
            "prompt": "Explicit new policy wins",
            "prompt_mode": "verbatim",
            "prompt_extend": true
        }))
        .expect("mixed legacy and current arguments parse");
        assert_eq!(args.effective_prompt_mode(), ImagePromptMode::Verbatim);
    }

    #[test]
    fn adapters_preserve_prompt_bytes_and_only_enhance_on_explicit_mode() {
        let exact_prompt = "  保留标点：猫。\r\n--style raw  ";
        let mut args = test_args();
        args.prompt = exact_prompt.to_string();
        let config = test_config("qwen", None);

        let openai = build_openai_images_body(&config, &args, "gpt-image-2", "png");
        let google = build_google_image_body(&args, None);
        let qwen = build_qwen_image_body(&config, &args, "qwen-image-plus");
        assert_eq!(openai["prompt"], exact_prompt);
        assert_eq!(google["contents"][0]["parts"][0]["text"], exact_prompt);
        assert_eq!(
            qwen["input"]["messages"][0]["content"][0]["text"],
            exact_prompt
        );
        assert_eq!(qwen["parameters"]["prompt_extend"], false);
        assert!(supports_explicit_prompt_enhancement(ImageProvider::Qwen));
        assert!(!supports_explicit_prompt_enhancement(ImageProvider::OpenAi));
        assert!(!supports_explicit_prompt_enhancement(ImageProvider::Google));

        args.prompt_mode = Some(ImagePromptMode::AgentRefined);
        assert_eq!(
            build_qwen_image_body(&config, &args, "qwen-image-plus")["parameters"]["prompt_extend"],
            false
        );
        args.prompt_mode = Some(ImagePromptMode::ProviderEnhanced);
        assert_eq!(
            build_qwen_image_body(&config, &args, "qwen-image-plus")["parameters"]["prompt_extend"],
            true
        );
    }

    #[test]
    fn openai_parser_accepts_images_api_b64() {
        let image_b64 = STANDARD.encode(b"fake-png");
        let value = json!({
            "created": 1713833628,
            "data": [{
                "b64_json": image_b64,
                "mime_type": "image/png",
                "revised_prompt": "a refined prompt"
            }],
            "usage": { "total_tokens": 12 }
        });

        let parsed = parse_openai_image_response(&value, "png").expect("parse image");

        assert_eq!(parsed.payload, ImagePayload::Base64(image_b64));
        assert_eq!(parsed.media_type, "image/png");
        assert_eq!(parsed.revised_prompt.as_deref(), Some("a refined prompt"));
        assert_eq!(parsed.usage.as_ref().unwrap()["total_tokens"], 12);
    }

    #[test]
    fn openai_parser_accepts_data_url_image_payloads() {
        let data_url = format!("data:image/webp;base64,{}", STANDARD.encode(b"fake-webp"));
        let value = json!({
            "data": [{ "url": data_url }]
        });

        let parsed = parse_openai_image_response(&value, "png").expect("parse data url");

        assert_eq!(parsed.media_type, "image/webp");
        assert_eq!(parsed.payload, ImagePayload::Url(data_url.clone()));
        assert_eq!(
            decode_base64_image(&data_url, "OpenAI").unwrap(),
            b"fake-webp"
        );
    }

    #[test]
    fn openai_parser_accepts_responses_image_generation_call() {
        let image_b64 = STANDARD.encode(b"responses-png");
        let value = json!({
            "output": [{
                "type": "image_generation_call",
                "result": image_b64,
                "revised_prompt": "revised by mainline model"
            }]
        });

        let parsed = parse_openai_image_response(&value, "png").expect("parse responses call");

        assert_eq!(parsed.payload, ImagePayload::Base64(image_b64));
        assert_eq!(
            parsed.revised_prompt.as_deref(),
            Some("revised by mainline model")
        );
    }

    #[test]
    fn openai_parser_accepts_stream_completion_event() {
        let image_b64 = STANDARD.encode(b"stream-png");
        let value = json!({
            "type": "image_generation.completed",
            "b64_json": image_b64,
            "output_format": "jpeg",
            "usage": { "total_tokens": 7 }
        });

        let parsed = parse_openai_image_response(&value, "png").expect("parse stream event");

        assert_eq!(parsed.payload, ImagePayload::Base64(image_b64));
        assert_eq!(parsed.media_type, "image/jpeg");
        assert_eq!(parsed.usage.as_ref().unwrap()["total_tokens"], 7);
    }

    #[test]
    fn provider_error_preserves_non_json_body_preview() {
        let message = provider_error_from_body(
            "OpenAI image API",
            reqwest::StatusCode::BAD_GATEWAY,
            b"<html><body>upstream unavailable</body></html>",
        );

        assert!(message.contains("HTTP 502 Bad Gateway"));
        assert!(message.contains("non-JSON body"));
        assert!(message.contains("upstream unavailable"));
    }

    #[test]
    fn openai_body_uses_output_format_only_when_supported() {
        let args = test_args();
        let openai = test_config("open_ai", Some("https://api.openai.com/v1"));
        let zhipu = test_config("zhipu", Some("https://open.bigmodel.cn/api/paas/v4"));

        let gpt_body = build_openai_images_body(&openai, &args, "gpt-image-2", "webp");
        assert_eq!(gpt_body["output_format"], "webp");
        assert!(gpt_body.get("response_format").is_none());

        let dalle_body = build_openai_images_body(&openai, &args, "dall-e-3", "png");
        assert!(dalle_body.get("output_format").is_none());
        assert_eq!(dalle_body["response_format"], "b64_json");

        let zhipu_body = build_openai_images_body(&zhipu, &args, "glm-image", "png");
        assert!(zhipu_body.get("output_format").is_none());
        assert!(zhipu_body.get("response_format").is_none());
    }

    #[test]
    fn generated_images_materialize_as_transient_previews() {
        let mut args = test_args();
        args.filename = Some("launch poster".to_string());

        let (preview_path, suggested_filename) =
            resolve_preview_path(&args, "png").expect("preview path");

        assert_eq!(suggested_filename, "launch poster.png");
        assert!(preview_path.starts_with(
            std::env::temp_dir()
                .join("nexa")
                .join("generated-image-previews")
        ));
        assert!(!preview_path.to_string_lossy().contains("generated-images"));
    }

    #[test]
    fn generated_image_suggested_filename_matches_actual_format() {
        let mut args = test_args();
        args.filename = Some("launch poster.jpg".to_string());

        let (_preview_path, suggested_filename) =
            resolve_preview_path(&args, "png").expect("preview path");

        assert_eq!(suggested_filename, "launch poster.png");
    }
}
