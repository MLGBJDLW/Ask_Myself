use super::*;

pub(super) struct WorkerStageMetrics {
    pub(super) history_load_ms: u64,
    pub(super) context_build_ms: u64,
    pub(super) skill_select_ms: u64,
    pub(super) tool_registry_ms: u64,
    pub(super) request_build_ms: u64,
}

pub(super) struct PreparedSubagentWorker {
    pub(super) worker_cancel_token: CancellationToken,
    pub(super) role_profile: Option<&'static SubagentRoleProfile>,
    pub(super) requested_task_id: Option<String>,
    pub(super) session_id: String,
    pub(super) previous_session: Option<SubagentSessionSnapshot>,
    pub(super) config: AgentConfig,
    pub(super) provider: Box<dyn LlmProvider>,
    pub(super) effective_provider_type: ProviderType,
    pub(super) effective_model: Option<String>,
    pub(super) model_route_fallback: bool,
    pub(super) delegation_limits: DelegationLimitsV2,
    pub(super) context_snapshot: Arc<DelegationContextSnapshot>,
    pub(super) run_deadline_ms: Option<u64>,
    pub(super) effective_allowed_tools: Vec<String>,
    pub(super) effective_source_scope: Vec<String>,
    pub(super) preflight: SubagentPreflightReport,
    pub(super) evidence_handoff: Vec<EvidenceHandoffItem>,
    pub(super) enabled_skills: Vec<Skill>,
    pub(super) applied_skill_refs: Vec<AppliedSkillRef>,
    pub(super) tools: ToolRegistry,
    pub(super) request_text: String,
    pub(super) initial_output_credit: u32,
    pub(super) reserved_tokens: u32,
    pub(super) context_snapshot_artifact: serde_json::Value,
    pub(super) effective_model_budgets: serde_json::Value,
    pub(super) subtask_input: serde_json::Value,
    pub(super) source_scope_applied: bool,
    pub(super) metrics: WorkerStageMetrics,
}

