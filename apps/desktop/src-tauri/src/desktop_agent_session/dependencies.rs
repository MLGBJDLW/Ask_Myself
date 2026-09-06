use super::*;

pub(crate) async fn sync_enabled_desktop_mcp_servers(
    manager: &mut McpManager,
    enabled_servers: &[McpServer],
    timeout_secs: u64,
) -> Result<HashMap<String, String>, String> {
    Ok(manager
        .sync_servers(enabled_servers, Some(timeout_secs))
        .await)
}

#[derive(Clone)]
pub(crate) struct DesktopToolRegistrySnapshot {
    generation: String,
    tools: ToolRegistry,
}

static DESKTOP_TOOL_REGISTRY_SNAPSHOT: OnceLock<TokioMutex<Option<DesktopToolRegistrySnapshot>>> =
    OnceLock::new();

#[derive(Clone)]
pub(crate) struct DesktopPackageRegistrySnapshot {
    generation: String,
    allowed_tool_names: Vec<String>,
}

pub(crate) struct DesktopPackageRegistryResolution {
    pub(crate) tools: ToolRegistry,
    pub(crate) successful_snapshot: Option<DesktopPackageRegistrySnapshot>,
    pub(crate) used_last_known_good: bool,
    error: Option<String>,
}

static DESKTOP_PACKAGE_REGISTRY_SNAPSHOT: OnceLock<
    TokioMutex<Option<DesktopPackageRegistrySnapshot>>,
> = OnceLock::new();

/// Applies Package Host policy to the final per-turn desktop registry.
///
/// A failed assembly may reuse only the allowed-name projection from the
/// previous successful pass for the same package/MCP generation. Reusing the
/// current pre-filter registry would bypass disabled packages and ownership
/// validation, so a missing or stale projection fails closed.
pub(crate) fn resolve_desktop_package_registry(
    current_prefilter: &ToolRegistry,
    assembled: Result<RuntimeCapabilitySet, PackageHostContractError>,
    generation: Option<&str>,
    last_known_good: Option<&DesktopPackageRegistrySnapshot>,
) -> DesktopPackageRegistryResolution {
    match assembled {
        Ok(capabilities) => {
            let successful_snapshot = generation.map(|generation| DesktopPackageRegistrySnapshot {
                generation: generation.to_string(),
                allowed_tool_names: capabilities.tools.tool_names(),
            });
            DesktopPackageRegistryResolution {
                tools: capabilities.tools,
                successful_snapshot,
                used_last_known_good: false,
                error: None,
            }
        }
        Err(error) => {
            let fallback = generation.and_then(|generation| {
                last_known_good.filter(|snapshot| snapshot.generation == generation)
            });
            DesktopPackageRegistryResolution {
                tools: fallback
                    .map(|snapshot| current_prefilter.filtered(&snapshot.allowed_tool_names))
                    .unwrap_or_default(),
                successful_snapshot: None,
                used_last_known_good: fallback.is_some(),
                error: Some(error.to_string()),
            }
        }
    }
}

pub(crate) fn desktop_tool_registry_generation(
    assembler: &PackageRuntimeAssembler,
    enabled_servers: &[McpServer],
) -> Result<String, String> {
    let snapshot = serde_json::to_vec(&(assembler.snapshot(), enabled_servers))
        .map_err(|error| format!("Failed to serialize tool registry generation: {error}"))?;
    Ok(blake3::hash(&snapshot).to_hex().to_string())
}

