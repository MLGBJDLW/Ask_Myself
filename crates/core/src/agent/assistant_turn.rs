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
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: assistant_msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: None,
                token_count: estimate_message_tokens_for_model(model, assistant_msg),
                created_at: String::new(),
                sort_order: *sort_order,
                thinking: assistant_reasoning_content,
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
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: assistant_msg.text_content(),
                tool_call_id: None,
                tool_calls: tool_calls.to_vec(),
                artifacts: None,
                token_count: estimate_message_tokens_for_model(model, assistant_msg),
                created_at: String::new(),
                sort_order: *sort_order,
                thinking: assistant_reasoning_content,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save intermediate assistant message: {e}");
            }
            *sort_order += 1;
        }
    }
}
