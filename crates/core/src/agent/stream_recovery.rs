use std::time::Duration;

use crate::error::CoreError;

pub(super) const MAX_CONTEXT_RECOVERY_ATTEMPTS: u32 = 2;
pub(super) const MAX_STREAM_DISCONNECT_RETRIES: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StreamRecoveryDecision {
    Reconnect {
        attempt: u32,
        status_message: String,
        reset_reason: String,
        delay: Duration,
    },
    NonStreamingFallback {
        status_message: String,
        reset_reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StreamConnectRetryDecision {
    Retry {
        attempt: u32,
        delay: Duration,
        thinking_message: String,
    },
    GiveUp {
        user_message: String,
        trace_message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ContextOverflowRecoveryDecision {
    Compact {
        attempt: u32,
        status_message: String,
    },
    GiveUp {
        user_message: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) struct StreamRecoveryPolicy {
    max_context_recovery_attempts: u32,
    max_connect_retries: u32,
    max_disconnect_retries: u32,
}

impl Default for StreamRecoveryPolicy {
    fn default() -> Self {
        Self {
            max_context_recovery_attempts: MAX_CONTEXT_RECOVERY_ATTEMPTS,
            max_connect_retries: 3,
            max_disconnect_retries: MAX_STREAM_DISCONNECT_RETRIES,
        }
    }
}

impl StreamRecoveryPolicy {
    pub(super) fn is_context_overflow_error(error: &CoreError) -> bool {
        match error {
            CoreError::ContextOverflow(..) => true,
            CoreError::Llm(message) | CoreError::TransientLlm(message) => {
                let lower = message.to_lowercase();
                lower.contains("context length")
                    || lower.contains("context window")
                    || lower.contains("prompt_too_long")
                    || lower.contains("prompt is too long")
                    || lower.contains("maximum context")
                    || lower.contains("too many tokens")
                    || (lower.contains("token limit") && lower.contains("input"))
            }
            _ => false,
        }
    }

    pub(super) fn decide_after_context_overflow(
        self,
        completed_attempts: u32,
        error: &CoreError,
    ) -> ContextOverflowRecoveryDecision {
        if completed_attempts >= self.max_context_recovery_attempts {
            return ContextOverflowRecoveryDecision::GiveUp {
                user_message: format!(
                    "Context compression circuit breaker opened after {} recovery attempt(s): {}",
                    self.max_context_recovery_attempts, error
                ),
            };
        }

        let attempt = completed_attempts + 1;
        ContextOverflowRecoveryDecision::Compact {
            attempt,
            status_message: format!(
                "Context window overflow detected. Compacting history and retrying ({}/{})",
                attempt, self.max_context_recovery_attempts
            ),
        }
    }

    pub(super) fn decide_after_rate_limit(
        self,
        completed_retries: u32,
        retry_after_secs: u64,
    ) -> StreamConnectRetryDecision {
        let attempt = completed_retries + 1;
        if attempt > self.max_connect_retries {
            return StreamConnectRetryDecision::GiveUp {
                user_message: format!("Rate limited after {} retries", self.max_connect_retries),
                trace_message: "rate limited".to_string(),
            };
        }

        let wait = if retry_after_secs > 0 {
            retry_after_secs
        } else {
            2u64.pow(attempt)
        };

        StreamConnectRetryDecision::Retry {
            attempt,
            delay: Duration::from_secs(wait),
            thinking_message: format!("Rate limited. Retrying in {wait}s..."),
        }
    }

    pub(super) fn decide_after_transient_error(
        self,
        completed_retries: u32,
        detail: &str,
    ) -> StreamConnectRetryDecision {
        let attempt = completed_retries + 1;
        if attempt > self.max_connect_retries {
            let message = format!(
                "Transient error after {} retries: {}",
                self.max_connect_retries, detail
            );
            return StreamConnectRetryDecision::GiveUp {
                user_message: message.clone(),
                trace_message: message,
            };
        }

        let wait = 2u64.pow(attempt - 1);
        StreamConnectRetryDecision::Retry {
            attempt,
            delay: Duration::from_secs(wait),
            thinking_message: format!("Connection error. Retrying in {wait}s..."),
        }
    }

    pub(super) fn decide_after_incomplete(
        self,
        force_non_streaming: bool,
        completed_retries: u32,
        detail: &str,
    ) -> StreamRecoveryDecision {
        if !force_non_streaming && completed_retries < self.max_disconnect_retries {
            let attempt = completed_retries + 1;
            let reset_reason = format!(
                "Stream interrupted; reconnecting model stream ({attempt}/{}).",
                self.max_disconnect_retries
            );
            let delay = Duration::from_millis(
                250_u64.saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1))),
            );
            return StreamRecoveryDecision::Reconnect {
                attempt,
                status_message: format!("{reset_reason} ({detail})"),
                reset_reason,
                delay,
            };
        }

        let reset_reason =
            "Stream interrupted repeatedly; switching this turn to non-streaming mode.".to_string();
        StreamRecoveryDecision::NonStreamingFallback {
            status_message: format!("{reset_reason} ({detail})"),
            reset_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnects_until_disconnect_retry_budget_is_used() {
        let decision =
            StreamRecoveryPolicy::default().decide_after_incomplete(false, 0, "connection closed");

        assert_eq!(
            decision,
            StreamRecoveryDecision::Reconnect {
                attempt: 1,
                status_message:
                    "Stream interrupted; reconnecting model stream (1/2). (connection closed)"
                        .to_string(),
                reset_reason: "Stream interrupted; reconnecting model stream (1/2).".to_string(),
                delay: Duration::from_millis(250),
            }
        );
    }

    #[test]
    fn detects_context_overflow_errors_from_structured_and_provider_messages() {
        assert!(StreamRecoveryPolicy::is_context_overflow_error(
            &CoreError::ContextOverflow(10, 5)
        ));
        assert!(StreamRecoveryPolicy::is_context_overflow_error(
            &CoreError::Llm("maximum context length exceeded".to_string())
        ));
        assert!(!StreamRecoveryPolicy::is_context_overflow_error(
            &CoreError::Llm("provider refused request".to_string())
        ));
    }

    #[test]
    fn context_overflow_compacts_until_recovery_budget_is_used() {
        let compact = StreamRecoveryPolicy::default()
            .decide_after_context_overflow(1, &CoreError::ContextOverflow(10, 5));
        let give_up = StreamRecoveryPolicy::default()
            .decide_after_context_overflow(2, &CoreError::ContextOverflow(10, 5));

        assert_eq!(
            compact,
            ContextOverflowRecoveryDecision::Compact {
                attempt: 2,
                status_message:
                    "Context window overflow detected. Compacting history and retrying (2/2)"
                        .to_string(),
            }
        );
        assert_eq!(
            give_up,
            ContextOverflowRecoveryDecision::GiveUp {
                user_message:
                    "Context compression circuit breaker opened after 2 recovery attempt(s): LLM context window exceeded: 10 tokens > 5 max"
                        .to_string(),
            }
        );
    }

    #[test]
    fn rate_limit_retry_uses_retry_after_before_giving_up() {
        let retry = StreamRecoveryPolicy::default().decide_after_rate_limit(0, 7);
        let give_up = StreamRecoveryPolicy::default().decide_after_rate_limit(3, 0);

        assert_eq!(
            retry,
            StreamConnectRetryDecision::Retry {
                attempt: 1,
                delay: Duration::from_secs(7),
                thinking_message: "Rate limited. Retrying in 7s...".to_string(),
            }
        );
        assert_eq!(
            give_up,
            StreamConnectRetryDecision::GiveUp {
                user_message: "Rate limited after 3 retries".to_string(),
                trace_message: "rate limited".to_string(),
            }
        );
    }

    #[test]
    fn transient_retry_uses_exponential_backoff_before_giving_up() {
        let retry = StreamRecoveryPolicy::default().decide_after_transient_error(1, "closed");
        let give_up = StreamRecoveryPolicy::default().decide_after_transient_error(3, "closed");

        assert_eq!(
            retry,
            StreamConnectRetryDecision::Retry {
                attempt: 2,
                delay: Duration::from_secs(2),
                thinking_message: "Connection error. Retrying in 2s...".to_string(),
            }
        );
        assert_eq!(
            give_up,
            StreamConnectRetryDecision::GiveUp {
                user_message: "Transient error after 3 retries: closed".to_string(),
                trace_message: "Transient error after 3 retries: closed".to_string(),
            }
        );
    }

    #[test]
    fn switches_to_non_streaming_after_disconnect_retry_budget() {
        let decision =
            StreamRecoveryPolicy::default().decide_after_incomplete(false, 2, "connection closed");

        assert_eq!(
            decision,
            StreamRecoveryDecision::NonStreamingFallback {
                status_message:
                    "Stream interrupted repeatedly; switching this turn to non-streaming mode. (connection closed)"
                        .to_string(),
                reset_reason:
                    "Stream interrupted repeatedly; switching this turn to non-streaming mode."
                        .to_string(),
            }
        );
    }

    #[test]
    fn keeps_forced_non_streaming_mode_on_fallback_path() {
        let decision =
            StreamRecoveryPolicy::default().decide_after_incomplete(true, 0, "connection closed");

        assert!(matches!(
            decision,
            StreamRecoveryDecision::NonStreamingFallback { .. }
        ));
    }
}
