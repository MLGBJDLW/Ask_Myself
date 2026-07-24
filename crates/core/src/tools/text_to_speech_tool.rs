//! Text-to-speech generation through configured cloud providers.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
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
            "azure_speech" => synthesize_azure(&client, &config, text, &voice, speed).await?,
            "dashscope_speech" => {
                synthesize_dashscope(&client, &config, text, &model, &voice, speed).await?
            }
            "sherpa_onnx" => synthesize_sherpa_onnx(&config, text, &model, &voice, speed).await?,
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

async fn synthesize_azure(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
    text: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let format = normalize_format(&config.output_format);
    let (output_format, fallback_media_type) = match format {
        "mp3" => ("audio-24khz-48kbitrate-mono-mp3", "audio/mpeg"),
        _ => ("riff-24khz-16bit-mono-pcm", "audio/wav"),
    };
    let locale = voice.split('-').take(2).collect::<Vec<_>>().join("-");
    let rate_percent = ((speed - 1.0) * 100.0).round() as i32;
    let rate = if rate_percent >= 0 {
        format!("+{rate_percent}%")
    } else {
        format!("{rate_percent}%")
    };
    let ssml = format!(
        "<speak version=\"1.0\" xml:lang=\"{}\"><voice name=\"{}\"><prosody rate=\"{}\">{}</prosody></voice></speak>",
        xml_escape(&locale),
        xml_escape(voice),
        rate,
        xml_escape(text),
    );
    let response = client
        .post(base_url(config))
        .header("Ocp-Apim-Subscription-Key", config.api_key.trim())
        .header(CONTENT_TYPE, "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", output_format)
        .header("User-Agent", crate::USER_AGENT)
        .body(ssml)
        .send()
        .await
        .map_err(|error| {
            CoreError::TransientLlm(format!("Azure Speech request failed: {error}"))
        })?;
    binary_response(response, "Azure Speech", fallback_media_type).await
}

async fn synthesize_dashscope(
    client: &reqwest::Client,
    config: &TextToSpeechConfig,
    text: &str,
    model: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let format = match normalize_format(&config.output_format) {
        "wav" => "wav",
        "opus" => "opus",
        _ => "mp3",
    };
    let endpoint = format!(
        "{}/SpeechSynthesizer",
        base_url(config).trim_end_matches('/')
    );
    let response = client
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&json!({
            "model": model,
            "input": {
                "text": text,
                "voice": voice,
                "format": format,
                "sample_rate": 24000,
                "rate": speed
            }
        }))
        .send()
        .await
        .map_err(|error| {
            CoreError::TransientLlm(format!("DashScope speech request failed: {error}"))
        })?;
    let status = response.status();
    let bytes =
        read_bounded_response(response, "DashScope Speech", MAX_MINIMAX_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(provider_error("DashScope Speech", status, &bytes));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CoreError::Llm(format!("DashScope returned invalid JSON: {error}")))?;
    if let Some(encoded) = value
        .pointer("/output/audio/data")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Ok(GeneratedSpeech {
            bytes: decode_base64_audio(encoded, "DashScope")?,
            media_type: media_type_for_format(format).to_string(),
        });
    }
    let audio_url = value
        .pointer("/output/audio/url")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CoreError::Llm("DashScope speech response did not include output.audio.url.".into())
        })?;
    let parsed = Url::parse(audio_url).map_err(|error| {
        CoreError::Llm(format!("DashScope returned an invalid audio URL: {error}"))
    })?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(parsed.scheme(), "http" | "https")
        || !(host == "aliyuncs.com" || host.ends_with(".aliyuncs.com"))
    {
        return Err(CoreError::Llm(
            "DashScope returned an audio URL outside the aliyuncs.com service boundary.".into(),
        ));
    }
    let response = client.get(parsed).send().await.map_err(|error| {
        CoreError::TransientLlm(format!("DashScope audio download failed: {error}"))
    })?;
    binary_response(
        response,
        "DashScope Speech audio",
        media_type_for_format(format),
    )
    .await
}

