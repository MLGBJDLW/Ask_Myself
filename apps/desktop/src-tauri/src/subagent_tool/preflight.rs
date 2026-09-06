use super::*;
pub(super) fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}
pub(super) fn normalize_string_list(
    value: Option<Vec<String>>,
    limit: usize,
) -> Option<Vec<String>> {
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for item in value.unwrap_or_default() {
        let trimmed = item.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        normalized.push(trimmed.to_string());
        if normalized.len() >= limit {
            break;
        }
    }
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}
pub(super) fn truncate_excerpt(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let mut cut = max_chars;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let trimmed = content[..cut].trim_end();
    format!("{trimmed}...[truncated]")
}
pub(super) fn applied_skills(skills: &[Skill]) -> Vec<AppliedSkillRef> {
    skills
        .iter()
        .map(|skill| AppliedSkillRef {
            id: skill.id.clone(),
            name: skill.name.clone(),
        })
        .collect()
}
pub(super) fn filter_enabled_skills(
    skills: &[Skill],
    allowed_skill_ids: Option<&[String]>,
) -> Vec<Skill> {
    match allowed_skill_ids {
        Some(ids) => {
            let allowed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            skills
                .iter()
                .filter(|skill| allowed.contains(skill.id.as_str()))
                .cloned()
                .collect()
        }
        None => skills.to_vec(),
    }
}
pub(super) fn load_skill_index_snapshot(db: &Database) -> SkillIndexSnapshot {
    let mut skills = nexa_core::skills::load_builtin_skills();
    skills.extend(db.get_enabled_skills().unwrap_or_default());
    let encoded = serde_json::to_vec(&skills).unwrap_or_default();
    SkillIndexSnapshot {
        generation: blake3::hash(&encoded).to_hex().to_string(),
        skills: Arc::from(skills),
    }
}
pub(super) fn load_delegation_context_snapshot(
    db: &Database,
    conversation_id: Option<&str>,
    model: &str,
    context_limit: Option<u32>,
    handoff_token_budget: u32,
) -> DelegationContextSnapshot {
    // `context_limit` is model capacity; handoff is a separate parent-history
    // allocation. Never report the smaller allocation as the model window.
    let token_budget = context_limit.map_or(handoff_token_budget, |limit| {
        handoff_token_budget.min(limit)
    });
    let mut selected = Vec::new();
    let mut token_estimate = 0u32;
    let mut dropped_invalid_messages = 0usize;
    if let Some(conversation_id) = conversation_id {
        if let Ok(messages) = db.get_messages(conversation_id) {
            for message in messages
                .into_iter()
                .filter(nexa_core::conversation::conversation_message_is_model_history)
                .rev()
            {
                // Delegate conversational intent and final assistant output, not
                // provider-specific tool protocol records. Evidence needed by a
                // child is handed off through typed evidence cards instead.
                if message.role == Role::Tool {
                    continue;
                }
                let content = conversation_message_llm_context_content(&message).to_string();
                let message_tokens = estimate_tokens_for_model(model, &content);
                if message_tokens > token_budget {
                    dropped_invalid_messages = dropped_invalid_messages.saturating_add(1);
                    continue;
                }
                if !selected.is_empty()
                    && token_estimate.saturating_add(message_tokens) > token_budget
                {
                    break;
                }
                let mut projected = Message::text(message.role.clone(), content);
                if message.role == Role::Assistant {
                    projected.reasoning_content =
                        nexa_core::conversation::conversation_message_reasoning_replay(&message);
                    if let Some(envelope) = conversation_message_provider_turn(&message) {
                        projected.set_provider_turn(envelope);
                    }
                }
                let context = MessageNormalizationContext {
                    provider: None,
                    model: Some(model),
                    conversation_id: Some(conversation_id),
                    turn_id: None,
                    message_index: selected.len(),
                    source: MessageSource::SubagentHandoff,
                    invalid_assistant: InvalidAssistantHandling::Drop,
                };
                match normalize_assistant_message(projected, &context) {
                    Ok(Some(projected)) => {
                        token_estimate = token_estimate.saturating_add(message_tokens);
                        selected.push((message.id, projected));
                    }
                    Ok(None) | Err(_) => {
                        dropped_invalid_messages = dropped_invalid_messages.saturating_add(1);
                    }
                }
            }
        }
    }
    selected.reverse();
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    match context_limit {
        Some(limit) => {
            hasher.update(&[1]);
            hasher.update(&limit.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hasher.update(&handoff_token_budget.to_le_bytes());
    for (id, message) in &selected {
        hasher.update(id.as_bytes());
        hasher.update(message.text_content().as_bytes());
        if let Some(tool_calls) = message.tool_calls.as_ref() {
            hasher.update(&serde_json::to_vec(tool_calls).unwrap_or_default());
        }
    }
    let (selected_message_ids, messages): (Vec<_>, Vec<_>) = selected.into_iter().unzip();
    DelegationContextSnapshot {
        id: hasher.finalize().to_hex().to_string(),
        selected_message_ids: Arc::from(selected_message_ids),
        messages: Arc::from(messages),
        token_estimate,
        context_limit,
        handoff_token_budget: token_budget,
        dropped_invalid_messages,
    }
}
pub(super) fn normalize_role_id(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}
pub(super) fn role_profile_by_id(role_id: &str) -> Option<&'static SubagentRoleProfile> {
    let normalized = normalize_role_id(role_id);
    SUBAGENT_ROLE_PROFILES
        .iter()
        .find(|profile| profile.id == normalized)
}
pub(super) fn infer_role_profile(role: Option<&str>) -> Option<&'static SubagentRoleProfile> {
    let text = role?.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    SUBAGENT_ROLE_PROFILES.iter().find(|profile| {
        text.contains(profile.id) || text.contains(&profile.label.to_ascii_lowercase())
    })
}
pub(super) fn resolve_role_profile(
    role_id: Option<&str>,
    role: Option<&str>,
) -> Result<Option<&'static SubagentRoleProfile>, CoreError> {
    if let Some(raw_id) = role_id.map(str::trim).filter(|value| !value.is_empty()) {
        return role_profile_by_id(raw_id).map(Some).ok_or_else(|| {
            let allowed = SUBAGENT_ROLE_PROFILES
                .iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>()
                .join(", ");
            CoreError::InvalidInput(format!(
                "Unknown subagent role_id '{raw_id}'. Allowed role_id values: {allowed}."
            ))
        });
    }
    Ok(infer_role_profile(role))
}
pub(super) fn role_id_values() -> Vec<&'static str> {
    SUBAGENT_ROLE_PROFILES
        .iter()
        .map(|profile| profile.id)
        .collect()
}
pub(super) fn normalize_workflow_template_id(
    value: Option<String>,
) -> Result<Option<String>, CoreError> {
    let Some(raw_template) = trim_optional(value) else {
        return Ok(None);
    };
    let normalized = normalize_role_id(&raw_template);
    if workflow_template_by_id(&normalized).is_some() {
        return Ok(Some(normalized));
    }
    let allowed = workflow_template_id_values().join(", ");
    Err(CoreError::InvalidInput(format!(
        "Unknown workflow_template '{raw_template}'. Allowed workflow_template values: {allowed}."
    )))
}
pub(super) fn expand_workflow_template_tasks(
    template: &WorkflowTemplateDefinition,
    batch_goal: &str,
    parallel_group: Option<&str>,
) -> Vec<BatchSubagentTaskArgs> {
    let shared_context = format!(
        "Workflow template: {} ({})\n{}\n\nOverall batch goal:\n{}",
        template.label,
        template.id,
        template.description,
        batch_goal.trim()
    );
    let group = parallel_group
        .map(str::to_string)
        .unwrap_or_else(|| template.id.to_string());
    template
        .tasks
        .iter()
        .map(|task_template| {
            let profile = role_profile_by_id(task_template.role_id);
            let mut acceptance_criteria = task_template
                .acceptance_criteria
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>();
            acceptance_criteria.push(
                "Tie the result back to the overall batch goal and state unresolved uncertainty."
                    .to_string(),
            );
            BatchSubagentTaskArgs {
                route: SubagentRouteArgs::default(),
                id: Some(format!("{}-{}", template.id, task_template.id)),
                task_id: None,
                task: format!(
                    "Overall goal:\n{}\n\nTemplate step:\n{}",
                    batch_goal.trim(),
                    task_template.task
                ),
                role_id: Some(task_template.role_id.to_string()),
                role: profile.map(|profile| profile.label.to_string()),
                model_policy: None,
                context: Some(shared_context.clone()),
                expected_output: Some(task_template.expected_output.to_string()),
                max_iterations: None,
                timeout_secs: None,
                acceptance_criteria: Some(acceptance_criteria),
                evidence_chunk_ids: None,
                source_ids: None,
                allowed_tools: None,
                parallel_group: Some(group.clone()),
                deliverable_style: Some(task_template.deliverable_style.to_string()),
                return_sections: Some(role_sections(profile)),
            }
        })
        .collect()
}
pub(super) fn role_sections(profile: Option<&SubagentRoleProfile>) -> Vec<String> {
    profile
        .map(|profile| {
            profile
                .default_sections
                .iter()
                .map(|section| (*section).to_string())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "Conclusion".to_string(),
                "Key evidence or reasoning".to_string(),
                "Risks or open questions".to_string(),
            ]
        })
}
pub(super) fn build_subagent_system_prompt(
    base_prompt: &str,
    role: Option<&str>,
    role_profile: Option<&SubagentRoleProfile>,
) -> String {
    let mut prompt = base_prompt.trim().to_string();
    prompt.push_str("\n\n## Subagent Instructions\n\n");
    prompt.push_str(
        "You are a short-lived worker spawned by another agent. Focus only on the delegated subtask. Keep your work scoped, use tools only when they materially help, and return a compact result for the supervisor agent rather than addressing the end user directly.",
    );
    prompt.push_str(
        "\n\nTreat supervisor-provided acceptance criteria as requirements. If explicit evidence handoff is provided, ground your answer in that evidence before doing broader retrieval. If you are one of several parallel workers, produce an independent result instead of speculating about what sibling workers might find.",
    );
    if let Some(profile) = role_profile {
        prompt.push_str("\n\n## Role Profile\n\n");
        prompt.push_str("- id: ");
        prompt.push_str(profile.id);
        prompt.push_str("\n- label: ");
        prompt.push_str(profile.label);
        prompt.push_str("\n- instructions: ");
        prompt.push_str(profile.instructions);
        prompt.push_str(
            "\n\nFollow this profile even when the free-form role text is vague. If the profile and free-form role conflict, prefer the profile.",
        );
    }
    if let Some(role) = role.map(str::trim).filter(|value| !value.is_empty()) {
        prompt.push_str("\n\n## Assigned Role\n\n");
        prompt.push_str(role);
    }
    prompt
}
pub(super) fn build_return_sections(
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
) -> Vec<String> {
    normalize_string_list(args.return_sections.clone(), 8)
        .unwrap_or_else(|| role_sections(role_profile))
}
pub(super) fn resolve_source_scope(
    parent_scope: &[String],
    requested_scope: Option<&[String]>,
) -> Vec<String> {
    match requested_scope {
        Some(requested) => {
            if parent_scope.is_empty() {
                requested.to_vec()
            } else {
                let parent: BTreeSet<&str> = parent_scope.iter().map(String::as_str).collect();
                let narrowed: Vec<String> = requested
                    .iter()
                    .filter(|id| parent.contains(id.as_str()))
                    .cloned()
                    .collect();
                narrowed
            }
        }
        _ => parent_scope.to_vec(),
    }
}
pub(super) fn resolve_allowed_tools(
    base_allowed_tools: &[String],
    requested_allowed_tools: Option<&[String]>,
) -> Vec<String> {
    match requested_allowed_tools {
        Some(requested) => {
            let allowed: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
            let narrowed: Vec<String> = requested
                .iter()
                .filter(|name| allowed.contains(name.as_str()))
                .cloned()
                .collect();
            narrowed
        }
        _ => base_allowed_tools.to_vec(),
    }
}
pub(super) fn resolve_allowed_tools_for_role(
    base_allowed_tools: &[String],
    requested_allowed_tools: Option<&[String]>,
    role_profile: Option<&SubagentRoleProfile>,
) -> Vec<String> {
    // Role recommendations supply defaults, not a second hidden permission
    // boundary. Explicit selections may use any tool granted by the parent.
    if requested_allowed_tools.is_some() {
        return resolve_allowed_tools(base_allowed_tools, requested_allowed_tools);
    }
    let role_allowed_tools = match role_profile {
        Some(profile) => {
            let base: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
            profile
                .recommended_tools
                .iter()
                .filter(|name| base.contains(**name))
                .map(|name| (*name).to_string())
                .collect()
        }
        None => base_allowed_tools.to_vec(),
    };
    resolve_allowed_tools(&role_allowed_tools, requested_allowed_tools)
}
pub(super) const SUBAGENT_PREFLIGHT_SCHEMA_VERSION: u32 = 1;
pub(super) const SUBAGENT_PREFLIGHT_MARKER: &str = "NEXA_SUBAGENT_PREFLIGHT=";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SubagentPreflightStage {
    History,
    Provider,
    Policy,
    Budget,
    Timeout,
}
impl std::fmt::Display for SubagentPreflightStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::History => "history",
            Self::Provider => "provider",
            Self::Policy => "policy",
            Self::Budget => "budget",
            Self::Timeout => "timeout",
        };
        formatter.write_str(value)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubagentPreflightFailure {
    pub(super) schema_version: u32,
    pub(super) stage: SubagentPreflightStage,
    pub(super) code: String,
    pub(super) retryable: bool,
    pub(super) message: String,
}
pub(super) fn subagent_preflight_failure(
    stage: SubagentPreflightStage,
    code: &str,
    retryable: bool,
    message: impl Into<String>,
) -> CoreError {
    let failure = SubagentPreflightFailure {
        schema_version: SUBAGENT_PREFLIGHT_SCHEMA_VERSION,
        stage,
        code: code.to_string(),
        retryable,
        message: message.into(),
    };
    let encoded = serde_json::to_string(&failure).unwrap_or_else(|_| "{}".to_string());
    CoreError::InvalidInput(format!(
        "Subagent preflight failed at {} ({}): {}\n{SUBAGENT_PREFLIGHT_MARKER}{encoded}",
        failure.stage, failure.code, failure.message
    ))
}
pub(super) fn subagent_preflight_failure_from_error(
    error: &CoreError,
) -> Option<SubagentPreflightFailure> {
    let CoreError::InvalidInput(message) = error else {
        return None;
    };
    let encoded = message.split_once(SUBAGENT_PREFLIGHT_MARKER)?.1.trim();
    serde_json::from_str(encoded).ok()
}
pub(super) fn subagent_admission_failure(error: &CoreError) -> CoreError {
    let message = error.to_string();
    if message.contains("queue deadline") {
        subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "queue_deadline_exceeded",
            true,
            message,
        )
    } else if message.contains("cancelled while waiting") {
        subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "queue_wait_cancelled",
            false,
            message,
        )
    } else {
        subagent_preflight_failure(
            SubagentPreflightStage::Budget,
            "admission_rejected",
            false,
            message,
        )
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubagentPreflightReport {
    pub(super) schema_version: u32,
    pub(super) completed_stages: Vec<SubagentPreflightStage>,
    pub(super) provider_id: String,
    pub(super) effective_model: String,
    pub(super) inherited_tool_count: usize,
    pub(super) requested_tool_count: usize,
    pub(super) effective_tool_count: usize,
    pub(super) requested_source_count: usize,
    pub(super) effective_source_count: usize,
    pub(super) context_message_count: usize,
    pub(super) dropped_invalid_context_messages: usize,
    pub(super) reserved_tokens: u32,
    pub(super) remaining_token_budget: u32,
    pub(super) remaining_call_budget: u32,
    pub(super) run_deadline_ms: u64,
}
pub(super) fn validate_subagent_preflight(
    args: &SpawnSubagentArgs,
    effective_model: &str,
    provider_id: &str,
    baseline_allowed_tools: &[String],
    effective_allowed_tools: &[String],
    inherited_source_scope: &[String],
    effective_source_scope: &[String],
    context_snapshot: &DelegationContextSnapshot,
) -> Result<SubagentPreflightReport, CoreError> {
    let history_context = MessageNormalizationContext {
        provider: Some(provider_id),
        model: Some(effective_model),
        conversation_id: None,
        turn_id: None,
        message_index: 0,
        source: MessageSource::SubagentHandoff,
        invalid_assistant: InvalidAssistantHandling::Reject,
    };
    validate_message_sequence(&context_snapshot.messages, history_context).map_err(|error| {
        subagent_preflight_failure(
            SubagentPreflightStage::History,
            "inherited_history_invalid",
            false,
            error.to_string(),
        )
    })?;
    if let Some(requested) = args.allowed_tools.as_deref() {
        let interactive: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|name| is_interactive_surface_tool(name))
            .collect();
        if !interactive.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "interactive_tool_requires_parent_proxy",
                false,
                format!(
                    "Delegated workers cannot directly control interactive browser or desktop surfaces: {}. Ask the parent agent to perform the approved action.",
                    interactive.join(", ")
                ),
            ));
        }
        let inherited: BTreeSet<&str> = baseline_allowed_tools.iter().map(String::as_str).collect();
        let denied: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|name| !inherited.contains(name))
            .collect();
        if !denied.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_scope_widening",
                false,
                format!(
                    "Requested tool(s) are not available to the parent: {}.",
                    denied.join(", ")
                ),
            ));
        }
        if !requested.is_empty() && effective_allowed_tools.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_scope_empty_after_narrowing",
                false,
                "No requested tools remain after role and delegation-depth restrictions.",
            ));
        }
    }
    if let Some(requested) = args.source_ids.as_deref() {
        if !inherited_source_scope.is_empty() {
            let inherited: BTreeSet<&str> =
                inherited_source_scope.iter().map(String::as_str).collect();
            let denied: Vec<&str> = requested
                .iter()
                .map(String::as_str)
                .filter(|source_id| !inherited.contains(source_id))
                .collect();
            if !denied.is_empty() {
                return Err(subagent_preflight_failure(
                    SubagentPreflightStage::Policy,
                    "source_scope_widening",
                    false,
                    format!(
                        "Requested source(s) are outside the parent scope: {}.",
                        denied.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(SubagentPreflightReport {
        schema_version: SUBAGENT_PREFLIGHT_SCHEMA_VERSION,
        completed_stages: vec![
            SubagentPreflightStage::History,
            SubagentPreflightStage::Provider,
            SubagentPreflightStage::Policy,
        ],
        provider_id: provider_id.to_string(),
        effective_model: effective_model.to_string(),
        inherited_tool_count: baseline_allowed_tools.len(),
        requested_tool_count: args.allowed_tools.as_ref().map_or(0, Vec::len),
        effective_tool_count: effective_allowed_tools.len(),
        requested_source_count: args.source_ids.as_ref().map_or(0, Vec::len),
        effective_source_count: effective_source_scope.len(),
        context_message_count: context_snapshot.messages.len(),
        dropped_invalid_context_messages: context_snapshot.dropped_invalid_messages,
        reserved_tokens: 0,
        remaining_token_budget: 0,
        remaining_call_budget: 0,
        run_deadline_ms: 0,
    })
}
pub(super) fn finalize_subagent_preflight(
    report: &mut SubagentPreflightReport,
    budget: &BudgetSnapshot,
    reserved_tokens: u32,
    run_deadline_ms: u64,
) -> Result<(), CoreError> {
    if budget.remaining_calls == 0 {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Budget,
            "call_budget_exhausted",
            false,
            "No delegated call budget remains for this turn.",
        ));
    }
    report.reserved_tokens = reserved_tokens;
    report.remaining_token_budget = budget.remaining_tokens;
    report.remaining_call_budget = budget.remaining_calls;
    report.completed_stages.push(SubagentPreflightStage::Budget);
    if run_deadline_ms == 0 {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "run_deadline_invalid",
            false,
            "The delegated run deadline is zero.",
        ));
    }
    report.run_deadline_ms = run_deadline_ms;
    report
        .completed_stages
        .push(SubagentPreflightStage::Timeout);
    Ok(())
}
pub(super) fn build_evidence_handoff(
    db: &Database,
    chunk_ids: Option<&[String]>,
) -> Vec<EvidenceHandoffItem> {
    chunk_ids
        .unwrap_or(&[])
        .iter()
        .take(8)
        .filter_map(|chunk_id| {
            let card = search::get_evidence_card(db, chunk_id).ok()?;
            Some(EvidenceHandoffItem {
                chunk_id: card.chunk_id.to_string(),
                path: card.document_path,
                title: card.document_title,
                excerpt: truncate_excerpt(&card.content, 1400),
            })
        })
        .collect()
}
