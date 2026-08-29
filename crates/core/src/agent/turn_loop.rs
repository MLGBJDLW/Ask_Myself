//! Turn-level state machine and ReAct loop implementation.

use super::assistant_turn;
use super::finalization;
use super::model_step;
use super::output_recovery::{
    OutputRecovery, OutputRecoveryCause, OutputRecoveryDecision, OutputRecoveryFailure,
    ToolRoundRejectionCause,
};
use super::steering::SteeringDrainContext;
use super::tool_dispatch;
use super::turn_budget::{TurnBudget, TurnStepMode, TurnStepPurpose};
use super::turn_state::{TurnOutcome, TurnPhase, TurnStateMachine};
use super::usage_accounting;
use super::*;
use crate::llm::FinishReason;

struct ReplayableSystemPersistenceContext<'a> {
    db: &'a Database,
    conversation_id: Option<&'a str>,
    model: &'a str,
    layout: prompt_layout::PromptLayout,
    messages: &'a [Message],
    sort_order: &'a mut i64,
    persisted_contents: &'a mut Vec<String>,
}

fn awaiting_user_input_interaction_id(
    summaries: &[tool_dispatch::ToolDispatchSummary],
) -> Option<String> {
    summaries.iter().find_map(|summary| {
        if summary.is_error {
            return None;
        }
        let artifact = summary.artifacts.as_ref()?.as_object()?;
        (artifact.get("kind").and_then(serde_json::Value::as_str) == Some("questionRequest")
            && artifact.get("version").and_then(serde_json::Value::as_u64) == Some(2)
            && artifact.get("status").and_then(serde_json::Value::as_str) == Some("pending"))
        .then(|| {
            artifact
                .get("interactionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .flatten()
    })
}

fn successful_action_reconciliation_observation(
    calls: &[ToolCallRequest],
    summaries: &[tool_dispatch::ToolDispatchSummary],
) -> bool {
    calls.iter().any(|call| {
        let action = serde_json::from_str::<serde_json::Value>(&call.arguments)
            .ok()
            .and_then(|args| {
                args.get("action")
                    .and_then(serde_json::Value::as_str)
                    .map(|action| action.trim().to_ascii_lowercase())
            });
        let is_observation = call.name == "computer_observe"
            || (call.name == "browser_session" && action.as_deref() == Some("observe"));
        is_observation
            && summaries
                .iter()
                .find(|summary| summary.call_id == call.id)
                .is_some_and(|summary| !summary.is_error)
    })
}

fn successful_executable_action(
    calls: &[ToolCallRequest],
    summaries: &[tool_dispatch::ToolDispatchSummary],
) -> bool {
    calls.iter().any(|call| {
        loop_guard::tool_call_is_action_progress(&call.name)
            && summaries
                .iter()
                .find(|summary| summary.call_id == call.id)
                .is_some_and(|summary| !summary.is_error)
    })
}

fn cumulative_run_step_output_budget(
    configured_step_max: u32,
    actual_run_limit: Option<u32>,
    actual_spent: u32,
    estimated_prompt: u32,
) -> Option<u32> {
    let Some(limit) = actual_run_limit else {
        return Some(configured_step_max);
    };
    let remaining = limit.saturating_sub(actual_spent);
    (remaining > estimated_prompt.saturating_add(255))
        .then(|| configured_step_max.min(remaining.saturating_sub(estimated_prompt).max(256)))
}

async fn emit_tool_dispatch_failure(
    tx: &mpsc::Sender<AgentEvent>,
    db: &Database,
    trace: &mut Option<AgentTrace>,
    turn_id: Option<&str>,
    route_kind: AgentRouteKind,
    persisted_trace_items: &mut Vec<PersistedTraceItem>,
    error: &CoreError,
) {
    let frontend_message = "Nexa stopped the turn because a tool result could not be saved. No follow-up model request was sent.";
    let trace_message = format!("tool_result_persistence_failed: {error}");
    append_persisted_trace_status(persisted_trace_items, frontend_message, "error");
    emit_error_and_finalize_turn(
        tx,
        db,
        trace,
        turn_id,
        route_kind,
        persisted_trace_items,
        TurnErrorMessages {
            frontend_message: frontend_message.to_string(),
            trace_message,
        },
    )
    .await;
}

impl AgentExecutor {
    /// Run the agent loop for a single user turn.
    ///
    /// * `history` — prior conversation messages (already stored in DB).
    /// * `user_parts` — content parts for the new user input (text + optional images).
    /// * `db` — database handle passed through to tools and privacy config.
    /// * `conversation_id` — optional conversation ID for source scoping.
    /// * `tx` — channel for streaming [`AgentEvent`]s to the caller (e.g. Tauri).
    /// * `next_sort_order` — the sort_order to use for the first message saved
    ///   by the executor (intermediate + final). The caller should set this to
    ///   one past the last message it already persisted (e.g. the user message).
    ///
    /// Returns the final assistant [`Message`] on success.
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        history: Vec<Message>,
        user_parts: Vec<ContentPart>,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        tx: mpsc::Sender<AgentEvent>,
        next_sort_order: i64,
    ) -> Result<Message, CoreError> {
        self.run_with_source_scope(
            history,
            user_parts,
            db,
            conversation_id,
            turn_id,
            None,
            tx,
            next_sort_order,
        )
        .await
    }

    /// Run the agent loop with an optional explicit source scope override.
    ///
    /// This is primarily useful for short-lived delegated workers that should
    /// inherit the parent's retrieval scope without persisting their internal
    /// reasoning into the parent's conversation history.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_source_scope(
        &self,
        history: Vec<Message>,
        user_parts: Vec<ContentPart>,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        source_scope_override: Option<Vec<String>>,
        tx: mpsc::Sender<AgentEvent>,
        next_sort_order: i64,
    ) -> Result<Message, CoreError> {
        let model = self.config.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let output_budget_plan = self.config.resolved_output_budget(model);
        let max_response_tokens = output_budget_plan.effective_tokens;
        let mut turn_state = TurnStateMachine::new();

        // --- 0. Early cancellation check before any work ----------------------
        if self.cancel_token.is_cancelled() {
            turn_state.finish(TurnOutcome::Cancelled);
            let msg = Message::text(Role::Assistant, "Request cancelled by user.".to_string());
            let _ = tx
                .send(AgentEvent::Done {
                    message: msg.clone(),
                    usage_total: Usage::default(),
                    last_prompt_tokens: 0,
                    context_breakdown: None,
                    cached: false,
                    finish_reason: Some("stop".to_string()),
                })
                .await;
            return Ok(msg);
        }

        // --- 0b. Pre-summarize evicted history if context is getting full -----
        turn_state.transition_to(TurnPhase::PreparingContext);
        let history_before_summarization = prompt_cache::message_sequence_fingerprint(&history);
        let (history, pre_summarization_usage) = self
            .summarize_if_needed(
                history,
                model,
                max_response_tokens,
                context_compaction::CompactionRunContext {
                    db,
                    conversation_id,
                    turn_id,
                },
                self.config.max_actual_tokens_per_run,
            )
            .await?;
        let history_was_compacted =
            history_before_summarization != prompt_cache::message_sequence_fingerprint(&history);

        // --- 1. Build initial messages with context-window trimming -----------
        // Extract user query text early — used for both tool selection and
        // skill trigger matching.
        let user_query_text_for_tools: String = user_parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        let mut pending_action_reconciliation = user_query_text_for_tools
            .contains("Checkpoint reason: user_stop_requires_action_reconciliation:");

        let skills = self.skills_override.clone().unwrap_or_else(|| {
            crate::skills::get_available_skills_for_query(db, &user_query_text_for_tools)
                .unwrap_or_default()
        });
        let auto_loaded_skills = self.auto_loaded_skills_override.clone().unwrap_or_else(|| {
            crate::skills::select_skills_from_pool(skills.clone(), &user_query_text_for_tools, 3)
        });

        // --- Trace: initialize ------------------------------------------------
        let ctx_window_for_trace = self
            .config
            .context_window_resolution
            .and_then(|resolved| resolved.capacity_tokens)
            .or(self.config.context_window)
            .unwrap_or(0) as usize;
        let mut trace = if self.config.trace_enabled {
            Some(AgentTrace::begin(
                conversation_id.unwrap_or(""),
                &user_query_text_for_tools,
                model,
                ctx_window_for_trace,
            ))
        } else {
            None
        };

        // Resolve source scope early so we can pass `has_sources` into tool selection.
        let source_scope: Vec<String> =
            source_scope_override.unwrap_or_else(|| match conversation_id {
                Some(cid) => db
                    .get_effective_conversation_source_scope(cid)
                    .unwrap_or_default(),
                None => Vec::new(),
            });
        let has_sources = !source_scope.is_empty();
        turn_state.transition_to(TurnPhase::Planning);
        let route_plan = route_user_turn(
            &user_query_text_for_tools,
            &self.config.system_prompt,
            has_sources,
        );
        let mut loop_recorder = TurnLoopRecorder::new(route_plan.kind, self.config.max_iterations);
        let mut task_plan = build_task_plan(TaskPlanningInput {
            user_query: &user_query_text_for_tools,
            route_kind: route_plan.kind.as_str(),
            has_sources,
            source_scope_count: source_scope.len(),
            collection_context: system_prompt_has_collection_context(&self.config.system_prompt),
        });
        let orchestration_policy = resolve_orchestration_profile(OrchestrationProfileInput {
            profile: self.config.orchestration_profile,
            custom: self.config.custom_orchestration.clone(),
            max_iterations: self.config.max_iterations,
            max_parallel: self.config.subagent_max_parallel,
            max_calls_per_turn: self.config.subagent_max_calls_per_turn,
            delegated_token_budget: self.config.subagent_token_budget,
            verification_reserve_percent: self.config.subagent_verification_reserve_percent,
        });
        let mut workflow_ir = compile_workflow_ir(
            &task_plan,
            &orchestration_policy,
            self.config.power_mode.is_nexus(),
        )
        .map_err(CoreError::InvalidInput)?;
        if self.config.execution_mode.is_plan() {
            workflow_ir.configure_for_plan_mode();
        } else {
            if self.config.request_kind.requires_workspace_isolation() {
                workflow_ir.configure_for_scheduled_isolated_patch();
            }
            let verification_roots = if source_scope.is_empty() {
                let sources = db.list_sources()?;
                if sources.len() == 1 {
                    sources
                        .into_iter()
                        .map(|source| std::path::PathBuf::from(source.root_path))
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                source_scope
                    .iter()
                    .map(|source_id| {
                        db.get_source(source_id)
                            .map(|source| std::path::PathBuf::from(source.root_path))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };
            workflow_ir.configure_project_verification_support(
                crate::workflow_ir::detect_project_verification_support(&verification_roots),
            );
        }
        let mut workspace_isolation = if !self.config.execution_mode.is_plan()
            && (workflow_ir.requires_runtime_write_isolation()
                || self.config.request_kind.requires_workspace_isolation())
        {
            Some(WorkspaceIsolationRuntime::prepare(
                db,
                &source_scope,
                turn_id,
            )?)
        } else {
            None
        };
        let execution_source_scope = if let Some(source_id) = workspace_isolation
            .as_ref()
            .and_then(WorkspaceIsolationRuntime::source_id)
        {
            vec![source_id.to_string()]
        } else {
            source_scope.clone()
        };
        let task_plan_value = workflow_ir.task_plan_checkpoint(&task_plan);
        let workflow_ir_value = serde_json::to_value(&workflow_ir)
            .unwrap_or_else(|_| serde_json::json!({ "error": "serializeWorkflowIr" }));
        let _ = tx
            .send(AgentEvent::ControllerStatus {
                code: "route_selected".to_string(),
                content: format!("Route selected: {:?}", route_plan.kind),
                tone: Some("muted".to_string()),
            })
            .await;
        emit_task_plan_update(&tx, &task_plan, "planning", "Typed task plan created").await;
        let _ = tx
            .send(AgentEvent::ControllerStatus {
                code: "workflow_compiled".to_string(),
                content: format!(
                    "Workflow IR v{} compiled: {} nodes, {} verification gates",
                    workflow_ir.version,
                    workflow_ir.nodes.len(),
                    workflow_ir.verification_gates.len()
                ),
                tone: Some("muted".to_string()),
            })
            .await;
        if let Some(tid) = turn_id {
            let route_label = format!("{:?}", route_plan.kind);
            let _ = db.update_conversation_turn_progress(tid, Some(&route_label), None);
            if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                let _ = db.update_agent_task_run_progress(
                    &task_run.id,
                    Some("running"),
                    Some("planning"),
                    Some(route_plan.kind.as_str()),
                    Some("Planning execution and evidence requirements"),
                    Some(&task_plan_value),
                    None,
                );
            }
        }

        debug!("Agent route selected: {:?}", route_plan.kind);

        let expose_model_task_plan = self.config.power_mode.is_nexus()
            || self.config.orchestration_profile != OrchestrationProfile::Balanced
            || self.config.request_kind.requires_workspace_isolation();
        let balanced_model_tools =
            (!expose_model_task_plan).then(|| self.tools.without_names(&["update_plan"]));
        let model_tool_registry = balanced_model_tools.as_ref().unwrap_or(&self.tools);
        let layout =
            prompt_layout::PromptLayout::for_request(self.config.provider_type, Some(model));
        let effective_context_capacity = self
            .config
            .context_window_resolution
            .and_then(|resolved| resolved.capacity_tokens)
            .or(self.config.context_window);
        let cache_stable_tool_surface = if layout.allow_dynamic_tool_visibility {
            None
        } else {
            Some(prompt_layout::select_cache_stable_tool_surface(
                model_tool_registry,
                model,
                effective_context_capacity,
                max_response_tokens,
            )?)
        };
        let effective_dynamic_tool_visibility = cache_stable_tool_surface
            .as_ref()
            .map(prompt_layout::CacheStableToolSurface::uses_dynamic_discovery)
            .unwrap_or_else(|| {
                layout.effective_dynamic_tool_visibility(self.config.dynamic_tool_visibility)
            });
        let mut tool_defs = if let Some(surface) = cache_stable_tool_surface {
            debug!(
                mode = ?surface.mode,
                tool_count = surface.definitions.len(),
                "Selected cache-stable tool surface"
            );
            surface.definitions
        } else if effective_dynamic_tool_visibility {
            model_tool_registry.select_tools_for_decision(&route_plan.visibility_decision)
        } else {
            model_tool_registry.definitions()
        };
        if self.config.power_mode.is_nexus() {
            // Nexus orchestration must be available on the first model step.
            // Leaving delegation behind dynamic discovery made the high-power
            // mode behave like one large worker whenever routing omitted it.
            let delegation_tools = [
                "spawn_subagent_batch",
                "spawn_subagent",
                "judge_subagent_results",
            ]
            .iter()
            .filter_map(|name| self.tools.get(name).map(|tool| tool.definition()))
            .collect();
            tool_defs = merge_tool_definitions(tool_defs, delegation_tools);
        }
        if workspace_isolation.is_some() {
            WorkspaceIsolationRuntime::retain_safe_tool_definitions(&mut tool_defs);
        }
        if let Some(ref mut t) = trace {
            t.tools_offered = tool_defs.len() as u32;
            t.route_kind = Some(route_plan.kind.as_str().to_string());
            t.task_plan = Some(task_plan_value.clone());
            t.workflow_ir = Some(workflow_ir_value);
            t.orchestration_profile = Some(self.config.orchestration_profile.as_str().to_string());
            t.collaboration_mode = Some(self.config.collaboration_mode.as_str().to_string());
            t.tool_visibility_decision = Some(route_plan.visibility_decision.clone());
        }
        let volatile_system_sections = self
            .config
            .volatile_system_sections
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut controller_state_sections_owned = prompt_layout::turn_scaffolding_sections(
            &route_plan.prompt_section,
            expose_model_task_plan.then_some(&task_plan),
            effective_dynamic_tool_visibility && model_tool_registry.contains("tool_search"),
            layout,
        );
        if expose_model_task_plan {
            controller_state_sections_owned.push(orchestration_policy.prompt_section());
        }
        if let Some(isolation) = workspace_isolation.as_ref() {
            controller_state_sections_owned.push(isolation.prompt_section());
        }
        if self.config.power_mode.is_nexus() || self.config.orchestration_profile.is_ultra() {
            controller_state_sections_owned.push(workflow_ir.to_prompt_section());
        }
        let controller_state_sections = controller_state_sections_owned
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut messages = context::prepare_messages_with_options(
            &self.config.system_prompt,
            &history,
            &user_parts,
            model,
            max_response_tokens,
            self.config.context_window,
            &skills,
            &auto_loaded_skills,
            &tool_defs,
            context::PrepareMessagesOptions {
                include_skill_system_prompt: layout.include_skill_system_prompt,
                volatile_system_sections: &volatile_system_sections,
                evidence_sections: &[],
                controller_state_sections: &controller_state_sections,
                append_volatile_system_prompt_to_tail: layout.append_volatile_system_prompt_to_tail,
                endpoint_context_resolution: self.config.context_window_resolution,
            },
        );
        let current_user_was_preserved = messages
            .iter()
            .rev()
            .find(|message| message.role == Role::User)
            .is_some_and(|message| message.parts.as_slice() == user_parts.as_slice());
        if !current_user_was_preserved {
            return Err(CoreError::InvalidInput(
                "The current user message could not fit in the model context after reserving system, response, and tool budgets. Reduce enabled tools or use a larger context window."
                    .to_string(),
            ));
        }

        // --- 2. Privacy redaction on outgoing user content --------------------
        let privacy_cfg = db.load_privacy_config().unwrap_or_default();
        if privacy_cfg.enabled {
            for msg in &mut messages {
                if msg.role == Role::User {
                    for part in &mut msg.parts {
                        if let ContentPart::Text { text } = part {
                            *text = privacy::redact_content(text, &privacy_cfg.redact_patterns);
                        }
                    }
                }
            }
        }

        // Compaction is a real model action. Seed the run ledger with its
        // provider usage (or a conservative estimate for unreported attempts)
        // so delegated-worker hard caps cover every LLM request, not only the
        // visible ReAct samples.
        let mut total_usage = pre_summarization_usage;
        let mut last_prompt_tokens: u32 = 0;
        let mut last_context_breakdown: Option<context::ContextUsageBreakdown> = None;
        let mut sort_order = next_sort_order;
        let mut accumulated_content = String::new();
        let mut last_iteration_content = String::new();
        let mut last_finish_reason: Option<String> = None;
        let mut persisted_trace_items: Vec<PersistedTraceItem> = Vec::new();
        append_developer_persisted_trace_status(
            &mut persisted_trace_items,
            &output_budget_plan.diagnostic(),
            "info",
        );
        let mut persisted_replayable_system_contents: Vec<String> = Vec::new();
        for event in loop_recorder.events().iter().cloned() {
            append_persisted_trace_loop_event(&mut persisted_trace_items, event);
        }
        append_persisted_trace_visibility(
            &mut persisted_trace_items,
            &route_plan.visibility_decision,
        );
        append_persisted_trace_loaded_skills(&mut persisted_trace_items, &auto_loaded_skills);
        self.seed_prompt_cache_from_previous_turn(db, conversation_id, turn_id);

        // --- 3c. Extract user query text and build cache key -----------------
        let user_query_text = &user_query_text_for_tools;

        let cache_source_filter: Option<String> = if source_scope.is_empty() {
            None
        } else {
            let mut sorted = source_scope.clone();
            sorted.sort();
            Some(sorted.join(","))
        };

        // --- 3c'. Try direct dispatch (skip LLM for simple commands) ---------
        if !self.config.execution_mode.is_plan() {
            turn_state.transition_to(TurnPhase::DirectDispatch);
            if let Some(msg) = self
                .try_direct_dispatch(
                    user_query_text,
                    db,
                    &source_scope,
                    &tx,
                    conversation_id,
                    turn_id,
                    next_sort_order,
                )
                .await
            {
                turn_state.finish(TurnOutcome::DirectDispatch);
                return Ok(msg);
            }
        }

        // --- 3d. Check answer cache before ReAct loop ------------------------
        if !self.config.execution_mode.is_plan() {
            turn_state.transition_to(TurnPhase::CacheLookup);
            if let Some(msg) = self
                .try_cached_answer(
                    user_query_text,
                    cache_source_filter.as_deref(),
                    db,
                    &tx,
                    conversation_id,
                    turn_id,
                    model,
                    sort_order,
                    route_plan.kind,
                    &mut trace,
                )
                .await
            {
                turn_state.finish(TurnOutcome::Cached);
                return Ok(msg);
            }
        }

        // Macro for cancellation checkpoints — saves partial conversation and
        // returns gracefully when the token is cancelled.
        macro_rules! check_cancelled {
            ($last_tool_calls:expr, $long_task_state:expr, $task_plan:expr, $iteration:expr) => {
                if self.cancel_token.is_cancelled() {
                    warn!("Agent execution cancelled by user");
                    let live_state = $long_task_state.checkpoint_live_state(
                        &$task_plan,
                        Some(&workflow_ir),
                        $iteration,
                        self.config.max_iterations,
                        &loop_recorder,
                    );
                    if let Err(err) = create_task_checkpoint_for_turn_with_state(
                        db,
                        turn_id,
                        "cancelled",
                        Some(&live_state),
                    ) {
                        warn!("Failed to create cancellation resume checkpoint: {err}");
                    }
                    let final_msg = self
                        .finish_cancelled_turn(
                            finalization::CancellationFinalizationContext {
                                db,
                                tx: &tx,
                                conversation_id,
                                turn_id,
                                model,
                                route_kind: route_plan.kind,
                                persisted_trace_items: &mut persisted_trace_items,
                                loop_recorder: &mut loop_recorder,
                                trace: &mut trace,
                                sort_order: &mut sort_order,
                            },
                            $last_tool_calls.as_deref(),
                            &mut accumulated_content,
                            total_usage.clone(),
                            last_prompt_tokens,
                            last_context_breakdown.clone(),
                        )
                        .await;
                    turn_state.finish(TurnOutcome::Cancelled);
                    return Ok(final_msg);
                }
            };
        }

        // --- 3e. Auto pre-search for KnowledgeRetrieval route ----------------
        // Eagerly execute search_knowledge_base so the LLM already has evidence
        // in context instead of depending on it to call the tool itself.
        turn_state.transition_to(TurnPhase::PreSearch);
        let prefetch_observed = self
            .prefetch_knowledge_results(
                route_plan.kind,
                user_query_text,
                db,
                &source_scope,
                &tx,
                conversation_id,
                turn_id,
                model,
                &mut sort_order,
                &mut persisted_replayable_system_contents,
                &mut messages,
                &mut persisted_trace_items,
                &mut task_plan,
            )
            .await;

        // --- 4. ReAct loop ----------------------------------------------------
        let mut last_tool_calls: Option<Vec<ToolCallRequest>> = None;
        let mut context_recovery_attempts = 0u32;
        let context_pipeline = ContextPipeline::new_with_resolution(
            model,
            self.config.context_window,
            self.config.context_window_resolution,
            max_response_tokens,
        );
        let mut loop_guard = AgentLoopGuard::new();
        let mut long_task_state = LongTaskState::new();
        let mut force_non_streaming_llm = llm_streaming_disabled_by_env()
            || self.config.provider_type.is_some_and(|provider| {
                crate::llm::provider_uses_non_streaming_fallback(provider, model)
            });
        let mut reasoning_disabled_for_tool_loop = false;
        let mut model_action_observed = prefetch_observed;
        let mut prompt_was_compacted = history_was_compacted;

        // Nexus owns reconnaissance waves at runtime. This is a deterministic
        // controller action compiled from Workflow IR, including retries for
        // failed workers, not a suggestion that depends on the model deciding
        // to delegate.
        let automatic_reconnaissance = (self.config.power_mode.is_nexus()
            || self.config.orchestration_profile.is_ultra())
            && !self.config.execution_mode.is_plan()
            && self.config.request_kind == AgentRequestKind::MainAgentStep
            && !matches!(
                task_plan.delegation.mode,
                crate::intelligence::DelegationMode::Disabled
            )
            && self.tools.contains("spawn_subagent_batch");
        if automatic_reconnaissance {
            while let Some(arguments) =
                workflow_ir.reconnaissance_batch_arguments(&task_plan.objective)
            {
                let node_ids = workflow_ir
                    .ready_node_ids()
                    .into_iter()
                    .filter(|id| {
                        workflow_ir
                            .nodes
                            .iter()
                            .any(|node| node.id == *id && node.phase == "reconnaissance")
                    })
                    .collect::<Vec<_>>();
                for node_id in &node_ids {
                    workflow_ir
                        .start_node(node_id)
                        .map_err(CoreError::InvalidInput)?;
                }

                let call = ToolCallRequest {
                    id: format!("workflow-recon-{}", Uuid::new_v4()),
                    name: "spawn_subagent_batch".to_string(),
                    arguments: serde_json::to_string(&arguments)
                        .map_err(|error| CoreError::Internal(error.to_string()))?,
                    thought_signature: None,
                };
                let verified_call = VerifiedToolCallBatch::seal(vec![call.clone()], false, true)
                    .map_err(|_| {
                        CoreError::Internal(
                            "Workflow IR produced an invalid synthetic tool call".to_string(),
                        )
                    })?;
                let mut synthetic_assistant = Message {
                    role: Role::Assistant,
                    parts: vec![ContentPart::Text {
                        text: "Nexus is starting the independent reconnaissance wave compiled by Workflow IR."
                            .to_string(),
                    }],
                    name: None,
                    tool_calls: Some(vec![call.clone()]),
                    reasoning_content: None,
                    prompt_cache_hint: None,
                };
                let synthetic_route = crate::llm::provider_turn::RouteSnapshot::unknown(
                    "nexaController",
                    model,
                    ReasoningReplayPolicy::NotRequired,
                );
                let synthetic_envelope = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                    Uuid::new_v4().to_string(),
                    Uuid::new_v4().to_string(),
                    synthetic_route,
                    synthetic_assistant.text_content(),
                    None,
                    None,
                    vec![call.clone()],
                    false,
                );
                synthetic_assistant.set_provider_turn(synthetic_envelope);
                messages.push(synthetic_assistant.clone());
                let synthetic_envelope = self.persist_intermediate_tool_call_assistant(
                    assistant_turn::AssistantTurnPersistenceContext {
                        db,
                        conversation_id,
                        turn_id,
                        model,
                        route_kind: route_plan.kind,
                        persisted_trace_items: &mut persisted_trace_items,
                        sort_order: &mut sort_order,
                    },
                    &synthetic_assistant,
                    verified_call.as_slice(),
                    None,
                    "Nexus runtime scheduled the first Workflow IR reconnaissance wave.",
                )?;
                if let Some(message) = messages.last_mut() {
                    message.set_provider_turn(synthetic_envelope);
                }
                let status = format!(
                    "Nexus started {} independent reconnaissance workers from Workflow IR.",
                    node_ids.len()
                );
                append_internal_persisted_trace_status(&mut persisted_trace_items, &status, "info");
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "workflow_wave_started".to_string(),
                        content: status,
                        tone: Some("info".to_string()),
                    })
                    .await;

                turn_state.transition_to(TurnPhase::ToolDispatch);
                let mut started_call_ids = HashSet::new();
                let mut tool_run_started_ids = HashSet::new();
                let dispatch_outcome = match self
                    .dispatch_tool_calls(
                        tool_dispatch::ToolDispatchContext {
                            db,
                            tx: &tx,
                            conversation_id,
                            turn_id,
                            source_scope: &source_scope,
                            model,
                            privacy_cfg: &privacy_cfg,
                            route_kind: route_plan.kind,
                            iteration: 0,
                            tool_defs: &mut tool_defs,
                            messages: &mut messages,
                            persisted_trace_items: &mut persisted_trace_items,
                            task_plan: &mut task_plan,
                            loop_recorder: &mut loop_recorder,
                            loop_guard: &mut loop_guard,
                            trace: &mut trace,
                            sort_order: &mut sort_order,
                            pending_action_reconciliation,
                        },
                        &verified_call,
                        None,
                        &mut started_call_ids,
                        &mut tool_run_started_ids,
                    )
                    .await
                {
                    Ok(summaries) => summaries,
                    Err(error) => {
                        emit_tool_dispatch_failure(
                            &tx,
                            db,
                            &mut trace,
                            turn_id,
                            route_plan.kind,
                            &mut persisted_trace_items,
                            &error,
                        )
                        .await;
                        turn_state.finish(TurnOutcome::Failed);
                        return Err(error);
                    }
                };
                if let Some(reason) = dispatch_outcome.terminal_loop_guard_reason {
                    let trace_message =
                        format!("agent_loop_stopped_during_reconnaissance: {reason}");
                    emit_error_and_finalize_turn(
                        &tx,
                        db,
                        &mut trace,
                        turn_id,
                        route_plan.kind,
                        &persisted_trace_items,
                        TurnErrorMessages {
                            frontend_message: "Nexa stopped the reconnaissance wave after repeated tool failures continued beyond one recovery prompt.".to_string(),
                            trace_message: trace_message.clone(),
                        },
                    )
                    .await;
                    turn_state.finish(TurnOutcome::Failed);
                    return Err(CoreError::Agent(trace_message));
                }
                let summaries = dispatch_outcome.summaries;
                let summary = summaries.iter().find(|summary| summary.call_id == call.id);
                if summary.is_some_and(|summary| !summary.is_error) {
                    model_action_observed = true;
                }
                workflow_ir.apply_reconnaissance_batch_result(
                    &node_ids,
                    summary.and_then(|summary| summary.artifacts.as_ref()),
                    summary.is_none_or(|summary| summary.is_error),
                    summary
                        .map(|summary| summary.content.as_str())
                        .unwrap_or("Nexus reconnaissance returned no result."),
                );
                workflow_ir.apply_checkpoint_to_task_plan(&mut task_plan);
                emit_task_plan_update(
                    &tx,
                    &task_plan,
                    "tooling",
                    "Workflow IR reconnaissance checkpoint recorded",
                )
                .await;
                if let Some(tid) = turn_id {
                    if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                        let checkpoint = workflow_ir.task_plan_checkpoint(&task_plan);
                        let _ = db.update_agent_task_run_progress(
                            &task_run.id,
                            Some("running"),
                            Some("tooling"),
                            Some(route_plan.kind.as_str()),
                            Some("Workflow IR reconnaissance checkpoint recorded"),
                            Some(&checkpoint),
                            None,
                        );
                    }
                }
                if let Some(ref mut agent_trace) = trace {
                    agent_trace.workflow_ir = serde_json::to_value(&workflow_ir).ok();
                }
                let completed = node_ids
                    .iter()
                    .filter(|id| workflow_ir.checkpoint.completed_node_ids.contains(id))
                    .count();
                let status = format!(
                    "Nexus reconnaissance wave finished: {completed}/{} workflow nodes completed.",
                    node_ids.len()
                );
                append_internal_persisted_trace_status(
                    &mut persisted_trace_items,
                    &status,
                    if completed == node_ids.len() {
                        "success"
                    } else {
                        "warning"
                    },
                );
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "workflow_wave_completed".to_string(),
                        content: status,
                        tone: Some(if completed == node_ids.len() {
                            "success".to_string()
                        } else {
                            "warning".to_string()
                        }),
                    })
                    .await;
            }
        }

        let mut workflow_gate_repair_rounds = 0u8;
        let mut output_recovery = OutputRecovery::default();
        let mut turn_budget = TurnBudget::new(self.config.max_iterations);
        let mut next_step_purpose = TurnStepPurpose::Normal;
        'react_loop: loop {
            let Some(step_permit) = turn_budget.permit(next_step_purpose) else {
                break 'react_loop;
            };
            next_step_purpose = TurnStepPurpose::Normal;
            let iteration = step_permit.sample_index;
            turn_state.start_iteration(iteration);
            let step_started = TurnLoopEvent::StepStarted {
                iteration,
                remaining_iterations: step_permit.remaining_tool_rounds.unwrap_or(u32::MAX),
            };
            loop_recorder.record(step_started.clone());
            append_persisted_trace_loop_event(&mut persisted_trace_items, step_started);
            // ── Cancellation checkpoint: before LLM call ─────────────────
            check_cancelled!(
                last_tool_calls,
                long_task_state,
                task_plan,
                step_permit.tool_rounds_used
            );
            let steering_texts = {
                let mut steering_ctx = SteeringDrainContext {
                    db,
                    conversation_id,
                    tx: &tx,
                    model,
                    sort_order: &mut sort_order,
                    privacy_cfg: &privacy_cfg,
                };
                self.drain_steering_messages(&mut messages, &mut steering_ctx)
                    .await
            };
            if !steering_texts.is_empty() {
                self.expand_tool_defs_for_steering(&mut tool_defs, &steering_texts, has_sources);
                append_internal_persisted_trace_status(
                    &mut persisted_trace_items,
                    "Applied user steering before the next model step.",
                    "info",
                );
                let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
                messages = context_pipeline.trim_after_tool_results(&messages);
                prompt_was_compacted |=
                    before_trim != prompt_cache::message_sequence_fingerprint(&messages);
            }
            debug!(
                "Agent provider sample {}; tool rounds used={}, configured_limit={:?}",
                iteration + 1,
                step_permit.tool_rounds_used,
                turn_budget.configured_tool_round_limit(),
            );

            // A finite tool-round policy reserves a distinct answer-only sample
            // after the final verified tool round. Recovery samples retain that
            // mode without spending another logical tool round.
            if iteration > 0 || step_permit.mode == TurnStepMode::FinalAnswerOnly {
                let budget_hint = if step_permit.mode == TurnStepMode::FinalAnswerOnly {
                    "[System: The configured tool-round budget is complete. This is the reserved final-answer step. Do not call tools. Synthesize the complete answer from the evidence and tool results already available.]".to_string()
                } else if step_permit
                    .remaining_tool_rounds
                    .is_some_and(|remaining| remaining <= 1)
                {
                    "[System: One verified tool round remains before a separate answer-only synthesis step. Use it only for the most critical remaining action.]".to_string()
                } else if let (Some(remaining), Some(limit)) = (
                    step_permit.remaining_tool_rounds,
                    turn_budget.configured_tool_round_limit(),
                ) {
                    if remaining <= limit.saturating_div(2) {
                        format!(
                            "[System: You have {remaining} verified tool round(s) remaining, followed by a separate answer-only synthesis step.]"
                        )
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                if let Some(message) = prompt_ir::controller_state_message(budget_hint) {
                    messages.push(message);
                }
            }

            long_task_state.refresh_plan_recitation(
                &mut messages,
                &task_plan,
                step_permit.tool_rounds_used,
                self.config.max_iterations,
                layout.append_volatile_system_prompt_to_tail,
            );
            prompt_was_compacted |= self
                .compact_before_model_step_if_needed(LongTaskCompactionContext {
                    db,
                    conversation_id,
                    turn_id,
                    tx: &tx,
                    model,
                    messages: &mut messages,
                    context_pipeline,
                    tool_defs: &tool_defs,
                    turn_state: &mut turn_state,
                    loop_recorder: &mut loop_recorder,
                    persisted_trace_items: &mut persisted_trace_items,
                    total_usage: &mut total_usage,
                })
                .await;
            self.persist_unpersisted_replayable_system_messages(
                ReplayableSystemPersistenceContext {
                    db,
                    conversation_id,
                    model,
                    layout,
                    messages: &messages,
                    sort_order: &mut sort_order,
                    persisted_contents: &mut persisted_replayable_system_contents,
                },
            );

            // Tool discovery and steering can expand the surface between model
            // steps. Re-apply the isolation boundary so neither project
            // manifests nor external MCP processes can bypass the worktree.
            if workspace_isolation.is_some() {
                WorkspaceIsolationRuntime::retain_safe_tool_definitions(&mut tool_defs);
            }

            let force_answer_only = output_recovery.reserves_answer_channel()
                || step_permit.mode == TurnStepMode::FinalAnswerOnly;
            let estimated_prompt = if self.config.max_actual_tokens_per_run.is_some() {
                context::estimate_context_usage_breakdown_for_model(
                    model, &messages, &tool_defs, None,
                )
                .total_tokens
            } else {
                0
            };
            let Some(model_step_max_response_tokens) = cumulative_run_step_output_budget(
                max_response_tokens,
                self.config.max_actual_tokens_per_run,
                total_usage.total_tokens,
                estimated_prompt,
            ) else {
                let actual_limit = self.config.max_actual_tokens_per_run.unwrap_or_default();
                let remaining = actual_limit.saturating_sub(total_usage.total_tokens);
                let trace_message = format!(
                    "agent actual token limit exhausted before model step: spent={}, remaining={}, estimated_prompt={}, limit={actual_limit}",
                    total_usage.total_tokens, remaining, estimated_prompt,
                );
                append_developer_persisted_trace_status(
                    &mut persisted_trace_items,
                    &trace_message,
                    "error",
                );
                emit_error_and_finalize_turn(
                    &tx,
                    db,
                    &mut trace,
                    turn_id,
                    route_plan.kind,
                    &persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: "The delegated worker reached its cumulative token limit before another safe model step. Return the evidence already gathered or start a new worker with an explicit larger budget.".to_string(),
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                turn_state.finish(TurnOutcome::Failed);
                return Err(CoreError::Agent(trace_message));
            };
            let model_step_result = self
                .run_model_step(model_step::ModelStepContext {
                    db,
                    tx: &tx,
                    conversation_id,
                    turn_id,
                    route_kind: route_plan.kind,
                    model,
                    max_response_tokens: model_step_max_response_tokens,
                    has_sources,
                    privacy_cfg: &privacy_cfg,
                    messages: &mut messages,
                    tool_defs: &mut tool_defs,
                    accumulated_content: &mut accumulated_content,
                    persisted_trace_items: &mut persisted_trace_items,
                    trace: &mut trace,
                    sort_order: &mut sort_order,
                    context_recovery_attempts: &mut context_recovery_attempts,
                    force_non_streaming_llm: &mut force_non_streaming_llm,
                    reasoning_disabled_for_tool_loop: &mut reasoning_disabled_for_tool_loop,
                    force_answer_only,
                    suppress_tools: !step_permit.allows_tools(),
                    requires_first_action: !model_action_observed,
                    total_usage: &mut total_usage,
                })
                .await;
            let model_step = match model_step_result {
                Ok(step) => step,
                Err(error) => {
                    self.record_model_step_failure(
                        db,
                        conversation_id,
                        turn_id,
                        model,
                        iteration,
                        &error,
                    );
                    return Err(error);
                }
            };
            let model_step::ModelStepOutput {
                mut full_content,
                mut tool_calls,
                tool_call_assembly_rejected,
                provider_replay,
                chunk_usage,
                iteration_thinking,
                answer_delta_seen,
                thinking_delta_seen,
                finish_reason: step_finish_reason,
                mut started_call_ids,
                mut tool_run_started_ids,
                prompt_cache_observation,
                request_latency_ms,
                time_to_first_token_ms,
                sample_id,
                route_snapshot,
                reasoning_was_requested,
                discarded_sample_tokens,
            } = match model_step {
                model_step::ModelStepOutcome::Completed(output) => *output,
                model_step::ModelStepOutcome::Restart {
                    prompt_was_compacted: restart_was_compacted,
                } => {
                    prompt_was_compacted |= restart_was_compacted;
                    next_step_purpose = TurnStepPurpose::Recovery;
                    continue 'react_loop;
                }
            };
            model_action_observed |= !tool_run_started_ids.is_empty();
            if let Some(isolation) = workspace_isolation.as_mut() {
                isolation.rewrite_tool_calls(&mut tool_calls)?;
            }
            let step_finish_reason_kind = step_finish_reason.clone();
            let step_finish_reason = step_finish_reason
                .as_ref()
                .map(|reason| format!("{reason:?}").to_lowercase());
            last_finish_reason = step_finish_reason.clone();
            let request_was_compacted = prompt_was_compacted;
            if discarded_sample_tokens > 0 {
                total_usage.prompt_tokens = total_usage
                    .prompt_tokens
                    .saturating_add(discarded_sample_tokens);
                total_usage.total_tokens = total_usage
                    .total_tokens
                    .saturating_add(discarded_sample_tokens);
                append_developer_persisted_trace_status(
                    &mut persisted_trace_items,
                    &format!(
                        "discarded physical model sample accounted conservatively: estimated_tokens={discarded_sample_tokens}"
                    ),
                    "warning",
                );
            }
            // -- 4b. Accumulate usage ------------------------------------------
            let usage_report = self
                .record_model_step_usage(
                    usage_accounting::UsageAccountingContext {
                        db,
                        conversation_id,
                        turn_id,
                        tx: &tx,
                        model,
                        messages: &mut messages,
                        context_pipeline,
                        tool_defs: &tool_defs,
                        turn_state: &mut turn_state,
                        loop_recorder: &mut loop_recorder,
                        persisted_trace_items: &mut persisted_trace_items,
                        trace: &mut trace,
                        total_usage: &mut total_usage,
                        last_prompt_tokens: &mut last_prompt_tokens,
                        last_context_breakdown: &mut last_context_breakdown,
                    },
                    usage_accounting::ModelStepUsageObservation {
                        iteration,
                        tool_call_count: tool_calls.len(),
                        finish_reason: last_finish_reason.clone(),
                        chunk_usage,
                        request_latency_ms,
                        time_to_first_token_ms,
                        cache_outcome_reason: prompt_cache_observation
                            .as_ref()
                            .map(prompt_cache::PromptCacheTraceObservation::cache_outcome_reason),
                    },
                )
                .await;
            if let Some(observation) = prompt_cache_observation {
                if let Some(value) = prompt_cache::prompt_cache_observation_to_value(
                    &observation,
                    request_was_compacted,
                ) {
                    append_persisted_trace_prompt_cache(&mut persisted_trace_items, value);
                }
            }
            prompt_was_compacted = usage_report.compacted_after_step;

            if self.config.max_actual_tokens_per_run.is_some_and(|limit| {
                total_usage.total_tokens > limit
                    || (total_usage.total_tokens == limit && !tool_calls.is_empty())
            }) {
                let limit = self.config.max_actual_tokens_per_run.unwrap_or_default();
                let trace_message = format!(
                    "agent actual token limit reached before tool dispatch: spent={}, limit={limit}",
                    total_usage.total_tokens,
                );
                append_developer_persisted_trace_status(
                    &mut persisted_trace_items,
                    &trace_message,
                    "error",
                );
                emit_error_and_finalize_turn(
                    &tx,
                    db,
                    &mut trace,
                    turn_id,
                    route_plan.kind,
                    &persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: if tool_calls.is_empty() {
                            "The delegated worker exceeded its cumulative token limit; the over-budget final sample was rejected."
                                .to_string()
                        } else {
                            "The delegated worker reached its cumulative token limit. Its pending tool calls were not executed."
                                .to_string()
                        },
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                turn_state.finish(TurnOutcome::Failed);
                return Err(CoreError::Agent(trace_message));
            }

            let recovery_decision = output_recovery.observe(
                step_finish_reason_kind.as_ref(),
                &full_content,
                !tool_calls.is_empty(),
            );
            let mut tool_round_rejection_cause = None;
            let recovery_failure = match recovery_decision {
                OutputRecoveryDecision::Continue {
                    cause,
                    had_visible_content,
                } => {
                    append_persisted_trace_thinking(
                        &mut persisted_trace_items,
                        &iteration_thinking,
                    );
                    if had_visible_content
                        || matches!(
                            cause,
                            OutputRecoveryCause::ProviderPause | OutputRecoveryCause::ContextLimit
                        )
                    {
                        let recovery_reasoning =
                            self.reasoning_content_for_iteration(&iteration_thinking, false);
                        let mut recovery_message = Message {
                            role: Role::Assistant,
                            parts: vec![ContentPart::Text {
                                text: full_content.clone(),
                            }],
                            name: None,
                            tool_calls: None,
                            reasoning_content: recovery_reasoning.clone(),
                            prompt_cache_hint: None,
                        };
                        recovery_message.set_provider_turn(
                            crate::llm::provider_turn::ProviderTurnEnvelope::capture_with_replay_payload(
                                Uuid::new_v4().to_string(),
                                sample_id,
                                route_snapshot,
                                recovery_message.text_content(),
                                crate::llm::reasoning_replay::sanitize_reasoning_text(Some(
                                    &iteration_thinking,
                                ))
                                .as_deref(),
                                recovery_reasoning.as_deref(),
                                Vec::new(),
                                reasoning_was_requested,
                                provider_replay,
                            ),
                        );
                        messages.push(recovery_message);
                    }

                    let (code, status) = match cause {
                        OutputRecoveryCause::OutputLimit => (
                            "output_limit_continuation",
                            format!(
                                "The provider reached its per-request output limit ({model_step_max_response_tokens} tokens; {}). Nexa is continuing automatically; this recovery does not consume a tool round.",
                                output_budget_plan.authority.label(),
                            ),
                        ),
                        OutputRecoveryCause::EmptyTerminal => (
                            "final_answer_recovery",
                            "The provider ended without answer text. Nexa is continuing once; this recovery does not consume a tool round.".to_string(),
                        ),
                        OutputRecoveryCause::ProviderPause => (
                            "provider_pause_continuation",
                            "The provider paused its server-side tool turn. Nexa is resuming from committed provider state; this does not consume a local tool round.".to_string(),
                        ),
                        OutputRecoveryCause::ContextLimit => (
                            "context_limit_rollover",
                            "The provider reached the model context limit. Nexa is rolling context forward from committed history; this does not consume a tool round.".to_string(),
                        ),
                    };
                    append_internal_persisted_trace_status(
                        &mut persisted_trace_items,
                        &status,
                        "warning",
                    );
                    let _ = tx
                        .send(AgentEvent::ControllerStatus {
                            code: code.to_string(),
                            content: status,
                            tone: Some("warning".to_string()),
                        })
                        .await;
                    if let Some(message) = prompt_ir::controller_state_message(
                        cause.controller_prompt(had_visible_content),
                    ) {
                        messages.push(message);
                    }
                    next_step_purpose = TurnStepPurpose::Recovery;
                    continue 'react_loop;
                }
                OutputRecoveryDecision::RejectToolRound(cause) => {
                    tool_round_rejection_cause = Some(cause);
                    append_internal_persisted_trace_status(
                        &mut persisted_trace_items,
                        "The provider terminal state did not commit a safe tool-call response. The draft calls will be rejected and re-planned without execution.",
                        "warning",
                    );
                    None
                }
                OutputRecoveryDecision::ToolRound => None,
                OutputRecoveryDecision::Final(final_content) => {
                    full_content = final_content;
                    None
                }
                OutputRecoveryDecision::Reject(failure) => Some(failure),
            };

            if !step_permit.allows_tools()
                && !tool_calls.is_empty()
                && tool_round_rejection_cause.is_none()
            {
                tool_round_rejection_cause = Some(ToolRoundRejectionCause::ToolsSuppressed);
                append_internal_persisted_trace_status(
                    &mut persisted_trace_items,
                    "The provider returned client tool calls during an answer-only sample. Nexa rejected them at the dispatch boundary.",
                    "warning",
                );
            }

            if let Some(recovery_failure) = recovery_failure {
                let finish_reason = step_finish_reason.as_deref().unwrap_or("unknown");
                let frontend_message = match &recovery_failure {
                    OutputRecoveryFailure::ContentFiltered => "The provider blocked the response before producing a final answer. Its reasoning was kept separate; revise the request and try again.".to_string(),
                    OutputRecoveryFailure::OutputLimit => "The provider repeatedly reached its per-request output limit without producing new answer or verified tool progress. Nexa stopped the stalled recovery; the configured tool-round budget was not the cause.".to_string(),
                    OutputRecoveryFailure::EmptyTerminal => "The provider repeatedly finished without producing a final answer in the answer channel. Its reasoning was kept separate; retry the response or choose another model.".to_string(),
                    OutputRecoveryFailure::MalformedToolCall => "The provider reported a malformed tool call without a recoverable committed envelope. Nexa executed no draft call; retry with a different model or provider route.".to_string(),
                    OutputRecoveryFailure::ProtocolIncomplete => "The provider ended without the terminal protocol required by this route. Nexa executed no draft tool call; verify the endpoint dialect or choose another provider route.".to_string(),
                    OutputRecoveryFailure::UnsupportedTerminal(raw) => format!("The provider returned an unsupported terminal reason ('{raw}'). Nexa treated it conservatively and executed no draft tool call; update this endpoint's compatibility profile or choose another route."),
                };
                let trace_message = format!(
                    "provider_finished_without_answer: finish_reason={finish_reason}, recovery_failure={recovery_failure:?}, answer_delta_seen={answer_delta_seen}, thinking_delta_seen={thinking_delta_seen}"
                );
                append_persisted_trace_status(
                    &mut persisted_trace_items,
                    &frontend_message,
                    "error",
                );
                emit_error_and_finalize_turn(
                    &tx,
                    db,
                    &mut trace,
                    turn_id,
                    route_plan.kind,
                    &persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message,
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                turn_state.finish(TurnOutcome::Failed);
                return Err(CoreError::Agent(trace_message));
            }

            let protocol_guard_calls = tool_calls.clone();
            let verified_tool_calls = match VerifiedToolCallBatch::seal(
                tool_calls,
                tool_call_assembly_rejected,
                step_finish_reason_kind
                    .as_ref()
                    .is_some_and(FinishReason::allows_completed_client_tools)
                    && tool_round_rejection_cause.is_none(),
            ) {
                Ok(verified) => verified,
                Err(rejected) => {
                    // Provider streams may terminate after emitting only part of a
                    // function call. Never persist, replay, or execute that partial
                    // protocol envelope. Re-plan from a plain controller message so
                    // the next request is valid even when the partial assistant also
                    // contained visible text.
                    if accumulated_content.ends_with(&full_content) {
                        accumulated_content
                            .truncate(accumulated_content.len().saturating_sub(full_content.len()));
                    }
                    let rejection_reason = match tool_round_rejection_cause {
                        Some(ToolRoundRejectionCause::OutputLimit) => "The provider reached its output limit while assembling a tool call. Nexa discarded the truncated parameters before re-planning.",
                        Some(ToolRoundRejectionCause::ProviderPause) => "The provider paused before the client tool envelope committed. Nexa discarded the draft call before resuming provider state.",
                        Some(ToolRoundRejectionCause::ContextLimit) => "The provider reached the context limit before the tool envelope committed. Nexa discarded the draft call before context rollover.",
                        Some(ToolRoundRejectionCause::MalformedToolCall) => "The provider reported a malformed tool call. Nexa discarded the draft before re-planning.",
                        Some(ToolRoundRejectionCause::ToolsSuppressed) => "The provider returned tool calls during the reserved answer-only step. Nexa discarded them before re-planning.",
                        Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => "The provider returned an incomplete tool-call envelope, so Nexa discarded that sample before re-planning.",
                    };
                    let _ = tx
                        .send(AgentEvent::StreamReset {
                            reason: rejection_reason.to_string(),
                            discard_sample: true,
                        })
                        .await;
                    let trace_message = format!(
                        "provider_returned_incomplete_tool_calls: incomplete_count={}, duplicate_id_count={}, oversized_count={}, assembly_rejected={}, terminal_rejected={}",
                        rejected.incomplete_count,
                        rejected.duplicate_id_count,
                        rejected.oversized_count,
                        rejected.assembly_rejected,
                        rejected.terminal_rejected,
                    );
                    debug!("{trace_message}");
                    let rejection_status = match tool_round_rejection_cause {
                        Some(ToolRoundRejectionCause::OutputLimit) => "The provider output limit truncated tool parameters. Nexa rejected them before execution; large create_file requests should be split into create plus append operations.",
                        Some(ToolRoundRejectionCause::ProviderPause) => "The provider paused with an uncommitted client tool draft. Nexa rejected the draft and will resume only from committed provider state.",
                        Some(ToolRoundRejectionCause::ContextLimit) => "The model context ended with an uncommitted tool draft. Nexa rejected it before context rollover.",
                        Some(ToolRoundRejectionCause::MalformedToolCall) => "The provider reported a malformed tool call. Nexa rejected it before persistence or execution and requested a fresh plan.",
                        Some(ToolRoundRejectionCause::ToolsSuppressed) => "The provider returned a tool call after client tools were suppressed for final synthesis. Nexa rejected it before persistence or execution.",
                        Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => "The provider returned an incomplete tool-call envelope. Nexa discarded it before persistence or execution and requested a fresh plan.",
                    };
                    append_internal_persisted_trace_status(
                        &mut persisted_trace_items,
                        rejection_status,
                        "warning",
                    );
                    let _ = tx
                        .send(AgentEvent::ControllerStatus {
                            code: match tool_round_rejection_cause {
                                Some(ToolRoundRejectionCause::OutputLimit) => {
                                    "tool_calls_truncated_by_output_limit"
                                }
                                Some(ToolRoundRejectionCause::ProviderPause) => {
                                    "tool_calls_rejected_at_provider_pause"
                                }
                                Some(ToolRoundRejectionCause::ContextLimit) => {
                                    "tool_calls_rejected_at_context_limit"
                                }
                                Some(ToolRoundRejectionCause::MalformedToolCall) => {
                                    "malformed_tool_calls_rejected"
                                }
                                Some(ToolRoundRejectionCause::ToolsSuppressed) => {
                                    "answer_only_tool_calls_rejected"
                                }
                                Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => {
                                    "incomplete_tool_calls_rejected"
                                }
                            }
                            .to_string(),
                            content: rejection_status.to_string(),
                            tone: Some("warning".to_string()),
                        })
                        .await;
                    // Assembly fragments are provider protocol drafts, not tool
                    // executions. StreamReset already removes their preparing
                    // previews; keep the rejection as controller/internal trace
                    // state instead of manufacturing a failed chat tool card.

                    let protocol_fault_code = match tool_round_rejection_cause {
                        Some(ToolRoundRejectionCause::OutputLimit) => "output_limit_tool_envelope",
                        Some(ToolRoundRejectionCause::ProviderPause) => {
                            "provider_pause_tool_envelope"
                        }
                        Some(ToolRoundRejectionCause::ContextLimit) => {
                            "context_limit_tool_envelope"
                        }
                        Some(ToolRoundRejectionCause::MalformedToolCall) => {
                            "malformed_tool_envelope"
                        }
                        Some(ToolRoundRejectionCause::ToolsSuppressed) => {
                            "answer_only_tool_envelope"
                        }
                        Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => {
                            "incomplete_tool_envelope"
                        }
                    };
                    if let Some(intervention) = loop_guard
                        .observe_protocol_rejection(protocol_fault_code, &protocol_guard_calls)
                    {
                        append_developer_persisted_trace_status(
                            &mut persisted_trace_items,
                            &intervention.reason,
                            if intervention.action == LoopGuardAction::StopLoop {
                                "error"
                            } else {
                                "warning"
                            },
                        );
                        if intervention.action == LoopGuardAction::StopLoop {
                            let trace_message =
                                format!("provider_tool_protocol_stalled: {}", intervention.reason);
                            emit_error_and_finalize_turn(
                                &tx,
                                db,
                                &mut trace,
                                turn_id,
                                route_plan.kind,
                                &persisted_trace_items,
                                TurnErrorMessages {
                                    frontend_message: "The provider repeatedly returned the same rejected tool envelope without committed progress. Nexa stopped the no-progress loop without executing the draft tool call; the configured tool-round budget was not exhausted.".to_string(),
                                    trace_message: trace_message.clone(),
                                },
                            )
                            .await;
                            turn_state.finish(TurnOutcome::Failed);
                            return Err(CoreError::Agent(trace_message));
                        }
                        if let Some(message) =
                            prompt_ir::controller_state_message(intervention.prompt)
                        {
                            messages.push(message);
                        }
                    }

                    {
                        let replan_instruction = match tool_round_rejection_cause {
                            Some(ToolRoundRejectionCause::OutputLimit) => {
                                let recommended_chars = output_budget_plan
                                    .recommended_text_tool_chunk_chars(
                                        model_step_max_response_tokens,
                                    );
                                format!(
                                    "The previous provider response hit its {model_step_max_response_tokens}-token output limit while assembling a tool call and was discarded before execution. Re-plan from the user request. For create_file/write_note, keep each content value below about {recommended_chars} characters, create a small first chunk, then use append operations with nextExpectedBytes. Otherwise emit one complete tool call with valid JSON arguments. This protocol repair does not consume a verified tool round."
                                )
                            }
                            Some(ToolRoundRejectionCause::ProviderPause) => "The provider paused before its client tool draft committed. Resume from committed provider state only; do not execute or continue the partial client call. This recovery does not consume a verified tool round.".to_string(),
                            Some(ToolRoundRejectionCause::ContextLimit) => "The model context ended before its tool draft committed. Re-plan after context rollover and emit a fresh, complete call; do not continue the partial envelope. This recovery does not consume a verified tool round.".to_string(),
                            Some(ToolRoundRejectionCause::MalformedToolCall) => "The provider reported a malformed tool call. Emit a fresh call whose id and name are non-empty and whose arguments are one exact JSON object. Do not continue the malformed envelope. This repair does not consume a verified tool round.".to_string(),
                            Some(ToolRoundRejectionCause::ToolsSuppressed) => "Client tools are unavailable because this is the reserved answer-only synthesis step. Do not emit any tool call. Produce the best complete visible answer from the committed evidence and tool results already in context. This protocol repair does not consume another tool round.".to_string(),
                            Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => "The previous provider response contained an incomplete tool-call envelope and was discarded before execution. Re-plan from the user request. If a tool is still needed, emit a new call with a non-empty id and name plus one complete JSON object for arguments; do not continue the partial call. This protocol repair does not consume a verified tool round.".to_string(),
                        };
                        if let Some(message) =
                            prompt_ir::controller_state_message(replan_instruction)
                        {
                            messages.push(message);
                        }
                        next_step_purpose = TurnStepPurpose::Recovery;
                        continue 'react_loop;
                    }
                }
            };
            let tool_calls = verified_tool_calls.as_slice().to_vec();

            last_iteration_content = full_content.clone();

            // -- 4c. Build assistant message -----------------------------------
            let assistant_reasoning_content =
                self.reasoning_content_for_iteration(&iteration_thinking, !tool_calls.is_empty());
            let mut assistant_msg = Message {
                role: Role::Assistant,
                parts: vec![ContentPart::Text { text: full_content }],
                name: None,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.clone())
                },
                reasoning_content: assistant_reasoning_content.clone(),
                prompt_cache_hint: None,
            };
            let provider_turn_envelope =
                crate::llm::provider_turn::ProviderTurnEnvelope::capture_with_replay_payload(
                    Uuid::new_v4().to_string(),
                    sample_id,
                    route_snapshot,
                    assistant_msg.text_content(),
                    crate::llm::reasoning_replay::sanitize_reasoning_text(Some(
                        &iteration_thinking,
                    ))
                    .as_deref(),
                    assistant_reasoning_content.as_deref(),
                    tool_calls.clone(),
                    reasoning_was_requested,
                    provider_replay,
                );
            assistant_msg.set_provider_turn(provider_turn_envelope);
            messages.push(assistant_msg.clone());
            let loop_guard_intervention =
                loop_guard.observe_model_step(&assistant_msg.text_content(), &tool_calls);

            if let Some(intervention) = loop_guard_intervention.as_ref() {
                if intervention.action == LoopGuardAction::StopLoop {
                    let event = TurnLoopEvent::LoopGuardIntervention {
                        reason: intervention.reason.clone(),
                        action: intervention.action.as_str().to_string(),
                    };
                    loop_recorder.record(event.clone());
                    append_persisted_trace_loop_event(&mut persisted_trace_items, event);
                    append_developer_persisted_trace_status(
                        &mut persisted_trace_items,
                        &intervention.reason,
                        "error",
                    );
                    append_persisted_trace_thinking(
                        &mut persisted_trace_items,
                        &iteration_thinking,
                    );
                    messages.pop();
                    accumulated_content.truncate(
                        accumulated_content
                            .len()
                            .saturating_sub(assistant_msg.text_content().len()),
                    );
                    last_iteration_content.clear();
                    let _ = tx
                        .send(AgentEvent::StreamReset {
                            reason: "A repeated agent loop was stopped before another model or tool step."
                                .to_string(),
                            discard_sample: true,
                        })
                        .await;
                    let trace_message = format!("agent_loop_stopped: {}", intervention.reason);
                    emit_error_and_finalize_turn(
                        &tx,
                        db,
                        &mut trace,
                        turn_id,
                        route_plan.kind,
                        &persisted_trace_items,
                        TurnErrorMessages {
                            frontend_message: "Nexa stopped this turn after the model repeated the same unproductive step following one bounded recovery prompt. No further repeated tool call was executed; retry with a different strategy or narrower task.".to_string(),
                            trace_message: trace_message.clone(),
                        },
                    )
                    .await;
                    turn_state.finish(TurnOutcome::Failed);
                    return Err(CoreError::Agent(trace_message));
                }
            }

            // -- 4d. Check termination -----------------------------------------
            if tool_calls.is_empty() {
                if let Some(intervention) = loop_guard_intervention.as_ref() {
                    if intervention.action == LoopGuardAction::ChangeStrategy
                        && turn_budget.can_start_normal_step()
                    {
                        let event = TurnLoopEvent::LoopGuardIntervention {
                            reason: intervention.reason.clone(),
                            action: intervention.action.as_str().to_string(),
                        };
                        loop_recorder.record(event.clone());
                        append_persisted_trace_loop_event(&mut persisted_trace_items, event);
                        append_developer_persisted_trace_status(
                            &mut persisted_trace_items,
                            &intervention.reason,
                            "warning",
                        );
                        let _ = tx
                            .send(AgentEvent::ControllerStatus {
                                code: "loop_guard_intervention".to_string(),
                                content: intervention.reason.clone(),
                                tone: Some("warning".to_string()),
                            })
                            .await;
                        self.persist_loop_guard_assistant_draft(
                            assistant_turn::AssistantTurnPersistenceContext {
                                db,
                                conversation_id,
                                turn_id,
                                model,
                                route_kind: route_plan.kind,
                                persisted_trace_items: &mut persisted_trace_items,
                                sort_order: &mut sort_order,
                            },
                            &assistant_msg,
                            assistant_reasoning_content.clone(),
                            &iteration_thinking,
                        );
                        if let Some(message) =
                            prompt_ir::controller_state_message(intervention.prompt.clone())
                        {
                            messages.push(message);
                        }
                        continue;
                    }
                }
                let steering_messages = self.collect_steering_messages(None).await;
                let has_effective_steering = steering_messages
                    .iter()
                    .any(Self::steering_message_has_effective_content);
                if has_effective_steering {
                    self.persist_steered_assistant_draft(
                        assistant_turn::AssistantTurnPersistenceContext {
                            db,
                            conversation_id,
                            turn_id,
                            model,
                            route_kind: route_plan.kind,
                            persisted_trace_items: &mut persisted_trace_items,
                            sort_order: &mut sort_order,
                        },
                        &assistant_msg,
                        assistant_reasoning_content.clone(),
                        &iteration_thinking,
                    );
                }
                let steering_texts = {
                    let mut steering_ctx = SteeringDrainContext {
                        db,
                        conversation_id,
                        tx: &tx,
                        model,
                        sort_order: &mut sort_order,
                        privacy_cfg: &privacy_cfg,
                    };
                    self.apply_steering_messages(
                        &mut messages,
                        &mut steering_ctx,
                        steering_messages,
                    )
                    .await
                };
                if !steering_texts.is_empty() {
                    self.expand_tool_defs_for_steering(
                        &mut tool_defs,
                        &steering_texts,
                        has_sources,
                    );
                    let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
                    messages = context_pipeline.trim_after_tool_results(&messages);
                    prompt_was_compacted |=
                        before_trim != prompt_cache::message_sequence_fingerprint(&messages);
                    continue;
                }
                if workflow_ir.completion_contract.require_verification_gates {
                    workflow_ir.sync_from_task_plan(&task_plan);
                    let final_audit = audit_final_answer(
                        &task_plan,
                        &assistant_msg.text_content(),
                        evidence_signals_from_trace(&persisted_trace_items),
                    )
                    .to_artifact();
                    workflow_ir.observe_final_answer_audit(&final_audit);
                    if self.config.request_kind.requires_workspace_isolation()
                        && workflow_ir.ready_for_runtime_independent_review()
                    {
                        let review = workspace_isolation
                            .as_ref()
                            .ok_or_else(|| {
                                CoreError::Internal(
                                    "scheduled isolated review has no controller runtime"
                                        .to_string(),
                                )
                            })?
                            .review_isolated_patch();
                        match review {
                            Ok(report) => {
                                workflow_ir
                                    .record_runtime_independent_review(true, report.detail.clone());
                                let _ = tx
                                    .send(AgentEvent::ControllerStatus {
                                        code: "workspace_isolation_reviewed".to_string(),
                                        content: report.detail,
                                        tone: Some("success".to_string()),
                                    })
                                    .await;
                            }
                            Err(error) => {
                                workflow_ir
                                    .record_runtime_independent_review(false, error.to_string());
                            }
                        }
                    }
                    if (workflow_ir.requires_runtime_write_isolation()
                        || self.config.request_kind.requires_workspace_isolation())
                        && workflow_ir.ready_to_promote_isolated_writes()
                    {
                        let promotion = workspace_isolation
                            .as_mut()
                            .ok_or_else(|| {
                                CoreError::Internal(
                                    "write-isolation gate has no controller runtime".to_string(),
                                )
                            })?
                            .promote_verified_patch();
                        match promotion {
                            Ok(report) => {
                                workflow_ir
                                    .record_runtime_write_isolation(true, report.detail.clone());
                                let _ = tx
                                    .send(AgentEvent::ControllerStatus {
                                        code: "workspace_isolation_promoted".to_string(),
                                        content: report.detail,
                                        tone: Some("success".to_string()),
                                    })
                                    .await;
                            }
                            Err(error) => {
                                workflow_ir
                                    .record_runtime_write_isolation(false, error.to_string());
                            }
                        }
                    }
                    if let Some(ref mut agent_trace) = trace {
                        agent_trace.workflow_ir = serde_json::to_value(&workflow_ir).ok();
                    }
                    if !workflow_ir.completion_allowed() {
                        let blockers = workflow_ir.completion_blockers();
                        let status =
                            format!("Workflow completion blocked by: {}", blockers.join(", "));
                        append_developer_persisted_trace_status(
                            &mut persisted_trace_items,
                            &status,
                            "warning",
                        );
                        let _ = tx
                            .send(AgentEvent::ControllerStatus {
                                code: "workflow_gate_blocked".to_string(),
                                content: status,
                                tone: Some("warning".to_string()),
                            })
                            .await;
                        let repair_limit = orchestration_policy.retry_limit.max(1);
                        if workflow_gate_repair_rounds >= repair_limit
                            || !turn_budget.can_start_normal_step()
                        {
                            append_persisted_trace_status(
                                &mut persisted_trace_items,
                                "Workflow repair limit reached before all completion gates passed.",
                                "error",
                            );
                            break 'react_loop;
                        }
                        workflow_gate_repair_rounds = workflow_gate_repair_rounds.saturating_add(1);
                        if let Some(message) = prompt_ir::controller_state_message(format!(
                            "Workflow IR refused finalization. Resolve these blockers with concrete tool calls before answering again: {}. Run the required checks, record their exact passed/failed outcomes with record_verification, and use an independent reviewer when that gate is listed. Do not merely claim completion.",
                            blockers.join(", ")
                        )) {
                            messages.push(message);
                        }
                        continue;
                    }
                }
                let active_goal = if self.config.execution_mode.is_plan()
                    || !self.config.request_kind.is_main_agent()
                    || !turn_budget.can_start_normal_step()
                {
                    None
                } else {
                    conversation_id
                        .and_then(|id| db.get_conversation_goal(id).ok().flatten())
                        .filter(|goal| {
                            goal.status == crate::conversation::ConversationGoalStatus::Active
                        })
                };
                if let Some(goal) = active_goal {
                    let status = format!(
                        "Goal remains active; continuing execution: {}",
                        goal.objective
                    );
                    append_developer_persisted_trace_status(
                        &mut persisted_trace_items,
                        &status,
                        "info",
                    );
                    let _ = tx
                        .send(AgentEvent::ControllerStatus {
                            code: "goal_continuation".to_string(),
                            content: status,
                            tone: Some("info".to_string()),
                        })
                        .await;
                    if let Some(message) = prompt_ir::controller_state_message(format!(
                        "The durable goal is still active: {}\n\nDo not end this turn with a plan, progress update, or partial answer. Continue taking concrete actions. When the objective is actually achieved and verified, call update_goal with status complete. If and only if progress genuinely requires user input or an external state change, call update_goal with status blocked.",
                        goal.objective
                    )) {
                        messages.push(message);
                    }
                    continue;
                }
                append_persisted_trace_thinking(&mut persisted_trace_items, &iteration_thinking);
                turn_state.transition_to(TurnPhase::Finalizing);
                let finalized = self
                    .finish_successful_turn(
                        finalization::TurnFinalizationContext {
                            db,
                            tx: &tx,
                            conversation_id,
                            turn_id,
                            model,
                            route_kind: route_plan.kind,
                            persisted_trace_items: &mut persisted_trace_items,
                            task_plan: &mut task_plan,
                            loop_recorder: &mut loop_recorder,
                            trace: &mut trace,
                            sort_order,
                        },
                        assistant_msg,
                        assistant_reasoning_content,
                        answer_delta_seen,
                        user_query_text,
                        cache_source_filter.as_deref(),
                        total_usage,
                        last_prompt_tokens,
                        last_context_breakdown,
                        last_finish_reason,
                    )
                    .await;
                let assistant_msg = match finalized {
                    Ok(message) => message,
                    Err(error) => {
                        turn_state.finish(TurnOutcome::Failed);
                        return Err(error);
                    }
                };
                turn_state.finish(TurnOutcome::Success);
                return Ok(assistant_msg);
            }

            // -- 4d'. Save intermediate assistant message (with tool_calls) ----
            let provider_turn_envelope = self.persist_intermediate_tool_call_assistant(
                assistant_turn::AssistantTurnPersistenceContext {
                    db,
                    conversation_id,
                    turn_id,
                    model,
                    route_kind: route_plan.kind,
                    persisted_trace_items: &mut persisted_trace_items,
                    sort_order: &mut sort_order,
                },
                &assistant_msg,
                &tool_calls,
                assistant_reasoning_content.clone(),
                &iteration_thinking,
            )?;
            assistant_msg.set_provider_turn(provider_turn_envelope.clone());
            if let Some(message) = messages.last_mut() {
                message.set_provider_turn(provider_turn_envelope);
            }

            last_tool_calls = Some(tool_calls.clone());

            // ── Cancellation checkpoint: before tool execution ────────
            check_cancelled!(
                last_tool_calls,
                long_task_state,
                task_plan,
                turn_budget.tool_rounds_used()
            );

            let loop_guard_block_reason = loop_guard_intervention
                .as_ref()
                .filter(|intervention| intervention.action == LoopGuardAction::BlockToolCalls)
                .map(|intervention| {
                    let event = TurnLoopEvent::LoopGuardIntervention {
                        reason: intervention.reason.clone(),
                        action: intervention.action.as_str().to_string(),
                    };
                    loop_recorder.record(event.clone());
                    append_persisted_trace_loop_event(&mut persisted_trace_items, event);
                    append_developer_persisted_trace_status(
                        &mut persisted_trace_items,
                        &intervention.reason,
                        "warning",
                    );
                    intervention.reason.clone()
                });
            if let Some(reason) = loop_guard_block_reason.as_ref() {
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "loop_guard_intervention".to_string(),
                        content: reason.clone(),
                        tone: Some("warning".to_string()),
                    })
                    .await;
            }
            let tool_dispatch_block =
                loop_guard_block_reason.map(tool_dispatch::ToolDispatchBlock::LoopGuard);

            // -- 4e. Execute tool calls in parallel ------------------------------
            turn_state.transition_to(TurnPhase::ToolDispatch);
            let dispatch_outcome = match self
                .dispatch_tool_calls(
                    tool_dispatch::ToolDispatchContext {
                        db,
                        tx: &tx,
                        conversation_id,
                        turn_id,
                        source_scope: &execution_source_scope,
                        model,
                        privacy_cfg: &privacy_cfg,
                        route_kind: route_plan.kind,
                        iteration,
                        tool_defs: &mut tool_defs,
                        messages: &mut messages,
                        persisted_trace_items: &mut persisted_trace_items,
                        task_plan: &mut task_plan,
                        loop_recorder: &mut loop_recorder,
                        loop_guard: &mut loop_guard,
                        trace: &mut trace,
                        sort_order: &mut sort_order,
                        pending_action_reconciliation,
                    },
                    &verified_tool_calls,
                    tool_dispatch_block,
                    &mut started_call_ids,
                    &mut tool_run_started_ids,
                )
                .await
            {
                Ok(summaries) => summaries,
                Err(error) => {
                    emit_tool_dispatch_failure(
                        &tx,
                        db,
                        &mut trace,
                        turn_id,
                        route_plan.kind,
                        &mut persisted_trace_items,
                        &error,
                    )
                    .await;
                    turn_state.finish(TurnOutcome::Failed);
                    return Err(error);
                }
            };
            let dispatch_summaries = dispatch_outcome.summaries;
            turn_budget.record_verified_tool_round();
            if let Some(reason) = dispatch_outcome.terminal_loop_guard_reason {
                let trace_message = format!("agent_loop_stopped_after_tool_errors: {reason}");
                emit_error_and_finalize_turn(
                    &tx,
                    db,
                    &mut trace,
                    turn_id,
                    route_plan.kind,
                    &persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: "Nexa stopped this turn after repeated tool failures continued beyond one recovery prompt. The completed error results were kept, and no additional model request was sent.".to_string(),
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                turn_state.finish(TurnOutcome::Failed);
                return Err(CoreError::Agent(trace_message));
            }
            model_action_observed |= successful_executable_action(&tool_calls, &dispatch_summaries);
            if pending_action_reconciliation
                && successful_action_reconciliation_observation(&tool_calls, &dispatch_summaries)
            {
                pending_action_reconciliation = false;
                let status = "Fresh interactive state was observed after the stop checkpoint. The reconciliation fence is cleared; re-plan from this observation and request one-shot approval before any new input.";
                append_developer_persisted_trace_status(
                    &mut persisted_trace_items,
                    status,
                    "warning",
                );
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "action_reconciliation_observed".to_string(),
                        content: status.to_string(),
                        tone: Some("warning".to_string()),
                    })
                    .await;
                if let Some(message) = prompt_ir::controller_state_message(status) {
                    messages.push(message);
                }
            }
            for call in &tool_calls {
                if let Some(summary) = dispatch_summaries
                    .iter()
                    .find(|summary| summary.call_id == call.id)
                {
                    workflow_ir.observe_tool_result(
                        &call.id,
                        &call.name,
                        summary.is_error,
                        summary.artifacts.as_ref(),
                        &summary.content,
                    );
                }
            }
            let awaiting_interaction_id = awaiting_user_input_interaction_id(&dispatch_summaries);
            workflow_ir.sync_from_task_plan(&task_plan);
            if let Some(ref mut agent_trace) = trace {
                agent_trace.workflow_ir = serde_json::to_value(&workflow_ir).ok();
            }
            if let Some(tid) = turn_id {
                if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                    let checkpoint = workflow_ir.task_plan_checkpoint(&task_plan);
                    let (status, phase, summary) = if awaiting_interaction_id.is_some() {
                        (
                            "awaiting_user_input",
                            "awaiting_user_input",
                            "Workflow checkpoint saved while waiting for user input",
                        )
                    } else {
                        (
                            "running",
                            "tooling",
                            "Workflow IR checkpoint updated after tool dispatch",
                        )
                    };
                    let _ = db.update_agent_task_run_progress(
                        &task_run.id,
                        Some(status),
                        Some(phase),
                        Some(route_plan.kind.as_str()),
                        Some(summary),
                        Some(&checkpoint),
                        None,
                    );
                }
            }
            if let Some(interaction_id) = awaiting_interaction_id {
                let live_state = long_task_state.checkpoint_live_state(
                    &task_plan,
                    Some(&workflow_ir),
                    turn_budget.tool_rounds_used(),
                    self.config.max_iterations,
                    &loop_recorder,
                );
                if let Err(error) = create_task_checkpoint_for_turn_with_state(
                    db,
                    turn_id,
                    &format!("awaiting_user_input:{interaction_id}"),
                    Some(&live_state),
                ) {
                    warn!("Failed to save awaiting-user-input checkpoint: {error}");
                }
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "awaiting_user_input".to_string(),
                        content: "Waiting for your response".to_string(),
                        tone: Some("attention".to_string()),
                    })
                    .await;
                turn_state.finish(TurnOutcome::AwaitingUserInput);
                return Err(CoreError::AwaitingUserInput { interaction_id });
            }
            last_tool_calls = None;

            // ── Cancellation checkpoint: after tool execution ─────────
            check_cancelled!(
                last_tool_calls,
                long_task_state,
                task_plan,
                turn_budget.tool_rounds_used()
            );

            // Re-trim messages to fit context window after appending tool results.
            // This prevents unbounded growth across iterations.
            let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
            messages = context_pipeline.trim_after_tool_results(&messages);
            prompt_was_compacted |=
                before_trim != prompt_cache::message_sequence_fingerprint(&messages);

            let completed_tool_round_index = turn_budget.tool_rounds_used().saturating_sub(1);
            if long_task_state.should_checkpoint_after_tool_round(completed_tool_round_index) {
                let reason = format!("auto_tool_round_{}", turn_budget.tool_rounds_used());
                let live_state = long_task_state.checkpoint_live_state(
                    &task_plan,
                    Some(&workflow_ir),
                    completed_tool_round_index,
                    self.config.max_iterations,
                    &loop_recorder,
                );
                match create_task_checkpoint_for_turn_with_state(
                    db,
                    turn_id,
                    &reason,
                    Some(&live_state),
                ) {
                    Ok(Some(_checkpoint_id)) => {
                        long_task_state.record_checkpoint(completed_tool_round_index);
                        let summary = format!(
                            "Resume checkpoint saved after tool round {}.",
                            turn_budget.tool_rounds_used()
                        );
                        append_developer_persisted_trace_status(
                            &mut persisted_trace_items,
                            &summary,
                            "info",
                        );
                        let _ = tx
                            .send(AgentEvent::ControllerStatus {
                                code: "resume_checkpoint_saved".to_string(),
                                content: summary,
                                tone: Some("muted".to_string()),
                            })
                            .await;
                    }
                    Ok(None) => {}
                    Err(err) => warn!("Failed to create auto resume checkpoint: {err}"),
                }
            }

            // Loop back → next LLM call with tool results.
        }

        // Graceful fallback: return partial answer instead of hard error.
        warn!(
            "Agent reached the configured tool-round limit ({:?}) after {} verified round(s); returning the reserved partial answer",
            turn_budget.configured_tool_round_limit(),
            turn_budget.tool_rounds_used(),
        );
        turn_state.transition_to(TurnPhase::Finalizing);
        let final_iteration = turn_budget.tool_rounds_used().saturating_sub(1);
        let live_state = long_task_state.checkpoint_live_state(
            &task_plan,
            Some(&workflow_ir),
            final_iteration,
            self.config.max_iterations,
            &loop_recorder,
        );
        match create_task_checkpoint_for_turn_with_state(
            db,
            turn_id,
            "max_tool_rounds",
            Some(&live_state),
        ) {
            Ok(Some(_checkpoint_id)) => {
                append_persisted_trace_status(
                    &mut persisted_trace_items,
                    "Saved a resume checkpoint after reaching the configured tool-round limit.",
                    "warning",
                );
            }
            Ok(None) => {}
            Err(err) => warn!("Failed to create max-iterations resume checkpoint: {err}"),
        }

        let final_content = if !last_iteration_content.trim().is_empty() {
            last_iteration_content
        } else {
            accumulated_content
        };
        let final_msg = self
            .finish_max_iterations(
                finalization::TurnFinalizationContext {
                    db,
                    tx: &tx,
                    conversation_id,
                    turn_id,
                    model,
                    route_kind: route_plan.kind,
                    persisted_trace_items: &mut persisted_trace_items,
                    task_plan: &mut task_plan,
                    loop_recorder: &mut loop_recorder,
                    trace: &mut trace,
                    sort_order,
                },
                final_content,
                total_usage,
                last_prompt_tokens,
                last_context_breakdown,
                last_finish_reason,
            )
            .await;
        turn_state.finish(TurnOutcome::MaxIterations);
        Ok(final_msg)
    }
}

