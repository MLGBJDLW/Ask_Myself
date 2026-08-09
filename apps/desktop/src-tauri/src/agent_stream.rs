use log::warn;
use nexa_core::agent::{AgentEvent, StreamBlockChannel, ToolRunItem};
use nexa_core::agent_run::{
    AgentRunDisplayKind, AgentRunEvent, AgentRunEventImportance, AgentRunEventVisibility,
};
use nexa_core::llm::{ContentPart, Message};
use nexa_core::runtime::AgentRunEventOutbox;
use serde::Serialize;
use tauri::AppHandle;
use uuid::Uuid;

use crate::app_events::emit_main_window_event;

/// Envelope for agent stream events sent to frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentFrontendEvent {
    conversation_id: String,
    #[serde(rename = "runEvent")]
    run_event: AgentRunEvent,
}

pub(crate) fn emit_agent_frontend_event(
    event_outbox: &AgentRunEventOutbox,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event: AgentEvent,
) {
    let event = compact_agent_event_for_frontend(event);
    let run_event =
        AgentRunEvent::from_agent_event(&event).with_context(Some(task_run_id), turn_id, Some(0));
    if let Err(error) = event_outbox.submit(run_event) {
        warn!("Failed to submit RunEvent for {conversation_id}: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_agent_frontend_event_with_presentation(
    event_outbox: &AgentRunEventOutbox,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event: AgentEvent,
    visibility: AgentRunEventVisibility,
    display_kind: AgentRunDisplayKind,
    importance: AgentRunEventImportance,
) {
    let event = compact_agent_event_for_frontend(event);
    let run_event = AgentRunEvent::from_agent_event(&event)
        .with_context(Some(task_run_id), turn_id, Some(0))
        .with_presentation(visibility, display_kind, importance);
    if let Err(error) = event_outbox.submit(run_event) {
        warn!("Failed to submit RunEvent for {conversation_id}: {error}");
    }
}

pub(crate) fn emit_agent_run_frontend_event(
    handle: &AppHandle,
    conversation_id: &str,
    run_event: &AgentRunEvent,
) {
    let payload = AgentFrontendEvent {
        conversation_id: conversation_id.to_string(),
        run_event: run_event.clone(),
    };
    emit_main_window_event(handle, "agent://run-event", &payload);
}

pub(crate) enum PendingStreamDelta {
    Text(String),
    Thinking(String),
}

const MAX_STREAM_BLOCK_DELTA_BYTES: usize = 8 * 1024;
pub(crate) const MAX_FRONTEND_TOOL_CONTENT_CHARS: usize = 128 * 1024;
const MAX_FRONTEND_TOOL_ARGUMENT_CHARS: usize = 128 * 1024;
const MAX_FRONTEND_MESSAGE_TEXT_CHARS: usize = 64 * 1024;
pub(crate) const MAX_FRONTEND_ARTIFACT_STRING_CHARS: usize = 32 * 1024;
const MAX_FRONTEND_ARTIFACT_ITEMS: usize = 512;
const MAX_FRONTEND_ARTIFACT_DEPTH: usize = 8;
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
    message.parts.retain_mut(|part| {
        match part {
            ContentPart::Text { text } => {
                *text = truncate_task_event_text(text, MAX_FRONTEND_MESSAGE_TEXT_CHARS);
                true
            }
            ContentPart::Image { data, .. } => {
                if data.len() > MAX_TASK_EVENT_TEXT_CHARS {
                    *data = "[image data omitted from stream event]".to_string();
                }
                true
            }
            // Provider replay envelopes can contain opaque signatures or
            // encrypted state. They are durable backend protocol state, never
            // a frontend stream payload.
            ContentPart::ProviderTurn { .. } => false,
        }
    });
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
        AgentEvent::Steering { content } => AgentEvent::Steering {
            content: truncate_task_event_text(&content, MAX_TASK_EVENT_TEXT_CHARS),
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
            context_breakdown,
            cached,
            finish_reason,
        } => AgentEvent::Done {
            message: compact_message_for_frontend(message),
            usage_total,
            last_prompt_tokens,
            context_breakdown,
            cached,
            finish_reason,
        },
        other => other,
    }
}

pub(crate) struct StreamBlockEmitter {
    event_outbox: AgentRunEventOutbox,
    answer_block_id: String,
    thinking_block_id: String,
    answer_offset: usize,
    thinking_offset: usize,
}

impl StreamBlockEmitter {
    pub(crate) fn new(event_outbox: AgentRunEventOutbox) -> Self {
        Self {
            event_outbox,
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
        AgentRunEvent::from_agent_event(event).with_context(Some(task_run_id), turn_id, Some(0))
    }

    pub(crate) fn emit_event(&self, conversation_id: &str, run_event: AgentRunEvent) {
        if let Err(error) = self.event_outbox.submit(run_event) {
            warn!("Failed to submit RunEvent for {conversation_id}: {error}");
        }
    }

    pub(crate) fn flush_pending(
        &mut self,
        pending: &mut Option<PendingStreamDelta>,
        conversation_id: &str,
        task_run_id: &str,
        turn_id: Option<&str>,
    ) {
        let Some(delta) = pending.take() else {
            return;
        };
        match delta {
            PendingStreamDelta::Text(delta) if !delta.is_empty() => self.emit_block_delta(
                conversation_id,
                task_run_id,
                turn_id,
                StreamBlockChannel::Answer,
                delta,
            ),
            PendingStreamDelta::Thinking(content) if !content.is_empty() => self.emit_block_delta(
                conversation_id,
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
        task_run_id: &str,
        turn_id: Option<&str>,
        channel: StreamBlockChannel,
        delta: String,
    ) {
        let (block_id, mut current_offset) = match channel {
            StreamBlockChannel::Answer => (self.answer_block_id.clone(), self.answer_offset),
            StreamBlockChannel::Thinking => (self.thinking_block_id.clone(), self.thinking_offset),
        };
        for chunk in split_text_by_utf8_bytes(&delta, MAX_STREAM_BLOCK_DELTA_BYTES) {
            let run_event = AgentRunEvent::output_delta(
                task_run_id,
                turn_id,
                0,
                &block_id,
                channel,
                current_offset,
                chunk,
            );
            if let Err(error) = self.event_outbox.submit(run_event) {
                warn!("Failed to submit stream block for {conversation_id}: {error}");
                break;
            }
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
            | AgentEvent::ToolRunStarted { .. }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::llm::provider_turn::{ProviderTurnEnvelope, RouteSnapshot};
    use nexa_core::llm::reasoning_profile::{ReasoningApiStyle, ReasoningReplayPolicy};
    use nexa_core::llm::Role;

    #[test]
    fn frontend_messages_drop_provider_replay_envelopes() {
        let mut message = Message::text(Role::Assistant, "visible answer");
        message.set_provider_turn(ProviderTurnEnvelope::capture(
            "turn-item",
            "sample",
            RouteSnapshot {
                provider_endpoint_id: "openai-public".to_string(),
                provider_family: "openai".to_string(),
                api_style: ReasoningApiStyle::OpenAiResponses,
                model_id: "gpt-5.6".to_string(),
                reasoning_profile_id: "openai-responses-reasoning-v1".to_string(),
                reasoning_profile_version: 1,
                replay_policy: ReasoningReplayPolicy::RequiredOnToolCall,
            },
            "visible answer",
            None,
            None,
            Vec::new(),
            true,
        ));

        let compacted = compact_message_for_frontend(message);

        assert_eq!(compacted.text_content(), "visible answer");
        assert!(compacted.provider_turn().is_none());
    }
}
