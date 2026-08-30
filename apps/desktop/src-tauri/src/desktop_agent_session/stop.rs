use super::*;

pub fn execution_mode_artifact(execution_mode: AgentExecutionMode) -> serde_json::Value {
    serde_json::json!({
        "kind": "executionMode",
        "version": 1,
        "mode": execution_mode.as_str(),
    })
}

pub fn power_mode_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentPowerMode",
        "version": 1,
        "mode": config.power_mode.as_str(),
        "policy": {
            "orchestration": if config.power_mode.is_nexus() { "proactiveParallelSubagents" } else { "standard" },
            "reasoningEnabled": config.reasoning_enabled,
            "reasoningEffort": config.reasoning_effort.as_ref().map(ToString::to_string),
            "thinkingBudget": config.thinking_budget,
            "maxParallel": config.subagent_max_parallel,
            "maxCallsPerTurn": config.subagent_max_calls_per_turn,
            "delegatedTokenBudget": config.subagent_token_budget,
            "verificationReservePercent": config.subagent_verification_reserve_percent,
        },
    })
}

pub(crate) fn automatic_delegated_worker_cap(total: u32, parallel: u32) -> u64 {
    u64::from(total)
        .div_ceil(u64::from(parallel.max(1)))
        .saturating_mul(2)
        .max(8_192)
        .min(u64::from(total))
}

pub fn collaboration_mode_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentCollaborationMode",
        "version": 1,
        "mode": config.collaboration_mode.as_str(),
        "preset": config.moa_preset.as_str(),
        "contract": {
            "advisorsReceiveTools": false,
            "aggregatorRetainsTools": true,
            "privateCacheSafeTail": true,
            "independentFromNexus": true,
        },
    })
}

pub fn orchestration_profile_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "orchestrationProfile",
        "version": 1,
        "profile": config.orchestration_profile.as_str(),
        "providerReasoningEffort": config.reasoning_effort.as_ref().map(ToString::to_string),
        "policy": {
            "maxIterations": config.max_iterations,
            "maxParallel": config.subagent_max_parallel,
            "maxCallsPerTurn": config.subagent_max_calls_per_turn,
            "delegatedTokenBudget": config.subagent_token_budget,
            "verificationReservePercent": config.subagent_verification_reserve_percent,
        },
    })
}

pub async fn request_desktop_running_agent_stop(
    task_state: nexa_core::runtime::ActiveAgentTurn,
    request: DesktopRunningAgentStopRequest,
) -> Result<(), DesktopRunningAgentStopError> {
    let DesktopRunningAgentStopRequest {
        db,
        app_handle,
        conversation_id,
        pending_approvals,
    } = request;
    let task_run_id = task_state.handle.run_id.clone();
    let task_orchestrator_run_id = task_state.orchestrator_run_id.clone();
    let turn_id = task_state.handle.turn_id.clone();
    if let Err(error) =
        fence_and_checkpoint_desktop_agent_turn(task_state, db.as_ref(), &pending_approvals).await
    {
        let failed_closed = reconcile_authoritative_run_event_outbox_failure(
            &db,
            &task_run_id,
            task_orchestrator_run_id.as_deref(),
            &turn_id,
            &error,
        );
        if !failed_closed {
            let _ = nexa_core::task_run::AgentTaskRuntime::new(db.as_ref())
                .fail_pre_executor_launch_if_open(&task_run_id, error.reason_code());
            let _ = reconcile_authoritative_run_event_outbox_failure(
                &db,
                &task_run_id,
                task_orchestrator_run_id.as_deref(),
                &turn_id,
                &error,
            );
        }
        emit_agent_task_run_update(&db, &app_handle, &conversation_id, &task_run_id);
        return Err(DesktopRunningAgentStopError {
            message: format!("Could not preserve a resumable stop: {error}"),
        });
    }

    emit_agent_task_run_update(&db, &app_handle, &conversation_id, &task_run_id);
    Ok(())
}

