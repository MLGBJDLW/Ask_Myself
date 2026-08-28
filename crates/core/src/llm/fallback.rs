//! Safe, policy-authorized provider fallback.
//!
//! A route may advance only on a retryable provider failure and only before
//! any response chunk is exposed. The selection callback commits durable run
//! provenance before the first fallback chunk leaves this wrapper.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use futures::{stream, stream::BoxStream, StreamExt};

use super::reasoning_profile::ReasoningReplayPolicy;
use super::{
    provider_stream_event_from_error, CompletionRequest, CompletionResponse, LlmProvider,
    ProviderStreamEvent, ProviderType, ReplayHistoryProjection,
};
use crate::error::CoreError;

pub type FallbackSelectionCallback =
    Arc<dyn Fn(usize, usize, &str) -> Result<(), CoreError> + Send + Sync>;

pub struct AutomaticFallbackCandidate {
    pub fallback_index: usize,
    pub provider: Box<dyn LlmProvider>,
    pub model: String,
    pub provider_type: ProviderType,
}

struct Route {
    fallback_index: usize,
    provider: Box<dyn LlmProvider>,
    model: String,
    provider_type: ProviderType,
}

/// Provider chain for an already validated `automatic` Capability Binding.
/// It never invents targets and never crosses an unapproved route boundary;
/// callers supply only the frozen, policy-eligible candidates.
pub struct AutomaticFallbackProvider {
    routes: Vec<Route>,
    active_position: AtomicUsize,
    on_selected: FallbackSelectionCallback,
}

impl AutomaticFallbackProvider {
    pub fn new(
        primary_fallback_index: usize,
        primary: Box<dyn LlmProvider>,
        primary_model: String,
        primary_provider_type: ProviderType,
        fallbacks: Vec<AutomaticFallbackCandidate>,
        on_selected: FallbackSelectionCallback,
    ) -> Result<Self, CoreError> {
        let mut routes = vec![Route {
            fallback_index: primary_fallback_index,
            provider: primary,
            model: primary_model,
            provider_type: primary_provider_type,
        }];
        for fallback in fallbacks {
            if fallback.fallback_index <= routes.last().map_or(0, |route| route.fallback_index) {
                return Err(CoreError::InvalidInput(
                    "Automatic fallback candidates must be strictly ordered".to_string(),
                ));
            }
            routes.push(Route {
                fallback_index: fallback.fallback_index,
                provider: fallback.provider,
                model: fallback.model,
                provider_type: fallback.provider_type,
            });
        }
        Ok(Self {
            routes,
            active_position: AtomicUsize::new(0),
            on_selected,
        })
    }

    fn active_position(&self) -> usize {
        self.active_position
            .load(Ordering::Acquire)
            .min(self.routes.len().saturating_sub(1))
    }

    fn request_for_route(&self, request: &CompletionRequest, position: usize) -> CompletionRequest {
        let mut request = request.clone();
        request.model = self.routes[position].model.clone();
        request.provider_type = Some(self.routes[position].provider_type);
        let route = &self.routes[position];
        if let ReplayHistoryProjection::Caller(history_policy) =
            route.provider.replay_history_projection(&request)
        {
            let mut route_snapshot = route.provider.route_snapshot(&request);
            route_snapshot.replay_policy = history_policy;
            request.messages = super::reasoning_replay::prepare_provider_replay_history(
                &request.messages,
                &route_snapshot,
            )
            .messages;
        }
        request
    }

