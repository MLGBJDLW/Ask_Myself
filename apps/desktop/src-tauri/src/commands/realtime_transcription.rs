use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use url::Url;
use uuid::Uuid;

use super::realtime_transcript::RealtimeTranscript;
use crate::app_events::emit_app_event;

const REALTIME_TRANSCRIPTION_EVENT: &str = "speech-to-text:realtime";
const REALTIME_COMMAND_BUFFER: usize = 64;
const MAX_AUDIO_CHUNK_BYTES: usize = 256 * 1024;
const SESSION_ID_HEADER: &str = "x-nexa-session-id";
const FINAL_TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
const REPLAY_FINAL_TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(180);
const REPLAY_PCM_CHUNK_BYTES: usize = 64 * 1024;

type RealtimeSessions = Arc<Mutex<HashMap<String, mpsc::Sender<RealtimeCommand>>>>;

#[derive(Clone, Default)]
pub struct RealtimeTranscriptionState {
    sessions: RealtimeSessions,
}

enum RealtimeCommand {
    Append(Vec<u8>),
    Finish(oneshot::Sender<Result<String, String>>),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeDialect {
    OpenAi,
    DashScope,
}

impl RealtimeDialect {
    fn from_config(config: &nexa_core::app_settings::SpeechToTextConfig) -> Result<Self, String> {
        if !config.is_configured() {
            return Err("Realtime transcription is not fully configured".to_string());
        }
        match config.api_style.as_str() {
            "openai_realtime_transcription" if config.model.trim() == "gpt-live-transcribe" => {
                Ok(Self::OpenAi)
            }
            "dashscope_realtime_asr" if config.model.trim() == "qwen3-asr-flash-realtime" => {
                Ok(Self::DashScope)
            }
            "openai_realtime_transcription" | "dashscope_realtime_asr" => Err(format!(
                "Unsupported realtime transcription model for {}: {}",
                config.api_style,
                config.model.trim()
            )),
            value => Err(format!(
                "Unsupported realtime transcription API style: {value}"
            )),
        }
    }

    fn sample_rate(self) -> u32 {
        match self {
            Self::OpenAi => 24_000,
            Self::DashScope => 16_000,
        }
    }