pub(crate) async fn fence_and_checkpoint_desktop_agent_turn(
    task_state: nexa_core::runtime::ActiveAgentTurn,
    db: &Database,
    pending_approvals: &PendingToolApprovals,
) -> Result<(), AgentRunEventOutboxFailure> {
    let task_run_id = task_state.handle.run_id.clone();
    let turn_id = task_state.handle.turn_id.clone();
    let event_outbox = Arc::clone(&task_state.event_outbox);

    // Establish the execution boundary first. No model/tool future remains
    // alive while the outbox drains and commits the resumable checkpoint.
    task_state.cancel_token.cancel();
    task_state.task.abort();
    let _ = task_state.task.await;

    resolve_desktop_pending_approvals_for_stopped_run(
        db,
        event_outbox.as_ref(),
        &task_run_id,
        &turn_id,
        pending_approvals,
    )
    .await?;

    let action_receipts = match nexa_core::activity::ActivityRuntime::with_database(db.clone()) {
        Ok(runtime) => action_receipts_requiring_reconciliation(&runtime, &turn_id).await,
        Err(error) => {
            warn!("Could not read action receipts while stopping; forcing reconciliation: {error}");
            vec!["activity_registry_unavailable".to_string()]
        }
    };
    let checkpoint_reason = if action_receipts.is_empty() {
        "user_stop".to_string()
    } else {
        format!(
            "user_stop_requires_action_reconciliation:{}",
            action_receipts.join(",")
        )
    };

    event_outbox
        .pause_with_checkpoint(&turn_id, &checkpoint_reason)
        .await
        .map(|_| ())
}

