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

#[derive(Debug, PartialEq, Eq)]
enum ParsedRealtimeServerEvent {
    Delta {
        item_id: Option<String>,
        text: String,
    },
    Completed {
        item_id: Option<String>,
        text: String,
    },
    Error(String),
    Other,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RealtimeTranscriptionFrontendEvent<'a> {
    session_id: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<&'a str>,
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

fn build_session_update(model: &str, language: Option<&str>) -> Value {
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
                        "rate": 24_000
                    },
                    "transcription": transcription,
                    "turn_detection": null
                }
            }
        }
    })
}

fn parse_server_event(event: &Value) -> ParsedRealtimeServerEvent {
    let item_id = event
        .get("item_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "conversation.item.input_audio_transcription.delta" => ParsedRealtimeServerEvent::Delta {
            item_id,
            text: event
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "conversation.item.input_audio_transcription.completed" => {
            ParsedRealtimeServerEvent::Completed {
                item_id,
                text: event
                    .get("transcript")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        }
        "error" => ParsedRealtimeServerEvent::Error(
            event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| event.get("message").and_then(Value::as_str))
                .unwrap_or("OpenAI Realtime session failed")
                .to_string(),
        ),
        _ => ParsedRealtimeServerEvent::Other,
    }
}

fn emit_realtime_event(
    app_handle: &AppHandle,
    session_id: &str,
    kind: &str,
    text: Option<&str>,
    item_id: Option<&str>,
) {
    emit_app_event(
        app_handle,
        REALTIME_TRANSCRIPTION_EVENT,
        &RealtimeTranscriptionFrontendEvent {
            session_id,
            kind,
            text,
            item_id,
        },
    );
}

fn resolve_pending_final(
    pending_final: &mut Option<oneshot::Sender<Result<String, String>>>,
    result: Result<String, String>,
) {
    if let Some(sender) = pending_final.take() {
        let _ = sender.send(result);
    }
}

