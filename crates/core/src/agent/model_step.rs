//! One model sampling step: request construction, streaming, recovery, and stream-time steering.

use super::steering::SteeringDrainContext;
use super::*;

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
}

pub(super) enum ModelStepOutcome {
    Completed(Box<ModelStepOutput>),
    Restart,
}

pub(super) struct ModelStepOutput {
    pub(super) full_content: String,
    pub(super) tool_calls: Vec<ToolCallRequest>,
    pub(super) chunk_usage: Option<Usage>,
    pub(super) iteration_thinking: String,
    pub(super) last_finish_reason: Option<String>,
    pub(super) started_call_ids: HashSet<String>,
    pub(super) tool_run_started_ids: HashSet<String>,
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
        } = ctx;

        // -- 4a. Stream LLM response (with rate-limit retry) ----------------
        let stream_recovery_policy = StreamRecoveryPolicy::default();
        let current_request = CompletionRequest {
            model: model.to_string(),
            messages: (*messages).clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools: if tool_defs.is_empty() {
                None
            } else {
                Some((*tool_defs).clone())
            },
            stop: None,
            thinking_budget: if self.config.reasoning_enabled.unwrap_or(false) {
                Some(self.config.thinking_budget.unwrap_or(10_000))
            } else {
                None
            },
            reasoning_effort: if self.config.reasoning_enabled.unwrap_or(false) {
                self.config.reasoning_effort.clone()
            } else {
                None
            },
            provider_type: self.config.provider_type,
            parallel_tool_calls: true,
        };
        let accumulated_len_before_iteration = accumulated_content.len();
        let mut sampling_retries = 0u32;
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        let mut chunk_usage: Option<Usage> = None;
        let mut iteration_thinking = String::new();
        let mut last_finish_reason: Option<String>;
        let mut preparing_call_ids: HashSet<String> = HashSet::new();
        let mut started_call_ids: HashSet<String> = HashSet::new();
        let mut tool_run_started_ids: HashSet<String> = HashSet::new();

        loop {
            let mut retry_count = 0u32;
            let mut stream: futures::stream::BoxStream<'_, ProviderStreamEvent> =
                if *force_non_streaming_llm {
                    info!("Initiating LLM completion in non-streaming mode");
                    match self.provider.complete(&current_request).await {
                        Ok(response) => {
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
                                        thinking_message,
                                    } => {
                                        retry_count = attempt;
                                        warn!(
                                            "Rate limited. Retry {} after {}s",
                                            retry_count,
                                            delay.as_secs()
                                        );
                                        let _ = tx
                                            .send(AgentEvent::Thinking {
                                                content: thinking_message,
                                            })
                                            .await;
                                        tokio::time::sleep(delay).await;
                                    }
                                    StreamConnectRetryDecision::GiveUp {
                                        user_message,
                                        trace_message,
                                    } => {
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
                                        thinking_message,
                                    } => {
                                        retry_count = attempt;
                                        warn!(
                                            "Transient error (retry {}): {}. Retrying after {}s",
                                            retry_count,
                                            msg,
                                            delay.as_secs()
                                        );
                                        let _ = tx
                                            .send(AgentEvent::Thinking {
                                                content: thinking_message,
                                            })
                                            .await;
                                        tokio::time::sleep(delay).await;
                                    }
                                    StreamConnectRetryDecision::GiveUp {
                                        user_message,
                                        trace_message,
                                    } => {
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
                                            .recover_context_overflow(messages, model, tx)
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
            last_finish_reason = None;
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
                        // Forward thinking deltas.
                        if let Some(ref thinking) = chunk.thinking_delta {
                            if !thinking.is_empty() {
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
                            last_finish_reason = Some(format!("{:?}", fr).to_lowercase());
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
                if !full_content.trim().is_empty() {
                    let draft_reasoning =
                        self.reasoning_content_for_iteration(&iteration_thinking, false);
                    messages.push(Message {
                        role: Role::Assistant,
                        parts: vec![ContentPart::Text {
                            text: full_content.clone(),
                        }],
                        name: None,
                        tool_calls: None,
                        reasoning_content: draft_reasoning,
                    });
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
                    self.drain_steering_messages_from(messages, &mut steering_ctx, Some(steering))
                        .await
                };
                if steering_texts.is_empty() {
                    return Ok(ModelStepOutcome::Restart);
                }
                let reason = "Steering message received; restarting the model response.";
                let _ = tx
                    .send(AgentEvent::StreamReset {
                        reason: reason.to_string(),
                    })
                    .await;
                accumulated_content.truncate(accumulated_len_before_iteration);
                self.expand_tool_defs_for_steering(tool_defs, &steering_texts, has_sources);
                append_persisted_trace_status(
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
                *messages = trim_to_context_window(
                    messages,
                    max_ctx.saturating_sub(context_safety_buffer(max_ctx)),
                    max_response_tokens,
                );
                return Ok(ModelStepOutcome::Restart);
            }

            if let Some(detail) = stream_incomplete_detail {
                match stream_recovery_policy.decide_after_incomplete(
                    *force_non_streaming_llm,
                    sampling_retries,
                    &detail,
                ) {
                    StreamRecoveryDecision::Reconnect {
                        attempt,
                        status_message,
                        reset_reason,
                        delay,
                    } => {
                        sampling_retries = attempt;
                        let _ = tx
                            .send(AgentEvent::Status {
                                content: status_message,
                                tone: Some("muted".to_string()),
                            })
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
                        status_message,
                        reset_reason,
                    } => {
                        let _ = tx
                            .send(AgentEvent::Status {
                                content: status_message,
                                tone: Some("muted".to_string()),
                            })
                            .await;

                        match self.provider.complete(&current_request).await {
                            Ok(response) => {
                                *force_non_streaming_llm = true;
                                let _ = tx
                                    .send(AgentEvent::StreamReset {
                                        reason: reset_reason,
                                    })
                                    .await;

                                accumulated_content.truncate(accumulated_len_before_iteration);
                                full_content = response.content;
                                accumulated_content.push_str(&full_content);
                                iteration_thinking = response.thinking.unwrap_or_default();
                                tool_calls = response.tool_calls.unwrap_or_default();
                                preparing_call_ids.clear();
                                started_call_ids.clear();
                                tool_run_started_ids.clear();
                                chunk_usage = Some(response.usage);
                                last_finish_reason =
                                    Some(format!("{:?}", response.finish_reason).to_lowercase());

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

        Ok(ModelStepOutcome::Completed(Box::new(ModelStepOutput {
            full_content,
            tool_calls,
            chunk_usage,
            iteration_thinking,
            last_finish_reason,
            started_call_ids,
            tool_run_started_ids,
        })))
    }
}
