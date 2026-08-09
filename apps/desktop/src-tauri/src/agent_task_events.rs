use log::warn;
use nexa_core::agent_run::{
    AgentRunDisplayKind, AgentRunEvent, AgentRunEventImportance, AgentRunEventVisibility,
    AgentRunPhase,
};
use nexa_core::conversation::AgentTaskRun;
use nexa_core::db::Database;
use nexa_core::runtime::AgentRunEventOutbox;
use serde::Serialize;
use tauri::AppHandle;

use crate::app_events::{emit_main_window_event, emit_window_event};

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
        Ok(task_run) => emit_agent_task_run_snapshot(app_handle, conversation_id, task_run),
        Err(err) => warn!("Failed to load task run {task_run_id} for event: {err}"),
    }
}

pub(crate) fn emit_agent_task_run_snapshot(
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run: AgentTaskRun,
) {
    let task_run_id = task_run.id.clone();
    let payload = AgentTaskRunUpdatedEvent {
        conversation_id: conversation_id.to_string(),
        event_type: "taskRunUpdated",
        task_run,
    };
    emit_main_window_event(app_handle, "agent://task-snapshot", &payload);
    emit_window_event(
        app_handle,
        "companion",
        "companion://projection-changed",
        &serde_json::json!({ "runId": task_run_id }),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_internal_agent_run_status_event(
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event_outbox: &AgentRunEventOutbox,
    phase: AgentRunPhase,
    label: &str,
    status: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    let run_event =
        AgentRunEvent::status_update(task_run_id, turn_id, 0, phase, label, status, payload)
            .with_presentation(
                AgentRunEventVisibility::Internal,
                AgentRunDisplayKind::Status,
                AgentRunEventImportance::Low,
            );
    if let Err(error) = event_outbox.submit(run_event) {
        warn!("Failed to submit internal RunEvent for {conversation_id}: {error}");
    }
}