    /// Lock an in-progress tool loop to the route that produced its latest
    /// replayable assistant tool-call turn. A final assistant answer closes
    /// the routing boundary; steering user messages inside the loop do not.
    fn locked_position_for_request(&self, request: &CompletionRequest) -> Option<usize> {
        let locked_route = request.messages.iter().rev().find_map(|message| {
            if message.role == super::Role::Assistant
                && message
                    .tool_calls
                    .as_ref()
                    .is_none_or(|calls| calls.is_empty())
            {
                if message.provider_turn().is_some_and(|envelope| {
                    matches!(
                        envelope.capture_status,
                        super::reasoning_profile::ReasoningCaptureStatus::Interrupted
                            | super::reasoning_profile::ReasoningCaptureStatus::Truncated
                    )
                }) {
                    return None;
                }
                return Some(None);
            }
            message
                .provider_turn()
                .filter(|envelope| !envelope.tool_calls.is_empty())
                .map(|envelope| Some(&envelope.route))
        })??;

        self.routes
            .iter()
            .enumerate()
            .find_map(|(position, route)| {
                let mut route_request = request.clone();
                route_request.model = route.model.clone();
                route_request.provider_type = Some(route.provider_type);
                route
                    .provider
                    .route_snapshot(&route_request)
                    .same_route_identity(locked_route)
                    .then_some(position)
            })
    }

    fn route_window(&self, request: &CompletionRequest) -> (usize, usize) {
        if let Some(locked_position) = self.locked_position_for_request(request) {
            (locked_position, locked_position + 1)
        } else {
            (self.active_position(), self.routes.len())
        }
    }

    fn fallback_reason(&self, from_position: usize) -> &'static str {
        if self.routes[from_position].fallback_index == 0 {
            "primary_invocation_failed_automatic_fallback"
        } else {
            "fallback_invocation_failed_automatic_fallback"
        }
    }

    fn select_route(&self, from_position: usize, to_position: usize) -> Result<(), CoreError> {
        if from_position == to_position {
            return Ok(());
        }
        self.active_position
            .compare_exchange(
                from_position,
                to_position,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|actual| {
                CoreError::Conflict(format!(
                    "Automatic fallback route changed concurrently from {from_position} to {actual}"
                ))
            })?;
        let result = (self.on_selected)(
            self.routes[from_position].fallback_index,
            self.routes[to_position].fallback_index,
            self.fallback_reason(from_position),
        );
        if result.is_err() {
            let _ = self.active_position.compare_exchange(
                to_position,
                from_position,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        result
    }

    async fn open_stream_from<'a>(
        &'a self,
        request: &CompletionRequest,
        start_position: usize,
        end_position: usize,
    ) -> Result<(usize, BoxStream<'a, ProviderStreamEvent>), CoreError> {
        let mut last_retryable = None;
        for position in start_position..end_position {
            let route_request = self.request_for_route(request, position);
            match self.routes[position]
                .provider
                .stream_events(&route_request)
                .await
            {
                Ok(stream) => return Ok((position, stream)),
                Err(error) if automatic_fallback_error(&error) => last_retryable = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_retryable.unwrap_or_else(|| {
            CoreError::TransientLlm("Automatic fallback chain is exhausted".to_string())
        }))
    }
}

struct FallbackStreamState<'a> {
    owner: &'a AutomaticFallbackProvider,
    request: CompletionRequest,
    selected_position: usize,
    current_position: usize,
    end_position: usize,
    current: BoxStream<'a, ProviderStreamEvent>,
    emitted_output: bool,
}

#[async_trait]
impl LlmProvider for AutomaticFallbackProvider {
    fn name(&self) -> &str {
        self.routes[self.active_position()].provider.name()
    }

    fn stream_max_retries(&self) -> Option<u32> {
        self.routes[self.active_position()]
            .provider
            .stream_max_retries()
    }

    fn prompt_cache_profile(&self, _model: &str) -> super::prompt_cache::PromptCacheProfile {
        let position = self.active_position();
        self.routes[position]
            .provider
            .prompt_cache_profile(&self.routes[position].model)
    }

    fn reasoning_replay_policy(
        &self,
        _model: &str,
    ) -> super::reasoning_profile::ReasoningReplayPolicy {
        let position = self.active_position();
        self.routes[position]
            .provider
            .reasoning_replay_policy(&self.routes[position].model)
    }

    fn reasoning_replay_history_policy(
        &self,
        _model: &str,
    ) -> super::reasoning_profile::ReasoningReplayPolicy {
        // `request_for_route` applies the concrete route contract immediately
        // before opening that route.
        ReasoningReplayPolicy::NotRequired
    }

