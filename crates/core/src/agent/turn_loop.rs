//! Turn-level state machine and ReAct loop implementation.

use super::assistant_turn;
use super::finalization;
use super::model_step;
use super::steering::SteeringDrainContext;
use super::tool_dispatch;
use super::turn_state::{TurnOutcome, TurnPhase, TurnStateMachine};
use super::usage_accounting;
use super::*;

struct ReplayableSystemPersistenceContext<'a> {
    db: &'a Database,
    conversation_id: Option<&'a str>,
    model: &'a str,
    layout: prompt_layout::PromptLayout,
    messages: &'a [Message],
    sort_order: &'a mut i64,
    persisted_contents: &'a mut Vec<String>,
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
        let max_response_tokens = self.config.max_tokens.unwrap_or(4096);
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
        let history = self
            .summarize_if_needed(
                history,
                model,
                max_response_tokens,
                db,
                conversation_id,
                turn_id,
            )
            .await;
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

        let skills = self.skills_override.clone().unwrap_or_else(|| {
            crate::skills::get_available_skills_for_query(db, &user_query_text_for_tools)
                .unwrap_or_default()
        });
        let auto_loaded_skills = self.auto_loaded_skills_override.clone().unwrap_or_else(|| {
            crate::skills::select_skills_from_pool(skills.clone(), &user_query_text_for_tools, 3)
        });

        // --- Trace: initialize ------------------------------------------------
        let ctx_window_for_trace =
            self.config
                .context_window
                .unwrap_or_else(|| model_context_window(model)) as usize;
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
            && workflow_ir.requires_runtime_write_isolation()
        {
            Some(WorkspaceIsolationRuntime::prepare(db, &source_scope)?)
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

        let layout =
            prompt_layout::PromptLayout::for_request(self.config.provider_type, Some(model));
        let cache_stable_tool_surface = if layout.allow_dynamic_tool_visibility {
            None
        } else {
            Some(prompt_layout::select_cache_stable_tool_surface(
                &self.tools,
                model,
                self.config.context_window,
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
            self.tools
                .select_tools_for_decision(&route_plan.visibility_decision)
        } else {
            self.tools.definitions()
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
            &task_plan,
            effective_dynamic_tool_visibility && self.tools.contains("tool_search"),
            layout,
        );
        controller_state_sections_owned.push(orchestration_policy.prompt_section());
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

        let mut total_usage = Usage::default();
        let mut last_prompt_tokens: u32 = 0;
        let mut last_context_breakdown: Option<context::ContextUsageBreakdown> = None;
        let mut sort_order = next_sort_order;
        let mut accumulated_content = String::new();
        let mut last_iteration_content = String::new();
        let mut last_finish_reason: Option<String> = None;
        let mut persisted_trace_items: Vec<PersistedTraceItem> = Vec::new();
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
        self.prefetch_knowledge_results(
            route_plan.kind,
            user_query_text,
            db,
            &source_scope,
            &tx,
            conversation_id,
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
        let context_pipeline =
            ContextPipeline::new(model, self.config.context_window, max_response_tokens);
        let mut loop_guard = AgentLoopGuard::new();
        let mut long_task_state = LongTaskState::new();
        let mut force_non_streaming_llm = llm_streaming_disabled_by_env();
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
                let synthetic_assistant = Message {
                    role: Role::Assistant,
                    parts: vec![ContentPart::Text {
                        text: "Nexus is starting the independent reconnaissance wave compiled by Workflow IR."
                            .to_string(),
                    }],
                    name: None,
                    tool_calls: Some(vec![call.clone()]),
                    reasoning_content: None,
                };
                messages.push(synthetic_assistant.clone());
                self.persist_intermediate_tool_call_assistant(
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
                    std::slice::from_ref(&call),
                    None,
                    "Nexus runtime scheduled the first Workflow IR reconnaissance wave.",
                );
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
                let summaries = self
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
                        },
                        std::slice::from_ref(&call),
                        None,
                        &mut started_call_ids,
                        &mut tool_run_started_ids,
                    )
                    .await;
                let summary = summaries.iter().find(|summary| summary.call_id == call.id);
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
        'react_loop: for iteration in 0..self.config.max_iterations {
            turn_state.start_iteration(iteration);
            let step_started = TurnLoopEvent::StepStarted {
                iteration,
                remaining_iterations: self.config.max_iterations.saturating_sub(iteration),
            };
            loop_recorder.record(step_started.clone());
            append_persisted_trace_loop_event(&mut persisted_trace_items, step_started);
            // ── Cancellation checkpoint: before LLM call ─────────────────
            check_cancelled!(last_tool_calls, long_task_state, task_plan, iteration);
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
                let max_ctx = self
                    .config
                    .context_window
                    .unwrap_or_else(|| model_context_window(model));
                let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
                messages = trim_to_context_window(
                    &messages,
                    max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                    max_response_tokens,
                );
                prompt_was_compacted |=
                    before_trim != prompt_cache::message_sequence_fingerprint(&messages);
            }
            debug!(
                "Agent iteration {}/{}",
                iteration + 1,
                self.config.max_iterations
            );

            // Inject iteration-budget hint to help the model plan tool usage.
            let remaining = self.config.max_iterations - iteration;
            if iteration > 0 {
                let budget_hint = if remaining <= 1 {
                    "[System: This is your FINAL tool-use round. You MUST provide your complete answer now. Do not make additional tool calls — synthesize all evidence gathered so far.]".to_string()
                } else if iteration >= self.config.max_iterations / 2 {
                    format!(
                        "[System: You have {} tool-use round(s) remaining. Start synthesizing if you have sufficient evidence, or make your most critical remaining searches.]",
                        remaining
                    )
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
                iteration,
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

            let model_step_result = self
                .run_model_step(model_step::ModelStepContext {
                    db,
                    tx: &tx,
                    conversation_id,
                    turn_id,
                    route_kind: route_plan.kind,
                    model,
                    max_response_tokens,
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
                chunk_usage,
                iteration_thinking,
                last_finish_reason: step_finish_reason,
                mut started_call_ids,
                mut tool_run_started_ids,
                prompt_cache_observation,
            } = match model_step {
                model_step::ModelStepOutcome::Completed(output) => *output,
                model_step::ModelStepOutcome::Restart {
                    prompt_was_compacted: restart_was_compacted,
                } => {
                    prompt_was_compacted |= restart_was_compacted;
                    continue 'react_loop;
                }
            };
            if let Some(isolation) = workspace_isolation.as_mut() {
                isolation.rewrite_tool_calls(&mut tool_calls)?;
            }
            last_finish_reason = step_finish_reason;
            let request_was_compacted = prompt_was_compacted;
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
                    iteration,
                    tool_calls.len(),
                    last_finish_reason.clone(),
                    chunk_usage,
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

            if !full_content.trim().is_empty() {
                last_iteration_content = full_content.clone();
            } else if !iteration_thinking.is_empty() && tool_calls.is_empty() {
                // All content went to thinking (e.g. entire response wrapped in
                // <think> tags). Use thinking as the visible content so the DB
                // message is not empty.
                full_content = iteration_thinking.clone();
                last_iteration_content = full_content.clone();
            }

            // -- 4c. Build assistant message -----------------------------------
            let assistant_reasoning_content =
                self.reasoning_content_for_iteration(&iteration_thinking, !tool_calls.is_empty());
            let assistant_msg = Message {
                role: Role::Assistant,
                parts: vec![ContentPart::Text { text: full_content }],
                name: None,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.clone())
                },
                reasoning_content: assistant_reasoning_content.clone(),
            };
            messages.push(assistant_msg.clone());
            let loop_guard_intervention =
                loop_guard.observe_model_step(&assistant_msg.text_content(), &tool_calls);

            // -- 4d. Check termination -----------------------------------------
            if tool_calls.is_empty() {
                if let Some(intervention) = loop_guard_intervention.as_ref() {
                    if intervention.action == LoopGuardAction::ChangeStrategy
                        && iteration + 1 < self.config.max_iterations
                    {
                        let event = TurnLoopEvent::LoopGuardIntervention {
                            reason: intervention.reason.clone(),
                            action: intervention.action.as_str().to_string(),
                        };
                        loop_recorder.record(event.clone());
                        append_persisted_trace_loop_event(&mut persisted_trace_items, event);
                        append_persisted_trace_status(
                            &mut persisted_trace_items,
                            &intervention.reason,
                            "warning",
                        );
                        let _ = tx
                            .send(AgentEvent::Status {
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
                    let max_ctx = self
                        .config
                        .context_window
                        .unwrap_or_else(|| model_context_window(model));
                    let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
                    messages = trim_to_context_window(
                        &messages,
                        max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                        max_response_tokens,
                    );
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
                    if workflow_ir.requires_runtime_write_isolation()
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
                        append_persisted_trace_status(
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
                            || iteration + 1 >= self.config.max_iterations
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
                    || self.config.request_kind != AgentRequestKind::MainAgentStep
                    || iteration + 1 >= self.config.max_iterations
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
                    append_persisted_trace_status(&mut persisted_trace_items, &status, "info");
                    let _ = tx
                        .send(AgentEvent::Status {
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
                let assistant_msg = self
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
                        user_query_text,
                        cache_source_filter.as_deref(),
                        total_usage,
                        last_prompt_tokens,
                        last_context_breakdown,
                        last_finish_reason,
                    )
                    .await;
                turn_state.finish(TurnOutcome::Success);
                return Ok(assistant_msg);
            }

            // -- 4d'. Save intermediate assistant message (with tool_calls) ----
            self.persist_intermediate_tool_call_assistant(
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
            );

            last_tool_calls = Some(tool_calls.clone());

            // ── Cancellation checkpoint: before tool execution ────────
            check_cancelled!(last_tool_calls, long_task_state, task_plan, iteration);

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
                    append_persisted_trace_status(
                        &mut persisted_trace_items,
                        &intervention.reason,
                        "warning",
                    );
                    intervention.reason.clone()
                });
            if let Some(reason) = loop_guard_block_reason.as_ref() {
                let _ = tx
                    .send(AgentEvent::Status {
                        content: reason.clone(),
                        tone: Some("warning".to_string()),
                    })
                    .await;
            }

            // -- 4e. Execute tool calls in parallel ------------------------------
            turn_state.transition_to(TurnPhase::ToolDispatch);
            let dispatch_summaries = self
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
                    },
                    &tool_calls,
                    loop_guard_block_reason,
                    &mut started_call_ids,
                    &mut tool_run_started_ids,
                )
                .await;
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
            workflow_ir.sync_from_task_plan(&task_plan);
            if let Some(ref mut agent_trace) = trace {
                agent_trace.workflow_ir = serde_json::to_value(&workflow_ir).ok();
            }
            if let Some(tid) = turn_id {
                if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                    let checkpoint = workflow_ir.task_plan_checkpoint(&task_plan);
                    let _ = db.update_agent_task_run_progress(
                        &task_run.id,
                        Some("running"),
                        Some("tooling"),
                        Some(route_plan.kind.as_str()),
                        Some("Workflow IR checkpoint updated after tool dispatch"),
                        Some(&checkpoint),
                        None,
                    );
                }
            }
            last_tool_calls = None;

            // ── Cancellation checkpoint: after tool execution ─────────
            check_cancelled!(last_tool_calls, long_task_state, task_plan, iteration);

            // Re-trim messages to fit context window after appending tool results.
            // This prevents unbounded growth across iterations.
            let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
            messages = context_pipeline.trim_after_tool_results(&messages);
            prompt_was_compacted |=
                before_trim != prompt_cache::message_sequence_fingerprint(&messages);

            if long_task_state.should_checkpoint_after_tool_round(iteration) {
                let reason = format!("auto_tool_round_{}", iteration.saturating_add(1));
                let live_state = long_task_state.checkpoint_live_state(
                    &task_plan,
                    Some(&workflow_ir),
                    iteration,
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
                        long_task_state.record_checkpoint(iteration);
                        let summary = format!(
                            "Resume checkpoint saved after tool round {}.",
                            iteration.saturating_add(1)
                        );
                        append_persisted_trace_status(&mut persisted_trace_items, &summary, "info");
                        let _ = tx
                            .send(AgentEvent::Status {
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
            "Agent reached max iterations ({}); returning partial answer",
            self.config.max_iterations
        );
        turn_state.transition_to(TurnPhase::Finalizing);
        let final_iteration = self.config.max_iterations.saturating_sub(1);
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
            "max_iterations",
            Some(&live_state),
        ) {
            Ok(Some(_checkpoint_id)) => {
                append_persisted_trace_status(
                    &mut persisted_trace_items,
                    "Saved a resume checkpoint after reaching max iterations.",
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
