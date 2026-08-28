//! One provider model attempt, including transport recovery.
//!
//! The module keeps the unprojected request as its source of truth. Every wire
//! invocation derives a fresh request from that source so a provider adapter
//! that selects a concrete fallback route can apply that route's replay
//! contract without inheriting a lossy projection made for an earlier route.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{stream::BoxStream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::events::{
    AgentEvent, ConnectionErrorCategory, ConnectionStateEvent, ConnectionStateKind,
};
use super::stream_recovery::{
    StreamConnectRetryDecision, StreamRecoveryDecision, StreamRecoveryPolicy,
};
use crate::error::CoreError;
use crate::llm::provider_turn::{ProviderReplayPayload, RouteSnapshot};
use crate::llm::{
    CompletionRequest, CompletionResponse, LlmProvider, ProviderHostedToolEvent,
    ProviderStreamEvent, ReplayHistoryProjection, StreamChunk,
};

/// Immutable provenance for the provider sample whose output was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcceptedModelAttempt {
    pub(super) sample_id: String,
    pub(super) route_snapshot: RouteSnapshot,
    /// Number of prior replay units excluded for this concrete accepted route.
    pub(super) replay_projection_omitted_units: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelAttemptTiming {
    pub(super) request_latency_ms: u64,
    pub(super) time_to_first_token_ms: Option<u64>,
}

#[derive(Debug)]
pub(super) struct ModelAttemptProviderEvent {
    pub(super) event: AcceptedProviderEvent,
    pub(super) accepted: AcceptedModelAttempt,
    pub(super) first_for_sample: bool,
}

/// Provider output accepted by the attempt. Transport failures are consumed
/// inside this module and cannot leak through this interface.
#[derive(Debug)]
pub(super) enum AcceptedProviderEvent {
    Chunk(Box<StreamChunk>),
    HostedTool(Box<ProviderHostedToolEvent>),
    ReplayState(Box<ProviderReplayPayload>),
}

