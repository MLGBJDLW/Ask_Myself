use super::*;

pub async fn finalize_desktop_agent_turn(finalization: DesktopAgentTurnFinalization<'_>) {
    let DesktopAgentTurnFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        event_seq,
        outcome,
    } = finalization;

    let status_before_publication = db
        .get_agent_task_run(task_run_id)
        .ok()
        .map(|run| run.status);
    let resumably_suspended = matches!(
        status_before_publication.as_deref(),
        Some("awaiting_user_input" | "paused")
    ) || matches!(
        &outcome.result,
        Some(Err(CoreError::AwaitingUserInput { .. }))
    );
    let publication = if resumably_suspended {
        event_seq.flush().await.map(|_| ())
    } else {
        // Finalization submits its own outcome candidate. The outbox accepts
        // exactly one of this, the stream-forwarder candidate, or a concurrent
        // stop candidate, so no check/flush race can split ledger and snapshot.
        let terminal_candidate = match &outcome.result {
            Some(Ok(message)) => AgentRunEvent::terminal_status(
                task_run_id,
                Some(turn_id),
                0,
                &message.text_content(),
                "completed",
                Some(&serde_json::json!({ "reason": "turn_finalization" })),
            ),
            Some(Err(CoreError::Cancelled(message))) => AgentRunEvent::terminal_error(
                task_run_id,
                Some(turn_id),
                0,
                "Agent execution cancelled.",
                "cancelled",
                Some(&serde_json::json!({ "reason": message })),
            ),
            Some(Err(error)) => AgentRunEvent::terminal_error(
                task_run_id,
                Some(turn_id),
                0,
                "Agent execution failed unexpectedly.",
                "failed",
                Some(&serde_json::json!({ "reason": error.to_string() })),
            ),
            None => AgentRunEvent::terminal_error(
                task_run_id,
                Some(turn_id),
                0,
                "Agent execution timed out.",
                "timed_out",
                Some(&serde_json::json!({ "reason": "timeout" })),
            ),
        };
        match event_seq.submit(terminal_candidate) {
            Ok(()) | Err(AgentRunEventSubmitError::AlreadyClosed) => {}
            Err(
                error @ (AgentRunEventSubmitError::QueueFull
                | AgentRunEventSubmitError::ActorUnavailable),
            ) => {
                warn!("Terminal RunEvent submission failed closed for {task_run_id}: {error}");
            }
            Err(error) => {
                warn!("Terminal RunEvent candidate was rejected for {task_run_id}: {error}");
                emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
                repair_orphaned_tool_calls(db, conversation_id);
                return;
            }
        }
        event_seq.wait_for_terminal_commit().await.map(|_| ())
    };
    if let Err(error) = publication {
        warn!("Run Event outbox did not settle before finalizing {task_run_id}: {error}");
        reconcile_authoritative_run_event_outbox_failure(
            db,
            task_run_id,
            task_orchestrator_run_id,
            turn_id,
            &error,
        );
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        repair_orphaned_tool_calls(db, conversation_id);
        return;
    }

    let turn_snapshot = db.get_conversation_turn(turn_id).ok();
    let trace_artifacts = serde_json::json!({
        "turnId": turn_id,
        "turnStatus": turn_snapshot.as_ref().map(|turn| turn.status.clone()),
        "routeKind": turn_snapshot.as_ref().and_then(|turn| turn.route_kind.clone()),
        "trace": turn_snapshot.as_ref().and_then(|turn| turn.trace.clone()),
    });
    let previous_task_artifacts = db
        .get_agent_task_run(task_run_id)
        .ok()
        .and_then(|run| run.artifacts);
    let subtask_runs = db
        .list_agent_subtask_runs(task_run_id)
        .unwrap_or_else(|err| {
            warn!("Failed to load subtask runs for {task_run_id}: {err}");
            Vec::new()
        });
    let task_artifacts =
        build_final_task_artifacts(previous_task_artifacts, trace_artifacts, &subtask_runs);
    let verification_status = task_artifacts
        .get("verification")
        .and_then(|verification| verification.get("overallStatus"))
        .and_then(|status| status.as_str());
    let current_task_status = db
        .get_agent_task_run(task_run_id)
        .ok()
        .map(|run| run.status);
    if current_task_status.as_deref() == Some("awaiting_user_input")
        || matches!(
            &outcome.result,
            Some(Err(CoreError::AwaitingUserInput { .. }))
        )
    {
        if current_task_status.as_deref() == Some("awaiting_user_input") {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("awaiting_user_input"),
                Some("awaiting_user_input"),
                None,
                Some("Waiting for user input"),
                None,
                Some(&task_artifacts),
            );
        }
        // A fast response can re-queue this same durable run before the
        // suspended executor reaches finalization. Never let that stale
        // executor overwrite the resumed or explicitly cancelled state.
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        return;
    }
    if current_task_status.as_deref() == Some("paused") {
        let _ = db.update_agent_task_run_progress(
            task_run_id,
            Some("paused"),
            Some("paused"),
            None,
            Some("Paused with a resumable checkpoint"),
            None,
            Some(&task_artifacts),
        );
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        return;
    }
    let (task_status, task_summary, task_error): (&str, &str, Option<String>) =
        match current_task_status.as_deref() {
            Some("cancelled") => ("cancelled", "Stopped by user", None),
            Some("timed_out") => (
                "timed_out",
                "Agent execution timed out",
                Some("Agent execution timed out.".to_string()),
            ),
            Some("failed") => (
                "failed",
                "Agent execution failed",
                outcome
                    .result
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(ToString::to_string),
            ),
            Some("completed") => {
                if verification_status.is_some_and(|status| status != "passed") {
                    ("completed", "Task completed with verification gap", None)
                } else {
                    ("completed", "Task completed", None)
                }
            }
            _ if outcome.timed_out => (
                "timed_out",
                "Agent execution timed out",
                Some("Agent execution timed out.".to_string()),
            ),
            _ => match &outcome.result {
                Some(Err(CoreError::Cancelled(message))) => (
                    "cancelled",
                    "Agent execution cancelled",
                    Some(message.clone()),
                ),
                Some(Err(err)) => ("failed", "Agent execution failed", Some(err.to_string())),
                _ => match turn_snapshot.as_ref().map(|turn| turn.status.as_str()) {
                    Some("cancelled") => ("cancelled", "Stopped by user", None),
                    Some("error") => ("failed", "Agent execution failed", None),
                    Some("cached") => ("completed", "Answered from cache", None),
                    _ if verification_status.is_some_and(|status| status != "passed") => {
                        ("completed", "Task completed with verification gap", None)
                    }
                    _ => ("completed", "Task completed", None),
                },
            },
        };

    let _ = db.finish_agent_task_run(
        task_run_id,
        task_status,
        Some(task_summary),
        task_error.as_deref(),
        Some(&task_artifacts),
    );
    if let Some(run_id) = task_orchestrator_run_id {
        if let Err(err) =
            db.transition_workflow_automation_run(run_id, task_status, Some(task_summary))
        {
            warn!("Failed to transition Task Orchestrator run {run_id}: {err}");
        }
    }
    if task_status == "completed" {
        if let Some(Ok(message)) = &outcome.result {
            if let Err(error) = db.record_project_turn_completion(
                conversation_id,
                turn_id,
                task_run_id,
                &message.text_content(),
            ) {
                warn!(
                    "Failed to publish project workspace turn completion for conversation {conversation_id} turn {turn_id}: {error}"
                );
            }
        }
    }
    // The executor's Done/Error event is the canonical terminal event. Task
    // finalization updates the materialized snapshot only; appending a status
    // here would make a non-terminal event follow the terminal event.
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);

    if !matches!(&outcome.result, Some(Ok(_))) {
        repair_orphaned_tool_calls(db, conversation_id);
    }
}

