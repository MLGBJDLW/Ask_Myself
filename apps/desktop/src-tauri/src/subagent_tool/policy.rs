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
/// Only explicit task budgets constrain delegated execution. Tool timeouts
/// describe individual tool operations, not the lifetime of a worker.
pub(super) fn resolve_delegation_run_deadline_ms(
    config: &AgentConfig,
    requested_timeout_secs: Option<u32>,
    configured_run_deadline_ms: Option<u64>,
) -> Option<u64> {
    [
        config
            .agent_timeout_secs
            .filter(|value| *value > 0)
            .map(|secs| u64::from(secs) * 1_000),
        requested_timeout_secs.map(|secs| u64::from(secs) * 1_000),
        configured_run_deadline_ms,
    ]
    .into_iter()
    .flatten()
    .min()
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
) -> Option<u32> {
    let requested = u64::from(config.max_tokens?);
    Some(
        requested
            .min(catalog_limit.unwrap_or(u64::MAX))
            .min(u64::from(u32::MAX)) as u32,
    )
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
        DelegationLimitPolicy::Auto => {
            // Preserve automatic provenance. The executor resolves model
            // capability and prompt headroom together; copying a catalog cap
            // into this field would masquerade as an explicit override and
            // could reserve the entire context window for output.
            config.max_tokens = None;
        }
    }
    let mut resolved_output = resolve_delegated_max_output(config, catalog_output_limit);
    if let Some(context_window) = config.context_window {
        resolved_output = resolved_output.map(|output| output.min(context_window));
    }
    config.max_tokens = resolved_output;
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
        Some("researcher" | "planner") => INITIAL_SUBAGENT_OUTPUT_CREDIT,
        _ => INITIAL_SUBAGENT_OUTPUT_CREDIT,
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
    requested_credit.min(config.max_tokens.unwrap_or(requested_credit))
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
    if runtime.delegation_depth.saturating_add(1) >= MAX_SUBAGENT_DELEGATION_DEPTH {
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