#[cfg(test)]
mod awaiting_user_input_tests {
    use super::*;

    #[test]
    fn only_pending_v2_question_artifacts_suspend_the_loop() {
        let pending = tool_dispatch::ToolDispatchSummary {
            call_id: "call-1".into(),
            content: "wait".into(),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "questionRequest",
                "version": 2,
                "status": "pending",
                "interactionId": "interaction-1",
            })),
        };
        assert_eq!(
            awaiting_user_input_interaction_id(&[pending]),
            Some("interaction-1".into())
        );

        let answered = tool_dispatch::ToolDispatchSummary {
            call_id: "call-2".into(),
            content: "already answered".into(),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "questionRequest",
                "version": 2,
                "status": "answered",
                "interactionId": "interaction-2",
            })),
        };
        assert_eq!(awaiting_user_input_interaction_id(&[answered]), None);
    }

    #[test]
    fn cumulative_worker_budget_shrinks_each_physical_model_step() {
        assert_eq!(
            cumulative_run_step_output_budget(32_000, Some(40_000), 0, 10_000),
            Some(30_000)
        );
        assert_eq!(
            cumulative_run_step_output_budget(32_000, Some(40_000), 20_000, 5_000),
            Some(15_000)
        );
        assert_eq!(
            cumulative_run_step_output_budget(32_000, Some(40_000), 40_000, 0),
            None
        );
        assert_eq!(
            cumulative_run_step_output_budget(8_192, None, 999_999, 999_999),
            Some(8_192)
        );
    }
}

