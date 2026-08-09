//! Single-owner ordered delivery for the public RunEvent protocol.

use log::{error, warn};
use nexa_core::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunEventPersistence};
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::runtime::AgentRunEventOutbox;
use nexa_core::task_run::AgentTaskRuntime;
use tauri::AppHandle;

use crate::agent_stream::emit_agent_run_frontend_event;
use crate::agent_task_events::emit_agent_task_run_snapshot;

pub(crate) fn spawn_agent_run_outbox(
    app_handle: AppHandle,
    db_executor: DatabaseExecutor,
    conversation_id: String,
    task_run_id: String,
    initial_sequence: u64,
) -> AgentRunEventOutbox {
    let (outbox, mut receiver) = AgentRunEventOutbox::channel();
    tauri::async_runtime::spawn(async move {
        let mut sequence = initial_sequence;
        let mut terminal_delivered = false;

        while let Some(mut event) = receiver.recv().await {
            if terminal_delivered {
                warn!("Discarding RunEvent after terminal event for {task_run_id}");
                continue;
            }

            sequence = sequence.saturating_add(1);
            event.event_seq = sequence;
            if let Err(contract_error) = event.validate_durable_contract() {
                error!("Rejecting invalid RunEvent {task_run_id}#{sequence}: {contract_error}");
                emit_fail_closed_terminal(
                    &app_handle,
                    &conversation_id,
                    &task_run_id,
                    &event.turn_id,
                    sequence,
                );
                break;
            }

            let updates_task_projection = !matches!(
                event.kind,
                AgentRunEventKind::OutputDelta
                    | AgentRunEventKind::Thinking
                    | AgentRunEventKind::UsageUpdated
            );
            let snapshot = if event.is_durable() {
                let durable_event = event.clone();
                let durable_run_id = task_run_id.clone();
                match db_executor
                    .write(move |database| {
                        database.save_agent_run_event(&durable_event)?;
                        if updates_task_projection {
                            AgentTaskRuntime::new(database)
                                .apply_run_event(&durable_run_id, &durable_event)
                                .map(Some)
                        } else {
                            Ok(None)
                        }
                    })
                    .await
                {
                    Ok(execution) => execution.value,
                    Err(persist_error) => {
                        error!(
                            "Stopping RunEvent outbox after persistence failure for {task_run_id}#{sequence}: {persist_error}"
                        );
                        emit_fail_closed_terminal(
                            &app_handle,
                            &conversation_id,
                            &task_run_id,
                            &event.turn_id,
                            sequence,
                        );
                        break;
                    }
                }
            } else {
                None
            };

            emit_agent_run_frontend_event(&app_handle, &conversation_id, &event);
            if let Some(task_run) = snapshot {
                emit_agent_task_run_snapshot(&app_handle, &conversation_id, task_run);
            }
            terminal_delivered = event.is_terminal();
        }
    });
    outbox
}

fn emit_fail_closed_terminal(
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: &str,
    sequence: u64,
) {
    let mut terminal = AgentRunEvent::terminal_error(
        task_run_id,
        Some(turn_id),
        sequence,
        "The response stream could not be stored safely. Retry this message.",
        "failed",
        Some(&serde_json::json!({ "reason": "run_event_persistence_failed" })),
    );
    terminal.persistence = AgentRunEventPersistence::Ephemeral;
    emit_agent_run_frontend_event(app_handle, conversation_id, &terminal);
}