fn replay_wav_data_bytes(header: &[u8; 44]) -> Result<u32, String> {
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
    if audio_format != 1 || channels != 1 || sample_rate != 24_000 || bits_per_sample != 16 {
        return Err("Managed Realtime replay requires mono 24 kHz little-endian PCM16".to_string());
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
    if config.api_style != "openai_realtime_transcription" || !config.is_configured() {
        return Err("OpenAI Live transcription is not fully configured".to_string());
    }
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
            .map_err(|error| format!("Invalid OpenAI authorization header: {error}"))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        nexa_core::USER_AGENT
            .parse()
            .map_err(|error| format!("Invalid User-Agent header: {error}"))?,
    );

    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| format!("Unable to reconnect to OpenAI Realtime: {error}"))?;
    socket
        .send(Message::Text(
            build_session_update(&config.model, config.language.as_deref())
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| format!("Unable to configure OpenAI Realtime replay: {error}"))?;

    let mut file = tokio::fs::File::open(wav_path)
        .await
        .map_err(|error| format!("Unable to open managed Realtime audio: {error}"))?;
    let mut header = [0_u8; 44];
    file.read_exact(&mut header)
        .await
        .map_err(|error| format!("Unable to read managed Realtime WAV header: {error}"))?;
    let mut remaining = replay_wav_data_bytes(&header)? as usize;
    file.seek(SeekFrom::Start(44))
        .await
        .map_err(|error| format!("Unable to seek managed Realtime audio: {error}"))?;
    let mut buffer = vec![0_u8; REPLAY_PCM_CHUNK_BYTES];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len());
        file.read_exact(&mut buffer[..chunk_len])
            .await
            .map_err(|error| format!("Unable to read managed Realtime audio: {error}"))?;
        let payload = serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": BASE64.encode(&buffer[..chunk_len]),
        });
        socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .map_err(|error| format!("Unable to replay audio to OpenAI Realtime: {error}"))?;
        remaining -= chunk_len;

        if let Ok(Some(incoming)) =
            tokio::time::timeout(Duration::from_millis(1), socket.next()).await
        {
            match incoming {
                Ok(Message::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<Value>(&text) {
                        if let ParsedRealtimeServerEvent::Error(message) =
                            parse_server_event(&event)
                        {
                            return Err(message);
                        }
                    }
                }
                Ok(Message::Ping(payload)) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("OpenAI Realtime replay heartbeat failed: {error}"))?,
                Ok(Message::Close(_)) => {
                    return Err("OpenAI Realtime replay closed while uploading audio".to_string())
                }
                Err(error) => return Err(format!("OpenAI Realtime replay failed: {error}")),
                Ok(_) => {}
            }
        }
    }
    socket
        .send(Message::Text(
            serde_json::json!({ "type": "input_audio_buffer.commit" })
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| format!("Unable to commit OpenAI Realtime replay: {error}"))?;

    tokio::time::timeout(REPLAY_FINAL_TRANSCRIPT_TIMEOUT, async {
        while let Some(incoming) = socket.next().await {
            match incoming {
                Ok(Message::Text(text)) => {
                    let Ok(event) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    match parse_server_event(&event) {
                        ParsedRealtimeServerEvent::Completed { text, .. } => return Ok(text),
                        ParsedRealtimeServerEvent::Error(message) => return Err(message),
                        _ => {}
                    }
                }
                Ok(Message::Ping(payload)) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("OpenAI Realtime replay heartbeat failed: {error}"))?,
                Ok(Message::Close(_)) | Err(_) => {
                    return Err(
                        "OpenAI Realtime replay closed before returning a transcript".to_string(),
                    )
                }
                Ok(_) => {}
            }
        }
        Err("OpenAI Realtime replay ended before returning a transcript".to_string())
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
    if config.api_style != "openai_realtime_transcription" || !config.is_configured() {
        return Err("OpenAI Live transcription is not fully configured".to_string());
    }

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
            .map_err(|error| format!("Invalid OpenAI authorization header: {error}"))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        nexa_core::USER_AGENT
            .parse()
            .map_err(|error| format!("Invalid User-Agent header: {error}"))?,
    );

    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| format!("Unable to connect to OpenAI Realtime: {error}"))?;
    let (mut socket_sink, mut socket_stream) = socket.split();
    socket_sink
        .send(Message::Text(
            build_session_update(&config.model, config.language.as_deref())
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| format!("Unable to configure OpenAI Realtime: {error}"))?;

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

        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    match command {
                        Some(RealtimeCommand::Append(audio_data)) => {
                            let payload = serde_json::json!({
                                "type": "input_audio_buffer.append",
                                "audio": BASE64.encode(audio_data),
                            });
                            if let Err(error) = socket_sink.send(Message::Text(payload.to_string().into())).await {
                                terminal_error = Some(format!("Unable to stream audio to OpenAI Realtime: {error}"));
                                break;
                            }
                        }
                        Some(RealtimeCommand::Finish(response)) => {
                            if pending_final.is_some() {
                                let _ = response.send(Err("Realtime transcription is already finishing".to_string()));
                                continue;
                            }
                            let commit = serde_json::json!({ "type": "input_audio_buffer.commit" });
                            if let Err(error) = socket_sink.send(Message::Text(commit.to_string().into())).await {
                                let message = format!("Unable to commit OpenAI Realtime audio: {error}");
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
                                "closed",
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
                            match parse_server_event(&event) {
                                ParsedRealtimeServerEvent::Delta { item_id, text } => {
                                    if !text.is_empty() {
                                        emit_realtime_event(
                                            &app_handle,
                                            &actor_session_id,
                                            "delta",
                                            Some(&text),
                                            item_id.as_deref(),
                                        );
                                    }
                                }
                                ParsedRealtimeServerEvent::Completed { item_id, text } => {
                                    emit_realtime_event(
                                        &app_handle,
                                        &actor_session_id,
                                        "completed",
                                        Some(&text),
                                        item_id.as_deref(),
                                    );
                                    resolve_pending_final(&mut pending_final, Ok(text));
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
                                terminal_error = Some(format!("OpenAI Realtime heartbeat failed: {error}"));
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            terminal_error = Some(if pending_final.is_some() {
                                "OpenAI Realtime closed before returning a transcript".to_string()
                            } else {
                                "OpenAI Realtime connection closed unexpectedly".to_string()
                            });
                            break;
                        }
                        Some(Err(error)) => {
                            terminal_error = Some(format!("OpenAI Realtime connection failed: {error}"));
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
                "error",
                Some(&message),
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
        build_realtime_endpoint, build_session_update, parse_server_event, raw_realtime_audio,
        replay_wav_data_bytes, ParsedRealtimeServerEvent, MAX_AUDIO_CHUNK_BYTES,
    };

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

        assert_eq!(replay_wav_data_bytes(&header).unwrap(), 128);
        header[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
        assert!(replay_wav_data_bytes(&header).is_err());
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
    fn session_update_uses_live_transcription_pcm_and_language_hints() {
        let payload = build_session_update("gpt-live-transcribe", Some("zh-cn, en"));
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
    }

    #[test]
    fn server_transcript_events_are_normalized_for_the_frontend() {
        assert_eq!(
            parse_server_event(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.delta",
                "item_id": "item_1",
                "delta": "hello"
            })),
            ParsedRealtimeServerEvent::Delta {
                item_id: Some("item_1".to_string()),
                text: "hello".to_string(),
            }
        );
        assert_eq!(
            parse_server_event(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "item_id": "item_1",
                "transcript": "hello world"
            })),
            ParsedRealtimeServerEvent::Completed {
                item_id: Some("item_1".to_string()),
                text: "hello world".to_string(),
            }
        );
        assert_eq!(
            parse_server_event(&serde_json::json!({
                "type": "error",
                "error": { "message": "bad session" }
            })),
            ParsedRealtimeServerEvent::Error("bad session".to_string())
        );
    }
}
