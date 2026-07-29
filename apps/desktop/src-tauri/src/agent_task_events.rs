use log::warn;
use nexa_core::agent_run::{
    AgentRunDisplayKind, AgentRunEvent, AgentRunEventImportance, AgentRunEventVisibility,
    AgentRunPhase,
};
use nexa_core::conversation::AgentTaskRun;
use nexa_core::db::Database;
use nexa_core::runtime::AgentRunEventSequencer;
use nexa_core::task_run::AgentTaskRuntime;
use serde::Serialize;
use tauri::AppHandle;

use crate::agent_stream::emit_agent_run_frontend_event;
use crate::app_events::emit_app_event;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskRunUpdatedEvent {
    conversation_id: String,
    #[serde(rename = "type")]
    event_type: &'static str,
    task_run: AgentTaskRun,
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

pub(crate) fn persist_durable_run_event(db: &Database, run_event: &AgentRunEvent) {
    if !run_event.is_durable() {
        return;
    }
    if let Err(err) = db.save_agent_run_event(run_event) {
        warn!(
            "Failed to persist durable run event {}#{}: {err}",
            run_event.run_id, run_event.event_seq
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_internal_agent_run_status_event(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event_seq: &AgentRunEventSequencer,
    phase: AgentRunPhase,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    let next_seq = event_seq.next();
    let run_event = AgentRunEvent::status_update(
        task_run_id,
        turn_id,
        next_seq,
        phase,
        label,
        status,
        payload,
    )
    .with_presentation(
        AgentRunEventVisibility::Internal,
        AgentRunDisplayKind::Status,
        AgentRunEventImportance::Low,
    );
    emit_agent_run_frontend_event(app_handle, conversation_id, &run_event);
    record_task_progress_for_agent_event(db, app_handle, conversation_id, task_run_id, &run_event);
}

pub(crate) fn record_task_progress_for_agent_event(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    run_event: &AgentRunEvent,
) {
    persist_durable_run_event(db, run_event);
    match AgentTaskRuntime::new(db).apply_run_event(task_run_id, run_event) {
        Ok(_) => {
            emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        }
        Err(err) => warn!("Failed to apply task event for {task_run_id}: {err}"),
    }
}