async fn action_receipts_requiring_reconciliation(
    runtime: &nexa_core::activity::ActivityRuntime,
    turn_id: &str,
) -> Vec<String> {
    let records = runtime
        .list()
        .into_iter()
        .filter(|record| {
            record.turn_id.as_deref() == Some(turn_id)
                && matches!(
                    record.surface,
                    nexa_core::activity::ActivitySurface::Desktop
                        | nexa_core::activity::ActivitySurface::Browser
                )
                && matches!(
                    record.owner_tool.as_str(),
                    "computer_control" | "browser_session"
                )
        })
        .collect::<Vec<_>>();

    // A trusted terminal close receipt proves that this browser session has
    // reached a state with no observable page. Earlier page mutations from the
    // same turn are therefore subsumed by that close attempt; carrying any of
    // them across Stop/Resume would create a fence that a removed or retained
    // empty session can never clear. Later receipts, every other browser
    // session, and the desktop surface remain fail-closed.
    let mut terminal_browser_sessions = HashMap::new();
    for record in records.iter().filter(|record| {
        record.owner_tool == "browser_session"
            && matches!(
                record.state,
                nexa_core::activity::ActivityState::Completed
                    | nexa_core::activity::ActivityState::Failed
            )
    }) {
        match runtime
            .observe(
                &record.activity_id,
                record.last_event_seq.saturating_sub(1),
                Duration::ZERO,
            )
            .await
        {
            Ok(observation) if browser_terminal_boundary_is_known(&observation) => {
                if let (Some(session_id), Some(terminal_at)) =
                    (&record.session_id, record.completed_at)
                {
                    let replace = terminal_browser_sessions
                        .get(session_id)
                        .is_none_or(|(known_terminal_at, _)| terminal_at > *known_terminal_at);
                    if replace {
                        terminal_browser_sessions.insert(
                            session_id.clone(),
                            (terminal_at, record.activity_id.clone()),
                        );
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    "Could not inspect terminal browser cleanup receipt {}; preserving the reconciliation fence: {error}",
                    record.activity_id
                );
            }
        }
    }

    records
        .into_iter()
        .filter(|record| {
            if record.owner_tool != "browser_session" {
                return true;
            }
            let Some((terminal_at, terminal_activity_id)) = record
                .session_id
                .as_ref()
                .and_then(|session_id| terminal_browser_sessions.get(session_id))
            else {
                return true;
            };
            if record.activity_id == *terminal_activity_id {
                return false;
            }
            !record
                .completed_at
                .is_some_and(|completed_at| completed_at <= *terminal_at)
        })
        .map(|record| record.activity_id)
        .collect()
}

fn browser_terminal_boundary_is_known(
    observation: &nexa_core::activity::ActivityObservation,
) -> bool {
    if observation.record.surface != nexa_core::activity::ActivitySurface::Browser
        || observation.record.owner_tool != "browser_session"
    {
        return false;
    }
    let Some(session_id) = observation.record.session_id.as_deref() else {
        return false;
    };
    let Some(detail) = observation
        .events
        .last()
        .and_then(|event| event.payload.get("detail"))
    else {
        return false;
    };
    if detail
        .get("browserSessionId")
        .and_then(serde_json::Value::as_str)
        != Some(session_id)
    {
        return false;
    }

    match observation.record.state {
        nexa_core::activity::ActivityState::Failed => {
            detail.get("stage").and_then(serde_json::Value::as_str) == Some("cleanup_pending")
                && detail.get("action").and_then(serde_json::Value::as_str) == Some("close_session")
                && detail
                    .get("sessionRetainedForRetry")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }
        nexa_core::activity::ActivityState::Completed => {
            if detail.get("stage").and_then(serde_json::Value::as_str) != Some("observed") {
                return false;
            }
            match detail.get("action").and_then(serde_json::Value::as_str) {
                Some("close_session") => {
                    detail
                        .get("sessionClosed")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                }
                Some("close_tab") => {
                    detail
                        .get("tabId")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|tab_id| !tab_id.is_empty())
                        && detail.get("tabClosed").and_then(serde_json::Value::as_bool)
                            == Some(true)
                        && detail
                            .get("remainingTabCount")
                            .and_then(serde_json::Value::as_u64)
                            == Some(0)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub(crate) async fn resolve_desktop_pending_approvals_for_stopped_run(
    db: &Database,
    event_outbox: &AgentRunEventOutbox,
    task_run_id: &str,
    turn_id: &str,
    pending_approvals: &PendingToolApprovals,
) -> Result<(), AgentRunEventOutboxFailure> {
    // Remove the in-memory senders before any fallible persistence work. The
    // executor is already fenced, so every drained prompt is terminally denied.
    let registered = {
        let mut pending = pending_approvals.lock().await;
        let request_ids = pending
            .iter()
            .filter_map(|(request_id, approval)| {
                (approval.task_run_id == task_run_id).then(|| request_id.clone())
            })
            .collect::<Vec<_>>();
        for request_id in &request_ids {
            if let Some(approval) = pending.remove(request_id) {
                let _ = approval.sender.send(ApprovalDecision::Deny);
            }
        }
        request_ids
    };

    event_outbox.flush().await?;
    let durable_events = db.list_agent_run_events(task_run_id).map_err(|error| {
        AgentRunEventOutboxFailure::Persistence {
            message: error.to_string(),
        }
    })?;
    let (mut unresolved, resolved) = approval_resolution_state(&durable_events);
    unresolved.extend(
        registered
            .into_iter()
            .filter(|request_id| !resolved.contains(request_id)),
    );

    let mut unresolved = unresolved.into_iter().collect::<Vec<_>>();
    unresolved.sort();
    for request_id in unresolved {
        event_outbox
            .submit(
                AgentRunEvent::from_agent_event(&AgentEvent::ApprovalResolved {
                    request_id,
                    decision: ApprovalDecision::Deny,
                })
                .with_context(Some(task_run_id), Some(turn_id), None),
            )
            .map_err(submit_error_as_outbox_failure)?;
    }
    Ok(())
}

pub(crate) fn approval_resolution_state(
    events: &[AgentRunEvent],
) -> (HashSet<String>, HashSet<String>) {
    let mut unresolved = HashSet::new();
    let mut resolved = HashSet::new();
    for event in events {
        match event.kind {
            AgentRunEventKind::ApprovalRequested => {
                if let Some(request_id) = event
                    .payload
                    .get("request")
                    .and_then(|request| request.get("id"))
                    .and_then(serde_json::Value::as_str)
                {
                    if !resolved.contains(request_id) {
                        unresolved.insert(request_id.to_string());
                    }
                }
            }
            AgentRunEventKind::ApprovalResolved => {
                if let Some(request_id) = event
                    .payload
                    .get("requestId")
                    .and_then(serde_json::Value::as_str)
                {
                    unresolved.remove(request_id);
                    resolved.insert(request_id.to_string());
                }
            }
            _ => {}
        }
    }
    (unresolved, resolved)
}

pub(crate) fn submit_error_as_outbox_failure(
    error: AgentRunEventSubmitError,
) -> AgentRunEventOutboxFailure {
    match error {
        AgentRunEventSubmitError::QueueFull => AgentRunEventOutboxFailure::QueueFull,
        other => AgentRunEventOutboxFailure::Persistence {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod cleanup_receipt_tests {
    use super::{action_receipts_requiring_reconciliation, browser_terminal_boundary_is_known};
    use std::collections::HashSet;
    use std::time::Duration;

    #[tokio::test]
    async fn persisted_cleanup_pending_detail_does_not_require_input_reconciliation() {
        let db = nexa_core::db::Database::open_memory().unwrap();
        let runtime = nexa_core::activity::ActivityRuntime::with_database(db).unwrap();
        let record = runtime
            .start(
                nexa_core::activity::ActivitySpec::new(
                    nexa_core::activity::ActivitySurface::Browser,
                    "browser_session",
                )
                .with_activity_id("browser_action:turn:cleanup-call:token")
                .with_session_id("browser-session-1"),
            )
            .unwrap();
        runtime
            .transition(
                &record.activity_id,
                nexa_core::activity::ActivityState::Failed,
                serde_json::json!({
                    "stage": "cleanup_pending",
                    "action": "close_session",
                    "browserSessionId": "browser-session-1",
                    "sessionRetainedForRetry": true,
                }),
            )
            .unwrap();

        let observation = runtime
            .observe(&record.activity_id, 0, Duration::ZERO)
            .await
            .unwrap();

        assert!(browser_terminal_boundary_is_known(&observation));
        assert_eq!(
            observation.events.last().unwrap().payload["detail"]["stage"],
            "cleanup_pending"
        );
    }

    #[tokio::test]
    async fn cleanup_pending_subsumes_only_earlier_receipts_from_the_same_browser_session() {
        let db = nexa_core::db::Database::open_memory().unwrap();
        let runtime = nexa_core::activity::ActivityRuntime::with_database(db).unwrap();
        let turn_id = "turn-with-terminal-browser-cleanup";

        for (activity_id, session_id) in [
            ("browser_action:turn:navigate:same", "browser-session-1"),
            ("browser_action:turn:navigate:other", "browser-session-2"),
        ] {
            let record = runtime
                .start(
                    nexa_core::activity::ActivitySpec::new(
                        nexa_core::activity::ActivitySurface::Browser,
                        "browser_session",
                    )
                    .with_activity_id(activity_id)
                    .with_session_id(session_id)
                    .with_turn_id(turn_id),
                )
                .unwrap();
            runtime
                .transition(
                    &record.activity_id,
                    nexa_core::activity::ActivityState::Completed,
                    serde_json::json!({ "stage": "action_completed" }),
                )
                .unwrap();
        }

        let cleanup = runtime
            .start(
                nexa_core::activity::ActivitySpec::new(
                    nexa_core::activity::ActivitySurface::Browser,
                    "browser_session",
                )
                .with_activity_id("browser_action:turn:cleanup:same")
                .with_session_id("browser-session-1")
                .with_turn_id(turn_id),
            )
            .unwrap();
        runtime
            .transition(
                &cleanup.activity_id,
                nexa_core::activity::ActivityState::Failed,
                serde_json::json!({
                    "stage": "cleanup_pending",
                    "action": "close_session",
                    "browserSessionId": "browser-session-1",
                    "sessionRetainedForRetry": true,
                }),
            )
            .unwrap();

        let later_same_session = runtime
            .start(
                nexa_core::activity::ActivitySpec::new(
                    nexa_core::activity::ActivitySurface::Browser,
                    "browser_session",
                )
                .with_activity_id("browser_action:turn:navigate:same-after-cleanup")
                .with_session_id("browser-session-1")
                .with_turn_id(turn_id),
            )
            .unwrap();
        runtime
            .transition(
                &later_same_session.activity_id,
                nexa_core::activity::ActivityState::Completed,
                serde_json::json!({ "stage": "action_completed" }),
            )
            .unwrap();

        let receipts = action_receipts_requiring_reconciliation(&runtime, turn_id)
            .await
            .into_iter()
            .collect::<HashSet<_>>();

        assert!(!receipts.contains("browser_action:turn:navigate:same"));
        assert!(!receipts.contains("browser_action:turn:cleanup:same"));
        assert_eq!(
            receipts,
            HashSet::from([
                "browser_action:turn:navigate:other".to_string(),
                "browser_action:turn:navigate:same-after-cleanup".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn successful_terminal_close_receipts_do_not_create_an_unobservable_resume_fence() {
        let db = nexa_core::db::Database::open_memory().unwrap();
        let runtime = nexa_core::activity::ActivityRuntime::with_database(db).unwrap();
        let turn_id = "turn-with-successful-terminal-closes";

        for (activity_id, session_id, detail) in [
            (
                "browser_action:turn:close-session:terminal",
                "browser-session-closed",
                serde_json::json!({
                    "stage": "observed",
                    "action": "close_session",
                    "browserSessionId": "browser-session-closed",
                    "sessionClosed": true,
                }),
            ),
            (
                "browser_action:turn:close-tab:terminal",
                "browser-session-empty",
                serde_json::json!({
                    "stage": "observed",
                    "action": "close_tab",
                    "browserSessionId": "browser-session-empty",
                    "tabId": "tab-final",
                    "tabClosed": true,
                    "remainingTabCount": 0,
                }),
            ),
            (
                "browser_action:turn:close-tab:nonfinal",
                "browser-session-nonfinal",
                serde_json::json!({
                    "stage": "observed",
                    "action": "close_tab",
                    "browserSessionId": "browser-session-nonfinal",
                    "tabId": "tab-one",
                    "tabClosed": true,
                    "remainingTabCount": 1,
                }),
            ),
            (
                "browser_action:turn:close-session:wrong-scope",
                "browser-session-expected",
                serde_json::json!({
                    "stage": "observed",
                    "action": "close_session",
                    "browserSessionId": "browser-session-other",
                    "sessionClosed": true,
                }),
            ),
        ] {
            let record = runtime
                .start(
                    nexa_core::activity::ActivitySpec::new(
                        nexa_core::activity::ActivitySurface::Browser,
                        "browser_session",
                    )
                    .with_activity_id(activity_id)
                    .with_session_id(session_id)
                    .with_turn_id(turn_id),
                )
                .unwrap();
            runtime
                .transition(
                    &record.activity_id,
                    nexa_core::activity::ActivityState::Completed,
                    detail,
                )
                .unwrap();
        }

        let receipts = action_receipts_requiring_reconciliation(&runtime, turn_id)
            .await
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(
            receipts,
            HashSet::from([
                "browser_action:turn:close-tab:nonfinal".to_string(),
                "browser_action:turn:close-session:wrong-scope".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn cleanup_retry_success_becomes_the_latest_terminal_boundary() {
        let db = nexa_core::db::Database::open_memory().unwrap();
        let runtime = nexa_core::activity::ActivityRuntime::with_database(db).unwrap();
        let turn_id = "turn-with-successful-cleanup-retry";

        for (activity_id, state, detail) in [
            (
                "browser_action:turn:cleanup:pending",
                nexa_core::activity::ActivityState::Failed,
                serde_json::json!({
                    "stage": "cleanup_pending",
                    "action": "close_session",
                    "browserSessionId": "browser-session-1",
                    "sessionRetainedForRetry": true,
                }),
            ),
            (
                "browser_action:turn:cleanup:retry-success",
                nexa_core::activity::ActivityState::Completed,
                serde_json::json!({
                    "stage": "observed",
                    "action": "close_session",
                    "browserSessionId": "browser-session-1",
                    "sessionClosed": true,
                }),
            ),
        ] {
            let record = runtime
                .start(
                    nexa_core::activity::ActivitySpec::new(
                        nexa_core::activity::ActivitySurface::Browser,
                        "browser_session",
                    )
                    .with_activity_id(activity_id)
                    .with_session_id("browser-session-1")
                    .with_turn_id(turn_id),
                )
                .unwrap();
            runtime
                .transition(&record.activity_id, state, detail)
                .unwrap();
        }

        assert!(action_receipts_requiring_reconciliation(&runtime, turn_id)
            .await
            .is_empty());
    }
}

pub struct DesktopRunningAgentStopError {
    pub message: String,
}

pub fn annotate_user_artifacts_with_execution_mode(
    artifacts: Option<serde_json::Value>,
    execution_mode: AgentExecutionMode,
    power_mode: AgentPowerMode,
    collaboration_mode: AgentCollaborationMode,
    moa_preset: MoaPresetId,
    orchestration_profile: OrchestrationProfile,
) -> Option<serde_json::Value> {
    if !execution_mode.is_plan()
        && !power_mode.is_nexus()
        && !collaboration_mode.is_moa()
        && orchestration_profile == OrchestrationProfile::Balanced
    {
        return artifacts;
    }

    let insert_markers = |map: &mut serde_json::Map<String, serde_json::Value>| {
        if execution_mode.is_plan() {
            map.insert(
                "executionMode".to_string(),
                execution_mode_artifact(execution_mode),
            );
        }
        if power_mode.is_nexus() {
            map.insert(
                "powerMode".to_string(),
                serde_json::json!({
                    "kind": "agentPowerMode",
                    "version": 1,
                    "mode": power_mode.as_str(),
                }),
            );
        }
        if collaboration_mode.is_moa() {
            map.insert(
                "collaborationMode".to_string(),
                serde_json::json!({
                    "kind": "agentCollaborationMode",
                    "version": 1,
                    "mode": collaboration_mode.as_str(),
                    "preset": moa_preset.as_str(),
                }),
            );
        }
        if orchestration_profile != OrchestrationProfile::Balanced {
            map.insert(
                "orchestrationProfile".to_string(),
                serde_json::json!({
                    "kind": "orchestrationProfile",
                    "version": 1,
                    "profile": orchestration_profile.as_str(),
                }),
            );
        }
    };
    match artifacts {
        None => {
            let mut map = serde_json::Map::new();
            map.insert(
                "kind".to_string(),
                serde_json::Value::String("chatSendContext".to_string()),
            );
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
        Some(serde_json::Value::Object(mut map)) => {
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert(
                "kind".to_string(),
                serde_json::Value::String("chatSendContext".to_string()),
            );
            map.insert("userArtifacts".to_string(), value);
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
    }
}
