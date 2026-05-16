use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nexa_core::agent::{AgentEvent, StreamBlockChannel, ToolRunItem};
use nexa_core::agent_run::AgentRunEvent;
use nexa_core::db::Database;
use nexa_core::llm::{ContentPart, Message};
use serde::Serialize;
use tauri::AppHandle;
use uuid::Uuid;

use crate::app_events::emit_app_event;

/// Envelope for agent stream events sent to frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentFrontendEvent {
    conversation_id: String,
    #[serde(rename = "runEvent")]
    run_event: AgentRunEvent,
}

pub(crate) fn emit_agent_frontend_event(
    handle: &AppHandle,
    event_seq: &AtomicU64,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event: AgentEvent,
) -> AgentRunEvent {
    let event = compact_agent_event_for_frontend(event);
    let event_seq = event_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let run_event = AgentRunEvent::from_agent_event(&event).with_context(
        Some(task_run_id),
        turn_id,
        Some(event_seq),
    );
    let payload = AgentFrontendEvent {
        conversation_id: conversation_id.to_string(),
        run_event: run_event.clone(),
    };
    emit_app_event(handle, "agent:event", &payload);
    run_event
}

pub(crate) enum PendingStreamDelta {
    Text(String),
    Thinking(String),
}

const MAX_STREAM_BLOCK_DELTA_BYTES: usize = 8 * 1024;
pub(crate) const MAX_FRONTEND_TOOL_CONTENT_CHARS: usize = 64 * 1024;
const MAX_FRONTEND_TOOL_ARGUMENT_CHARS: usize = 16 * 1024;
const MAX_FRONTEND_MESSAGE_TEXT_CHARS: usize = 64 * 1024;
pub(crate) const MAX_FRONTEND_ARTIFACT_STRING_CHARS: usize = 8 * 1024;
const MAX_FRONTEND_ARTIFACT_ITEMS: usize = 64;
const MAX_FRONTEND_ARTIFACT_DEPTH: usize = 6;
pub(crate) const MAX_TASK_EVENT_TEXT_CHARS: usize = 4_000;

pub(crate) fn split_text_by_utf8_bytes(text: &str, max_bytes: usize) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let max_bytes = max_bytes.max(1);
    if text.len() <= max_bytes {
        return vec![text];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = start
                + text[start..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1);
        }
        chunks.push(&text[start..end]);
        start = end;
    }
    chunks
}

