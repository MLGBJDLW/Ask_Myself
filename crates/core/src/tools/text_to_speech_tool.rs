//! Text-to-speech generation through configured cloud providers.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app_settings::TextToSpeechConfig;
use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolInputStreamingMode, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/synthesize_speech.json");
const MAX_GENERATED_AUDIO_BYTES: usize = 32 * 1024 * 1024;
const MAX_MINIMAX_RESPONSE_BYTES: usize = MAX_GENERATED_AUDIO_BYTES * 2 + 1024 * 1024;

pub struct SynthesizeSpeechTool;

#[derive(Debug, Deserialize)]
struct SynthesizeSpeechArgs {
    text: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    filename: Option<String>,
}

#[derive(Debug)]
struct GeneratedSpeech {
    bytes: Vec<u8>,
    media_type: String,
}

#[async_trait]
impl Tool for SynthesizeSpeechTool {
    fn name(&self) -> &str {
        "synthesize_speech"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        // Keep speech synthesis discoverable for direct narration requests while
        // retaining its network classification for policy and approval UIs.
        &[ToolCategory::Core, ToolCategory::Web]
    }

    fn input_streaming(&self) -> ToolInputStreamingMode {
        ToolInputStreamingMode::UiPreview
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SynthesizeSpeechArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid synthesize_speech arguments: {error}"))
        })?;
        let text = args.text.trim();
        if text.is_empty() {
            return Ok(error_result(call_id, "Speech text cannot be empty."));
        }
        if text.chars().count() > 20_000 {
            return Ok(error_result(
                call_id,
                "Speech text is too long; keep a single request under 20000 characters.",
            ));
        }

        let config = db.load_app_config()?.text_to_speech;
        if !config.is_configured() {
            return Ok(error_result(
                call_id,
                "Text-to-speech is not configured. Add a provider API key, model, and voice in Settings.",
            ));
        }
        let model = selected(args.model.as_deref(), &config.model);
        let voice = selected(args.voice.as_deref(), &config.voice);
        let speed = args.speed.unwrap_or(config.speed).clamp(0.5, 2.0);
        let client = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|error| {
                CoreError::InvalidInput(format!("Failed to build HTTP client: {error}"))
            })?;

        let generated = match config.api_style.as_str() {
            "elevenlabs_speech" => {
                synthesize_elevenlabs(&client, &config, text, &model, &voice, speed).await?
            }
            "minimax_speech" => {
                synthesize_minimax(&client, &config, text, &model, &voice, speed).await?
            }
            _ => synthesize_openai(&client, &config, text, &model, &voice, speed).await?,
        };

        let extension = extension_for_media_type(&generated.media_type);
        let suggested_filename = safe_filename(args.filename.as_deref(), extension);
        let preview_path = std::env::temp_dir()
            .join("nexa")
            .join("generated-audio-previews")
            .join(format!(
                "{}-{}.{}",
                file_stem(&suggested_filename),
                Uuid::new_v4(),
                extension
            ));
        if let Some(parent) = preview_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&preview_path, &generated.bytes)?;

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "Speech preview ready. It has not been saved to the workspace.\nProvider: {}\nModel: {}\nVoice: {}\nSize: {} bytes\nPreview: {}",
                config.provider,
                model,
                voice,
                generated.bytes.len(),
                preview_path.to_string_lossy(),
            ),
            is_error: false,
            artifacts: Some(json!({
                "kind": "generatedAudio",
                "provider": config.provider,
                "model": model,
                "voice": voice,
                "path": preview_path.to_string_lossy(),
                "previewPath": preview_path.to_string_lossy(),
                "suggestedFilename": suggested_filename,
                "mediaType": generated.media_type,
                "bytes": generated.bytes.len(),
                "saved": false,
                "transient": true,
            })),
        })
    }
}

fn selected(override_value: Option<&str>, configured: &str) -> String {
    override_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(configured.trim())
        .to_string()
}

async fn synthesize_openai(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
    text: &str,
    model: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let format = normalize_format(&config.output_format);
    let endpoint = format!("{}/audio/speech", base_url(config).trim_end_matches('/'));
    let response = client
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&json!({
            "model": model,
            "input": text,
            "voice": voice,
            "response_format": format,
            "speed": speed,
        }))
        .send()
        .await
        .map_err(|error| {
            CoreError::TransientLlm(format!("OpenAI speech request failed: {error}"))
        })?;
    binary_response(response, "OpenAI Speech", media_type_for_format(format)).await
}

async fn synthesize_elevenlabs(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
    text: &str,
    model: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let mut endpoint = Url::parse(&format!("{}/", base_url(config).trim_end_matches('/')))
        .map_err(|error| {
            CoreError::InvalidInput(format!("Invalid ElevenLabs base URL: {error}"))
        })?;
    endpoint
        .path_segments_mut()
        .map_err(|_| {
            CoreError::InvalidInput("ElevenLabs base URL cannot be used as an API root.".into())
        })?
        .pop_if_empty()
        .push("text-to-speech")
        .push(voice);
    endpoint
        .query_pairs_mut()
        .append_pair("output_format", "mp3_44100_128");
    let response = client
        .post(endpoint)
        .header("xi-api-key", config.api_key.trim())
        .header(ACCEPT, "audio/mpeg")
        .json(&json!({
            "text": text,
            "model_id": model,
            "voice_settings": { "speed": speed },
        }))
        .send()
        .await
        .map_err(|error| {
            CoreError::TransientLlm(format!("ElevenLabs speech request failed: {error}"))
        })?;
    binary_response(response, "ElevenLabs", "audio/mpeg").await
}

