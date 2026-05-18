use std::sync::atomic::{AtomicU64, Ordering};

use log::warn;
use nexa_core::agent_run::{AgentRunEvent, AgentRunPhase};
use nexa_core::conversation::{AgentTaskRun, AgentTaskRunEvent};
use nexa_core::db::Database;
use nexa_core::task_run::AgentTaskRuntime;
use serde::Serialize;
use tauri::AppHandle;

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
    let payload = run_event.task_event_payload(payload);
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

pub(crate) fn record_agent_run_status_task_event(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event_seq: &AtomicU64,
    phase: AgentRunPhase,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    let next_seq = event_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let run_event = AgentRunEvent::status_update(
        task_run_id,
        turn_id,
        next_seq,
        phase,
        label,
        status,
        payload,
    );
    record_agent_run_task_event(
        db,
        app_handle,
        conversation_id,
        task_run_id,
        &run_event,
        run_event.task_event_type(),
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
    run_event: &AgentRunEvent,
) {
    match AgentTaskRuntime::new(db).apply_run_event(task_run_id, run_event) {
        Ok(task_event) => {
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
            emit_agent_task_event(app_handle, conversation_id, task_event);
        }
        Err(err) => warn!("Failed to apply task event for {task_run_id}: {err}"),
    }
}
