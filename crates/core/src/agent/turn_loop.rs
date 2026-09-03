//! Turn-level state machine and ReAct loop implementation.

use super::assistant_turn;
use super::final_answer_hygiene::FinalAnswerHygieneScope;
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

struct ProviderContextLimitRecoveryContext<'a> {
    db: &'a Database,
    tx: &'a mpsc::Sender<AgentEvent>,
    conversation_id: Option<&'a str>,
    turn_id: Option<&'a str>,
    route_kind: AgentRouteKind,
    model: &'a str,
    messages: &'a mut Vec<Message>,
    total_usage: &'a mut Usage,
    completed_attempts: &'a mut u32,
    trace: &'a mut Option<AgentTrace>,
    persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
}

struct RecoveryAssistantMessageContext<'a> {
    full_content: &'a str,
    iteration_thinking: &'a str,
    recovery_reasoning: Option<String>,
    sample_id: &'a str,
    route_snapshot: &'a crate::llm::provider_turn::RouteSnapshot,
    reasoning_was_requested: bool,
    provider_replay: Option<&'a crate::llm::provider_turn::ProviderReplayPayload>,
}

fn capture_recovery_assistant_message(ctx: RecoveryAssistantMessageContext<'_>) -> Message {
    let RecoveryAssistantMessageContext {
        full_content,
        iteration_thinking,
        recovery_reasoning,
        sample_id,
        route_snapshot,
        reasoning_was_requested,
        provider_replay,
    } = ctx;
    let mut message = Message {
        role: Role::Assistant,
        parts: vec![ContentPart::Text {
            text: full_content.to_string(),
        }],
        name: None,
        tool_calls: None,
        reasoning_content: recovery_reasoning.clone(),
        prompt_cache_hint: None,
    };
    message.set_provider_turn(
        crate::llm::provider_turn::ProviderTurnEnvelope::capture_with_replay_payload(
            Uuid::new_v4().to_string(),
            sample_id.to_string(),
            route_snapshot.clone(),
            message.text_content(),
            crate::llm::reasoning_replay::sanitize_reasoning_text(Some(iteration_thinking))
                .as_deref(),
            recovery_reasoning.as_deref(),
            Vec::new(),
            reasoning_was_requested,
            provider_replay.cloned(),
        ),
    );
    message
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActionReconciliationFence {
    computer: bool,
    browser: bool,
    unknown: bool,
}

impl ActionReconciliationFence {
    fn from_resume_prompt(prompt: &str) -> Self {
        const MARKER: &str = "Checkpoint reason: user_stop_requires_action_reconciliation:";
        let Some(receipts) = prompt.split_once(MARKER).map(|(_, receipts)| receipts) else {
            return Self::default();
        };
        let computer = receipts.contains("computer_control:");
        let browser = receipts.contains("browser_action:") || receipts.contains("browser_session:");
        Self {
            computer,
            browser,
            unknown: !computer && !browser,
        }
    }

    fn blocks_interactive_input(self) -> bool {
        self.computer || self.browser || self.unknown
    }

    fn observe_tool_results(
        &mut self,
        calls: &[ToolCallRequest],
        summaries: &[tool_dispatch::ToolDispatchSummary],
    ) {
        for call in calls {
            let Some(summary) = summaries.iter().find(|summary| summary.call_id == call.id) else {
                continue;
            };
            let code = summary
                .artifacts
                .as_ref()
                .and_then(|artifacts| artifacts.get("code"))
                .and_then(serde_json::Value::as_str);
            let effect = summary
                .artifacts
                .as_ref()
                .and_then(|artifacts| {
                    artifacts
                        .pointer("/data/effect")
                        .or_else(|| artifacts.pointer("/toolOutput/data/effect"))
                })
                .and_then(serde_json::Value::as_str);
            let action = serde_json::from_str::<serde_json::Value>(&call.arguments)
                .ok()
                .and_then(|arguments| {
                    arguments
                        .get("action")
                        .and_then(serde_json::Value::as_str)
                        .map(|action| action.trim().to_ascii_lowercase())
                });
            if call.name == "computer_control"
                && (crate::workflow_ir::tool_result_effect_may_have_occurred(
                    summary.artifacts.as_ref(),
                ) || matches!(
                    code,
                    Some("computer_action_uncertain" | "computer_action_timeout_uncertain")
                ) || effect == Some("unverifiable"))
            {
                self.computer = true;
            }
            if call.name == "browser_session"
                && matches!(
                    code,
                    Some("browser_action_uncertain" | "browser_action_timeout_uncertain")
                )
            {
                self.browser = true;
            }
            if !summary.is_error
                && call.name == "computer_observe"
                && matches!(
                    action.as_deref(),
                    Some("capture_window" | "wait_for_change")
                )
                && crate::workflow_ir::is_verified_desktop_observation(
                    Some(&call.arguments),
                    summary.artifacts.as_ref(),
                )
            {
                self.computer = false;
                self.unknown = false;
            }
            if !summary.is_error
                && call.name == "browser_session"
                && action.as_deref() == Some("observe")
                && crate::workflow_ir::is_verified_browser_visual_observation(
                    "browser_session",
                    summary.artifacts.as_ref(),
                )
            {
                self.browser = false;
                self.unknown = false;
            }
        }
    }
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

fn effective_tool_surface(tool_defs: &[ToolDefinition], suppress_tools: bool) -> &[ToolDefinition] {
    if suppress_tools {
        &[]
    } else {
        tool_defs
    }
}

async fn commit_buffered_answer_projection(
    tx: &mpsc::Sender<AgentEvent>,
    accumulated_content: &mut String,
    visible_delta: &str,
) {
    if visible_delta.is_empty() {
        return;
    }
    accumulated_content.push_str(visible_delta);
    let _ = tx
        .send(AgentEvent::TextDelta {
            delta: visible_delta.to_string(),
        })
        .await;
}

fn rollback_rejected_sample_projection(
    accumulated_content: &mut String,
    sample_content: &str,
    sample_was_projected: bool,
) {
    if sample_was_projected && accumulated_content.ends_with(sample_content) {
        accumulated_content.truncate(
            accumulated_content
                .len()
                .saturating_sub(sample_content.len()),
        );
    }
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
    async fn recover_provider_context_limit(
        &self,
        ctx: ProviderContextLimitRecoveryContext<'_>,
    ) -> Result<(), CoreError> {
        let ProviderContextLimitRecoveryContext {
            db,
            tx,
            conversation_id,
            turn_id,
            route_kind,
            model,
            messages,
            total_usage,
            completed_attempts,
            trace,
            persisted_trace_items,
        } = ctx;
        let terminal_error =
            CoreError::Llm("provider completed the sample at its context limit".to_string());
        let (attempt, status_message) = match StreamRecoveryPolicy::default()
            .decide_after_context_overflow(*completed_attempts, &terminal_error)
        {
            ContextOverflowRecoveryDecision::Compact {
                attempt,
                status_message,
            } => (attempt, status_message),
            ContextOverflowRecoveryDecision::GiveUp { user_message } => {
                let trace_message =
                    format!("provider_context_limit_recovery_exhausted: {terminal_error}");
                append_persisted_trace_status(persisted_trace_items, &user_message, "error");
                emit_error_and_finalize_turn(
                    tx,
                    db,
                    trace,
                    turn_id,
                    route_kind,
                    persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: user_message,
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                return Err(CoreError::Agent(trace_message));
            }
        };
        *completed_attempts = attempt;
        let _ = tx
            .send(AgentEvent::Status {
                content: status_message,
                tone: Some("muted".to_string()),
            })
            .await;
        let recovered = self
            .recover_context_overflow(
                messages,
                model,
                tx,
                context_compaction::CompactionRunContext {
                    db,
                    conversation_id,
                    turn_id,
                },
                total_usage,
            )
            .await?;
        if recovered {
            return Ok(());
        }

        let trace_message = "provider_context_limit_compaction_made_no_progress";
        let frontend_message = "The provider reached its context limit, but committed history could not be compacted any further. No draft tool call was executed.";
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
                trace_message: trace_message.to_string(),
            },
        )
        .await;
        Err(CoreError::Agent(trace_message.to_string()))
    }

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
        // Establish tool authority before schema selection or prompt
        // accounting. Answer-only turns must not pay context for a tool
        // surface that can never be transmitted.
        let mut turn_budget = TurnBudget::new(self.config.max_iterations);
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
        let mut action_reconciliation =
            ActionReconciliationFence::from_resume_prompt(&user_query_text_for_tools);

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
        let mut task_plan = build_task_plan(TaskPlanningInput::for_requirements(
            &user_query_text_for_tools,
            &route_plan.requirements,
            has_sources,
            source_scope.len(),
        ));
        let orchestration_policy = resolve_orchestration_profile(OrchestrationProfileInput {
            profile: self.config.orchestration_profile,
            custom: self.config.custom_orchestration.clone(),
            max_iterations: self.config.max_iterations,
            max_parallel: self.config.subagent_max_parallel,
            max_calls_per_turn: self.config.subagent_max_calls_per_turn,
            delegated_token_budget: self.config.subagent_token_budget,
            verification_reserve_percent: self.config.subagent_verification_reserve_percent,
        });
        let requires_workspace_isolation = self.config.request_kind.requires_workspace_isolation();
        let mut workflow_ir = crate::workflow_ir::compile_turn_workflow_ir(
            &task_plan,
            &orchestration_policy,
            self.config.power_mode.is_nexus(),
            requires_workspace_isolation,
        )
        .map_err(CoreError::InvalidInput)?;
        if self.config.execution_mode.is_plan() {
            if let Some(workflow) = workflow_ir.as_mut() {
                workflow.configure_for_plan_mode();
            }
        } else if workflow_ir.as_ref().is_some_and(|workflow| {
            workflow.completion_contract.require_verification_gates || requires_workspace_isolation
        }) {
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
            if let Some(workflow) = workflow_ir.as_mut() {
                workflow.configure_project_verification_support(
                    crate::workflow_ir::detect_project_verification_support(&verification_roots),
                );
            }
        }
        let mut workspace_isolation = if !self.config.execution_mode.is_plan()
            && (workflow_ir
                .as_ref()
                .is_some_and(|workflow| workflow.requires_runtime_write_isolation())
                || requires_workspace_isolation)
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
        let task_plan_value = workflow_ir
            .as_ref()
            .map(|workflow| workflow.task_plan_checkpoint(&task_plan))
            .unwrap_or_else(|| {
                serde_json::to_value(&task_plan)
                    .unwrap_or_else(|_| serde_json::json!({ "error": "serializeTaskPlan" }))
            });
        let workflow_ir_value = workflow_ir
            .as_ref()
            .and_then(|workflow| serde_json::to_value(workflow).ok());
        let _ = tx
            .send(AgentEvent::ControllerStatus {
                code: "route_selected".to_string(),
                content: format!("Route selected: {:?}", route_plan.kind),
                tone: Some("muted".to_string()),
            })
            .await;
        emit_task_plan_update(&tx, &task_plan, "planning", "Typed task plan created").await;
        if let Some(workflow) = workflow_ir.as_ref() {
            let _ = tx
                .send(AgentEvent::ControllerStatus {
                    code: "workflow_compiled".to_string(),
                    content: format!(
                        "Workflow IR v{} compiled: {} nodes, {} verification gates",
                        workflow.version,
                        workflow.nodes.len(),
                        workflow.verification_gates.len()
                    ),
                    tone: Some("muted".to_string()),
                })
                .await;
        }
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
        let tools_allowed_initially = turn_budget.can_dispatch_tool_round();
        let cache_stable_tool_surface =
            if !tools_allowed_initially || layout.allow_dynamic_tool_visibility {
                None
            } else {
                Some(prompt_layout::select_cache_stable_tool_surface(
                    model_tool_registry,
                    model,
                    effective_context_capacity,
                    max_response_tokens,
                )?)
            };
        let effective_dynamic_tool_visibility = tools_allowed_initially
            && cache_stable_tool_surface
                .as_ref()
                .map(prompt_layout::CacheStableToolSurface::uses_dynamic_discovery)
                .unwrap_or_else(|| {
                    layout.effective_dynamic_tool_visibility(self.config.dynamic_tool_visibility)
                });
        let mut tool_defs = if !tools_allowed_initially {
            Vec::new()
        } else if let Some(surface) = cache_stable_tool_surface {
            debug!(
                mode = ?surface.mode,
                tool_count = surface.definitions.len(),
                "Selected cache-stable tool surface"
            );
            surface.definitions
        } else if effective_dynamic_tool_visibility {
            model_tool_registry.select_tools_for_decision(&route_plan.requirements)
        } else {
            model_tool_registry.definitions()
        };
        if tools_allowed_initially && self.config.power_mode.is_nexus() {
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
            t.workflow_ir = workflow_ir_value;
            t.orchestration_profile = Some(self.config.orchestration_profile.as_str().to_string());
            t.collaboration_mode = Some(self.config.collaboration_mode.as_str().to_string());
            t.tool_visibility_decision = Some(route_plan.requirements.clone());
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
        let expose_workflow_ir =
            self.config.power_mode.is_nexus() || self.config.orchestration_profile.is_ultra();
        if let Some(workflow) = workflow_ir.as_ref().filter(|_| expose_workflow_ir) {
            controller_state_sections_owned.push(workflow.to_prompt_section());
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
        for event in loop_recorder.events().iter().cloned() {
            append_persisted_trace_loop_event(&mut persisted_trace_items, event);
        }
        append_persisted_trace_visibility(&mut persisted_trace_items, &route_plan.requirements);
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
        if !self.config.execution_mode.is_plan() && turn_budget.can_dispatch_tool_round() {
            turn_state.transition_to(TurnPhase::DirectDispatch);
            match self
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
                direct_dispatch_runner::DirectDispatchOutcome::Completed(message) => {
                    turn_state.finish(TurnOutcome::DirectDispatch);
                    return Ok(message);
                }
                direct_dispatch_runner::DirectDispatchOutcome::ExecutedButFailed => {
                    turn_budget.record_verified_tool_round();
                }
                direct_dispatch_runner::DirectDispatchOutcome::NotMatched => {}
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
                        workflow_ir.as_ref(),
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
        let prefetch_enters_dispatch = route_plan.kind == AgentRouteKind::KnowledgeRetrieval
            && !user_query_text.is_empty()
            && turn_budget.can_dispatch_tool_round();
        let prefetch_observed = if prefetch_enters_dispatch {
            let observed = self
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
                    &mut messages,
                    &mut persisted_trace_items,
                    &mut task_plan,
                )
                .await;
            // Graph-guided retrieval is one controller-owned logical batch.
            // Count the dispatch even if the provider returns no evidence or
            // the local search tool reports an error.
            turn_budget.record_verified_tool_round();
            observed
        } else {
            false
        };

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
            let workflow_ir = workflow_ir
                .as_mut()
                .expect("Nexus or Ultra reconnaissance requires compiled Workflow IR");
            while turn_budget.can_dispatch_tool_round() {
                let Some(arguments) =
                    workflow_ir.reconnaissance_batch_arguments(&task_plan.objective)
                else {
                    break;
                };
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
                            tool_round_index: turn_budget.tool_rounds_used(),
                            tool_defs: &mut tool_defs,
                            messages: &mut messages,
                            persisted_trace_items: &mut persisted_trace_items,
                            task_plan: &mut task_plan,
                            loop_recorder: &mut loop_recorder,
                            loop_guard: &mut loop_guard,
                            trace: &mut trace,
                            sort_order: &mut sort_order,
                            pending_action_reconciliation: action_reconciliation
                                .blocks_interactive_input(),
                        },
                        &verified_call,
                        None,
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
                // Controller-owned reconnaissance is a real dispatched tool
                // batch and consumes the same semantic round authority as a
                // model-directed batch.
                turn_budget.record_verified_tool_round();
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
            if !turn_budget.can_dispatch_tool_round()
                && workflow_ir
                    .reconnaissance_batch_arguments(&task_plan.objective)
                    .is_some()
            {
                let status = format!(
                    "Automatic reconnaissance stopped after {} verified tool round(s) because the configured tool-round budget is complete.",
                    turn_budget.tool_rounds_used()
                );
                append_internal_persisted_trace_status(&mut persisted_trace_items, &status, "info");
                let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "workflow_reconnaissance_budget_complete".to_string(),
                        content: status,
                        tone: Some("muted".to_string()),
                    })
                    .await;
            }
        }

        let mut workflow_gate_repair_rounds = 0u8;
        let mut output_recovery = OutputRecovery::default();
        let mut contaminated_final_retries = 0u8;
        let mut clean_final_retry_active = false;
        let mut next_step_purpose = TurnStepPurpose::Normal;
        let mut final_answer_hygiene_scope =
            FinalAnswerHygieneScope::from_user_text(user_query_text);
        'react_loop: loop {
            let step_purpose = next_step_purpose;
            let Some(step_permit) = turn_budget.permit(step_purpose) else {
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
                final_answer_hygiene_scope.observe_user_texts(&steering_texts);
                if step_permit.allows_tools() {
                    self.expand_tool_defs_for_steering(
                        &mut tool_defs,
                        &steering_texts,
                        has_sources,
                    );
                }
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
            if step_purpose != TurnStepPurpose::Recovery
                && (iteration > 0 || step_permit.mode == TurnStepMode::FinalAnswerOnly)
            {
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

            // Tool discovery and steering can expand the surface between model
            // steps. Re-apply the isolation boundary before any request
            // accounting so the estimated and transmitted surfaces stay
            // identical.
            if workspace_isolation.is_some() {
                WorkspaceIsolationRuntime::retain_safe_tool_definitions(&mut tool_defs);
            }
            let suppress_tools_for_step = clean_final_retry_active || !step_permit.allows_tools();
            let effective_tool_defs =
                effective_tool_surface(tool_defs.as_slice(), suppress_tools_for_step);
            prompt_was_compacted |= self
                .compact_before_model_step_if_needed(LongTaskCompactionContext {
                    db,
                    conversation_id,
                    turn_id,
                    tx: &tx,
                    model,
                    messages: &mut messages,
                    context_pipeline,
                    tool_defs: effective_tool_defs,
                    turn_state: &mut turn_state,
                    loop_recorder: &mut loop_recorder,
                    persisted_trace_items: &mut persisted_trace_items,
                    total_usage: &mut total_usage,
                })
                .await;
            let buffer_answer_projection = output_recovery.reserves_answer_channel();
            let force_answer_only = clean_final_retry_active
                || buffer_answer_projection
                || step_permit.mode == TurnStepMode::FinalAnswerOnly;
            let estimated_prompt = if self.config.max_actual_tokens_per_run.is_some() {
                context::estimate_context_usage_breakdown_for_model(
                    model,
                    &messages,
                    effective_tool_defs,
                    None,
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
            let wire_model_step_max_response_tokens =
                if self.config.max_actual_tokens_per_run.is_some() {
                    // A caller-authorized cumulative worker cap must bound each
                    // physical request as the remaining budget shrinks.
                    Some(model_step_max_response_tokens)
                } else {
                    output_budget_plan.wire_max_tokens()
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
                    wire_max_response_tokens: wire_model_step_max_response_tokens,
                    has_sources,
                    privacy_cfg: &privacy_cfg,
                    messages: &mut messages,
                    final_answer_hygiene_scope: &mut final_answer_hygiene_scope,
                    tool_defs: &mut tool_defs,
                    accumulated_content: &mut accumulated_content,
                    persisted_trace_items: &mut persisted_trace_items,
                    trace: &mut trace,
                    sort_order: &mut sort_order,
                    context_recovery_attempts: &mut context_recovery_attempts,
                    force_non_streaming_llm: &mut force_non_streaming_llm,
                    reasoning_disabled_for_tool_loop: &mut reasoning_disabled_for_tool_loop,
                    force_answer_only,
                    suppress_tools: suppress_tools_for_step,
                    buffer_answer_projection,
                    output_recovery: &output_recovery,
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
                        tool_defs: effective_tool_surface(
                            tool_defs.as_slice(),
                            suppress_tools_for_step,
                        ),
                        turn_state: &mut turn_state,
                        loop_recorder: &mut loop_recorder,
                        persisted_trace_items: &mut persisted_trace_items,
                        trace: &mut trace,
                        total_usage: &mut total_usage,
                        last_prompt_tokens: &mut last_prompt_tokens,
                        last_context_breakdown: &mut last_context_breakdown,
                    },
                    usage_accounting::ModelStepUsageObservation {
                        sample_index: iteration,
                        trace_iteration: step_permit.tool_rounds_used,
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

            let provider_state_fingerprint = provider_replay
                .as_ref()
                .filter(|replay| replay.resumes_provider_pause())
                .and_then(|replay| replay.state_fingerprint());
            let recovery_decision = output_recovery.observe_with_provider_state(
                step_finish_reason_kind.as_ref(),
                &full_content,
                !tool_calls.is_empty(),
                provider_state_fingerprint.as_deref(),
            );
            let resumes_provider_pause = matches!(
                &recovery_decision,
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::ProviderPause,
                    ..
                } | OutputRecoveryDecision::RejectToolRound {
                    cause: ToolRoundRejectionCause::ProviderPause,
                    ..
                }
            );
            if resumes_provider_pause
                && provider_replay
                    .as_ref()
                    .is_none_or(|replay| !replay.resumes_provider_pause())
            {
                let trace_message = "provider_pause_missing_replay_state: the provider paused a hosted-tool turn without replayable native assistant blocks".to_string();
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
                        frontend_message: "The provider paused its hosted-tool turn without enough native state to resume safely. Nexa stopped instead of restarting or duplicating the provider tool.".to_string(),
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                turn_state.finish(TurnOutcome::Failed);
                return Err(CoreError::Agent(trace_message));
            }
            let mut tool_round_rejection_cause = None;
            let mut tool_round_rejection_committed_progress = false;
            let mut staged_tool_round_visible_delta = None;
            let mut staged_tool_round_working_delta = None;
            let recovery_failure = match recovery_decision {
                OutputRecoveryDecision::Continue {
                    cause,
                    visible_delta,
                } => {
                    let committed_visible_delta = !visible_delta.is_empty();
                    let had_visible_content = output_recovery.has_visible_content();
                    if buffer_answer_projection {
                        commit_buffered_answer_projection(
                            &tx,
                            &mut accumulated_content,
                            &visible_delta,
                        )
                        .await;
                    }
                    append_persisted_trace_thinking(
                        &mut persisted_trace_items,
                        &iteration_thinking,
                    );
                    if committed_visible_delta || cause == OutputRecoveryCause::ProviderPause {
                        let recovery_reasoning =
                            self.reasoning_content_for_iteration(&iteration_thinking, false);
                        messages.push(capture_recovery_assistant_message(
                            RecoveryAssistantMessageContext {
                                full_content: &visible_delta,
                                iteration_thinking: &iteration_thinking,
                                recovery_reasoning,
                                sample_id: &sample_id,
                                route_snapshot: &route_snapshot,
                                reasoning_was_requested,
                                provider_replay: provider_replay.as_ref(),
                            },
                        ));
                    }

                    if cause == OutputRecoveryCause::ContextLimit {
                        if let Err(error) = self
                            .recover_provider_context_limit(ProviderContextLimitRecoveryContext {
                                db,
                                tx: &tx,
                                conversation_id,
                                turn_id,
                                route_kind: route_plan.kind,
                                model,
                                messages: &mut messages,
                                total_usage: &mut total_usage,
                                completed_attempts: &mut context_recovery_attempts,
                                trace: &mut trace,
                                persisted_trace_items: &mut persisted_trace_items,
                            })
                            .await
                        {
                            turn_state.finish(TurnOutcome::Failed);
                            return Err(error);
                        }
                        prompt_was_compacted = true;
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
                        output_recovery.controller_prompt(cause, had_visible_content),
                    ) {
                        messages.push(message);
                    }
                    next_step_purpose = TurnStepPurpose::Recovery;
                    continue 'react_loop;
                }
                OutputRecoveryDecision::RejectToolRound {
                    cause,
                    committed_progress,
                } => {
                    tool_round_rejection_cause = Some(cause);
                    tool_round_rejection_committed_progress = committed_progress;
                    if cause == ToolRoundRejectionCause::ProviderPause {
                        append_persisted_trace_thinking(
                            &mut persisted_trace_items,
                            &iteration_thinking,
                        );
                        let recovery_reasoning =
                            self.reasoning_content_for_iteration(&iteration_thinking, false);
                        messages.push(capture_recovery_assistant_message(
                            RecoveryAssistantMessageContext {
                                full_content: &full_content,
                                iteration_thinking: &iteration_thinking,
                                recovery_reasoning,
                                sample_id: &sample_id,
                                route_snapshot: &route_snapshot,
                                reasoning_was_requested,
                                provider_replay: provider_replay.as_ref(),
                            },
                        ));
                    }
                    append_internal_persisted_trace_status(
                        &mut persisted_trace_items,
                        "The provider terminal state did not commit a safe tool-call response. The draft calls will be rejected and re-planned without execution.",
                        "warning",
                    );
                    if cause == ToolRoundRejectionCause::ContextLimit {
                        if let Err(error) = self
                            .recover_provider_context_limit(ProviderContextLimitRecoveryContext {
                                db,
                                tx: &tx,
                                conversation_id,
                                turn_id,
                                route_kind: route_plan.kind,
                                model,
                                messages: &mut messages,
                                total_usage: &mut total_usage,
                                completed_attempts: &mut context_recovery_attempts,
                                trace: &mut trace,
                                persisted_trace_items: &mut persisted_trace_items,
                            })
                            .await
                        {
                            turn_state.finish(TurnOutcome::Failed);
                            return Err(error);
                        }
                        prompt_was_compacted = true;
                    }
                    None
                }
                OutputRecoveryDecision::ToolRound {
                    visible_delta,
                    working_delta,
                } => {
                    if buffer_answer_projection {
                        staged_tool_round_visible_delta = Some(visible_delta);
                        staged_tool_round_working_delta = Some(working_delta);
                    }
                    None
                }
                OutputRecoveryDecision::Final {
                    content,
                    visible_delta,
                } => {
                    if buffer_answer_projection {
                        commit_buffered_answer_projection(
                            &tx,
                            &mut accumulated_content,
                            &visible_delta,
                        )
                        .await;
                    }
                    full_content = content;
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
                    output_recovery.rollback_staged_tool_round();
                    // Provider streams may terminate after emitting only part of a
                    // function call. Never persist, replay, or execute that partial
                    // protocol envelope. Re-plan from a plain controller message so
                    // the next request is valid even when the partial assistant also
                    // contained visible text.
                    rollback_rejected_sample_projection(
                        &mut accumulated_content,
                        &full_content,
                        !buffer_answer_projection,
                    );
                    let rejection_reason = match tool_round_rejection_cause {
                        Some(ToolRoundRejectionCause::OutputLimit) => "tool_draft_output_limit",
                        Some(ToolRoundRejectionCause::ProviderPause) => "tool_draft_provider_pause",
                        Some(ToolRoundRejectionCause::ContextLimit) => "tool_draft_context_limit",
                        Some(ToolRoundRejectionCause::MalformedToolCall) => "tool_draft_malformed",
                        Some(ToolRoundRejectionCause::ToolsSuppressed) => "tool_draft_suppressed",
                        Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => {
                            "tool_draft_incomplete"
                        }
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
                    let protocol_intervention = if tool_round_rejection_committed_progress {
                        loop_guard.record_protocol_progress();
                        None
                    } else {
                        loop_guard
                            .observe_protocol_rejection(protocol_fault_code, &protocol_guard_calls)
                    };
                    if let Some(intervention) = protocol_intervention {
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
                            Some(ToolRoundRejectionCause::OutputLimit) => "The previous tool-call draft was truncated at the provider output boundary and was not executed. Retry with a smaller operation. For large file content, create a small first chunk and append bounded chunks.".to_string(),
                            Some(ToolRoundRejectionCause::ProviderPause) => "The provider paused before the client tool draft committed. Resume from committed provider state; do not continue the discarded draft.".to_string(),
                            Some(ToolRoundRejectionCause::ContextLimit) => "The context ended before the tool draft committed. After rollover, retry once using the exposed tool schema; do not continue the discarded draft.".to_string(),
                            Some(ToolRoundRejectionCause::MalformedToolCall) => "The previous tool-call draft was malformed and was not executed. Retry once using the exposed tool schema; change strategy if it fails again.".to_string(),
                            Some(ToolRoundRejectionCause::ToolsSuppressed) => "Client tools are unavailable during the reserved answer-only step. Produce the best complete visible answer from the committed evidence and tool results.".to_string(),
                            Some(ToolRoundRejectionCause::ProtocolIncomplete) | None => "The previous tool-call draft was incomplete and was not executed. Retry once from the user request using the exposed tool schema.".to_string(),
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
            output_recovery.commit_staged_tool_round();
            let tool_round_working_delta = staged_tool_round_working_delta
                .take()
                .filter(|delta| !delta.trim().is_empty());
            if let Some(working_delta) = tool_round_working_delta.as_ref() {
                // This text arrived in the provider answer field only because a
                // reasoning-only sample had already exhausted its output budget.
                // Re-emit it on the typed working lane before the tool round.
                let _ = tx
                    .send(AgentEvent::Thinking {
                        content: working_delta.clone(),
                    })
                    .await;
                full_content.clear();
            }
            if let Some(visible_delta) = staged_tool_round_visible_delta.take() {
                commit_buffered_answer_projection(&tx, &mut accumulated_content, &visible_delta)
                    .await;
                full_content = visible_delta;
            }
            let tool_calls = verified_tool_calls.as_slice().to_vec();

            last_iteration_content = full_content.clone();

            let display_iteration_thinking = match (
                iteration_thinking.trim().is_empty(),
                tool_round_working_delta.as_deref(),
            ) {
                (_, None) => iteration_thinking.clone(),
                (true, Some(working)) => working.to_string(),
                (false, Some(working)) => format!("{iteration_thinking}\n\n{working}"),
            };

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
                        &display_iteration_thinking,
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
                            reason: "agent_loop_stopped".to_string(),
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
                if let Some(marker) =
                    final_answer_hygiene_scope.contamination_marker(&assistant_msg.text_content())
                {
                    messages.pop();
                    accumulated_content.truncate(
                        accumulated_content
                            .len()
                            .saturating_sub(assistant_msg.text_content().len()),
                    );
                    last_iteration_content.clear();
                    let _ = tx
                        .send(AgentEvent::StreamReset {
                            reason: "contaminated_final_answer".to_string(),
                            discard_sample: true,
                        })
                        .await;
                    let trace_message = format!(
                        "discarded final-answer sample containing reserved internal marker: {marker}"
                    );
                    append_developer_persisted_trace_status(
                        &mut persisted_trace_items,
                        &trace_message,
                        "warning",
                    );
                    if contaminated_final_retries >= 1 {
                        emit_error_and_finalize_turn(
                            &tx,
                            db,
                            &mut trace,
                            turn_id,
                            route_plan.kind,
                            &persisted_trace_items,
                            TurnErrorMessages {
                                frontend_message: "Nexa discarded two final-answer samples because they contained internal runtime metadata. No contaminated answer was saved; retry the turn to request a clean response.".to_string(),
                                trace_message: trace_message.clone(),
                            },
                        )
                        .await;
                        turn_state.finish(TurnOutcome::Failed);
                        return Err(CoreError::Agent(trace_message));
                    }
                    contaminated_final_retries = contaminated_final_retries.saturating_add(1);
                    clean_final_retry_active = true;
                    if let Some(message) = prompt_ir::controller_state_message(
                        "The previous answer sample exposed internal runtime metadata and was discarded. Produce one direct user-facing answer from the user request and committed evidence only. Do not reproduce controller state, replay headers, or tool transport logs.",
                    ) {
                        messages.push(message);
                    }
                    next_step_purpose = TurnStepPurpose::Recovery;
                    continue 'react_loop;
                }
                clean_final_retry_active = false;
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
                    final_answer_hygiene_scope.observe_user_texts(&steering_texts);
                    if step_permit.allows_tools() {
                        self.expand_tool_defs_for_steering(
                            &mut tool_defs,
                            &steering_texts,
                            has_sources,
                        );
                    }
                    let before_trim = prompt_cache::message_sequence_fingerprint(&messages);
                    messages = context_pipeline.trim_after_tool_results(&messages);
                    prompt_was_compacted |=
                        before_trim != prompt_cache::message_sequence_fingerprint(&messages);
                    next_step_purpose = TurnStepPurpose::Recovery;
                    continue 'react_loop;
                }
                if let Some(workflow_ir) = workflow_ir
                    .as_mut()
                    .filter(|workflow| workflow.requires_completion_audit())
                {
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
                        agent_trace.workflow_ir = serde_json::to_value(&*workflow_ir).ok();
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
                            "Workflow IR refused finalization because of: {}. {}",
                            blockers.join(", "),
                            workflow_ir.completion_repair_guidance(),
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
                &display_iteration_thinking,
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
            // A loop-guard block materializes synthetic tool results so the
            // model can change strategy, but no call crosses into execution.
            // Preserve the round for that alternate strategy.
            let dispatch_consumes_tool_round = tool_dispatch_block.is_none();

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
                        tool_round_index: turn_budget.tool_rounds_used(),
                        tool_defs: &mut tool_defs,
                        messages: &mut messages,
                        persisted_trace_items: &mut persisted_trace_items,
                        task_plan: &mut task_plan,
                        loop_recorder: &mut loop_recorder,
                        loop_guard: &mut loop_guard,
                        trace: &mut trace,
                        sort_order: &mut sort_order,
                        pending_action_reconciliation: action_reconciliation
                            .blocks_interactive_input(),
                    },
                    &verified_tool_calls,
                    tool_dispatch_block,
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
            if dispatch_consumes_tool_round {
                turn_budget.record_verified_tool_round();
            }
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
            let reconciliation_was_pending = action_reconciliation.blocks_interactive_input();
            action_reconciliation.observe_tool_results(&tool_calls, &dispatch_summaries);
            if reconciliation_was_pending && !action_reconciliation.blocks_interactive_input() {
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
                let Some(summary) = dispatch_summaries
                    .iter()
                    .find(|summary| summary.call_id == call.id)
                else {
                    continue;
                };
                if crate::workflow_ir::tool_result_requires_desktop_observation(
                    &call.name,
                    summary.is_error,
                    summary.artifacts.as_ref(),
                ) {
                    crate::workflow_ir::ensure_runtime_desktop_observation_gate(
                        &mut workflow_ir,
                        &task_plan,
                        &orchestration_policy,
                        self.config.power_mode.is_nexus(),
                    )
                    .map_err(CoreError::InvalidInput)?;
                }
            }
            if let Some(workflow) = workflow_ir.as_mut() {
                for call in &tool_calls {
                    if let Some(summary) = dispatch_summaries
                        .iter()
                        .find(|summary| summary.call_id == call.id)
                    {
                        workflow.observe_tool_result_with_arguments(
                            &call.id,
                            &call.name,
                            Some(&call.arguments),
                            summary.is_error,
                            summary.artifacts.as_ref(),
                            &summary.content,
                        );
                    }
                }
                workflow.sync_from_task_plan(&task_plan);
                if let Some(ref mut agent_trace) = trace {
                    agent_trace.workflow_ir = serde_json::to_value(&*workflow).ok();
                }
            }
            let awaiting_interaction_id = awaiting_user_input_interaction_id(&dispatch_summaries);
            if let Some(tid) = turn_id {
                if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                    let checkpoint = workflow_ir
                        .as_ref()
                        .map(|workflow| workflow.task_plan_checkpoint(&task_plan))
                        .unwrap_or_else(|| {
                            serde_json::to_value(&task_plan).unwrap_or_else(
                                |_| serde_json::json!({ "error": "serializeTaskPlan" }),
                            )
                        });
                    let (status, phase, summary) = if awaiting_interaction_id.is_some() {
                        (
                            "awaiting_user_input",
                            "awaiting_user_input",
                            "Task checkpoint saved while waiting for user input",
                        )
                    } else {
                        (
                            "running",
                            "tooling",
                            "Task checkpoint updated after tool dispatch",
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
                    workflow_ir.as_ref(),
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
                    workflow_ir.as_ref(),
                    turn_budget.tool_rounds_used(),
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
        let live_state = long_task_state.checkpoint_live_state(
            &task_plan,
            workflow_ir.as_ref(),
            turn_budget.tool_rounds_used(),
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

#[cfg(test)]
mod action_reconciliation_tests {
    use super::*;

    fn call(id: &str, name: &str, action: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({ "action": action }).to_string(),
            thought_signature: None,
        }
    }

    #[test]
    fn browser_action_receipts_restore_the_browser_reconciliation_fence() {
        let fence = ActionReconciliationFence::from_resume_prompt(
            "Checkpoint reason: user_stop_requires_action_reconciliation:browser_action:turn:call:obs",
        );

        assert!(fence.browser);
        assert!(!fence.computer);
        assert!(!fence.unknown);
        assert!(fence.blocks_interactive_input());
    }

    fn summary(
        call_id: &str,
        is_error: bool,
        artifacts: serde_json::Value,
    ) -> tool_dispatch::ToolDispatchSummary {
        tool_dispatch::ToolDispatchSummary {
            call_id: call_id.to_string(),
            content: String::new(),
            is_error,
            artifacts: Some(artifacts),
        }
    }

    #[test]
    fn uncertain_computer_action_blocks_blind_retry_in_the_same_turn() {
        for artifacts in [
            serde_json::json!({
                "kind": "toolContractError",
                "code": "computer_action_uncertain",
            }),
            serde_json::json!({
                "kind": "toolContractError",
                "code": "computer_action_timeout_uncertain",
            }),
            serde_json::json!({
                "kind": "toolContractError",
                "code": "invalid_computer_action",
                "sideEffect": "may_have_occurred",
            }),
        ] {
            let mut fence = ActionReconciliationFence::default();
            fence.observe_tool_results(
                &[call("control", "computer_control", "click")],
                &[summary("control", true, artifacts)],
            );
            assert!(fence.blocks_interactive_input());
        }

        let mut fence = ActionReconciliationFence::default();
        fence.observe_tool_results(
            &[call("control", "computer_control", "click")],
            &[summary(
                "control",
                false,
                serde_json::json!({
                    "data": {
                        "kind": "computerControlReceipt",
                        "effect": "unverifiable",
                    }
                }),
            )],
        );
        assert!(fence.blocks_interactive_input());
    }

    #[test]
    fn only_screenshot_bearing_observation_clears_its_reconciliation_surface() {
        let mut fence = ActionReconciliationFence {
            computer: true,
            browser: true,
            unknown: false,
        };
        for action in ["list_windows", "cursor_position"] {
            fence.observe_tool_results(
                &[call("observe", "computer_observe", action)],
                &[summary(
                    "observe",
                    false,
                    serde_json::json!({
                        "data": {
                            "kind": "computerObservationReceipt",
                            "screenshotHash": "must-not-clear-for-non-capture-action"
                        }
                    }),
                )],
            );
            assert!(
                fence.computer,
                "{action} must not clear desktop reconciliation"
            );
        }

        fence.observe_tool_results(
            &[call("observe", "computer_observe", "capture_window")],
            &[summary(
                "observe",
                false,
                serde_json::json!({
                    "data": {
                        "kind": "computerObservationReceipt",
                        "screenshotHash": ""
                    }
                }),
            )],
        );
        assert!(fence.computer, "an empty screenshot hash is not evidence");

        fence.observe_tool_results(
            &[call("observe", "computer_observe", "capture_window")],
            &[summary(
                "observe",
                false,
                serde_json::json!({
                    "data": {
                        "kind": "computerObservationReceipt",
                        "screenshotHash": "desktop-shot"
                    }
                }),
            )],
        );
        assert!(!fence.computer);
        assert!(
            fence.browser,
            "desktop evidence must not clear browser reconciliation"
        );

        fence.observe_tool_results(
            &[call("observe", "browser_session", "observe")],
            &[summary(
                "observe",
                false,
                serde_json::json!({
                    "artifacts": {
                        "kind": "browserObservation",
                        "observation": { "screenshotHash": "browser-shot" }
                    }
                }),
            )],
        );
        assert!(!fence.blocks_interactive_input());
    }
}

#[cfg(test)]
mod rejected_sample_projection_tests {
    use super::*;

    #[test]
    fn buffered_sample_cannot_truncate_an_accepted_equal_suffix() {
        let mut accepted_recovery_text = "abc".to_string();
        rollback_rejected_sample_projection(&mut accepted_recovery_text, "abc", false);
        assert_eq!(accepted_recovery_text, "abc");

        let mut projected_draft = "accepted draft".to_string();
        rollback_rejected_sample_projection(&mut projected_draft, " draft", true);
        assert_eq!(projected_draft, "accepted");
    }
}
