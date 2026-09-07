use super::*;

pub(super) async fn await_subagent_worker_completion<T, F>(
    call_label: &str,
    cancel_token: &CancellationToken,
    fatal_error_rx: &mut mpsc::UnboundedReceiver<String>,
    run_future: F,
    run_deadline_ms: Option<u64>,
) -> Result<T, CoreError>
where
    F: std::future::Future<Output = Result<T, CoreError>>,
{
    tokio::select! {
        biased;
        error = fatal_error_rx.recv() => Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' failed: {}",
            error.unwrap_or_else(|| "worker emitted an unspecified fatal error".to_string())
        ))),
        _ = cancel_token.cancelled() => Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' was cancelled by the parent turn."
        ))),
        result = with_optional_timeout(run_deadline_ms, run_future) => match result {
            Ok(result) => result,
            Err(_) => {
                cancel_token.cancel();
                Err(CoreError::Agent(format!(
                    "Delegated execution '{call_label}' timed out after {}ms.",
                    run_deadline_ms.unwrap_or_default()
                )))
            }
        }
    }
}

pub(super) async fn run_subagent_once(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    steering_rx: Option<mpsc::UnboundedReceiver<AgentSteeringMessage>>,
    lifecycle_events: Option<SubagentEventBridge>,
) -> Result<SubagentRunArtifact, CoreError> {
    let launch_started = Instant::now();
    let prepared = prepare_subagent_worker(
        &runtime,
        &db,
        inherited_source_scope,
        &args,
        &call_label,
        worker_id.as_deref(),
    )
    .await?;
    let admitted = admit_subagent_worker(
        &runtime,
        &db,
        &call_label,
        &args,
        batch_slots,
        launch_started,
        &prepared,
    )
    .await?;
    let parent_task_run_id = runtime.parent_task_run_id.clone();
    let AdmittedSubagentWorker {
        mut subtask,
        subtask_run_id,
        _lane_permit,
        _batch_permit,
    } = admitted;
    let PreparedSubagentWorker {
        worker_cancel_token,
        role_profile,
        requested_task_id,
        session_id,
        previous_session,
        config,
        provider,
        effective_provider_type,
        effective_model,
        model_route_fallback,
        delegation_limits,
        context_snapshot,
        run_deadline_ms,
        effective_allowed_tools,
        effective_source_scope,
        preflight,
        evidence_handoff,
        enabled_skills,
        applied_skill_refs,
        tools,
        request_text,
        initial_output_credit,
        reserved_tokens,
        context_snapshot_artifact,
        effective_model_budgets,
        source_scope_applied,
        ..
    } = prepared;
    let (final_message, capture) = execute_subagent_worker(
        SubagentExecutionInput {
            runtime: &runtime,
            db: &db,
            call_label: &call_label,
            launch_started,
            parent_task_run_id: parent_task_run_id.clone(),
            subtask_run_id: subtask_run_id.clone(),
            session_id: session_id.clone(),
            worker_cancel_token: worker_cancel_token.clone(),
            effective_provider_type,
            effective_model: effective_model.clone(),
            worker_actual_token_limit: delegation_limits
                .max_actual_tokens_per_worker
                .and_then(|limit| u32::try_from(limit).ok()),
            run_deadline_ms,
            context_messages: context_snapshot.messages.as_ref().to_vec(),
            effective_source_scope: effective_source_scope.clone(),
            initial_output_credit,
            reserved_tokens,
            provider,
            tools,
            config,
            enabled_skills,
            request_text,
            steering_rx,
            lifecycle_events,
        },
        &mut subtask,
    )
    .await?;
    let run = settle_subagent_artifact(
        &runtime,
        SubagentSettlementInput {
            worker_id,
            call_label,
            args,
            role_profile,
            requested_task_id,
            session_id,
            previous_session,
            effective_model,
            model_route_fallback,
            evidence_handoff,
            effective_source_scope,
            effective_allowed_tools,
            applied_skill_refs,
            preflight,
            context_snapshot_artifact,
            effective_model_budgets,
            source_scope_applied,
        },
        final_message,
        capture,
        &mut subtask,
    );
    Ok(run)
}
pub(super) fn isolated_subagent_runtime() -> Result<tokio::runtime::Runtime, CoreError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CoreError::Internal(format!(
                "Failed to build isolated subagent runtime: {error}"
            ))
        })
}
pub(super) async fn settle_worker_lifecycle(
    lifecycle: &SubagentLifecycleRuntime,
    agent_id: &str,
    cancellation: &CancellationToken,
    outcome: Result<&SubagentRunArtifact, &CoreError>,
) {
    let (status, result, error) = match outcome {
        Ok(run) => (
            SubagentLifecycleStatus::Completed,
            serde_json::to_value(run).ok(),
            None,
        ),
        Err(error) => (
            if cancellation.is_cancelled() {
                SubagentLifecycleStatus::Cancelled
            } else {
                SubagentLifecycleStatus::Failed
            },
            None,
            Some(error.to_string()),
        ),
    };
    if let Err(error) = lifecycle.finish(agent_id, status, result, error).await {
        warn!("Failed to settle lifecycle for subagent {agent_id}: {error}");
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_worker_with_lifecycle(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    registration: crate::subagent_lifecycle::SubagentWorkerRegistration,
) -> Result<SubagentRunArtifact, CoreError> {
    let lifecycle = runtime.lifecycle.clone();
    let agent_id = registration.agent_id.clone();
    let cancellation = registration.cancel_token.clone();
    if let Err(error) = registration.events.start().await {
        let _ = lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Failed);
        return Err(error);
    }
    lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Running)?;
    let outcome = run_subagent_once(
        runtime.scoped_to_worker(registration.cancel_token),
        db,
        inherited_source_scope,
        call_label,
        worker_id,
        args,
        batch_slots,
        Some(registration.steering_rx),
        Some(registration.events),
    )
    .await;
    settle_worker_lifecycle(&lifecycle, &agent_id, &cancellation, outcome.as_ref()).await;
    outcome
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_registered_subagent_isolated(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    registration: crate::subagent_lifecycle::SubagentWorkerRegistration,
) -> Result<SubagentRunArtifact, CoreError> {
    let isolated_runtime = isolated_subagent_runtime()?;
    let (result_tx, result_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("nexa-subagent-worker".to_string())
        .spawn(move || {
            let result = isolated_runtime.block_on(run_worker_with_lifecycle(
                runtime,
                db,
                inherited_source_scope,
                call_label,
                worker_id,
                args,
                batch_slots,
                registration,
            ));
            let _ = result_tx.send(result);
        })
        .map_err(|error| {
            CoreError::Internal(format!("Failed to start isolated subagent thread: {error}"))
        })?;
    result_rx.await.map_err(|_| {
        CoreError::Agent("Isolated subagent thread exited without a result".to_string())
    })?
}
pub(super) fn launch_detached_subagent(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    args: SpawnSubagentArgs,
    registration: crate::subagent_lifecycle::SubagentWorkerRegistration,
) -> Result<(), CoreError> {
    let isolated_runtime = isolated_subagent_runtime()?;
    let agent_id = registration.agent_id.clone();
    std::thread::Builder::new()
        .name("nexa-subagent-worker".to_string())
        .spawn(move || {
            if let Err(error) = isolated_runtime.block_on(run_worker_with_lifecycle(
                runtime,
                db,
                inherited_source_scope,
                agent_id.clone(),
                Some(agent_id.clone()),
                args,
                None,
                registration,
            )) {
                warn!("Detached subagent {agent_id} failed: {error}");
            }
        })
        .map_err(|error| {
            CoreError::Internal(format!("Failed to start detached subagent thread: {error}"))
        })?;
    Ok(())
}
pub(super) fn failed_subagent_run_artifact(
    label: String,
    fallback: SpawnSubagentArgs,
    parallel_group: Option<String>,
    error: &CoreError,
) -> SubagentRunArtifact {
    SubagentRunArtifact {
        id: label.clone(),
        session_id: label,
        resumed_from_task_id: None,
        previous_session: None,
        status: "error".to_string(),
        task: fallback.task,
        role_id: fallback.role_id.clone(),
        role_name: resolve_role_profile(fallback.role_id.as_deref(), fallback.role.as_deref())
            .ok()
            .flatten()
            .map(|profile| profile.label.to_string()),
        role: fallback.role,
        model_policy: fallback.model_policy,
        effective_model: None,
        model_route_fallback: false,
        expected_output: fallback.expected_output,
        acceptance_criteria: fallback.acceptance_criteria,
        evidence_chunk_ids: fallback.evidence_chunk_ids,
        evidence_handoff: Vec::new(),
        requested_source_scope: fallback.source_ids,
        effective_source_scope: Vec::new(),
        requested_allowed_tools: fallback.allowed_tools,
        allowed_tools: Vec::new(),
        allowed_skills: Vec::new(),
        parallel_group,
        deliverable_style: fallback.deliverable_style,
        return_sections: fallback.return_sections,
        result: format!("Subagent failed: {error}"),
        finish_reason: None,
        usage_total: Usage::default(),
        tool_events: Vec::new(),
        thinking: None,
        source_scope_applied: false,
        is_error: true,
        error_message: Some(error.to_string()),
        preflight_failure: subagent_preflight_failure_from_error(error),
        preflight: None,
        context_snapshot: None,
        effective_model_budgets: None,
    }
}
pub(super) fn summarize_subagent_run(run: &SubagentRunArtifact) -> String {
    let role_suffix = run
        .role_name
        .as_deref()
        .or(run.role.as_deref())
        .map(|role| format!(" ({role})"))
        .unwrap_or_default();
    format!(
        "{}{}: {}",
        run.task,
        role_suffix,
        truncate_excerpt(&run.result, 220)
    )
}