    fn replay_history_projection(&self, _request: &CompletionRequest) -> ReplayHistoryProjection {
        ReplayHistoryProjection::ProviderSelectedRoute
    }

    fn route_snapshot(&self, request: &CompletionRequest) -> super::provider_turn::RouteSnapshot {
        let position = self
            .locked_position_for_request(request)
            .unwrap_or_else(|| self.active_position());
        let request = self.request_for_route(request, position);
        self.routes[position].provider.route_snapshot(&request)
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        self.routes[self.active_position()]
            .provider
            .list_models()
            .await
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let active_position = self.active_position();
        let (selected_position, end_position) = self.route_window(request);
        if selected_position != active_position {
            return Err(CoreError::Conflict(
                "The provider route changed after this tool loop was locked".to_string(),
            ));
        }
        let mut last_retryable = None;
        for position in selected_position..end_position {
            let route_request = self.request_for_route(request, position);
            match self.routes[position]
                .provider
                .complete(&route_request)
                .await
            {
                Ok(response) => {
                    self.select_route(selected_position, position)?;
                    return Ok(response);
                }
                Err(error) if automatic_fallback_error(&error) => last_retryable = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_retryable.unwrap_or_else(|| {
            CoreError::TransientLlm("Automatic fallback chain is exhausted".to_string())
        }))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        let active_position = self.active_position();
        let (selected_position, end_position) = self.route_window(request);
        if selected_position != active_position {
            return Err(CoreError::Conflict(
                "The provider route changed after this tool loop was locked".to_string(),
            ));
        }
        let (current_position, current) = self
            .open_stream_from(request, selected_position, end_position)
            .await?;
        let state = FallbackStreamState {
            owner: self,
            request: request.clone(),
            selected_position,
            current_position,
            end_position,
            current,
            emitted_output: false,
        };
        Ok(Box::pin(stream::unfold(Some(state), |state| async move {
            let mut state = state?;
            loop {
                match state.current.next().await {
                    Some(ProviderStreamEvent::Chunk { chunk }) => {
                        if !state.emitted_output {
                            if let Err(error) = state
                                .owner
                                .select_route(state.selected_position, state.current_position)
                            {
                                return Some((
                                    ProviderStreamEvent::TerminalError {
                                        failure: error.into(),
                                    },
                                    None,
                                ));
                            }
                            state.selected_position = state.current_position;
                        }
                        state.emitted_output = true;
                        return Some((ProviderStreamEvent::Chunk { chunk }, Some(state)));
                    }
                    Some(ProviderStreamEvent::HostedTool { tool }) => {
                        if !state.emitted_output {
                            if let Err(error) = state
                                .owner
                                .select_route(state.selected_position, state.current_position)
                            {
                                return Some((
                                    ProviderStreamEvent::TerminalError {
                                        failure: error.into(),
                                    },
                                    None,
                                ));
                            }
                            state.selected_position = state.current_position;
                        }
                        state.emitted_output = true;
                        return Some((ProviderStreamEvent::HostedTool { tool }, Some(state)));
                    }
                    Some(ProviderStreamEvent::ReplayState { replay }) => {
                        if !state.emitted_output {
                            if let Err(error) = state
                                .owner
                                .select_route(state.selected_position, state.current_position)
                            {
                                return Some((
                                    ProviderStreamEvent::TerminalError {
                                        failure: error.into(),
                                    },
                                    None,
                                ));
                            }
                            state.selected_position = state.current_position;
                        }
                        state.emitted_output = true;
                        return Some((ProviderStreamEvent::ReplayState { replay }, Some(state)));
                    }
                    Some(ProviderStreamEvent::RecoverableError { message })
                        if !state.emitted_output
                            && state.current_position + 1 < state.end_position =>
                    {
                        match state
                            .owner
                            .open_stream_from(
                                &state.request,
                                state.current_position + 1,
                                state.end_position,
                            )
                            .await
                        {
                            Ok((position, stream)) => {
                                state.current_position = position;
                                state.current = stream;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    initial_error = %message,
                                    fallback_error = %error,
                                    "automatic fallback stream failed before output"
                                );
                                return Some((provider_stream_event_from_error(error), None));
                            }
                        }
                    }
                    Some(event) => return Some((event, Some(state))),
                    None if !state.emitted_output
                        && state.current_position + 1 < state.end_position =>
                    {
                        match state
                            .owner
                            .open_stream_from(
                                &state.request,
                                state.current_position + 1,
                                state.end_position,
                            )
                            .await
                        {
                            Ok((position, stream)) => {
                                state.current_position = position;
                                state.current = stream;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    fallback_error = %error,
                                    "automatic fallback failed after an empty provider stream"
                                );
                                return Some((provider_stream_event_from_error(error), None));
                            }
                        }
                    }
                    None if !state.emitted_output => {
                        return Some((
                            ProviderStreamEvent::RecoverableError {
                                message: "Provider stream ended before producing output"
                                    .to_string(),
                            },
                            None,
                        ));
                    }
                    None => return None,
                }
            }
        })))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        self.routes[self.active_position()]
            .provider
            .health_check()
            .await
    }

    async fn runtime_metadata(&self) -> Option<serde_json::Value> {
        let position = self.active_position();
        Some(serde_json::json!({
            "capabilityRegistryFallbackIndex": self.routes[position].fallback_index,
            "provider": self.routes[position].provider.name(),
            "model": self.routes[position].model,
            "providerMetadata": self.routes[position].provider.runtime_metadata().await,
        }))
    }
}

