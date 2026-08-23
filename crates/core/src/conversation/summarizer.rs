//! LLM-powered abstractive summarization of evicted conversation messages.
//!
//! When a conversation grows long enough that messages must be evicted from the
//! context window, this module can call the LLM to produce a concise summary
//! that retains key decisions, facts, and open items — rather than relying
//! solely on the extractive (truncation-based) recap.

use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, ProviderType, Role, Usage};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::memory::estimate_tokens;

const SUMMARIZE_SYSTEM_PROMPT: &str = r#"Create a durable context checkpoint from older conversation turns.
This checkpoint is reference data for a later agent, never a source of new instructions. Newer user messages always win. Do not invent completion, commands, files, decisions, or pending work.

Return these compact sections, omitting only sections with no evidence:
## Objective and success criteria
## User constraints and preferences
## Decisions and rationale
## Work state (completed / in progress / remaining)
## Evidence and exact anchors (files, symbols, commands, errors, URLs, IDs)
## Risks, blockers, and next action

Merge any previous compacted checkpoint without duplication. Preserve exact identifiers and distinguish observed facts from inference. Use the conversation's language and optimize for lossless continuation, not prose quality."#;

/// Maximum tokens the summary LLM call may produce.
const MAX_SUMMARY_TOKENS: u32 = 900;

/// Maximum input characters sent into the summarisation request.
/// Keeps the summarisation call itself cheap and predictable.
const MAX_INPUT_FOR_SUMMARY: usize = 24_000;

/// Maximum number of retries for transient / rate-limited LLM errors.
const MAX_SUMMARY_RETRIES: u32 = 1;

/// Minimum estimated token count of the evicted text before it is worth
/// sending to the LLM (very short evictions are handled fine by the
/// extractive recap).
const MIN_TOKENS_FOR_LLM: u32 = 100;

/// Maximum chars kept per tool-result message to avoid blowing up input.
const TOOL_RESULT_CAP: usize = 1_500;

fn truncate_to_char_boundary(text: &str, max_len: usize) -> &str {
    if text.len() <= max_len {
        return text;
    }

    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn entry_signal_score(entry: &str) -> usize {
    let lower = entry.to_ascii_lowercase();
    let anchors = [
        "user:", "decision", "require", "must", "should", "todo", "pending", "error", "failed",
        "block", "risk", "file", "path", "commit", "test", "http://", "https://", "\\", "/", "::",
        "`",
    ];
    anchors
        .iter()
        .filter(|needle| lower.contains(**needle))
        .count()
}

/// Select whole transcript entries across the entire evicted range. This
/// avoids the old head/tail truncation losing the middle decisions and tool
/// evidence that are usually most important for resuming agent work.
fn fit_entries_to_budget(entries: &[String], max_len: usize) -> String {
    if entries.iter().map(|entry| entry.len() + 1).sum::<usize>() <= max_len {
        return entries.join("\n");
    }

    let mut selected = vec![false; entries.len()];
    for keep in selected.iter_mut().take(entries.len().min(2)) {
        *keep = true;
    }
    for keep in selected.iter_mut().skip(entries.len().saturating_sub(6)) {
        *keep = true;
    }

    let mut candidates = entries
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected[*index])
        .map(|(index, entry)| (index, entry_signal_score(entry)))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(index, score)| (std::cmp::Reverse(*score), *index));

    let mut used = selected
        .iter()
        .enumerate()
        .filter(|(_, keep)| **keep)
        .map(|(index, _)| entries[index].len() + 1)
        .sum::<usize>();
    for (index, _) in candidates {
        let entry_len = entries[index].len() + 1;
        if used + entry_len <= max_len {
            selected[index] = true;
            used += entry_len;
        }
    }

    let mut output = String::new();
    let mut omitted = false;
    for (index, entry) in entries.iter().enumerate() {
        if selected[index] {
            if omitted {
                output.push_str("[...lower-signal entries omitted; chronology continues...]\n");
                omitted = false;
            }
            output.push_str(entry);
            output.push('\n');
        } else {
            omitted = true;
        }
    }
    output.trim_end().to_string()
}