pub(super) async fn prepare_subagent_worker(
    runtime: &DelegationRuntime,
    db: &Database,
    inherited_source_scope: Vec<String>,
    args: &SpawnSubagentArgs,
    call_label: &str,
    worker_id: Option<&str>,
) -> Result<PreparedSubagentWorker, CoreError> {
    if runtime.delegation_depth >= MAX_SUBAGENT_DELEGATION_DEPTH {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Policy,
            "recursion_depth_exceeded",
            false,
            format!(
                "Recursive delegated execution is blocked beyond depth {}.",
                MAX_SUBAGENT_DELEGATION_DEPTH
            ),
        ));
    }
    let worker_cancel_token = runtime.cancel_token.child_token();
    let role_profile = resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())?;
    let requested_task_id = args.task_id.clone();
    let session_id = requested_task_id.clone().unwrap_or_else(|| {
        worker_id
            .map(str::to_string)
            .unwrap_or_else(|| call_label.to_string())
    });
    let history_load_started = Instant::now();
    let previous_session = requested_task_id
        .as_deref()
        .and_then(|task_id| runtime.get_session_snapshot(task_id));
    let history_load_ms = instant_elapsed_ms(history_load_started);
    let context_build_started = Instant::now();
    let (mut config, provider_config) = resolve_subagent_route(runtime, db, &args.route)?;
    let model_route_fallback = args.route.model.is_none()
        && apply_delegated_model_policy(&mut config, &provider_config, args.model_policy.as_ref());
    let catalog_authoritative = config.model.as_deref().is_some_and(|model| {
        nexa_core::provider_catalog::endpoint_model_catalog_limits_are_authoritative(
            provider_catalog_key(provider_config.provider_type),
            provider_config.base_url.as_deref(),
            model,
        )
    });
    config.catalog_limits_authoritative = Some(catalog_authoritative);
    apply_explicit_worker_reasoning(&mut config, &provider_config, &args.route)?;
    // Workers execute a handoff; the parent's fan-out and final-synthesis
    // policies must not become recursive worker completion requirements.
    config.power_mode = Default::default();
    config.collaboration_mode = Default::default();
    config.orchestration_profile = Default::default();
    config.custom_orchestration = None;
    config.volatile_system_sections.retain(|section| {
        ![
            "## Nexus Execution Policy",
            "## Orchestration Quality Profile",
            "## Mixture-of-Agents Collaboration",
        ]
        .iter()
        .any(|header| section.starts_with(header))
    });
    let effective_model = config.model.clone();
    let effective_model_id = effective_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| {
            subagent_preflight_failure(
                SubagentPreflightStage::Provider,
                "model_unresolved",
                false,
                "No effective model was resolved.",
            )
        })?
        .to_string();
    let provider = create_provider(provider_config.clone()).map_err(|error| {
        subagent_preflight_failure(
            SubagentPreflightStage::Provider,
            "provider_configuration_invalid",
            false,
            error.to_string(),
        )
    })?;
    let provider_id = provider.name().to_string();
    let effective_provider_type = config
        .provider_type
        .unwrap_or(provider_config.provider_type);
    let catalog_limits = effective_model
        .as_deref()
        .filter(|_| catalog_authoritative)
        .and_then(|model| model_limits_from_catalog(effective_provider_type, model));
    let mut delegation_limits = runtime.budget.limits().await;
    if runtime.base_config.delegation_limits_v2.is_none()
        && (args.route.agent_config_id.is_some()
            || effective_model != runtime.base_config.model
            || provider_config.provider_type != runtime.provider_config.provider_type
            || provider_config.base_url != runtime.provider_config.base_url)
    {
        // Legacy parent model limits are not explicit worker limits. Resolve
        // those defaults from the selected route while keeping shared budgets.
        delegation_limits.input_context_policy = config
            .context_window
            .map(|limit| DelegationLimitPolicy::Explicit(u64::from(limit)))
            .unwrap_or(DelegationLimitPolicy::Auto);
        delegation_limits.max_output_tokens_per_worker = config
            .max_tokens
            .map(|limit| DelegationLimitPolicy::Explicit(u64::from(limit)))
            .unwrap_or(DelegationLimitPolicy::Auto);
    }
    config.max_iterations = args.max_iterations.unwrap_or(config.max_iterations);
    let resolved_model_context = resolve_endpoint_model_context_window(
        provider_catalog_key(provider_config.provider_type),
        provider_config.base_url.as_deref(),
        &effective_model_id,
        None,
    );
    let context_authority = apply_delegated_model_limits(
        &mut config,
        delegation_limits.input_context_policy,
        delegation_limits.max_output_tokens_per_worker,
        resolved_model_context,
        catalog_limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens),
        runtime.base_config.delegation_limits_v2.is_some(),
    );
    config.context_window_resolution = Some(ResolvedContextWindow {
        capacity_tokens: config.context_window,
        authority: context_authority,
    });
    if let Some(worker_limit) = delegation_limits
        .max_actual_tokens_per_worker
        .and_then(|limit| u32::try_from(limit).ok())
    {
        config.max_tokens = Some(config.max_tokens.unwrap_or(worker_limit).min(worker_limit));
        config.max_actual_tokens_per_run = Some(worker_limit);
    }
    let model_context_limit = config.context_window;
    let handoff_budget_snapshot = runtime.budget.snapshot().await;
    let fair_share_divisor = handoff_budget_snapshot
        .max_parallel
        .min(handoff_budget_snapshot.remaining_calls.unwrap_or(u32::MAX))
        .max(1);
    let fair_share = handoff_budget_snapshot.remaining_tokens / fair_share_divisor;
    let control_lane_role = matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    );
    let automatic_handoff_budget = if control_lane_role {
        delegation_limits
            .max_actual_tokens_per_worker
            .and_then(|limit| u32::try_from(limit).ok())
            .unwrap_or(fair_share)
            .saturating_mul(3)
            / 5
    } else {
        fair_share.saturating_mul(3) / 5
    };
    let mut handoff_token_budget = delegation_limits
        .handoff_context_tokens_per_worker
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(automatic_handoff_budget);
    if let Some(model_context_limit) = model_context_limit {
        handoff_token_budget = handoff_token_budget
            .min(model_context_limit.saturating_mul(3) / 5)
            .max(1.min(model_context_limit));
    } else {
        handoff_token_budget = handoff_token_budget.max(1);
    }
    let context_snapshot = runtime.context_snapshot(
        &db,
        effective_model.as_deref().unwrap_or("default"),
        model_context_limit,
        handoff_token_budget,
    );
    let context_build_ms = instant_elapsed_ms(context_build_started);
    let run_deadline_ms = resolve_delegation_run_deadline_ms(
        &runtime.base_config,
        args.timeout_secs,
        delegation_limits.run_deadline_ms,
    );
    config.agent_timeout_secs =
        run_deadline_ms.map(|ms| u32::try_from(ms.div_ceil(1_000)).unwrap_or(u32::MAX).max(1));
    config.request_kind = AgentRequestKind::SubagentWorker;
    config.system_prompt =
        build_subagent_system_prompt(&config.system_prompt, args.role.as_deref(), role_profile);
    let available_tool_names = runtime
        .get_tool_registry()
        .map_err(|error| {
            subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_registry_unavailable",
                false,
                error.to_string(),
            )
        })?
        .tool_names();
    let baseline_allowed_tools =
        normalize_allowed_tools(runtime.allowed_tools.as_deref(), &available_tool_names);
    let mut effective_allowed_tools = resolve_allowed_tools_for_role(
        &baseline_allowed_tools,
        args.allowed_tools.as_deref(),
        role_profile,
    );
    if runtime.delegation_depth.saturating_add(1) >= MAX_SUBAGENT_DELEGATION_DEPTH {
        effective_allowed_tools.retain(|name| !is_subagent_tool_name(name));
    }
    // Interactive browser/computer control is parent-scoped until delegated
    // workers have a parent approval proxy and a surface capability lease.
    // `conversation_id=None` must never become a shared tenant key.
    effective_allowed_tools.retain(|name| !is_interactive_surface_tool(name));
    let effective_source_scope =
        resolve_source_scope(&inherited_source_scope, args.source_ids.as_deref());
    let mut preflight = validate_subagent_preflight(
        &args,
        &effective_model_id,
        &provider_id,
        &baseline_allowed_tools,
        &effective_allowed_tools,
        &inherited_source_scope,
        &effective_source_scope,
        &context_snapshot,
    )?;
    let evidence_handoff = build_evidence_handoff(&db, args.evidence_chunk_ids.as_deref());
    let skill_select_started = Instant::now();
    let selected_skill_query = format!(
        "{}\n{}",
        args.task,
        args.context.clone().unwrap_or_default()
    );
    let skill_index = runtime
        .skill_index
        .get_or_init(|| load_skill_index_snapshot(&db));
    let enabled_skills = nexa_core::skills::select_available_skills_from_pool(
        filter_enabled_skills(&skill_index.skills, runtime.allowed_skill_ids.as_deref()),
        &selected_skill_query,
    );
    let applied_skill_refs = applied_skills(&enabled_skills);
    let skill_select_ms = instant_elapsed_ms(skill_select_started);
    let tool_registry_started = Instant::now();
    let tools =
        build_subagent_executor_tools(&runtime, &effective_allowed_tools, &worker_cancel_token)
            .map_err(|error| {
                subagent_preflight_failure(
                    SubagentPreflightStage::Policy,
                    "tool_registry_construction_failed",
                    false,
                    error.to_string(),
                )
            })?;
    let tool_registry_ms = instant_elapsed_ms(tool_registry_started);
    let request_build_started = Instant::now();
    let request_text = build_subagent_request(
        &args,
        role_profile,
        &effective_source_scope,
        &effective_allowed_tools,
        &applied_skill_refs,
        &evidence_handoff,
        previous_session.as_ref(),
    );
    let request_build_ms = instant_elapsed_ms(request_build_started);
    let initial_output_credit = initial_output_credit(role_profile, &args, &config);
    let inherited_skill_tokens = enabled_skills.iter().fold(0_u32, |total, skill| {
        total.saturating_add(estimate_tokens_for_model(
            &effective_model_id,
            &skill.content,
        ))
    });
    let reserved_tokens = estimate_reserved_tokens(
        &config,
        &request_text,
        &tools,
        context_snapshot.token_estimate,
        inherited_skill_tokens,
        initial_output_credit,
    );
    let budget_snapshot = runtime.budget.snapshot().await;
    finalize_subagent_preflight(
        &mut preflight,
        &budget_snapshot,
        reserved_tokens,
        run_deadline_ms,
    )?;
    let mut subtask_input = subtask_input_payload(
        "subagent_run",
        call_label,
        worker_id,
        args,
        role_profile,
        &effective_source_scope,
        &effective_allowed_tools,
        &applied_skill_refs,
        reserved_tokens,
        run_deadline_ms.map(|ms| ms.div_ceil(1_000)),
    );
    subtask_input["delegationLimitsV2"] =
        serde_json::to_value(&delegation_limits).unwrap_or_else(|_| serde_json::json!({}));
    subtask_input["initialOutputCredit"] = serde_json::json!(initial_output_credit);
    subtask_input["inheritedSkillTokens"] = serde_json::json!(inherited_skill_tokens);
    subtask_input["skillIndexGeneration"] = serde_json::json!(&skill_index.generation);
    subtask_input["preflight"] =
        serde_json::to_value(&preflight).unwrap_or_else(|_| serde_json::json!({}));
    let context_snapshot_artifact = serde_json::json!({
        "id": &context_snapshot.id,
        "selectedMessageIds": &context_snapshot.selected_message_ids,
        "tokenEstimate": context_snapshot.token_estimate,
        "contextCapacity": context_snapshot.context_limit,
        "contextAuthority": context_authority,
        "handoffTokenBudget": context_snapshot.handoff_token_budget,
        "droppedInvalidMessages": context_snapshot.dropped_invalid_messages,
    });
    let output_authority = match delegation_limits.max_output_tokens_per_worker {
        DelegationLimitPolicy::Explicit(_) => "user_override",
        DelegationLimitPolicy::Auto
            if catalog_limits
                .as_ref()
                .and_then(|limits| limits.max_output_tokens)
                .is_some() =>
        {
            "catalog_ceiling"
        }
        DelegationLimitPolicy::Auto => "safe_default",
    };
    let effective_model_budgets = serde_json::json!({
        "provider": provider_catalog_key(provider_config.provider_type),
        "model": config.model,
        "reasoningEffort": config.reasoning_effort,
        "maxIterations": config.max_iterations,
        "runDeadlineMs": run_deadline_ms,
        "contextCapacity": config.context_window,
        "parentHistoryHandoff": context_snapshot.handoff_token_budget,
        "maxOutputPerStep": config.max_tokens,
        "maxActualTokensPerWorker": config.max_actual_tokens_per_run,
        "contextAuthority": context_authority,
        "outputAuthority": output_authority,
    });
    subtask_input["contextSnapshot"] = context_snapshot_artifact.clone();
    subtask_input["effectiveModelBudgets"] = effective_model_budgets.clone();
    Ok(PreparedSubagentWorker {
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
        subtask_input,
        source_scope_applied: !inherited_source_scope.is_empty()
            || args
                .source_ids
                .as_deref()
                .is_some_and(|ids| !ids.is_empty()),
        metrics: WorkerStageMetrics {
            history_load_ms,
            context_build_ms,
            skill_select_ms,
            tool_registry_ms,
            request_build_ms,
        },
    })
}