pub async fn build_desktop_agent_session_dependencies(
    request: DesktopAgentSessionDependencyRequest<'_>,
) -> DesktopAgentSessionDependencies {
    let DesktopAgentSessionDependencyRequest {
        subscription_runtime,
        db,
        mcp_manager,
        event_seq,
        conversation_id,
        task_run_id,
        turn_id,
        message,
        pinned_skill_ids,
        provider_config,
        executor_config,
        root_allowed_tools,
        subagent_allowed_tools,
        subagent_allowed_skill_ids,
        subagent_lifecycle,
        cancel_token,
        plan_mode,
        mcp_call_timeout_secs,
        terminal_state,
        browser_state,
    } = request;

    let skill_select_started = Instant::now();
    let selected_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_available_skills_for_query(db, message)
    } else {
        nexa_core::skills::get_available_skills_for_query_with_pinned(db, message, pinned_skill_ids)
    }
    .unwrap_or_else(|err| {
        warn!("Failed to select skills for task run {task_run_id}: {err}");
        Vec::new()
    });

    let max_loaded_skills = 3usize.max(pinned_skill_ids.len());
    let auto_loaded_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_active_skills_for_query(db, message, max_loaded_skills)
    } else {
        nexa_core::skills::get_active_skills_for_query_with_pinned(
            db,
            message,
            max_loaded_skills,
            pinned_skill_ids,
        )
    }
    .unwrap_or_else(|err| {
        warn!("Failed to auto-load skills for task run {task_run_id}: {err}");
        Vec::new()
    });
    let skill_select_ms = elapsed_ms(skill_select_started);

    let tool_registry_started = Instant::now();
    let package_assembler = PackageRuntimeAssembler::database_builtin(db);
    emit_agent_frontend_event_with_presentation(
        event_seq,
        conversation_id,
        task_run_id,
        Some(turn_id),
        AgentEvent::Status {
            content: "Loading tools and MCP servers".to_string(),
            tone: None,
        },
        AgentRunEventVisibility::Internal,
        AgentRunDisplayKind::Status,
        AgentRunEventImportance::Low,
    );
    let enabled_servers = db.get_enabled_mcp_servers().map_err(|error| {
        warn!("Failed to load enabled MCP servers: {error}");
        error
    });
    let configuration_generation = package_assembler
        .as_ref()
        .ok()
        .zip(enabled_servers.as_ref().ok())
        .and_then(|(assembler, servers)| {
            desktop_tool_registry_generation(assembler, servers)
                .map_err(|error| warn!("{error}"))
                .ok()
        });
    let mut manager = mcp_manager.lock().await;
    let generation = configuration_generation.as_ref().map(|generation| {
        format!(
            "{mcp_manager:p}:{generation}:{}",
            manager.connection_generation()
        )
    });
    let snapshot_cache = DESKTOP_TOOL_REGISTRY_SNAPSHOT.get_or_init(|| TokioMutex::new(None));
    let mut snapshot_guard = snapshot_cache.lock().await;
    let cached_tools = generation.as_ref().and_then(|generation| {
        snapshot_guard
            .as_ref()
            .filter(|snapshot| snapshot.generation == *generation)
            .map(|snapshot| snapshot.tools.clone())
    });
    let mut active_generation = cached_tools.as_ref().and(generation.clone());
    let (mut tools, mcp_sync_ms) = if let Some(tools) = cached_tools {
        (tools, 0)
    } else {
        let mut tools = package_assembler
            .as_ref()
            .map(PackageRuntimeAssembler::builtin_tool_registry)
            .unwrap_or_else(|error| {
                warn!("Failed to initialize Package Runtime Assembler: {error}");
                canonical_builtin_tool_registry()
            });
        let mcp_sync_started = Instant::now();
        let mut registry_snapshot_complete = enabled_servers.is_ok();
        if let Ok(enabled_servers) = enabled_servers.as_ref() {
            match sync_enabled_desktop_mcp_servers(
                &mut manager,
                enabled_servers,
                mcp_call_timeout_secs,
            )
            .await
            {
                Ok(errors) => {
                    registry_snapshot_complete = errors.is_empty();
                    for (server_id, error) in errors {
                        warn!("Failed to sync MCP server {server_id}: {error}");
                    }
                }
                Err(error) => {
                    registry_snapshot_complete = false;
                    warn!("Failed to sync enabled MCP servers: {error}");
                }
            }
            if let Err(error) = manager
                .register_tools_with_recovery(&mut tools, Arc::downgrade(mcp_manager))
                .await
            {
                registry_snapshot_complete = false;
                warn!("Failed to register MCP tools: {error}");
            }
        }
        let mcp_sync_ms = elapsed_ms(mcp_sync_started);
        if registry_snapshot_complete {
            if let Some(configuration_generation) = configuration_generation {
                let generation = format!(
                    "{mcp_manager:p}:{configuration_generation}:{}",
                    manager.connection_generation()
                );
                active_generation = Some(generation.clone());
                *snapshot_guard = Some(DesktopToolRegistrySnapshot {
                    generation,
                    tools: tools.clone(),
                });
            }
        }
        (tools, mcp_sync_ms)
    };
    drop(snapshot_guard);
    drop(manager);

    let delegation_runtime = {
        let mut runtime = DelegationRuntime::new(
            provider_config,
            executor_config,
            subagent_allowed_tools,
            subagent_allowed_skill_ids,
            subagent_lifecycle,
            cancel_token,
            Some(task_run_id.to_string()),
            Some(conversation_id.to_string()),
        );
        if subscription_runtime {
            runtime = runtime.require_explicit_route();
        }
        tools.register(Box::new(SubagentTool::from_runtime(runtime.clone())));
        tools.register(Box::new(SubagentModelsTool));
        tools.register(Box::new(SubagentBatchTool::from_runtime(runtime.clone())));
        if !subscription_runtime {
            tools.register(Box::new(JudgeSubagentResultsTool::from_runtime(
                runtime.clone(),
            )));
        }
        tools.register(Box::new(ObserveSubagentBatchTool::from_runtime(
            runtime.clone(),
        )));
        for lifecycle_tool in SubagentLifecycleTool::all(runtime.clone()) {
            tools.register(Box::new(lifecycle_tool));
        }
        Some(runtime)
    };
    if let Some(terminal_state) = terminal_state {
        tools.register(Box::new(TerminalAgentTool::new(terminal_state)));
    }
    tools = tools.without_names(&["browser_session"]);
    tools.register(Box::new(NativeBrowserSessionTool::new(browser_state)));
    let before_package_filter_count = tools.tool_names().len();
    let current_prefilter = tools.clone();
    let assembled = package_assembler.and_then(|assembler| assembler.assemble_tool_registry(tools));
    let package_snapshot_cache =
        DESKTOP_PACKAGE_REGISTRY_SNAPSHOT.get_or_init(|| TokioMutex::new(None));
    let mut package_snapshot_guard = package_snapshot_cache.lock().await;
    let resolution = resolve_desktop_package_registry(
        &current_prefilter,
        assembled,
        active_generation.as_deref(),
        package_snapshot_guard.as_ref(),
    );
    if let Some(snapshot) = resolution.successful_snapshot.clone() {
        *package_snapshot_guard = Some(snapshot);
    }
    drop(package_snapshot_guard);
    tools = resolution.tools;
    let after_package_filter_count = tools.tool_names().len();
    if before_package_filter_count != after_package_filter_count {
        info!(
            "Package Runtime Assembler resolved tool registry from {before_package_filter_count} to {after_package_filter_count} tools"
        );
    }
    let missing_core_tools = missing_core_runtime_tools(&tools);
    if let Some(error) = resolution.error {
        if resolution.used_last_known_good {
            warn!(
                "Failed to filter tool registry through Package Host for task run {task_run_id}: {error}; retaining the previous successfully filtered registry projection for this generation"
            );
        } else {
            warn!(
                "Failed to filter tool registry through Package Host for task run {task_run_id}: {error}; no matching successful projection exists, so the registry is failing closed"
            );
        }
    } else if missing_core_tools.is_empty() {
        info!(
            "RegistryHealth status=healthy tool_count={after_package_filter_count} missing_core_tools=[]"
        );
    } else {
        warn!(
            "RegistryHealth status=degraded tool_count={after_package_filter_count} missing_core_tools={missing_core_tools:?}; Package Host filtering remains authoritative"
        );
    }
    if plan_mode {
        let before_count = tools.tool_names().len();
        tools = tools.plan_mode_filtered();
        let after_count = tools.tool_names().len();
        info!(
            "Plan mode tool registry filtered from {before_count} to {after_count} read-only tools"
        );
        emit_agent_frontend_event(
            event_seq,
            conversation_id,
            task_run_id,
            Some(turn_id),
            AgentEvent::Status {
                content: "Plan mode active: write, execution, MCP, automation, and delegation tools are disabled."
                    .to_string(),
                tone: Some("info".to_string()),
            },
        );
    }

    if let Some(root_allowed_tools) = root_allowed_tools.as_deref() {
        let before_count = tools.tool_names().len();
        tools = filter_root_tool_registry(tools, root_allowed_tools);
        info!(
            "Root workflow tool allowlist filtered registry from {before_count} to {} tools",
            tools.tool_names().len()
        );
    }
    // Delegated workers inherit the already-filtered root registry and can
    // only narrow it further through their own role/tool policy.
    if let Some(runtime) = delegation_runtime {
        runtime.set_tool_registry(tools.clone());
    }

    DesktopAgentSessionDependencies {
        tools,
        selected_skills,
        auto_loaded_skills,
        metrics: DesktopAgentDependencyMetrics {
            skill_select_ms,
            mcp_sync_ms,
            tool_registry_ms: elapsed_ms(tool_registry_started),
        },
    }
}

