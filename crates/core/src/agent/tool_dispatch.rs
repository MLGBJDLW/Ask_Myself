//! Tool-call scheduling, approval, execution, and LLM-context insertion.

use super::*;

pub(super) struct ToolDispatchContext<'a> {
    pub(super) db: &'a Database,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) source_scope: &'a [String],
    pub(super) model: &'a str,
    pub(super) privacy_cfg: &'a privacy::PrivacyConfig,
    pub(super) route_kind: AgentRouteKind,
    pub(super) iteration: u32,
    pub(super) tool_defs: &'a mut Vec<ToolDefinition>,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) task_plan: &'a mut AgentTaskPlan,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) loop_guard: &'a mut AgentLoopGuard,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) sort_order: &'a mut i64,
}

impl AgentExecutor {
    pub(super) async fn dispatch_tool_calls(
        &self,
        ctx: ToolDispatchContext<'_>,
        tool_calls: &[ToolCallRequest],
        loop_guard_block_reason: Option<String>,
        started_call_ids: &mut HashSet<String>,
        tool_run_started_ids: &mut HashSet<String>,
    ) {
        let ToolDispatchContext {
            db,
            tx,
            conversation_id,
            turn_id,
            source_scope,
            model,
            privacy_cfg,
            route_kind,
            iteration,
            tool_defs,
            messages,
            persisted_trace_items,
            task_plan,
            loop_recorder,
            loop_guard,
            trace,
            sort_order,
        } = ctx;

        // -- 4e. Execute tool calls in parallel ------------------------------
        // Emit ToolCallStart only once the provider has finished assembling
        // the complete argument string and the call is ready to execute.
        for tc in tool_calls {
            let running_run = build_tool_run_item(
                &self.tools,
                &tc.id,
                &tc.name,
                ToolRunStatus::Running,
                Some(&tc.arguments),
                None,
                None,
                None,
                None,
                None,
            );
            let run_event = if tool_run_started_ids.insert(tc.id.clone()) {
                AgentEvent::ToolRunStarted { run: running_run }
            } else {
                AgentEvent::ToolRunUpdated { run: running_run }
            };
            let _ = tx.send(run_event).await;
            if started_call_ids.insert(tc.id.clone()) {
                let _ = tx
                    .send(AgentEvent::ToolCallStart {
                        call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .await;
            }
        }

        // Build futures for all tool calls and execute concurrently.
        let offered_tool_names: HashSet<String> =
            tool_defs.iter().map(|tool| tool.name.clone()).collect();
        let tool_policy = ToolSchedulerPolicy::new(
            self.config.tool_timeout_secs,
            self.config.dynamic_tool_visibility,
            offered_tool_names,
        );
        for tc in tool_calls {
            let decision = tool_policy.decision_for(tc);
            let policy_label = if loop_guard_block_reason.is_some() {
                "blockedByLoopGuard"
            } else {
                decision.policy_label
            };
            loop_recorder.tool_scheduled(
                iteration,
                &tc.id,
                &tc.name,
                decision.timeout.map(|timeout| timeout.as_secs()),
                policy_label,
            );
            append_persisted_trace_loop_event(
                persisted_trace_items,
                TurnLoopEvent::ToolScheduled {
                    iteration,
                    call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    timeout_secs: decision.timeout.map(|timeout| timeout.as_secs()),
                    policy: policy_label.to_string(),
                },
            );
        }
        enum ToolExecutionOutcome {
            Result(crate::tools::ToolResult, ToolRunStatus),
            ExecutionError(CoreError),
            Cancelled,
            Timeout,
        }

        struct FinishedToolExecution {
            index: usize,
            call: ToolCallRequest,
            timeout: Option<Duration>,
            outcome: ToolExecutionOutcome,
            elapsed: Duration,
        }

        #[derive(Clone)]
        struct CompletedToolForContext {
            call: ToolCallRequest,
            content: String,
            duration_ms: u64,
            artifacts: Option<serde_json::Value>,
        }

        let tool_batches = tool_call_execution_batches(&self.tools, &tool_policy, tool_calls);
        let mut completed_for_context: Vec<Option<CompletedToolForContext>> =
            vec![None; tool_calls.len()];
        let mut post_tool_loop_guard_prompt: Option<String> = None;

        for tool_batch in tool_batches {
            let mut tool_futures = FuturesUnordered::new();
            for index in tool_batch {
                let tc = tool_calls[index].clone();
                let tool_span = info_span!("tool_execution", tool = %tc.name);
                let progress_tx = tx.clone();
                let approval_tx = tx.clone();
                let run_tx = tx.clone();
                let progress_call_id = tc.id.clone();
                let progress_tool_name = tc.name.clone();
                let tool_policy = &tool_policy;
                let loop_guard_block_reason = loop_guard_block_reason.clone();
                tool_futures.push(
                    async move {
                        let scheduling = tool_policy.decision_for(&tc);
                        let invocation = self
                            .tools
                            .build_invocation(&tc.id, &tc.name, scheduling.parsed_args);
                        let parsed_args = invocation.arguments.clone();
                        let tool_timeout = scheduling.timeout;
                        let capabilities = invocation.capabilities.clone();
                        if let Some(reason) = loop_guard_block_reason.as_deref() {
                            let blocked = loop_guard_blocked_result(&tc, reason);
                            return FinishedToolExecution {
                                index,
                                call: tc,
                                timeout: tool_timeout,
                                outcome: ToolExecutionOutcome::Result(
                                    blocked,
                                    ToolRunStatus::Failed,
                                ),
                                elapsed: Duration::ZERO,
                            };
                        }
                        if let Some(blocked) = scheduling.synthetic_result {
                            return FinishedToolExecution {
                                index,
                                call: tc,
                                timeout: tool_timeout,
                                outcome: ToolExecutionOutcome::Result(
                                    blocked,
                                    ToolRunStatus::Failed,
                                ),
                                elapsed: Duration::ZERO,
                            };
                        }
                        let tool_requires_confirm =
                            self.tools.requires_confirmation(&tc.name, &parsed_args);
                        let shell_requires_confirm = tc.name == "run_shell"
                            && self.config.shell_access_mode.requires_confirmation();
                        if let Some(ref approval_cb) = self.approval_callback {
                            let baseline = if tool_requires_confirm || shell_requires_confirm {
                                PolicyEffect::RequireApproval
                            } else {
                                PolicyEffect::Allow
                            };
                            let policy_decision = evaluate_policy_with_baseline(
                                &[],
                                &PolicySubject::from_invocation(&invocation),
                                baseline,
                            );
                            if policy_decision.denied {
                                let denied = crate::tools::ToolResult {
                                    call_id: tc.id.clone(),
                                    content: format!(
                                        "Policy denied permission for {}: {}",
                                        tc.name,
                                        policy_decision.reasons.join(" ")
                                    ),
                                    is_error: true,
                                    artifacts: Some(serde_json::json!({
                                        "kind": "policyDecision",
                                        "effect": policy_decision.effect.as_str(),
                                        "reasons": policy_decision.reasons,
                                        "matchedRuleIds": policy_decision.matched_rule_ids,
                                    })),
                                };
                                return FinishedToolExecution {
                                    index,
                                    call: tc,
                                    timeout: tool_timeout,
                                    outcome: ToolExecutionOutcome::Result(
                                        denied,
                                        ToolRunStatus::Declined,
                                    ),
                                    elapsed: Duration::ZERO,
                                };
                            }
                            if policy_decision.needs_approval {
                                if let Some(decision) = self.config.tool_approval_mode.short_circuit() {
                                    if !decision.is_allowed() {
                                        let denied = crate::tools::ToolResult {
                                            call_id: tc.id.clone(),
                                            content: format!(
                                                "Tool approval mode denied permission for {}.",
                                                tc.name
                                            ),
                                            is_error: true,
                                            artifacts: None,
                                        };
                                        return FinishedToolExecution {
                                            index,
                                            call: tc,
                                            timeout: tool_timeout,
                                            outcome: ToolExecutionOutcome::Result(
                                                denied,
                                                ToolRunStatus::Declined,
                                            ),
                                            elapsed: Duration::ZERO,
                                        };
                                    }
                                } else {
                                    let _ = run_tx
                                        .send(AgentEvent::ToolRunUpdated {
                                            run: build_tool_run_item(
                                                &self.tools,
                                                &tc.id,
                                                &tc.name,
                                                ToolRunStatus::ApprovalPending,
                                                Some(&tc.arguments),
                                                None,
                                                None,
                                                None,
                                                Some("waiting for approval".to_string()),
                                                None,
                                            ),
                                        })
                                        .await;
                                    let risk = policy_decision.risk_level;
                                    let reason = self
                                        .tools
                                        .confirmation_message(&tc.name, &parsed_args)
                                        .unwrap_or_else(|| describe_request(&tc.name, &parsed_args));
                                    let req = ApprovalRequest::new(
                                        Uuid::new_v4().to_string(),
                                        &tc.name,
                                        &parsed_args,
                                        risk,
                                        reason,
                                    );
                                    let _ = approval_tx
                                        .send(AgentEvent::ApprovalRequested {
                                            request: req.clone(),
                                        })
                                        .await;
                                    let decision = approval_cb(req.clone()).await;
                                    let _ = approval_tx
                                        .send(AgentEvent::ApprovalResolved {
                                            request_id: req.id.clone(),
                                            decision,
                                        })
                                        .await;
                                    if !decision.is_allowed() {
                                        let denied = crate::tools::ToolResult {
                                            call_id: tc.id.clone(),
                                            content: format!(
                                                "User denied permission for {}.",
                                                tc.name
                                            ),
                                            is_error: true,
                                            artifacts: None,
                                        };
                                        return FinishedToolExecution {
                                            index,
                                            call: tc,
                                            timeout: tool_timeout,
                                            outcome: ToolExecutionOutcome::Result(
                                                denied,
                                                ToolRunStatus::Declined,
                                            ),
                                            elapsed: Duration::ZERO,
                                        };
                                    }
                                    let _ = run_tx
                                        .send(AgentEvent::ToolRunUpdated {
                                            run: build_tool_run_item(
                                                &self.tools,
                                                &tc.id,
                                                &tc.name,
                                                ToolRunStatus::Running,
                                                Some(&tc.arguments),
                                                None,
                                                None,
                                                None,
                                                None,
                                                None,
                                            ),
                                        })
                                        .await;
                                }
                            }
                        } else {
                            let baseline_requires_confirmation = if tc.name == "run_shell" {
                                shell_requires_confirm
                            } else {
                                self.config.require_tool_confirmation && tool_requires_confirm
                            };
                            let baseline = if baseline_requires_confirmation {
                                PolicyEffect::RequireApproval
                            } else {
                                PolicyEffect::Allow
                            };
                            let policy_decision = evaluate_policy_with_baseline(
                                &[],
                                &PolicySubject::from_invocation(&invocation),
                                baseline,
                            );
                            if policy_decision.denied {
                                let declined = crate::tools::ToolResult {
                                    call_id: tc.id.clone(),
                                    content: format!(
                                        "Policy denied permission for {}: {}",
                                        tc.name,
                                        policy_decision.reasons.join(" ")
                                    ),
                                    is_error: true,
                                    artifacts: Some(serde_json::json!({
                                        "kind": "policyDecision",
                                        "effect": policy_decision.effect.as_str(),
                                        "reasons": policy_decision.reasons,
                                        "matchedRuleIds": policy_decision.matched_rule_ids,
                                    })),
                                };
                                return FinishedToolExecution {
                                    index,
                                    call: tc,
                                    timeout: tool_timeout,
                                    outcome: ToolExecutionOutcome::Result(
                                        declined,
                                        ToolRunStatus::Declined,
                                    ),
                                    elapsed: Duration::ZERO,
                                };
                            }
                            if policy_decision.needs_approval {
                                if let Some(decision) = self.config.tool_approval_mode.short_circuit() {
                                    if !decision.is_allowed() {
                                        let declined = crate::tools::ToolResult {
                                            call_id: tc.id.clone(),
                                            content: format!(
                                                "Tool approval mode denied permission for {}.",
                                                tc.name
                                            ),
                                            is_error: true,
                                            artifacts: None,
                                        };
                                        return FinishedToolExecution {
                                            index,
                                            call: tc,
                                            timeout: tool_timeout,
                                            outcome: ToolExecutionOutcome::Result(
                                                declined,
                                                ToolRunStatus::Declined,
                                            ),
                                            elapsed: Duration::ZERO,
                                        };
                                    }
                                } else if let Some(ref cb) = self.confirmation_callback {
                                    let message = self
                                        .tools
                                        .confirmation_message(&tc.name, &parsed_args)
                                        .unwrap_or_else(|| format!("Execute tool: {}", tc.name));
                                    if !cb(message).await {
                                        let declined = crate::tools::ToolResult {
                                            call_id: tc.id.clone(),
                                            content: "Operation cancelled by user.".to_string(),
                                            is_error: true,
                                            artifacts: None,
                                        };
                                        return FinishedToolExecution {
                                            index,
                                            call: tc,
                                            timeout: tool_timeout,
                                            outcome: ToolExecutionOutcome::Result(
                                                declined,
                                                ToolRunStatus::Declined,
                                            ),
                                            elapsed: Duration::ZERO,
                                        };
                                    }
                                }
                            }
                        }

                        let tool_start = std::time::Instant::now();
                        let execute_tool = async {
                            let exec_fut = self.tools.execute_with_run_context(
                                &tc.name,
                                crate::tools::ToolExecutionContext {
                                    call_id: &tc.id,
                                    arguments: &tc.arguments,
                                    db,
                                    source_scope,
                                    conversation_id,
                                    tool_registry: Some(&self.tools),
                                    cancel_token: Some(&self.cancel_token),
                                },
                            );
                            tokio::pin!(exec_fut);
                            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
                            heartbeat
                                .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                            heartbeat.tick().await;
                            loop {
                                tokio::select! {
                                    biased;
                                    r = &mut exec_fut => break r,
                                    _ = heartbeat.tick() => {
                                        let note = format!("running {}...", progress_tool_name);
                                        debug!(
                                            "tool heartbeat: {} (call_id={})",
                                            progress_tool_name, progress_call_id,
                                        );
                                        let _ = progress_tx
                                            .send(AgentEvent::ToolCallProgress {
                                                call_id: progress_call_id.clone(),
                                                note: note.clone(),
                                            })
                                            .await;
                                        let _ = progress_tx
                                            .send(AgentEvent::ToolRunUpdated {
                                                run: build_tool_run_item(
                                                    &self.tools,
                                                    &progress_call_id,
                                                    &progress_tool_name,
                                                    ToolRunStatus::Running,
                                                    Some(&tc.arguments),
                                                    None,
                                                    None,
                                                    None,
                                                    Some(note),
                                                    None,
                                                ),
                                            })
                                            .await;
                                    }
                                }
                            }
                        };
                        let execute_to_outcome = async {
                            if let Some(timeout) = tool_timeout {
                                match tokio::time::timeout(timeout, execute_tool).await {
                                    Ok(Ok(result)) => {
                                        let status = if result.is_error {
                                            ToolRunStatus::Failed
                                        } else {
                                            ToolRunStatus::Completed
                                        };
                                        ToolExecutionOutcome::Result(result, status)
                                    }
                                    Ok(Err(err)) => ToolExecutionOutcome::ExecutionError(err),
                                    Err(_) => ToolExecutionOutcome::Timeout,
                                }
                            } else {
                                match execute_tool.await {
                                    Ok(result) => {
                                        let status = if result.is_error {
                                            ToolRunStatus::Failed
                                        } else {
                                            ToolRunStatus::Completed
                                        };
                                        ToolExecutionOutcome::Result(result, status)
                                    }
                                    Err(err) => ToolExecutionOutcome::ExecutionError(err),
                                }
                            }
                        };
                        let outcome = if matches!(
                            capabilities.interrupt_behavior,
                            ToolInterruptBehavior::Cancel
                        ) {
                            tokio::select! {
                                biased;
                                _ = self.cancel_token.cancelled() => ToolExecutionOutcome::Cancelled,
                                outcome = execute_to_outcome => outcome,
                            }
                        } else {
                            execute_to_outcome.await
                        };
                        let tool_elapsed = tool_start.elapsed();
                        FinishedToolExecution {
                            index,
                            call: tc,
                            timeout: tool_timeout,
                            outcome,
                            elapsed: tool_elapsed,
                        }
                    }
                    .instrument(tool_span),
                );
            }

            while let Some(finished_tool) = tool_futures.next().await {
                let tc = finished_tool.call;
                let tool_elapsed = finished_tool.elapsed;
                let (tool_msg, tool_context_msg, tool_artifacts, tool_is_error, run_status) =
                    match finished_tool.outcome {
                        ToolExecutionOutcome::Result(result, status) => {
                            let context_content = result.llm_context_content();
                            (
                                result.content,
                                context_content,
                                result.artifacts,
                                result.is_error,
                                status,
                            )
                        }
                        ToolExecutionOutcome::ExecutionError(e) => {
                            let structured = crate::tools::structured_tool_error_result(
                                &tc.id,
                                "tool_execution_failed",
                                format!("{} failed: {e}", tc.name),
                                serde_json::json!({
                                    "tool": &tc.name,
                                    "arguments": "must match this tool's JSON schema exactly",
                                    "recovery": "inspect the error, adjust only the invalid fields, and retry if the request still needs this tool"
                                }),
                                true,
                            );
                            let err_content = structured.content.clone();
                            (
                                err_content.clone(),
                                err_content,
                                structured.artifacts,
                                true,
                                ToolRunStatus::Failed,
                            )
                        }
                        ToolExecutionOutcome::Cancelled => {
                            let structured = crate::tools::structured_tool_error_result(
                                &tc.id,
                                "tool_cancelled",
                                format!("tool '{}' was cancelled by user request.", tc.name),
                                serde_json::json!({
                                    "tool": &tc.name,
                                    "recovery": "stop using this tool for the interrupted request unless the user asks to resume"
                                }),
                                false,
                            );
                            let err_content = structured.content.clone();
                            (
                                err_content.clone(),
                                err_content,
                                structured.artifacts,
                                true,
                                ToolRunStatus::Cancelled,
                            )
                        }
                        ToolExecutionOutcome::Timeout => {
                            let timeout_secs =
                                finished_tool.timeout.map(|d| d.as_secs()).unwrap_or(0);
                            warn!("Tool '{}' timed out after {}s", tc.name, timeout_secs);
                            let structured = crate::tools::structured_tool_error_result(
                                    &tc.id,
                                    "tool_timeout",
                                    format!(
                                        "tool '{}' timed out after {} seconds. Try a simpler query or different approach.",
                                        tc.name,
                                        timeout_secs
                                    ),
                                    serde_json::json!({
                                        "tool": &tc.name,
                                        "timeoutSeconds": timeout_secs,
                                        "recovery": "retry with narrower arguments, fewer files, or a smaller limit"
                                    }),
                                    true,
                                );
                            let err_content = structured.content.clone();
                            (
                                err_content.clone(),
                                err_content,
                                structured.artifacts,
                                true,
                                ToolRunStatus::TimedOut,
                            )
                        }
                    };

                let _ = tx
                    .send(AgentEvent::ToolCallResult {
                        call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        content: tool_msg.clone(),
                        is_error: tool_is_error,
                        artifacts: tool_artifacts.clone(),
                    })
                    .await;
                let _ = tx
                    .send(AgentEvent::ToolRunCompleted {
                        run: build_tool_run_item(
                            &self.tools,
                            &tc.id,
                            &tc.name,
                            run_status,
                            Some(&tc.arguments),
                            Some(tool_msg.clone()),
                            Some(tool_is_error),
                            tool_artifacts.clone(),
                            None,
                            Some(tool_elapsed.as_millis() as u64),
                        ),
                    })
                    .await;

                if !tool_is_error && tc.name == "tool_search" {
                    let activation = tool_discovery::activate_tool_search_matches(
                        &self.tools,
                        tool_defs,
                        tool_artifacts.as_ref(),
                    );
                    if activation.has_changes() {
                        let content = format!(
                            "Activated {} deferred tool(s): {}",
                            activation.activated.len(),
                            activation.activated.join(", ")
                        );
                        append_persisted_trace_status(persisted_trace_items, &content, "muted");
                        if let Some(ref mut t) = trace {
                            t.tools_offered = tool_defs.len() as u32;
                        }
                        let _ = tx
                            .send(AgentEvent::Status {
                                content,
                                tone: Some("muted".to_string()),
                            })
                            .await;
                    }
                }

                // Redact tool output before adding to context.
                let content = if privacy_cfg.enabled {
                    privacy::redact_content(&tool_msg, &privacy_cfg.redact_patterns)
                } else {
                    tool_msg
                };
                let context_content = if privacy_cfg.enabled {
                    privacy::redact_content(&tool_context_msg, &privacy_cfg.redact_patterns)
                } else {
                    tool_context_msg
                };

                append_persisted_trace_tool(
                    persisted_trace_items,
                    &self.tools,
                    &tc.name,
                    &tc.arguments,
                    &tc.id,
                    if tool_is_error { "error" } else { "done" },
                    Some(content.clone()),
                    Some(tool_is_error),
                    tool_artifacts.clone(),
                );
                let finished = TurnLoopEvent::ToolFinished {
                    iteration,
                    call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    duration_ms: tool_elapsed.as_millis() as u64,
                    is_error: tool_is_error,
                };
                loop_recorder.record(finished.clone());
                append_persisted_trace_loop_event(persisted_trace_items, finished);
                if let Some(intervention) = loop_guard.observe_tool_result(tool_is_error) {
                    let event = TurnLoopEvent::LoopGuardIntervention {
                        reason: intervention.reason.clone(),
                        action: intervention.action.as_str().to_string(),
                    };
                    loop_recorder.record(event.clone());
                    append_persisted_trace_loop_event(persisted_trace_items, event);
                    append_persisted_trace_status(
                        persisted_trace_items,
                        &intervention.reason,
                        "warning",
                    );
                    let _ = tx
                        .send(AgentEvent::Status {
                            content: intervention.reason.clone(),
                            tone: Some("warning".to_string()),
                        })
                        .await;
                    post_tool_loop_guard_prompt.get_or_insert(intervention.prompt);
                }
                if advance_task_plan_for_tool_result(task_plan, &tc.name, tool_is_error) {
                    emit_task_plan_update(
                        tx,
                        task_plan,
                        if tool_is_error {
                            "recovering"
                        } else {
                            "tooling"
                        },
                        if tool_is_error {
                            "Tool failed; execution plan marked for recovery"
                        } else {
                            "Execution plan advanced after tool result"
                        },
                    )
                    .await;
                }
                if let Some(tid) = turn_id {
                    let trace = build_turn_trace(route_kind, persisted_trace_items);
                    let _ = db.update_conversation_turn_progress(
                        tid,
                        Some(&format!("{:?}", route_kind)),
                        Some(&trace),
                    );
                }

                completed_for_context[finished_tool.index] = Some(CompletedToolForContext {
                    call: tc,
                    content: context_content,
                    duration_ms: tool_elapsed.as_millis() as u64,
                    artifacts: tool_artifacts,
                });
            }
        }

        for completed in completed_for_context.into_iter().flatten() {
            let tc = completed.call;
            let content = completed.content;
            let duration_ms = completed.duration_ms;
            let tool_artifacts = completed.artifacts;

            // Save tool result message to DB.
            if let Some(cid) = conversation_id {
                let tool_conv_msg = ConversationMessage {
                    id: Uuid::new_v4().to_string(),
                    conversation_id: cid.to_string(),
                    role: Role::Tool,
                    content: content.clone(),
                    tool_call_id: Some(tc.id.clone()),
                    tool_calls: vec![],
                    artifacts: tool_artifacts.clone(),
                    token_count: estimate_tokens_for_model(model, &content),
                    created_at: String::new(),
                    sort_order: *sort_order,
                    thinking: None,
                    image_attachments: None,
                };
                if let Err(e) = db.add_message(&tool_conv_msg) {
                    warn!("Failed to save tool result message: {e}");
                }
                *sort_order += 1;
            }

            // Truncate large tool results for LLM context to prevent
            // crowding out conversation history.
            let context_content = compact_tool_result_for_context(&tc.name, &content);

            messages.push(Message::text_with_name(
                Role::Tool,
                context_content,
                tc.id.clone(),
            ));

            // Trace: record tool execution step
            if let Some(ref mut t) = trace {
                t.add_step(TraceStep {
                    iteration,
                    tool_name: Some(tc.name.clone()),
                    tool_duration_ms: Some(duration_ms),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    context_usage_pct: 0.0,
                    was_compacted: false,
                });
            }
        }
        if let Some(prompt) = post_tool_loop_guard_prompt {
            messages.push(Message::text(Role::System, prompt));
        }
    }
}
