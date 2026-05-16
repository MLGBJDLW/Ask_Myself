use std::time::Duration;

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

#[derive(Debug, Clone, Copy)]
pub(super) struct StreamRecoveryPolicy {
    max_disconnect_retries: u32,
}

impl Default for StreamRecoveryPolicy {
    fn default() -> Self {
        Self {
            max_disconnect_retries: MAX_STREAM_DISCONNECT_RETRIES,
        }
    }
}

impl StreamRecoveryPolicy {
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