    fn waits_for_session_finished(self) -> bool {
        self == Self::DashScope
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptUpdate {
    AppendDelta,
    ReplaceSnapshot,
}

impl TranscriptUpdate {
    fn wire_name(self) -> &'static str {
        match self {
            Self::AppendDelta => "appendDelta",
            Self::ReplaceSnapshot => "replaceSnapshot",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedRealtimeServerEvent {
    Interim {
        utterance_id: Option<String>,
        text: String,
        update: TranscriptUpdate,
    },
    Final {
        utterance_id: Option<String>,
        text: String,
    },
    SessionFinished,
    Error(String),
    Other,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RealtimeTranscriptionFrontendEvent<'a> {
    session_id: &'a str,
    sequence: u64,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    utterance_id: Option<&'a str>,
}

fn build_realtime_endpoint(base_url: &str, model: &str) -> Result<String, String> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|error| format!("Invalid Realtime base URL: {error}"))?;
    let websocket_scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        "wss" => "wss",
        "ws" => "ws",
        scheme => return Err(format!("Unsupported Realtime URL scheme: {scheme}")),
    };
    url.set_scheme(websocket_scheme)
        .map_err(|_| "Unable to set Realtime WebSocket scheme".to_string())?;

    let path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/realtime") {
        let realtime_path = if path.is_empty() {
            "/realtime".to_string()
        } else {
            format!("{path}/realtime")
        };
        url.set_path(&realtime_path);
    } else if url.path().ends_with('/') {
        url.set_path(&path);
    }
    url.set_query(None);
    url.query_pairs_mut().append_pair("model", model.trim());
    Ok(url.to_string())
}

fn language_hints(language: Option<&str>) -> Vec<String> {
    language
        .unwrap_or_default()
        .split(|character: char| {
            character == ',' || character == ';' || character == '/' || character.is_whitespace()
        })
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
        .map(ToOwned::to_owned)
        .collect()
}

fn build_session_update(dialect: RealtimeDialect, model: &str, language: Option<&str>) -> Value {
    match dialect {
        RealtimeDialect::OpenAi => {
            let mut transcription = serde_json::json!({
                "model": model.trim(),
                "delay": "low"
            });
            let languages = language_hints(language);
            if !languages.is_empty() {
                transcription["languages"] = serde_json::json!(languages);
            }
            serde_json::json!({
                "type": "session.update",
                "session": {
                    "type": "transcription",
                    "audio": {
                        "input": {
                            "format": {
                                "type": "audio/pcm",
                                "rate": dialect.sample_rate()
                            },
                            "transcription": transcription,
                            "turn_detection": null
                        }
                    }
                }
            })
        }
        RealtimeDialect::DashScope => {
            let mut transcription = serde_json::Map::new();
            if let Some(language) = language_hints(language).into_iter().next() {
                transcription.insert("language".to_string(), Value::String(language));
            }
            serde_json::json!({
                "event_id": format!("event_{}", Uuid::new_v4().simple()),
                "type": "session.update",
                "session": {
                    "input_audio_format": "pcm",
                    "sample_rate": dialect.sample_rate(),
                    "input_audio_transcription": transcription,
                    "turn_detection": { "type": "server_vad", "threshold": 0.0, "silence_duration_ms": 400 }
                }
            })
        }
    }
}

fn parse_server_event(dialect: RealtimeDialect, event: &Value) -> ParsedRealtimeServerEvent {
    let utterance_id = event
        .get("item_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "conversation.item.input_audio_transcription.delta"
            if dialect == RealtimeDialect::OpenAi =>
        {
            ParsedRealtimeServerEvent::Interim {
                utterance_id,
                text: event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                update: TranscriptUpdate::AppendDelta,
            }
        }
        "conversation.item.input_audio_transcription.text"
            if dialect == RealtimeDialect::DashScope =>
        {
            ParsedRealtimeServerEvent::Interim {
                utterance_id,
                text: format!(
                    "{}{}",
                    event
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    event
                        .get("stash")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ),
                update: TranscriptUpdate::ReplaceSnapshot,
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            ParsedRealtimeServerEvent::Final {
                utterance_id,
                text: event
                    .get("transcript")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        }
        "conversation.item.input_audio_transcription.failed" => ParsedRealtimeServerEvent::Error(
            event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Realtime transcription item failed")
                .to_string(),
        ),
        "session.finished" => ParsedRealtimeServerEvent::SessionFinished,
        "error" => ParsedRealtimeServerEvent::Error(
            event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| event.get("message").and_then(Value::as_str))
                .unwrap_or("Realtime transcription session failed")
                .to_string(),
        ),
        _ => ParsedRealtimeServerEvent::Other,
    }
}

fn emit_realtime_event(
    app_handle: &AppHandle,
    session_id: &str,
    sequence: &mut u64,
    kind: &str,
    text: Option<&str>,
    update: Option<TranscriptUpdate>,
    utterance_id: Option<&str>,
) {
    *sequence = sequence.saturating_add(1);
    emit_app_event(
        app_handle,
        REALTIME_TRANSCRIPTION_EVENT,
        &RealtimeTranscriptionFrontendEvent {
            session_id,
            sequence: *sequence,
            kind,
            text,
            update: update.map(TranscriptUpdate::wire_name),
            utterance_id,
        },
    );
}

fn finish_messages(dialect: RealtimeDialect) -> Vec<Value> {
    vec![serde_json::json!({
        "event_id": format!("event_{}", Uuid::new_v4().simple()),
        "type": if dialect == RealtimeDialect::DashScope { "session.finish" } else { "input_audio_buffer.commit" }
    })]
}

fn append_message(audio_data: &[u8]) -> Value {
    serde_json::json!({
        "event_id": format!("event_{}", Uuid::new_v4().simple()),
        "type": "input_audio_buffer.append",
        "audio": BASE64.encode(audio_data),
    })
}

fn resolve_pending_final(
    pending_final: &mut Option<oneshot::Sender<Result<String, String>>>,
    result: Result<String, String>,
) {
    if let Some(sender) = pending_final.take() {
        let _ = sender.send(result);
    }
}

fn replay_wav_data_bytes(header: &[u8; 44], expected_sample_rate: u32) -> Result<u32, String> {
    if &header[0..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || &header[12..16] != b"fmt "
        || &header[36..40] != b"data"
    {
        return Err("Managed Realtime replay requires a canonical PCM WAV".to_string());
    }
    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes(header[24..28].try_into().expect("four bytes"));
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);
    if audio_format != 1
        || channels != 1
        || sample_rate != expected_sample_rate
        || bits_per_sample != 16
    {
        return Err(format!(
            "Managed Realtime replay requires mono {expected_sample_rate} Hz little-endian PCM16"
        ));
    }
    Ok(u32::from_le_bytes(
        header[40..44].try_into().expect("four bytes"),
    ))
}

/// Replay a finalized native spool through a fresh Realtime transcription
/// session. The file path never crosses IPC and only one bounded PCM chunk is
/// base64-encoded at a time.
pub(super) async fn transcribe_realtime_spool(
    wav_path: &Path,
    config: &nexa_core::app_settings::SpeechToTextConfig,
) -> Result<String, String> {
    let dialect = RealtimeDialect::from_config(config)?;
    let endpoint = build_realtime_endpoint(
        config.base_url.as_deref().unwrap_or_default(),
        &config.model,
    )?;
    let mut request = endpoint
        .into_client_request()
        .map_err(|error| format!("Invalid Realtime WebSocket request: {error}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key.trim())
            .parse()
            .map_err(|error| format!("Invalid realtime authorization header: {error}"))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        nexa_core::USER_AGENT
            .parse()
            .map_err(|error| format!("Invalid User-Agent header: {error}"))?,
    );

    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| format!("Unable to reconnect to realtime transcription: {error}"))?;
    socket
        .send(Message::Text(
            build_session_update(dialect, &config.model, config.language.as_deref())
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| format!("Unable to configure realtime transcription replay: {error}"))?;

    let mut file = tokio::fs::File::open(wav_path)
        .await
        .map_err(|error| format!("Unable to open managed Realtime audio: {error}"))?;
    let mut header = [0_u8; 44];
    file.read_exact(&mut header)
        .await
        .map_err(|error| format!("Unable to read managed Realtime WAV header: {error}"))?;
    let mut remaining = replay_wav_data_bytes(&header, dialect.sample_rate())? as usize;
    file.seek(SeekFrom::Start(44))
        .await
        .map_err(|error| format!("Unable to seek managed Realtime audio: {error}"))?;
    let mut buffer = vec![0_u8; REPLAY_PCM_CHUNK_BYTES];
    let mut transcript = RealtimeTranscript::default();
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len());
        file.read_exact(&mut buffer[..chunk_len])
            .await
            .map_err(|error| format!("Unable to read managed Realtime audio: {error}"))?;
        let payload = append_message(&buffer[..chunk_len]);
        socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .map_err(|error| {
                format!("Unable to replay audio to realtime transcription: {error}")
            })?;
        remaining -= chunk_len;

        if let Ok(Some(incoming)) =
            tokio::time::timeout(Duration::from_millis(1), socket.next()).await
        {
            match incoming {
                Ok(Message::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<Value>(&text) {
                        match parse_server_event(dialect, &event) {
                            ParsedRealtimeServerEvent::Error(message) => return Err(message),
                            ParsedRealtimeServerEvent::Interim {
                                utterance_id,
                                text,
                                update,
                            } => transcript.update(
                                utterance_id.as_deref(),
                                &text,
                                update == TranscriptUpdate::AppendDelta,
                                false,
                            ),
                            ParsedRealtimeServerEvent::Final { utterance_id, text } => {
                                transcript.update(utterance_id.as_deref(), &text, false, true)
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Ping(payload)) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("Realtime replay heartbeat failed: {error}"))?,
                Ok(Message::Close(_)) => {
                    return Err("Realtime replay closed while uploading audio".to_string())
                }
                Err(error) => return Err(format!("Realtime transcription replay failed: {error}")),
                Ok(_) => {}
            }
        }
    }
    for message in finish_messages(dialect) {
        socket
            .send(Message::Text(message.to_string().into()))
            .await
            .map_err(|error| format!("Unable to finish realtime transcription replay: {error}"))?;
    }

    tokio::time::timeout(REPLAY_FINAL_TRANSCRIPT_TIMEOUT, async {
        while let Some(incoming) = socket.next().await {
            match incoming {
                Ok(Message::Text(text)) => {
                    let Ok(event) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    match parse_server_event(dialect, &event) {
                        ParsedRealtimeServerEvent::Interim {
                            utterance_id,
                            text,
                            update,
                        } => {
                            transcript.update(
                                utterance_id.as_deref(),
                                &text,
                                update == TranscriptUpdate::AppendDelta,
                                false,
                            );
                        }
                        ParsedRealtimeServerEvent::Final { utterance_id, text } => {
                            transcript.update(utterance_id.as_deref(), &text, false, true);
                            if !dialect.waits_for_session_finished() {
                                return transcript.finish();
                            }
                        }
                        ParsedRealtimeServerEvent::SessionFinished => return transcript.finish(),
                        ParsedRealtimeServerEvent::Error(message) => return Err(message),
                        _ => {}
                    }
                }
                Ok(Message::Ping(payload)) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("Realtime replay heartbeat failed: {error}"))?,
                Ok(Message::Close(_)) | Err(_) => {
                    return Err("Realtime replay closed before returning a transcript".to_string())
                }
                Ok(_) => {}
            }
        }
        Err("Realtime replay ended before returning a transcript".to_string())
    })
    .await
    .map_err(|_| "Timed out waiting for the replayed Realtime transcript".to_string())?
}

#[tauri::command]
pub async fn start_realtime_transcription_cmd(
    app_handle: AppHandle,
    app_state: State<'_, super::AppState>,
    realtime_state: State<'_, RealtimeTranscriptionState>,
) -> Result<String, String> {
    let config = app_state
        .db
        .load_app_config()
        .map_err(|error| error.to_string())?
        .speech_to_text;
    let dialect = RealtimeDialect::from_config(&config)?;

    let endpoint = build_realtime_endpoint(
        config.base_url.as_deref().unwrap_or_default(),
        &config.model,
    )?;
    let mut request = endpoint
        .into_client_request()
        .map_err(|error| format!("Invalid Realtime WebSocket request: {error}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key.trim())
            .parse()
            .map_err(|error| format!("Invalid realtime authorization header: {error}"))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        nexa_core::USER_AGENT
            .parse()
            .map_err(|error| format!("Invalid User-Agent header: {error}"))?,
    );

    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| format!("Unable to connect to realtime transcription: {error}"))?;
    let (mut socket_sink, mut socket_stream) = socket.split();
    socket_sink
        .send(Message::Text(
            build_session_update(dialect, &config.model, config.language.as_deref())
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| format!("Unable to configure realtime transcription: {error}"))?;

    let session_id = Uuid::new_v4().to_string();
    let (command_tx, mut command_rx) = mpsc::channel(REALTIME_COMMAND_BUFFER);
    realtime_state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), command_tx);

    let sessions = realtime_state.sessions.clone();
    let actor_session_id = session_id.clone();
    tokio::spawn(async move {
        let mut pending_final = None;
        let mut terminal_error = None;
        let mut transcript = RealtimeTranscript::default();
        let mut frontend_sequence = 0_u64;

        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    match command {
                        Some(RealtimeCommand::Append(audio_data)) => {
                            let payload = append_message(&audio_data);
                            if let Err(error) = socket_sink.send(Message::Text(payload.to_string().into())).await {
                                terminal_error = Some(format!("Unable to stream audio to realtime transcription: {error}"));
                                break;
                            }
                        }
                        Some(RealtimeCommand::Finish(response)) => {
                            if pending_final.is_some() {
                                let _ = response.send(Err("Realtime transcription is already finishing".to_string()));
                                continue;
                            }
                            let mut finish_error = None;
                            for message in finish_messages(dialect) {
                                if let Err(error) = socket_sink.send(Message::Text(message.to_string().into())).await {
                                    finish_error = Some(format!("Unable to finish realtime transcription audio: {error}"));
                                    break;
                                }
                            }
                            if let Some(message) = finish_error {
                                let _ = response.send(Err(message.clone()));
                                terminal_error = Some(message);
                                break;
                            }
                            pending_final = Some(response);
                        }
                        Some(RealtimeCommand::Cancel) | None => {
                            resolve_pending_final(
                                &mut pending_final,
                                Err("Realtime transcription was cancelled".to_string()),
                            );
                            let _ = socket_sink.send(Message::Close(None)).await;
                            emit_realtime_event(
                                &app_handle,
                                &actor_session_id,
                                &mut frontend_sequence,
                                "closed",
                                None,
                                None,
                                None,
                            );
                            break;
                        }
                    }
                }
                incoming = socket_stream.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            let event = match serde_json::from_str::<Value>(&text) {
                                Ok(event) => event,
                                Err(_) => continue,
                            };
                            match parse_server_event(dialect, &event) {
                                ParsedRealtimeServerEvent::Interim { utterance_id, text, update } => {
                                    transcript.update(utterance_id.as_deref(), &text, update == TranscriptUpdate::AppendDelta, false);
                                    if update == TranscriptUpdate::ReplaceSnapshot || !text.is_empty() {
                                        emit_realtime_event(
                                            &app_handle,
                                            &actor_session_id,
                                            &mut frontend_sequence,
                                            "interim",
                                            Some(&transcript.snapshot()),
                                            Some(TranscriptUpdate::ReplaceSnapshot),
                                            utterance_id.as_deref(),
                                        );
                                    }
                                }
                                ParsedRealtimeServerEvent::Final { utterance_id, text } => {
                                    transcript.update(utterance_id.as_deref(), &text, false, true);
                                    emit_realtime_event(
                                        &app_handle,
                                        &actor_session_id,
                                        &mut frontend_sequence,
                                        "final",
                                        Some(&transcript.snapshot()),
                                        Some(TranscriptUpdate::ReplaceSnapshot),
                                        utterance_id.as_deref(),
                                    );
                                    if !dialect.waits_for_session_finished() {
                                        resolve_pending_final(&mut pending_final, transcript.finish());
                                        let _ = socket_sink.send(Message::Close(None)).await;
                                        break;
                                    }
                                }
                                ParsedRealtimeServerEvent::SessionFinished => {
                                    let transcript = match transcript.finish() {
                                        Ok(transcript) => transcript,
                                        Err(message) => {
                                            terminal_error = Some(message);
                                            break;
                                        }
                                    };
                                    resolve_pending_final(&mut pending_final, Ok(transcript));
                                    let _ = socket_sink.send(Message::Close(None)).await;
                                    break;
                                }
                                ParsedRealtimeServerEvent::Error(message) => {
                                    terminal_error = Some(message);
                                    break;
                                }
                                ParsedRealtimeServerEvent::Other => {}
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if let Err(error) = socket_sink.send(Message::Pong(payload)).await {
                                terminal_error = Some(format!("Realtime transcription heartbeat failed: {error}"));
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            terminal_error = Some(if pending_final.is_some() {
                                "Realtime transcription closed before returning a transcript".to_string()
                            } else {
                                "Realtime transcription connection closed unexpectedly".to_string()
                            });
                            break;
                        }
                        Some(Err(error)) => {
                            terminal_error = Some(format!("Realtime transcription connection failed: {error}"));
                            break;
                        }
                        Some(Ok(_)) => {}
                    }
                }
            }
        }

        if let Some(message) = terminal_error {
            emit_realtime_event(
                &app_handle,
                &actor_session_id,
                &mut frontend_sequence,
                "error",
                Some(&message),
                None,
                None,
            );
            resolve_pending_final(&mut pending_final, Err(message));
        }
        sessions.lock().await.remove(&actor_session_id);
    });

    Ok(session_id)
}