pub(super) struct AdmittedSubagentWorker {
    pub(super) subtask: SubtaskRecorder,
    pub(super) subtask_run_id: Option<String>,
    pub(super) _lane_permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) _batch_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn admit_subagent_worker(
    runtime: &DelegationRuntime,
    db: &Database,
    call_label: &str,
    args: &SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    launch_started: Instant,
    prepared: &PreparedSubagentWorker,
) -> Result<AdmittedSubagentWorker, CoreError> {
    let role_profile = prepared.role_profile;
    let reserved_tokens = prepared.reserved_tokens;
    let effective_model = &prepared.effective_model;
    let model_route_fallback = prepared.model_route_fallback;
    let delegation_limits = &prepared.delegation_limits;
    let worker_cancel_token = &prepared.worker_cancel_token;
    let subtask_input = &prepared.subtask_input;
    let WorkerStageMetrics {
        history_load_ms,
        context_build_ms,
        skill_select_ms,
        tool_registry_ms,
        request_build_ms,
    } = prepared.metrics;
    let parent_task_run_id = runtime.parent_task_run_id.clone();
    let role_label = subtask_role_label(&args, role_profile, "Subagent");
    let mut subtask = SubtaskRecorder::create(
        &db,
        parent_task_run_id.as_deref(),
        &call_label,
        &role_label,
        &subtask_input,
        reserved_tokens,
        format!("Subagent queued: {call_label}"),
        serde_json::json!({
            "callLabel": &call_label,
            "role": role_label,
            "task": &args.task,
            "modelPolicy": &args.model_policy,
            "effectiveModel": &effective_model,
            "modelRouteFallback": model_route_fallback,
            "reservedTokens": reserved_tokens,
        }),
    )?;
    subtask.record_launch_metrics(&[
        (
            "launch_ack_ms",
            Some(instant_elapsed_ms(launch_started)),
            None,
            "measured",
        ),
        ("history_load_ms", Some(history_load_ms), None, "measured"),
        ("context_build_ms", Some(context_build_ms), None, "measured"),
        ("skill_select_ms", Some(skill_select_ms), None, "measured"),
        ("mcp_sync_ms", Some(0), None, "shared_snapshot"),
        ("tool_registry_ms", Some(tool_registry_ms), None, "measured"),
        ("attachment_prepare_ms", Some(0), None, "not_applicable"),
        ("request_build_ms", Some(request_build_ms), None, "measured"),
    ]);
    let subtask_run_id = subtask.id().map(str::to_string);
    let queue_started = Instant::now();
    let is_verification = role_profile.is_some_and(|profile| profile.id == "verifier");
    let _permit = match runtime
        .budget
        .begin_call(
            &call_label,
            reserved_tokens,
            is_verification,
            &worker_cancel_token,
        )
        .await
    {
        Ok(permit) => permit,
        Err(err) => {
            let err = subagent_admission_failure(&err);
            let output = serde_json::json!({
                "kind": "subagent_run_error",
                "callLabel": &call_label,
                "error": err.to_string(),
                "preflight": subagent_preflight_failure_from_error(&err),
            });
            subtask.finish(
                "failed",
                Some(&output),
                Some(&err.to_string()),
                Some(format!("Subagent failed: {call_label}")),
            );
            return Err(err);
        }
    };
    // Acquire the batch-local cap only after the role-aware global scheduler
    // has granted a lane. Explorers queued on their lane must never occupy
    // generic batch slots and starve the dedicated verifier lane.
    let _batch_permit = if let Some(batch_slots) = batch_slots {
        match acquire_batch_slot(
            batch_slots,
            &worker_cancel_token,
            &call_label,
            queue_started,
            delegation_limits.queue_deadline_ms,
        )
        .await
        {
            Ok(permit) => Some(permit),
            Err(error) => {
                let error = subagent_preflight_failure(
                    SubagentPreflightStage::Timeout,
                    "batch_queue_deadline_exceeded",
                    true,
                    error.to_string(),
                );
                runtime
                    .budget
                    .rollback_unstarted_worker(reserved_tokens, is_verification)
                    .await;
                subtask.finish("failed", None, Some(&error.to_string()), None);
                return Err(error);
            }
        }
    } else {
        None
    };
    if let Err(err) = subtask.mark_started(
        "running",
        format!("Subagent started: {call_label}"),
        serde_json::json!({
            "subtaskRunId": &subtask_run_id,
            "callLabel": &call_label,
            "reservedTokens": reserved_tokens,
            "queueWaitMs": u64::try_from(queue_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }),
    ) {
        runtime
            .budget
            .rollback_unstarted_worker(reserved_tokens, is_verification)
            .await;
        subtask.finish("failed", None, Some(&err.to_string()), None);
        return Err(err);
    }
    subtask.emit(
        format!("Subagent connecting: {call_label}"),
        "connecting",
        &serde_json::json!({
            "subtaskRunId": &subtask_run_id,
            "callLabel": &call_label,
        }),
    );
    Ok(AdmittedSubagentWorker {
        subtask,
        subtask_run_id,
        _lane_permit: _permit,
        _batch_permit,
    })
}

pub(super) struct SubagentSettlementInput {
    pub(super) worker_id: Option<String>,
    pub(super) call_label: String,
    pub(super) args: SpawnSubagentArgs,
    pub(super) role_profile: Option<&'static SubagentRoleProfile>,
    pub(super) requested_task_id: Option<String>,
    pub(super) session_id: String,
    pub(super) previous_session: Option<SubagentSessionSnapshot>,
    pub(super) effective_model: Option<String>,
    pub(super) model_route_fallback: bool,
    pub(super) evidence_handoff: Vec<EvidenceHandoffItem>,
    pub(super) effective_source_scope: Vec<String>,
    pub(super) effective_allowed_tools: Vec<String>,
    pub(super) applied_skill_refs: Vec<AppliedSkillRef>,
    pub(super) preflight: SubagentPreflightReport,
    pub(super) context_snapshot_artifact: serde_json::Value,
    pub(super) effective_model_budgets: serde_json::Value,
    pub(super) source_scope_applied: bool,
}

pub(super) struct SubagentExecutionInput<'a> {
    pub(super) runtime: &'a DelegationRuntime,
    pub(super) db: &'a Database,
    pub(super) call_label: &'a str,
    pub(super) launch_started: Instant,
    pub(super) parent_task_run_id: Option<String>,
    pub(super) subtask_run_id: Option<String>,
    pub(super) session_id: String,
    pub(super) worker_cancel_token: CancellationToken,
    pub(super) effective_provider_type: ProviderType,
    pub(super) effective_model: Option<String>,
    pub(super) worker_actual_token_limit: Option<u32>,
    pub(super) run_deadline_ms: Option<u64>,
    pub(super) context_messages: Vec<Message>,
    pub(super) effective_source_scope: Vec<String>,
    pub(super) initial_output_credit: u32,
    pub(super) reserved_tokens: u32,
    pub(super) provider: Box<dyn LlmProvider>,
    pub(super) tools: ToolRegistry,
    pub(super) config: AgentConfig,
    pub(super) enabled_skills: Vec<Skill>,
    pub(super) request_text: String,
    pub(super) steering_rx: Option<mpsc::UnboundedReceiver<AgentSteeringMessage>>,
    pub(super) lifecycle_events: Option<SubagentEventBridge>,
}

