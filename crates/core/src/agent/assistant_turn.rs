//! Assistant-message persistence for draft and tool-call turns.

use super::*;

pub(super) struct AssistantTurnPersistenceContext<'a> {
    pub(super) db: &'a Database,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) model: &'a str,
    pub(super) route_kind: AgentRouteKind,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) sort_order: &'a mut i64,
}

impl AgentExecutor {
    pub(super) fn reasoning_replay_policy_for_request(
        &self,
        model: &str,
        force_reasoning_off: bool,
    ) -> ReasoningReplayPolicy {
        if force_reasoning_off
            || self.config.reasoning_enabled == Some(false)
            || self.config.reasoning_effort == Some(ReasoningEffort::None)
        {
            ReasoningReplayPolicy::NotRequired
        } else {
            self.provider.reasoning_replay_policy(model)
        }
    }

    pub(super) fn reasoning_envelope_for_persistence(
        &self,
        model: &str,
        display_text: Option<&str>,
        replay_text: Option<&str>,
        has_tool_calls: bool,
    ) -> Option<ReasoningEnvelope> {
        let display_text = crate::llm::reasoning_replay::sanitize_reasoning_text(display_text);
        let replay_text = crate::llm::reasoning_replay::sanitize_reasoning_text(replay_text);
        let replay_policy = self.reasoning_replay_policy_for_request(model, false);
        let required_for_replay = has_tool_calls && replay_policy.requires_tool_call_payload();
        let status = if replay_text.is_some() {
            ReasoningCaptureStatus::Captured
        } else if required_for_replay {
            ReasoningCaptureStatus::OmittedByProvider
        } else if self.config.reasoning_enabled == Some(false) {
            ReasoningCaptureStatus::NotRequested
        } else {
            ReasoningCaptureStatus::NotRequired
        };

        if display_text.is_none()
            && replay_text.is_none()
            && !required_for_replay
            && matches!(
                replay_policy,
                ReasoningReplayPolicy::NotRequired | ReasoningReplayPolicy::Unknown
            )
        {
            return None;
        }

        let source_field = replay_text
            .is_some()
            .then(|| "reasoning_content".to_string());
        Some(ReasoningEnvelope {
            display_text,
            replay_payload: replay_text.map(serde_json::Value::String),
            status,
            required_for_replay,
            source_field,
            provider_id: self.provider.name().to_string(),
            model_id: model.to_string(),
        })
    }

    pub(super) fn persist_steered_assistant_draft(
        &self,
        ctx: AssistantTurnPersistenceContext<'_>,
        assistant_msg: &Message,
        assistant_reasoning_content: Option<String>,
        iteration_thinking: &str,
    ) {
        self.persist_replayable_assistant_draft(
            ctx,
            assistant_msg,
            assistant_reasoning_content,
            iteration_thinking,
            Some((
                "Applied user steering after an assistant draft and continued the turn.",
                true,
            )),
            "steered assistant draft",
        );
    }

    pub(super) fn persist_stream_interrupted_assistant_draft(
        &self,
        ctx: AssistantTurnPersistenceContext<'_>,
        assistant_msg: &Message,
        assistant_reasoning_content: Option<String>,
        iteration_thinking: &str,
    ) {
        self.persist_replayable_assistant_draft(
            ctx,
            assistant_msg,
            assistant_reasoning_content,
            iteration_thinking,
            None,
            "stream-interrupted assistant draft",
        );
    }

    pub(super) fn persist_loop_guard_assistant_draft(
        &self,
        ctx: AssistantTurnPersistenceContext<'_>,
        assistant_msg: &Message,
        assistant_reasoning_content: Option<String>,
        iteration_thinking: &str,
    ) {
        self.persist_replayable_assistant_draft(
            ctx,
            assistant_msg,
            assistant_reasoning_content,
            iteration_thinking,
            Some((
                "Loop guard requested a strategy change after an assistant draft and continued the turn.",
                false,
            )),
            "loop-guard assistant draft",
        );
    }

    fn persist_replayable_assistant_draft(
        &self,
        ctx: AssistantTurnPersistenceContext<'_>,
        assistant_msg: &Message,
        assistant_reasoning_content: Option<String>,
        iteration_thinking: &str,
        status_message: Option<(&str, bool)>,
        warning_label: &str,
    ) {
        let AssistantTurnPersistenceContext {
            db,
            conversation_id,
            turn_id,
            model,
            route_kind,
            persisted_trace_items,
            sort_order,
        } = ctx;

        append_persisted_trace_thinking(persisted_trace_items, iteration_thinking);
        if let Some((status_message, internal_status)) = status_message {
            if internal_status {
                append_internal_persisted_trace_status(
                    persisted_trace_items,
                    status_message,
                    "info",
                );
            } else {
                append_persisted_trace_status(persisted_trace_items, status_message, "info");
            }
        }
        if let Some(cid) = conversation_id {
            let reasoning_envelope = self.reasoning_envelope_for_persistence(
                model,
                Some(iteration_thinking),
                assistant_reasoning_content.as_deref(),
                false,
            );
            let display_thinking = reasoning_envelope
                .as_ref()
                .and_then(|envelope| envelope.display_text.clone());
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: assistant_msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: merge_reasoning_envelope_artifact(None, reasoning_envelope),
                token_count: estimate_message_tokens_for_model(model, assistant_msg),
                created_at: String::new(),
                sort_order: *sort_order,
                thinking: display_thinking,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save {warning_label}: {e}");
            } else {
                *sort_order += 1;
            }
        }
        if let Some(tid) = turn_id {
            let trace = build_turn_trace(route_kind, persisted_trace_items);
            let _ = db.update_conversation_turn_progress(
                tid,
                Some(&format!("{:?}", route_kind)),
                Some(&trace),
            );
        }
    }

    pub(super) fn persist_intermediate_tool_call_assistant(
        &self,
        ctx: AssistantTurnPersistenceContext<'_>,
        assistant_msg: &Message,
        tool_calls: &[ToolCallRequest],
        assistant_reasoning_content: Option<String>,
        iteration_thinking: &str,
    ) {
        let AssistantTurnPersistenceContext {
            db,
            conversation_id,
            turn_id,
            model,
            route_kind,
            persisted_trace_items,
            sort_order,
        } = ctx;

        append_persisted_trace_thinking(persisted_trace_items, iteration_thinking);
        if let Some(tid) = turn_id {
            let trace = build_turn_trace(route_kind, persisted_trace_items);
            let _ = db.update_conversation_turn_progress(
                tid,
                Some(&format!("{:?}", route_kind)),
                Some(&trace),
            );
        }
        if let Some(cid) = conversation_id {
            let reasoning_envelope = self.reasoning_envelope_for_persistence(
                model,
                Some(iteration_thinking),
                assistant_reasoning_content.as_deref(),
                true,
            );
            let display_thinking = reasoning_envelope
                .as_ref()
                .and_then(|envelope| envelope.display_text.clone());
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: assistant_msg.text_content(),
                tool_call_id: None,
                tool_calls: tool_calls.to_vec(),
                artifacts: merge_reasoning_envelope_artifact(None, reasoning_envelope),
                token_count: estimate_message_tokens_for_model(model, assistant_msg),
                created_at: String::new(),
                sort_order: *sort_order,
                thinking: display_thinking,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save intermediate assistant message: {e}");
            }
            *sort_order += 1;
        }
    }
}
