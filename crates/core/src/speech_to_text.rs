//! Low-latency microphone transcription backends.

use std::path::Path;
use std::process::Command;

use regex::Regex;
use serde::Deserialize;

use crate::app_settings::SpeechToTextConfig;
use crate::error::CoreError;

#[derive(Deserialize)]
struct CloudTranscript {
    text: String,
}

fn transcription_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/audio/transcriptions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/audio/transcriptions")
    }
}

pub async fn transcribe_cloud_wav(
    audio_data: Vec<u8>,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    if !config.is_configured() || config.api_style != "openai_transcription" {
        return Err(CoreError::InvalidInput(
            "Cloud speech-to-text is not fully configured".into(),
        ));
    }
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

    let response = reqwest::Client::new()
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
        .filter(|line| {
            !line.is_empty()
                && !line.contains("parse-options.cc")
                && !line.contains("Elapsed seconds")
                && !line.contains("Real time factor")
                && !line.ends_with(wav_name)
        })
        .last()
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

    let output = command.output().map_err(|error| CoreError::Io(error))?;
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
