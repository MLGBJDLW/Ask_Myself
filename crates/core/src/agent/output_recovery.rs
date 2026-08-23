//! Provider-neutral recovery policy for incomplete model output.
//!
//! Provider adapters normalize their terminal reasons into [`FinishReason`].
//! This module then decides whether a model step completes the user turn,
//! continues an output-limited response, proceeds to tools, or represents a
//! non-recoverable provider outcome. A per-request output limit is therefore
//! never confused with the lifetime of the user turn.

use crate::llm::FinishReason;

/// A bounded continuation budget prevents an always-on reasoner from ending
/// every physical sample at `length` forever when the surrounding agent turn
/// intentionally has no iteration cap.
const MAX_OUTPUT_LIMIT_CONTINUATIONS: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputRecoveryCause {
    OutputLimit,
    EmptyTerminal,
}

impl OutputRecoveryCause {
    pub(super) fn controller_prompt(self, had_visible_content: bool) -> &'static str {
        match (self, had_visible_content) {
            (Self::OutputLimit, true) => {
                "The provider reached its per-request output limit. Continue exactly where the visible answer stopped, without repeating prior text. Keep hidden reasoning out of the answer channel. You may use tools if the task still requires them."
            }
            (Self::OutputLimit, false) => {
                "The provider reached its per-request output limit before producing answer-channel text. Continue the unfinished task. Keep hidden reasoning separate, use tools if still needed, and finish with a concise visible answer."
            }
            (Self::EmptyTerminal, _) => {
                "The provider ended without answer-channel text. Continue the unfinished task once, keep hidden reasoning separate, and finish with a concise visible answer."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutputRecoveryFailure {
    ContentFiltered,
    OutputLimit,
    EmptyTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutputRecoveryDecision {
    Continue {
        cause: OutputRecoveryCause,
        had_visible_content: bool,
    },
    TruncatedToolRound,
    ToolRound,
    Final(String),
    Reject(OutputRecoveryFailure),
}

/// Tracks one recovery episode across as many tool and model steps as it needs.
///
/// Output-limit continuations are bounded independently from the surrounding
/// turn's iteration limit. This matters for active goals, whose model/tool loop
/// can intentionally be open-ended. Empty successful terminals get one
/// corrective continuation before the anomaly is surfaced.
#[derive(Debug, Default)]
pub(super) struct OutputRecovery {
    active: bool,
    visible_prefix: String,
    empty_terminal_retried: bool,
    output_limit_continuations: u8,
}

impl OutputRecovery {
    pub(super) fn reserves_answer_channel(&self) -> bool {
        self.active
    }

    pub(super) fn observe(
        &mut self,
        finish_reason: Option<&FinishReason>,
        content: &str,
        has_tool_calls: bool,
    ) -> OutputRecoveryDecision {
        if matches!(finish_reason, Some(FinishReason::ContentFilter)) {
            return OutputRecoveryDecision::Reject(OutputRecoveryFailure::ContentFiltered);
        }

        let has_visible_content = !content.trim().is_empty();
        if matches!(finish_reason, Some(FinishReason::Length)) {
            if self.output_limit_continuations >= MAX_OUTPUT_LIMIT_CONTINUATIONS {
                return OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit);
            }
            self.output_limit_continuations += 1;
            self.active = true;
            if has_visible_content {
                self.visible_prefix.push_str(content);
            }
            if has_tool_calls {
                return OutputRecoveryDecision::TruncatedToolRound;
            }
            return OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                had_visible_content: has_visible_content,
            };
        }

        if has_tool_calls {
            return OutputRecoveryDecision::ToolRound;
        }

        if has_visible_content {
            let mut final_content = std::mem::take(&mut self.visible_prefix);
            final_content.push_str(content);
            self.active = false;
            self.empty_terminal_retried = false;
            self.output_limit_continuations = 0;
            return OutputRecoveryDecision::Final(final_content);
        }

        if !self.empty_terminal_retried {
            self.active = true;
            self.empty_terminal_retried = true;
            return OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::EmptyTerminal,
                had_visible_content: false,
            };
        }

        OutputRecoveryDecision::Reject(OutputRecoveryFailure::EmptyTerminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_limit_recovery_remains_active_across_tools() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                ..
            }
        ));
        assert!(recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::ToolCalls), "", true),
            OutputRecoveryDecision::ToolRound
        );
        assert!(recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "done", false),
            OutputRecoveryDecision::Final("done".to_string())
        );
        assert!(!recovery.reserves_answer_channel());
    }

    #[test]
    fn output_limit_marks_tool_calls_as_truncated_and_reserves_answer_channel() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "partial answer; ", true),
            OutputRecoveryDecision::TruncatedToolRound
        );
        assert!(recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "finished", false),
            OutputRecoveryDecision::Final("partial answer; finished".to_string())
        );
    }

    #[test]
    fn output_limit_continuations_are_joined_but_bounded() {
        let mut recovery = OutputRecovery::default();
        for fragment in ["one ", "two "] {
            assert!(matches!(
                recovery.observe(Some(&FinishReason::Length), fragment, false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    ..
                }
            ));
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "three ", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit)
        );

        let mut completed = OutputRecovery::default();
        assert!(matches!(
            completed.observe(Some(&FinishReason::Length), "one ", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        assert_eq!(
            completed.observe(Some(&FinishReason::Stop), "two", false),
            OutputRecoveryDecision::Final("one two".to_string())
        );
    }

    #[test]
    fn empty_stop_gets_one_protocol_recovery_but_filter_never_retries() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Stop), "", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::EmptyTerminal,
                ..
            }
        ));
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::EmptyTerminal)
        );

        let mut filtered = OutputRecovery::default();
        assert_eq!(
            filtered.observe(Some(&FinishReason::ContentFilter), "", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::ContentFiltered)
        );
    }
}