#[derive(Debug)]
pub(super) struct ModelAttemptCompletion {
    pub(super) response: CompletionResponse,
    pub(super) accepted: AcceptedModelAttempt,
    pub(super) timing: ModelAttemptTiming,
    pub(super) switched_to_non_streaming: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelAttemptFailureStage {
    Connect,
    Stream,
    Completion,
    NonStreamingFallback,
}

#[derive(Debug)]
pub(super) struct ModelAttemptFailure {
    pub(super) stage: ModelAttemptFailureStage,
    pub(super) error: CoreError,
    pub(super) user_message: String,
    pub(super) trace_message: String,
    pub(super) accepted: Option<AcceptedModelAttempt>,
    pub(super) timing: ModelAttemptTiming,
}

#[derive(Debug)]
pub(super) struct ModelAttemptInterruption {
    pub(super) accepted: AcceptedModelAttempt,
    pub(super) user_message: String,
    pub(super) trace_message: String,
    pub(super) timing: ModelAttemptTiming,
}

#[derive(Debug)]
pub(super) struct ModelAttemptCancellation {
    pub(super) message: String,
    pub(super) accepted: Option<AcceptedModelAttempt>,
    pub(super) timing: ModelAttemptTiming,
}

#[derive(Debug)]
pub(super) struct ModelAttemptContextOverflow {
    pub(super) error: CoreError,
    pub(super) timing: ModelAttemptTiming,
}

/// Typed progress from one model-attempt seam.
///
/// `Provider` is the only variant the caller projects into answer, thinking,
/// or tool state. All transport retries and their user-visible connectivity
/// events have already been handled before a value is returned.
#[derive(Debug)]
pub(super) enum ModelAttemptProgress {
    /// The stream wire request is established. Callers may begin selecting
    /// `next()` against steering only after receiving this transition.
    StreamOpened,
    Provider(ModelAttemptProviderEvent),
    StreamComplete {
        accepted: AcceptedModelAttempt,
        timing: ModelAttemptTiming,
    },
    Completion(ModelAttemptCompletion),
    InterruptedAfterReplayBarrier(ModelAttemptInterruption),
    NeedsContextCompaction(ModelAttemptContextOverflow),
    Failed(ModelAttemptFailure),
    Cancelled(ModelAttemptCancellation),
}

type StreamOpenFuture<'provider> = Pin<
    Box<
        dyn Future<Output = Result<BoxStream<'provider, ProviderStreamEvent>, CoreError>>
            + Send
            + 'provider,
    >,
>;

type CompletionFuture<'provider> =
    Pin<Box<dyn Future<Output = Result<CompletionResponse, CoreError>> + Send + 'provider>>;

enum AttemptPhase<'provider> {
    ReadyToStream,
    OpeningStream(StreamOpenFuture<'provider>),
    WaitingToRetryStream(Pin<Box<tokio::time::Sleep>>),
    Streaming(BoxStream<'provider, ProviderStreamEvent>),
    ReadyToComplete {
        fallback_detail: Option<String>,
        switched_to_non_streaming: bool,
    },
    OpeningCompletion {
        future: CompletionFuture<'provider>,
        fallback_detail: Option<String>,
        switched_to_non_streaming: bool,
    },
    WaitingToRetryCompletion {
        sleep: Pin<Box<tokio::time::Sleep>>,
        fallback_detail: Option<String>,
        switched_to_non_streaming: bool,
    },
    Done,
}

enum ConnectErrorAction {
    Retry(Duration),
    Finish(Box<ModelAttemptFailure>),
}

#[derive(Debug, Clone, Copy)]
struct RecoverySignal {
    attempt: u32,
    max_attempts: u32,
}

struct ConnectionNotice {
    state: ConnectionStateKind,
    error_category: Option<ConnectionErrorCategory>,
    attempt: u32,
    max_attempts: u32,
    delay: Option<Duration>,
    recoverable: bool,
    accepted: Option<AcceptedModelAttempt>,
}

/// Deep transport seam for one logical model sample.
///
/// The provider may be an adapter (including automatic fallback); this module
/// depends only on [`LlmProvider`] and never inspects adapter internals.
pub(super) struct ModelAttempt<'provider, 'events> {
    provider: &'provider dyn LlmProvider,
    events: &'events mpsc::Sender<AgentEvent>,
    original_request: CompletionRequest,
    policy: StreamRecoveryPolicy,
    phase: AttemptPhase<'provider>,
    request_started_at: Instant,
    candidate_sample_id: Option<String>,
    accepted: Option<AcceptedModelAttempt>,
    sample_progress_seen: bool,
    replay_barrier_crossed: bool,
    disconnect_retries: u32,
    pending_recovery: Option<RecoverySignal>,
    time_to_first_token_ms: Option<u64>,
    initial_provider_id: String,
    initial_model_id: String,
    connect_retries: u32,
    candidate_projection_omitted_units: Option<usize>,
    pending_agent_events: VecDeque<AgentEvent>,
    pending_progress: Option<ModelAttemptProgress>,
    cancel_token: CancellationToken,
}

impl<'provider, 'events> ModelAttempt<'provider, 'events> {
    pub(super) fn new(
        provider: &'provider dyn LlmProvider,
        original_request: CompletionRequest,
        events: &'events mpsc::Sender<AgentEvent>,
        force_non_streaming: bool,
    ) -> Self {
        let initial_provider_id = provider.name().to_string();
        let initial_model_id = original_request.model.clone();
        let phase = if force_non_streaming {
            AttemptPhase::ReadyToComplete {
                fallback_detail: None,
                switched_to_non_streaming: false,
            }
        } else {
            AttemptPhase::ReadyToStream
        };
        Self {
            provider,
            events,
            original_request,
            policy: StreamRecoveryPolicy::default(),
            phase,
            request_started_at: Instant::now(),
            candidate_sample_id: None,
            accepted: None,
            sample_progress_seen: false,
            replay_barrier_crossed: false,
            disconnect_retries: 0,
            pending_recovery: None,
            time_to_first_token_ms: None,
            initial_provider_id,
            initial_model_id,
            connect_retries: 0,
            candidate_projection_omitted_units: None,
            pending_agent_events: VecDeque::new(),
            pending_progress: None,
            cancel_token: CancellationToken::new(),
        }
    }

    pub(super) fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = cancel_token;
        self
    }

    /// Advance the attempt until provider output, a caller reset point, or a
    /// terminal typed outcome is available.
    pub(super) async fn next(&mut self) -> ModelAttemptProgress {
        // A provider event already consumed into durable in-memory progress may
        // be delivered once even if cancellation arrived immediately after it.
        // This branch is synchronous inside `next_uncancelled`.
        if self.pending_progress.is_some() {
            return self.next_uncancelled().await;
        }
        let cancel_token = self.cancel_token.clone();
        if cancel_token.is_cancelled() {
            return self.finish_token_cancellation();
        }
        let progress = tokio::select! {
            // Cancellation wins before polling a merely-ready provider event.
            // If provider polling does consume an event, its handler records
            // and returns progress without another await, so it cannot be lost.
            biased;
            _ = cancel_token.cancelled() => None,
            progress = self.next_uncancelled() => Some(progress),
        };
        match progress {
            Some(progress) => progress,
            None => self.finish_token_cancellation(),
        }
    }

    async fn next_uncancelled(&mut self) -> ModelAttemptProgress {
        loop {
            if self.pending_progress.is_some() {
                // Accepted provider output is the semantic result of this
                // pull. Never await auxiliary connectivity delivery before
                // exposing it: a steering `select!` may cancel `next()` while
                // the bounded AgentEvent channel is full. Preserve the usual
                // event ordering opportunistically when capacity is available.
                self.try_flush_pending_agent_events();
                return self
                    .pending_progress
                    .take()
                    .expect("pending model-attempt progress remains until returned");
            }
            self.flush_pending_agent_events().await;

            match self.phase {
                AttemptPhase::ReadyToStream => {
                    self.begin_stream_open();
                }
                AttemptPhase::OpeningStream(_) => {
                    let result = match &mut self.phase {
                        AttemptPhase::OpeningStream(future) => future.as_mut().await,
                        _ => unreachable!("opening stream phase checked above"),
                    };
                    match result {
                        Ok(stream) => {
                            if self.connect_retries > 0 {
                                self.pending_recovery = Some(RecoverySignal {
                                    attempt: self.connect_retries,
                                    max_attempts: self.policy.max_connect_retries(),
                                });
                            }
                            // Opening a stream proves transport connectivity,
                            // not sample acceptance. Keep the pre-accept retry
                            // budget across streams that open successfully and
                            // then terminate before yielding provider output.
                            self.phase = AttemptPhase::Streaming(stream);
                            return ModelAttemptProgress::StreamOpened;
                        }
                        Err(error) => match self
                            .classify_connect_error(error, ModelAttemptFailureStage::Connect)
                        {
                            ConnectErrorAction::Retry(delay) => {
                                self.phase = AttemptPhase::WaitingToRetryStream(Box::pin(
                                    tokio::time::sleep(delay),
                                ));
                            }
                            ConnectErrorAction::Finish(failure) => {
                                self.phase = AttemptPhase::Done;
                                self.pending_progress =
                                    Some(progress_from_connect_failure(*failure));
                            }
                        },
                    }
                }
                AttemptPhase::WaitingToRetryStream(_) => {
                    match &mut self.phase {
                        AttemptPhase::WaitingToRetryStream(sleep) => sleep.as_mut().await,
                        _ => unreachable!("stream retry phase checked above"),
                    }
                    self.phase = AttemptPhase::ReadyToStream;
                }
                AttemptPhase::Streaming(_) => {
                    // Borrow the stream in place across the await. If a
                    // steering branch cancels `next()`, the stream remains in
                    // this phase and the next call resumes without data loss.
                    let event = match &mut self.phase {
                        AttemptPhase::Streaming(stream) => stream.next().await,
                        _ => unreachable!("streaming phase checked above"),
                    };
                    match event {
                        Some(ProviderStreamEvent::Chunk { chunk }) => {
                            let semantic_progress = chunk_is_visible(&chunk);
                            self.accept_provider_event(
                                AcceptedProviderEvent::Chunk(chunk),
                                semantic_progress,
                                false,
                            );
                        }
                        Some(ProviderStreamEvent::HostedTool { tool }) => {
                            self.accept_provider_event(
                                AcceptedProviderEvent::HostedTool(tool),
                                true,
                                true,
                            );
                        }
                        Some(ProviderStreamEvent::ReplayState { replay }) => {
                            self.accept_provider_event(
                                AcceptedProviderEvent::ReplayState(replay),
                                true,
                                false,
                            );
                        }
                        Some(ProviderStreamEvent::RecoverableError { message }) => {
                            self.recover_from_disconnect(message);
                        }
                        Some(ProviderStreamEvent::Cancelled { message }) => {
                            self.phase = AttemptPhase::Done;
                            return ModelAttemptProgress::Cancelled(ModelAttemptCancellation {
                                message,
                                accepted: self.accepted.clone(),
                                timing: self.timing(),
                            });
                        }
                        Some(ProviderStreamEvent::TerminalError { failure }) => {
                            let error = failure.into_core_error();
                            if self.replay_barrier_crossed {
                                // The side-effect barrier and provider error
                                // classification are independent. Suppress the
                                // replay, but preserve whether this was a rate
                                // limit, transient connection, or provider
                                // failure in the structured connection state.
                                self.interrupt_after_replay_barrier(error);
                            } else {
                                let resettable_progress_seen = self.sample_progress_seen;
                                let context_overflow =
                                    StreamRecoveryPolicy::is_context_overflow_error(&error);
                                match self
                                    .classify_connect_error(error, ModelAttemptFailureStage::Stream)
                                {
                                    ConnectErrorAction::Retry(delay) => {
                                        if resettable_progress_seen {
                                            self.pending_agent_events.push_back(
                                                AgentEvent::StreamReset {
                                                    reason: "The provider rejected an incomplete draft; Nexa discarded that sample before the typed retry."
                                                        .to_string(),
                                                    discard_sample: true,
                                                },
                                            );
                                        }
                                        self.discard_resettable_sample();
                                        self.phase = AttemptPhase::WaitingToRetryStream(Box::pin(
                                            tokio::time::sleep(delay),
                                        ));
                                    }
                                    ConnectErrorAction::Finish(failure) => {
                                        if resettable_progress_seen && context_overflow {
                                            self.pending_agent_events.push_back(
                                                AgentEvent::StreamReset {
                                                    reason: "The provider reported context overflow; Nexa discarded the incomplete draft before compaction."
                                                        .to_string(),
                                                    discard_sample: true,
                                                },
                                            );
                                            self.discard_resettable_sample();
                                        }
                                        self.phase = AttemptPhase::Done;
                                        self.pending_progress =
                                            Some(progress_from_connect_failure(*failure));
                                    }
                                }
                            }
                        }
                        None => {
                            // A clean empty stream remains a valid model sample
                            // for output recovery. Automatic fallback adapters
                            // convert their own no-output exhaustion into an
                            // explicit RecoverableError before this point.
                            if self.accepted.is_none() {
                                let accepted = self.capture_accepted_route();
                                self.accepted = Some(accepted.clone());
                            }
                            let accepted = self
                                .accepted
                                .clone()
                                .expect("clean stream completion binds empty-sample provenance");
                            if !self.sample_progress_seen {
                                let connect_retries = std::mem::take(&mut self.connect_retries);
                                self.queue_connected(&accepted, connect_retries);
                            }
                            self.phase = AttemptPhase::Done;
                            self.pending_progress = Some(ModelAttemptProgress::StreamComplete {
                                accepted,
                                timing: self.timing(),
                            });
                        }
                    }
                }
                AttemptPhase::ReadyToComplete {
                    ref fallback_detail,
                    switched_to_non_streaming,
                } => {
                    let fallback_detail = fallback_detail.clone();
                    self.begin_completion(fallback_detail, switched_to_non_streaming);
                }
                AttemptPhase::OpeningCompletion { .. } => {
                    let (result, fallback_detail, switched_to_non_streaming) = match &mut self.phase
                    {
                        AttemptPhase::OpeningCompletion {
                            future,
                            fallback_detail,
                            switched_to_non_streaming,
                        } => (
                            future.as_mut().await,
                            fallback_detail.clone(),
                            *switched_to_non_streaming,
                        ),
                        _ => unreachable!("opening completion phase checked above"),
                    };
                    match result {
                        Ok(response) => {
                            self.time_to_first_token_ms
                                .get_or_insert_with(|| elapsed_millis(self.request_started_at));
                            let accepted = self.capture_accepted_route();
                            self.accepted = Some(accepted.clone());
                            let connect_retries = self.connect_retries;
                            self.connect_retries = 0;
                            self.phase = AttemptPhase::Done;
                            self.queue_connected(&accepted, connect_retries);
                            self.pending_progress =
                                Some(ModelAttemptProgress::Completion(ModelAttemptCompletion {
                                    response,
                                    accepted,
                                    timing: self.timing(),
                                    switched_to_non_streaming,
                                }));
                        }
                        Err(error) => match self
                            .classify_connect_error(error, ModelAttemptFailureStage::Completion)
                        {
                            ConnectErrorAction::Retry(delay) => {
                                self.phase = AttemptPhase::WaitingToRetryCompletion {
                                    sleep: Box::pin(tokio::time::sleep(delay)),
                                    fallback_detail,
                                    switched_to_non_streaming,
                                };
                            }
                            ConnectErrorAction::Finish(failure) => {
                                self.phase = AttemptPhase::Done;
                                let progress = progress_from_connect_failure(*failure);
                                self.pending_progress = Some(match progress {
                                    ModelAttemptProgress::Failed(mut failure) => {
                                        apply_non_streaming_fallback_failure(
                                            &mut failure,
                                            fallback_detail.as_deref(),
                                        );
                                        ModelAttemptProgress::Failed(failure)
                                    }
                                    progress => progress,
                                });
                            }
                        },
                    }
                }
                AttemptPhase::WaitingToRetryCompletion { .. } => {
                    let (fallback_detail, switched_to_non_streaming) = match &mut self.phase {
                        AttemptPhase::WaitingToRetryCompletion {
                            sleep,
                            fallback_detail,
                            switched_to_non_streaming,
                        } => {
                            sleep.as_mut().await;
                            (fallback_detail.clone(), *switched_to_non_streaming)
                        }
                        _ => unreachable!("completion retry phase checked above"),
                    };
                    self.phase = AttemptPhase::ReadyToComplete {
                        fallback_detail,
                        switched_to_non_streaming,
                    };
                }
                AttemptPhase::Done => {
                    return ModelAttemptProgress::Failed(self.failure(
                        ModelAttemptFailureStage::Stream,
                        CoreError::Internal(
                            "Model attempt was advanced after completion".to_string(),
                        ),
                        None,
                        None,
                    ));
                }
            }
        }
    }

    fn finish_token_cancellation(&mut self) -> ModelAttemptProgress {
        self.phase = AttemptPhase::Done;
        self.pending_progress = None;
        ModelAttemptProgress::Cancelled(ModelAttemptCancellation {
            message: "cancelled by user".to_string(),
            accepted: self.accepted.clone(),
            timing: self.timing(),
        })
    }

    /// Whether the attempt currently owns an established provider stream.
    /// Every in-flight wire future is retained in the phase state, so cancelling
    /// `next()` never discards and resends a physical provider request.
    pub(super) fn accepts_stream_steering(&self) -> bool {
        matches!(self.phase, AttemptPhase::Streaming(_))
    }

    fn begin_stream_open(&mut self) {
        let request = self.request_for_invocation();
        self.candidate_sample_id = Some(Uuid::new_v4().to_string());
        info!(attempt = self.connect_retries + 1, "Initiating LLM stream");
        let provider = self.provider;
        self.phase =
            AttemptPhase::OpeningStream(Box::pin(
                async move { provider.stream_events(&request).await },
            ));
    }

    fn begin_completion(
        &mut self,
        fallback_detail: Option<String>,
        switched_to_non_streaming: bool,
    ) {
        let request = self.request_for_invocation();
        self.candidate_sample_id = Some(Uuid::new_v4().to_string());
        info!(
            attempt = self.connect_retries + 1,
            "Initiating LLM completion"
        );
        let provider = self.provider;
        self.phase = AttemptPhase::OpeningCompletion {
            future: Box::pin(async move { provider.complete(&request).await }),
            fallback_detail,
            switched_to_non_streaming,
        };
    }

    fn classify_connect_error(
        &mut self,
        error: CoreError,
        stage: ModelAttemptFailureStage,
    ) -> ConnectErrorAction {
        match error {
            CoreError::RateLimited { retry_after_secs } => match self
                .policy
                .decide_after_rate_limit(self.connect_retries, retry_after_secs)
            {
                StreamConnectRetryDecision::Retry { attempt, delay, .. } => {
                    self.connect_retries = attempt;
                    self.queue_reconnecting(
                        ConnectionErrorCategory::RateLimit,
                        attempt,
                        self.policy.max_connect_retries(),
                        delay,
                    );
                    ConnectErrorAction::Retry(delay)
                }
                StreamConnectRetryDecision::GiveUp {
                    user_message,
                    trace_message,
                } => {
                    self.queue_failed(
                        ConnectionErrorCategory::RateLimit,
                        self.connect_retries,
                        self.policy.max_connect_retries(),
                    );
                    ConnectErrorAction::Finish(Box::new(self.failure(
                        stage,
                        CoreError::RateLimited { retry_after_secs },
                        Some(user_message),
                        Some(trace_message),
                    )))
                }
            },
            CoreError::TransientLlm(message) => match self
                .policy
                .decide_after_transient_error(self.connect_retries, &message)
            {
                StreamConnectRetryDecision::Retry { attempt, delay, .. } => {
                    self.connect_retries = attempt;
                    self.queue_reconnecting(
                        ConnectionErrorCategory::Network,
                        attempt,
                        self.policy.max_connect_retries(),
                        delay,
                    );
                    ConnectErrorAction::Retry(delay)
                }
                StreamConnectRetryDecision::GiveUp {
                    user_message,
                    trace_message,
                } => {
                    self.queue_failed(
                        ConnectionErrorCategory::Network,
                        self.connect_retries,
                        self.policy.max_connect_retries(),
                    );
                    let error = if stage == ModelAttemptFailureStage::Connect {
                        CoreError::Llm(trace_message.clone())
                    } else {
                        CoreError::TransientLlm(message)
                    };
                    ConnectErrorAction::Finish(Box::new(self.failure(
                        stage,
                        error,
                        Some(user_message),
                        Some(trace_message),
                    )))
                }
            },
            error => ConnectErrorAction::Finish(Box::new(self.failure(stage, error, None, None))),
        }
    }

    fn accept_provider_event(
        &mut self,
        event: AcceptedProviderEvent,
        semantic_progress: bool,
        crosses_replay_barrier: bool,
    ) {
        let first_for_sample = self.accepted.is_none();
        if first_for_sample {
            let accepted = self.capture_accepted_route();
            self.accepted = Some(accepted.clone());
        }
        if semantic_progress && !self.sample_progress_seen {
            let accepted = self
                .accepted
                .clone()
                .expect("visible provider output has accepted route provenance");
            let connect_retries = std::mem::take(&mut self.connect_retries);
            self.queue_connected(&accepted, connect_retries);
        }
        if semantic_progress {
            self.sample_progress_seen = true;
            self.time_to_first_token_ms
                .get_or_insert_with(|| elapsed_millis(self.request_started_at));
        }
        self.replay_barrier_crossed |= crosses_replay_barrier;
        self.pending_progress = Some(ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
            event,
            accepted: self
                .accepted
                .clone()
                .expect("accepted route is captured before provider output is returned"),
            first_for_sample,
        }));
    }

    fn recover_from_disconnect(&mut self, detail: String) {
        match self.policy.decide_after_incomplete(
            false,
            self.disconnect_retries,
            self.replay_barrier_crossed,
            &detail,
        ) {
            StreamRecoveryDecision::StopAfterReplayBarrier {
                user_message,
                trace_message,
            } => {
                self.queue_failed(
                    ConnectionErrorCategory::Network,
                    self.disconnect_retries,
                    self.policy.max_disconnect_retries(),
                );
                self.phase = AttemptPhase::Done;
                self.pending_progress = Some(ModelAttemptProgress::InterruptedAfterReplayBarrier(
                    ModelAttemptInterruption {
                        accepted: self
                            .accepted
                            .clone()
                            .expect("a replay barrier always has accepted route provenance"),
                        user_message,
                        trace_message,
                        timing: self.timing(),
                    },
                ));
            }
            StreamRecoveryDecision::Reconnect {
                attempt,
                reset_reason,
                delay,
                ..
            } => {
                self.disconnect_retries = attempt;
                self.pending_recovery = Some(RecoverySignal {
                    attempt,
                    max_attempts: self.policy.max_disconnect_retries(),
                });
                self.queue_reconnecting(
                    ConnectionErrorCategory::Network,
                    attempt,
                    self.policy.max_disconnect_retries(),
                    delay,
                );
                self.pending_agent_events
                    .push_back(AgentEvent::StreamReset {
                        reason: reset_reason.clone(),
                        discard_sample: true,
                    });
                self.discard_resettable_sample();
                self.connect_retries = 0;
                self.phase =
                    AttemptPhase::WaitingToRetryStream(Box::pin(tokio::time::sleep(delay)));
            }
            StreamRecoveryDecision::NonStreamingFallback { reset_reason, .. } => {
                self.queue_connection_state(ConnectionNotice {
                    state: ConnectionStateKind::Degraded,
                    error_category: Some(ConnectionErrorCategory::Network),
                    attempt: self.disconnect_retries,
                    max_attempts: self.policy.max_disconnect_retries(),
                    delay: None,
                    recoverable: true,
                    accepted: self.accepted.clone(),
                });
                self.pending_agent_events
                    .push_back(AgentEvent::StreamReset {
                        reason: reset_reason.clone(),
                        discard_sample: true,
                    });
                self.pending_recovery = Some(RecoverySignal {
                    attempt: self.disconnect_retries,
                    max_attempts: self.policy.max_disconnect_retries(),
                });
                self.discard_resettable_sample();
                self.connect_retries = 0;
                self.phase = AttemptPhase::ReadyToComplete {
                    fallback_detail: Some(detail),
                    switched_to_non_streaming: true,
                };
            }
        }
    }

    fn interrupt_after_replay_barrier(&mut self, error: CoreError) {
        let category = match &error {
            CoreError::RateLimited { .. } => ConnectionErrorCategory::RateLimit,
            CoreError::TransientLlm(_) => ConnectionErrorCategory::Network,
            CoreError::Llm(_) => ConnectionErrorCategory::ProviderUnavailable,
            _ => ConnectionErrorCategory::Unknown,
        };
        let detail = error.to_string();
        let StreamRecoveryDecision::StopAfterReplayBarrier {
            user_message,
            trace_message,
        } = self
            .policy
            .decide_after_incomplete(false, self.disconnect_retries, true, &detail)
        else {
            unreachable!("a replay barrier must always suppress transport replay")
        };
        self.queue_failed(
            category,
            self.disconnect_retries,
            self.policy.max_disconnect_retries(),
        );
        self.phase = AttemptPhase::Done;
        self.pending_progress = Some(ModelAttemptProgress::InterruptedAfterReplayBarrier(
            ModelAttemptInterruption {
                accepted: self
                    .accepted
                    .clone()
                    .expect("a replay barrier always has accepted route provenance"),
                user_message,
                trace_message,
                timing: self.timing(),
            },
        ));
    }

    /// Rebuild every physical invocation from the original history, then honor
    /// the provider's typed projection-ownership contract.
    fn request_for_invocation(&mut self) -> CompletionRequest {
        let mut request = self.original_request.clone();
        let history_policy = match self.provider.replay_history_projection(&request) {
            ReplayHistoryProjection::ProviderSelectedRoute => {
                self.candidate_projection_omitted_units = None;
                return request;
            }
            ReplayHistoryProjection::Caller(policy) => policy,
        };
        let mut route = self.provider.route_snapshot(&request);
        route.replay_policy = history_policy;
        let projection = crate::llm::reasoning_replay::prepare_provider_replay_history(
            &self.original_request.messages,
            &route,
        );
        self.candidate_projection_omitted_units = Some(projection.omitted_units);
        request.messages = projection.messages;
        request
    }

    /// This is deliberately the only post-invocation route query. Streaming
    /// callers reach it only after the provider has yielded its first accepted
    /// Chunk/HostedTool; completion callers reach it only after success.
    fn capture_accepted_route(&mut self) -> AcceptedModelAttempt {
        let route_snapshot = self.provider.route_snapshot(&self.original_request);
        let replay_projection_omitted_units = self
            .candidate_projection_omitted_units
            .take()
            .unwrap_or_else(|| {
                crate::llm::reasoning_replay::prepare_provider_replay_history(
                    &self.original_request.messages,
                    &route_snapshot,
                )
                .omitted_units
            });
        AcceptedModelAttempt {
            sample_id: self
                .candidate_sample_id
                .take()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            route_snapshot,
            replay_projection_omitted_units,
        }
    }

    fn discard_resettable_sample(&mut self) {
        self.candidate_sample_id = None;
        self.candidate_projection_omitted_units = None;
        self.accepted = None;
        self.sample_progress_seen = false;
        self.replay_barrier_crossed = false;
    }

    fn timing(&self) -> ModelAttemptTiming {
        ModelAttemptTiming {
            request_latency_ms: elapsed_millis(self.request_started_at),
            time_to_first_token_ms: self.time_to_first_token_ms,
        }
    }

    fn failure(
        &self,
        stage: ModelAttemptFailureStage,
        error: CoreError,
        user_message: Option<String>,
        trace_message: Option<String>,
    ) -> ModelAttemptFailure {
        let fallback = error.to_string();
        ModelAttemptFailure {
            stage,
            user_message: user_message.unwrap_or_else(|| fallback.clone()),
            trace_message: trace_message.unwrap_or(fallback),
            error,
            accepted: self.accepted.clone(),
            timing: self.timing(),
        }
    }

    fn queue_connected(&mut self, accepted: &AcceptedModelAttempt, connect_retries: u32) {
        self.pending_agent_events
            .push_back(AgentEvent::ControllerStatus {
                code: "provider_connected".to_string(),
                content: "Provider connection established".to_string(),
                tone: None,
            });
        let recovered = self.pending_recovery.take().or_else(|| {
            (connect_retries > 0).then_some(RecoverySignal {
                attempt: connect_retries,
                max_attempts: self.policy.max_connect_retries(),
            })
        });
        if let Some(recovery) = recovered {
            self.queue_connection_state(ConnectionNotice {
                state: ConnectionStateKind::Recovered,
                error_category: None,
                attempt: recovery.attempt,
                max_attempts: recovery.max_attempts,
                delay: None,
                recoverable: false,
                accepted: Some(accepted.clone()),
            });
        }
    }

    fn queue_reconnecting(
        &mut self,
        category: ConnectionErrorCategory,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
    ) {
        warn!(
            attempt,
            max_attempts,
            ?delay,
            "Provider transport retry scheduled"
        );
        self.queue_connection_state(ConnectionNotice {
            state: ConnectionStateKind::Reconnecting,
            error_category: Some(category),
            attempt,
            max_attempts,
            delay: Some(delay),
            recoverable: true,
            accepted: self.accepted.clone(),
        });
    }

    fn queue_failed(&mut self, category: ConnectionErrorCategory, attempt: u32, max_attempts: u32) {
        self.queue_connection_state(ConnectionNotice {
            state: ConnectionStateKind::Failed,
            error_category: Some(category),
            attempt,
            max_attempts,
            delay: None,
            recoverable: false,
            accepted: self.accepted.clone(),
        });
    }

    fn queue_connection_state(&mut self, notice: ConnectionNotice) {
        let (provider_id, model_id) = notice.accepted.as_ref().map_or_else(
            || {
                (
                    self.initial_provider_id.clone(),
                    self.initial_model_id.clone(),
                )
            },
            |accepted| {
                (
                    accepted.route_snapshot.provider_family.clone(),
                    accepted.route_snapshot.model_id.clone(),
                )
            },
        );
        self.pending_agent_events
            .push_back(AgentEvent::ConnectionState {
                state: ConnectionStateEvent {
                    state: notice.state,
                    provider_id,
                    model_id,
                    error_category: notice.error_category,
                    attempt: notice.attempt,
                    max_attempts: notice.max_attempts,
                    next_retry_at: notice.delay.and_then(next_retry_at),
                    recoverable: notice.recoverable,
                    queued_user_inputs: 0,
                    turn_preserved: true,
                },
            });
    }

    async fn flush_pending_agent_events(&mut self) {
        while let Some(event) = self.pending_agent_events.front().cloned() {
            let _ = self.events.send(event).await;
            self.pending_agent_events.pop_front();
        }
    }

    fn try_flush_pending_agent_events(&mut self) {
        while let Some(event) = self.pending_agent_events.front().cloned() {
            if self.events.try_send(event).is_err() {
                break;
            }
            self.pending_agent_events.pop_front();
        }
    }
}

