//! Provider-neutral recovery policy for incomplete model output.
//!
//! Provider adapters normalize their terminal reasons into [`FinishReason`].
//! This module then decides whether a model step completes the user turn,
//! continues an output-limited response, proceeds to tools, or represents a
//! non-recoverable provider outcome. A per-request output limit is therefore
//! never confused with the lifetime of the user turn.

use crate::llm::FinishReason;

/// Output recovery stops only after repeated samples make no answer or tool
/// progress. Novel continuation text and verified tool rounds reset this
/// liveness streak; they are not counted against an arbitrary turn-wide cap.
const CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD: u8 = 3;

/// Return the byte length of the longest suffix of `existing` that is also a
/// prefix of `fragment`. Scanning only the relevant tail keeps the work linear
/// in the provider fragment instead of the full accumulated answer.
fn longest_suffix_prefix_overlap(existing: &str, fragment: &str) -> usize {
    let pattern = fragment.as_bytes();
    if pattern.is_empty() || existing.is_empty() {
        return 0;
    }

    let mut prefix_lengths = vec![0; pattern.len()];
    let mut matched = 0;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = prefix_lengths[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
            prefix_lengths[index] = matched;
        }
    }

    matched = 0;
    let tail_start = existing.len().saturating_sub(pattern.len());
    let tail = &existing.as_bytes()[tail_start..];
    for (index, byte) in tail.iter().copied().enumerate() {
        while matched > 0 && byte != pattern[matched] {
            matched = prefix_lengths[matched - 1];
        }
        if byte == pattern[matched] {
            matched += 1;
            if matched == pattern.len() {
                if index + 1 == tail.len() {
                    return matched;
                }
                matched = prefix_lengths[matched - 1];
            }
        }
    }
    debug_assert!(fragment.is_char_boundary(matched));
    matched
}

fn append_with_overlap(existing: &mut String, fragment: &str) -> bool {
    let overlap = longest_suffix_prefix_overlap(existing, fragment);
    let overlap = if fragment.is_char_boundary(overlap) {
        overlap
    } else {
        0
    };
    let novel = &fragment[overlap..];
    if novel.trim().is_empty() {
        return false;
    }
    existing.push_str(novel);
    true
}

