use super::*;
pub(super) fn default_subagent_tool_names() -> Vec<String> {
    SUBAGENT_TOOL_SPECS
        .iter()
        .filter(|spec| spec.enabled_by_default)
        .map(|spec| spec.name.to_string())
        .collect()
}
pub(super) fn canonical_tool_name(name: &str) -> &str {
    match name {
        "compare" => "compare_documents",
        "date_search" => "search_by_date",
        other => other,
    }
}
pub(super) fn normalize_allowed_tools(
    allowed_tools: Option<&[String]>,
    available_tool_names: &[String],
) -> Vec<String> {
    let available: BTreeSet<&str> = available_tool_names.iter().map(String::as_str).collect();
    match allowed_tools {
        Some(names) => names
            .iter()
            .filter_map(|name| {
                let trimmed = canonical_tool_name(name.trim());
                available.contains(trimmed).then(|| trimmed.to_string())
            })
            .collect(),
        None => default_subagent_tool_names()
            .into_iter()
            .filter(|name| available.contains(name.as_str()))
            .collect(),
    }
}
pub(super) fn is_subagent_tool_name(name: &str) -> bool {
    matches!(
        name,
        "spawn_subagent"
            | "spawn_subagent_batch"
            | "judge_subagent_results"
            | "observe_subagent_batch"
            | "observe_subagent"
            | "wait_subagent"
            | "send_subagent_input"
            | "cancel_subagent"
            | "close_subagent"
    )
}
pub(super) fn is_interactive_surface_tool(name: &str) -> bool {
    SUBAGENT_INTERACTIVE_SURFACE_TOOLS.contains(&name)
}
pub(super) fn compatible_auxiliary_model(
    config: &AgentConfig,
    provider_config: &ProviderConfig,
) -> Option<String> {
    let provider_matches = config
        .summarization_provider_type
        .is_none_or(|provider_type| provider_type == provider_config.provider_type);
    provider_matches
        .then(|| config.summarization_model.as_deref().map(str::trim))
        .flatten()
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}
/// Route delegated phases without ever sending a model identifier to credentials
/// or an endpoint configured for a different provider. A missing compatible
/// auxiliary model is an explicit fallback to the parent model.
pub(super) fn apply_delegated_model_policy(
    config: &mut AgentConfig,
    provider_config: &ProviderConfig,
    policy: Option<&ModelRoutingClass>,
) -> bool {
    if !matches!(
        policy,
        Some(ModelRoutingClass::Fast | ModelRoutingClass::IndependentReviewer)
    ) {
        return false;
    }
    if let Some(model) = compatible_auxiliary_model(config, provider_config) {
        config.model = Some(model);
        false
    } else {
        true
    }
}
pub(super) fn apply_nexus_worker_reasoning_policy(
    config: &mut AgentConfig,
    role_profile: Option<&SubagentRoleProfile>,
) {
    if !config.power_mode.is_nexus() {
        return;
    }
    let (Some(provider), Some(model)) = (config.provider_type, config.model.as_deref()) else {
        return;
    };
    let desired = if matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    ) {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::Low
    };
    let desired_budget: u32 = if matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    ) {
        16_384
    } else {
        4_096
    };
    let Some(reasoning) = model_capabilities_from_catalog(provider, model)
        .and_then(|capabilities| capabilities.reasoning)
    else {
        // Unknown/local/custom endpoints must not inherit an unbounded parent
        // reasoning contract. Preserve an explicit off setting; otherwise
        // clamp only the control family already in use. Provider adapters still
        // decide whether that family is valid on the wire.
        if config.reasoning_enabled == Some(false)
            || config.reasoning_effort == Some(ReasoningEffort::None)
        {
            config.reasoning_enabled = Some(false);
            config.reasoning_effort = Some(ReasoningEffort::None);
            config.thinking_budget = None;
        } else if let Some(current_budget) = config.thinking_budget {
            let bounded = current_budget.min(desired_budget);
            config.reasoning_enabled = Some(bounded > 0);
            config.reasoning_effort = None;
            config.thinking_budget = Some(bounded);
        } else if config.reasoning_effort.is_some() || config.reasoning_enabled == Some(true) {
            config.reasoning_enabled = Some(true);
            config.reasoning_effort = Some(desired);
            config.thinking_budget = None;
        }
        return;
    };
    let effort_rank = |effort: &ReasoningEffort| match effort {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal => 1,
        ReasoningEffort::Low => 2,
        ReasoningEffort::Medium => 3,
        ReasoningEffort::High => 4,
        ReasoningEffort::XHigh => 5,
        ReasoningEffort::Ultra => 6,
        ReasoningEffort::Max => 6,
    };
    let supported = reasoning
        .effort_levels
        .iter()
        .filter_map(|level| ReasoningEffort::from_wire(level))
        .filter(|effort| *effort != ReasoningEffort::None)
        .collect::<Vec<_>>();
    let selected = supported
        .iter()
        .filter(|effort| effort_rank(effort) >= effort_rank(&desired))
        .min_by_key(|effort| effort_rank(effort))
        .cloned()
        .or_else(|| {
            supported
                .iter()
                .max_by_key(|effort| effort_rank(effort))
                .cloned()
        });
    if let Some(selected) = selected {
        config.reasoning_enabled = Some(true);
        config.reasoning_effort = Some(selected);
        config.thinking_budget = None;
    } else if let Some(budget) = reasoning.thinking_budget.filter(|budget| budget.enabled) {
        let bounded = desired_budget
            .max(budget.min_tokens.unwrap_or_default())
            .min(budget.max_tokens.unwrap_or(desired_budget));
        config.reasoning_enabled = Some(bounded > 0 || reasoning.mode.as_deref() == Some("always"));
        config.reasoning_effort = None;
        config.thinking_budget = Some(bounded);
    }
}
pub(super) fn apply_judge_recovery_controls(request: &mut CompletionRequest) {
    let Some(provider) = request.provider_type else {
        request.reasoning_enabled = Some(false);
        request.reasoning_effort = None;
        request.thinking_budget = None;
        return;
    };
    let reasoning = model_capabilities_from_catalog(provider, &request.model)
        .and_then(|capabilities| capabilities.reasoning);
    if reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.mode.as_deref())
        != Some("always")
    {
        request.reasoning_enabled = Some(false);
        request.reasoning_effort = (provider == ProviderType::OpenRouter
            && reasoning.as_ref().is_some_and(|reasoning| {
                reasoning.effort_levels.iter().any(|level| level == "none")
            }))
        .then_some(ReasoningEffort::None);
        request.thinking_budget = None;
        return;
    }
    request.reasoning_enabled = Some(true);
    request.reasoning_effort = reasoning
        .into_iter()
        .flat_map(|reasoning| reasoning.effort_levels)
        .filter_map(|level| ReasoningEffort::from_wire(&level))
        .find(|effort| *effort != ReasoningEffort::None);
    request.thinking_budget = None;
}
pub(super) fn resolve_delegation_timeout_secs(config: &AgentConfig, requested: Option<u32>) -> u64 {
    requested.unwrap_or_else(|| {
        let tool_timeout = config
            .tool_timeout_secs
            .filter(|timeout| *timeout > 0)
            .unwrap_or(60);
        let turn_timeout = config
            .agent_timeout_secs
            .filter(|timeout| *timeout > 0)
            .unwrap_or(180);
        tool_timeout
            .saturating_mul(2)
            .min(turn_timeout)
            .clamp(15, 180)
    }) as u64
}
pub(super) fn resolve_delegation_run_deadline_ms(
    config: &AgentConfig,
    requested_timeout_secs: Option<u32>,
    legacy_timeout_secs: u64,
    configured_run_deadline_ms: u64,
) -> u64 {
    if config.delegation_limits_v2.is_some() {
        requested_timeout_secs
            .map(|requested| configured_run_deadline_ms.min(u64::from(requested) * 1_000))
            .unwrap_or(configured_run_deadline_ms)
    } else {
        configured_run_deadline_ms.min(legacy_timeout_secs.saturating_mul(1_000))
    }
}
pub(super) fn delegated_failure_status(error_text: &str) -> &'static str {
    if error_text.contains("timed out")
        || error_text.contains("provider-connect deadline")
        || error_text.contains("first-token deadline")
        || error_text.contains("queue deadline")
    {
        "timed_out"
    } else if error_text.contains("cancelled") {
        "cancelled"
    } else {
        "failed"
    }
}
pub(super) fn estimate_subagent_timeout_secs(
    runtime: &DelegationRuntime,
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
) -> u64 {
    match args.timeout_secs {
        Some(requested) => resolve_delegation_timeout_secs(&runtime.base_config, Some(requested)),
        None => {
            let base = resolve_delegation_timeout_secs(&runtime.base_config, None);
            role_profile
                .map(|profile| base.min(profile.default_timeout_secs as u64).max(15))
                .unwrap_or(base)
        }
    }
}
pub(super) fn estimate_reserved_tokens(
    config: &AgentConfig,
    request_text: &str,
    tools: &ToolRegistry,
    inherited_context_tokens: u32,
    inherited_skill_tokens: u32,
    initial_output_credit: u32,
) -> u32 {
    let model = config.model.as_deref().unwrap_or("gpt-4o-mini");
    estimate_tokens_for_model(model, &config.system_prompt)
        .saturating_add(estimate_tokens_for_model(model, request_text))
        .saturating_add(estimate_tool_tokens_for_model(model, &tools.definitions()))
        .saturating_add(inherited_context_tokens)
        .saturating_add(inherited_skill_tokens)
        .saturating_add(initial_output_credit)
}
pub(super) fn resolve_delegated_max_output(
    config: &AgentConfig,
    catalog_limit: Option<u64>,
) -> u32 {
    let fallback_limit = u64::from(CONSERVATIVE_SUBAGENT_MAX_TOKENS);
    let effective_limit = catalog_limit
        .unwrap_or(fallback_limit)
        .min(u64::from(u32::MAX)) as u32;
    let requested_limit = config
        .max_tokens
        .unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS)
        .max(256);
    requested_limit.min(effective_limit.max(1))
}
pub(super) fn apply_delegated_model_limits(
    config: &mut AgentConfig,
    input_context_policy: DelegationLimitPolicy,
    max_output_policy: DelegationLimitPolicy,
    resolved_context: ResolvedContextWindow,
    catalog_output_limit: Option<u64>,
    independent_v2_limits: bool,
) -> ContextWindowAuthority {
    let existing_context_window = config.context_window;
    let context_authority = match input_context_policy {
        DelegationLimitPolicy::Explicit(_) => ContextWindowAuthority::UserOverride,
        DelegationLimitPolicy::Auto
            if !independent_v2_limits && existing_context_window.is_some() =>
        {
            ContextWindowAuthority::UserOverride
        }
        DelegationLimitPolicy::Auto => resolved_context.authority,
    };
    config.context_window = match input_context_policy {
        // An explicit delegated window is authoritative. In particular, never
        // clamp it to an endpoint-agnostic model-name fallback.
        DelegationLimitPolicy::Explicit(limit) => u32::try_from(limit).ok(),
        DelegationLimitPolicy::Auto if independent_v2_limits => resolved_context.capacity_tokens,
        DelegationLimitPolicy::Auto => config.context_window.or(resolved_context.capacity_tokens),
    };
    match max_output_policy {
        DelegationLimitPolicy::Explicit(limit) => {
            config.max_tokens = u32::try_from(limit).ok();
        }
        DelegationLimitPolicy::Auto if independent_v2_limits => {
            config.max_tokens = Some(
                catalog_output_limit
                    .map(|limit| {
                        limit
                            .min(u64::from(CONSERVATIVE_SUBAGENT_MAX_TOKENS))
                            .min(u64::from(u32::MAX)) as u32
                    })
                    .unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS),
            );
        }
        DelegationLimitPolicy::Auto => {}
    }
    let mut resolved_output = resolve_delegated_max_output(config, catalog_output_limit);
    if let Some(context_window) = config.context_window {
        let prompt_reserve = (context_window / 10).max(1_024).min(context_window);
        resolved_output =
            resolved_output.min(context_window.saturating_sub(prompt_reserve).max(256));
    }
    config.max_tokens = Some(resolved_output);
    context_authority
}
pub(super) fn initial_output_credit(
    role_profile: Option<&SubagentRoleProfile>,
    args: &SpawnSubagentArgs,
    config: &AgentConfig,
) -> u32 {
    let role_credit = match role_profile.map(|profile| profile.id) {
        Some("critic" | "verifier") => 4_096,
        Some("writer") => 16_384,
        Some("researcher" | "planner") => 8_192,
        _ => 8_192,
    };
    let explicit_long_form = args.deliverable_style.as_deref().is_some_and(|style| {
        let style = style.to_ascii_lowercase();
        style.contains("long") || style.contains("comprehensive")
    });
    let requested_credit = if explicit_long_form {
        32_768
    } else {
        role_credit
    };
    requested_credit.min(config.max_tokens.unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS))
}
pub(super) fn build_subagent_executor_tools(
    runtime: &DelegationRuntime,
    allowed_tool_names: &[String],
    worker_cancel_token: &CancellationToken,
) -> Result<ToolRegistry, CoreError> {
    let filtered = runtime
        .get_tool_registry()?
        .filtered(allowed_tool_names)
        .without_names(SUBAGENT_INTERACTIVE_SURFACE_TOOLS)
        .without_names(&[
            "spawn_subagent",
            "spawn_subagent_batch",
            "judge_subagent_results",
            "observe_subagent_batch",
            "observe_subagent",
            "wait_subagent",
            "send_subagent_input",
            "cancel_subagent",
            "close_subagent",
        ]);
    if !runtime.can_delegate_further() {
        return Ok(filtered);
    }
    let child_runtime = runtime.spawn_child_runtime(worker_cancel_token.child_token());
    let mut registry = filtered;
    if allowed_tool_names
        .iter()
        .any(|name| name == "spawn_subagent")
    {
        registry.register(Box::new(SubagentTool::from_runtime(child_runtime.clone())));
    }
    if allowed_tool_names
        .iter()
        .any(|name| name == "observe_subagent_batch")
    {
        registry.register(Box::new(ObserveSubagentBatchTool::from_runtime(
            child_runtime.clone(),
        )));
    }
    if allowed_tool_names
        .iter()
        .any(|name| name == "spawn_subagent_batch")
    {
        registry.register(Box::new(SubagentBatchTool::from_runtime(
            child_runtime.clone(),
        )));
    }
    if allowed_tool_names
        .iter()
        .any(|name| name == "judge_subagent_results")
    {
        registry.register(Box::new(JudgeSubagentResultsTool::from_runtime(
            child_runtime.clone(),
        )));
    }
    for lifecycle_tool in SubagentLifecycleTool::all(child_runtime.clone()) {
        if allowed_tool_names
            .iter()
            .any(|name| name == lifecycle_tool.name())
        {
            registry.register(Box::new(lifecycle_tool));
        }
    }
    Ok(registry)
}