pub(crate) fn filter_root_tool_registry(
    tools: ToolRegistry,
    allowed_tools: &[String],
) -> ToolRegistry {
    let normalized = allowed_tools
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    tools.filtered(&normalized)
}

pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn requires_explicit_desktop_approval(req: &ApprovalRequest) -> bool {
    req.tool_name == "computer_control"
        || req.target_kind == "screen_disclosure"
        || req.target_kind == "browser_action"
}

pub(crate) fn desktop_approval_mode_decision(
    approval_mode: ToolApprovalMode,
    req: &ApprovalRequest,
) -> Option<ApprovalDecision> {
    approval_mode.short_circuit().and_then(|decision| {
        (!requires_explicit_desktop_approval(req) || !decision.is_allowed()).then_some(decision)
    })
}

pub(crate) fn build_desktop_approval_callback(
    input: DesktopApprovalCallbackInput,
) -> ApprovalCallback {
    let DesktopApprovalCallbackInput {
        db,
        task_run_id,
        approval_runtime,
        cancellation,
    } = input;
    let pending = approval_runtime.pending;
    let session_store = approval_runtime.session_store;
    let approval_mode = approval_runtime.approval_mode;

    Arc::new(move |req: ApprovalRequest| {
        let db = Arc::clone(&db);
        let pending = Arc::clone(&pending);
        let store = session_store.clone();
        let task_run_id = task_run_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            let permission_key = ToolPermissionKey::from_request(&req);
            let hard_confirmation = requires_explicit_desktop_approval(&req);
            if let Some(decision) = desktop_approval_mode_decision(approval_mode, &req) {
                return decision;
            }

            if let Ok(Some(policy)) = db.resolve_tool_permission_policy(&permission_key) {
                if policy == "never" {
                    return ApprovalDecision::Deny;
                }
            }

            if !hard_confirmation
                && matches!(
                    store.resolve(&permission_key),
                    Some(ApprovalDecision::AllowSession)
                )
            {
                return ApprovalDecision::AllowOnce;
            }

            let (tx, rx) = tokio::sync::oneshot::channel();
            pending.lock().await.insert(
                req.id.clone(),
                PendingToolApproval {
                    task_run_id: task_run_id.clone(),
                    sender: tx,
                },
            );
            let decision = tokio::select! {
                biased;
                _ = cancellation.cancelled() => ApprovalDecision::Deny,
                decision = rx => decision.unwrap_or(ApprovalDecision::Deny),
                _ = tokio::time::sleep(Duration::from_secs(60)) => ApprovalDecision::Deny,
            };
            pending.lock().await.remove(&req.id);
            match decision {
                ApprovalDecision::AllowSession => {
                    if hard_confirmation {
                        return ApprovalDecision::AllowOnce;
                    }
                    store.set(&req.permission_key, ApprovalDecision::AllowSession);
                }
                ApprovalDecision::Never => {
                    if hard_confirmation {
                        return ApprovalDecision::Deny;
                    }
                    let key = ToolPermissionKey::from_request(&req);
                    let _ = db.save_tool_permission_policy(&key, "never");
                }
                _ => {}
            }
            decision
        })
    })
}
