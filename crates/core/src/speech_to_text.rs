//! Low-latency microphone transcription backends.

use std::io::SeekFrom;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use regex::Regex;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .expect("static async speech-to-text HTTP client should build")
    })
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

/// Transcribe a managed WAV as fixed-size provider units. Only one bounded WAV
/// segment is held at a time, so multipart and base64/JSON providers cannot
/// turn a 60-minute recording into a whole-file native allocation.
pub async fn transcribe_cloud_wav_path(
    audio_path: &Path,
    config: &SpeechToTextConfig,
) -> Result<String, CoreError> {
    if !config.is_configured() {
        return Err(CoreError::InvalidInput(
            "Cloud speech-to-text is not fully configured".into(),
        ));
    }
    const MAX_SEGMENT_DATA_BYTES: usize = 8 * 1024 * 1024;
    let mut file = tokio::fs::File::open(audio_path).await?;
    let mut source_header = [0_u8; 44];
    file.read_exact(&mut source_header).await?;
    let wav = managed_pcm16_wav_spec(&source_header)?;
    let expected_file_bytes = 44_u64.saturating_add(u64::from(wav.data_bytes));
    let actual_file_bytes = file.metadata().await?.len();
    if actual_file_bytes != expected_file_bytes {
        return Err(CoreError::InvalidInput(format!(
            "Managed speech WAV length mismatch: expected {expected_file_bytes}, found {actual_file_bytes}"
        )));
    }

    let block_align = usize::from(wav.channels) * 2;
    let max_segment_bytes = MAX_SEGMENT_DATA_BYTES / block_align * block_align;
    let total_data_bytes = wav.data_bytes as usize;
    let overlap_bytes = (wav.sample_rate as usize)
        .saturating_mul(block_align)
        .min(max_segment_bytes / 4);
    let mut segment_offset = 0_usize;
    let mut transcript = String::new();
    while segment_offset < total_data_bytes {
        let segment_data_bytes = (total_data_bytes - segment_offset).min(max_segment_bytes);
        let segment_header = pcm16_wav_header(wav.sample_rate, wav.channels, segment_data_bytes)?;
        let mut segment = vec![0_u8; 44 + segment_data_bytes];
        segment[..44].copy_from_slice(&segment_header);
        file.seek(SeekFrom::Start(44 + segment_offset as u64))
            .await?;
        file.read_exact(&mut segment[44..]).await?;
        let segment_transcript = match config.api_style.as_str() {
            "openai_transcription" => transcribe_openai_compatible_wav(segment, config).await?,
            "dashscope_asr" => transcribe_dashscope_wav(segment, config).await?,
            _ => {
                return Err(CoreError::InvalidInput(format!(
                    "Unsupported cloud speech-to-text API style: {}",
                    config.api_style
                )))
            }
        };
        merge_overlapping_transcript(&mut transcript, segment_transcript.trim());
        if segment_offset + segment_data_bytes >= total_data_bytes {
            break;
        }
        segment_offset =
            segment_offset.saturating_add(segment_data_bytes.saturating_sub(overlap_bytes));
    }
    Ok(transcript)
}

fn merge_overlapping_transcript(transcript: &mut String, next: &str) {
    if next.is_empty() {
        return;
    }
    if transcript.is_empty() {
        transcript.push_str(next);
        return;
    }
    let existing_chars = transcript.chars().collect::<Vec<_>>();
    let next_chars = next.chars().collect::<Vec<_>>();
    let max_overlap = existing_chars.len().min(next_chars.len()).min(256);
    let overlap = (1..=max_overlap).rev().find(|length| {
        let existing_overlap = &existing_chars[existing_chars.len() - length..];
        let next_overlap = &next_chars[..*length];
        let minimum_length = if existing_overlap
            .iter()
            .any(|character| !character.is_ascii())
        {
            2
        } else {
            4
        };
        *length >= minimum_length
            && existing_overlap
                .iter()
                .collect::<String>()
                .eq_ignore_ascii_case(&next_overlap.iter().collect::<String>())
    });
    let skip_chars = overlap.unwrap_or_default();
    let remainder = next_chars[skip_chars..].iter().collect::<String>();
    if !remainder.is_empty() {
        let needs_ascii_separator = transcript
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric())
            && remainder
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric());
        if needs_ascii_separator {
            transcript.push(' ');
        }
        transcript.push_str(&remainder);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ManagedPcm16WavSpec {
    channels: u16,
    sample_rate: u32,
    data_bytes: u32,
}

