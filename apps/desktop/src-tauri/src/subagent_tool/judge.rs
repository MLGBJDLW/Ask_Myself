use super::*;
pub(super) fn extract_json_block(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))?
        .trim();
    fenced.strip_suffix("```").map(str::trim)
}
pub(super) fn build_judge_system_prompt(base_prompt: &str) -> String {
    let mut prompt = base_prompt.trim().to_string();
    prompt.push_str("\n\n## Adjudicator Instructions\n\n");
    prompt.push_str(
        "You are an adjudicator reviewing delegated worker outputs. Compare candidates strictly against the supplied rubric and return a compact, structured judgement. Do not invent evidence beyond the candidate content you were given.",
    );
    prompt
}
pub(super) fn build_judge_request(args: &JudgeSubagentResultsArgs) -> String {
    let mut request = String::new();
    if let Some(task) = args
        .task
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Adjudication task:\n");
        request.push_str(task);
        request.push_str("\n\n");
    }
    request.push_str("Decision mode:\n");
    request.push_str(
        args.decision_mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("single_best"),
    );
    request.push_str("\n\n");
    if let Some(expected_output) = args
        .expected_output
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Expected output:\n");
        request.push_str(expected_output);
        request.push_str("\n\n");
    }
    if let Some(group) = args
        .parallel_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Parallel group:\n");
        request.push_str(group);
        request.push_str("\n\n");
    }
    if let Some(rubric) = args.rubric.as_ref().filter(|items| !items.is_empty()) {
        request.push_str("Rubric:\n");
        for item in rubric {
            request.push_str("- ");
            request.push_str(item);
            request.push('\n');
        }
        request.push('\n');
    }
    request.push_str("Candidates:\n");
    for candidate in &args.candidates {
        request.push_str(&format!(
            "\n--- Candidate {} ---\n",
            candidate.label.as_deref().unwrap_or(&candidate.id)
        ));
        request.push_str(&format!("id: {}\n", candidate.id));
        request.push_str("result:\n");
        request.push_str(candidate.result.trim());
        request.push('\n');
        if let Some(evidence_summary) = candidate
            .evidence_summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request.push_str("evidence summary:\n");
            request.push_str(evidence_summary);
            request.push('\n');
        }
        if let Some(concerns) = candidate
            .concerns
            .as_ref()
            .filter(|items| !items.is_empty())
        {
            request.push_str("concerns:\n");
            for concern in concerns {
                request.push_str("- ");
                request.push_str(concern);
                request.push('\n');
            }
        }
    }
    let required_winners = args.required_winner_count.unwrap_or(1).clamp(1, 4);
    request.push_str(
        "\nReturn ONLY JSON with this shape:\n{\"winnerIds\":[\"candidate-id\"],\"confidence\":\"high|medium|low\",\"summary\":\"short final recommendation\",\"rationale\":\"why these candidates won\"}\n",
    );
    request.push_str(&format!(
        "Select exactly {required_winners} winner id(s) unless the evidence clearly supports a tie."
    ));
    request
}
#[async_trait]
impl Tool for JudgeSubagentResultsTool {
    fn name(&self) -> &str {
        "judge_subagent_results"
    }
    fn description(&self) -> &str {
        &delegation_tool_def(&JUDGE_SUBAGENT_RESULTS_DEF, JUDGE_SUBAGENT_RESULTS_JSON).description
    }
    fn parameters_schema(&self) -> serde_json::Value {
        delegation_tool_def(&JUDGE_SUBAGENT_RESULTS_DEF, JUDGE_SUBAGENT_RESULTS_JSON)
            .parameters
            .clone()
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let launch_started = Instant::now();
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            conversation_id,
            ..
        } = context;
        let mut args: JudgeSubagentResultsArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid judge_subagent_results arguments: {e}"))
        })?;
        if args.candidates.len() < 2 {
            return Err(CoreError::InvalidInput(
                "judge_subagent_results requires at least two candidates".into(),
            ));
        }
        args.task = trim_optional(args.task);
        args.expected_output = trim_optional(args.expected_output);
        args.parallel_group = trim_optional(args.parallel_group);
        args.decision_mode = trim_optional(args.decision_mode);
        args.rubric = normalize_string_list(args.rubric.take(), 8);
        let provider = create_provider(self.runtime.provider_config.clone())
            .map_err(|e| CoreError::Llm(e.to_string()))?;
        let model =
            compatible_auxiliary_model(&self.runtime.base_config, &self.runtime.provider_config)
                .or_else(|| self.runtime.base_config.model.clone())
                .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let mut judge_config = self.runtime.base_config.clone();
        judge_config.model = Some(model.clone());
        let system_prompt = build_judge_system_prompt(&self.runtime.base_config.system_prompt);
        let user_prompt = build_judge_request(&args);
        let reserved_tokens = estimate_tokens_for_model(&model, &system_prompt)
            .saturating_add(estimate_tokens_for_model(&model, &user_prompt))
            .saturating_add(1_200);
        let subtask_input = serde_json::json!({
            "kind": "subagent_judgement",
            "callLabel": call_id,
            "task": &args.task,
            "rubric": &args.rubric,
            "decisionMode": &args.decision_mode,
            "requiredWinnerCount": args.required_winner_count,
            "expectedOutput": &args.expected_output,
            "parallelGroup": &args.parallel_group,
            "candidateCount": args.candidates.len(),
            "candidateIds": args.candidates.iter().map(|candidate| candidate.id.as_str()).collect::<Vec<_>>(),
            "model": &model,
            "reservedTokens": reserved_tokens,
        });
        let parent_task_run_id = self.runtime.parent_task_run_id.clone();
        let mut subtask = SubtaskRecorder::create(
            db,
            parent_task_run_id.as_deref(),
            call_id,
            "Adjudicator",
            &subtask_input,
            reserved_tokens,
            format!("Subagent judge queued: {call_id}"),
            serde_json::json!({
                "callLabel": call_id,
                "candidateCount": args.candidates.len(),
                "reservedTokens": reserved_tokens,
            }),
        )?;
        let launch_elapsed_ms = Some(instant_elapsed_ms(launch_started));
        subtask.record_launch_metrics(&[
            ("launch_ack_ms", launch_elapsed_ms, None, "measured"),
            ("history_load_ms", launch_elapsed_ms, None, "not_applicable"),
            ("context_build_ms", launch_elapsed_ms, None, "measured"),
            ("skill_select_ms", launch_elapsed_ms, None, "not_applicable"),
            ("mcp_sync_ms", launch_elapsed_ms, None, "shared_snapshot"),
            (
                "tool_registry_ms",
                launch_elapsed_ms,
                None,
                "not_applicable",
            ),
            (
                "attachment_prepare_ms",
                launch_elapsed_ms,
                None,
                "not_applicable",
            ),
            ("request_build_ms", launch_elapsed_ms, None, "measured"),
        ]);
        let subtask_run_id = subtask.id().map(str::to_string);
        let _permit = match self
            .runtime
            .budget
            .begin_judge_call(
                "judge_subagent_results",
                reserved_tokens,
                &self.runtime.cancel_token,
            )
            .await
        {
            Ok(permit) => {
                if let Err(err) = subtask.mark_started(
                    "adjudicating",
                    format!("Subagent judge started: {call_id}"),
                    serde_json::json!({
                        "subtaskRunId": &subtask_run_id,
                        "callLabel": call_id,
                        "reservedTokens": reserved_tokens,
                    }),
                ) {
                    self.runtime
                        .budget
                        .rollback_unstarted_judge(reserved_tokens)
                        .await;
                    subtask.finish("failed", None, Some(&err.to_string()), None);
                    return Err(err);
                }
                permit
            }
            Err(err) => {
                let output = serde_json::json!({
                    "kind": "subagent_judgement_error",
                    "callLabel": call_id,
                    "error": err.to_string(),
                });
                subtask.finish(
                    "failed",
                    Some(&output),
                    Some(&err.to_string()),
                    Some(format!("Subagent judge failed: {call_id}")),
                );
                return Err(err);
            }
        };
        let judge_limits = self.runtime.budget.limits().await;
        let catalog_output = nexa_core::provider_catalog::endpoint_model_output_limit(
            provider_catalog_key(self.runtime.provider_config.provider_type),
            self.runtime.provider_config.base_url.as_deref(),
            &model,
        );
        let max_output = match judge_limits.max_output_tokens_per_worker {
            DelegationLimitPolicy::Auto => catalog_output,
            DelegationLimitPolicy::Explicit(limit) => {
                Some(limit.min(u64::from(catalog_output.unwrap_or(u32::MAX))) as u32)
            }
        };
        let request = CompletionRequest {
            model: model.clone(),
            messages: vec![
                nexa_core::llm::Message::text(nexa_core::llm::Role::System, system_prompt),
                nexa_core::llm::Message::text(nexa_core::llm::Role::User, user_prompt),
            ],
            temperature: Some(0.1),
            max_tokens: max_output,
            tools: None,
            stop: None,
            thinking_budget: judge_config.thinking_budget,
            reasoning_enabled: judge_config.reasoning_enabled,
            reasoning_effort: judge_config.reasoning_effort.clone(),
            provider_type: judge_config.provider_type,
            routing_session_id: None,
            parallel_tool_calls: true,
        };
        let judge_cancel_token = self.runtime.cancel_token.child_token();
        let judge_timeout_ms = resolve_delegation_run_deadline_ms(
            &self.runtime.base_config,
            None,
            judge_limits.run_deadline_ms,
        );
        let judge_cost_micros =
            nexa_core::usage_analytics::usage_cost_metadata(self.runtime.base_config.provider_type)
                .0;
        let invocation_id = format!(
            "judge:{}:{}",
            subtask_run_id
                .as_deref()
                .or(parent_task_run_id.as_deref())
                .or(conversation_id)
                .unwrap_or("detached"),
            call_id
        );
        // complete() resolves when the entire answer is ready. A first-token
        // deadline cannot measure progress here and must not restart reasoning.
        let judge_response = async {
            with_optional_timeout(judge_timeout_ms, provider.complete(&request))
                .await
                .map_err(|_| {
                    CoreError::Agent(format!(
                        "Delegated adjudication timed out after {}ms.",
                        judge_timeout_ms.unwrap_or_default()
                    ))
                })?
        };
        tokio::pin!(judge_response);
        let judge_failure_usage = Usage {
            prompt_tokens: reserved_tokens.saturating_sub(1_200),
            total_tokens: reserved_tokens,
            ..Usage::default()
        };
        let response = tokio::select! {
            _ = judge_cancel_token.cancelled() => {
                self.runtime
                    .budget
                    .finish_call(reserved_tokens, &judge_failure_usage, judge_cost_micros)
                    .await;
                let err = CoreError::Agent(
                    "Delegated adjudication was cancelled by the parent turn.".into()
                );
                let output = serde_json::json!({
                    "kind": "subagent_judgement_error",
                    "callLabel": call_id,
                    "error": err.to_string(),
                });
                subtask.finish(
                    "failed",
                    Some(&output),
                    Some(&err.to_string()),
                    Some(format!("Subagent judge failed: {call_id}")),
                );
                return Err(err);
            }
            result = &mut judge_response => match result {
                Ok(response) => {
                    self.runtime
                        .budget
                        .finish_call(reserved_tokens, &response.usage, judge_cost_micros)
                        .await;
                    response
                }
                Err(err) => {
                    self.runtime
                        .budget
                        .finish_call(reserved_tokens, &judge_failure_usage, judge_cost_micros)
                        .await;
                    let output = serde_json::json!({
                        "kind": "subagent_judgement_error",
                        "callLabel": call_id,
                        "error": err.to_string(),
                    });
                    subtask.finish(
                        "failed",
                        Some(&output),
                        Some(&err.to_string()),
                        Some(format!("Subagent judge failed: {call_id}")),
                    );
                    return Err(err);
                }
            }
        };
        let elapsed_ms = instant_elapsed_ms(launch_started);
        subtask.record_launch_metrics(&[
            (
                "provider_connect_ms",
                Some(elapsed_ms),
                Some(invocation_id.as_str()),
                "completion_boundary",
            ),
            (
                "first_sse_byte_ms",
                None,
                Some(invocation_id.as_str()),
                "not_applicable_completion_mode",
            ),
            (
                "first_visible_token_ms",
                Some(elapsed_ms),
                Some(invocation_id.as_str()),
                "measured",
            ),
            (
                "frontend_first_paint_ms",
                None,
                Some(invocation_id.as_str()),
                "not_applicable_background_worker",
            ),
        ]);
        let provider_type = self.runtime.base_config.provider_type;
        let provider_id = nexa_core::usage_analytics::provider_type_id(provider_type);
        let (estimated_cost_micros, currency, pricing_version) =
            nexa_core::usage_analytics::usage_cost_metadata(provider_type);
        let raw_usage =
            serde_json::to_value(&response.usage).unwrap_or_else(|_| serde_json::json!({}));
        if let Err(error) = db.record_ai_usage(&nexa_core::usage_analytics::AiUsageRecordInput {
            invocation_id: &invocation_id,
            occurred_at: None,
            provider_id,
            provider_type: provider_id,
            model_id: &model,
            raw_model_id: Some(&model),
            modality: "language_model",
            operation_kind: "judge",
            conversation_id,
            turn_id: None,
            run_id: parent_task_run_id.as_deref(),
            subtask_run_id: subtask_run_id.as_deref(),
            project_id: None,
            prompt_tokens: u64::from(response.usage.prompt_tokens),
            completion_tokens: u64::from(response.usage.completion_tokens),
            thinking_tokens: u64::from(response.usage.thinking_tokens.unwrap_or(0)),
            total_tokens: u64::from(
                response.usage.total_tokens.max(
                    response
                        .usage
                        .prompt_tokens
                        .saturating_add(response.usage.completion_tokens),
                ),
            ),
            cache_read_tokens: u64::from(response.usage.cache_read_tokens.unwrap_or(0)),
            cache_miss_tokens: u64::from(response.usage.cache_miss_tokens.unwrap_or(0)),
            cache_creation_tokens: u64::from(response.usage.cache_creation_tokens.unwrap_or(0)),
            usage_source: "provider",
            request_status: "success",
            latency_ms: None,
            time_to_first_token_ms: None,
            upstream_provider_id: None,
            cache_outcome_reason: None,
            estimated_cost_micros,
            currency,
            pricing_version,
            provider_raw: &raw_usage,
        }) {
            warn!("Failed to persist judge usage: {error}");
        }
        let raw_response = response.content.trim().to_string();
        let parsed = extract_json_block(&raw_response)
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .unwrap_or_else(|| serde_json::json!({ "summary": raw_response }));
        let winner_ids = parsed
            .get("winnerIds")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let summary = parsed
            .get("summary")
            .and_then(|value| value.as_str())
            .unwrap_or(raw_response.as_str())
            .to_string();
        let rationale = parsed
            .get("rationale")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let confidence = parsed
            .get("confidence")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let budget = self.runtime.budget.snapshot().await;
        let artifact = JudgeDecisionArtifact {
            kind: "subagent_judgement",
            task: args.task,
            rubric: args.rubric,
            decision_mode: args
                .decision_mode
                .unwrap_or_else(|| "single_best".to_string()),
            expected_output: args.expected_output,
            parallel_group: args.parallel_group,
            winner_ids,
            confidence,
            summary: summary.clone(),
            rationale,
            raw_response: raw_response.clone(),
            candidates: args.candidates,
            usage_total: response.usage,
            budget,
        };
        let artifact_value = serde_json::to_value(&artifact).unwrap_or_default();
        let output = serde_json::json!({
            "kind": "subagent_judgement",
            "judgement": &artifact,
        });
        subtask.finish(
            "completed",
            Some(&output),
            None,
            Some(format!("Subagent judge completed: {call_id}")),
        );
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: summary,
            is_error: false,
            artifacts: Some(artifact_value),
        })
    }
}