#[derive(Debug, Clone)]
pub struct SummarizationResult {
    pub summary: String,
    /// Present only when an LLM request completed. Extractive fallbacks do not
    /// pretend to have consumed provider tokens.
    pub usage: Option<Usage>,
    /// Number of physical provider attempts started for this logical summary.
    /// Failed or timed-out attempts often have no provider usage payload, so
    /// the caller uses this count for conservative run-budget accounting.
    pub attempts: u32,
    pub control: ControlledSummarization,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlledSummarization {
    Abstractive,
    ExtractiveFallback { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub struct SummarizationControlPolicy {
    pub attempt_timeout: Duration,
    pub max_retries: u32,
}

#[derive(Debug)]
pub struct SummarizationFailure {
    pub error: CoreError,
    /// Physical provider attempts already started before cancellation/error.
    pub attempts: u32,
}

impl std::fmt::Display for SummarizationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for SummarizationFailure {}

impl From<SummarizationFailure> for CoreError {
    fn from(failure: SummarizationFailure) -> Self {
        failure.error
    }
}

impl Default for SummarizationControlPolicy {
    fn default() -> Self {
        Self {
            attempt_timeout: Duration::from_secs(45),
            max_retries: MAX_SUMMARY_RETRIES,
        }
    }
}

/// Conservative token reservation for one physical summarization request.
/// This lets delegated workers admit compaction through the same cumulative
/// token ledger as their normal model steps instead of spending off-ledger.
pub fn summarization_attempt_token_reservation(evicted_messages: &[Message]) -> u32 {
    let entries = build_conversation_entries(evicted_messages);
    if entries.is_empty() {
        return 0;
    }
    let input = fit_entries_to_budget(&entries, MAX_INPUT_FOR_SUMMARY);
    if estimate_tokens(&input) < MIN_TOKENS_FOR_LLM {
        return 0;
    }
    estimate_tokens(SUMMARIZE_SYSTEM_PROMPT)
        .saturating_add(estimate_tokens(&input))
        .saturating_add(MAX_SUMMARY_TOKENS)
}

pub const fn maximum_summarization_attempts() -> u32 {
    MAX_SUMMARY_RETRIES + 1
}

/// Summarise evicted messages and retain the invocation-level provider usage.
pub async fn summarize_evicted_messages_with_usage(
    provider: &dyn LlmProvider,
    model: &str,
    provider_type: Option<ProviderType>,
    evicted_messages: &[Message],
    extractive_fallback: &str,
) -> SummarizationResult {
    let cancellation = CancellationToken::new();
    summarize_evicted_messages_with_controls(
        provider,
        model,
        provider_type,
        evicted_messages,
        extractive_fallback,
        &cancellation,
        Instant::now() + Duration::from_secs(75),
        SummarizationControlPolicy::default(),
    )
    .await
    .unwrap_or_else(|error| SummarizationResult {
        summary: extractive_fallback.to_string(),
        usage: None,
        attempts: 0,
        control: ControlledSummarization::ExtractiveFallback {
            reason: error.to_string(),
        },
    })
}

/// Summarise with an abortable provider future and an operation-wide deadline.
/// Dropping the timed-out future prevents a provider request from continuing
/// invisibly after the maintenance operation has selected its fallback.
#[allow(clippy::too_many_arguments)]
pub async fn summarize_evicted_messages_with_controls(
    provider: &dyn LlmProvider,
    model: &str,
    provider_type: Option<ProviderType>,
    evicted_messages: &[Message],
    extractive_fallback: &str,
    cancellation: &CancellationToken,
    deadline: Instant,
    policy: SummarizationControlPolicy,
) -> Result<SummarizationResult, SummarizationFailure> {
    let entries = build_conversation_entries(evicted_messages);
    if entries.is_empty() {
        return Ok(SummarizationResult {
            summary: extractive_fallback.to_string(),
            usage: None,
            attempts: 0,
            control: ControlledSummarization::ExtractiveFallback {
                reason: "empty_summary_input".to_string(),
            },
        });
    }
    let input = fit_entries_to_budget(&entries, MAX_INPUT_FOR_SUMMARY);
    if estimate_tokens(&input) < MIN_TOKENS_FOR_LLM {
        return Ok(SummarizationResult {
            summary: extractive_fallback.to_string(),
            usage: None,
            attempts: 0,
            control: ControlledSummarization::ExtractiveFallback {
                reason: "input_below_threshold".to_string(),
            },
        });
    }

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, SUMMARIZE_SYSTEM_PROMPT),
            Message::text(
                Role::User,
                format!(
                    "Build the context checkpoint from this chronological section:\n\n{}",
                    input
                ),
            ),
        ],
        max_tokens: Some(MAX_SUMMARY_TOKENS),
        temperature: Some(0.1),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_enabled: None,
        reasoning_effort: None,
        provider_type,
        routing_session_id: None,
        parallel_tool_calls: true,
    };