fn managed_pcm16_wav_spec(header: &[u8; 44]) -> Result<ManagedPcm16WavSpec, CoreError> {
    if &header[0..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || &header[12..16] != b"fmt "
        || &header[36..40] != b"data"
    {
        return Err(CoreError::InvalidInput(
            "Managed speech input must be a canonical PCM WAV".into(),
        ));
    }
    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes(header[24..28].try_into().expect("four bytes"));
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);
    let data_bytes = u32::from_le_bytes(header[40..44].try_into().expect("four bytes"));
    if audio_format != 1 || channels == 0 || bits_per_sample != 16 || data_bytes == 0 {
        return Err(CoreError::InvalidInput(
            "Managed speech input must contain aligned 16-bit PCM audio".into(),
        ));
    }
    let block_align = u32::from(channels) * 2;
    if data_bytes % block_align != 0 {
        return Err(CoreError::InvalidInput(
            "Managed speech WAV contains an incomplete PCM frame".into(),
        ));
    }
    Ok(ManagedPcm16WavSpec {
        channels,
        sample_rate,
        data_bytes,
    })
}

fn pcm16_wav_header(
    sample_rate: u32,
    channels: u16,
    data_bytes: usize,
) -> Result<[u8; 44], CoreError> {
    let data_bytes = u32::try_from(data_bytes)
        .map_err(|_| CoreError::InvalidInput("Speech WAV segment is too large".into()))?;
    let block_align = channels
        .checked_mul(2)
        .ok_or_else(|| CoreError::InvalidInput("Speech WAV channel count overflow".into()))?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| CoreError::InvalidInput("Speech WAV byte rate overflow".into()))?;
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36_u32 + data_bytes).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    Ok(header)
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
    fn managed_wav_segments_preserve_pcm_contract_and_bounded_sizes() {
        let header = pcm16_wav_header(16_000, 1, 8 * 1024 * 1024).unwrap();
        let spec = managed_pcm16_wav_spec(&header).unwrap();

        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.data_bytes, 8 * 1024 * 1024);
        assert_eq!(
            u32::from_le_bytes(header[4..8].try_into().unwrap()),
            36 + spec.data_bytes
        );
        assert_eq!(u16::from_le_bytes(header[32..34].try_into().unwrap()), 2);

        let mut malformed = header;
        malformed[40..44].copy_from_slice(&3_u32.to_le_bytes());
        assert!(managed_pcm16_wav_spec(&malformed).is_err());
    }

    #[test]
    fn provider_segment_overlap_is_reconciled_without_duplicate_text() {
        let mut english = "hello world".to_string();
        merge_overlapping_transcript(&mut english, "world again");
        assert_eq!(english, "hello world again");

        let mut chinese = "你好世界".to_string();
        merge_overlapping_transcript(&mut chinese, "世界继续");
        assert_eq!(chinese, "你好世界继续");

        let mut incidental = "a".to_string();
        merge_overlapping_transcript(&mut incidental, "and then");
        assert_eq!(incidental, "a and then");
    }

    #[test]
    fn sherpa_output_removes_sense_voice_tags() {
        let wav = Path::new("voice.wav");
        let output = "voice.wav\n<|zh|><|NEUTRAL|><|Speech|>你好世界\nElapsed seconds: 0.2";
        assert_eq!(parse_sherpa_stdout(output, wav), "你好世界");
    }
}
