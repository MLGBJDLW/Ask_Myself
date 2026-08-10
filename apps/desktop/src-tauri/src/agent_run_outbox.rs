//! Single-owner ordered delivery for the public RunEvent protocol.

use std::time::Duration;

use log::error;
use nexa_core::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunEventPersistence};
use nexa_core::conversation::AgentTaskRun;
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::error::CoreError;
use nexa_core::runtime::{AgentRunEventOutbox, AgentRunEventOutboxFailureHandle};
use nexa_core::task_run::AgentTaskRuntime;
use tauri::AppHandle;

use crate::agent_stream::emit_agent_run_frontend_event;
use crate::agent_task_events::emit_agent_task_run_snapshot;

const LIVE_JOURNAL_FLUSH_INTERVAL_MS: u64 = 100;
const LIVE_JOURNAL_MAX_BATCH: usize = 32;

pub(crate) fn spawn_agent_run_outbox(
    app_handle: AppHandle,
    db_executor: DatabaseExecutor,
    conversation_id: String,
    task_run_id: String,
    initial_sequence: u64,
) -> AgentRunEventOutbox {
    let (outbox, mut receiver) = AgentRunEventOutbox::channel();
    let failure_handle = outbox.failure_handle();
    tauri::async_runtime::spawn(async move {
        let mut sequence = initial_sequence;
        let mut last_turn_id = String::new();
        let mut pending_live_events = Vec::with_capacity(LIVE_JOURNAL_MAX_BATCH);
        let mut flush_tick =
            tokio::time::interval(Duration::from_millis(LIVE_JOURNAL_FLUSH_INTERVAL_MS));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        flush_tick.tick().await;

        loop {
            tokio::select! {
                maybe_event = receiver.recv() => {
                    let Some(mut event) = maybe_event else {
                        if let Err(error) = persist_run_event_batch(
                            &db_executor,
                            &task_run_id,
                            &mut pending_live_events,
                        ).await {
                            stop_after_persistence_failure(
                                &failure_handle,
                                &app_handle,
                                &conversation_id,
                                &task_run_id,
                                &last_turn_id,
                                sequence.saturating_add(1),
                                &error,
                            );
                        }
                        break;
                    };

                    sequence = sequence.saturating_add(1);
                    event.event_seq = sequence;
                    last_turn_id = event.turn_id.clone();
                    if let Err(contract_error) = event.validate_durable_contract() {
                        error!("Rejecting invalid RunEvent {task_run_id}#{sequence}: {contract_error}");
                        failure_handle.fail_closed();
                        emit_fail_closed_terminal(
                            &app_handle,
                            &conversation_id,
                            &task_run_id,
                            &event.turn_id,
                            sequence,
                        );
                        break;
                    }

                    if !event.is_durable() {
                        emit_agent_run_frontend_event(&app_handle, &conversation_id, &event);
                        if event.is_terminal() {
                            break;
                        }
                        continue;
                    }

                    if is_live_projection_event(&event) {
                        // Live deltas are projected immediately. Their ordered durable
                        // journal is committed in bounded transactions on the writer lane.
                        emit_agent_run_frontend_event(&app_handle, &conversation_id, &event);
                        pending_live_events.push(event);
                        if pending_live_events.len() >= LIVE_JOURNAL_MAX_BATCH {
                            if let Err(error) = persist_run_event_batch(
                                &db_executor,
                                &task_run_id,
                                &mut pending_live_events,
                            ).await {
                                stop_after_persistence_failure(
                                    &failure_handle,
                                    &app_handle,
                                    &conversation_id,
                                    &task_run_id,
                                    &last_turn_id,
                                    sequence.saturating_add(1),
                                    &error,
                                );
                                break;
                            }
                        }
                        continue;
                    }

                    let terminal = event.is_terminal();
                    pending_live_events.push(event.clone());
                    match persist_run_event_batch(
                        &db_executor,
                        &task_run_id,
                        &mut pending_live_events,
                    ).await {
                        Ok(snapshots) => {
                            emit_agent_run_frontend_event(&app_handle, &conversation_id, &event);
                            for snapshot in snapshots {
                                emit_agent_task_run_snapshot(
                                    &app_handle,
                                    &conversation_id,
                                    snapshot,
                                );
                            }
                        }
                        Err(error) => {
                            stop_after_persistence_failure(
                                &failure_handle,
                                &app_handle,
                                &conversation_id,
                                &task_run_id,
                                &event.turn_id,
                                sequence,
                                &error,
                            );
                            break;
                        }
                    }
                    if terminal {
                        break;
                    }
                }
                _ = flush_tick.tick(), if !pending_live_events.is_empty() => {
                    if let Err(error) = persist_run_event_batch(
                        &db_executor,
                        &task_run_id,
                        &mut pending_live_events,
                    ).await {
                        stop_after_persistence_failure(
                            &failure_handle,
                            &app_handle,
                            &conversation_id,
                            &task_run_id,
                            &last_turn_id,
                            sequence.saturating_add(1),
                            &error,
                        );
                        break;
                    }
                }
            }
        }
    });
    outbox
}

fn is_live_projection_event(event: &AgentRunEvent) -> bool {
    matches!(
        event.kind,
        AgentRunEventKind::OutputDelta
            | AgentRunEventKind::Thinking
            | AgentRunEventKind::UsageUpdated
    )
}

async fn persist_run_event_batch(
    db_executor: &DatabaseExecutor,
    task_run_id: &str,
    pending: &mut Vec<AgentRunEvent>,
) -> Result<Vec<AgentTaskRun>, CoreError> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let events = std::mem::take(pending);
    let durable_run_id = task_run_id.to_string();
    db_executor
        .write(move |database| {
            database.save_agent_run_events(&events)?;
            let runtime = AgentTaskRuntime::new(database);
            let mut snapshots = Vec::new();
            for event in &events {
                if !is_live_projection_event(event) {
                    snapshots.push(runtime.apply_run_event(&durable_run_id, event)?);
                }
            }
            Ok(snapshots)
        })
        .await
        .map(|execution| execution.value)
}

fn stop_after_persistence_failure(
    failure_handle: &AgentRunEventOutboxFailureHandle,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: &str,
    terminal_sequence: u64,
    error: &CoreError,
) {
    error!("Stopping RunEvent journal after persistence failure for {task_run_id}: {error}");
    failure_handle.fail_closed();
    emit_fail_closed_terminal(
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        terminal_sequence,
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_high_frequency_projection_events_use_deferred_journaling() {
        let output = AgentRunEvent::output_delta(
            "run-1",
            Some("turn-1"),
            1,
            "block-1",
            nexa_core::agent::StreamBlockChannel::Answer,
            0,
            "hello",
        );
        let status = AgentRunEvent::status_update(
            "run-1",
            Some("turn-1"),
            2,
            nexa_core::agent_run::AgentRunPhase::Routing,
            "routing",
            Some("running"),
            None,
        );

        assert!(is_live_projection_event(&output));
        assert!(!is_live_projection_event(&status));
    }
}