    let mut retry_count = 0u32;
    let mut attempts = 0u32;
    let max_retries = policy.max_retries.min(1);
    loop {
        if cancellation.is_cancelled() {
            return Err(SummarizationFailure {
                error: CoreError::Cancelled("Context summarization was cancelled".to_string()),
                attempts,
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(fallback(
                extractive_fallback,
                "total_deadline_exceeded",
                attempts,
            ));
        }
        let attempt_timeout = policy.attempt_timeout.min(remaining);
        attempts = attempts.saturating_add(1);
        let attempt = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(SummarizationFailure {
                    error: CoreError::Cancelled(
                        "Context summarization was cancelled".to_string(),
                    ),
                    attempts,
                });
            }
            result = tokio::time::timeout(attempt_timeout, provider.complete(&request)) => result,
        };
        let response = match attempt {
            Ok(response) => response,
            Err(_) => {
                return Ok(fallback(
                    extractive_fallback,
                    "provider_attempt_timed_out",
                    attempts,
                ));
            }
        };
        match response {
            Ok(response) => {
                let text = response.content.trim();
                let summary = if text.is_empty() {
                    extractive_fallback.to_string()
                } else {
                    text.to_string()
                };
                return Ok(SummarizationResult {
                    summary,
                    usage: Some(response.usage),
                    attempts,
                    control: ControlledSummarization::Abstractive,
                });
            }
            Err(CoreError::RateLimited { retry_after_secs }) => {
                retry_count += 1;
                if retry_count > max_retries {
                    warn!(
                        "Summarizer: rate limited after {} retries, falling back to extractive recap",
                        max_retries
                    );
                    return Ok(fallback(extractive_fallback, "rate_limited", attempts));
                }
                let wait = if retry_after_secs > 0 {
                    retry_after_secs
                } else {
                    2u64.pow(retry_count)
                };
                warn!(
                    "Summarizer: rate limited, retry {}/{} after {}s",
                    retry_count, max_retries, wait
                );
                if !controlled_wait(cancellation, deadline, Duration::from_secs(wait))
                    .await
                    .map_err(|error| SummarizationFailure { error, attempts })?
                {
                    return Ok(fallback(
                        extractive_fallback,
                        "total_deadline_exceeded",
                        attempts,
                    ));
                }
            }
            Err(CoreError::TransientLlm(msg)) => {
                retry_count += 1;
                if retry_count > max_retries {
                    warn!(
                        "Summarizer: transient error after {} retries: {}, falling back to extractive recap",
                        max_retries, msg
                    );
                    return Ok(fallback(
                        extractive_fallback,
                        "transient_provider_error",
                        attempts,
                    ));
                }
                let wait = 2u64.pow(retry_count - 1); // 1s, 2s
                warn!(
                    "Summarizer: transient error (retry {}/{}): {}. Retrying after {}s",
                    retry_count, max_retries, msg, wait
                );
                if !controlled_wait(cancellation, deadline, Duration::from_secs(wait))
                    .await
                    .map_err(|error| SummarizationFailure { error, attempts })?
                {
                    return Ok(fallback(
                        extractive_fallback,
                        "total_deadline_exceeded",
                        attempts,
                    ));
                }
            }
            Err(e) => {
                // Non-retryable error (auth, bad request, etc.)
                warn!("Summarizer: non-retryable error: {e}, falling back to extractive recap");
                return Ok(fallback(
                    extractive_fallback,
                    "non_retryable_provider_error",
                    attempts,
                ));
            }
        }
    }
}