pub(super) async fn execute_subagent_worker(
    input: SubagentExecutionInput<'_>,
    subtask: &mut SubtaskRecorder,
) -> Result<(Message, EventCapture), CoreError> {
    let SubagentExecutionInput {
        runtime,
        db,
        call_label,
        launch_started,
        parent_task_run_id,
        subtask_run_id,
        session_id,
        worker_cancel_token,
        effective_provider_type,
        effective_model,
        worker_actual_token_limit,
        run_deadline_ms,
        context_messages,
        effective_source_scope,
        initial_output_credit,
        reserved_tokens,
        provider,
        tools,
        config,
        enabled_skills,
        request_text,
        steering_rx,
        lifecycle_events,
    } = input;
    let estimated_cost_micros =
        nexa_core::usage_analytics::usage_cost_metadata(Some(effective_provider_type)).0;
    let non_streaming_completion = llm_streaming_disabled_by_env()
        || provider_uses_non_streaming_fallback(
            effective_provider_type,
            effective_model.as_deref().unwrap_or_default(),
        );
    let mut executor = AgentExecutor::new(provider, tools, config)
        .with_usage_identity(
            format!(
                "subagent:{}",
                subtask_run_id.as_deref().unwrap_or(&session_id)
            ),
            parent_task_run_id.clone(),
            subtask_run_id.clone(),
        )
        .with_cancel_token(worker_cancel_token.clone())
        .with_skills_override(enabled_skills);
    if let Some(steering_rx) = steering_rx {
        executor = executor.with_steering_receiver(steering_rx);
    }
    let (tx, event_rx) = mpsc::channel::<AgentEvent>(64);
    let pump = SubagentEventPump::spawn(
        event_rx,
        SubagentEventPumpConfig {
            cancel_token: worker_cancel_token.clone(),
            worker_actual_token_limit,
            telemetry_db: db.clone(),
            telemetry_identity: parent_task_run_id.zip(subtask_run_id),
            telemetry_call_label: call_label.to_string(),
            lifecycle: lifecycle_events,
            launch_started,
            non_streaming_completion,
        },
    );
    let mut fatal_error_rx = pump.fatal_error_rx;
    let mut event_task = pump.task;
    let run_future = executor.run_with_source_scope(
        context_messages,
        vec![ContentPart::Text { text: request_text }],
        db,
        None,
        None,
        Some(effective_source_scope),
        tx,
        0,
    );
    let final_result = await_subagent_worker_completion(
        call_label,
        &worker_cancel_token,
        &mut fatal_error_rx,
        run_future,
        run_deadline_ms,
    )
    .await;
    let mut capture = match tokio::time::timeout(Duration::from_millis(500), &mut event_task).await
    {
        Ok(Ok(capture)) => capture,
        Ok(Err(error)) => {
            warn!("Subagent event collector failed for {call_label}: {error}");
            EventCapture::default()
        }
        Err(_) => {
            event_task.abort();
            let _ = event_task.await;
            warn!("Subagent event collector exceeded its 500ms shutdown deadline for {call_label}");
            EventCapture::default()
        }
    };
    if final_result.is_err() && capture.usage_total.total_tokens == 0 {
        capture.usage_total.prompt_tokens = reserved_tokens.saturating_sub(initial_output_credit);
        capture.usage_total.total_tokens = reserved_tokens;
    }
    runtime
        .budget
        .finish_call(reserved_tokens, &capture.usage_total, estimated_cost_micros)
        .await;
    match final_result {
        Ok(message) => Ok((message, capture)),
        Err(err) => {
            let error_text = err.to_string();
            let failure_status = delegated_failure_status(&error_text);
            let output = serde_json::json!({
                "kind": "subagent_run_error",
                "callLabel": call_label,
                "error": &error_text,
                "emittedError": capture.error_message,
                "usageTotal": capture.usage_total,
                "toolEvents": capture.tool_events,
            });
            subtask.finish(
                failure_status,
                Some(&output),
                Some(&error_text),
                Some(format!("Subagent {failure_status}: {call_label}")),
            );
            Err(err)
        }
    }
}

