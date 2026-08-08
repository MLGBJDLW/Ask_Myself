//! One model sampling step: request construction, streaming, recovery, and stream-time steering.

use super::steering::SteeringDrainContext;
use super::*;
use crate::llm::FinishReason;

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
            force_answer_only,
        } = ctx;

        // -- 4a. Stream LLM response (with rate-limit retry) ----------------
        let stream_recovery_policy = StreamRecoveryPolicy::default();
        self.config.native_search_plan.validate()?;
        let mut request_tools = (*tool_defs).clone();
        if let Some(marker) = self.config.native_search_plan.marker() {
            request_tools
                .retain(|tool| tool.name != crate::llm::native_search::NATIVE_WEB_SEARCH_MARKER);
            request_tools.push(marker);
        }
        let current_request = CompletionRequest {
            model: model.to_string(),
            messages: (*messages).clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools: if request_tools.is_empty() {
                None
            } else {
                Some(request_tools)
            },
            stop: None,
            thinking_budget: if !force_answer_only && self.config.reasoning_enabled.unwrap_or(false)
            {
                self.config.thinking_budget
            } else {
                None
            },
            reasoning_enabled: if force_answer_only {
                Some(false)
            } else {
                self.config.reasoning_enabled
            },
            reasoning_effort: if !force_answer_only
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
                    match self.provider.complete(&current_request).await {
                        Ok(response) => {
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
                        match self.provider.stream_events(&current_request).await {
                            Ok(s) => {
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
                    let draft_message = Message {
                        role: Role::Assistant,
                        parts: vec![ContentPart::Text {
                            text: full_content.clone(),
                        }],
                        name: None,
                        tool_calls: None,
                        reasoning_content: draft_reasoning.clone(),
                        prompt_cache_hint: None,
                    };
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

                        match self.provider.complete(&current_request).await {
                            Ok(response) => {
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
        })))
    }
}