fn progress_from_connect_failure(failure: ModelAttemptFailure) -> ModelAttemptProgress {
    let ModelAttemptFailure {
        stage,
        error,
        user_message,
        trace_message,
        accepted,
        timing,
    } = failure;
    match error {
        CoreError::Cancelled(message) => {
            ModelAttemptProgress::Cancelled(ModelAttemptCancellation {
                message,
                accepted,
                timing,
            })
        }
        error if StreamRecoveryPolicy::is_context_overflow_error(&error) => {
            ModelAttemptProgress::NeedsContextCompaction(ModelAttemptContextOverflow {
                error,
                timing,
            })
        }
        error => ModelAttemptProgress::Failed(ModelAttemptFailure {
            stage,
            error,
            user_message,
            trace_message,
            accepted,
            timing,
        }),
    }
}

fn apply_non_streaming_fallback_failure(
    failure: &mut ModelAttemptFailure,
    fallback_detail: Option<&str>,
) {
    let Some(detail) = fallback_detail else {
        return;
    };
    let fallback_error = format!(
        "Stream interrupted and non-streaming retry failed: {}",
        failure.error
    );
    failure.stage = ModelAttemptFailureStage::NonStreamingFallback;
    failure.user_message = fallback_error;
    failure.trace_message = format!("{detail}; fallback failed: {}", failure.error);
    failure.error = CoreError::StreamIncomplete(failure.trace_message.clone());
}

