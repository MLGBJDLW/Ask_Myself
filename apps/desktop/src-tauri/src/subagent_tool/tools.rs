use super::*;
#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }
    fn description(&self) -> &str {
        &delegation_tool_def(&SPAWN_SUBAGENT_DEF, SPAWN_SUBAGENT_JSON).description
    }
    fn parameters_schema(&self) -> serde_json::Value {
        spawn_subagent_parameters_schema()
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id,
            turn_id,
            activity_runtime,
            ..
        } = context;
        let args: SpawnSubagentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent arguments: {e}"))
        })?;
        let args = normalize_spawn_args(args)?;
        let agent_id = format!("subagent-{}", uuid::Uuid::new_v4());
        let registration = self.runtime.lifecycle.register(RegisterSubagentRequest {
            agent_id: agent_id.clone(),
            parent_call_id: call_id.to_string(),
            task: args.task.clone(),
            role_id: args.role_id.clone(),
            role: args.role.clone(),
            conversation_id: conversation_id.map(str::to_string),
            turn_id: turn_id.map(str::to_string),
            task_run_id: self.runtime.parent_task_run_id.clone(),
            cancel_token: self.runtime.cancel_token.child_token(),
            activity_runtime: activity_runtime.cloned().unwrap_or_default(),
        })?;
        if let Err(error) = launch_detached_subagent(
            self.runtime.clone(),
            db.clone(),
            source_scope.to_vec(),
            args.clone(),
            registration,
        ) {
            let _ = self
                .runtime
                .lifecycle
                .set_status(&agent_id, SubagentLifecycleStatus::Failed);
            let _ = self.runtime.lifecycle.close(&agent_id);
            return Err(error);
        }
        let content = format!(
            "Subagent {agent_id} spawned and is running. Use observe_subagent for incremental events, wait_subagent before consuming its final result, send_subagent_input to steer it, or cancel_subagent to stop it."
        );
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_result",
                "id": agent_id,
                "sessionId": args.task_id,
                "status": "running",
                "task": args.task,
                "roleId": args.role_id,
                "role": args.role,
                "expectedOutput": args.expected_output,
                "acceptanceCriteria": args.acceptance_criteria,
                "parallelGroup": args.parallel_group,
                "result": "",
                "isError": false,
                "lifecycleTools": {
                    "observe": "observe_subagent",
                    "wait": "wait_subagent",
                    "sendInput": "send_subagent_input",
                    "cancel": "cancel_subagent",
                    "close": "close_subagent",
                },
            })),
        })
    }
}
#[async_trait]
impl Tool for SubagentBatchTool {
    fn name(&self) -> &str {
        "spawn_subagent_batch"
    }
    fn description(&self) -> &str {
        &delegation_tool_def(&SPAWN_SUBAGENT_BATCH_DEF, SPAWN_SUBAGENT_BATCH_JSON).description
    }
    fn parameters_schema(&self) -> serde_json::Value {
        spawn_subagent_batch_parameters_schema()
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id,
            turn_id,
            activity_runtime,
            ..
        } = context;
        let mut args: SpawnSubagentBatchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent_batch arguments: {e}"))
        })?;
        args.batch_goal = trim_optional(args.batch_goal);
        args.parallel_group = trim_optional(args.parallel_group);
        args.workflow_template = normalize_workflow_template_id(args.workflow_template)?;
        let workflow_template = args
            .workflow_template
            .as_deref()
            .and_then(workflow_template_by_id);
        if args.tasks.is_empty() {
            let Some(template) = workflow_template else {
                return Err(CoreError::InvalidInput(
                    "spawn_subagent_batch requires either explicit tasks or workflow_template plus batch_goal".into(),
                ));
            };
            let Some(batch_goal) = args.batch_goal.clone() else {
                return Err(CoreError::InvalidInput(
                    "spawn_subagent_batch workflow_template expansion requires a non-empty batch_goal".into(),
                ));
            };
            if args.parallel_group.is_none() {
                args.parallel_group = Some(template.id.to_string());
            }
            args.tasks = expand_workflow_template_tasks(
                template,
                &batch_goal,
                args.parallel_group.as_deref(),
            );
        }
        let batch_goal = args.batch_goal.clone();
        let workflow_template_id = args.workflow_template.clone();
        let workflow_template_label = workflow_template.map(|template| template.label);
        let workflow_template_description = workflow_template.map(|template| template.description);
        let parallel_group = args.parallel_group.clone();
        let completion_policy =
            DelegationCompletionPolicy::resolve(&args, args.tasks.len().min(8))?;
        let requested_max_parallel = args.max_parallel;
        let cancel_remaining = args.cancel_remaining.unwrap_or(false);
        let normalized_tasks: Vec<(Option<String>, SpawnSubagentArgs)> = args
            .tasks
            .into_iter()
            .take(8)
            .enumerate()
            .map(|(index, mut task)| {
                if task.parallel_group.is_none() {
                    task.parallel_group = parallel_group.clone();
                }
                if task.id.is_none() {
                    task.id = Some(format!("{}-{}", call_id, index + 1));
                }
                normalize_batch_task_args(task)
            })
            .collect::<Result<_, _>>()?;
        let budget_before = self.runtime.budget.snapshot().await;
        let requested_parallel = requested_max_parallel
            .unwrap_or_else(|| {
                workflow_template
                    .map(|template| template.max_parallel)
                    .unwrap_or(budget_before.max_parallel)
            })
            .clamp(1, 8);
        let effective_parallel = requested_parallel.min(budget_before.max_parallel).max(1) as usize;
        let runtime = self.runtime.clone();
        let db = db.clone();
        let inherited_source_scope = source_scope.to_vec();
        let parent_conversation_id = conversation_id.map(str::to_string);
        let parent_turn_id = turn_id.map(str::to_string);
        let activity_runtime = activity_runtime.cloned().unwrap_or_default();
        let batch_parallel_group = parallel_group.clone();
        let worker_count = normalized_tasks.len();
        let batch_id = format!(
            "{}:{}",
            self.runtime
                .parent_task_run_id
                .as_deref()
                .unwrap_or("detached"),
            call_id
        );
        runtime.register_batch(&batch_id, worker_count);
        let batch_slots = Arc::new(tokio::sync::Semaphore::new(effective_parallel));
        let mut worker_cancel_tokens = Vec::with_capacity(worker_count);
        let mut lifecycle_workers = Vec::with_capacity(worker_count);
        let mut pending = FuturesUnordered::new();
        for (index, (worker_id, task_args)) in normalized_tasks.into_iter().enumerate() {
            let db = db.clone();
            let inherited_source_scope = inherited_source_scope.clone();
            let batch_parallel_group = batch_parallel_group.clone();
            let worker_cancel = runtime.cancel_token.child_token();
            let batch_runtime = runtime.clone();
            let batch_slots = Arc::clone(&batch_slots);
            worker_cancel_tokens.push(worker_cancel.clone());
            let lifecycle_cancellation_for_join = worker_cancel.clone();
            runtime.add_batch_cancel_token(&batch_id, worker_cancel.clone());
            let worker_batch_id = batch_id.clone();
            let detached_label = worker_id
                .clone()
                .unwrap_or_else(|| format!("{}-{}", call_id, index + 1));
            let detached_fallback = task_args.clone();
            let detached_parallel_group = batch_parallel_group.clone();
            let batch_call_id = call_id.to_string();
            let lifecycle_agent_id = format!("subagent-{}", uuid::Uuid::new_v4());
            let registration = runtime.lifecycle.register(RegisterSubagentRequest {
                agent_id: lifecycle_agent_id.clone(),
                parent_call_id: call_id.to_string(),
                task: task_args.task.clone(),
                role_id: task_args.role_id.clone(),
                role: task_args.role.clone(),
                conversation_id: parent_conversation_id.clone(),
                turn_id: parent_turn_id.clone(),
                task_run_id: self.runtime.parent_task_run_id.clone(),
                cancel_token: worker_cancel,
                activity_runtime: activity_runtime.clone(),
            })?;
            lifecycle_workers.push(serde_json::json!({
                "agentId": &lifecycle_agent_id,
                "workerId": &worker_id,
                "task": &task_args.task,
                "roleId": &task_args.role_id,
                "role": &task_args.role,
            }));
            let lifecycle_for_join = runtime.lifecycle.clone();
            let lifecycle_agent_id_for_join = lifecycle_agent_id.clone();
            let worker_task = tokio::spawn(async move {
                let label = worker_id
                    .clone()
                    .unwrap_or_else(|| format!("{}-{}", batch_call_id, index + 1));
                let fallback = task_args.clone();
                let run = match run_registered_subagent_isolated(
                    batch_runtime.clone(),
                    db,
                    inherited_source_scope,
                    label.clone(),
                    worker_id,
                    task_args,
                    Some(batch_slots),
                    registration,
                )
                .await
                {
                    Ok(run) => run,
                    Err(err) => {
                        failed_subagent_run_artifact(label, fallback, batch_parallel_group, &err)
                    }
                };
                batch_runtime.record_batch_result(&worker_batch_id, index, run.clone());
                (index, run)
            });
            pending.push(async move {
                match worker_task.await {
                    Ok(result) => result,
                    Err(join_error) => {
                        let error = CoreError::Agent(format!(
                            "Delegated worker task terminated unexpectedly: {join_error}"
                        ));
                        settle_worker_lifecycle(
                            &lifecycle_for_join,
                            &lifecycle_agent_id_for_join,
                            &lifecycle_cancellation_for_join,
                            Err(&error),
                        )
                        .await;
                        (
                            index,
                            failed_subagent_run_artifact(
                                detached_label,
                                detached_fallback,
                                detached_parallel_group,
                                &error,
                            ),
                        )
                    }
                }
            });
        }
        let policy_deadline = match &completion_policy {
            DelegationCompletionPolicy::Deadline { deadline_ms } => {
                Some(tokio::time::Instant::now() + Duration::from_millis(*deadline_ms))
            }
            _ => None,
        };
        let mut indexed_runs = Vec::with_capacity(worker_count);
        let mut policy_deadline_reached = false;
        while !pending.is_empty() {
            let next = if let Some(deadline) = policy_deadline {
                match tokio::time::timeout_at(deadline, pending.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        policy_deadline_reached = true;
                        None
                    }
                }
            } else {
                pending.next().await
            };
            let Some((index, run)) = next else {
                break;
            };
            indexed_runs.push((index, run));
            let completed_runs = indexed_runs
                .iter()
                .map(|(_, run)| run.clone())
                .collect::<Vec<_>>();
            if completion_policy.is_satisfied(&completed_runs, pending.len()) {
                break;
            }
        }
        let policy_satisfied = policy_deadline_reached
            || completion_policy.is_satisfied(
                &indexed_runs
                    .iter()
                    .map(|(_, run)| run.clone())
                    .collect::<Vec<_>>(),
                pending.len(),
            );
        let pending_at_policy_completion = pending.len();
        let continuing_workers = if !pending.is_empty() && !cancel_remaining {
            // Each entry owns a Tokio JoinHandle. Dropping the collector
            // detaches those tasks rather than cancelling them; their normal
            // completion path persists a supplemental subtask timeline event.
            pending.len()
        } else {
            0
        };
        if !pending.is_empty() && cancel_remaining {
            // Dropping a future is not cancellation. Signal every worker first,
            // then provide a bounded settlement window for durable final state.
            for token in &worker_cancel_tokens {
                token.cancel();
            }
            let settle_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            while !pending.is_empty() {
                match tokio::time::timeout_at(settle_deadline, pending.next()).await {
                    Ok(Some((index, run))) => indexed_runs.push((index, run)),
                    _ => break,
                }
            }
        }
        let unsettled_workers = if cancel_remaining { pending.len() } else { 0 };
        drop(pending);
        indexed_runs.sort_by_key(|(index, _)| *index);
        let runs = indexed_runs
            .into_iter()
            .map(|(_, run)| run)
            .collect::<Vec<_>>();
        let budget_after = self.runtime.budget.snapshot().await;
        let completed_runs = runs.iter().filter(|run| !run.is_error).count();
        let failed_runs = runs.len().saturating_sub(completed_runs);
        let mut content = format!("Completed {} delegated worker(s) in batch", runs.len());
        if let Some(goal) = batch_goal.as_deref() {
            content.push_str(&format!(" for: {goal}"));
        }
        if let Some(template_label) = workflow_template_label {
            content.push_str(&format!(" using {template_label}"));
        }
        if pending_at_policy_completion > 0 {
            content.push_str(&format!(
                "; completion policy released the parent with {pending_at_policy_completion} worker(s) still settling"
            ));
            content.push_str(&format!(
                ". Call observe_subagent_batch with batchId '{batch_id}' before final synthesis to receive supplemental evidence, wait for more results, or cancel residual workers"
            ));
        }
        content.push_str(".\n\n");
        for run in &runs {
            content.push_str("- ");
            content.push_str(&summarize_subagent_run(run));
            content.push('\n');
        }
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: failed_runs > 0 && completed_runs == 0,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_batch_result",
                "batchId": &batch_id,
                "lifecycleWorkers": lifecycle_workers,
                "batchGoal": batch_goal,
                "workflowTemplate": workflow_template_id,
                "workflowTemplateLabel": workflow_template_label,
                "workflowTemplateDescription": workflow_template_description,
                "parallelGroup": parallel_group,
                "requestedMaxParallel": requested_parallel,
                "effectiveMaxParallel": effective_parallel,
                "completionPolicy": completion_policy,
                "completionPolicySatisfied": policy_satisfied,
                "pendingAtPolicyCompletion": pending_at_policy_completion,
                "unsettledWorkers": unsettled_workers,
                "continuingWorkers": continuing_workers,
                "supplementalEvidenceTool": (continuing_workers > 0).then_some("observe_subagent_batch"),
                "cancelRemaining": cancel_remaining,
                "completedRuns": completed_runs,
                "failedRuns": failed_runs,
                "budgetBefore": budget_before,
                "budgetAfter": budget_after,
                "runs": runs,
            })),
        })
    }
}
#[async_trait]
impl Tool for ObserveSubagentBatchTool {
    fn name(&self) -> &str {
        "observe_subagent_batch"
    }
    fn description(&self) -> &str {
        "Observe supplemental results from a delegated batch after quorum, first-success, deadline, or parent-decides released the parent. Optionally wait for more results or cancel residual workers."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "batchId": {
                    "type": "string",
                    "description": "batchId returned by spawn_subagent_batch"
                },
                "waitMs": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "description": "One steering-friendly wait quantum for another supplemental result"
                },
                "cancelRemaining": {
                    "type": "boolean",
                    "default": false,
                    "description": "Cancel workers that have not yet settled"
                }
            },
            "required": ["batchId"],
            "additionalProperties": false
        })
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let args: ObserveSubagentBatchArgs =
            serde_json::from_str(context.arguments).map_err(|error| {
                CoreError::InvalidInput(format!(
                    "Invalid observe_subagent_batch arguments: {error}"
                ))
            })?;
        let batch_id = args.batch_id.trim();
        if batch_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "observe_subagent_batch requires batchId".into(),
            ));
        }
        if args.cancel_remaining {
            self.runtime.cancel_batch(batch_id);
        }
        let wait_ms = args.wait_ms.unwrap_or(0).min(2_500);
        let baseline_count = self
            .runtime
            .batch_snapshot(batch_id)
            .ok_or_else(|| CoreError::NotFound(format!("Delegated batch {batch_id}")))?
            .1
            .len();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
        let (expected_workers, runs) = loop {
            let notified = self.runtime.batch_notify.notified();
            tokio::pin!(notified);
            // Register before reading the snapshot so a completion between
            // the read and the await cannot be lost by notify_waiters().
            notified.as_mut().enable();
            let Some((expected, runs)) = self.runtime.batch_snapshot(batch_id) else {
                return Err(CoreError::NotFound(format!("Delegated batch {batch_id}")));
            };
            if runs.len() >= expected
                || runs.len() > baseline_count
                || wait_ms == 0
                || tokio::time::Instant::now() >= deadline
            {
                break (expected, runs);
            }
            if tokio::time::timeout_at(deadline, &mut notified)
                .await
                .is_err()
            {
                break self
                    .runtime
                    .batch_snapshot(batch_id)
                    .unwrap_or((expected, runs));
            }
        };
        let completed_workers = runs.len();
        let pending_workers = expected_workers.saturating_sub(completed_workers);
        let mut content = format!(
            "Delegated batch {batch_id}: {completed_workers}/{expected_workers} worker(s) settled"
        );
        if pending_workers > 0 {
            content.push_str(&format!("; {pending_workers} still running"));
        }
        content.push_str(".\n\n");
        for run in &runs {
            content.push_str("- ");
            content.push_str(&summarize_subagent_run(run));
            content.push('\n');
        }
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_batch_observation",
                "batchId": batch_id,
                "expectedWorkers": expected_workers,
                "completedWorkers": completed_workers,
                "pendingWorkers": pending_workers,
                "cancelRequested": args.cancel_remaining,
                "runs": runs,
            })),
        })
    }
}
#[async_trait]
impl Tool for SubagentLifecycleTool {
    fn name(&self) -> &str {
        match self.action {
            SubagentLifecycleAction::Observe => "observe_subagent",
            SubagentLifecycleAction::Wait => "wait_subagent",
            SubagentLifecycleAction::SendInput => "send_subagent_input",
            SubagentLifecycleAction::Cancel => "cancel_subagent",
            SubagentLifecycleAction::Close => "close_subagent",
        }
    }
    fn description(&self) -> &str {
        match self.action {
            SubagentLifecycleAction::Observe => {
                "Read a spawned subagent's current state and incremental lifecycle events without blocking the parent turn."
            }
            SubagentLifecycleAction::Wait => {
                "Wait for a spawned subagent to settle, up to a bounded timeout, and return its authoritative result snapshot."
            }
            SubagentLifecycleAction::SendInput => {
                "Steer an active spawned subagent with additional user-authored input."
            }
            SubagentLifecycleAction::Cancel => {
                "Request cooperative cancellation of an active spawned subagent."
            }
            SubagentLifecycleAction::Close => {
                "Release a terminal subagent handle after its result has been consumed."
            }
        }
    }
    fn parameters_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::json!({
            "agentId": {
                "type": "string",
                "description": "Stable agent id returned by spawn_subagent"
            }
        });
        let required = if matches!(self.action, SubagentLifecycleAction::SendInput) {
            properties["input"] = serde_json::json!({
                "type": "string",
                "description": "Additional instruction to inject at the next safe model boundary"
            });
            vec!["agentId", "input"]
        } else {
            if matches!(self.action, SubagentLifecycleAction::Observe) {
                properties["afterSeq"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "description": "Return only lifecycle events after this cursor"
                });
                properties["waitMs"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "description": "Optional long-poll duration for new events"
                });
            } else if matches!(self.action, SubagentLifecycleAction::Wait) {
                properties["waitMs"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "default": 2500,
                    "description": "One steering-friendly wait quantum for terminal state"
                });
            }
            vec!["agentId"]
        };
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let args: SubagentLifecycleArgs =
            serde_json::from_str(context.arguments).map_err(|error| {
                CoreError::InvalidInput(format!("Invalid {} arguments: {error}", self.name()))
            })?;
        let agent_id = args.agent_id.trim();
        if agent_id.is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "{} requires agentId",
                self.name()
            )));
        }
        self.runtime
            .lifecycle
            .ensure_conversation(agent_id, self.runtime.parent_conversation_id.as_deref())?;
        let (content, artifacts) = match self.action {
            SubagentLifecycleAction::Observe => {
                let observation = self
                    .runtime
                    .lifecycle
                    .observe(
                        agent_id,
                        args.after_seq.unwrap_or(0),
                        Duration::from_millis(args.wait_ms.unwrap_or(0).min(2_500)),
                    )
                    .await?;
                let content = format!(
                    "Subagent {agent_id} is {:?}; received {} lifecycle event(s), cursor {}.",
                    observation.worker.status,
                    observation.events.len(),
                    observation.cursor,
                );
                (
                    content,
                    serde_json::json!({
                        "kind": "subagent_observation",
                        "observation": observation,
                    }),
                )
            }
            SubagentLifecycleAction::Wait => {
                let wait_result = self
                    .runtime
                    .lifecycle
                    .wait(
                        agent_id,
                        Duration::from_millis(args.wait_ms.unwrap_or(2_500).min(2_500)),
                    )
                    .await?;
                let result_text = wait_result
                    .worker
                    .result
                    .as_ref()
                    .and_then(|value| value.get("result"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let mut content = format!(
                    "Subagent {agent_id} is {:?}{}.",
                    wait_result.worker.status,
                    if wait_result.timed_out {
                        " (wait timed out; worker remains active)"
                    } else {
                        ""
                    }
                );
                if !result_text.is_empty() {
                    content.push_str("\n\n");
                    content.push_str(result_text);
                }
                (
                    content,
                    serde_json::json!({
                        "kind": "subagent_wait_result",
                        "worker": wait_result.worker,
                        "timedOut": wait_result.timed_out,
                    }),
                )
            }
            SubagentLifecycleAction::SendInput => {
                let input = args
                    .input
                    .as_deref()
                    .map(str::trim)
                    .filter(|input| !input.is_empty())
                    .ok_or_else(|| {
                        CoreError::InvalidInput("send_subagent_input requires input".into())
                    })?;
                let bridge = self
                    .runtime
                    .lifecycle
                    .send_input(agent_id, input.to_string())?;
                bridge
                    .emit(
                        SubagentLifecycleEventKind::InputQueued,
                        serde_json::json!({
                            "bytes": input.len(),
                            "state": "queued",
                            "acknowledgement": "channel_enqueue_only",
                        }),
                    )
                    .await?;
                (
                    format!("Input queued for subagent {agent_id}; wait for an inputApplied lifecycle event to confirm it reached a model boundary."),
                    serde_json::json!({
                        "kind": "subagent_input_queued",
                        "agentId": agent_id,
                        "state": "queued",
                    }),
                )
            }
            SubagentLifecycleAction::Cancel => {
                let bridge = self.runtime.lifecycle.cancel(agent_id)?;
                bridge
                    .emit(
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "status": "cancelling" }),
                    )
                    .await?;
                (
                    format!("Cancellation requested for subagent {agent_id}."),
                    serde_json::json!({
                        "kind": "subagent_cancellation",
                        "agentId": agent_id,
                        "status": "cancelling",
                    }),
                )
            }
            SubagentLifecycleAction::Close => {
                let snapshot = self.runtime.lifecycle.close(agent_id)?;
                (
                    format!("Closed terminal subagent handle {agent_id}."),
                    serde_json::json!({
                        "kind": "subagent_closed",
                        "worker": snapshot,
                    }),
                )
            }
        };
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(artifacts),
        })
    }
}
