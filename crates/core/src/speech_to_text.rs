//! Low-latency microphone transcription backends.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use regex::Regex;
use serde::Deserialize;

use crate::app_settings::SpeechToTextConfig;
use crate::error::CoreError;

#[derive(Deserialize)]
struct CloudTranscript {
    text: String,
}

#[derive(Deserialize)]
struct DashScopeTranscript {
    choices: Vec<DashScopeTranscriptChoice>,
}

#[derive(Deserialize)]
struct DashScopeTranscriptChoice {
    message: DashScopeTranscriptMessage,
}

#[derive(Deserialize)]
struct DashScopeTranscriptMessage {
    content: String,
}

fn transcription_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/audio/transcriptions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/audio/transcriptions")
    }
}

fn async_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

fn blocking_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .expect("static speech-to-text HTTP client should build")
    })
}

pub async fn transcribe_cloud_wav(
    audio_data: Vec<u8>,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    if !config.is_configured() {
        return Err(CoreError::InvalidInput(
            "Cloud speech-to-text is not fully configured".into(),
        ));
    }
    match config.api_style.as_str() {
        "openai_transcription" => transcribe_openai_compatible_wav(audio_data, config).await,
        "dashscope_asr" => transcribe_dashscope_wav(audio_data, config).await,
        _ => Err(CoreError::InvalidInput(format!(
            "Unsupported cloud speech-to-text API style: {}",
            config.api_style
        ))),
    }
}

async fn transcribe_openai_compatible_wav(
    audio_data: Vec<u8>,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    let endpoint = transcription_endpoint(config.base_url.as_deref().unwrap_or_default());
    let file = reqwest::multipart::Part::bytes(audio_data)
        .file_name("voice.wav")
        .mime_str("audio/wav")
        .map_err(|error| CoreError::InvalidInput(format!("Invalid audio MIME type: {error}")))?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", file)
        .text("model", config.model.trim().to_string())
        .text("response_format", "json".to_string());
    if let Some(language) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        form = form.text("language", language.to_string());
    }

    let response = async_client()
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .multipart(form)
        .send()
        .await
        .map_err(|error| CoreError::Llm(format!("Speech transcription request failed: {error}")))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        CoreError::Llm(format!("Speech transcription response failed: {error}"))
    })?;
    if !status.is_success() {
        return Err(CoreError::Llm(format!(
            "Speech transcription provider returned {status}: {}",
            body.chars().take(500).collect::<String>()
        )));
    }
    let transcript: CloudTranscript = serde_json::from_str(&body)
        .map_err(|error| CoreError::Parse(format!("Invalid transcription response: {error}")))?;
    Ok(transcript.text.trim().to_string())
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

async fn transcribe_dashscope_wav(
    audio_data: Vec<u8>,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    let endpoint = chat_completions_endpoint(config.base_url.as_deref().unwrap_or_default());
    let data_url = format!("data:audio/wav;base64,{}", BASE64.encode(audio_data));
    let mut asr_options = serde_json::json!({ "enable_itn": true });
    if let Some(language) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        asr_options["language"] = serde_json::Value::String(language.to_string());
    }
    let body = serde_json::json!({
        "model": config.model.trim(),
        "messages": [{
            "role": "user",
            "content": [{
                "type": "input_audio",
                "input_audio": { "data": data_url }
            }]
        }],
        "stream": false,
        "asr_options": asr_options
    });
    let response = async_client()
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|error| CoreError::Llm(format!("Qwen ASR request failed: {error}")))?;
    let status = response.status();
    let response_body = response
        .text()
        .await
        .map_err(|error| CoreError::Llm(format!("Qwen ASR response failed: {error}")))?;
    if !status.is_success() {
        return Err(CoreError::Llm(format!(
            "Qwen ASR returned {status}: {}",
            response_body.chars().take(500).collect::<String>()
        )));
    }
    let transcript: DashScopeTranscript = serde_json::from_str(&response_body)
        .map_err(|error| CoreError::Parse(format!("Invalid Qwen ASR response: {error}")))?;
    transcript
        .choices
        .first()
        .map(|choice| choice.message.content.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| CoreError::Parse("Qwen ASR returned no transcript text".into()))
}

/// Blocking cloud transcription for source-ingestion workers. The shared
/// client reuses connections across chunks and jobs.
pub fn transcribe_cloud_wav_blocking(
    audio_data: Vec<u8>,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    if !config.is_configured() {
        return Err(CoreError::InvalidInput(
            "Cloud speech-to-text is not fully configured".into(),
        ));
    }
    let mut last_error = None;
    for attempt in 0..3 {
        let result = match config.api_style.as_str() {
            "openai_transcription" => {
                transcribe_openai_compatible_wav_blocking(&audio_data, config)
            }
            "dashscope_asr" => transcribe_dashscope_wav_blocking(&audio_data, config),
            _ => Err(CoreError::InvalidInput(format!(
                "Unsupported cloud speech-to-text API style: {}",
                config.api_style
            ))),
        };
        match result {
            Ok(text) => return Ok(text),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(250 * (1 << attempt)));
                }
            }
        }
    }
    Err(last_error.expect("three transcription attempts always record an error"))
}

