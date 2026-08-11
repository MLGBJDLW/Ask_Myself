//! One model sampling step: request construction, streaming, recovery, and stream-time steering.

use super::steering::SteeringDrainContext;
use super::*;
use crate::llm::FinishReason;

fn request_tools_with_native_search_plan(
    tool_defs: &[ToolDefinition],
    plan: crate::llm::native_search::NativeSearchPlan,
) -> Result<Vec<ToolDefinition>, CoreError> {
    let has_exposed_search = tool_defs
        .iter()
        .any(|tool| tool.name == crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL);
    let mut request_tools = tool_defs
        .iter()
        .filter(|tool| !crate::llm::native_search::is_native_marker(tool))
        .cloned()
        .collect::<Vec<_>>();
    if has_exposed_search {
        plan.validate()?;
        request_tools.extend(plan.marker());
    }
    Ok(request_tools)
}

fn effective_sample_replay_policy(
    route_policy: ReasoningReplayPolicy,
    iteration_thinking: &str,
    output_payload: &crate::llm::provider_turn::ProviderReplayPayload,
    tool_calls: &[ToolCallRequest],
) -> ReasoningReplayPolicy {
    let sample_contains_reasoning_state = !iteration_thinking.trim().is_empty()
        || output_payload.is_present()
        || tool_calls.iter().any(|call| {
            call.thought_signature
                .as_deref()
                .is_some_and(|signature| !signature.trim().is_empty())
        });

    if !tool_calls.is_empty()
        && matches!(
            route_policy,
            ReasoningReplayPolicy::Unknown | ReasoningReplayPolicy::Forbidden
        )
        && !sample_contains_reasoning_state
    {
        ReasoningReplayPolicy::NotRequired
    } else {
        route_policy
    }
}

fn stream_chunk_has_semantic_output(chunk: &crate::llm::StreamChunk) -> bool {
    !chunk.delta.is_empty()
        || chunk
            .thinking_delta
            .as_deref()
            .is_some_and(|thinking| !thinking.is_empty())
        || chunk.tool_call_delta.is_some()
}

fn completion_has_semantic_output(response: &crate::llm::CompletionResponse) -> bool {
    !response.content.is_empty()
        || response
            .thinking
            .as_deref()
            .is_some_and(|thinking| !thinking.is_empty())
        || response
            .tool_calls
            .as_ref()
            .is_some_and(|tool_calls| !tool_calls.is_empty())
}

pub(super) struct ModelStepContext<'a> {
    pub(super) db: &'a Database,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) route_kind: AgentRouteKind,
    pub(super) model: &'a str,
    pub(super) max_response_tokens: u32,
    pub(super) has_sources: bool,
    pub(super) privacy_cfg: &'a privacy::PrivacyConfig,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) tool_defs: &'a mut Vec<ToolDefinition>,
    pub(super) accumulated_content: &'a mut String,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) sort_order: &'a mut i64,
    pub(super) context_recovery_attempts: &'a mut u32,
    pub(super) force_non_streaming_llm: &'a mut bool,
    pub(super) reasoning_disabled_for_tool_loop: &'a mut bool,
    pub(super) force_answer_only: bool,
}

pub(super) enum ModelStepOutcome {
    Completed(Box<ModelStepOutput>),
    Restart { prompt_was_compacted: bool },
}

pub(super) struct ModelStepOutput {
    pub(super) full_content: String,
    pub(super) tool_calls: Vec<ToolCallRequest>,
    pub(super) chunk_usage: Option<Usage>,
    pub(super) iteration_thinking: String,
    pub(super) answer_delta_seen: bool,
    pub(super) thinking_delta_seen: bool,
    pub(super) finish_reason: Option<FinishReason>,
    pub(super) started_call_ids: HashSet<String>,
    pub(super) tool_run_started_ids: HashSet<String>,
    pub(super) prompt_cache_observation: Option<prompt_cache::PromptCacheTraceObservation>,
    pub(super) request_latency_ms: u64,
    pub(super) time_to_first_token_ms: Option<u64>,
    pub(super) sample_id: String,
    pub(super) route_snapshot: crate::llm::provider_turn::RouteSnapshot,
    pub(super) reasoning_was_requested: bool,
}

#[allow(clippy::too_many_arguments)]
fn reset_iteration_capture_for_new_sample(
    accumulated_content: &mut String,
    accumulated_len_before_iteration: usize,
    full_content: &mut String,
    tool_calls: &mut Vec<ToolCallRequest>,
    chunk_usage: &mut Option<Usage>,
    iteration_thinking: &mut String,
    answer_delta_seen: &mut bool,
    thinking_delta_seen: &mut bool,
    finish_reason: &mut Option<FinishReason>,
    preparing_call_ids: &mut HashSet<String>,
    started_call_ids: &mut HashSet<String>,
    tool_run_started_ids: &mut HashSet<String>,
    chunk_count: &mut usize,
) {
    accumulated_content.truncate(accumulated_len_before_iteration);
    full_content.clear();
    tool_calls.clear();
    *chunk_usage = None;
    iteration_thinking.clear();
    *answer_delta_seen = false;
    *thinking_delta_seen = false;
    *finish_reason = None;
    preparing_call_ids.clear();
    started_call_ids.clear();
    tool_run_started_ids.clear();
    *chunk_count = 0;
}