/// Append only answer text that has not already been returned during this
/// recovery episode. The containment check catches repeated and alternating
/// provider fragments; suffix/prefix overlap removes normal boundary replay.
fn append_novel_continuation(existing: &mut String, fragment: &str) -> bool {
    if fragment.trim().is_empty() || (!existing.is_empty() && existing.contains(fragment)) {
        return false;
    }
    append_with_overlap(existing, fragment)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputRecoveryCause {
    OutputLimit,
    EmptyTerminal,
    ProviderPause,
    ContextLimit,
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
            (Self::ProviderPause, _) => {
                "The provider paused its server-side tool turn. Resume from the committed provider state, do not repeat completed local tools, and continue the unfinished task."
            }
            (Self::ContextLimit, _) => {
                "The provider reached the model context limit. Continue after context rollover, preserve committed tool results, and finish the unfinished task without repeating side effects."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutputRecoveryFailure {
    ContentFiltered,
    OutputLimit,
    EmptyTerminal,
    MalformedToolCall,
    ProtocolIncomplete,
    UnsupportedTerminal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolRoundRejectionCause {
    OutputLimit,
    ProviderPause,
    ContextLimit,
    MalformedToolCall,
    ProtocolIncomplete,
    ToolsSuppressed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutputRecoveryDecision {
    Continue {
        cause: OutputRecoveryCause,
        had_visible_content: bool,
    },
    RejectToolRound(ToolRoundRejectionCause),
    ToolRound,
    Final(String),
    Reject(OutputRecoveryFailure),
}

/// Tracks one recovery episode across as many tool and model steps as it needs.
///
/// Output-limit continuations are progress-based and independent from the
/// surrounding tool-round budget. Empty successful terminals get one
/// corrective continuation before the anomaly is surfaced.
#[derive(Debug, Default)]
pub(super) struct OutputRecovery {
    active: bool,
    visible_prefix: String,
    empty_terminal_retried: bool,
    consecutive_no_progress_output_limits: u8,
    consecutive_empty_resumable_terminals: u8,
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

        if let Some(FinishReason::Unknown(reason)) = finish_reason {
            return OutputRecoveryDecision::Reject(OutputRecoveryFailure::UnsupportedTerminal(
                reason.clone(),
            ));
        }

        if matches!(finish_reason, Some(FinishReason::MalformedToolCall)) {
            return if has_tool_calls {
                OutputRecoveryDecision::RejectToolRound(ToolRoundRejectionCause::MalformedToolCall)
            } else {
                OutputRecoveryDecision::Reject(OutputRecoveryFailure::MalformedToolCall)
            };
        }

        if matches!(
            finish_reason,
            Some(FinishReason::ProtocolIncomplete | FinishReason::Other)
        ) {
            return if has_tool_calls {
                OutputRecoveryDecision::RejectToolRound(ToolRoundRejectionCause::ProtocolIncomplete)
            } else {
                OutputRecoveryDecision::Reject(OutputRecoveryFailure::ProtocolIncomplete)
            };
        }

        let has_visible_content = !content.trim().is_empty();
        if matches!(finish_reason, Some(FinishReason::Length)) {
            if has_tool_calls {
                // A length-terminated tool envelope is discarded in full. Its
                // adjacent prose is a draft from the same rejected sample and
                // must not leak back into the eventual answer.
                return OutputRecoveryDecision::RejectToolRound(
                    ToolRoundRejectionCause::OutputLimit,
                );
            }

            self.active = true;
            self.consecutive_empty_resumable_terminals = 0;
            let made_visible_progress =
                has_visible_content && append_novel_continuation(&mut self.visible_prefix, content);
            if made_visible_progress {
                self.consecutive_no_progress_output_limits = 0;
            } else {
                self.consecutive_no_progress_output_limits =
                    self.consecutive_no_progress_output_limits.saturating_add(1);
                if self.consecutive_no_progress_output_limits
                    >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD
                {
                    return OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit);
                }
            }
            return OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                had_visible_content: made_visible_progress,
            };
        }

        let resumable_terminal = match finish_reason {
            Some(FinishReason::ProviderPause) => Some(OutputRecoveryCause::ProviderPause),
            Some(FinishReason::ContextLimit) => Some(OutputRecoveryCause::ContextLimit),
            _ => None,
        };
        if let Some(cause) = resumable_terminal {
            if has_tool_calls {
                return OutputRecoveryDecision::RejectToolRound(match cause {
                    OutputRecoveryCause::ProviderPause => ToolRoundRejectionCause::ProviderPause,
                    OutputRecoveryCause::ContextLimit => ToolRoundRejectionCause::ContextLimit,
                    OutputRecoveryCause::OutputLimit | OutputRecoveryCause::EmptyTerminal => {
                        unreachable!("only resumable provider terminals reach this branch")
                    }
                });
            }
            self.active = true;
            self.consecutive_no_progress_output_limits = 0;
            if has_visible_content {
                self.consecutive_empty_resumable_terminals = 0;
                self.visible_prefix.push_str(content);
            } else {
                self.consecutive_empty_resumable_terminals =
                    self.consecutive_empty_resumable_terminals.saturating_add(1);
                if self.consecutive_empty_resumable_terminals
                    >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD
                {
                    return OutputRecoveryDecision::Reject(
                        OutputRecoveryFailure::ProtocolIncomplete,
                    );
                }
            }
            return OutputRecoveryDecision::Continue {
                cause,
                had_visible_content: has_visible_content,
            };
        }

        if has_tool_calls {
            self.consecutive_no_progress_output_limits = 0;
            self.consecutive_empty_resumable_terminals = 0;
            return OutputRecoveryDecision::ToolRound;
        }

        if has_visible_content {
            let mut final_content = std::mem::take(&mut self.visible_prefix);
            append_with_overlap(&mut final_content, content);
            self.active = false;
            self.empty_terminal_retried = false;
            self.consecutive_no_progress_output_limits = 0;
            self.consecutive_empty_resumable_terminals = 0;
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
    fn truncated_tool_round_does_not_reintroduce_its_discarded_visible_draft() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "partial answer; ", true),
            OutputRecoveryDecision::RejectToolRound(ToolRoundRejectionCause::OutputLimit)
        );
        assert!(!recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "finished", false),
            OutputRecoveryDecision::Final("finished".to_string())
        );
    }

    #[test]
    fn output_limit_continuations_with_visible_progress_are_joined() {
        let mut recovery = OutputRecovery::default();
        for fragment in ["one ", "two ", "three "] {
            assert!(matches!(
                recovery.observe(Some(&FinishReason::Length), fragment, false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    ..
                }
            ));
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "four", false),
            OutputRecoveryDecision::Final("one two three four".to_string())
        );
    }

    #[test]
    fn repeated_non_empty_output_limit_fragments_stall_without_growing_the_answer() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "same fragment", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                had_visible_content: true,
            }
        );
        for _ in 0..CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD - 1 {
            assert_eq!(
                recovery.observe(Some(&FinishReason::Length), "same fragment", false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    had_visible_content: false,
                }
            );
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "same fragment", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit)
        );
        assert_eq!(recovery.visible_prefix, "same fragment");
    }

    #[test]
    fn overlapping_utf8_continuations_append_only_novel_suffixes() {
        let mut recovery = OutputRecovery::default();
        for fragment in ["开始你好世界", "世界继续前进"] {
            assert!(matches!(
                recovery.observe(Some(&FinishReason::Length), fragment, false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    had_visible_content: true,
                }
            ));
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "继续前进完成", false),
            OutputRecoveryDecision::Final("开始你好世界继续前进完成".to_string())
        );
    }

    #[test]
    fn successful_tool_progress_resets_empty_output_limit_stalls() {
        let mut recovery = OutputRecovery::default();
        for _ in 0..3 {
            assert!(matches!(
                recovery.observe(Some(&FinishReason::Length), "", false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    ..
                }
            ));
            assert_eq!(
                recovery.observe(Some(&FinishReason::ToolCalls), "", true),
                OutputRecoveryDecision::ToolRound
            );
        }
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

    #[test]
    fn provider_pause_and_context_limit_are_typed_continuations() {
        let mut paused = OutputRecovery::default();
        assert!(matches!(
            paused.observe(Some(&FinishReason::ProviderPause), "", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ProviderPause,
                ..
            }
        ));

        let mut context = OutputRecovery::default();
        assert!(matches!(
            context.observe(Some(&FinishReason::ContextLimit), "partial", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ContextLimit,
                had_visible_content: true,
            }
        ));
    }

    #[test]
    fn malformed_and_unknown_terminals_never_become_success() {
        let mut malformed = OutputRecovery::default();
        assert_eq!(
            malformed.observe(Some(&FinishReason::MalformedToolCall), "partial", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::MalformedToolCall)
        );

        let mut unknown = OutputRecovery::default();
        assert_eq!(
            unknown.observe(
                Some(&FinishReason::Unknown("future_reason".to_string())),
                "apparently complete",
                false,
            ),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::UnsupportedTerminal(
                "future_reason".to_string()
            ))
        );
    }
}
