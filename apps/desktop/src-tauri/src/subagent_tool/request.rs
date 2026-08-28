use super::*;
pub(super) fn build_subagent_request(
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
    effective_source_scope: &[String],
    effective_allowed_tools: &[String],
    allowed_skills: &[AppliedSkillRef],
    evidence_handoff: &[EvidenceHandoffItem],
    previous_session: Option<&SubagentSessionSnapshot>,
) -> String {
    let sections = build_return_sections(args, role_profile);
    let mut request = String::from(
        "Complete the delegated task below. If information is missing, make the smallest reasonable assumption, state it briefly, and continue.\n\n## Supervisor Handoff Packet\n",
    );
    request.push_str("```json\n");
    request.push_str(
        &serde_json::to_string_pretty(&serde_json::json!({
            "task": args.task.trim(),
            "roleId": role_profile.map(|profile| profile.id),
            "roleName": role_profile.map(|profile| profile.label),
            "role": args.role,
            "parallelGroup": args.parallel_group,
            "expectedOutput": args.expected_output,
            "deliverableStyle": args.deliverable_style,
            "requiredSections": sections,
            "acceptanceCriteria": args.acceptance_criteria,
            "sourceScope": effective_source_scope,
            "allowedTools": effective_allowed_tools,
            "allowedSkills": allowed_skills,
            "evidenceChunkIds": args.evidence_chunk_ids,
            "taskId": args.task_id,
            "resumingTaskId": previous_session.map(|snapshot| snapshot.task_id.as_str()),
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    request.push_str("\n```\n\n## Delegated Task\n");
    request.push_str(args.task.trim());
    if let Some(profile) = role_profile {
        request.push_str("\n\nAssigned role profile:\n");
        request.push_str(profile.label);
        request.push_str(" (");
        request.push_str(profile.id);
        request.push_str(")\n");
        request.push_str(profile.instructions);
    }
    if let Some(role) = args
        .role
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\nRequested perspective:\n");
        request.push_str(role);
    }
    if let Some(group) = args
        .parallel_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\nParallel group:\n");
        request.push_str(group);
        request.push_str(
            "\nTreat this as an independent branch of work. Do not assume what sibling workers will conclude.",
        );
    }
    if let Some(context) = args
        .context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Supervisor Context\n");
        request.push_str(&truncate_excerpt(context, 4_000));
    }
    if let Some(snapshot) = previous_session {
        request.push_str("\n\n## Resumed Subagent Session\n");
        request.push_str("You are continuing a previous delegated session with task_id `");
        request.push_str(&snapshot.task_id);
        request.push_str("`. Treat the prior result as context, not as final truth.\n\n");
        request.push_str("Previous task:\n");
        request.push_str(&truncate_excerpt(&snapshot.task, 1_000));
        request.push_str("\n\nPrevious result:\n");
        request.push_str(&truncate_excerpt(&snapshot.result, 4_000));
    }
    if let Some(expected_output) = args
        .expected_output
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Desired Output\n");
        request.push_str(expected_output);
    }
    if let Some(style) = args
        .deliverable_style
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Deliverable Style\n");
        request.push_str(style);
    }
    if let Some(criteria) = args
        .acceptance_criteria
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        request.push_str("\n\n## Acceptance Criteria\n");
        for item in criteria {
            request.push_str("- ");
            request.push_str(item);
            request.push('\n');
        }
    }
    if !effective_source_scope.is_empty() {
        request.push_str("\n## Source Scope Restriction\n");
        for source_id in effective_source_scope {
            request.push_str("- ");
            request.push_str(source_id);
            request.push('\n');
        }
    }
    if !effective_allowed_tools.is_empty() {
        request.push_str("\n## Delegated Tool Access\n");
        for tool_name in effective_allowed_tools {
            request.push_str("- ");
            request.push_str(tool_name);
            request.push('\n');
        }
    }
    if !allowed_skills.is_empty() {
        request.push_str("\n## Delegated Skills\n");
        for skill in allowed_skills {
            request.push_str("- ");
            request.push_str(&skill.name);
            request.push_str(" (");
            request.push_str(&skill.id);
            request.push_str(")\n");
        }
    }
    if !evidence_handoff.is_empty() {
        request.push_str("\n## Evidence Handoff\n");
        for evidence in evidence_handoff {
            request.push_str(&format!(
                "\n--- Evidence ---\n[chunk_id: {}]\nPath: {}\nTitle: {}\nExcerpt:\n{}\n",
                evidence.chunk_id, evidence.path, evidence.title, evidence.excerpt
            ));
        }
    }
    request.push_str("\n\n## Response Contract\nReturn a concise result with these sections:\n");
    for (index, section) in sections.iter().enumerate() {
        request.push_str(&format!("{}. {}\n", index + 1, section));
    }
    request.push_str(
        "\nGround claims in the handed-off evidence or retrieved data. If source scope or tool access prevents certainty, state that plainly instead of guessing.",
    );
    request
}
pub(super) fn normalize_spawn_args(
    mut args: SpawnSubagentArgs,
) -> Result<SpawnSubagentArgs, CoreError> {
    args.task = args.task.trim().to_string();
    if args.task.is_empty() {
        return Err(CoreError::InvalidInput(
            "spawn_subagent requires a non-empty task".into(),
        ));
    }
    args.role_id = trim_optional(args.role_id).map(|role_id| normalize_role_id(&role_id));
    resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())?;
    args.role = trim_optional(args.role);
    args.task_id = trim_optional(args.task_id);
    args.context = trim_optional(args.context);
    args.expected_output = trim_optional(args.expected_output);
    args.parallel_group = trim_optional(args.parallel_group);
    args.deliverable_style = trim_optional(args.deliverable_style);
    args.timeout_secs = args.timeout_secs.map(|value| value.clamp(15, 180));
    args.acceptance_criteria = normalize_string_list(args.acceptance_criteria.take(), 8);
    args.evidence_chunk_ids = normalize_string_list(args.evidence_chunk_ids.take(), 8);
    args.source_ids = normalize_string_list(args.source_ids.take(), 16);
    args.allowed_tools = normalize_string_list(args.allowed_tools.take(), 16);
    args.return_sections = normalize_string_list(args.return_sections.take(), 8);
    Ok(args)
}
pub(super) fn normalize_batch_task_args(
    task: BatchSubagentTaskArgs,
) -> Result<(Option<String>, SpawnSubagentArgs), CoreError> {
    let worker_id = trim_optional(task.id);
    let args = normalize_spawn_args(SpawnSubagentArgs {
        task: task.task,
        task_id: task.task_id,
        role_id: task.role_id,
        role: task.role,
        model_policy: task.model_policy,
        context: task.context,
        expected_output: task.expected_output,
        max_iterations: task.max_iterations,
        timeout_secs: task.timeout_secs,
        acceptance_criteria: task.acceptance_criteria,
        evidence_chunk_ids: task.evidence_chunk_ids,
        source_ids: task.source_ids,
        allowed_tools: task.allowed_tools,
        parallel_group: task.parallel_group,
        deliverable_style: task.deliverable_style,
        return_sections: task.return_sections,
    })?;
    Ok((worker_id, args))
}
pub(super) async fn emit_subagent_lifecycle_event(
    bridge: Option<&SubagentEventBridge>,
    kind: SubagentLifecycleEventKind,
    detail: serde_json::Value,
) {
    let Some(bridge) = bridge else {
        return;
    };
    if let Err(error) = bridge.emit(kind, detail).await {
        warn!("Failed to bridge subagent lifecycle event {kind:?}: {error}");
    }
}
pub(super) async fn flush_subagent_deltas(
    bridge: Option<&SubagentEventBridge>,
    pending_thinking: &mut String,
    pending_output: &mut String,
) {
    if !pending_thinking.is_empty() {
        let delta = std::mem::take(pending_thinking);
        emit_subagent_lifecycle_event(
            bridge,
            SubagentLifecycleEventKind::ThinkingDelta,
            serde_json::json!({ "delta": delta }),
        )
        .await;
    }
    if !pending_output.is_empty() {
        let delta = std::mem::take(pending_output);
        emit_subagent_lifecycle_event(
            bridge,
            SubagentLifecycleEventKind::OutputDelta,
            serde_json::json!({ "delta": delta }),
        )
        .await;
    }
}
