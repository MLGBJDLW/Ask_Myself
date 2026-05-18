//! Turn-level state machine and ReAct loop implementation.

use super::assistant_turn;
use super::finalization;
use super::model_step;
use super::steering::SteeringDrainContext;
use super::tool_dispatch;
use super::turn_state::{TurnOutcome, TurnPhase, TurnStateMachine};
use super::usage_accounting;
use super::*;

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
                    cached: false,
                    finish_reason: Some("stop".to_string()),
                })
                .await;
            return Ok(msg);
        }

        // --- 0b. Pre-summarize evicted history if context is getting full -----
        turn_state.transition_to(TurnPhase::PreparingContext);
        let history = self
            .summarize_if_needed(history, model, max_response_tokens)
            .await;

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
            crate::skills::get_active_skills_for_query(db, &user_query_text_for_tools, 5)
                .unwrap_or_default()
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
        let task_plan_value = serde_json::to_value(&task_plan)
            .unwrap_or_else(|_| serde_json::json!({ "error": "serializeTaskPlan" }));
        let _ = tx
            .send(AgentEvent::Status {
                content: format!("Route selected: {:?}", route_plan.kind),
                tone: Some("muted".to_string()),
            })
            .await;
        emit_task_plan_update(&tx, &task_plan, "planning", "Typed task plan created").await;
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

        let mut tool_defs = if self.config.dynamic_tool_visibility {
            let selected = self
                .tools
                .select_tools(&user_query_text_for_tools, has_sources);
            if route_plan.extra_categories.is_empty() {
                selected
            } else {
                let extra_categories: std::collections::HashSet<ToolCategory> =
                    route_plan.extra_categories.iter().copied().collect();
                let extra = self.tools.definitions_for_categories(&extra_categories);
                merge_tool_definitions(selected, extra)
            }
        } else {
            self.tools.definitions()
        };
        if let Some(ref mut t) = trace {
            t.tools_offered = tool_defs.len() as u32;
            t.route_kind = Some(route_plan.kind.as_str().to_string());
            t.task_plan = Some(task_plan_value.clone());
        }
        let mut messages = context::prepare_messages(
            &self.config.system_prompt,
            &history,
            &user_parts,
            model,
            max_response_tokens,
            self.config.context_window,
            &skills,
            &tool_defs,
        );
        if !route_plan.prompt_section.trim().is_empty() {
            let insert_at = messages.len().min(1);
            messages.insert(
                insert_at,
                Message::text(Role::System, route_plan.prompt_section.clone()),
            );
        }
        let plan_insert_at = messages.len().min(2);
        messages.insert(
            plan_insert_at,
            Message::text(Role::System, task_plan.to_prompt_section()),
        );

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
        let mut sort_order = next_sort_order;
        let mut accumulated_content = String::new();
        let mut last_iteration_content = String::new();
        let mut last_finish_reason: Option<String> = None;
        let mut persisted_trace_items: Vec<PersistedTraceItem> = Vec::new();
        for event in loop_recorder.events().iter().cloned() {
            append_persisted_trace_loop_event(&mut persisted_trace_items, event);
        }

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

        // --- 3d. Check answer cache before ReAct loop ------------------------
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

        // Macro for cancellation checkpoints — saves partial conversation and
        // returns gracefully when the token is cancelled.
        macro_rules! check_cancelled {
            ($last_tool_calls:expr) => {
                if self.cancel_token.is_cancelled() {
                    warn!("Agent execution cancelled by user");
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
        let mut force_non_streaming_llm = llm_streaming_disabled_by_env();
        'react_loop: for iteration in 0..self.config.max_iterations {
            turn_state.start_iteration(iteration);
            let step_started = TurnLoopEvent::StepStarted {
                iteration,
                remaining_iterations: self.config.max_iterations.saturating_sub(iteration),
            };
            loop_recorder.record(step_started.clone());
            append_persisted_trace_loop_event(&mut persisted_trace_items, step_started);
            // ── Cancellation checkpoint: before LLM call ─────────────────
            check_cancelled!(last_tool_calls);
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
                append_persisted_trace_status(
                    &mut persisted_trace_items,
                    "Applied user steering before the next model step.",
                    "info",
                );
                let max_ctx = self
                    .config
                    .context_window
                    .unwrap_or_else(|| model_context_window(model));
                messages = trim_to_context_window(
                    &messages,
                    max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                    max_response_tokens,
                );
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
                if !budget_hint.is_empty() {
                    messages.push(Message::text(Role::System, budget_hint));
                }
            }

            let model_step = self
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
                .await?;
            let model_step::ModelStepOutput {
                mut full_content,
                tool_calls,
                chunk_usage,
                iteration_thinking,
                last_finish_reason: step_finish_reason,
                mut started_call_ids,
                mut tool_run_started_ids,
            } = match model_step {
                model_step::ModelStepOutcome::Completed(output) => output,
                model_step::ModelStepOutcome::Restart => continue 'react_loop,
            };
            last_finish_reason = step_finish_reason;
            // -- 4b. Accumulate usage ------------------------------------------
            self.record_model_step_usage(
                usage_accounting::UsageAccountingContext {
                    tx: &tx,
                    model,
                    messages: &mut messages,
                    context_pipeline,
                    turn_state: &mut turn_state,
                    loop_recorder: &mut loop_recorder,
                    persisted_trace_items: &mut persisted_trace_items,
                    trace: &mut trace,
                    total_usage: &mut total_usage,
                    last_prompt_tokens: &mut last_prompt_tokens,
                },
                iteration,
                tool_calls.len(),
                last_finish_reason.clone(),
                chunk_usage,
            )
            .await;

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
                        messages.push(Message::text(Role::System, intervention.prompt.clone()));
                        continue;
                    }
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
                    self.drain_steering_messages(&mut messages, &mut steering_ctx)
                        .await
                };
                if !steering_texts.is_empty() {
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
                    self.expand_tool_defs_for_steering(
                        &mut tool_defs,
                        &steering_texts,
                        has_sources,
                    );
                    let max_ctx = self
                        .config
                        .context_window
                        .unwrap_or_else(|| model_context_window(model));
                    messages = trim_to_context_window(
                        &messages,
                        max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                        max_response_tokens,
                    );
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
            check_cancelled!(last_tool_calls);

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
            self.dispatch_tool_calls(
                tool_dispatch::ToolDispatchContext {
                    db,
                    tx: &tx,
                    conversation_id,
                    turn_id,
                    source_scope: &source_scope,
                    model,
                    privacy_cfg: &privacy_cfg,
                    route_kind: route_plan.kind,
                    iteration,
                    tool_defs: &tool_defs,
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
            last_tool_calls = None;

            // ── Cancellation checkpoint: after tool execution ─────────
            check_cancelled!(last_tool_calls);

            // Re-trim messages to fit context window after appending tool results.
            // This prevents unbounded growth across iterations.
            messages = context_pipeline.trim_after_tool_results(&messages);

            // Loop back → next LLM call with tool results.
        }

        // Graceful fallback: return partial answer instead of hard error.
        warn!(
            "Agent reached max iterations ({}); returning partial answer",
            self.config.max_iterations
        );
        turn_state.transition_to(TurnPhase::Finalizing);

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
                last_finish_reason,
            )
            .await;
        turn_state.finish(TurnOutcome::MaxIterations);
        Ok(final_msg)
    }
}