impl AgentExecutor {
    fn persist_interrupted_provider_draft(
        &self,
        ctx: assistant_turn::AssistantTurnPersistenceContext<'_>,
        accepted: &model_attempt::AcceptedModelAttempt,
        full_content: &str,
        iteration_thinking: &str,
        reasoning_was_requested: bool,
    ) {
        if full_content.trim().is_empty() && iteration_thinking.trim().is_empty() {
            return;
        }

        let draft_reasoning = self.reasoning_content_for_iteration(iteration_thinking, false);
        let mut draft_message = Message {
            role: Role::Assistant,
            parts: vec![ContentPart::Text {
                text: full_content.to_string(),
            }],
            name: None,
            tool_calls: None,
            reasoning_content: draft_reasoning.clone(),
            prompt_cache_hint: None,
        };
        let mut draft_envelope = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
            Uuid::new_v4().to_string(),
            accepted.sample_id.clone(),
            accepted.route_snapshot.clone(),
            draft_message.text_content(),
            crate::llm::reasoning_replay::sanitize_reasoning_text(Some(iteration_thinking))
                .as_deref(),
            draft_reasoning.as_deref(),
            Vec::new(),
            reasoning_was_requested,
        );
        draft_envelope.capture_status = ReasoningCaptureStatus::Interrupted;
        draft_message.set_provider_turn(draft_envelope);
        self.persist_stream_interrupted_assistant_draft(
            ctx,
            &draft_message,
            draft_reasoning,
            iteration_thinking,
        );
    }

    pub(super) async fn run_model_step(
        &self,
        ctx: ModelStepContext<'_>,
    ) -> Result<ModelStepOutcome, CoreError> {
        let ModelStepContext {
            db,
            tx,
            conversation_id,
            turn_id,
            route_kind,
            model,
            max_response_tokens,
            has_sources,
            privacy_cfg,
            messages,
            tool_defs,
            accumulated_content,
            persisted_trace_items,
            trace,
            sort_order,
            context_recovery_attempts,
            force_non_streaming_llm,
            reasoning_disabled_for_tool_loop,
            force_answer_only,
        } = ctx;

        // -- 4a. Project one model attempt -----------------------------------
        let context_recovery_policy = StreamRecoveryPolicy::default();
        let request_tools =
            request_tools_with_native_search_plan(tool_defs, self.config.native_search_plan)?;
        let mut current_request = CompletionRequest {
            model: model.to_string(),
            messages: messages.to_vec(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools: if request_tools.is_empty() {
                None
            } else {
                Some(request_tools)
            },
            stop: None,
            thinking_budget: if !force_answer_only
                && !*reasoning_disabled_for_tool_loop
                && self.config.reasoning_enabled.unwrap_or(false)
            {
                self.config.thinking_budget
            } else {
                None
            },
            reasoning_enabled: if force_answer_only || *reasoning_disabled_for_tool_loop {
                Some(false)
            } else {
                self.config.reasoning_enabled
            },
            reasoning_effort: if !force_answer_only
                && !*reasoning_disabled_for_tool_loop
                && self.config.reasoning_enabled.unwrap_or(false)
            {
                self.config.reasoning_effort.clone()
            } else {
                None
            },
            provider_type: self.config.provider_type,
            routing_session_id: conversation_id
                .and_then(crate::llm::prompt_cache::privacy_preserving_routing_session_id),
            parallel_tool_calls: true,
        };
        // The attempt seam owns route-specific replay projection. Keep this
        // request unprojected so an automatic fallback can select a concrete
        // route without inheriting a lossy primary-route projection.
        let reasoning_was_requested = current_request.reasoning_enabled != Some(false)
            && current_request.reasoning_effort != Some(ReasoningEffort::None);
        let mut accepted_attempt: Option<model_attempt::AcceptedModelAttempt> = None;
        self.begin_prompt_cache_observation(model, messages, tool_defs);
        let accumulated_len_before_iteration = accumulated_content.len();
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        let mut chunk_usage: Option<Usage> = None;
        let mut iteration_thinking = String::new();
        let mut answer_delta_seen = false;
        let mut thinking_delta_seen = false;
        let mut finish_reason: Option<FinishReason> = None;
        let mut preparing_call_ids: HashSet<String> = HashSet::new();
        let mut started_call_ids: HashSet<String> = HashSet::new();
        let mut tool_run_started_ids: HashSet<String> = HashSet::new();
        let mut chunk_count: usize = 0;
        let mut stream_interrupted_by_steering: Option<AgentSteeringMessage> = None;
        let mut steering_closed = false;

        let mut model_attempt = model_attempt::ModelAttempt::new(
            self.provider.as_ref(),
            current_request.clone(),
            tx,
            *force_non_streaming_llm,
        )
        .with_cancel_token(self.cancel_token.clone());

        macro_rules! bind_accepted_attempt {
            ($next:expr, $first_for_sample:expr) => {{
                let next = $next;
                let sample_changed = accepted_attempt
                    .as_ref()
                    .is_some_and(|current| current.sample_id.as_str() != next.sample_id.as_str());
                if sample_changed {
                    reset_iteration_capture_for_new_sample(
                        accumulated_content,
                        accumulated_len_before_iteration,
                        &mut full_content,
                        &mut tool_calls,
                        &mut chunk_usage,
                        &mut iteration_thinking,
                        &mut answer_delta_seen,
                        &mut thinking_delta_seen,
                        &mut finish_reason,
                        &mut preparing_call_ids,
                        &mut started_call_ids,
                        &mut tool_run_started_ids,
                        &mut chunk_count,
                    );
                }
                let newly_bound = $first_for_sample || accepted_attempt.is_none() || sample_changed;
                if newly_bound && next.replay_projection_omitted_units > 0 {
                    append_developer_persisted_trace_status(
                        persisted_trace_items,
                        &format!(
                            "provider_replay_boundary: omitted_units={}, provider={}, api_style={}",
                            next.replay_projection_omitted_units,
                            next.route_snapshot.provider_family,
                            next.route_snapshot.api_style_id()
                        ),
                        "warning",
                    );
                }
                accepted_attempt = Some(next);
            }};
        }

        let attempt_timing = 'model_attempt: loop {
            let progress = if model_attempt.accepts_stream_steering()
                && self.steering_rx.is_some()
                && !steering_closed
            {
                tokio::select! {
                    maybe_steering = self.wait_for_steering_message() => {
                        match maybe_steering {
                            Some(steering) => {
                                stream_interrupted_by_steering = Some(steering);
                                break 'model_attempt None;
                            }
                            None => {
                                steering_closed = true;
                                continue 'model_attempt;
                            }
                        }
                    }
                    progress = model_attempt.next() => progress,
                }
            } else {
                model_attempt.next().await
            };

            match progress {
                model_attempt::ModelAttemptProgress::StreamOpened => {}
                model_attempt::ModelAttemptProgress::Provider(
                    model_attempt::ModelAttemptProviderEvent {
                        event: model_attempt::AcceptedProviderEvent::Chunk(chunk),
                        accepted,
                        first_for_sample,
                    },
                ) => {
                    bind_accepted_attempt!(accepted, first_for_sample);
                    if stream_chunk_has_semantic_output(&chunk) {
                        *context_recovery_attempts = 0;
                    }
                    chunk_count += 1;
                    // Forward thinking deltas.
                    if let Some(ref thinking) = chunk.thinking_delta {
                        if !thinking.is_empty() {
                            thinking_delta_seen = true;
                            iteration_thinking.push_str(thinking);
                            let _ = tx
                                .send(AgentEvent::Thinking {
                                    content: thinking.clone(),
                                })
                                .await;
                        }
                    }
                    // Forward text deltas.
                    if !chunk.delta.is_empty() {
                        answer_delta_seen = true;
                        full_content.push_str(&chunk.delta);
                        accumulated_content.push_str(&chunk.delta);
                        let _ = tx.send(AgentEvent::TextDelta { delta: chunk.delta }).await;
                    }
                    // Accumulate tool-call deltas.
                    if let Some(ref tc_delta) = chunk.tool_call_delta {
                        accumulate_tool_call(&mut tool_calls, tc_delta);

                        // Emit a stable preparing signal while arguments are
                        // still being assembled. Do not stream partial
                        // generic arguments to the UI; they are often
                        // invalid JSON until the provider finishes the call.
                        if let Some((tc_index, tc)) = resolve_delta_target(&tool_calls, tc_delta) {
                            let partial_args_value =
                                serde_json::from_str::<serde_json::Value>(&tc.arguments)
                                    .unwrap_or(serde_json::Value::Null);
                            let capabilities =
                                self.tools.run_capabilities(&tc.name, &partial_args_value);
                            let preview_arguments = if matches!(
                                capabilities.input_streaming,
                                ToolInputStreamingMode::UiPreview
                                    | ToolInputStreamingMode::ToolConsumesPartial
                            ) {
                                Some(tc.arguments.as_str())
                            } else {
                                None
                            };
                            if !tc.name.is_empty() && preparing_call_ids.insert(tc.id.clone()) {
                                if tool_run_started_ids.insert(tc.id.clone()) {
                                    let _ = tx
                                        .send(AgentEvent::ToolRunStarted {
                                            run: build_tool_run_item(
                                                &self.tools,
                                                &tc.id,
                                                &tc.name,
                                                ToolRunStatus::Preparing,
                                                preview_arguments,
                                                None,
                                                None,
                                                None,
                                                None,
                                                None,
                                            ),
                                        })
                                        .await;
                                }
                                let _ = tx
                                    .send(AgentEvent::ToolCallPreparing {
                                        call_id: tc.id.clone(),
                                        tool_name: tc.name.clone(),
                                        args_bytes: tc.arguments.len() as u32,
                                        index: tc_index as u32,
                                    })
                                    .await;
                            } else if !tc.name.is_empty()
                                && preview_arguments.is_some()
                                && !tc_delta.arguments_delta.is_empty()
                            {
                                let _ = tx
                                    .send(AgentEvent::ToolRunUpdated {
                                        run: build_tool_run_item(
                                            &self.tools,
                                            &tc.id,
                                            &tc.name,
                                            ToolRunStatus::Preparing,
                                            preview_arguments,
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
                    }
                    if let Some(ref fr) = chunk.finish_reason {
                        finish_reason = Some(fr.clone());
                    }
                    if let Some(u) = chunk.usage {
                        chunk_usage = Some(u);
                    }
                }
                model_attempt::ModelAttemptProgress::Provider(
                    model_attempt::ModelAttemptProviderEvent {
                        event: model_attempt::AcceptedProviderEvent::HostedTool(tool),
                        accepted,
                        first_for_sample,
                    },
                ) => {
                    bind_accepted_attempt!(accepted, first_for_sample);
                    *context_recovery_attempts = 0;
                    let status = tool.status;
                    let run = build_provider_hosted_tool_run_item(&self.tools, &tool);
                    match status {
                        ProviderHostedToolStatus::Running => {
                            let event = if tool_run_started_ids.insert(run.call_id.clone()) {
                                AgentEvent::ToolRunStarted { run }
                            } else {
                                AgentEvent::ToolRunUpdated { run }
                            };
                            let _ = tx.send(event).await;
                        }
                        ProviderHostedToolStatus::Completed | ProviderHostedToolStatus::Failed => {
                            if tool_run_started_ids.insert(run.call_id.clone()) {
                                let mut started = run.clone();
                                started.status = ToolRunStatus::Running;
                                started.content = None;
                                started.is_error = None;
                                let _ = tx.send(AgentEvent::ToolRunStarted { run: started }).await;
                            }
                            append_persisted_trace_tool_run(persisted_trace_items, &run);
                            let _ = tx.send(AgentEvent::ToolRunCompleted { run }).await;
                        }
                    }
                }
                model_attempt::ModelAttemptProgress::StreamComplete { accepted, timing } => {
                    bind_accepted_attempt!(accepted, false);
                    info!(
                        "Stream complete: {chunk_count} chunks, {} chars",
                        full_content.len()
                    );
                    break 'model_attempt Some(timing);
                }
                model_attempt::ModelAttemptProgress::Completion(completion) => {
                    let model_attempt::ModelAttemptCompletion {
                        response,
                        accepted,
                        timing,
                        switched_to_non_streaming,
                    } = completion;
                    bind_accepted_attempt!(accepted, true);
                    if completion_has_semantic_output(&response) {
                        *context_recovery_attempts = 0;
                    }
                    *force_non_streaming_llm |= switched_to_non_streaming;

                    accumulated_content.truncate(accumulated_len_before_iteration);
                    full_content = response.content;
                    answer_delta_seen = !full_content.is_empty();
                    accumulated_content.push_str(&full_content);
                    iteration_thinking = response.thinking.unwrap_or_default();
                    thinking_delta_seen = !iteration_thinking.is_empty();
                    tool_calls = response.tool_calls.unwrap_or_default();
                    preparing_call_ids.clear();
                    started_call_ids.clear();
                    tool_run_started_ids.clear();
                    chunk_usage = Some(response.usage);
                    finish_reason = Some(response.finish_reason);

                    if !iteration_thinking.is_empty() {
                        let _ = tx
                            .send(AgentEvent::Thinking {
                                content: iteration_thinking.clone(),
                            })
                            .await;
                    }
                    if !full_content.is_empty() {
                        let _ = tx
                            .send(AgentEvent::TextDelta {
                                delta: full_content.clone(),
                            })
                            .await;
                    }
                    for (index, tool_call) in tool_calls.iter().enumerate() {
                        if tool_call.name.is_empty() {
                            continue;
                        }
                        let args_value =
                            serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                                .unwrap_or(serde_json::Value::Null);
                        let capabilities =
                            self.tools.run_capabilities(&tool_call.name, &args_value);
                        let preview_arguments = if matches!(
                            capabilities.input_streaming,
                            ToolInputStreamingMode::UiPreview
                                | ToolInputStreamingMode::ToolConsumesPartial
                        ) {
                            Some(tool_call.arguments.as_str())
                        } else {
                            None
                        };
                        preparing_call_ids.insert(tool_call.id.clone());
                        if tool_run_started_ids.insert(tool_call.id.clone()) {
                            let _ = tx
                                .send(AgentEvent::ToolRunStarted {
                                    run: build_tool_run_item(
                                        &self.tools,
                                        &tool_call.id,
                                        &tool_call.name,
                                        ToolRunStatus::Preparing,
                                        preview_arguments,
                                        None,
                                        None,
                                        None,
                                        None,
                                        None,
                                    ),
                                })
                                .await;
                        }
                        let _ = tx
                            .send(AgentEvent::ToolCallPreparing {
                                call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                args_bytes: tool_call.arguments.len() as u32,
                                index: index as u32,
                            })
                            .await;
                    }
                    info!(
                        "Completion complete: {} chars, {} tool calls",
                        full_content.len(),
                        tool_calls.len()
                    );
                    break 'model_attempt Some(timing);
                }
                model_attempt::ModelAttemptProgress::InterruptedAfterVisibleOutput(
                    interruption,
                ) => {
                    let model_attempt::ModelAttemptInterruption {
                        accepted,
                        user_message,
                        trace_message,
                        timing: _timing,
                    } = interruption;
                    bind_accepted_attempt!(accepted, false);
                    warn!("LLM stream interrupted after visible output: {trace_message}");
                    let accepted = accepted_attempt
                        .as_ref()
                        .expect("interrupted visible output has accepted provenance");
                    self.persist_interrupted_provider_draft(
                        assistant_turn::AssistantTurnPersistenceContext {
                            db,
                            conversation_id,
                            turn_id,
                            model,
                            route_kind,
                            persisted_trace_items: &mut *persisted_trace_items,
                            sort_order: &mut *sort_order,
                        },
                        accepted,
                        &full_content,
                        &iteration_thinking,
                        reasoning_was_requested,
                    );
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
                    return Err(CoreError::StreamIncomplete(trace_message));
                }
                model_attempt::ModelAttemptProgress::NeedsContextCompaction(overflow) => {
                    let model_attempt::ModelAttemptContextOverflow {
                        error,
                        timing: _timing,
                    } = overflow;
                    match context_recovery_policy
                        .decide_after_context_overflow(*context_recovery_attempts, &error)
                    {
                        ContextOverflowRecoveryDecision::Compact {
                            attempt,
                            status_message,
                        } => {
                            *context_recovery_attempts = attempt;
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
                                    db,
                                    conversation_id,
                                    turn_id,
                                )
                                .await?;
                            if !recovered {
                                emit_error_and_finalize_turn(
                                    tx,
                                    db,
                                    trace,
                                    turn_id,
                                    route_kind,
                                    persisted_trace_items,
                                    TurnErrorMessages {
                                        frontend_message: format!(
                                            "Context overflow could not be reduced further: {error}"
                                        ),
                                        trace_message: error.to_string(),
                                    },
                                )
                                .await;
                                return Err(error);
                            }
                            if accepted_attempt.is_some() {
                                reset_iteration_capture_for_new_sample(
                                    accumulated_content,
                                    accumulated_len_before_iteration,
                                    &mut full_content,
                                    &mut tool_calls,
                                    &mut chunk_usage,
                                    &mut iteration_thinking,
                                    &mut answer_delta_seen,
                                    &mut thinking_delta_seen,
                                    &mut finish_reason,
                                    &mut preparing_call_ids,
                                    &mut started_call_ids,
                                    &mut tool_run_started_ids,
                                    &mut chunk_count,
                                );
                            }
                            accepted_attempt = None;
                            current_request.messages = messages.to_vec();
                            model_attempt = model_attempt::ModelAttempt::new(
                                self.provider.as_ref(),
                                current_request.clone(),
                                tx,
                                *force_non_streaming_llm,
                            )
                            .with_cancel_token(self.cancel_token.clone());
                            continue 'model_attempt;
                        }
                        ContextOverflowRecoveryDecision::GiveUp { user_message } => {
                            emit_error_and_finalize_turn(
                                tx,
                                db,
                                trace,
                                turn_id,
                                route_kind,
                                persisted_trace_items,
                                TurnErrorMessages {
                                    frontend_message: user_message,
                                    trace_message: error.to_string(),
                                },
                            )
                            .await;
                            return Err(error);
                        }
                    }
                }
                model_attempt::ModelAttemptProgress::Failed(failure) => {
                    let model_attempt::ModelAttemptFailure {
                        stage: _stage,
                        error,
                        user_message,
                        trace_message,
                        accepted: _accepted,
                        timing: _timing,
                    } = failure;
                    error!("LLM model attempt failed: {trace_message}");
                    emit_error_and_finalize_turn(
                        tx,
                        db,
                        trace,
                        turn_id,
                        route_kind,
                        persisted_trace_items,
                        TurnErrorMessages {
                            frontend_message: user_message,
                            trace_message,
                        },
                    )
                    .await;
                    return Err(error);
                }
                model_attempt::ModelAttemptProgress::Cancelled(cancellation) => {
                    let model_attempt::ModelAttemptCancellation {
                        message,
                        accepted,
                        timing: _timing,
                    } = cancellation;
                    if let Some(accepted) = accepted {
                        bind_accepted_attempt!(accepted, false);
                        let accepted = accepted_attempt
                            .as_ref()
                            .expect("accepted cancellation preserves provider provenance");
                        self.persist_interrupted_provider_draft(
                            assistant_turn::AssistantTurnPersistenceContext {
                                db,
                                conversation_id,
                                turn_id,
                                model,
                                route_kind,
                                persisted_trace_items: &mut *persisted_trace_items,
                                sort_order: &mut *sort_order,
                            },
                            accepted,
                            &full_content,
                            &iteration_thinking,
                            reasoning_was_requested,
                        );
                    }
                    warn!("LLM model attempt cancelled: {message}");
                    emit_error_and_finalize_turn(
                        tx,
                        db,
                        trace,
                        turn_id,
                        route_kind,
                        persisted_trace_items,
                        TurnErrorMessages {
                            frontend_message: message.clone(),
                            trace_message: message.clone(),
                        },
                    )
                    .await;
                    return Err(CoreError::Cancelled(message));
                }
            }
        };

        if let Some(steering) = stream_interrupted_by_steering {
            let steering_messages = self.collect_steering_messages(Some(steering)).await;
            let has_effective_steering = steering_messages
                .iter()
                .any(Self::steering_message_has_effective_content);
            if !full_content.trim().is_empty() {
                let draft_reasoning =
                    self.reasoning_content_for_iteration(&iteration_thinking, false);
                let mut draft_message = Message {
                    role: Role::Assistant,
                    parts: vec![ContentPart::Text {
                        text: full_content.clone(),
                    }],
                    name: None,
                    tool_calls: None,
                    reasoning_content: draft_reasoning.clone(),
                    prompt_cache_hint: None,
                };
                let accepted = accepted_attempt
                    .as_ref()
                    .expect("visible steering draft has accepted provenance");
                let mut draft_envelope = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                    Uuid::new_v4().to_string(),
                    accepted.sample_id.clone(),
                    accepted.route_snapshot.clone(),
                    draft_message.text_content(),
                    crate::llm::reasoning_replay::sanitize_reasoning_text(Some(
                        &iteration_thinking,
                    ))
                    .as_deref(),
                    draft_reasoning.as_deref(),
                    Vec::new(),
                    reasoning_was_requested,
                );
                draft_envelope.capture_status = ReasoningCaptureStatus::Interrupted;
                draft_message.set_provider_turn(draft_envelope);
                messages.push(draft_message.clone());
                if has_effective_steering {
                    self.persist_stream_interrupted_assistant_draft(
                        assistant_turn::AssistantTurnPersistenceContext {
                            db,
                            conversation_id,
                            turn_id,
                            model,
                            route_kind,
                            persisted_trace_items: &mut *persisted_trace_items,
                            sort_order: &mut *sort_order,
                        },
                        &draft_message,
                        draft_reasoning,
                        &iteration_thinking,
                    );
                }
            }
            let steering_texts = {
                let mut steering_ctx = SteeringDrainContext {
                    db,
                    conversation_id,
                    tx,
                    model,
                    sort_order,
                    privacy_cfg,
                };
                self.apply_steering_messages(messages, &mut steering_ctx, steering_messages)
                    .await
            };
            if steering_texts.is_empty() {
                return Ok(ModelStepOutcome::Restart {
                    prompt_was_compacted: false,
                });
            }
            let reason = "Steering message received; restarting the model response.";
            let _ = tx
                .send(AgentEvent::StreamReset {
                    reason: reason.to_string(),
                })
                .await;
            accumulated_content.truncate(accumulated_len_before_iteration);
            self.expand_tool_defs_for_steering(tool_defs, &steering_texts, has_sources);
            append_internal_persisted_trace_status(
                persisted_trace_items,
                "Applied user steering during streaming and restarted the model response.",
                "info",
            );
            if let Some(tid) = turn_id {
                let trace = build_turn_trace(route_kind, persisted_trace_items);
                let _ = db.update_conversation_turn_progress(
                    tid,
                    Some(&format!("{:?}", route_kind)),
                    Some(&trace),
                );
            }
            let max_ctx = self
                .config
                .context_window
                .unwrap_or_else(|| model_context_window(model));
            let before_trim = prompt_cache::message_sequence_fingerprint(messages);
            *messages = trim_to_context_window(
                messages,
                max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                max_response_tokens,
            );
            return Ok(ModelStepOutcome::Restart {
                prompt_was_compacted: before_trim
                    != prompt_cache::message_sequence_fingerprint(messages),
            });
        }

        let mut attempt_timing =
            attempt_timing.expect("completed model attempt must report timing");
        let accepted_attempt = accepted_attempt.ok_or_else(|| {
            CoreError::Internal(
                "Completed model attempt did not bind provider provenance".to_string(),
            )
        })?;
        let mut accepted_sample_id = accepted_attempt.sample_id;
        let mut accepted_route_snapshot = accepted_attempt.route_snapshot;

        let output_payload = crate::llm::provider_turn::ProviderReplayPayload::capture(
            &accepted_route_snapshot,
            Some(&iteration_thinking),
            &tool_calls,
        );
        let effective_replay_policy = effective_sample_replay_policy(
            accepted_route_snapshot.replay_policy,
            &iteration_thinking,
            &output_payload,
            &tool_calls,
        );
        if effective_replay_policy != accepted_route_snapshot.replay_policy {
            accepted_route_snapshot.replay_policy = effective_replay_policy;
            append_developer_persisted_trace_status(
                    persisted_trace_items,
                    "provider_replay_sample: no reasoning state was present, so this tool sample requires no reasoning replay",
                    "info",
                );
        }
        let current_accepted_route = &accepted_route_snapshot;
        let unsafe_tool_turn = !tool_calls.is_empty()
            && !current_accepted_route
                .replay_policy
                .authorizes_tool_call(output_payload.is_present());
        if unsafe_tool_turn {
            let rejected_stream_sample = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                Uuid::new_v4().to_string(),
                accepted_sample_id.clone(),
                current_accepted_route.clone(),
                full_content.clone(),
                crate::llm::reasoning_replay::sanitize_reasoning_text(Some(&iteration_thinking))
                    .as_deref(),
                None,
                tool_calls.clone(),
                reasoning_was_requested,
            );
            self.persist_provider_sample_without_message(
                db,
                conversation_id,
                turn_id,
                &rejected_stream_sample,
            )?;
            append_developer_persisted_trace_status(
                    persisted_trace_items,
                    "reasoning_replay_recovery: tool calls omitted required replay payload; starting a new reasoning-disabled sample before dispatch",
                    "warning",
                );
            let _ = tx
                    .send(AgentEvent::ControllerStatus {
                        code: "reasoning_replay_recovery".to_string(),
                        content: "The provider omitted replay state required for a safe tool turn. Restarting this step before any tool runs.".to_string(),
                        tone: Some("warning".to_string()),
                    })
                    .await;

            let required_route = current_accepted_route.clone();
            let mut safe_request = current_request.clone();
            safe_request.reasoning_enabled = Some(false);
            safe_request.reasoning_effort = None;
            safe_request.thinking_budget = None;
            let safe_completion = loop {
                let mut safe_attempt = model_attempt::ModelAttempt::new(
                    self.provider.as_ref(),
                    safe_request.clone(),
                    tx,
                    true,
                )
                .with_cancel_token(self.cancel_token.clone());
                match safe_attempt.next().await {
                    model_attempt::ModelAttemptProgress::Completion(completion) => {
                        break completion;
                    }
                    model_attempt::ModelAttemptProgress::NeedsContextCompaction(overflow) => {
                        let model_attempt::ModelAttemptContextOverflow {
                            error,
                            timing: _timing,
                        } = overflow;
                        match context_recovery_policy
                            .decide_after_context_overflow(*context_recovery_attempts, &error)
                        {
                            ContextOverflowRecoveryDecision::Compact {
                                attempt,
                                status_message,
                            } => {
                                *context_recovery_attempts = attempt;
                                let _ = tx
                                    .send(AgentEvent::Status {
                                        content: status_message,
                                        tone: Some("muted".to_string()),
                                    })
                                    .await;
                                let recovered = match self
                                    .recover_context_overflow(
                                        messages,
                                        model,
                                        tx,
                                        db,
                                        conversation_id,
                                        turn_id,
                                    )
                                    .await
                                {
                                    Ok(recovered) => recovered,
                                    Err(recovery_error) => {
                                        let trace_message = format!(
                                            "reasoning_replay_safe_restart_context_recovery_failed: {recovery_error}"
                                        );
                                        append_developer_persisted_trace_status(
                                            persisted_trace_items,
                                            &trace_message,
                                            "error",
                                        );
                                        emit_error_and_finalize_turn(
                                            tx,
                                            db,
                                            trace,
                                            turn_id,
                                            route_kind,
                                            persisted_trace_items,
                                            TurnErrorMessages {
                                                frontend_message: "Context recovery failed while preparing a replay-safe tool step. No tools were executed.".to_string(),
                                                trace_message,
                                            },
                                        )
                                        .await;
                                        return Err(recovery_error);
                                    }
                                };
                                if !recovered {
                                    emit_error_and_finalize_turn(
                                        tx,
                                        db,
                                        trace,
                                        turn_id,
                                        route_kind,
                                        persisted_trace_items,
                                        TurnErrorMessages {
                                            frontend_message: format!(
                                                "Context overflow could not be reduced for the replay-safe tool step: {error}"
                                            ),
                                            trace_message: error.to_string(),
                                        },
                                    )
                                    .await;
                                    return Err(error);
                                }
                                safe_request.messages = messages.to_vec();
                            }
                            ContextOverflowRecoveryDecision::GiveUp { user_message } => {
                                emit_error_and_finalize_turn(
                                    tx,
                                    db,
                                    trace,
                                    turn_id,
                                    route_kind,
                                    persisted_trace_items,
                                    TurnErrorMessages {
                                        frontend_message: user_message,
                                        trace_message: error.to_string(),
                                    },
                                )
                                .await;
                                return Err(error);
                            }
                        }
                    }
                    model_attempt::ModelAttemptProgress::Failed(failure) => {
                        let model_attempt::ModelAttemptFailure {
                            stage,
                            error: _error,
                            user_message: _user_message,
                            trace_message: provider_trace_message,
                            accepted: _accepted,
                            timing: _timing,
                        } = failure;
                        let trace_message = format!(
                            "reasoning_replay_safe_restart_failed: provider={}, model={}, stage={stage:?}, error={provider_trace_message}",
                            self.provider.name(),
                            model
                        );
                        append_developer_persisted_trace_status(
                            persisted_trace_items,
                            &trace_message,
                            "error",
                        );
                        emit_error_and_finalize_turn(
                            tx,
                            db,
                            trace,
                            turn_id,
                            route_kind,
                            persisted_trace_items,
                            TurnErrorMessages {
                                frontend_message: "The provider could not safely restart the incomplete tool step. No tools were executed.".to_string(),
                                trace_message: trace_message.clone(),
                            },
                        )
                        .await;
                        return Err(CoreError::Agent(trace_message));
                    }
                    model_attempt::ModelAttemptProgress::Cancelled(cancellation) => {
                        let model_attempt::ModelAttemptCancellation {
                            message,
                            accepted: _accepted,
                            timing: _timing,
                        } = cancellation;
                        let trace_message =
                            format!("reasoning_replay_safe_restart_cancelled: {message}");
                        append_developer_persisted_trace_status(
                            persisted_trace_items,
                            &trace_message,
                            "error",
                        );
                        emit_error_and_finalize_turn(
                            tx,
                            db,
                            trace,
                            turn_id,
                            route_kind,
                            persisted_trace_items,
                            TurnErrorMessages {
                                frontend_message: message.clone(),
                                trace_message,
                            },
                        )
                        .await;
                        return Err(CoreError::Cancelled(message));
                    }
                    unexpected => {
                        let progress_kind = match &unexpected {
                            model_attempt::ModelAttemptProgress::StreamOpened => "stream_opened",
                            model_attempt::ModelAttemptProgress::Provider(_) => "provider_output",
                            model_attempt::ModelAttemptProgress::StreamComplete { .. } => {
                                "stream_complete"
                            }
                            model_attempt::ModelAttemptProgress::InterruptedAfterVisibleOutput(
                                _,
                            ) => "interrupted_after_visible_output",
                            model_attempt::ModelAttemptProgress::Completion(_) => "completion",
                            model_attempt::ModelAttemptProgress::NeedsContextCompaction(_) => {
                                "context_compaction"
                            }
                            model_attempt::ModelAttemptProgress::Failed(_) => "failed",
                            model_attempt::ModelAttemptProgress::Cancelled(_) => "cancelled",
                        };
                        let trace_message = format!(
                            "reasoning_replay_safe_restart_unexpected_progress: {progress_kind}"
                        );
                        append_developer_persisted_trace_status(
                            persisted_trace_items,
                            &trace_message,
                            "error",
                        );
                        emit_error_and_finalize_turn(
                            tx,
                            db,
                            trace,
                            turn_id,
                            route_kind,
                            persisted_trace_items,
                            TurnErrorMessages {
                                frontend_message: "The provider returned an invalid state while preparing a replay-safe tool step. No tools were executed.".to_string(),
                                trace_message: trace_message.clone(),
                            },
                        )
                        .await;
                        return Err(CoreError::Internal(trace_message));
                    }
                }
            };

            let model_attempt::ModelAttemptCompletion {
                response,
                accepted: safe_accepted,
                timing: safe_timing,
                switched_to_non_streaming: _switched_to_non_streaming,
            } = safe_completion;
            let safe_route = safe_accepted.route_snapshot.clone();
            if !required_route.same_route_identity(&safe_route) {
                let route_changed_sample = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                    Uuid::new_v4().to_string(),
                    safe_accepted.sample_id.clone(),
                    safe_route.clone(),
                    response.content.clone(),
                    response.thinking.as_deref(),
                    response.thinking.as_deref(),
                    response.tool_calls.clone().unwrap_or_default(),
                    false,
                );
                self.persist_provider_sample_without_message(
                    db,
                    conversation_id,
                    turn_id,
                    &route_changed_sample,
                )?;
                let trace_message = format!(
                    "reasoning_replay_safe_restart_route_changed: from={}:{} to={}:{}",
                    required_route.provider_family,
                    required_route.api_style_id(),
                    safe_route.provider_family,
                    safe_route.api_style_id()
                );
                emit_error_and_finalize_turn(
                    tx,
                    db,
                    trace,
                    turn_id,
                    route_kind,
                    persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: "The provider could not safely restart the tool step on the same route. No tools were executed.".to_string(),
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                return Err(CoreError::Agent(trace_message));
            }
            let safe_tool_calls = response.tool_calls.as_deref().unwrap_or_default();
            let safe_payload = crate::llm::provider_turn::ProviderReplayPayload::capture(
                &safe_route,
                response.thinking.as_deref(),
                safe_tool_calls,
            );
            if !safe_tool_calls.is_empty()
                && !safe_route
                    .replay_policy
                    .authorizes_tool_call(safe_payload.is_present())
            {
                let rejected_safe_restart =
                    crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                        Uuid::new_v4().to_string(),
                        safe_accepted.sample_id.clone(),
                        safe_route.clone(),
                        response.content.clone(),
                        response.thinking.as_deref(),
                        None,
                        safe_tool_calls.to_vec(),
                        false,
                    );
                self.persist_provider_sample_without_message(
                    db,
                    conversation_id,
                    turn_id,
                    &rejected_safe_restart,
                )?;
                let trace_message = format!(
                    "reasoning_replay_safe_restart_payload_missing: provider={}, model={}, api_style={}",
                    safe_route.provider_family,
                    safe_route.model_id,
                    safe_route.api_style_id()
                );
                emit_error_and_finalize_turn(
                    tx,
                    db,
                    trace,
                    turn_id,
                    route_kind,
                    persisted_trace_items,
                    TurnErrorMessages {
                        frontend_message: "The provider's safe restart was still missing mandatory signed replay state. No tools were executed.".to_string(),
                        trace_message: trace_message.clone(),
                    },
                )
                .await;
                return Err(CoreError::Agent(trace_message));
            }
            if safe_accepted.replay_projection_omitted_units > 0 {
                append_developer_persisted_trace_status(
                    persisted_trace_items,
                    &format!(
                        "provider_replay_boundary: omitted_units={}, provider={}, api_style={}",
                        safe_accepted.replay_projection_omitted_units,
                        safe_route.provider_family,
                        safe_route.api_style_id()
                    ),
                    "warning",
                );
            }
            accepted_sample_id = safe_accepted.sample_id;
            accepted_route_snapshot = safe_route;
            attempt_timing = safe_timing;
            if completion_has_semantic_output(&response) {
                *context_recovery_attempts = 0;
            }
            *reasoning_disabled_for_tool_loop = true;
            let reset_reason = "The provider omitted required replay state, so Nexa safely restarted the same route with reasoning disabled before any tool ran.".to_string();

            let recovered_tool_calls = response.tool_calls.unwrap_or_default();
            let recovered_thinking =
                crate::llm::reasoning_replay::sanitize_reasoning_text(response.thinking.as_deref());
            let _ = tx
                .send(AgentEvent::StreamReset {
                    reason: reset_reason,
                })
                .await;
            accumulated_content.truncate(accumulated_len_before_iteration);
            full_content = response.content;
            answer_delta_seen = !full_content.is_empty();
            accumulated_content.push_str(&full_content);
            iteration_thinking = recovered_thinking.unwrap_or_default();
            thinking_delta_seen = !iteration_thinking.is_empty();
            tool_calls = recovered_tool_calls;
            preparing_call_ids.clear();
            started_call_ids.clear();
            tool_run_started_ids.clear();
            chunk_usage = Some(response.usage);
            finish_reason = Some(response.finish_reason);
            if !iteration_thinking.is_empty() {
                let _ = tx
                    .send(AgentEvent::Thinking {
                        content: iteration_thinking.clone(),
                    })
                    .await;
            }
            if !full_content.is_empty() {
                let _ = tx
                    .send(AgentEvent::TextDelta {
                        delta: full_content.clone(),
                    })
                    .await;
            }
        }

        let prompt_cache_observation =
            self.complete_prompt_cache_observation(chunk_usage.as_ref(), None);

        Ok(ModelStepOutcome::Completed(Box::new(ModelStepOutput {
            full_content,
            tool_calls,
            chunk_usage,
            iteration_thinking,
            answer_delta_seen,
            thinking_delta_seen,
            finish_reason,
            started_call_ids,
            tool_run_started_ids,
            prompt_cache_observation,
            request_latency_ms: attempt_timing.request_latency_ms,
            time_to_first_token_ms: attempt_timing.time_to_first_token_ms,
            sample_id: accepted_sample_id,
            route_snapshot: accepted_route_snapshot,
            reasoning_was_requested,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_sample_without_reasoning_state_does_not_require_replay() {
        let tool_call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"README.md"}"#.to_string(),
            thought_signature: None,
        };

        assert_eq!(
            effective_sample_replay_policy(
                ReasoningReplayPolicy::Unknown,
                "",
                &crate::llm::provider_turn::ProviderReplayPayload::None,
                std::slice::from_ref(&tool_call),
            ),
            ReasoningReplayPolicy::NotRequired
        );
        assert_eq!(
            effective_sample_replay_policy(
                ReasoningReplayPolicy::Unknown,
                "visible reasoning",
                &crate::llm::provider_turn::ProviderReplayPayload::None,
                std::slice::from_ref(&tool_call),
            ),
            ReasoningReplayPolicy::Unknown
        );

        let signed_call = ToolCallRequest {
            thought_signature: Some("unverified-signature".to_string()),
            ..tool_call
        };
        assert_eq!(
            effective_sample_replay_policy(
                ReasoningReplayPolicy::Forbidden,
                "",
                &crate::llm::provider_turn::ProviderReplayPayload::None,
                &[signed_call],
            ),
            ReasoningReplayPolicy::Forbidden
        );
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn native_search_plan_is_validated_only_for_an_exposed_search_tool() {
        let unsupported = crate::llm::native_search::NativeSearchPlan {
            mode: crate::llm::native_search::SearchExecutionMode::ProviderNative,
            ..crate::llm::native_search::NativeSearchPlan::default()
        };
        let hidden = request_tools_with_native_search_plan(&[tool("read_file")], unsupported)
            .expect("ordinary routes must not validate a hidden search capability");
        assert!(!hidden
            .iter()
            .any(crate::llm::native_search::is_native_marker));
        assert_eq!(hidden[0].name, "read_file");

        let error = request_tools_with_native_search_plan(
            &[tool(crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL)],
            unsupported,
        )
        .expect_err("an exposed provider-native search must remain fail-closed");
        assert!(error.to_string().contains("unavailable"));

        let supported = crate::llm::native_search::NativeSearchPlan {
            mode: crate::llm::native_search::SearchExecutionMode::ProviderNative,
            dialect: Some(crate::llm::native_search::NativeSearchDialect::OpenAiResponses),
            capability: Some(crate::model_catalog::NativeWebSearchCapability {
                dialect: crate::llm::native_search::NativeSearchDialect::OpenAiResponses,
                supports_domains: true,
                supports_recency: false,
                supports_locale: false,
                supports_location: true,
                supports_citations: true,
                supports_stream_events: true,
                can_mix_client_tools: true,
            }),
            trusted_endpoint: true,
        };
        let visible = request_tools_with_native_search_plan(
            &[tool(crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL)],
            supported,
        )
        .expect("trusted exposed search");
        assert!(visible
            .iter()
            .any(crate::llm::native_search::is_native_marker));
    }
}