pub(super) fn settle_subagent_artifact(
    runtime: &DelegationRuntime,
    input: SubagentSettlementInput,
    final_message: Message,
    capture: EventCapture,
    subtask: &mut SubtaskRecorder,
) -> SubagentRunArtifact {
    let result_text = final_message.text_content().trim().to_string();
    let result_text = if result_text.is_empty() {
        "(Subagent returned no text.)".to_string()
    } else {
        result_text
    };
    let args = input.args;
    let run = SubagentRunArtifact {
        id: input.worker_id.unwrap_or_else(|| input.call_label.clone()),
        session_id: input.session_id.clone(),
        resumed_from_task_id: input
            .requested_task_id
            .clone()
            .filter(|_| input.previous_session.is_some()),
        previous_session: input.previous_session,
        status: "done".to_string(),
        task: args.task,
        role_id: input.role_profile.map(|profile| profile.id.to_string()),
        role_name: input.role_profile.map(|profile| profile.label.to_string()),
        role: args.role,
        model_policy: args.model_policy,
        effective_model: input.effective_model,
        model_route_fallback: input.model_route_fallback,
        expected_output: args.expected_output,
        acceptance_criteria: args.acceptance_criteria,
        evidence_chunk_ids: args.evidence_chunk_ids,
        evidence_handoff: input.evidence_handoff,
        requested_source_scope: args.source_ids,
        effective_source_scope: input.effective_source_scope,
        requested_allowed_tools: args.allowed_tools,
        allowed_tools: input.effective_allowed_tools,
        allowed_skills: input.applied_skill_refs,
        parallel_group: args.parallel_group,
        deliverable_style: args.deliverable_style,
        return_sections: args.return_sections,
        result: result_text,
        finish_reason: capture.finish_reason,
        usage_total: capture.usage_total,
        tool_events: capture.tool_events,
        thinking: if capture.thinking.is_empty() {
            None
        } else {
            Some(capture.thinking)
        },
        source_scope_applied: input.source_scope_applied,
        is_error: false,
        error_message: None,
        preflight_failure: None,
        preflight: Some(input.preflight),
        context_snapshot: Some(input.context_snapshot_artifact),
        effective_model_budgets: Some(input.effective_model_budgets),
    };
    runtime.save_session_snapshot(SubagentSessionSnapshot {
        task_id: input.session_id,
        last_run_id: run.id.clone(),
        task: run.task.clone(),
        role_id: run.role_id.clone(),
        role_name: run.role_name.clone(),
        result: run.result.clone(),
        finish_reason: run.finish_reason.clone(),
        usage_total: run.usage_total.clone(),
        tool_event_count: run.tool_events.len(),
    });
    let output = serde_json::json!({
        "kind": "subagent_run",
        "run": &run,
    });
    subtask.finish(
        "completed",
        Some(&output),
        None,
        Some(format!("Subagent completed: {}", run.id)),
    );
    run
}