fn automatic_fallback_error(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::RateLimited { .. } | CoreError::TransientLlm(_) | CoreError::StreamIncomplete(_)
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::stream;

    use super::*;
    use crate::llm::{
        FinishReason, Message, ProviderStreamFailure, Role, StreamChunk, ToolCallRequest, Usage,
    };

    #[derive(Clone)]
    enum Behavior {
        Stream(Vec<ProviderStreamEvent>),
        StreamRateLimited,
        StreamCancelled,
        CompleteTransient,
        CompleteSuccess,
    }

    struct MockProvider {
        name: &'static str,
        behavior: Behavior,
        models: Arc<Mutex<Vec<String>>>,
        replay_policy: ReasoningReplayPolicy,
        history_replay_policy: ReasoningReplayPolicy,
        histories: Option<Arc<Mutex<Vec<Vec<Message>>>>>,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
            self.replay_policy
        }

        fn reasoning_replay_history_policy(&self, _model: &str) -> ReasoningReplayPolicy {
            self.history_replay_policy
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(Vec::new())
        }

        async fn complete(
            &self,
            request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            self.models.lock().unwrap().push(request.model.clone());
            if let Some(histories) = &self.histories {
                histories.lock().unwrap().push(request.messages.clone());
            }
            match self.behavior {
                Behavior::CompleteTransient => {
                    Err(CoreError::TransientLlm("temporary outage".to_string()))
                }
                Behavior::CompleteSuccess => Ok(CompletionResponse {
                    content: request.model.clone(),
                    tool_calls: None,
                    finish_reason: FinishReason::Stop,
                    usage: Usage::default(),
                    thinking: None,
                    provider_replay: None,
                }),
                Behavior::Stream(_) | Behavior::StreamRateLimited | Behavior::StreamCancelled => {
                    unreachable!("stream fixture used for completion")
                }
            }
        }

        async fn stream_events(
            &self,
            request: &CompletionRequest,
        ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
            self.models.lock().unwrap().push(request.model.clone());
            if let Some(histories) = &self.histories {
                histories.lock().unwrap().push(request.messages.clone());
            }
            match &self.behavior {
                Behavior::Stream(events) => Ok(Box::pin(stream::iter(events.clone()))),
                Behavior::StreamRateLimited => Err(CoreError::RateLimited {
                    retry_after_secs: 7,
                }),
                Behavior::StreamCancelled => {
                    Err(CoreError::Cancelled("user stopped fallback".to_string()))
                }
                Behavior::CompleteTransient | Behavior::CompleteSuccess => {
                    unreachable!("completion fixture used for stream")
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

    fn hosted_tool() -> ProviderStreamEvent {
        ProviderStreamEvent::HostedTool {
            tool: Box::new(crate::llm::ProviderHostedToolEvent {
                call_id: "hosted-call-1".to_string(),
                tool_name: "web_search".to_string(),
                kind: crate::llm::ProviderHostedToolKind::WebSearch,
                provider_id: "search-1".to_string(),
                status: crate::llm::ProviderHostedToolStatus::Completed,
                arguments: Some("{\"query\":\"Nexa\"}".to_string()),
                content: Some("result".to_string()),
                artifacts: None,
            }),
        }
    }

    fn provider(
        name: &'static str,
        behavior: Behavior,
        models: Arc<Mutex<Vec<String>>>,
    ) -> Box<dyn LlmProvider> {
        Box::new(MockProvider {
            name,
            behavior,
            models,
            replay_policy: ReasoningReplayPolicy::Unknown,
            history_replay_policy: ReasoningReplayPolicy::Unknown,
            histories: None,
        })
    }

    fn provider_with_policy(
        name: &'static str,
        behavior: Behavior,
        models: Arc<Mutex<Vec<String>>>,
        replay_policy: ReasoningReplayPolicy,
    ) -> Box<dyn LlmProvider> {
        Box::new(MockProvider {
            name,
            behavior,
            models,
            replay_policy,
            history_replay_policy: replay_policy,
            histories: None,
        })
    }

    fn provider_with_policy_and_history(
        name: &'static str,
        behavior: Behavior,
        models: Arc<Mutex<Vec<String>>>,
        replay_policy: ReasoningReplayPolicy,
        histories: Arc<Mutex<Vec<Vec<Message>>>>,
    ) -> Box<dyn LlmProvider> {
        Box::new(MockProvider {
            name,
            behavior,
            models,
            replay_policy,
            history_replay_policy: replay_policy,
            histories: Some(histories),
        })
    }

    fn provider_with_output_and_history_policy(
        name: &'static str,
        behavior: Behavior,
        models: Arc<Mutex<Vec<String>>>,
        replay_policy: ReasoningReplayPolicy,
        history_replay_policy: ReasoningReplayPolicy,
        histories: Arc<Mutex<Vec<Vec<Message>>>>,
    ) -> Box<dyn LlmProvider> {
        Box::new(MockProvider {
            name,
            behavior,
            models,
            replay_policy,
            history_replay_policy,
            histories: Some(histories),
        })
    }

    fn request() -> CompletionRequest {
        CompletionRequest {
            model: "primary-model".to_string(),
            ..CompletionRequest::default()
        }
    }

    #[tokio::test]
    async fn recoverable_failure_before_output_advances_and_records_fallback() {
        let primary_models = Arc::new(Mutex::new(Vec::new()));
        let fallback_models = Arc::new(Mutex::new(Vec::new()));
        let selections = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&selections);
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::Stream(vec![ProviderStreamEvent::RecoverableError {
                    message: "connection reset".to_string(),
                }]),
                Arc::clone(&primary_models),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::Stream(vec![chunk("fallback answer")]),
                    Arc::clone(&fallback_models),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(move |from, to, reason| {
                recorded
                    .lock()
                    .unwrap()
                    .push((from, to, reason.to_string()));
                Ok(())
            }),
        )
        .unwrap();

        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Chunk { chunk }] if chunk.delta == "fallback answer"
        ));
        assert_eq!(*primary_models.lock().unwrap(), vec!["primary-model"]);
        assert_eq!(*fallback_models.lock().unwrap(), vec!["fallback-model"]);
        assert_eq!(
            *selections.lock().unwrap(),
            vec![(
                0,
                1,
                "primary_invocation_failed_automatic_fallback".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn fallback_preserves_hosted_tool_events_from_the_selected_route() {
        let selections = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&selections);
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::Stream(vec![ProviderStreamEvent::RecoverableError {
                    message: "connection reset".to_string(),
                }]),
                Arc::new(Mutex::new(Vec::new())),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::Stream(vec![hosted_tool()]),
                    Arc::new(Mutex::new(Vec::new())),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(move |from, to, reason| {
                recorded
                    .lock()
                    .unwrap()
                    .push((from, to, reason.to_string()));
                Ok(())
            }),
        )
        .unwrap();

        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::HostedTool { tool }]
                if tool.call_id == "hosted-call-1"
                    && tool.status == crate::llm::ProviderHostedToolStatus::Completed
        ));
        assert_eq!(
            *selections.lock().unwrap(),
            vec![(
                0,
                1,
                "primary_invocation_failed_automatic_fallback".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn fallback_open_after_recoverable_error_preserves_rate_limit_classification() {
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::Stream(vec![ProviderStreamEvent::RecoverableError {
                    message: "connection reset".to_string(),
                }]),
                Arc::new(Mutex::new(Vec::new())),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::StreamRateLimited,
                    Arc::new(Mutex::new(Vec::new())),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();

        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::TerminalError {
                failure: ProviderStreamFailure::RateLimited {
                    retry_after_secs: 7
                }
            }]
        ));
    }

    #[tokio::test]
    async fn fallback_open_after_empty_stream_preserves_cancellation_classification() {
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::Stream(Vec::new()),
                Arc::new(Mutex::new(Vec::new())),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::StreamCancelled,
                    Arc::new(Mutex::new(Vec::new())),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();

        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Cancelled { message }]
                if message == "user stopped fallback"
        ));
    }

    #[tokio::test]
    async fn replay_policy_tracks_the_route_that_produced_output() {
        let models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider_with_policy(
                "primary",
                Behavior::Stream(vec![chunk("primary answer")]),
                Arc::clone(&models),
                ReasoningReplayPolicy::NotRequired,
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider_with_policy(
                    "deepseek",
                    Behavior::Stream(vec![]),
                    models,
                    ReasoningReplayPolicy::RequiredOnToolCall,
                ),
                model: "deepseek-v4-pro".to_string(),
                provider_type: ProviderType::DeepSeek,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();

        assert_eq!(
            wrapper.reasoning_replay_policy("ignored"),
            ReasoningReplayPolicy::NotRequired
        );
        assert_eq!(
            wrapper.reasoning_replay_history_policy("ignored"),
            ReasoningReplayPolicy::NotRequired
        );
        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(
            wrapper.reasoning_replay_policy("ignored"),
            ReasoningReplayPolicy::NotRequired
        );
    }

    #[tokio::test]
    async fn fallback_projects_history_for_the_concrete_selected_route() {
        let primary_history = Arc::new(Mutex::new(Vec::new()));
        let fallback_history = Arc::new(Mutex::new(Vec::new()));
        let models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider_with_policy_and_history(
                "primary",
                Behavior::Stream(vec![ProviderStreamEvent::RecoverableError {
                    message: "primary unavailable".to_string(),
                }]),
                Arc::clone(&models),
                ReasoningReplayPolicy::NotRequired,
                Arc::clone(&primary_history),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider_with_policy_and_history(
                    "deepseek",
                    Behavior::Stream(vec![chunk("fallback answer")]),
                    models,
                    ReasoningReplayPolicy::RequiredOnToolCall,
                    Arc::clone(&fallback_history),
                ),
                model: "deepseek-v4-pro".to_string(),
                provider_type: ProviderType::DeepSeek,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        let mut route_request = request();
        route_request.messages = vec![
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "dependent answer"),
        ];

        let events = wrapper
            .stream_events(&route_request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 1);
        assert!(primary_history.lock().unwrap()[0]
            .iter()
            .any(|message| message.role == Role::Tool));
        let fallback_history = fallback_history.lock().unwrap();
        let fallback = &fallback_history[0];
        assert!(!fallback.iter().any(|message| message.role == Role::Tool));
        assert!(fallback
            .iter()
            .any(|message| { message.text_content().contains("Provider replay boundary") }));
        assert_eq!(
            wrapper.reasoning_replay_policy("ignored"),
            ReasoningReplayPolicy::RequiredOnToolCall
        );
    }

    #[tokio::test]
    async fn concrete_history_policy_omits_an_incompatible_prior_tool_unit() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider_with_output_and_history_policy(
                "primary",
                Behavior::Stream(vec![chunk("answer")]),
                models,
                ReasoningReplayPolicy::NotRequired,
                ReasoningReplayPolicy::RequiredOnToolCall,
                Arc::clone(&history),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            Vec::new(),
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        let mut route_request = request();
        route_request.messages = vec![
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "dependent answer"),
        ];

        let events = wrapper
            .stream_events(&route_request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 1);
        let history = history.lock().unwrap();
        assert!(!history[0].iter().any(|message| message.role == Role::Tool));
        assert!(history[0]
            .iter()
            .any(|message| message.text_content().contains("Provider replay boundary")));
    }

    #[tokio::test]
    async fn explicit_reasoning_disable_preserves_a_safe_not_required_tool_unit() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider_with_output_and_history_policy(
                "primary",
                Behavior::Stream(vec![chunk("answer")]),
                models,
                ReasoningReplayPolicy::NotRequired,
                ReasoningReplayPolicy::RequiredOnToolCall,
                Arc::clone(&history),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            Vec::new(),
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();
        let mut route_request = request();
        route_request.reasoning_enabled = Some(false);
        let route = wrapper.route_snapshot(&route_request);
        let tool_call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        };
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![tool_call.clone()]);
        assistant.set_provider_turn(crate::llm::provider_turn::ProviderTurnEnvelope::capture(
            "turn-1",
            "sample-1",
            route,
            "",
            None,
            None,
            vec![tool_call],
            false,
        ));
        route_request.messages = vec![
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "dependent answer"),
        ];

        let events = wrapper
            .stream_events(&route_request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        let mut effort_request = route_request;
        effort_request.reasoning_enabled = None;
        effort_request.reasoning_effort = Some(crate::llm::ReasoningEffort::None);
        let effort_events = wrapper
            .stream_events(&effort_request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 1);
        assert_eq!(effort_events.len(), 1);
        let history = history.lock().unwrap();
        assert_eq!(history.len(), 2);
        for invocation in history.iter() {
            assert!(invocation.iter().any(|message| message.role == Role::Tool));
            assert!(!invocation
                .iter()
                .any(|message| message.text_content().contains("Provider replay boundary")));
        }
    }

    #[tokio::test]
    async fn fallback_projects_each_route_from_the_original_history() {
        let primary_history = Arc::new(Mutex::new(Vec::new()));
        let fallback_history = Arc::new(Mutex::new(Vec::new()));
        let models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider_with_policy_and_history(
                "primary",
                Behavior::Stream(vec![ProviderStreamEvent::RecoverableError {
                    message: "primary unavailable".to_string(),
                }]),
                Arc::clone(&models),
                ReasoningReplayPolicy::RequiredOnToolCall,
                Arc::clone(&primary_history),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider_with_policy_and_history(
                    "fallback",
                    Behavior::Stream(vec![chunk("fallback answer")]),
                    models,
                    ReasoningReplayPolicy::NotRequired,
                    Arc::clone(&fallback_history),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        let mut route_request = request();
        route_request.messages = vec![
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "dependent answer"),
        ];

        let events = wrapper
            .stream_events(&route_request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 1);
        let primary_history = primary_history.lock().unwrap();
        assert!(!primary_history[0]
            .iter()
            .any(|message| message.role == Role::Tool));
        assert!(primary_history[0]
            .iter()
            .any(|message| message.text_content().contains("Provider replay boundary")));
        let fallback_history = fallback_history.lock().unwrap();
        assert!(fallback_history[0]
            .iter()
            .any(|message| message.role == Role::Tool));
        assert!(!fallback_history[0]
            .iter()
            .any(|message| message.text_content().contains("Provider replay boundary")));
    }

    #[tokio::test]
    async fn failure_after_output_never_mixes_a_fallback_stream() {
        let primary_models = Arc::new(Mutex::new(Vec::new()));
        let fallback_models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::Stream(vec![
                    chunk("visible"),
                    ProviderStreamEvent::RecoverableError {
                        message: "late reset".to_string(),
                    },
                ]),
                primary_models,
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::Stream(vec![chunk("must not appear")]),
                    Arc::clone(&fallback_models),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| panic!("fallback must not be selected after output")),
        )
        .unwrap();

        let events = wrapper
            .stream_events(&request())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[1],
            ProviderStreamEvent::RecoverableError { message } if message == "late reset"
        ));
        assert!(fallback_models.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn non_streaming_retryable_failure_uses_the_frozen_model() {
        let primary_models = Arc::new(Mutex::new(Vec::new()));
        let fallback_models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::CompleteTransient,
                Arc::clone(&primary_models),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::CompleteSuccess,
                    Arc::clone(&fallback_models),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();

        let response = wrapper.complete(&request()).await.unwrap();
        assert_eq!(response.content, "fallback-model");
        assert_eq!(*primary_models.lock().unwrap(), vec!["primary-model"]);
        assert_eq!(*fallback_models.lock().unwrap(), vec!["fallback-model"]);
    }

    #[tokio::test]
    async fn tool_loop_route_lock_forbids_cross_provider_fallback() {
        let primary_models = Arc::new(Mutex::new(Vec::new()));
        let fallback_models = Arc::new(Mutex::new(Vec::new()));
        let wrapper = AutomaticFallbackProvider::new(
            0,
            provider(
                "primary",
                Behavior::CompleteTransient,
                Arc::clone(&primary_models),
            ),
            "primary-model".to_string(),
            ProviderType::OpenAi,
            vec![AutomaticFallbackCandidate {
                fallback_index: 1,
                provider: provider(
                    "fallback",
                    Behavior::CompleteSuccess,
                    Arc::clone(&fallback_models),
                ),
                model: "fallback-model".to_string(),
                provider_type: ProviderType::OpenAi,
            }],
            Arc::new(|_, _, _| Ok(())),
        )
        .unwrap();

        let mut locked_request = request();
        let locked_route = wrapper.route_snapshot(&locked_request);
        let tool_call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        };
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![tool_call.clone()]);
        assistant.set_provider_turn(super::super::provider_turn::ProviderTurnEnvelope::capture(
            "turn-item-1",
            "sample-1",
            locked_route.clone(),
            "",
            None,
            None,
            vec![tool_call],
            false,
        ));
        let mut interrupted = Message::text(Role::Assistant, "partial draft");
        let mut interrupted_envelope = super::super::provider_turn::ProviderTurnEnvelope::capture(
            "turn-item-2",
            "sample-2",
            locked_route,
            "partial draft",
            None,
            None,
            Vec::new(),
            false,
        );
        interrupted_envelope.capture_status =
            super::super::reasoning_profile::ReasoningCaptureStatus::Interrupted;
        interrupted.set_provider_turn(interrupted_envelope);
        locked_request.messages = vec![
            Message::text(Role::User, "continue the task"),
            assistant,
            Message::text_with_name(Role::Tool, "result", "call-1"),
            interrupted,
            Message::text(Role::User, "steer the active tool loop"),
        ];

        let error = wrapper
            .complete(&locked_request)
            .await
            .expect_err("locked tool loop must fail closed on its selected route");

        assert!(matches!(error, CoreError::TransientLlm(_)));
        assert_eq!(*primary_models.lock().unwrap(), vec!["primary-model"]);
        assert!(fallback_models.lock().unwrap().is_empty());
    }
}