async fn synthesize_sherpa_onnx(
    config: &TextToSpeechConfig,
    text: &str,
    model_family: &str,
    voice: &str,
    speed: f32,
) -> Result<GeneratedSpeech, CoreError> {
    let family = match model_family.trim().to_ascii_lowercase().as_str() {
        "vits" => "vits",
        "kokoro" => "kokoro",
        "kitten" => "kitten",
        value => {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported sherpa-onnx TTS family '{value}'. Use vits, kokoro, or kitten."
            )))
        }
    };
    let executable = required_local_value(config.executable_path.as_deref(), "executable path")?;
    let model_path = required_local_file(config.model_path.as_deref(), "model file")?;
    let tokens_path = required_local_file(config.tokens_path.as_deref(), "tokens file")?;
    let speaker_id = voice.trim().parse::<u32>().map_err(|_| {
        CoreError::InvalidInput(
            "sherpa-onnx voice must be a numeric speaker ID, for example 0 or 18.".into(),
        )
    })?;
    let output_path = std::env::temp_dir()
        .join("nexa")
        .join("sherpa-onnx")
        .join(format!("{}.wav", Uuid::new_v4()));
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut command = Command::new(executable);
    command
        .kill_on_drop(true)
        .arg(format!("--{family}-model={}", model_path.display()))
        .arg(format!("--{family}-tokens={}", tokens_path.display()))
        .arg(format!("--num-threads={}", config.num_threads.clamp(1, 32)))
        .arg(format!("--sid={speaker_id}"))
        .arg(format!("--speed={speed}"))
        .arg(format!("--output-filename={}", output_path.display()));

    if matches!(family, "kokoro" | "kitten") {
        let voices_path = required_local_file(config.voices_path.as_deref(), "voices file")?;
        command.arg(format!("--{family}-voices={}", voices_path.display()));
    }
    if let Some(path) = optional_existing_path(config.data_dir.as_deref(), "data directory")? {
        command.arg(format!("--{family}-data-dir={}", path.display()));
    }
    if family != "kitten" {
        if let Some(lexicon) = config
            .lexicon_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            command.arg(format!("--{family}-lexicon={lexicon}"));
        }
    }
    command.arg(text);

    let output = tokio::time::timeout(Duration::from_secs(180), command.output())
        .await
        .map_err(|_| {
            CoreError::TransientLlm("sherpa-onnx synthesis timed out after 180 seconds.".into())
        })?
        .map_err(|error| {
            CoreError::InvalidInput(format!(
                "Failed to launch sherpa-onnx executable '{executable}': {error}"
            ))
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&output_path);
        let stderr: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(800)
            .collect();
        return Err(CoreError::Llm(format!(
            "sherpa-onnx exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }
    let bytes = std::fs::read(&output_path).map_err(|error| {
        CoreError::Llm(format!(
            "sherpa-onnx did not produce readable audio: {error}"
        ))
    })?;
    let _ = std::fs::remove_file(&output_path);
    if bytes.is_empty() {
        return Err(CoreError::Llm("sherpa-onnx returned empty audio.".into()));
    }
    if bytes.len() > MAX_GENERATED_AUDIO_BYTES {
        return Err(CoreError::Llm(format!(
            "sherpa-onnx audio exceeds the {MAX_GENERATED_AUDIO_BYTES}-byte safety limit."
        )));
    }
    Ok(GeneratedSpeech {
        bytes,
        media_type: "audio/wav".to_string(),
    })
}

fn required_local_value<'a>(value: Option<&'a str>, label: &str) -> Result<&'a str, CoreError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::InvalidInput(format!("sherpa-onnx {label} is required.")))
}

fn required_local_file(value: Option<&str>, label: &str) -> Result<std::path::PathBuf, CoreError> {
    let path = std::path::PathBuf::from(required_local_value(value, label)?);
    if !path.is_file() {
        return Err(CoreError::InvalidInput(format!(
            "sherpa-onnx {label} does not exist or is not a file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn optional_existing_path(
    value: Option<&str>,
    label: &str,
) -> Result<Option<std::path::PathBuf>, CoreError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(value);
    if !path.exists() {
        return Err(CoreError::InvalidInput(format!(
            "sherpa-onnx {label} does not exist: {}",
            path.display()
        )));
    }
    Ok(Some(path))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn decode_base64_audio(value: &str, provider: &str) -> Result<Vec<u8>, CoreError> {
    if value.len() > MAX_MINIMAX_RESPONSE_BYTES {
        return Err(CoreError::Llm(format!(
            "{provider} audio exceeds the encoded response safety limit."
        )));
    }
    let bytes = BASE64_STANDARD.decode(value.trim()).map_err(|error| {
        CoreError::Llm(format!(
            "{provider} returned malformed base64 audio: {error}"
        ))
    })?;
    if bytes.len() > MAX_GENERATED_AUDIO_BYTES {
        return Err(CoreError::Llm(format!(
            "{provider} audio exceeds the {MAX_GENERATED_AUDIO_BYTES}-byte safety limit."
        )));
    }
    Ok(bytes)
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
            "azure_speech" => {
                "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1".to_string()
            }
            "dashscope_speech" => {
                "https://dashscope.aliyuncs.com/api/v1/services/audio/tts".to_string()
            }
            _ => "https://api.openai.com/v1".to_string(),
        })
}

fn normalize_format(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "wav" => "wav",
        "opus" => "opus",
        "aac" => "aac",
        "flac" => "flac",
        "ogg" => "ogg",
        _ => "mp3",
    }
}

fn media_type_for_format(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "opus" => "audio/opus",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        _ => "audio/mpeg",
    }
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type.to_ascii_lowercase().as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/opus" => "opus",
        "audio/ogg" => "ogg",
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

    #[test]
    fn escapes_azure_ssml_text() {
        assert_eq!(xml_escape("A&B <voice>"), "A&amp;B &lt;voice&gt;");
    }

    #[test]
    fn decodes_bounded_base64_audio() {
        assert_eq!(decode_base64_audio("SUQz", "test").unwrap(), b"ID3");
        assert!(decode_base64_audio("not base64", "test").is_err());
    }
}