async fn synthesize_minimax(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
    text: &str,
    model: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let endpoint = format!("{}/t2a_v2", base_url(config).trim_end_matches('/'));
    let response = client
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&json!({
            "model": model,
            "text": text,
            "stream": false,
            "voice_setting": { "voice_id": voice, "speed": speed },
            "audio_setting": {
                "format": "mp3",
                "sample_rate": 32000,
                "bitrate": 128000,
                "channel": 1
            }
        }))
        .send()
        .await
        .map_err(|error| {
            CoreError::TransientLlm(format!("MiniMax speech request failed: {error}"))
        })?;
    let status = response.status();
    let bytes =
        read_bounded_response(response, "MiniMax Speech", MAX_MINIMAX_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(provider_error("MiniMax Speech", status, &bytes));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CoreError::Llm(format!("MiniMax returned invalid JSON: {error}")))?;
    let audio_hex = value
        .pointer("/data/audio")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CoreError::Llm("MiniMax speech response did not include data.audio.".into())
        })?;
    Ok(GeneratedSpeech {
        bytes: decode_hex(audio_hex)?,
        media_type: "audio/mpeg".to_string(),
    })
}

async fn binary_response(
    response: reqwest::Response,
    provider: &str,
    fallback_media_type: &str,
) -> Result<GeneratedSpeech, CoreError> {
    let status = response.status();
    let media_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .filter(|value| value.starts_with("audio/"))
        .unwrap_or(fallback_media_type)
        .to_string();
    let bytes = read_bounded_response(response, provider, MAX_GENERATED_AUDIO_BYTES).await?;
    if !status.is_success() {
        return Err(provider_error(provider, status, &bytes));
    }
    if bytes.is_empty() {
        return Err(CoreError::Llm(format!("{provider} returned empty audio.")));
    }
    Ok(GeneratedSpeech { bytes, media_type })
}

async fn read_bounded_response(
    mut response: reqwest::Response,
    provider: &str,
    limit: usize,
) -> Result<Vec<u8>, CoreError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(CoreError::Llm(format!(
            "{provider} response exceeds the {limit}-byte safety limit."
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CoreError::Llm(format!("Failed to read {provider} response: {error}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(CoreError::Llm(format!(
                "{provider} response exceeds the {limit}-byte safety limit."
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn provider_error(provider: &str, status: reqwest::StatusCode, bytes: &[u8]) -> CoreError {
    let message = serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).chars().take(300).collect());
    CoreError::Llm(format!("{provider} returned HTTP {status}: {message}"))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, CoreError> {
    decode_hex_with_limit(value, MAX_GENERATED_AUDIO_BYTES)
}

fn decode_hex_with_limit(value: &str, limit: usize) -> Result<Vec<u8>, CoreError> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return Err(CoreError::Llm(
            "MiniMax returned malformed hex audio.".into(),
        ));
    }
    if value.len() / 2 > limit {
        return Err(CoreError::Llm(format!(
            "MiniMax audio exceeds the {limit}-byte safety limit."
        )));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| CoreError::Llm("MiniMax returned malformed hex audio.".into()))
        })
        .collect()
}

fn base_url(config: &TextToSpeechConfig) -> String {
    config
        .base_url
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| match config.api_style.as_str() {
            "elevenlabs_speech" => "https://api.elevenlabs.io/v1".to_string(),
            "minimax_speech" => "https://api.minimax.io/v1".to_string(),
            _ => "https://api.openai.com/v1".to_string(),
        })
}

fn normalize_format(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "wav" => "wav",
        "opus" => "opus",
        "aac" => "aac",
        "flac" => "flac",
        _ => "mp3",
    }
}

fn media_type_for_format(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "opus" => "audio/opus",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        _ => "audio/mpeg",
    }
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type.to_ascii_lowercase().as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/opus" | "audio/ogg" => "opus",
        "audio/aac" => "aac",
        "audio/flac" => "flac",
        _ => "mp3",
    }
}

fn safe_filename(filename: Option<&str>, extension: &str) -> String {
    let raw = filename
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generated-speech");
    let mut safe: String = raw
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => character,
        })
        .collect();
    if let Some((stem, suffix)) = safe.rsplit_once('.') {
        if !stem.is_empty()
            && (1..=8).contains(&suffix.len())
            && suffix
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            safe.truncate(stem.len());
        }
    }
    let safe = safe.trim().chars().take(96).collect::<String>();
    let stem = if safe.is_empty() {
        "generated-speech"
    } else {
        &safe
    };
    format!("{stem}.{extension}")
}

fn file_stem(filename: &str) -> &str {
    Path::new(filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("generated-speech")
}

fn error_result(call_id: &str, message: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: message.into(),
        is_error: true,
        artifacts: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_minimax_hex_audio() {
        assert_eq!(decode_hex("494433").unwrap(), b"ID3");
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex_with_limit("494433", 2).is_err());
    }

    #[test]
    fn filenames_cannot_escape_preview_directory() {
        let filename = safe_filename(Some("../../voice:demo"), "mp3");
        assert_eq!(filename, "..-..-voice-demo.mp3");
        assert_eq!(Path::new(&filename).components().count(), 1);
    }
}
