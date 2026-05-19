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

use crate::db::Database;
use crate::error::CoreError;
use crate::plugins::image_generation::{
    ImageGenerationRequest, ImageProvider, ResolvedImageConfig,
};

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_image.json");

pub struct GenerateImageTool;

#[derive(Debug, Deserialize)]
struct GenerateImageArgs {
    prompt: String,
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
    #[serde(default)]
    background: Option<String>,
    #[serde(default, alias = "outputFormat")]
    output_format: Option<String>,
    #[serde(default, alias = "negativePrompt")]
    negative_prompt: Option<String>,
    #[serde(default, alias = "promptExtend")]
    prompt_extend: Option<bool>,
    #[serde(default)]
    watermark: Option<bool>,
    #[serde(default, alias = "outputDir")]
    output_dir: Option<String>,
    #[serde(default, alias = "outputPath")]
    output_path: Option<String>,
    #[serde(default)]
    filename: Option<String>,
}

impl GenerateImageArgs {
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
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GenerateImageArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_image arguments: {e}"))
        })?;

        let prompt = args.prompt.trim();
        if prompt.is_empty() {
            return Ok(error_result(call_id, "Image prompt cannot be empty."));
        }
        if prompt.chars().count() > 32_000 {
            return Ok(error_result(
                call_id,
                "Image prompt is too long; keep it under 32000 characters.",
            ));
        }

        let runtime =
            crate::plugins::image_generation::resolve_runtime(db, &args.runtime_request())?;
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
            ImageProvider::OpenAi => {
                generate_openai_image(
                    &client,
                    &runtime.config,
                    &args,
                    &runtime.model,
                    runtime.output_format,
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
        let output_path = resolve_output_path(db, &args, extension)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &generated.bytes)?;

        let size_bytes = generated.bytes.len();
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "Generated image saved.\nProvider: {}\nModel: {}\nPath: {}\nSize: {} bytes",
                runtime.provider_name,
                runtime.model,
                output_path.display(),
                size_bytes
            ),
            is_error: false,
            artifacts: Some(json!({
                "kind": "generatedImage",
                "provider": runtime.provider_name,
                "model": runtime.model,
                "path": output_path.to_string_lossy(),
                "mediaType": media_type,
                "bytes": size_bytes,
                "prompt": prompt,
                "revisedPrompt": generated.revised_prompt,
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

async fn generate_openai_image(
    client: &reqwest::Client,
    config: &ResolvedImageConfig,
    args: &GenerateImageArgs,
    model: &str,
    output_format: &str,
) -> Result<GeneratedImage, CoreError> {
    let base_url = config.endpoint_base_url("https://api.openai.com/v1");
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/images/generations");
    let body = build_openai_images_body(config, args, model, output_format);

    let response = client
        .post(url)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| CoreError::TransientLlm(format!("OpenAI image request failed: {e}")))?;

    let (status, content_type, bytes) = read_provider_body(response, "OpenAI image API").await?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error_from_body(
            "OpenAI image API",
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

    let value = parse_json_body("OpenAI image API", &bytes)?;
    let parsed = parse_openai_image_response(&value, output_format)?;
    let materialized =
        materialize_image_payload(client, &parsed.payload, &parsed.media_type, "OpenAI").await?;
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
        "size": selected_text(args.size.as_deref(), config.size.as_deref(), "1024x1024"),
    });

    if should_send_openai_output_format(config, model) {
        body["output_format"] = json!(output_format);
    } else if is_dalle_model(model) {
        body["response_format"] = json!("b64_json");
    }

    if let Some(quality) = selected_optional(args.quality.as_deref(), config.quality.as_deref()) {
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
    let mut generation_config = json!({
        "responseModalities": ["TEXT", "IMAGE"]
    });
    if let Some(image_config) = google_image_config(args.size.as_deref().or(config.size.as_deref()))
    {
        generation_config["responseFormat"] = json!({ "image": image_config });
    }
    let body = json!({
        "contents": [{
            "parts": [{ "text": args.prompt.as_str() }]
        }],
        "generationConfig": generation_config
    });

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
    let mut parameters = json!({
        "prompt_extend": args.prompt_extend.unwrap_or(true),
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

    let body = json!({
        "model": model,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{ "text": args.prompt.as_str() }]
            }]
        },
        "parameters": parameters
    });

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

fn resolve_output_path(
    db: &Database,
    args: &GenerateImageArgs,
    extension: String,
) -> Result<PathBuf, CoreError> {
    let path = if let Some(output_path) = args
        .output_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let requested = PathBuf::from(output_path);
        if requested.is_absolute() {
            requested
        } else {
            default_output_dir(db)?.join(requested)
        }
    } else {
        let dir = if let Some(output_dir) = args
            .output_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let requested = PathBuf::from(output_dir);
            if requested.is_absolute() {
                requested
            } else {
                default_output_dir(db)?.join(requested)
            }
        } else {
            default_output_dir(db)?
        };
        dir.join(resolve_filename(args.filename.as_deref(), &extension))
    };

    validate_output_path(db, &path)?;
    Ok(path)
}

fn default_output_dir(db: &Database) -> Result<PathBuf, CoreError> {
    let sources = db.list_sources()?;
    if let Some(source) = sources.first() {
        return Ok(PathBuf::from(&source.root_path).join("generated-images"));
    }
    Ok(std::env::current_dir()?.join("generated-images"))
}

fn resolve_filename(filename: Option<&str>, extension: &str) -> String {
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
    if Path::new(&safe).extension().is_none() {
        safe.push('.');
        safe.push_str(extension);
    }
    safe
}

fn validate_output_path(db: &Database, path: &Path) -> Result<(), CoreError> {
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CoreError::InvalidInput(
            "Image output path must not contain '..'.".into(),
        ));
    }

    let parent = path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Image output path has no parent directory.".into())
    })?;
    std::fs::create_dir_all(parent)?;
    let canonical_parent = std::fs::canonicalize(parent)?;
    let target = canonical_parent.join(
        path.file_name()
            .ok_or_else(|| CoreError::InvalidInput("Image output path has no filename.".into()))?,
    );

    let sources = db.list_sources()?;
    if sources.is_empty() {
        let current = std::fs::canonicalize(std::env::current_dir()?)?;
        if target.starts_with(&current) {
            return Ok(());
        }
        return Err(CoreError::InvalidInput(format!(
            "Image output path must stay under the current directory when no sources are registered: {}",
            current.display()
        )));
    }

    for source in sources {
        let root = PathBuf::from(source.root_path);
        if let Ok(canonical_root) = std::fs::canonicalize(root) {
            if target.starts_with(canonical_root) {
                return Ok(());
            }
        }
    }

    Err(CoreError::InvalidInput(
        "Image output path must stay inside a registered source root.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args() -> GenerateImageArgs {
        GenerateImageArgs {
            prompt: "a precise product photo".to_string(),
            provider: None,
            api_style: None,
            provider_config_id: None,
            model: None,
            size: None,
            quality: None,
            background: None,
            output_format: None,
            negative_prompt: None,
            prompt_extend: None,
            watermark: None,
            output_dir: None,
            output_path: None,
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
}