pub async fn finalize_desktop_agent_stop(finalization: DesktopAgentStopFinalization<'_>) {
    let DesktopAgentStopFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        event_seq,
        reason,
        summary,
    } = finalization;
    let artifacts = serde_json::json!({ "reason": reason });

    let run_event = AgentRunEvent::terminal_status(
        task_run_id,
        Some(turn_id),
        0,
        summary,
        "cancelled",
        Some(&artifacts),
    );
    match event_seq.submit(run_event) {
        Ok(()) | Err(AgentRunEventSubmitError::AlreadyClosed) => {}
        Err(error) => {
            warn!("Failed to submit terminal stop RunEvent for {conversation_id}: {error}");
        }
    }
    if let Err(error) = event_seq.wait_for_terminal_commit().await {
        warn!("Run Event outbox did not durably stop {task_run_id}: {error}");
        reconcile_authoritative_run_event_outbox_failure(
            db,
            task_run_id,
            task_orchestrator_run_id,
            turn_id,
            &error,
        );
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        return;
    }
    let terminal_status = db
        .get_agent_task_run(task_run_id)
        .ok()
        .map(|run| run.status);
    if terminal_status.as_deref() == Some("cancelled") {
        let _ = db.finalize_conversation_turn(
            turn_id,
            "cancelled",
            None,
            Some(&serde_json::json!({ "reason": reason })),
        );
        let _ = db.finish_agent_task_run(
            task_run_id,
            "cancelled",
            Some(summary),
            None,
            Some(&artifacts),
        );
        if let Some(run_id) = task_orchestrator_run_id {
            if let Err(err) =
                db.transition_workflow_automation_run(run_id, "cancelled", Some(summary))
            {
                warn!("Failed to cancel Task Orchestrator run {run_id}: {err}");
            }
        }
    } else {
        warn!(
            "Stop lost terminal arbitration for {task_run_id}; preserving authoritative status {:?}",
            terminal_status
        );
    }
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
}

