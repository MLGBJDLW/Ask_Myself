use log::warn;
use nexa_core::agent::AgentEvent;
use nexa_core::agent_run::AgentRunEvent;
use nexa_core::conversation::{AgentTaskRun, AgentTaskRunEvent};
use nexa_core::db::Database;
use serde::Serialize;
use tauri::AppHandle;

use crate::agent_stream::{
    compact_agent_event_for_frontend, payload_with_agent_run_protocol, truncate_task_event_text,
    MAX_TASK_EVENT_TEXT_CHARS,
};
use crate::app_events::emit_app_event;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskRunUpdatedEvent {
    conversation_id: String,
    #[serde(rename = "type")]
    event_type: &'static str,
    task_run: AgentTaskRun,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskRunEventEnvelope {
    conversation_id: String,
    #[serde(rename = "type")]
    event_type: &'static str,
    task_event: AgentTaskRunEvent,
}

struct TaskEventEmitContext<'a> {
    db: &'a Database,
    app_handle: &'a AppHandle,
    conversation_id: &'a str,
    task_run_id: &'a str,
}

pub(crate) fn emit_agent_task_run_update(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
) {
    match db.get_agent_task_run(task_run_id) {
        Ok(task_run) => {
            let payload = AgentTaskRunUpdatedEvent {
                conversation_id: conversation_id.to_string(),
                event_type: "taskRunUpdated",
                task_run,
            };
            emit_app_event(app_handle, "agent:event", &payload);
        }
        Err(err) => warn!("Failed to load task run {task_run_id} for event: {err}"),
    }
}

pub(crate) fn emit_agent_task_event(
    app_handle: &AppHandle,
    conversation_id: &str,
    task_event: AgentTaskRunEvent,
) {
    let payload = AgentTaskRunEventEnvelope {
        conversation_id: conversation_id.to_string(),
        event_type: "taskRunEvent",
        task_event,
    };
    emit_app_event(app_handle, "agent:event", &payload);
}

fn record_and_emit_task_event(
    ctx: &TaskEventEmitContext<'_>,
    event_type: &str,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    match ctx
        .db
        .record_agent_task_run_event(ctx.task_run_id, event_type, label, status, payload)
    {
        Ok(event) => emit_agent_task_event(ctx.app_handle, ctx.conversation_id, event),
        Err(err) => warn!("Failed to record task event for {}: {err}", ctx.task_run_id),
    }
}

fn record_and_emit_agent_run_task_event(
    ctx: &TaskEventEmitContext<'_>,
    run_event: &AgentRunEvent,
    event_type: &str,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    let payload = payload_with_agent_run_protocol(run_event, payload);
    record_and_emit_task_event(ctx, event_type, label, status, Some(&payload));
}

pub(crate) fn record_agent_run_task_event(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    run_event: &AgentRunEvent,
    event_type: &str,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    let task_event_ctx = TaskEventEmitContext {
        db,
        app_handle,
        conversation_id,
        task_run_id,
    };
    record_and_emit_agent_run_task_event(
        &task_event_ctx,
        run_event,
        event_type,
        label,
        status,
        payload,
    );
}