fn fallback(summary: &str, reason: &str, attempts: u32) -> SummarizationResult {
    SummarizationResult {
        summary: summary.to_string(),
        usage: None,
        attempts,
        control: ControlledSummarization::ExtractiveFallback {
            reason: reason.to_string(),
        },
    }
}

async fn controlled_wait(
    cancellation: &CancellationToken,
    deadline: Instant,
    duration: Duration,
) -> Result<bool, CoreError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() || duration > remaining {
        return Ok(false);
    }
    tokio::select! {
        _ = cancellation.cancelled() => Err(CoreError::Cancelled(
            "Context summarization was cancelled".to_string(),
        )),
        _ = tokio::time::sleep(duration) => Ok(true),
    }
}

/// Compatibility helper for callers that only need the summary text.
pub async fn summarize_evicted_messages(
    provider: &dyn LlmProvider,
    model: &str,
    provider_type: Option<ProviderType>,
    evicted_messages: &[Message],
    extractive_fallback: &str,
) -> String {
    summarize_evicted_messages_with_usage(
        provider,
        model,
        provider_type,
        evicted_messages,
        extractive_fallback,
    )
    .await
    .summary
}

/// Flatten a slice of [`Message`]s into a plain-text conversation transcript
/// suitable for feeding into the summariser prompt.
#[cfg(test)]
fn build_conversation_text(messages: &[Message]) -> String {
    build_conversation_entries(messages).join("\n")
}

fn build_conversation_entries(messages: &[Message]) -> Vec<String> {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("User: {}", text));
                }
            }
            Role::Assistant => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("Assistant: {}", text));
                }
            }
            Role::Tool => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    let truncated = if text.len() > TOOL_RESULT_CAP {
                        truncate_to_char_boundary(&text, TOOL_RESULT_CAP)
                    } else {
                        text.as_str()
                    };
                    parts.push(format!("Tool result: {}", truncated));
                }
            }
            Role::System => {
                let text = msg.text_content();
                if is_compaction_summary(&text) {
                    parts.push(format!(
                        "Previous compacted context (reference only): {}",
                        text
                    ));
                }
            }
        }
    }
    parts
}