/// Reconcile host lifecycle projections after the outbox itself wins with a
/// fail-closed outcome. The task row owns the diagnosis; this helper only
/// fills any still-open turn and Task Orchestrator projections from it.
pub(crate) fn reconcile_authoritative_run_event_outbox_failure(
    db: &Database,
    task_run_id: &str,
    task_orchestrator_run_id: Option<&str>,
    turn_id: &str,
    failure: &AgentRunEventOutboxFailure,
) -> bool {
    let fallback_reason = failure.reason_code();
    let task = match db.get_agent_task_run(task_run_id) {
        Ok(task) => task,
        Err(error) => {
            warn!("Failed to load fail-closed task projection {task_run_id}: {error}");
            return false;
        }
    };

    let outbox_owned_failure = task.status == "failed"
        && task
            .error_message
            .as_deref()
            .is_some_and(|reason| reason.starts_with("run_event_"));
    if !outbox_owned_failure {
        warn!(
            "Outbox failure reconciliation found no outbox-owned failed task for {task_run_id}; preserving authoritative status {} and reason {:?}",
            task.status,
            task.error_message
        );
        return false;
    }

    let authoritative_summary = task
        .summary
        .as_deref()
        .unwrap_or("Run Event outbox failed closed");
    let authoritative_reason = task.error_message.as_deref().unwrap_or(fallback_reason);
    let trace = serde_json::json!({
        "runEventOutboxFailure": {
            "reason": authoritative_reason,
            "message": failure.to_string(),
        }
    });
    if let Err(error) = db.finalize_conversation_turn(turn_id, "error", None, Some(&trace)) {
        warn!("Failed to finalize fail-closed conversation turn {turn_id}: {error}");
    }
    if let Some(run_id) = task_orchestrator_run_id {
        if let Err(error) =
            db.transition_workflow_automation_run(run_id, "failed", Some(authoritative_summary))
        {
            warn!("Failed to reconcile Task Orchestrator run {run_id}: {error}");
        }
    }
    true
}

pub(crate) fn build_final_task_artifacts(
    previous_artifacts: Option<serde_json::Value>,
    trace_artifacts: serde_json::Value,
    subtask_runs: &[AgentSubtaskRun],
) -> serde_json::Value {
    let mut merged = match previous_artifacts {
        Some(serde_json::Value::Object(map)) => map,
        Some(previous) => {
            let mut map = serde_json::Map::new();
            map.insert("previous".to_string(), previous);
            map
        }
        None => serde_json::Map::new(),
    };
    merged.insert(
        "kind".to_string(),
        serde_json::Value::String("agentTaskArtifacts".to_string()),
    );
    merged.insert(
        "version".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    merged.insert("trace".to_string(), trace_artifacts);
    merged.insert(
        "subtasks".to_string(),
        serde_json::to_value(subtask_runs).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
    );
    serde_json::Value::Object(merged)
}

pub(crate) fn repair_orphaned_tool_calls(db: &Database, conversation_id: &str) {
    let msgs = match db.get_messages(conversation_id) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to load messages for orphan repair: {e}");
            return;
        }
    };

    let mut i = 0;
    while i < msgs.len() {
        if msgs[i].role == Role::Assistant && !msgs[i].tool_calls.is_empty() {
            let mut found_ids = std::collections::HashSet::new();
            let mut j = i + 1;
            while j < msgs.len() && msgs[j].role == Role::Tool {
                if let Some(ref tc_id) = msgs[j].tool_call_id {
                    found_ids.insert(tc_id.as_str());
                }
                j += 1;
            }

            let base_sort = if j > i + 1 {
                msgs[j - 1].sort_order
            } else {
                msgs[i].sort_order
            };

            let mut extra_sort = 1;
            for tc in &msgs[i].tool_calls {
                if !found_ids.contains(tc.id.as_str()) {
                    warn!(
                        "Inserting synthetic error response for orphaned tool_call {}",
                        tc.id
                    );
                    let synthetic = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.to_string(),
                        role: Role::Tool,
                        content: format!(
                            "Error: tool '{}' was interrupted before completing (agent timeout or cancellation).",
                            tc.name
                        ),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                        artifacts: None,
                        token_count: 20,
                        created_at: String::new(),
                        sort_order: base_sort + extra_sort,
                        thinking: None,
                        image_attachments: None,
                    };
                    if let Err(e) = db.add_message(&synthetic) {
                        warn!("Failed to insert synthetic tool response: {e}");
                    }
                    extra_sort += 1;
                }
            }
        }
        i += 1;
    }
}
