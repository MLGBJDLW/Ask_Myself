//! GenerateImageTool — text-to-image generation through configured providers.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
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
    let mut body = json!({
        "model": model,
        "prompt": args.prompt.as_str(),
        "n": 1,
        "size": selected_text(args.size.as_deref(), config.size.as_deref(), "1024x1024"),
        "output_format": output_format,
    });

    if let Some(quality) = selected_optional(args.quality.as_deref(), config.quality.as_deref()) {
        body["quality"] = json!(quality);
    }
    if let Some(background) = args
        .background
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        body["background"] = json!(background);
    }

    let response = client
        .post(url)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| CoreError::TransientLlm(format!("OpenAI image request failed: {e}")))?;

    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| CoreError::Llm(format!("Failed to parse OpenAI image response: {e}")))?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error(
            "OpenAI image API",
            status,
            &value,
        )));
    }

    let item = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| CoreError::Llm("OpenAI image response did not include data[0].".into()))?;

    let media_type = item
        .get("mime_type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| media_type_for_format(output_format))
        .to_string();

    if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
        let bytes = decode_base64_image(b64, "OpenAI")?;
        return Ok(GeneratedImage {
            bytes,
            media_type,
            provider_image_url: None,
            usage: value.get("usage").cloned(),
        });
    }

    let image_url = item.get("url").and_then(Value::as_str).ok_or_else(|| {
        CoreError::Llm("OpenAI image response did not include b64_json or url.".into())
    })?;
    let bytes = download_image(client, image_url).await?;
    Ok(GeneratedImage {
        bytes,
        media_type,
        provider_image_url: Some(image_url.to_string()),
        usage: value.get("usage").cloned(),
    })
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
        "responseModalities": ["Image"]
    });
    if let Some(image_config) = google_image_config(args.size.as_deref().or(config.size.as_deref()))
    {
        generation_config["imageConfig"] = image_config;
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
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| CoreError::Llm(format!("Failed to parse Google image response: {e}")))?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error(
            "Google Gemini image API",
            status,
            &value,
        )));
    }

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
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| CoreError::Llm(format!("Failed to parse Qwen image response: {e}")))?;
    if !status.is_success() {
        return Err(CoreError::Llm(provider_error(
            "Qwen image API",
            status,
            &value,
        )));
    }

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
        .ok_or_else(|| {
            CoreError::Llm(
                "Qwen image response did not include output.choices[0].message.content[0].image."
                    .into(),
            )
        })?;

    Ok(GeneratedImage {
        bytes: download_image(client, image_url).await?,
        media_type: "image/png".to_string(),
        provider_image_url: Some(image_url.to_string()),
        usage: value.get("usage").cloned(),
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
    general_purpose::STANDARD.decode(b64).map_err(|e| {
        CoreError::Llm(format!(
            "{provider} returned invalid base64 image data: {e}"
        ))
    })
}

fn provider_error(name: &str, status: reqwest::StatusCode, value: &Value) -> String {
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("no error message returned");
    format!("{name} returned HTTP {status}: {message}")
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