async fn session_sender(
    state: &RealtimeTranscriptionState,
    session_id: &str,
) -> Result<mpsc::Sender<RealtimeCommand>, String> {
    state
        .sessions
        .lock()
        .await
        .get(session_id)
        .cloned()
        .ok_or_else(|| "Realtime transcription session is not active".to_string())
}

fn raw_realtime_audio(body: &tauri::ipc::InvokeBody) -> Result<Vec<u8>, String> {
    let tauri::ipc::InvokeBody::Raw(audio_data) = body else {
        return Err("Realtime transcription requires a raw binary request body".to_string());
    };
    if audio_data.len() > MAX_AUDIO_CHUNK_BYTES {
        return Err(format!(
            "Realtime audio chunk exceeds {MAX_AUDIO_CHUNK_BYTES} bytes"
        ));
    }
    if !audio_data.len().is_multiple_of(2) {
        return Err("Realtime PCM16 audio must contain complete 16-bit samples".to_string());
    }
    Ok(audio_data.clone())
}

#[tauri::command]
pub async fn append_realtime_transcription_audio_cmd(
    request: tauri::ipc::Request<'_>,
    state: State<'_, RealtimeTranscriptionState>,
) -> Result<(), String> {
    let session_id = request
        .headers()
        .get(SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Realtime transcription session header is missing".to_string())?
        .to_string();
    let audio_data = raw_realtime_audio(request.body())?;
    if audio_data.is_empty() {
        return Ok(());
    }
    session_sender(&state, &session_id)
        .await?
        .send(RealtimeCommand::Append(audio_data))
        .await
        .map_err(|_| "Realtime transcription session has closed".to_string())
}

#[tauri::command]
pub async fn finish_realtime_transcription_cmd(
    session_id: String,
    state: State<'_, RealtimeTranscriptionState>,
) -> Result<String, String> {
    let sender = session_sender(&state, &session_id).await?;
    let (response_tx, response_rx) = oneshot::channel();
    sender
        .send(RealtimeCommand::Finish(response_tx))
        .await
        .map_err(|_| "Realtime transcription session has closed".to_string())?;
    match tokio::time::timeout(FINAL_TRANSCRIPT_TIMEOUT, response_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Realtime transcription session ended unexpectedly".to_string()),
        Err(_) => {
            let _ = sender.send(RealtimeCommand::Cancel).await;
            Err("Timed out waiting for the final Realtime transcript".to_string())
        }
    }
}

#[tauri::command]
pub async fn cancel_realtime_transcription_cmd(
    session_id: String,
    state: State<'_, RealtimeTranscriptionState>,
) -> Result<(), String> {
    let Some(sender) = state.sessions.lock().await.get(&session_id).cloned() else {
        return Ok(());
    };
    sender
        .send(RealtimeCommand::Cancel)
        .await
        .map_err(|_| "Realtime transcription session has closed".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        build_realtime_endpoint, build_session_update, finish_messages, parse_server_event,
        raw_realtime_audio, replay_wav_data_bytes, transcribe_realtime_spool,
        ParsedRealtimeServerEvent, RealtimeDialect, TranscriptUpdate, MAX_AUDIO_CHUNK_BYTES,
    };
    use futures::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn realtime_audio_requires_bounded_aligned_raw_pcm() {
        assert_eq!(
            raw_realtime_audio(&tauri::ipc::InvokeBody::Raw(vec![1, 2])).unwrap(),
            vec![1, 2]
        );
        assert!(raw_realtime_audio(&tauri::ipc::InvokeBody::Raw(vec![1])).is_err());
        assert!(raw_realtime_audio(&tauri::ipc::InvokeBody::Raw(vec![
            0;
            MAX_AUDIO_CHUNK_BYTES + 2
        ]))
        .is_err());
        assert!(
            raw_realtime_audio(&tauri::ipc::InvokeBody::Json(serde_json::json!({
                "audioData": [1, 2]
            })))
            .is_err()
        );
    }

    #[test]
    fn replay_accepts_only_canonical_mono_24khz_pcm16() {
        let mut header = [0_u8; 44];
        header[0..4].copy_from_slice(b"RIFF");
        header[8..12].copy_from_slice(b"WAVE");
        header[12..16].copy_from_slice(b"fmt ");
        header[20..22].copy_from_slice(&1_u16.to_le_bytes());
        header[22..24].copy_from_slice(&1_u16.to_le_bytes());
        header[24..28].copy_from_slice(&24_000_u32.to_le_bytes());
        header[34..36].copy_from_slice(&16_u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&128_u32.to_le_bytes());

        assert_eq!(replay_wav_data_bytes(&header, 24_000).unwrap(), 128);
        header[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
        assert!(replay_wav_data_bytes(&header, 24_000).is_err());
        assert_eq!(replay_wav_data_bytes(&header, 16_000).unwrap(), 128);
    }

    #[test]
    fn realtime_endpoint_upgrades_https_and_selects_the_model() {
        assert_eq!(
            build_realtime_endpoint("https://api.openai.com/v1", "gpt-live-transcribe")
                .expect("valid OpenAI endpoint"),
            "wss://api.openai.com/v1/realtime?model=gpt-live-transcribe"
        );
        assert_eq!(
            build_realtime_endpoint("https://proxy.example/v1/realtime/", "gpt-live-transcribe")
                .expect("valid proxy endpoint"),
            "wss://proxy.example/v1/realtime?model=gpt-live-transcribe"
        );
    }

    #[test]
    fn realtime_dialects_reject_models_without_a_verified_wire_contract() {
        let mut config = nexa_core::app_settings::SpeechToTextConfig {
            provider: "alibaba_model_studio".to_string(),
            api_style: "dashscope_realtime_asr".to_string(),
            api_key: "test-key".to_string(),
            base_url: Some("https://dashscope.aliyuncs.com/api-ws/v1".to_string()),
            model: "qwen3-asr-flash".to_string(),
            ..nexa_core::app_settings::SpeechToTextConfig::default()
        };
        assert!(RealtimeDialect::from_config(&config).is_err());
        config.model = "qwen3-asr-flash-realtime".to_string();
        assert_eq!(
            RealtimeDialect::from_config(&config),
            Ok(RealtimeDialect::DashScope)
        );
    }

    #[test]
    fn session_update_uses_live_transcription_pcm_and_language_hints() {
        let payload = build_session_update(
            RealtimeDialect::OpenAi,
            "gpt-live-transcribe",
            Some("zh-cn, en"),
        );
        assert_eq!(payload["type"], "session.update");
        assert_eq!(payload["session"]["type"], "transcription");
        assert_eq!(
            payload["session"]["audio"]["input"]["format"]["rate"],
            24_000
        );
        assert_eq!(
            payload["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-live-transcribe"
        );
        assert_eq!(
            payload["session"]["audio"]["input"]["transcription"]["delay"],
            "low"
        );
        assert_eq!(
            payload["session"]["audio"]["input"]["transcription"]["languages"],
            serde_json::json!(["zh-cn", "en"])
        );
        assert!(payload["session"]["audio"]["input"]["turn_detection"].is_null());

        let dashscope = build_session_update(
            RealtimeDialect::DashScope,
            "qwen3-asr-flash-realtime",
            Some("zh"),
        );
        assert_eq!(dashscope["session"]["input_audio_format"], "pcm");
        assert_eq!(dashscope["session"]["sample_rate"], 16_000);
        assert_eq!(
            dashscope["session"]["input_audio_transcription"]["language"],
            "zh"
        );
        assert_eq!(dashscope["session"]["turn_detection"]["type"], "server_vad");
        assert_eq!(
            dashscope["session"]["turn_detection"]["silence_duration_ms"],
            400
        );
        assert_eq!(finish_messages(RealtimeDialect::OpenAi).len(), 1);
        assert_eq!(finish_messages(RealtimeDialect::DashScope).len(), 1);
        assert_eq!(
            finish_messages(RealtimeDialect::DashScope)[0]["type"],
            "session.finish"
        );
    }

    #[test]
    fn server_transcript_events_are_normalized_for_the_frontend() {
        assert_eq!(
            parse_server_event(
                RealtimeDialect::OpenAi,
                &serde_json::json!({
                    "type": "conversation.item.input_audio_transcription.delta",
                    "item_id": "item_1",
                    "delta": "hello"
                })
            ),
            ParsedRealtimeServerEvent::Interim {
                utterance_id: Some("item_1".to_string()),
                text: "hello".to_string(),
                update: TranscriptUpdate::AppendDelta,
            }
        );
        assert_eq!(
            parse_server_event(
                RealtimeDialect::OpenAi,
                &serde_json::json!({
                    "type": "conversation.item.input_audio_transcription.completed",
                    "item_id": "item_1",
                    "transcript": "hello world"
                })
            ),
            ParsedRealtimeServerEvent::Final {
                utterance_id: Some("item_1".to_string()),
                text: "hello world".to_string(),
            }
        );
        assert_eq!(
            parse_server_event(
                RealtimeDialect::DashScope,
                &serde_json::json!({
                    "type": "conversation.item.input_audio_transcription.text",
                    "item_id": "item_qwen",
                    "text": "今天",
                    "stash": "天气"
                })
            ),
            ParsedRealtimeServerEvent::Interim {
                utterance_id: Some("item_qwen".to_string()),
                text: "今天天气".to_string(),
                update: TranscriptUpdate::ReplaceSnapshot,
            }
        );
        assert_eq!(
            parse_server_event(
                RealtimeDialect::DashScope,
                &serde_json::json!({
                    "type": "session.finished"
                })
            ),
            ParsedRealtimeServerEvent::SessionFinished
        );
        assert_eq!(
            parse_server_event(
                RealtimeDialect::OpenAi,
                &serde_json::json!({
                    "type": "error",
                    "error": { "message": "bad session" }
                })
            ),
            ParsedRealtimeServerEvent::Error("bad session".to_string())
        );
    }

    fn canonical_pcm_wav(sample_rate: u32) -> Vec<u8> {
        let data_bytes = 640_u32;
        let mut wav = vec![0_u8; 44 + data_bytes as usize];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"fmt ");
        wav[16..20].copy_from_slice(&16_u32.to_le_bytes());
        wav[20..22].copy_from_slice(&1_u16.to_le_bytes());
        wav[22..24].copy_from_slice(&1_u16.to_le_bytes());
        wav[24..28].copy_from_slice(&sample_rate.to_le_bytes());
        wav[28..32].copy_from_slice(&(sample_rate * 2).to_le_bytes());
        wav[32..34].copy_from_slice(&2_u16.to_le_bytes());
        wav[34..36].copy_from_slice(&16_u16.to_le_bytes());
        wav[36..40].copy_from_slice(b"data");
        wav[40..44].copy_from_slice(&data_bytes.to_le_bytes());
        wav
    }

    #[tokio::test]
    async fn dashscope_realtime_spool_uses_snapshot_and_session_finish_contract() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock realtime server");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade websocket");
            let mut event_types = Vec::new();
            while let Some(message) = socket.next().await {
                let message = message.expect("client websocket message");
                let Message::Text(text) = message else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).expect("client event json");
                let event_type = event["type"].as_str().unwrap_or_default().to_string();
                event_types.push(event_type.clone());
                if event_type == "session.update" {
                    assert_eq!(event["session"]["sample_rate"], 16_000);
                    assert_eq!(event["session"]["input_audio_format"], "pcm");
                    assert_eq!(event["session"]["turn_detection"]["type"], "server_vad");
                }
                if event_type == "input_audio_buffer.append" {
                    // A complete VAD utterance arrives while audio is still
                    // uploading, before the user ends the microphone session.
                    socket
                        .send(Message::Text(
                            serde_json::json!({
                                "type": "conversation.item.input_audio_transcription.completed",
                                "item_id": "first-utterance", "transcript": "第一句。"
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .expect("send first utterance during upload");
                }
                if event_type == "session.finish" {
                    socket
                        .send(Message::Text(
                            serde_json::json!({
                                "type": "conversation.item.input_audio_transcription.text",
                                "item_id": "qwen-item",
                                "text": "实时",
                                "stash": "转写"
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .expect("send interim");
                    socket
                        .send(Message::Text(
                            serde_json::json!({
                                "type": "conversation.item.input_audio_transcription.completed",
                                "item_id": "qwen-item",
                                "transcript": "实时转写完成"
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .expect("send final");
                    socket
                        .send(Message::Text(
                            serde_json::json!({ "type": "session.finished" })
                                .to_string()
                                .into(),
                        ))
                        .await
                        .expect("send session finish");
                    break;
                }
            }
            assert_eq!(
                event_types.first().map(String::as_str),
                Some("session.update")
            );
            assert!(event_types
                .iter()
                .any(|value| value == "input_audio_buffer.append"));
            assert!(!event_types
                .iter()
                .any(|value| value == "input_audio_buffer.commit"));
            assert_eq!(
                event_types.last().map(String::as_str),
                Some("session.finish")
            );
        });

        let wav_path =
            std::env::temp_dir().join(format!("nexa-qwen-realtime-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&wav_path, canonical_pcm_wav(16_000)).expect("write mock wav");
        let config = nexa_core::app_settings::SpeechToTextConfig {
            provider: "alibaba_model_studio".to_string(),
            api_style: "dashscope_realtime_asr".to_string(),
            api_key: "test-key".to_string(),
            base_url: Some(format!("http://{address}/api-ws/v1")),
            model: "qwen3-asr-flash-realtime".to_string(),
            language: Some("zh".to_string()),
            ..nexa_core::app_settings::SpeechToTextConfig::default()
        };

        let transcript = transcribe_realtime_spool(&wav_path, &config)
            .await
            .expect("mock realtime transcript");
        let _ = std::fs::remove_file(&wav_path);
        server.await.expect("mock server join");

        assert_eq!(transcript, "第一句。 实时转写完成");
    }
}
