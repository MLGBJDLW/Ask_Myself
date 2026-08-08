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

fn next_retry_at(delay: Duration) -> Option<String> {
    chrono::Duration::from_std(delay)
        .ok()
        .map(|delay| (chrono::Utc::now() + delay).to_rfc3339())
}

#[allow(clippy::too_many_arguments)]
fn connection_state_event(
    provider_id: &str,
    model_id: &str,
    state: ConnectionStateKind,
    error_category: Option<ConnectionErrorCategory>,
    attempt: u32,
    max_attempts: u32,
    delay: Option<Duration>,
    recoverable: bool,
) -> AgentEvent {
    AgentEvent::ConnectionState {
        state: ConnectionStateEvent {
            state,
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            error_category,
            attempt,
            max_attempts,
            next_retry_at: delay.and_then(next_retry_at),
            recoverable,
            queued_user_inputs: 0,
            turn_preserved: true,
        },
    }
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

impl AgentExecutor {
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

        // -- 4a. Stream LLM response (with rate-limit retry) ----------------
        let stream_recovery_policy = StreamRecoveryPolicy::default();
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
        let mut request_route_snapshot = self.provider.route_snapshot(&current_request);
        let reasoning_requested = current_request.reasoning_enabled == Some(true)
            || current_request.reasoning_effort.is_some()
            || current_request.thinking_budget.is_some();
        let may_call_tools = current_request
            .tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty());
        if reasoning_requested
            && may_call_tools
            && matches!(
                request_route_snapshot.replay_policy,
                ReasoningReplayPolicy::Unknown | ReasoningReplayPolicy::Forbidden
            )
        {
            current_request.reasoning_enabled = Some(false);
            current_request.reasoning_effort = None;
            current_request.thinking_budget = None;
            request_route_snapshot = self.provider.route_snapshot(&current_request);
            append_developer_persisted_trace_status(
                persisted_trace_items,
                "provider_replay_preflight: reasoning disabled before request because the selected tool route has no replay guarantee",
                "warning",
            );
        }
        let replay_projection = crate::llm::reasoning_replay::prepare_provider_replay_history(
            messages,
            &request_route_snapshot,
        );
        if replay_projection.omitted_units > 0 {
            append_developer_persisted_trace_status(
                persisted_trace_items,
                &format!(
                    "provider_replay_boundary: omitted_units={}, provider={}, api_style={}",
                    replay_projection.omitted_units,
                    request_route_snapshot.provider_family,
                    request_route_snapshot.api_style_id()
                ),
                "warning",
            );
        }
        current_request.messages = replay_projection.messages;
        let reasoning_was_requested = current_request.reasoning_enabled != Some(false)
            && current_request.reasoning_effort != Some(ReasoningEffort::None);
        let mut accepted_sample_id: String;
        let mut accepted_route_snapshot: crate::llm::provider_turn::RouteSnapshot;
        self.begin_prompt_cache_observation(model, messages, tool_defs);
        let request_started_at = std::time::Instant::now();
        let mut time_to_first_token_ms = None;
        let accumulated_len_before_iteration = accumulated_content.len();
        let mut sampling_retries = 0u32;
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        let mut chunk_usage: Option<Usage> = None;
        let mut iteration_thinking = String::new();
        let mut answer_delta_seen: bool;
        let mut thinking_delta_seen: bool;
        let mut finish_reason: Option<FinishReason>;
        let mut preparing_call_ids: HashSet<String> = HashSet::new();
        let mut started_call_ids: HashSet<String> = HashSet::new();
        let mut tool_run_started_ids: HashSet<String> = HashSet::new();
        // A successful stream reconnect is established in the next sampling
        // pass, where the connect-local retry counter starts at zero again.
        // Preserve the disconnect attempt across that boundary so the UI can
        // always close a previously emitted `Reconnecting` state.
        let mut pending_stream_recovery_attempt = None;

        loop {
            let mut retry_count = 0u32;
            let mut stream: futures::stream::BoxStream<'_, ProviderStreamEvent> =
                if *force_non_streaming_llm {
                    info!("Initiating LLM completion in non-streaming mode");
                    let candidate_sample_id = Uuid::new_v4().to_string();
                    match self.provider.complete(&current_request).await {
                        Ok(response) => {
                            accepted_sample_id = candidate_sample_id;
                            accepted_route_snapshot =
                                self.provider.route_snapshot(&current_request);
                            let _ = tx
                                .send(AgentEvent::ControllerStatus {
                                    code: "provider_connected".to_string(),
                                    content: "Provider connection established".to_string(),
                                    tone: None,
                                })
                                .await;
                            *context_recovery_attempts = 0;
                            stream_chunks_to_provider_events(completion_response_to_agent_stream(
                                response,
                            ))
                        }
                        Err(e) => {
                            emit_error_and_finalize_turn(
                                tx,
                                db,
                                trace,
                                turn_id,
                                route_kind,
                                persisted_trace_items,
                                TurnErrorMessages {
                                    frontend_message: e.to_string(),
                                    trace_message: e.to_string(),
                                },
                            )
                            .await;
                            return Err(e);
                        }
                    }
                } else {
                    loop {
                        info!("Initiating LLM stream, attempt {}", retry_count + 1);
                        let candidate_sample_id = Uuid::new_v4().to_string();
                        match self.provider.stream_events(&current_request).await {
                            Ok(s) => {
                                accepted_sample_id = candidate_sample_id;
                                accepted_route_snapshot =
                                    self.provider.route_snapshot(&current_request);
                                info!("LLM stream connected");
                                let _ = tx
                                    .send(AgentEvent::ControllerStatus {
                                        code: "provider_connected".to_string(),
                                        content: "Provider connection established".to_string(),
                                        tone: None,
                                    })
                                    .await;
                                let recovered_attempt = pending_stream_recovery_attempt
                                    .take()
                                    .map(|attempt| {
                                        (attempt, stream_recovery_policy.max_disconnect_retries())
                                    })
                                    .or_else(|| {
                                        (retry_count > 0).then_some((
                                            retry_count,
                                            stream_recovery_policy.max_connect_retries(),
                                        ))
                                    });
                                if let Some((attempt, max_attempts)) = recovered_attempt {
                                    let _ = tx
                                        .send(connection_state_event(
                                            self.provider.name(),
                                            model,
                                            ConnectionStateKind::Recovered,
                                            None,
                                            attempt,
                                            max_attempts,
                                            None,
                                            false,
                                        ))
                                        .await;
                                }
                                *context_recovery_attempts = 0;
                                break s;
                            }
                            Err(CoreError::RateLimited { retry_after_secs }) => {
                                match stream_recovery_policy
                                    .decide_after_rate_limit(retry_count, retry_after_secs)
                                {
                                    StreamConnectRetryDecision::Retry {
                                        attempt,
                                        delay,
                                        status_message: _,
                                    } => {
                                        retry_count = attempt;
                                        warn!(
                                            "Rate limited. Retry {} after {}s",
                                            retry_count,
                                            delay.as_secs()
                                        );
                                        let _ = tx
                                            .send(connection_state_event(
                                                self.provider.name(),
                                                model,
                                                ConnectionStateKind::Reconnecting,
                                                Some(ConnectionErrorCategory::RateLimit),
                                                retry_count,
                                                stream_recovery_policy.max_connect_retries(),
                                                Some(delay),
                                                true,
                                            ))
                                            .await;
                                        tokio::time::sleep(delay).await;
                                    }
                                    StreamConnectRetryDecision::GiveUp {
                                        user_message,
                                        trace_message,
                                    } => {
                                        let _ = tx
                                            .send(connection_state_event(
                                                self.provider.name(),
                                                model,
                                                ConnectionStateKind::Failed,
                                                Some(ConnectionErrorCategory::RateLimit),
                                                retry_count,
                                                stream_recovery_policy.max_connect_retries(),
                                                None,
                                                false,
                                            ))
                                            .await;
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
                                        return Err(CoreError::RateLimited { retry_after_secs });
                                    }
                                }
                            }
                            Err(CoreError::TransientLlm(msg)) => {
                                match stream_recovery_policy
                                    .decide_after_transient_error(retry_count, &msg)
                                {
                                    StreamConnectRetryDecision::Retry {
                                        attempt,
                                        delay,
                                        status_message: _,
                                    } => {
                                        retry_count = attempt;
                                        warn!(
                                            "Transient error (retry {}): {}. Retrying after {}s",
                                            retry_count,
                                            msg,
                                            delay.as_secs()
                                        );
                                        let _ = tx
                                            .send(connection_state_event(
                                                self.provider.name(),
                                                model,
                                                ConnectionStateKind::Reconnecting,
                                                Some(ConnectionErrorCategory::Network),
                                                retry_count,
                                                stream_recovery_policy.max_connect_retries(),
                                                Some(delay),
                                                true,
                                            ))
                                            .await;
                                        tokio::time::sleep(delay).await;
                                    }
                                    StreamConnectRetryDecision::GiveUp {
                                        user_message,
                                        trace_message,
                                    } => {
                                        let _ = tx
                                            .send(connection_state_event(
                                                self.provider.name(),
                                                model,
                                                ConnectionStateKind::Failed,
                                                Some(ConnectionErrorCategory::Network),
                                                retry_count,
                                                stream_recovery_policy.max_connect_retries(),
                                                None,
                                                false,
                                            ))
                                            .await;
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
                                        return Err(CoreError::Llm(trace_message));
                                    }
                                }
                            }
                            Err(e) if StreamRecoveryPolicy::is_context_overflow_error(&e) => {
                                match stream_recovery_policy
                                    .decide_after_context_overflow(*context_recovery_attempts, &e)
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
                                                            "Context overflow could not be reduced further: {}",
                                                            e
                                                        ),
                                                        trace_message: e.to_string(),
                                                    },
                                                )
                                                .await;
                                            return Err(e);
                                        }
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
                                                trace_message: e.to_string(),
                                            },
                                        )
                                        .await;
                                        return Err(e);
                                    }
                                }
                            }
                            Err(e) => {
                                emit_error_and_finalize_turn(
                                    tx,
                                    db,
                                    trace,
                                    turn_id,
                                    route_kind,
                                    persisted_trace_items,
                                    TurnErrorMessages {
                                        frontend_message: e.to_string(),
                                        trace_message: e.to_string(),
                                    },
                                )
                                .await;
                                return Err(e);
                            }
                        }
                    }
                };

            full_content.clear();
            tool_calls.clear();
            if sampling_retries > 0 {
                chunk_usage = None;
            }
            iteration_thinking.clear();
            answer_delta_seen = false;
            thinking_delta_seen = false;
            finish_reason = None;
            preparing_call_ids.clear();
            started_call_ids.clear();
            tool_run_started_ids.clear();
            let mut chunk_count: usize = 0;
            let mut stream_incomplete_detail: Option<String> = None;
            let mut stream_interrupted_by_steering: Option<AgentSteeringMessage> = None;
            let mut steering_closed = false;

            enum StreamLoopEvent {
                Steering(Option<AgentSteeringMessage>),
                Provider(Option<ProviderStreamEvent>),
            }

            loop {
                let stream_event = tokio::select! {
                    maybe_steering = self.wait_for_steering_message(), if self.steering_rx.is_some() && !steering_closed => {
                        StreamLoopEvent::Steering(maybe_steering)
                    }
                    maybe_provider_event = stream.next() => StreamLoopEvent::Provider(maybe_provider_event),
                };

                match stream_event {
                    StreamLoopEvent::Steering(Some(steering)) => {
                        stream_interrupted_by_steering = Some(steering);
                        break;
                    }
                    StreamLoopEvent::Steering(None) => {
                        steering_closed = true;
                    }
                    StreamLoopEvent::Provider(None) => break,
                    StreamLoopEvent::Provider(Some(ProviderStreamEvent::Chunk { chunk })) => {
                        chunk_count += 1;
                        if time_to_first_token_ms.is_none()
                            && (!chunk.delta.is_empty()
                                || chunk
                                    .thinking_delta
                                    .as_deref()
                                    .is_some_and(|value| !value.is_empty())
                                || chunk.tool_call_delta.is_some())
                        {
                            time_to_first_token_ms = Some(
                                u64::try_from(request_started_at.elapsed().as_millis())
                                    .unwrap_or(u64::MAX),
                            );
                        }
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
                            if let Some((tc_index, tc)) =
                                resolve_delta_target(&tool_calls, tc_delta)
                            {
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
                    StreamLoopEvent::Provider(Some(ProviderStreamEvent::RecoverableError {
                        message: detail,
                    })) => {
                        warn!("Stream incomplete — response may be truncated ({detail})");
                        info!(
                            "Stream ended incomplete: {chunk_count} chunks, {} chars — {detail}",
                            full_content.len()
                        );
                        stream_incomplete_detail = Some(detail);
                        break;
                    }
                    StreamLoopEvent::Provider(Some(ProviderStreamEvent::Cancelled { message })) => {
                        warn!("LLM stream cancelled: {message}");
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
                    StreamLoopEvent::Provider(Some(ProviderStreamEvent::TerminalError {
                        message,
                    })) => {
                        let e = CoreError::Llm(message);
                        error!("LLM stream error: {e}");
                        emit_error_and_finalize_turn(
                            tx,
                            db,
                            trace,
                            turn_id,
                            route_kind,
                            persisted_trace_items,
                            TurnErrorMessages {
                                frontend_message: e.to_string(),
                                trace_message: e.to_string(),
                            },
                        )
                        .await;
                        return Err(e);
                    }
                }
            }

            info!(
                "Stream complete: {chunk_count} chunks, {} chars",
                full_content.len()
            );

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
                    let mut draft_envelope =
                        crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                            Uuid::new_v4().to_string(),
                            accepted_sample_id.clone(),
                            accepted_route_snapshot.clone(),
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

            if let Some(detail) = stream_incomplete_detail {
                match stream_recovery_policy.decide_after_incomplete(
                    *force_non_streaming_llm,
                    sampling_retries,
                    &detail,
                ) {
                    StreamRecoveryDecision::Reconnect {
                        attempt,
                        status_message: _,
                        reset_reason,
                        delay,
                    } => {
                        sampling_retries = attempt;
                        pending_stream_recovery_attempt = Some(attempt);
                        let _ = tx
                            .send(connection_state_event(
                                self.provider.name(),
                                model,
                                ConnectionStateKind::Reconnecting,
                                Some(ConnectionErrorCategory::Network),
                                attempt,
                                stream_recovery_policy.max_disconnect_retries(),
                                Some(delay),
                                true,
                            ))
                            .await;
                        let _ = tx
                            .send(AgentEvent::StreamReset {
                                reason: reset_reason,
                            })
                            .await;
                        accumulated_content.truncate(accumulated_len_before_iteration);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    StreamRecoveryDecision::NonStreamingFallback {
                        status_message: _,
                        reset_reason,
                    } => {
                        let _ = tx
                            .send(connection_state_event(
                                self.provider.name(),
                                model,
                                ConnectionStateKind::Degraded,
                                Some(ConnectionErrorCategory::Network),
                                sampling_retries,
                                stream_recovery_policy.max_disconnect_retries(),
                                None,
                                true,
                            ))
                            .await;

                        let candidate_sample_id = Uuid::new_v4().to_string();
                        match self.provider.complete(&current_request).await {
                            Ok(response) => {
                                accepted_sample_id = candidate_sample_id;
                                accepted_route_snapshot =
                                    self.provider.route_snapshot(&current_request);
                                time_to_first_token_ms.get_or_insert_with(|| {
                                    u64::try_from(request_started_at.elapsed().as_millis())
                                        .unwrap_or(u64::MAX)
                                });
                                *force_non_streaming_llm = true;
                                let _ = tx
                                    .send(AgentEvent::StreamReset {
                                        reason: reset_reason,
                                    })
                                    .await;

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

                                let _ = tx
                                    .send(connection_state_event(
                                        self.provider.name(),
                                        model,
                                        ConnectionStateKind::Recovered,
                                        None,
                                        sampling_retries,
                                        stream_recovery_policy.max_disconnect_retries(),
                                        None,
                                        false,
                                    ))
                                    .await;
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
                            Err(err) => {
                                let _ = tx
                                    .send(connection_state_event(
                                        self.provider.name(),
                                        model,
                                        ConnectionStateKind::Failed,
                                        Some(ConnectionErrorCategory::Network),
                                        sampling_retries,
                                        stream_recovery_policy.max_disconnect_retries(),
                                        None,
                                        false,
                                    ))
                                    .await;
                                let message = format!(
                                    "Stream interrupted and non-streaming retry failed: {err}"
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
                                        trace_message: message,
                                    },
                                )
                                .await;
                                return Err(CoreError::StreamIncomplete(format!(
                                    "{detail}; fallback failed: {err}"
                                )));
                            }
                        }
                    }
                }
            }

            let current_accepted_route = &accepted_route_snapshot;
            let output_payload = crate::llm::provider_turn::ProviderReplayPayload::capture(
                current_accepted_route,
                Some(&iteration_thinking),
                &tool_calls,
            );
            let missing_required_tool_reasoning = current_accepted_route
                .replay_policy
                .requires_tool_call_payload()
                && !tool_calls.is_empty()
                && !output_payload.is_present();
            if missing_required_tool_reasoning {
                let rejected_stream_sample =
                    crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                        Uuid::new_v4().to_string(),
                        accepted_sample_id.clone(),
                        current_accepted_route.clone(),
                        full_content.clone(),
                        crate::llm::reasoning_replay::sanitize_reasoning_text(Some(
                            &iteration_thinking,
                        ))
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
                let (response, reset_reason) = {
                    let mut safe_request = current_request.clone();
                    safe_request.reasoning_enabled = Some(false);
                    safe_request.reasoning_effort = None;
                    safe_request.thinking_budget = None;
                    let safe_sample_id = Uuid::new_v4().to_string();
                    match self.provider.complete(&safe_request).await {
                        Ok(response) => {
                            let safe_route = self.provider.route_snapshot(&safe_request);
                            if !required_route.same_route_identity(&safe_route) {
                                let route_changed_sample =
                                    crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                                        Uuid::new_v4().to_string(),
                                        safe_sample_id,
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
                            let safe_tool_calls =
                                response.tool_calls.as_deref().unwrap_or_default();
                            let safe_payload =
                                crate::llm::provider_turn::ProviderReplayPayload::capture(
                                    &safe_route,
                                    response.thinking.as_deref(),
                                    safe_tool_calls,
                                );
                            if safe_route.replay_policy.requires_tool_call_payload()
                                && !safe_tool_calls.is_empty()
                                && !safe_payload.is_present()
                            {
                                let rejected_safe_restart =
                                    crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                                        Uuid::new_v4().to_string(),
                                        safe_sample_id,
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
                            accepted_sample_id = safe_sample_id;
                            accepted_route_snapshot = safe_route;
                            *reasoning_disabled_for_tool_loop = true;
                            let reset_reason = "The provider omitted required replay state, so Nexa safely restarted the same route with reasoning disabled before any tool ran.".to_string();
                            append_developer_persisted_trace_status(
                                persisted_trace_items,
                                "reasoning_replay_safe_restart: completed on the same route before tool dispatch",
                                "warning",
                            );
                            (response, reset_reason)
                        }
                        Err(error) => {
                            let trace_message = format!(
                                "reasoning_replay_safe_restart_failed: provider={}, model={}, error={error}",
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
                    }
                };

                let recovered_tool_calls = response.tool_calls.unwrap_or_default();
                let recovered_thinking = crate::llm::reasoning_replay::sanitize_reasoning_text(
                    response.thinking.as_deref(),
                );
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

            break;
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
            request_latency_ms: u64::try_from(request_started_at.elapsed().as_millis())
                .unwrap_or(u64::MAX),
            time_to_first_token_ms,
            sample_id: accepted_sample_id,
            route_snapshot: accepted_route_snapshot,
            reasoning_was_requested,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