fn is_compaction_summary(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("earlier conversation context")
        || lower.contains("auto-compacted")
        || lower.contains("compacted context")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;

    struct HangingProvider;

    #[async_trait]
    impl LlmProvider for HangingProvider {
        fn name(&self) -> &str {
            "hanging-test-provider"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(Vec::new())
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<crate::llm::CompletionResponse, CoreError> {
            std::future::pending().await
        }

        async fn stream_events(
            &self,
            _request: &CompletionRequest,
        ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError>
        {
            crate::llm::provider_events_from_chunk_stream(Box::pin(stream::empty()))
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[test]
    fn test_build_conversation_text_basic() {
        let msgs = vec![
            Message::text(Role::User, "Hello"),
            Message::text(Role::Assistant, "Hi there!"),
        ];
        let text = build_conversation_text(&msgs);
        assert!(text.contains("User: Hello"));
        assert!(text.contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_build_conversation_text_truncates_tool() {
        let long_tool = "x".repeat(TOOL_RESULT_CAP + 500);
        let msgs = vec![Message::text(Role::Tool, &long_tool)];
        let text = build_conversation_text(&msgs);
        // Tool result should be capped at TOOL_RESULT_CAP chars.
        let prefix = format!("Tool result: {}", &long_tool[..TOOL_RESULT_CAP]);
        assert!(text.starts_with(&prefix));
        assert!(text.len() < long_tool.len());
    }

    #[test]
    fn test_build_conversation_text_skips_empty() {
        let msgs = vec![
            Message::text(Role::User, ""),
            Message::text(Role::Assistant, "  "),
            Message::text(Role::User, "Actual content"),
        ];
        let text = build_conversation_text(&msgs);
        assert_eq!(text, "User: Actual content");
    }

    #[test]
    fn test_build_conversation_text_preserves_prior_compaction_summary() {
        let msgs = vec![
            Message::text(
                Role::System,
                "## Earlier conversation context (summarized)\nDecision: keep compact visible.",
            ),
            Message::text(Role::System, "You are a helpful assistant."),
            Message::text(Role::User, "Continue"),
        ];
        let text = build_conversation_text(&msgs);
        assert!(text.contains("Previous compacted context"));
        assert!(text.contains("Decision: keep compact visible."));
        assert!(!text.contains("You are a helpful assistant."));
        assert!(text.contains("User: Continue"));
    }

    #[test]
    fn budget_selection_preserves_high_signal_middle_entries() {
        let entries = vec![
            "User: start".to_string(),
            "Assistant: ordinary filler one".to_string(),
            "Assistant: ordinary filler two".to_string(),
            "Tool result: error: build failed at crates/core/src/agent.rs".to_string(),
            "Assistant: ordinary filler three".to_string(),
            "Assistant: tail one".to_string(),
            "Assistant: tail two".to_string(),
            "Assistant: tail three".to_string(),
            "Assistant: tail four".to_string(),
            "Assistant: tail five".to_string(),
            "Assistant: tail six".to_string(),
        ];
        let fitted = fit_entries_to_budget(&entries, 260);
        assert!(fitted.contains("User: start"));
        assert!(fitted.contains("build failed"));
        assert!(fitted.contains("tail six"));
    }

    #[tokio::test]
    async fn hanging_provider_uses_bounded_extractive_fallback() {
        let cancellation = CancellationToken::new();
        let messages = vec![Message::text(Role::User, "important context ".repeat(500))];
        let started = Instant::now();
        let result = summarize_evicted_messages_with_controls(
            &HangingProvider,
            "test-model",
            None,
            &messages,
            "deterministic fallback",
            &cancellation,
            Instant::now() + Duration::from_secs(2),
            SummarizationControlPolicy {
                attempt_timeout: Duration::from_millis(25),
                max_retries: 1,
            },
        )
        .await
        .expect("timeout should select fallback");

        // Tokenization is synchronous and can be comparatively slow on debug
        // builds; the provider wait itself remains capped at 25 ms.
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(result.summary, "deterministic fallback");
        assert_eq!(
            result.control,
            ControlledSummarization::ExtractiveFallback {
                reason: "provider_attempt_timed_out".to_string(),
            }
        );
        assert!(result.usage.is_none());
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn cancellation_aborts_provider_before_fallback_commit() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let messages = vec![Message::text(Role::User, "important context ".repeat(500))];
        let error = summarize_evicted_messages_with_controls(
            &HangingProvider,
            "test-model",
            None,
            &messages,
            "deterministic fallback",
            &cancellation,
            Instant::now() + Duration::from_secs(1),
            SummarizationControlPolicy {
                attempt_timeout: Duration::from_millis(100),
                max_retries: 0,
            },
        )
        .await
        .expect_err("cancellation must win");

        assert!(matches!(error.error, CoreError::Cancelled(_)));
        assert_eq!(error.attempts, 0);
    }

    #[tokio::test]
    async fn cancellation_after_request_start_reports_the_unbilled_attempt() {
        let cancellation = CancellationToken::new();
        let cancel_after_start = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_after_start.cancel();
        });
        let messages = vec![Message::text(Role::User, "important context ".repeat(500))];
        let error = summarize_evicted_messages_with_controls(
            &HangingProvider,
            "test-model",
            None,
            &messages,
            "deterministic fallback",
            &cancellation,
            Instant::now() + Duration::from_secs(1),
            SummarizationControlPolicy {
                attempt_timeout: Duration::from_secs(1),
                max_retries: 0,
            },
        )
        .await
        .expect_err("in-flight cancellation must remain terminal");

        assert!(matches!(error.error, CoreError::Cancelled(_)));
        assert_eq!(error.attempts, 1);
    }
}