fn compact_json_value_for_frontend(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    if depth >= MAX_FRONTEND_ARTIFACT_DEPTH {
        return match value {
            serde_json::Value::String(text) => serde_json::Value::String(truncate_task_event_text(
                text,
                MAX_FRONTEND_ARTIFACT_STRING_CHARS,
            )),
            other => serde_json::Value::String(truncate_task_event_text(
                &other.to_string(),
                MAX_FRONTEND_ARTIFACT_STRING_CHARS,
            )),
        };
    }

    match value {
        serde_json::Value::String(text) => serde_json::Value::String(truncate_task_event_text(
            text,
            MAX_FRONTEND_ARTIFACT_STRING_CHARS,
        )),
        serde_json::Value::Array(items) => {
            let mut out = items
                .iter()
                .take(MAX_FRONTEND_ARTIFACT_ITEMS)
                .map(|item| compact_json_value_for_frontend(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > MAX_FRONTEND_ARTIFACT_ITEMS {
                out.push(serde_json::json!({
                    "truncated": true,
                    "omittedItems": items.len() - MAX_FRONTEND_ARTIFACT_ITEMS,
                }));
            }
            serde_json::Value::Array(out)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (idx, (key, item)) in map.iter().enumerate() {
                if idx >= MAX_FRONTEND_ARTIFACT_ITEMS {
                    out.insert("_truncated".to_string(), serde_json::Value::Bool(true));
                    out.insert(
                        "_omittedKeys".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(
                            map.len() - MAX_FRONTEND_ARTIFACT_ITEMS,
                        )),
                    );
                    break;
                }
                out.insert(
                    key.clone(),
                    compact_json_value_for_frontend(item, depth + 1),
                );
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

fn compact_tool_run_for_frontend(mut run: ToolRunItem) -> ToolRunItem {
    run.arguments = run
        .arguments
        .map(|arguments| truncate_task_event_text(&arguments, MAX_FRONTEND_TOOL_ARGUMENT_CHARS));
    run.content = run
        .content
        .map(|content| truncate_task_event_text(&content, MAX_FRONTEND_TOOL_CONTENT_CHARS));
    run.progress_note = run
        .progress_note
        .map(|note| truncate_task_event_text(&note, MAX_TASK_EVENT_TEXT_CHARS));
    run.artifacts = run
        .artifacts
        .map(|artifacts| compact_json_value_for_frontend(&artifacts, 0));
    run
}

fn compact_message_for_frontend(mut message: Message) -> Message {
    for part in &mut message.parts {
        match part {
            ContentPart::Text { text } => {
                *text = truncate_task_event_text(text, MAX_FRONTEND_MESSAGE_TEXT_CHARS);
            }
            ContentPart::Image { data, .. } => {
                if data.len() > MAX_TASK_EVENT_TEXT_CHARS {
                    *data = "[image data omitted from stream event]".to_string();
                }
            }
        }
    }
    message.reasoning_content = message
        .reasoning_content
        .map(|text| truncate_task_event_text(&text, MAX_TASK_EVENT_TEXT_CHARS));
    message
}

pub(crate) fn compact_agent_event_for_frontend(event: AgentEvent) -> AgentEvent {
    match event {
        AgentEvent::ToolCallStart {
            call_id,
            tool_name,
            arguments,
        } => AgentEvent::ToolCallStart {
            call_id,
            tool_name,
            arguments: truncate_task_event_text(&arguments, MAX_FRONTEND_TOOL_ARGUMENT_CHARS),
        },
        AgentEvent::ToolCallArgsDelta {
            call_id,
            tool_name,
            arguments_delta,
            index,
        } => AgentEvent::ToolCallArgsDelta {
            call_id,
            tool_name,
            arguments_delta: truncate_task_event_text(
                &arguments_delta,
                MAX_FRONTEND_TOOL_ARGUMENT_CHARS,
            ),
            index,
        },
        AgentEvent::ToolCallResult {
            call_id,
            tool_name,
            content,
            is_error,
            artifacts,
        } => AgentEvent::ToolCallResult {
            call_id,
            tool_name,
            content: truncate_task_event_text(&content, MAX_FRONTEND_TOOL_CONTENT_CHARS),
            is_error,
            artifacts: artifacts.map(|value| compact_json_value_for_frontend(&value, 0)),
        },
        AgentEvent::ToolRunStarted { run } => AgentEvent::ToolRunStarted {
            run: compact_tool_run_for_frontend(run),
        },
        AgentEvent::ToolRunUpdated { run } => AgentEvent::ToolRunUpdated {
            run: compact_tool_run_for_frontend(run),
        },
        AgentEvent::ToolRunCompleted { run } => AgentEvent::ToolRunCompleted {
            run: compact_tool_run_for_frontend(run),
        },
        AgentEvent::Thinking { content } => AgentEvent::Thinking {
            content: truncate_task_event_text(&content, MAX_FRONTEND_TOOL_CONTENT_CHARS),
        },
        AgentEvent::Status { content, tone } => AgentEvent::Status {
            content: truncate_task_event_text(&content, MAX_TASK_EVENT_TEXT_CHARS),
            tone,
        },
        AgentEvent::PlanUpdated {
            plan,
            phase,
            summary,
        } => AgentEvent::PlanUpdated {
            plan: compact_json_value_for_frontend(&plan, 0),
            phase,
            summary: summary.map(|text| truncate_task_event_text(&text, MAX_TASK_EVENT_TEXT_CHARS)),
        },
        AgentEvent::Done {
            message,
            usage_total,
            last_prompt_tokens,
            cached,
            finish_reason,
        } => AgentEvent::Done {
            message: compact_message_for_frontend(message),
            usage_total,
            last_prompt_tokens,
            cached,
            finish_reason,
        },
        other => other,
    }
}

pub(crate) struct StreamBlockEmitter {
    event_seq: Arc<AtomicU64>,
    answer_block_id: String,
    thinking_block_id: String,
    answer_offset: usize,
    thinking_offset: usize,
}

impl StreamBlockEmitter {
    pub(crate) fn new(event_seq: Arc<AtomicU64>) -> Self {
        Self {
            event_seq,
            answer_block_id: new_stream_block_id(StreamBlockChannel::Answer),
            thinking_block_id: new_stream_block_id(StreamBlockChannel::Thinking),
            answer_offset: 0,
            thinking_offset: 0,
        }
    }

    pub(crate) fn rotate_blocks(&mut self) {
        self.answer_block_id = new_stream_block_id(StreamBlockChannel::Answer);
        self.thinking_block_id = new_stream_block_id(StreamBlockChannel::Thinking);
        self.answer_offset = 0;
        self.thinking_offset = 0;
    }

    pub(crate) fn next_run_event(
        &self,
        task_run_id: &str,
        turn_id: Option<&str>,
        event: &AgentEvent,
    ) -> AgentRunEvent {
        let event_seq = self.event_seq.fetch_add(1, Ordering::SeqCst) + 1;
        AgentRunEvent::from_agent_event(event).with_context(
            Some(task_run_id),
            turn_id,
            Some(event_seq),
        )
    }

    pub(crate) fn emit_event(
        &self,
        handle: &AppHandle,
        conversation_id: &str,
        run_event: AgentRunEvent,
    ) {
        let payload = AgentFrontendEvent {
            conversation_id: conversation_id.to_string(),
            run_event,
        };
        emit_app_event(handle, "agent:event", &payload);
    }

    pub(crate) fn flush_pending(
        &mut self,
        pending: &mut Option<PendingStreamDelta>,
        conversation_id: &str,
        handle: &AppHandle,
        db: &Database,
        task_run_id: &str,
        turn_id: Option<&str>,
    ) {
        let Some(delta) = pending.take() else {
            return;
        };
        match delta {
            PendingStreamDelta::Text(delta) if !delta.is_empty() => self.emit_block_delta(
                conversation_id,
                handle,
                db,
                task_run_id,
                turn_id,
                StreamBlockChannel::Answer,
                delta,
            ),
            PendingStreamDelta::Thinking(content) if !content.is_empty() => self.emit_block_delta(
                conversation_id,
                handle,
                db,
                task_run_id,
                turn_id,
                StreamBlockChannel::Thinking,
                content,
            ),
            _ => {}
        }
    }

    fn emit_block_delta(
        &mut self,
        conversation_id: &str,
        handle: &AppHandle,
        db: &Database,
        task_run_id: &str,
        turn_id: Option<&str>,
        channel: StreamBlockChannel,
        delta: String,
    ) {
        let (block_id, mut current_offset) = match channel {
            StreamBlockChannel::Answer => (self.answer_block_id.clone(), self.answer_offset),
            StreamBlockChannel::Thinking => (self.thinking_block_id.clone(), self.thinking_offset),
        };
        let channel_label = stream_block_channel_label(channel);
        for chunk in split_text_by_utf8_bytes(&delta, MAX_STREAM_BLOCK_DELTA_BYTES) {
            let event_seq = self.event_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let run_event = AgentRunEvent::output_delta(
                task_run_id,
                turn_id,
                event_seq,
                &block_id,
                channel,
                current_offset,
                chunk,
            );
            let payload = AgentFrontendEvent {
                conversation_id: conversation_id.to_string(),
                run_event: run_event.clone(),
            };
            emit_app_event(handle, "agent:event", &payload);

            let durable_payload = payload_with_agent_run_protocol(&run_event, None);
            let _ = db.record_agent_task_run_event(
                task_run_id,
                run_event.task_event_type(),
                channel_label,
                Some("running"),
                Some(&durable_payload),
            );
            current_offset += chunk.len();
        }

        match channel {
            StreamBlockChannel::Answer => self.answer_offset = current_offset,
            StreamBlockChannel::Thinking => self.thinking_offset = current_offset,
        }
    }
}

fn new_stream_block_id(channel: StreamBlockChannel) -> String {
    format!(
        "stream-{}-{}",
        stream_block_channel_label(channel),
        Uuid::new_v4()
    )
}

fn stream_block_channel_label(channel: StreamBlockChannel) -> &'static str {
    match channel {
        StreamBlockChannel::Answer => "answer",
        StreamBlockChannel::Thinking => "thinking",
    }
}

pub(crate) fn agent_event_rotates_stream_blocks(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::StreamReset { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolRunStarted { .. }
            | AgentEvent::ToolCallResult { .. }
            | AgentEvent::ToolRunCompleted { .. }
    )
}

pub(crate) fn truncate_task_event_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(max_chars.saturating_sub(32))
        .collect::<String>();
    out.push_str("\n[truncated]");
    out
}

pub(crate) fn payload_with_agent_run_protocol(
    run_event: &AgentRunEvent,
    payload: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut map = match payload {
        Some(serde_json::Value::Object(existing)) => existing.clone(),
        Some(existing) => {
            let mut map = serde_json::Map::new();
            map.insert("data".to_string(), existing.clone());
            map
        }
        None => serde_json::Map::new(),
    };
    map.insert(
        "agentRun".to_string(),
        serde_json::to_value(run_event).unwrap_or_else(|_| serde_json::json!({})),
    );
    serde_json::Value::Object(map)
}