pub(crate) fn record_task_progress_for_agent_event(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    event: &AgentEvent,
    run_event: &AgentRunEvent,
) {
    let event = compact_agent_event_for_frontend(event.clone());
    let task_event_ctx = TaskEventEmitContext {
        db,
        app_handle,
        conversation_id,
        task_run_id,
    };

    match &event {
        AgentEvent::StreamReset { reason } => {
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                run_event.task_event_type(),
                reason,
                Some("running"),
                None,
            );
        }
        AgentEvent::ToolCallStart {
            call_id,
            tool_name,
            arguments,
        } => {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("running"),
                Some("tooling"),
                None,
                Some(&format!("Running {tool_name}")),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            let payload = serde_json::json!({
                "callId": call_id,
                "toolName": tool_name,
                "arguments": truncate_task_event_text(arguments, MAX_TASK_EVENT_TEXT_CHARS),
            });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "tool",
                tool_name,
                Some("running"),
                Some(&payload),
            );
        }
        AgentEvent::ToolCallProgress { call_id, note } => {
            let payload = serde_json::json!({ "callId": call_id, "note": note });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "toolProgress",
                note,
                Some("running"),
                Some(&payload),
            );
        }
        AgentEvent::ToolCallResult {
            call_id,
            tool_name,
            content,
            is_error,
            artifacts,
        } => {
            let status = if *is_error { "failed" } else { "completed" };
            let payload = serde_json::json!({
                "callId": call_id,
                "toolName": tool_name,
                "isError": is_error,
                "content": truncate_task_event_text(content, MAX_TASK_EVENT_TEXT_CHARS),
                "artifacts": artifacts,
            });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "tool",
                tool_name,
                Some(status),
                Some(&payload),
            );
        }
        AgentEvent::ToolRunStarted { run }
        | AgentEvent::ToolRunUpdated { run }
        | AgentEvent::ToolRunCompleted { run } => {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("running"),
                Some("tooling"),
                None,
                Some(&run.tool_name),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            let payload = serde_json::json!({ "run": run });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                run_event.task_event_type(),
                &run.tool_name,
                Some(run.status.as_str()),
                Some(&payload),
            );
        }
        AgentEvent::Status { content, tone } => {
            if let Some(route) = content.strip_prefix("Route selected: ") {
                let _ = db.update_agent_task_run_progress(
                    task_run_id,
                    Some("running"),
                    Some("routing"),
                    Some(route.trim()),
                    Some("Route selected"),
                    None,
                    None,
                );
                emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            }
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "status",
                content,
                tone.as_deref(),
                None,
            );
        }
        AgentEvent::PlanUpdated {
            plan,
            phase,
            summary,
        } => {
            let phase = phase.as_deref().unwrap_or("planning");
            let summary = summary.as_deref().unwrap_or("Execution plan updated");
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("running"),
                Some(phase),
                None,
                Some(summary),
                Some(plan),
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "plan",
                summary,
                Some("running"),
                Some(plan),
            );
        }
        AgentEvent::Done { finish_reason, .. } => {
            let payload = serde_json::json!({ "finishReason": finish_reason });
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("running"),
                Some("finalizing"),
                None,
                Some("Finalizing answer"),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "status",
                "Final answer produced",
                Some("completed"),
                Some(&payload),
            );
        }
        AgentEvent::Error { message } => {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("failed"),
                Some("done"),
                None,
                Some("Agent execution failed"),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "error",
                message,
                Some("failed"),
                None,
            );
        }
        AgentEvent::AutoCompacted { evicted_count } => {
            let payload = serde_json::json!({ "evictedCount": evicted_count });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "status",
                "Conversation context compacted",
                Some("completed"),
                Some(&payload),
            );
        }
        AgentEvent::ApprovalRequested { request } => {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("waiting_approval"),
                Some("approval"),
                None,
                Some(&format!("Waiting for approval: {}", request.tool_name)),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            let payload = serde_json::to_value(request).unwrap_or_else(|_| serde_json::json!({}));
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "approval",
                &request.tool_name,
                Some("pending"),
                Some(&payload),
            );
        }
        AgentEvent::ApprovalResolved {
            request_id,
            decision,
        } => {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("running"),
                Some("tooling"),
                None,
                Some("Approval resolved"),
                None,
                None,
            );
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            let payload = serde_json::json!({
                "requestId": request_id,
                "decision": decision,
            });
            record_and_emit_agent_run_task_event(
                &task_event_ctx,
                run_event,
                "approval",
                "Approval resolved",
                Some("completed"),
                Some(&payload),
            );
        }
        AgentEvent::TextDelta { .. }
        | AgentEvent::StreamBlockDelta { .. }
        | AgentEvent::Thinking { .. }
        | AgentEvent::ToolCallPreparing { .. }
        | AgentEvent::ToolCallArgsDelta { .. }
        | AgentEvent::UsageUpdate { .. } => {}
    }
}
