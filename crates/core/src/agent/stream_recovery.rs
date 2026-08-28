use std::time::Duration;

use crate::error::CoreError;

pub(super) const MAX_CONTEXT_RECOVERY_ATTEMPTS: u32 = 2;
pub(super) const MAX_STREAM_DISCONNECT_RETRIES: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StreamRecoveryDecision {
    StopAfterReplayBarrier {
        user_message: String,
        trace_message: String,
    },
    Reconnect {
        attempt: u32,
        status_message: String,
        reset_reason: String,
        delay: Duration,
    },
    GiveUp {
        user_message: String,
        trace_message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StreamConnectRetryDecision {
    Retry {
        attempt: u32,
        delay: Duration,
        status_message: String,
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
            max_connect_retries: 4,
            max_disconnect_retries: MAX_STREAM_DISCONNECT_RETRIES,
        }
    }
}

impl StreamRecoveryPolicy {
    pub(super) fn with_stream_max_retries(mut self, configured: Option<u32>) -> Self {
        if let Some(max_retries) = configured {
            let max_retries = max_retries.min(10);
            self.max_connect_retries = max_retries;
            self.max_disconnect_retries = max_retries;
        }
        self
    }

    pub(super) fn max_connect_retries(self) -> u32 {
        self.max_connect_retries
    }

    pub(super) fn max_disconnect_retries(self) -> u32 {
        self.max_disconnect_retries
    }

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

        let delay = if retry_after_secs > 0 {
            Duration::from_secs(retry_after_secs)
        } else {
            retry_backoff_with_jitter(attempt)
        };

        StreamConnectRetryDecision::Retry {
            attempt,
            delay,
            status_message: format!("Rate limited. Retrying in {}s...", delay.as_secs()),
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

        let delay = retry_backoff_with_jitter(attempt);
        StreamConnectRetryDecision::Retry {
            attempt,
            delay,
            status_message: format!("Connection error. Retrying in {}s...", delay.as_secs()),
        }
    }

    pub(super) fn decide_after_incomplete(
        self,
        force_non_streaming: bool,
        completed_retries: u32,
        replay_barrier_crossed: bool,
        detail: &str,
    ) -> StreamRecoveryDecision {
        if replay_barrier_crossed {
            return StreamRecoveryDecision::StopAfterReplayBarrier {
                user_message: "The provider connection ended after a provider-hosted action had already started. Nexa kept the partial trace and did not replay the request because the remote action may have side effects.".to_string(),
                trace_message: format!(
                    "provider stream interrupted after irreversible hosted action; retry suppressed: {detail}"
                ),
            };
        }
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

        StreamRecoveryDecision::GiveUp {
            user_message: "The model stream disconnected repeatedly. Partial output was preserved; retry when the provider connection is stable.".to_string(),
            trace_message: format!(
                "model stream disconnected after {} replay attempt(s): {detail}",
                self.max_disconnect_retries
            ),
        }
    }
}

fn retry_backoff_with_jitter(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(3);
    let base_seconds = 5_u64.saturating_mul(2_u64.saturating_pow(exponent));
    let jitter_ms = 137_u64.saturating_mul(u64::from(attempt));
    Duration::from_secs(base_seconds.min(40)) + Duration::from_millis(jitter_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnects_until_disconnect_retry_budget_is_used() {
        let decision = StreamRecoveryPolicy::default().decide_after_incomplete(
            false,
            0,
            false,
            "connection closed",
        );

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
    fn provider_retry_override_controls_connect_and_disconnect_budgets() {
        let policy = StreamRecoveryPolicy::default().with_stream_max_retries(Some(1));
        assert_eq!(policy.max_connect_retries(), 1);
        assert_eq!(policy.max_disconnect_retries(), 1);
        assert!(matches!(
            policy.decide_after_incomplete(false, 1, false, "closed"),
            StreamRecoveryDecision::GiveUp { .. }
        ));
        assert!(matches!(
            policy.decide_after_transient_error(1, "reset"),
            StreamConnectRetryDecision::GiveUp { .. }
        ));
    }

    #[test]
    fn provider_hosted_action_irreversibly_disables_transport_replay() {
        let decision = StreamRecoveryPolicy::default().decide_after_incomplete(
            false,
            0,
            true,
            "connection closed",
        );

        assert!(matches!(
            decision,
            StreamRecoveryDecision::StopAfterReplayBarrier { ref trace_message, .. }
                if trace_message.contains("retry suppressed")
        ));
    }

    #[test]
    fn resettable_draft_output_keeps_transport_replay_available() {
        let decision = StreamRecoveryPolicy::default().decide_after_incomplete(
            false,
            0,
            false,
            "connection closed after draft tool arguments",
        );

        assert!(matches!(
            decision,
            StreamRecoveryDecision::Reconnect { attempt: 1, .. }
        ));
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
        let give_up = StreamRecoveryPolicy::default().decide_after_rate_limit(4, 0);

        assert_eq!(
            retry,
            StreamConnectRetryDecision::Retry {
                attempt: 1,
                delay: Duration::from_secs(7),
                status_message: "Rate limited. Retrying in 7s...".to_string(),
            }
        );
        assert_eq!(
            give_up,
            StreamConnectRetryDecision::GiveUp {
                user_message: "Rate limited after 4 retries".to_string(),
                trace_message: "rate limited".to_string(),
            }
        );
    }

    #[test]
    fn transient_retry_uses_exponential_backoff_before_giving_up() {
        let retry = StreamRecoveryPolicy::default().decide_after_transient_error(1, "closed");
        let give_up = StreamRecoveryPolicy::default().decide_after_transient_error(4, "closed");

        assert_eq!(
            retry,
            StreamConnectRetryDecision::Retry {
                attempt: 2,
                delay: Duration::from_millis(10_274),
                status_message: "Connection error. Retrying in 10s...".to_string(),
            }
        );
        assert_eq!(
            give_up,
            StreamConnectRetryDecision::GiveUp {
                user_message: "Transient error after 4 retries: closed".to_string(),
                trace_message: "Transient error after 4 retries: closed".to_string(),
            }
        );
    }

    #[test]
    fn gives_control_back_after_disconnect_retry_budget() {
        let decision = StreamRecoveryPolicy::default().decide_after_incomplete(
            false,
            2,
            false,
            "connection closed",
        );

        assert_eq!(
            decision,
            StreamRecoveryDecision::GiveUp {
                user_message: "The model stream disconnected repeatedly. Partial output was preserved; retry when the provider connection is stable.".to_string(),
                trace_message:
                    "model stream disconnected after 2 replay attempt(s): connection closed"
                        .to_string(),
            }
        );
    }

    #[test]
    fn forced_non_streaming_does_not_create_a_blind_fallback_loop() {
        let decision = StreamRecoveryPolicy::default().decide_after_incomplete(
            true,
            0,
            false,
            "connection closed",
        );

        assert!(matches!(decision, StreamRecoveryDecision::GiveUp { .. }));
    }

    #[test]
    fn transient_retry_backoff_is_bounded_and_jittered() {
        let policy = StreamRecoveryPolicy::default();
        let delays = (0..4)
            .map(
                |completed| match policy.decide_after_transient_error(completed, "reset") {
                    StreamConnectRetryDecision::Retry { delay, .. } => delay,
                    StreamConnectRetryDecision::GiveUp { .. } => panic!("retry budget ended early"),
                },
            )
            .collect::<Vec<_>>();

        assert_eq!(delays[0], Duration::from_millis(5_137));
        assert_eq!(delays[1], Duration::from_millis(10_274));
        assert_eq!(delays[2], Duration::from_millis(20_411));
        assert_eq!(delays[3], Duration::from_millis(40_548));
    }
}