fn transcribe_openai_compatible_wav_blocking(
    audio_data: &[u8],
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    let endpoint = transcription_endpoint(config.base_url.as_deref().unwrap_or_default());
    let file = reqwest::blocking::multipart::Part::bytes(audio_data.to_vec())
        .file_name("media-chunk.wav")
        .mime_str("audio/wav")
        .map_err(|error| CoreError::InvalidInput(format!("Invalid audio MIME type: {error}")))?;
    let mut form = reqwest::blocking::multipart::Form::new()
        .part("file", file)
        .text("model", config.model.trim().to_string())
        .text("response_format", "json".to_string());
    if let Some(language) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        form = form.text("language", language.to_string());
    }
    let response = blocking_client()
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .multipart(form)
        .send()
        .map_err(|error| CoreError::Llm(format!("Speech transcription request failed: {error}")))?;
    let status = response.status();
    let body = response.text().map_err(|error| {
        CoreError::Llm(format!("Speech transcription response failed: {error}"))
    })?;
    if !status.is_success() {
        return Err(CoreError::Llm(format!(
            "Speech transcription provider returned {status}: {}",
            body.chars().take(500).collect::<String>()
        )));
    }
    let transcript: CloudTranscript = serde_json::from_str(&body)
        .map_err(|error| CoreError::Parse(format!("Invalid transcription response: {error}")))?;
    Ok(transcript.text.trim().to_string())
}

fn transcribe_dashscope_wav_blocking(
    audio_data: &[u8],
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    let endpoint = chat_completions_endpoint(config.base_url.as_deref().unwrap_or_default());
    let data_url = format!("data:audio/wav;base64,{}", BASE64.encode(audio_data));
    let mut asr_options = serde_json::json!({ "enable_itn": true });
    if let Some(language) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        asr_options["language"] = serde_json::Value::String(language.to_string());
    }
    let body = serde_json::json!({
        "model": config.model.trim(),
        "messages": [{
            "role": "user",
            "content": [{ "type": "input_audio", "input_audio": { "data": data_url } }]
        }],
        "stream": false,
        "asr_options": asr_options
    });
    let response = blocking_client()
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&body)
        .send()
        .map_err(|error| CoreError::Llm(format!("Qwen ASR request failed: {error}")))?;
    let status = response.status();
    let response_body = response
        .text()
        .map_err(|error| CoreError::Llm(format!("Qwen ASR response failed: {error}")))?;
    if !status.is_success() {
        return Err(CoreError::Llm(format!(
            "Qwen ASR returned {status}: {}",
            response_body.chars().take(500).collect::<String>()
        )));
    }
    let transcript: DashScopeTranscript = serde_json::from_str(&response_body)
        .map_err(|error| CoreError::Parse(format!("Invalid Qwen ASR response: {error}")))?;
    transcript
        .choices
        .first()
        .map(|choice| choice.message.content.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| CoreError::Parse("Qwen ASR returned no transcript text".into()))
}

fn required_path(value: &Option<String>, label: &str) -> Result<String, CoreError> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| CoreError::InvalidInput(format!("Missing sherpa-onnx {label}")))
}

fn clean_sense_voice_tags(text: &str) -> String {
    Regex::new(r"<\|[^|>]+\|>")
        .expect("static regex")
        .replace_all(text, "")
        .trim()
        .to_string()
}

fn parse_sherpa_stdout(stdout: &str, wav_path: &Path) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) {
        if let Some(text) = value.get("text").and_then(|value| value.as_str()) {
            return clean_sense_voice_tags(text);
        }
    }

    let wav_name = wav_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    stdout
        .lines()
        .map(str::trim)
        .rfind(|line| {
            !line.is_empty()
                && !line.contains("parse-options.cc")
                && !line.contains("Elapsed seconds")
                && !line.contains("Real time factor")
                && !line.ends_with(wav_name)
        })
        .map(clean_sense_voice_tags)
        .unwrap_or_default()
}

pub fn transcribe_sherpa_wav(
    wav_path: &Path,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    if !config.is_configured() || config.api_style != "sherpa_onnx" {
        return Err(CoreError::InvalidInput(
            "sherpa-onnx speech-to-text is not fully configured".into(),
        ));
    }
    let executable = required_path(&config.executable_path, "executable")?;
    let tokens = required_path(&config.tokens_path, "tokens path")?;
    let mut command = Command::new(executable);
    command
        .arg(format!("--num-threads={}", config.num_threads.clamp(1, 32)))
        .arg(format!("--tokens={tokens}"));

    if config.sherpa_model_family == "zipformer" {
        command
            .arg(format!(
                "--encoder={}",
                required_path(&config.encoder_path, "encoder")?
            ))
            .arg(format!(
                "--decoder={}",
                required_path(&config.decoder_path, "decoder")?
            ))
            .arg(format!(
                "--joiner={}",
                required_path(&config.joiner_path, "joiner")?
            ));
    } else {
        command
            .arg(format!(
                "--sense-voice-model={}",
                required_path(&config.model_path, "model")?
            ))
            .arg(format!(
                "--sense-voice-language={}",
                config
                    .language
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .unwrap_or("auto")
            ))
            .arg("--sense-voice-use-itn=1");
    }
    command.arg(wav_path);
    crate::background_process::configure_std_background(&mut command);

    let output = command.output().map_err(CoreError::Io)?;
    if !output.status.success() {
        return Err(CoreError::Video(format!(
            "sherpa-onnx transcription failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = parse_sherpa_stdout(&String::from_utf8_lossy(&output.stdout), wav_path);
    if text.is_empty() {
        return Err(CoreError::Parse(
            "sherpa-onnx returned no transcript text".into(),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_base_or_full_path() {
        assert_eq!(
            transcription_endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            chat_completions_endpoint("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
        assert_eq!(
            transcription_endpoint("https://example.test/audio/transcriptions/"),
            "https://example.test/audio/transcriptions"
        );
    }

    #[test]
    fn sherpa_output_removes_sense_voice_tags() {
        let wav = Path::new("voice.wav");
        let output = "voice.wav\n<|zh|><|NEUTRAL|><|Speech|>你好世界\nElapsed seconds: 0.2";
        assert_eq!(parse_sherpa_stdout(output, wav), "你好世界");
    }
}