fn chunk_is_visible(chunk: &crate::llm::StreamChunk) -> bool {
    !chunk.delta.is_empty()
        || chunk
            .thinking_delta
            .as_deref()
            .is_some_and(|thinking| !thinking.is_empty())
        || chunk.tool_call_delta.is_some()
}

fn elapsed_millis(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn next_retry_at(delay: Duration) -> Option<String> {
    chrono::Duration::from_std(delay)
        .ok()
        .map(|delay| (chrono::Utc::now() + delay).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::task::Poll;

    use async_trait::async_trait;
    use futures::stream;

    use super::*;
    use crate::llm::fallback::{AutomaticFallbackCandidate, AutomaticFallbackProvider};
    use crate::llm::reasoning_profile::{ReasoningApiStyle, ReasoningReplayPolicy};
    use crate::llm::{
        FinishReason, Message, ProviderHostedToolEvent, ProviderHostedToolKind,
        ProviderHostedToolStatus, ProviderType, Role, StreamChunk, ToolCallRequest, Usage,
    };

    enum Invocation {
        Stream(Result<Vec<ProviderStreamEvent>, CoreError>),
        PendingStreamOpen,
        PendingStreamRead,
        StreamThenPending(Vec<ProviderStreamEvent>),
        RepeatingStreamChunks,
        Complete(Result<CompletionResponse, CoreError>),
        PendingComplete,
    }

    struct ScriptedProvider {
        name: &'static str,
        endpoint: &'static str,
        model: &'static str,
        replay_policy: ReasoningReplayPolicy,
        history_policy: ReasoningReplayPolicy,
        script: Mutex<VecDeque<Invocation>>,
        requests: Arc<Mutex<Vec<CompletionRequest>>>,
        route_queries: Arc<Mutex<usize>>,
    }

    impl ScriptedProvider {
        fn boxed(
            name: &'static str,
            endpoint: &'static str,
            model: &'static str,
            replay_policy: ReasoningReplayPolicy,
            script: Vec<Invocation>,
            requests: Arc<Mutex<Vec<CompletionRequest>>>,
            route_queries: Arc<Mutex<usize>>,
        ) -> Box<dyn LlmProvider> {
            Box::new(Self {
                name,
                endpoint,
                model,
                replay_policy,
                history_policy: replay_policy,
                script: Mutex::new(script.into()),
                requests,
                route_queries,
            })
        }

        fn route(&self, request: &CompletionRequest) -> RouteSnapshot {
            RouteSnapshot {
                provider_endpoint_id: self.endpoint.to_string(),
                provider_family: self.name.to_string(),
                api_style: ReasoningApiStyle::OpenAiResponses,
                model_id: request.model.clone(),
                reasoning_profile_id: format!("{}-profile", self.name),
                reasoning_profile_version: 1,
                replay_policy: if request.reasoning_enabled == Some(false)
                    || request.reasoning_effort == Some(crate::llm::ReasoningEffort::None)
                {
                    ReasoningReplayPolicy::NotRequired
                } else {
                    self.replay_policy
                },
            }
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
            self.replay_policy
        }

        fn reasoning_replay_history_policy(&self, _model: &str) -> ReasoningReplayPolicy {
            self.history_policy
        }

        fn route_snapshot(&self, request: &CompletionRequest) -> RouteSnapshot {
            *self.route_queries.lock().unwrap() += 1;
            self.route(request)
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(vec![self.model.to_string()])
        }

        async fn complete(
            &self,
            request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            self.requests.lock().unwrap().push(request.clone());
            let invocation = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted completion invocation");
            match invocation {
                Invocation::Complete(result) => result,
                Invocation::PendingComplete => std::future::pending().await,
                _ => panic!("expected scripted completion"),
            }
        }

        async fn stream_events(
            &self,
            request: &CompletionRequest,
        ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
            self.requests.lock().unwrap().push(request.clone());
            let invocation =
                self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
                    panic!("missing scripted stream invocation for {}", self.name)
                });
            match invocation {
                Invocation::Stream(result) => {
                    result.map(|events| Box::pin(stream::iter(events)) as BoxStream<'_, _>)
                }
                Invocation::PendingStreamOpen => std::future::pending().await,
                Invocation::PendingStreamRead => {
                    Ok(Box::pin(stream::pending()) as BoxStream<'_, _>)
                }
                Invocation::StreamThenPending(events) => {
                    Ok(Box::pin(stream::iter(events).chain(stream::pending())) as BoxStream<'_, _>)
                }
                Invocation::RepeatingStreamChunks => {
                    Ok(Box::pin(stream::repeat_with(|| ProviderStreamEvent::Chunk {
                        chunk: Box::new(StreamChunk {
                            delta: "repeat".to_string(),
                            tool_call_delta: None,
                            finish_reason: None,
                            usage: None,
                            thinking_delta: None,
                        }),
                    })) as BoxStream<'_, _>)
                }
                Invocation::Complete(_) | Invocation::PendingComplete => {
                    panic!("expected scripted stream")
                }
            }
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn chunk(text: &str) -> ProviderStreamEvent {
        ProviderStreamEvent::Chunk {
            chunk: Box::new(StreamChunk {
                delta: text.to_string(),
                tool_call_delta: None,
                finish_reason: None,
                usage: None,
                thinking_delta: None,
            }),
        }
    }

    fn response(text: &str) -> CompletionResponse {
        CompletionResponse {
            content: text.to_string(),
            tool_calls: None,
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
            thinking: None,
            provider_replay: None,
        }
    }

    fn request() -> CompletionRequest {
        CompletionRequest {
            model: "primary-model".to_string(),
            messages: vec![Message::text(Role::User, "answer")],
            ..CompletionRequest::default()
        }
    }

    fn event_channel() -> (mpsc::Sender<AgentEvent>, mpsc::Receiver<AgentEvent>) {
        mpsc::channel(64)
    }

    async fn expect_stream_opened(attempt: &mut ModelAttempt<'_, '_>) {
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::StreamOpened
        ));
        assert!(attempt.accepts_stream_steering());
    }

    #[tokio::test]
    async fn accepted_hosted_tool_is_ready_when_auxiliary_event_channel_is_full() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                ProviderStreamEvent::HostedTool {
                    tool: Box::new(ProviderHostedToolEvent {
                        call_id: "hosted-1".to_string(),
                        tool_name: "web_search".to_string(),
                        kind: ProviderHostedToolKind::WebSearch,
                        provider_id: "primary".to_string(),
                        status: ProviderHostedToolStatus::Running,
                        arguments: None,
                        content: None,
                        artifacts: None,
                    }),
                },
            ]))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = mpsc::channel(1);
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;
        tx.try_send(AgentEvent::Status {
            content: "occupy bounded channel".to_string(),
            tone: None,
        })
        .expect("fill auxiliary event channel");

        let mut next = Box::pin(attempt.next());
        let Poll::Ready(ModelAttemptProgress::Provider(accepted)) = futures::poll!(&mut next)
        else {
            panic!("accepted hosted tool must be observable before auxiliary event backpressure")
        };
        assert!(matches!(
            accepted.event,
            AcceptedProviderEvent::HostedTool(ref tool) if tool.call_id == "hosted-1"
        ));
        drop(next);

        assert!(matches!(rx.try_recv(), Ok(AgentEvent::Status { .. })));
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::StreamComplete { .. }
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentEvent::ControllerStatus { ref code, .. }) if code == "provider_connected"
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn connect_retries_do_not_accept_a_sample_before_provider_output() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let route_queries = Arc::new(Mutex::new(0));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Err(CoreError::TransientLlm("reset one".to_string()))),
                Invocation::Stream(Err(CoreError::TransientLlm("reset two".to_string()))),
                Invocation::Stream(Ok(vec![chunk("accepted")])),
            ],
            Arc::clone(&requests),
            Arc::clone(&route_queries),
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Provider(output) = attempt.next().await else {
            panic!("third connection should produce output")
        };
        assert_eq!(output.accepted.route_snapshot.provider_family, "primary");
        assert!(output.first_for_sample);
        assert_eq!(requests.lock().unwrap().len(), 3);
        assert_eq!(*route_queries.lock().unwrap(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_before_visible_output_reconnects_then_switches_to_completion() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let route_queries = Arc::new(Mutex::new(0));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Ok(vec![ProviderStreamEvent::RecoverableError {
                    message: "disconnect one".to_string(),
                }])),
                Invocation::Stream(Ok(vec![ProviderStreamEvent::RecoverableError {
                    message: "disconnect two".to_string(),
                }])),
                Invocation::Stream(Ok(vec![ProviderStreamEvent::RecoverableError {
                    message: "disconnect three".to_string(),
                }])),
                Invocation::Complete(Ok(response("fallback answer"))),
            ],
            Arc::clone(&requests),
            route_queries,
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        expect_stream_opened(&mut attempt).await;
        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Completion(completion) = attempt.next().await else {
            panic!("completion fallback should succeed")
        };
        assert_eq!(completion.response.content, "fallback answer");
        assert!(completion.switched_to_non_streaming);
        assert_eq!(requests.lock().unwrap().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn resettable_text_thinking_and_tool_deltas_can_reconnect_before_dispatch() {
        let resettable_events = vec![
            chunk("visible"),
            ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some("thinking".to_string()),
                }),
            },
            ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(crate::llm::ToolCallDelta {
                        id: "call-1".to_string(),
                        name: Some("read_file".to_string()),
                        arguments_delta: "{}".to_string(),
                        index: Some(0),
                        thought_signature: None,
                    }),
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }),
            },
        ];

        for resettable in resettable_events {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let provider = ScriptedProvider::boxed(
                "primary",
                "primary-endpoint",
                "primary-model",
                ReasoningReplayPolicy::NotRequired,
                vec![
                    Invocation::Stream(Ok(vec![
                        resettable,
                        ProviderStreamEvent::RecoverableError {
                            message: "late disconnect".to_string(),
                        },
                    ])),
                    Invocation::Stream(Ok(vec![chunk("recovered")])),
                ],
                Arc::clone(&requests),
                Arc::new(Mutex::new(0)),
            );
            let (tx, _rx) = event_channel();
            let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
            expect_stream_opened(&mut attempt).await;
            assert!(matches!(
                attempt.next().await,
                ModelAttemptProgress::Provider(_)
            ));
            expect_stream_opened(&mut attempt).await;
            assert!(matches!(
                attempt.next().await,
                ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                    event: AcceptedProviderEvent::Chunk(ref chunk),
                    ..
                }) if chunk.delta == "recovered"
            ));
            assert!(matches!(
                attempt.next().await,
                ModelAttemptProgress::StreamComplete { .. }
            ));
            assert_eq!(
                requests.lock().unwrap().len(),
                2,
                "resettable draft output should be replayed exactly once"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn provider_hosted_action_crosses_the_replay_barrier() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                ProviderStreamEvent::HostedTool {
                    tool: Box::new(ProviderHostedToolEvent {
                        call_id: "hosted-1".to_string(),
                        tool_name: "web_search".to_string(),
                        kind: ProviderHostedToolKind::WebSearch,
                        provider_id: "primary".to_string(),
                        status: ProviderHostedToolStatus::Running,
                        arguments: None,
                        content: None,
                        artifacts: None,
                    }),
                },
                ProviderStreamEvent::RecoverableError {
                    message: "late disconnect".to_string(),
                },
            ]))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(_)
        ));
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::InterruptedAfterReplayBarrier(_)
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_chunk_can_be_replayed_but_still_captures_then_replaces_provenance() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let route_queries = Arc::new(Mutex::new(0));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Ok(vec![
                    chunk(""),
                    ProviderStreamEvent::RecoverableError {
                        message: "disconnect".to_string(),
                    },
                ])),
                Invocation::Stream(Ok(vec![chunk("accepted")])),
            ],
            Arc::clone(&requests),
            Arc::clone(&route_queries),
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Provider(first) = attempt.next().await else {
            panic!("empty chunk is still an accepted provider event")
        };
        let first_sample_id = first.accepted.sample_id;
        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Provider(second) = attempt.next().await else {
            panic!("reconnected output")
        };
        assert_ne!(first_sample_id, second.accepted.sample_id);
        assert_eq!(*route_queries.lock().unwrap(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_reasoning_disable_preserves_safe_tool_history_for_concrete_route() {
        let request_with_safe_tool = |reasoning_enabled| {
            let route = RouteSnapshot {
                provider_endpoint_id: "primary-endpoint".to_string(),
                provider_family: "primary".to_string(),
                api_style: ReasoningApiStyle::OpenAiResponses,
                model_id: "primary-model".to_string(),
                reasoning_profile_id: "primary-profile".to_string(),
                reasoning_profile_version: 1,
                replay_policy: ReasoningReplayPolicy::NotRequired,
            };
            let tool_call = ToolCallRequest {
                id: "call-safe".to_string(),
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            };
            let mut assistant = Message::text(Role::Assistant, "");
            assistant.tool_calls = Some(vec![tool_call.clone()]);
            assistant.set_provider_turn(crate::llm::provider_turn::ProviderTurnEnvelope::capture(
                "turn-safe",
                "sample-safe",
                route,
                "",
                None,
                None,
                vec![tool_call],
                false,
            ));
            let mut request = request();
            request.reasoning_enabled = Some(reasoning_enabled);
            request.messages = vec![
                assistant,
                Message::text_with_name(Role::Tool, "safe result", "call-safe"),
                Message::text(Role::User, "continue"),
            ];
            request
        };

        let disabled_requests = Arc::new(Mutex::new(Vec::new()));
        let disabled = ScriptedProvider {
            name: "primary",
            endpoint: "primary-endpoint",
            model: "primary-model",
            replay_policy: ReasoningReplayPolicy::NotRequired,
            history_policy: ReasoningReplayPolicy::RequiredOnToolCall,
            script: Mutex::new(
                vec![Invocation::Stream(Ok(vec![chunk("completed after tool")]))].into(),
            ),
            requests: Arc::clone(&disabled_requests),
            route_queries: Arc::new(Mutex::new(0)),
        };
        let (disabled_tx, _disabled_rx) = event_channel();
        let mut disabled_attempt = ModelAttempt::new(
            &disabled,
            request_with_safe_tool(false),
            &disabled_tx,
            false,
        );

        expect_stream_opened(&mut disabled_attempt).await;
        let ModelAttemptProgress::Provider(disabled_output) = disabled_attempt.next().await else {
            panic!("reasoning-disabled route should see the safe tool result and complete")
        };

        assert!(matches!(
            disabled_output.event,
            AcceptedProviderEvent::Chunk(ref chunk) if chunk.delta == "completed after tool"
        ));
        assert_eq!(
            disabled_output.accepted.route_snapshot.replay_policy,
            ReasoningReplayPolicy::NotRequired
        );
        assert_eq!(disabled_output.accepted.replay_projection_omitted_units, 0);
        assert!(disabled_requests.lock().unwrap()[0]
            .messages
            .iter()
            .any(|message| message.role == Role::Tool));

        let normal_requests = Arc::new(Mutex::new(Vec::new()));
        let normal = ScriptedProvider {
            name: "primary",
            endpoint: "primary-endpoint",
            model: "primary-model",
            replay_policy: ReasoningReplayPolicy::NotRequired,
            history_policy: ReasoningReplayPolicy::RequiredOnToolCall,
            script: Mutex::new(vec![Invocation::Stream(Ok(vec![chunk("normal")]))].into()),
            requests: Arc::clone(&normal_requests),
            route_queries: Arc::new(Mutex::new(0)),
        };
        let (normal_tx, _normal_rx) = event_channel();
        let mut normal_attempt =
            ModelAttempt::new(&normal, request_with_safe_tool(true), &normal_tx, false);

        expect_stream_opened(&mut normal_attempt).await;
        let ModelAttemptProgress::Provider(normal_output) = normal_attempt.next().await else {
            panic!("normal request should still use its distinct history policy")
        };

        assert_eq!(normal_output.accepted.replay_projection_omitted_units, 1);
        assert!(!normal_requests.lock().unwrap()[0]
            .messages
            .iter()
            .any(|message| message.role == Role::Tool));
    }

    #[tokio::test(start_paused = true)]
    async fn fallback_route_provenance_and_history_are_captured_after_first_output() {
        let primary_requests = Arc::new(Mutex::new(Vec::new()));
        let fallback_requests = Arc::new(Mutex::new(Vec::new()));
        let primary_queries = Arc::new(Mutex::new(0));
        let fallback_queries = Arc::new(Mutex::new(0));
        let selections = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&selections);
        let wrapper = AutomaticFallbackProvider::new(
            0,
            ScriptedProvider::boxed(
                "primary",
                "primary-endpoint",
                "primary-model",
                ReasoningReplayPolicy::NotRequired,
                vec![Invocation::Stream(Ok(vec![
                    ProviderStreamEvent::RecoverableError {
                        message: "primary unavailable".to_string(),
                    },
                ]))],
                Arc::clone(&primary_requests),
                primary_queries,
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: ScriptedProvider::boxed(
                    "fallback",
                    "fallback-endpoint",
                    "fallback-model",
                    ReasoningReplayPolicy::RequiredOnToolCall,
                    vec![Invocation::Stream(Ok(vec![chunk("fallback answer")]))],
                    Arc::clone(&fallback_requests),
                    Arc::clone(&fallback_queries),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::DeepSeek,
            }],
            Arc::new(move |from, to, reason| {
                selected
                    .lock()
                    .unwrap()
                    .push((from, to, reason.to_string()));
                Ok(())
            }),
        )
        .unwrap();

        let primary_route = RouteSnapshot {
            provider_endpoint_id: "primary-endpoint".to_string(),
            provider_family: "primary".to_string(),
            api_style: ReasoningApiStyle::OpenAiResponses,
            model_id: "primary-model".to_string(),
            reasoning_profile_id: "primary-profile".to_string(),
            reasoning_profile_version: 1,
            replay_policy: ReasoningReplayPolicy::NotRequired,
        };
        let tool_call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        };
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![tool_call.clone()]);
        assistant.set_provider_turn(crate::llm::provider_turn::ProviderTurnEnvelope::capture(
            "turn-item",
            "old-sample",
            primary_route,
            "",
            None,
            None,
            vec![tool_call],
            false,
        ));
        let mut unprojected = request();
        unprojected.messages = vec![
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "dependent answer"),
            Message::text(Role::User, "continue"),
        ];
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(&wrapper, unprojected, &tx, false);

        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Provider(output) = attempt.next().await else {
            panic!("fallback output")
        };
        assert_eq!(output.accepted.route_snapshot.provider_family, "fallback");
        assert_eq!(output.accepted.route_snapshot.model_id, "fallback-model");
        assert_eq!(output.accepted.replay_projection_omitted_units, 1);
        assert_eq!(
            *selections.lock().unwrap(),
            vec![(
                0,
                1,
                "primary_invocation_failed_automatic_fallback".to_string()
            )]
        );
        assert!(primary_requests.lock().unwrap()[0]
            .messages
            .iter()
            .any(|message| message.role == Role::Tool));
        let fallback_request = &fallback_requests.lock().unwrap()[0];
        assert!(!fallback_request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool));
        // One query projects the fallback request. Capturing the accepted
        // wrapper route then queries the selected route once while rebuilding
        // its request and once for the immutable snapshot returned to us.
        assert_eq!(*fallback_queries.lock().unwrap(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn non_streaming_fallback_captures_the_route_selected_by_complete() {
        let primary_requests = Arc::new(Mutex::new(Vec::new()));
        let fallback_requests = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            ScriptedProvider::boxed(
                "primary",
                "primary-endpoint",
                "primary-model",
                ReasoningReplayPolicy::NotRequired,
                vec![Invocation::Complete(Err(CoreError::TransientLlm(
                    "primary unavailable".to_string(),
                )))],
                primary_requests,
                Arc::new(Mutex::new(0)),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: ScriptedProvider::boxed(
                    "fallback",
                    "fallback-endpoint",
                    "fallback-model",
                    ReasoningReplayPolicy::RequiredOnToolCall,
                    vec![Invocation::Complete(Ok(response("fallback")))],
                    Arc::clone(&fallback_requests),
                    Arc::new(Mutex::new(0)),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::DeepSeek,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(&wrapper, request(), &tx, true);

        let ModelAttemptProgress::Completion(output) = attempt.next().await else {
            panic!("fallback completion")
        };
        assert_eq!(output.response.content, "fallback");
        assert_eq!(output.accepted.route_snapshot.provider_family, "fallback");
        assert_eq!(output.accepted.route_snapshot.model_id, "fallback-model");
        assert_eq!(fallback_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stream_open_rate_limit_honors_retry_after_and_stops_at_exact_budget() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Err(CoreError::RateLimited {
                    retry_after_secs: 7,
                })),
                Invocation::Stream(Err(CoreError::RateLimited {
                    retry_after_secs: 7,
                })),
                Invocation::Stream(Err(CoreError::RateLimited {
                    retry_after_secs: 7,
                })),
                Invocation::Stream(Err(CoreError::RateLimited {
                    retry_after_secs: 7,
                })),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        let started_at = tokio::time::Instant::now();

        let ModelAttemptProgress::Failed(failure) = attempt.next().await else {
            panic!("fourth rate limit should exhaust three retries")
        };

        assert_eq!(failure.stage, ModelAttemptFailureStage::Connect);
        assert!(matches!(
            failure.error,
            CoreError::RateLimited {
                retry_after_secs: 7
            }
        ));
        assert_eq!(requests.lock().unwrap().len(), 4);
        assert_eq!(started_at.elapsed(), Duration::from_secs(21));

        let states = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                AgentEvent::ConnectionState { state } => Some(state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(states.len(), 4);
        for (index, state) in states[..3].iter().enumerate() {
            assert_eq!(state.state, ConnectionStateKind::Reconnecting);
            assert_eq!(
                state.error_category,
                Some(ConnectionErrorCategory::RateLimit)
            );
            assert_eq!(state.attempt, u32::try_from(index + 1).unwrap());
            assert_eq!(state.max_attempts, 3);
            assert!(state.next_retry_at.is_some());
            assert!(state.recoverable);
        }
        assert_eq!(states[3].state, ConnectionStateKind::Failed);
        assert_eq!(states[3].attempt, 3);
        assert_eq!(states[3].max_attempts, 3);
        assert!(!states[3].recoverable);
    }

    #[tokio::test(start_paused = true)]
    async fn context_overflow_requests_compaction_after_one_physical_call() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Err(CoreError::ContextOverflow(12, 8)))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        let ModelAttemptProgress::NeedsContextCompaction(overflow) = attempt.next().await else {
            panic!("context overflow should be delegated to the compaction owner")
        };

        assert!(matches!(overflow.error, CoreError::ContextOverflow(12, 8)));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_rate_limit_before_output_uses_owned_retry_budget() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Ok(vec![ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::RateLimited {
                        retry_after_secs: 7,
                    },
                }])),
                Invocation::Stream(Ok(vec![chunk("accepted after retry")])),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;
        let started_at = tokio::time::Instant::now();

        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::Provider(accepted) = attempt.next().await else {
            panic!("rate-limited terminal stream should retry before accepting output")
        };

        assert!(matches!(
            accepted.event,
            AcceptedProviderEvent::Chunk(ref chunk) if chunk.delta == "accepted after retry"
        ));
        assert_eq!(started_at.elapsed(), Duration::from_secs(7));
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
                event,
                AgentEvent::ConnectionState { state }
                    if state.state == ConnectionStateKind::Reconnecting
                        && state.error_category == Some(ConnectionErrorCategory::RateLimit)
                        && state.attempt == 1
            ))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_rate_limits_across_successful_opens_stop_at_exact_shared_budget() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let terminal_rate_limit = || ProviderStreamEvent::TerminalError {
            failure: crate::llm::ProviderStreamFailure::RateLimited {
                retry_after_secs: 7,
            },
        };
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Ok(vec![terminal_rate_limit()])),
                Invocation::Stream(Ok(vec![terminal_rate_limit()])),
                Invocation::Stream(Ok(vec![terminal_rate_limit()])),
                Invocation::Stream(Ok(vec![terminal_rate_limit()])),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        let started_at = tokio::time::Instant::now();
        for _ in 0..3 {
            expect_stream_opened(&mut attempt).await;
        }
        let ModelAttemptProgress::Failed(failure) = attempt.next().await else {
            panic!("fourth terminal rate limit should exhaust the shared retry budget")
        };

        assert_eq!(failure.stage, ModelAttemptFailureStage::Stream);
        assert!(matches!(
            failure.error,
            CoreError::RateLimited {
                retry_after_secs: 7
            }
        ));
        assert_eq!(requests.lock().unwrap().len(), 4);
        assert_eq!(started_at.elapsed(), Duration::from_secs(21));

        let states = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                AgentEvent::ConnectionState { state } => Some(state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(states.len(), 4);
        for (index, state) in states[..3].iter().enumerate() {
            assert_eq!(state.state, ConnectionStateKind::Reconnecting);
            assert_eq!(
                state.error_category,
                Some(ConnectionErrorCategory::RateLimit)
            );
            assert_eq!(state.attempt, u32::try_from(index + 1).unwrap());
            assert_eq!(state.max_attempts, 3);
            assert!(state.next_retry_at.is_some());
            assert!(state.recoverable);
        }
        assert_eq!(states[3].state, ConnectionStateKind::Failed);
        assert_eq!(
            states[3].error_category,
            Some(ConnectionErrorCategory::RateLimit)
        );
        assert_eq!(states[3].attempt, 3);
        assert_eq!(states[3].max_attempts, 3);
        assert!(!states[3].recoverable);
    }

    #[tokio::test(start_paused = true)]
    async fn invisible_metadata_does_not_reset_terminal_rate_limit_budget() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let metadata_then_rate_limit = || {
            Invocation::Stream(Ok(vec![
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        thinking_delta: None,
                    }),
                },
                ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::RateLimited {
                        retry_after_secs: 7,
                    },
                },
            ]))
        };
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                metadata_then_rate_limit(),
                metadata_then_rate_limit(),
                metadata_then_rate_limit(),
                metadata_then_rate_limit(),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        let started_at = tokio::time::Instant::now();
        for _ in 0..3 {
            assert!(matches!(
                attempt.next().await,
                ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                    event: AcceptedProviderEvent::Chunk(ref chunk),
                    ..
                }) if chunk.usage.is_some() && !chunk_is_visible(chunk)
            ));
            expect_stream_opened(&mut attempt).await;
        }
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                event: AcceptedProviderEvent::Chunk(ref chunk),
                ..
            }) if chunk.usage.is_some() && !chunk_is_visible(chunk)
        ));
        let ModelAttemptProgress::Failed(failure) = attempt.next().await else {
            panic!("fourth post-metadata rate limit should exhaust the shared retry budget")
        };

        assert_eq!(failure.stage, ModelAttemptFailureStage::Stream);
        assert!(matches!(failure.error, CoreError::RateLimited { .. }));
        assert_eq!(requests.lock().unwrap().len(), 4);
        assert_eq!(started_at.elapsed(), Duration::from_secs(21));

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(events.len(), 4);
        for (index, event) in events[..3].iter().enumerate() {
            assert!(matches!(
                event,
                AgentEvent::ConnectionState { state }
                    if state.state == ConnectionStateKind::Reconnecting
                        && state.error_category == Some(ConnectionErrorCategory::RateLimit)
                        && state.attempt == u32::try_from(index + 1).unwrap()
            ));
        }
        assert!(matches!(
            events[3],
            AgentEvent::ConnectionState { ref state }
                if state.state == ConnectionStateKind::Failed
                    && state.error_category == Some(ConnectionErrorCategory::RateLimit)
                    && state.attempt == 3
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_context_overflow_before_output_requests_compaction() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::ContextOverflow {
                        prompt_tokens: 12,
                        max_tokens: 8,
                    },
                },
            ]))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;

        let ModelAttemptProgress::NeedsContextCompaction(overflow) = attempt.next().await else {
            panic!("terminal context overflow should return the typed compaction outcome")
        };

        assert!(matches!(overflow.error, CoreError::ContextOverflow(12, 8)));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_rate_limit_after_resettable_draft_reconnects() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Stream(Ok(vec![
                    chunk("visible"),
                    ProviderStreamEvent::TerminalError {
                        failure: crate::llm::ProviderStreamFailure::RateLimited {
                            retry_after_secs: 7,
                        },
                    },
                ])),
                Invocation::Stream(Ok(vec![chunk("recovered")])),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                event: AcceptedProviderEvent::Chunk(ref chunk),
                ..
            }) if chunk.delta == "visible"
        ));

        expect_stream_opened(&mut attempt).await;
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                event: AcceptedProviderEvent::Chunk(ref chunk),
                ..
            }) if chunk.delta == "recovered"
        ));
        assert_eq!(requests.lock().unwrap().len(), 2);
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ConnectionState { state }
                if state.state == ConnectionStateKind::Reconnecting
                    && state.error_category == Some(ConnectionErrorCategory::RateLimit)
                    && state.next_retry_at.is_some()
        )));
        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::StreamReset { .. })));
    }

    #[tokio::test]
    async fn context_overflow_after_resettable_draft_requests_compaction_without_replay() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                chunk("incomplete draft"),
                ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::ContextOverflow {
                        prompt_tokens: 12,
                        max_tokens: 8,
                    },
                },
            ]))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(ModelAttemptProviderEvent {
                event: AcceptedProviderEvent::Chunk(ref chunk),
                ..
            }) if chunk.delta == "incomplete draft"
        ));

        let ModelAttemptProgress::NeedsContextCompaction(overflow) = attempt.next().await else {
            panic!("typed context overflow must go to compaction")
        };
        assert!(matches!(overflow.error, CoreError::ContextOverflow(12, 8)));
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AgentEvent::StreamReset { .. })));
    }

    #[tokio::test]
    async fn permanent_terminal_error_after_resettable_draft_never_replays() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                chunk("partial answer"),
                ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::provider("fatal"),
                },
            ]))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);
        expect_stream_opened(&mut attempt).await;
        assert!(matches!(
            attempt.next().await,
            ModelAttemptProgress::Provider(_)
        ));

        let ModelAttemptProgress::Failed(failure) = attempt.next().await else {
            panic!("permanent provider failure must not become a reconnect")
        };
        assert!(matches!(failure.error, CoreError::Llm(ref message) if message == "fatal"));
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(!std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AgentEvent::StreamReset { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_and_terminal_stream_events_never_retry() {
        let cancelled_requests = Arc::new(Mutex::new(Vec::new()));
        let cancelled = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                ProviderStreamEvent::Cancelled {
                    message: "user stopped".to_string(),
                },
            ]))],
            Arc::clone(&cancelled_requests),
            Arc::new(Mutex::new(0)),
        );
        let (cancelled_tx, mut cancelled_rx) = event_channel();
        let mut cancelled_attempt =
            ModelAttempt::new(cancelled.as_ref(), request(), &cancelled_tx, false);

        expect_stream_opened(&mut cancelled_attempt).await;
        assert!(matches!(
            cancelled_attempt.next().await,
            ModelAttemptProgress::Cancelled(ModelAttemptCancellation { ref message, .. })
                if message == "user stopped"
        ));
        assert_eq!(cancelled_requests.lock().unwrap().len(), 1);
        assert!(cancelled_rx.try_recv().is_err());

        let terminal_requests = Arc::new(Mutex::new(Vec::new()));
        let terminal = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(vec![
                ProviderStreamEvent::TerminalError {
                    failure: crate::llm::ProviderStreamFailure::provider("fatal"),
                },
            ]))],
            Arc::clone(&terminal_requests),
            Arc::new(Mutex::new(0)),
        );
        let (terminal_tx, mut terminal_rx) = event_channel();
        let mut terminal_attempt =
            ModelAttempt::new(terminal.as_ref(), request(), &terminal_tx, false);

        expect_stream_opened(&mut terminal_attempt).await;
        let ModelAttemptProgress::Failed(failure) = terminal_attempt.next().await else {
            panic!("terminal provider event should fail the attempt")
        };
        assert_eq!(failure.stage, ModelAttemptFailureStage::Stream);
        assert!(matches!(failure.error, CoreError::Llm(ref message) if message == "fatal"));
        assert_eq!(terminal_requests.lock().unwrap().len(), 1);
        assert!(terminal_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn core_error_cancellation_from_open_or_forced_completion_is_typed_and_not_retried() {
        let open_requests = Arc::new(Mutex::new(Vec::new()));
        let open_cancelled = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Err(CoreError::Cancelled(
                "cancelled while opening stream".to_string(),
            )))],
            Arc::clone(&open_requests),
            Arc::new(Mutex::new(0)),
        );
        let (open_tx, mut open_rx) = event_channel();
        let mut open_attempt =
            ModelAttempt::new(open_cancelled.as_ref(), request(), &open_tx, false);

        let ModelAttemptProgress::Cancelled(open_cancellation) = open_attempt.next().await else {
            panic!("stream-open CoreError::Cancelled should remain typed cancellation")
        };

        assert_eq!(open_cancellation.message, "cancelled while opening stream");
        assert!(open_cancellation.accepted.is_none());
        assert_eq!(open_requests.lock().unwrap().len(), 1);
        assert!(open_rx.try_recv().is_err());

        let completion_requests = Arc::new(Mutex::new(Vec::new()));
        let completion_cancelled = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Complete(Err(CoreError::Cancelled(
                "cancelled during safe completion restart".to_string(),
            )))],
            Arc::clone(&completion_requests),
            Arc::new(Mutex::new(0)),
        );
        let mut safe_restart_request = request();
        safe_restart_request.reasoning_enabled = Some(false);
        let (completion_tx, mut completion_rx) = event_channel();
        let mut completion_attempt = ModelAttempt::new(
            completion_cancelled.as_ref(),
            safe_restart_request,
            &completion_tx,
            true,
        );

        let ModelAttemptProgress::Cancelled(completion_cancellation) =
            completion_attempt.next().await
        else {
            panic!("forced completion CoreError::Cancelled should remain typed cancellation")
        };

        assert_eq!(
            completion_cancellation.message,
            "cancelled during safe completion restart"
        );
        assert!(completion_cancellation.accepted.is_none());
        let completion_requests = completion_requests.lock().unwrap();
        assert_eq!(completion_requests.len(), 1);
        assert_eq!(completion_requests[0].reasoning_enabled, Some(false));
        assert!(completion_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancel_token_interrupts_pending_stream_open_without_resend() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::PendingStreamOpen],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false)
            .with_cancel_token(cancel_token.clone());

        let mut next = Box::pin(attempt.next());
        assert!(matches!(futures::poll!(&mut next), Poll::Pending));
        assert_eq!(requests.lock().unwrap().len(), 1);
        cancel_token.cancel();

        let ModelAttemptProgress::Cancelled(cancellation) = next.await else {
            panic!("pending stream open should be interrupted by the attempt token")
        };
        assert_eq!(cancellation.message, "cancelled by user");
        assert!(cancellation.accepted.is_none());
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_token_interrupts_pending_stream_read_without_resend() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::PendingStreamRead],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false)
            .with_cancel_token(cancel_token.clone());
        expect_stream_opened(&mut attempt).await;

        let mut next = Box::pin(attempt.next());
        assert!(matches!(futures::poll!(&mut next), Poll::Pending));
        cancel_token.cancel();

        let ModelAttemptProgress::Cancelled(cancellation) = next.await else {
            panic!("pending stream read should be interrupted by the attempt token")
        };
        assert!(cancellation.accepted.is_none());
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_token_interrupts_stream_retry_sleep_without_resend() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Err(CoreError::RateLimited {
                retry_after_secs: 3_600,
            }))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false)
            .with_cancel_token(cancel_token.clone());

        let mut next = Box::pin(attempt.next());
        assert!(matches!(futures::poll!(&mut next), Poll::Pending));
        assert_eq!(requests.lock().unwrap().len(), 1);
        cancel_token.cancel();

        assert!(matches!(
            next.await,
            ModelAttemptProgress::Cancelled(ModelAttemptCancellation { accepted: None, .. })
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_token_interrupts_pending_forced_completion_without_resend() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::PendingComplete],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, true)
            .with_cancel_token(cancel_token.clone());

        let mut next = Box::pin(attempt.next());
        assert!(matches!(futures::poll!(&mut next), Poll::Pending));
        assert_eq!(requests.lock().unwrap().len(), 1);
        cancel_token.cancel();

        assert!(matches!(
            next.await,
            ModelAttemptProgress::Cancelled(ModelAttemptCancellation { accepted: None, .. })
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_token_interrupts_completion_backoff_without_resend() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Complete(Err(CoreError::RateLimited {
                retry_after_secs: 3_600,
            }))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, true)
            .with_cancel_token(cancel_token.clone());

        let mut next = Box::pin(attempt.next());
        assert!(matches!(futures::poll!(&mut next), Poll::Pending));
        assert_eq!(requests.lock().unwrap().len(), 1);
        cancel_token.cancel();

        assert!(matches!(
            next.await,
            ModelAttemptProgress::Cancelled(ModelAttemptCancellation { accepted: None, .. })
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn processed_provider_output_then_cancel_keeps_accepted_provenance() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::StreamThenPending(vec![chunk("accepted")])],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false)
            .with_cancel_token(cancel_token.clone());
        expect_stream_opened(&mut attempt).await;

        let ModelAttemptProgress::Provider(output) = attempt.next().await else {
            panic!("provider output should be processed before cancellation")
        };
        let sample_id = output.accepted.sample_id;
        cancel_token.cancel();
        let ModelAttemptProgress::Cancelled(cancellation) = attempt.next().await else {
            panic!("pending read after accepted output should observe cancellation")
        };

        assert_eq!(
            cancellation
                .accepted
                .expect("accepted output cancellation keeps provenance")
                .sample_id,
            sample_id
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancelled_token_preempts_an_always_ready_unconsumed_stream_event() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::RepeatingStreamChunks],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, _rx) = event_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false)
            .with_cancel_token(cancel_token.clone());
        expect_stream_opened(&mut attempt).await;

        let ModelAttemptProgress::Provider(first) = attempt.next().await else {
            panic!("first repeated chunk should be accepted normally")
        };
        let sample_id = first.accepted.sample_id;
        cancel_token.cancel();
        let progress = tokio::time::timeout(Duration::from_millis(100), attempt.next())
            .await
            .expect("an already-cancelled token must not be starved by ready chunks");
        let ModelAttemptProgress::Cancelled(cancellation) = progress else {
            panic!("an unconsumed ready chunk must not beat an already-cancelled token")
        };

        assert_eq!(
            cancellation
                .accepted
                .expect("cancellation after output keeps accepted provenance")
                .sample_id,
            sample_id
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn clean_empty_stream_completes_without_reconnect_or_completion_fallback() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![Invocation::Stream(Ok(Vec::new()))],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, false);

        expect_stream_opened(&mut attempt).await;
        let ModelAttemptProgress::StreamComplete { accepted, .. } = attempt.next().await else {
            panic!("a clean empty stream remains a completed model sample")
        };

        assert_eq!(accepted.route_snapshot.provider_family, "primary");
        assert_eq!(requests.lock().unwrap().len(), 1);
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(
                event,
                AgentEvent::ConnectionState { .. } | AgentEvent::StreamReset { .. }
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn non_streaming_completion_owns_one_exact_transient_retry_budget() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider::boxed(
            "primary",
            "primary-endpoint",
            "primary-model",
            ReasoningReplayPolicy::NotRequired,
            vec![
                Invocation::Complete(Err(CoreError::TransientLlm("reset one".to_string()))),
                Invocation::Complete(Err(CoreError::TransientLlm("reset two".to_string()))),
                Invocation::Complete(Err(CoreError::TransientLlm("reset three".to_string()))),
                Invocation::Complete(Err(CoreError::TransientLlm("give up".to_string()))),
            ],
            Arc::clone(&requests),
            Arc::new(Mutex::new(0)),
        );
        let (tx, mut rx) = event_channel();
        let mut attempt = ModelAttempt::new(provider.as_ref(), request(), &tx, true);
        let started_at = tokio::time::Instant::now();

        let ModelAttemptProgress::Failed(failure) = attempt.next().await else {
            panic!("fourth completion failure should exhaust three retries")
        };

        assert_eq!(failure.stage, ModelAttemptFailureStage::Completion);
        assert!(matches!(
            failure.error,
            CoreError::TransientLlm(ref message) if message == "give up"
        ));
        assert_eq!(requests.lock().unwrap().len(), 4);
        assert_eq!(started_at.elapsed(), Duration::from_secs(7));

        let states = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                AgentEvent::ConnectionState { state } => Some(state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            states.iter().map(|state| state.state).collect::<Vec<_>>(),
            vec![
                ConnectionStateKind::Reconnecting,
                ConnectionStateKind::Reconnecting,
                ConnectionStateKind::Reconnecting,
                ConnectionStateKind::Failed,
            ]
        );
        assert!(states
            .iter()
            .all(|state| state.error_category == Some(ConnectionErrorCategory::Network)));
    }
}