impl AgentExecutor {
    fn persist_unpersisted_replayable_system_messages(
        &self,
        ctx: ReplayableSystemPersistenceContext<'_>,
    ) {
        let ReplayableSystemPersistenceContext {
            db,
            conversation_id,
            model,
            layout,
            messages,
            sort_order,
            persisted_contents,
        } = ctx;

        if !layout.append_volatile_system_prompt_to_tail {
            return;
        }
        let Some(conversation_id) = conversation_id else {
            return;
        };
        let Some(current_user_index) = messages
            .iter()
            .rposition(|message| message.role == Role::User)
        else {
            return;
        };

        let mut seen_in_request: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for message in messages.iter().skip(current_user_index + 1) {
            if message.role != Role::System {
                continue;
            }
            let content = message.text_content();
            if content.trim().is_empty() {
                continue;
            }
            let seen_count = seen_in_request.entry(content.clone()).or_insert(0);
            let persisted_count = persisted_contents
                .iter()
                .filter(|persisted| *persisted == &content)
                .count();
            if *seen_count < persisted_count {
                *seen_count += 1;
                continue;
            }

            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: conversation_id.to_string(),
                role: Role::System,
                content: content.clone(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: Some(serde_json::json!({
                    "kind": "replayableRuntimeContext",
                    "version": 1,
                    "cachePurpose": "preserve exact-prefix provider prompt continuity across turns",
                })),
                token_count: estimate_message_tokens_for_model(model, message),
                created_at: String::new(),
                sort_order: *sort_order,
                thinking: None,
                image_attachments: None,
            };
            if let Err(err) = db.add_message(&conv_msg) {
                warn!("Failed to persist replayable runtime context: {err}");
                *seen_count += 1;
                continue;
            }
            persisted_contents.push(content);
            *sort_order += 1;
            *seen_count += 1;
        }
    }
}
