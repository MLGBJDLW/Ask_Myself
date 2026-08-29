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

const CONTINUATION_ACK_PREFIX: &str = "<nexa-continuation-ack:";

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

fn append_with_overlap(existing: &mut String, fragment: &str) -> String {
    let overlap = longest_suffix_prefix_overlap(existing, fragment);
    let overlap = if fragment.is_char_boundary(overlap) {
        overlap
    } else {
        0
    };
    let novel = &fragment[overlap..];
    if novel.is_empty() {
        return String::new();
    }
    existing.push_str(novel);
    novel.to_string()
}

fn strip_expected_continuation_ack<'a>(
    content: &'a str,
    expected_ack: Option<&str>,
) -> (&'a str, bool) {
    let Some(expected_ack) = expected_ack else {
        return (content, false);
    };
    let candidate = content.trim_start();
    let Some(rest) = candidate.strip_prefix(expected_ack) else {
        return (content, false);
    };
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    (rest, true)
}

#[derive(Debug, PartialEq, Eq)]
enum ContinuationCommit {
    Committed(String),
    CommittedWhitespace(String),
    AmbiguousReplay,
    Empty,
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
        visible_delta: String,
    },
    RejectToolRound {
        cause: ToolRoundRejectionCause,
        committed_progress: bool,
    },
    ToolRound {
        visible_delta: String,
    },
    Final {
        content: String,
        visible_delta: String,
    },
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
    pending_ambiguous_fragments: Vec<String>,
    expected_continuation_ack: Option<String>,
    next_continuation_ack: u32,
    empty_terminal_retried: bool,
    consecutive_no_progress_output_limits: u8,
    consecutive_no_progress_resumable_terminals: u8,
    last_provider_pause_state: Option<String>,
}

impl OutputRecovery {
    pub(super) fn reserves_answer_channel(&self) -> bool {
        self.active
    }

    pub(super) fn has_visible_content(&self) -> bool {
        !self.visible_prefix.trim().is_empty()
    }

    /// Arm a request-specific acknowledgement for the next continuation.
    ///
    /// Stateless completion APIs do not expose an output cursor, so an exact
    /// repeated fragment is ambiguous: it may be a replay or legitimate new
    /// text. The acknowledgement gives cooperative providers an explicit
    /// exactly-once signal without making text occurrence itself authoritative.
    pub(super) fn controller_prompt(
        &mut self,
        cause: OutputRecoveryCause,
        had_visible_content: bool,
    ) -> String {
        self.next_continuation_ack = self.next_continuation_ack.saturating_add(1);
        let acknowledgement = format!("{CONTINUATION_ACK_PREFIX}{}>", self.next_continuation_ack);
        self.expected_continuation_ack = Some(acknowledgement.clone());
        format!(
            "{} Begin the next answer-channel response with the exact control marker `{acknowledgement}` on its own line, then emit only the new continuation. Nexa removes the marker before display. The marker is required to confirm an intentional continuation whose text repeats an earlier fragment.",
            cause.controller_prompt(had_visible_content),
        )
    }

    fn commit_continuation_fragment(
        &mut self,
        fragment: &str,
        acknowledged: bool,
    ) -> ContinuationCommit {
        if fragment.is_empty() {
            return ContinuationCommit::Empty;
        }
        if fragment.trim().is_empty() {
            if !self.pending_ambiguous_fragments.is_empty() {
                if !acknowledged {
                    self.pending_ambiguous_fragments.push(fragment.to_string());
                    return ContinuationCommit::AmbiguousReplay;
                }

                let mut visible_delta = String::new();
                for pending in std::mem::take(&mut self.pending_ambiguous_fragments) {
                    self.visible_prefix.push_str(&pending);
                    visible_delta.push_str(&pending);
                }
                self.visible_prefix.push_str(fragment);
                visible_delta.push_str(fragment);
                return if visible_delta.trim().is_empty() {
                    ContinuationCommit::CommittedWhitespace(visible_delta)
                } else {
                    ContinuationCommit::Committed(visible_delta)
                };
            }
            self.visible_prefix.push_str(fragment);
            return ContinuationCommit::CommittedWhitespace(fragment.to_string());
        }

        if acknowledged {
            // An acknowledged fragment is a fresh response to the current
            // continuation cursor. If it confirms a previously ambiguous
            // duplicate, commit that logical fragment once rather than both
            // the proposal and its confirmation.
            let pending_confirms_current = self
                .pending_ambiguous_fragments
                .last()
                .is_some_and(|pending| pending == fragment);
            let pending = std::mem::take(&mut self.pending_ambiguous_fragments);
            let mut visible_delta = String::new();
            for pending_fragment in pending {
                self.visible_prefix.push_str(&pending_fragment);
                visible_delta.push_str(&pending_fragment);
            }
            if !pending_confirms_current {
                self.visible_prefix.push_str(fragment);
                visible_delta.push_str(fragment);
            }
            return if visible_delta.is_empty() {
                ContinuationCommit::Empty
            } else if visible_delta.trim().is_empty() {
                ContinuationCommit::CommittedWhitespace(visible_delta)
            } else {
                ContinuationCommit::Committed(visible_delta)
            };
        }

        let overlap = longest_suffix_prefix_overlap(&self.visible_prefix, fragment);
        let occurs_in_committed_answer =
            !self.visible_prefix.is_empty() && self.visible_prefix.contains(fragment);
        if overlap == fragment.len() || occurs_in_committed_answer {
            // Occurrence is not proof of replay. Hold one logical fragment
            // until either a fresh acknowledgement confirms it or later novel
            // text demonstrates that the provider moved past it.
            self.pending_ambiguous_fragments.push(fragment.to_string());
            return ContinuationCommit::AmbiguousReplay;
        }

        let mut visible_delta = String::new();
        for pending in std::mem::take(&mut self.pending_ambiguous_fragments) {
            self.visible_prefix.push_str(&pending);
            visible_delta.push_str(&pending);
        }
        visible_delta.push_str(&append_with_overlap(&mut self.visible_prefix, fragment));
        if visible_delta.is_empty() {
            ContinuationCommit::Empty
        } else if visible_delta.trim().is_empty() {
            ContinuationCommit::CommittedWhitespace(visible_delta)
        } else {
            ContinuationCommit::Committed(visible_delta)
        }
    }

    #[cfg(test)]
    fn observe(
        &mut self,
        finish_reason: Option<&FinishReason>,
        content: &str,
        has_tool_calls: bool,
    ) -> OutputRecoveryDecision {
        self.observe_with_provider_state(finish_reason, content, has_tool_calls, None)
    }

    pub(super) fn observe_with_provider_state(
        &mut self,
        finish_reason: Option<&FinishReason>,
        content: &str,
        has_tool_calls: bool,
        provider_state_fingerprint: Option<&str>,
    ) -> OutputRecoveryDecision {
        let expected_ack = self.expected_continuation_ack.take();
        let (content, continuation_acknowledged) =
            strip_expected_continuation_ack(content, expected_ack.as_deref());
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
                OutputRecoveryDecision::RejectToolRound {
                    cause: ToolRoundRejectionCause::MalformedToolCall,
                    committed_progress: false,
                }
            } else {
                OutputRecoveryDecision::Reject(OutputRecoveryFailure::MalformedToolCall)
            };
        }

        if matches!(
            finish_reason,
            Some(FinishReason::ProtocolIncomplete | FinishReason::Other)
        ) {
            return if has_tool_calls {
                OutputRecoveryDecision::RejectToolRound {
                    cause: ToolRoundRejectionCause::ProtocolIncomplete,
                    committed_progress: false,
                }
            } else {
                OutputRecoveryDecision::Reject(OutputRecoveryFailure::ProtocolIncomplete)
            };
        }

        let has_visible_content = !content.trim().is_empty();
        if matches!(finish_reason, Some(FinishReason::Length)) {
            self.last_provider_pause_state = None;
            if has_tool_calls {
                // A length-terminated tool envelope is discarded in full. Its
                // adjacent prose is a draft from the same rejected sample and
                // must not leak back into the eventual answer.
                return OutputRecoveryDecision::RejectToolRound {
                    cause: ToolRoundRejectionCause::OutputLimit,
                    committed_progress: false,
                };
            }

            self.active = true;
            self.consecutive_no_progress_resumable_terminals = 0;
            let (visible_delta, semantic_progress) =
                match self.commit_continuation_fragment(content, continuation_acknowledged) {
                    ContinuationCommit::Committed(delta) => (delta, true),
                    ContinuationCommit::CommittedWhitespace(delta) => (delta, false),
                    ContinuationCommit::AmbiguousReplay | ContinuationCommit::Empty => {
                        (String::new(), false)
                    }
                };
            if semantic_progress {
                self.consecutive_no_progress_output_limits = 0;
            } else {
                let stalled_before_visible_separator = !visible_delta.is_empty()
                    && self.consecutive_no_progress_output_limits
                        >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD;
                self.consecutive_no_progress_output_limits =
                    self.consecutive_no_progress_output_limits.saturating_add(1);
                if (visible_delta.is_empty() || stalled_before_visible_separator)
                    && self.consecutive_no_progress_output_limits
                        >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD
                {
                    return OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit);
                }
            }
            return OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta,
            };
        }

        let resumable_terminal = match finish_reason {
            Some(FinishReason::ProviderPause) => Some(OutputRecoveryCause::ProviderPause),
            Some(FinishReason::ContextLimit) => Some(OutputRecoveryCause::ContextLimit),
            _ => None,
        };
        if let Some(cause) = resumable_terminal {
            let provider_state_progress = if cause == OutputRecoveryCause::ProviderPause {
                provider_state_fingerprint.is_some_and(|fingerprint| {
                    let changed = self.last_provider_pause_state.as_deref() != Some(fingerprint);
                    self.last_provider_pause_state = Some(fingerprint.to_string());
                    changed
                })
            } else {
                self.last_provider_pause_state = None;
                false
            };
            // A resumable provider terminal starts a recovery episode even
            // when its concurrent client-tool envelope is rejected. The next
            // sample must still pass through buffered, progress-aware commit.
            self.active = true;
            if has_tool_calls {
                let rejection_cause = match cause {
                    OutputRecoveryCause::ProviderPause => ToolRoundRejectionCause::ProviderPause,
                    OutputRecoveryCause::ContextLimit => ToolRoundRejectionCause::ContextLimit,
                    OutputRecoveryCause::OutputLimit | OutputRecoveryCause::EmptyTerminal => {
                        unreachable!("only resumable provider terminals reach this branch")
                    }
                };
                return OutputRecoveryDecision::RejectToolRound {
                    cause: rejection_cause,
                    committed_progress: provider_state_progress,
                };
            }
            self.consecutive_no_progress_output_limits = 0;
            let (visible_delta, semantic_progress) =
                match self.commit_continuation_fragment(content, continuation_acknowledged) {
                    ContinuationCommit::Committed(delta) => (delta, true),
                    ContinuationCommit::CommittedWhitespace(delta) => (delta, false),
                    ContinuationCommit::AmbiguousReplay | ContinuationCommit::Empty => {
                        (String::new(), false)
                    }
                };
            if semantic_progress || provider_state_progress {
                self.consecutive_no_progress_resumable_terminals = 0;
            } else {
                let stalled_before_visible_separator = !visible_delta.is_empty()
                    && self.consecutive_no_progress_resumable_terminals
                        >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD;
                self.consecutive_no_progress_resumable_terminals = self
                    .consecutive_no_progress_resumable_terminals
                    .saturating_add(1);
                if (visible_delta.is_empty() || stalled_before_visible_separator)
                    && self.consecutive_no_progress_resumable_terminals
                        >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD
                {
                    return OutputRecoveryDecision::Reject(
                        OutputRecoveryFailure::ProtocolIncomplete,
                    );
                }
            }
            return OutputRecoveryDecision::Continue {
                cause,
                visible_delta,
            };
        }

        if has_tool_calls {
            let visible_delta = if self.active {
                match self.commit_continuation_fragment(content, continuation_acknowledged) {
                    ContinuationCommit::Committed(delta)
                    | ContinuationCommit::CommittedWhitespace(delta) => delta,
                    ContinuationCommit::AmbiguousReplay | ContinuationCommit::Empty => {
                        String::new()
                    }
                }
            } else {
                content.to_string()
            };
            self.consecutive_no_progress_output_limits = 0;
            self.consecutive_no_progress_resumable_terminals = 0;
            self.last_provider_pause_state = None;
            return OutputRecoveryDecision::ToolRound { visible_delta };
        }

        if has_visible_content {
            let visible_delta = match self
                .commit_continuation_fragment(content, continuation_acknowledged)
            {
                ContinuationCommit::Committed(delta)
                | ContinuationCommit::CommittedWhitespace(delta) => delta,
                ContinuationCommit::AmbiguousReplay | ContinuationCommit::Empty => {
                    self.active = true;
                    self.consecutive_no_progress_output_limits =
                        self.consecutive_no_progress_output_limits.saturating_add(1);
                    if self.consecutive_no_progress_output_limits
                        >= CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD
                    {
                        return OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit);
                    }
                    return OutputRecoveryDecision::Continue {
                        cause: OutputRecoveryCause::OutputLimit,
                        visible_delta: String::new(),
                    };
                }
            };
            let final_content = std::mem::take(&mut self.visible_prefix);
            self.active = false;
            self.pending_ambiguous_fragments.clear();
            self.expected_continuation_ack = None;
            self.empty_terminal_retried = false;
            self.consecutive_no_progress_output_limits = 0;
            self.consecutive_no_progress_resumable_terminals = 0;
            self.last_provider_pause_state = None;
            return OutputRecoveryDecision::Final {
                content: final_content,
                visible_delta,
            };
        }

        if !self.empty_terminal_retried {
            self.active = true;
            self.empty_terminal_retried = true;
            return OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::EmptyTerminal,
                visible_delta: String::new(),
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
            OutputRecoveryDecision::ToolRound {
                visible_delta: String::new(),
            }
        );
        assert!(recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "done", false),
            OutputRecoveryDecision::Final {
                content: "done".to_string(),
                visible_delta: "done".to_string(),
            }
        );
        assert!(!recovery.reserves_answer_channel());
    }

    #[test]
    fn truncated_tool_round_does_not_reintroduce_its_discarded_visible_draft() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "partial answer; ", true),
            OutputRecoveryDecision::RejectToolRound {
                cause: ToolRoundRejectionCause::OutputLimit,
                committed_progress: false,
            }
        );
        assert!(!recovery.reserves_answer_channel());
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "finished", false),
            OutputRecoveryDecision::Final {
                content: "finished".to_string(),
                visible_delta: "finished".to_string(),
            }
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
            OutputRecoveryDecision::Final {
                content: "one two three four".to_string(),
                visible_delta: "four".to_string(),
            }
        );
    }

    #[test]
    fn repeated_non_empty_output_limit_fragments_stall_without_growing_the_answer() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "same fragment", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta: "same fragment".to_string(),
            }
        );
        for _ in 0..CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD - 1 {
            assert_eq!(
                recovery.observe(Some(&FinishReason::Length), "same fragment", false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    visible_delta: String::new(),
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
    fn repeated_continuation_is_committed_when_later_text_disambiguates_it() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta: "foo".to_string(),
            }
        );
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);

        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta: String::new(),
            }
        );
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);

        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "bar", false),
            OutputRecoveryDecision::Final {
                content: "foofoobar".to_string(),
                visible_delta: "foobar".to_string(),
            }
        );
    }

    #[test]
    fn every_ambiguous_fragment_is_retained_until_later_progress() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo bar", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue {
                visible_delta,
                ..
            } if visible_delta.is_empty()
        ));
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "bar", false),
            OutputRecoveryDecision::Continue {
                visible_delta,
                ..
            } if visible_delta.is_empty()
        ));
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);

        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), " baz", false),
            OutputRecoveryDecision::Final {
                content: "foo barfoobar baz".to_string(),
                visible_delta: "foobar baz".to_string(),
            }
        );
    }

    #[test]
    fn recovery_tool_round_text_is_committed_through_overlap_normalization() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "abc", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        let _ = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);

        assert_eq!(
            recovery.observe(Some(&FinishReason::ToolCalls), "cde", true),
            OutputRecoveryDecision::ToolRound {
                visible_delta: "de".to_string(),
            }
        );
        assert_eq!(recovery.visible_prefix, "abcde");
    }

    #[test]
    fn fresh_continuation_ack_preserves_intentional_exact_repetition() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        let prompt = recovery.controller_prompt(OutputRecoveryCause::OutputLimit, true);
        let acknowledgement = recovery
            .expected_continuation_ack
            .clone()
            .expect("controller prompt must arm an acknowledgement");
        assert!(prompt.contains(&acknowledgement));

        assert_eq!(
            recovery.observe(
                Some(&FinishReason::Length),
                &format!("{acknowledgement}\nfoo"),
                false,
            ),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta: "foo".to_string(),
            }
        );
        assert_eq!(recovery.visible_prefix, "foofoo");
    }

    #[test]
    fn overlapping_utf8_continuations_append_only_novel_suffixes() {
        let mut recovery = OutputRecovery::default();
        for fragment in ["开始你好世界", "世界继续前进"] {
            assert!(matches!(
                recovery.observe(Some(&FinishReason::Length), fragment, false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    ..
                }
            ));
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "继续前进完成", false),
            OutputRecoveryDecision::Final {
                content: "开始你好世界继续前进完成".to_string(),
                visible_delta: "完成".to_string(),
            }
        );
    }

    #[test]
    fn overlap_normalization_preserves_whitespace_only_suffixes() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "foo ", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::OutputLimit,
                visible_delta: " ".to_string(),
            }
        );
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "bar", false),
            OutputRecoveryDecision::Final {
                content: "foo bar".to_string(),
                visible_delta: "bar".to_string(),
            }
        );
    }

    #[test]
    fn whitespace_waits_behind_earlier_ambiguous_fragments() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue {
                visible_delta,
                ..
            } if visible_delta.is_empty()
        ));
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), " ", false),
            OutputRecoveryDecision::Continue {
                visible_delta,
                ..
            } if visible_delta.is_empty()
        ));
        assert_eq!(
            recovery.observe(Some(&FinishReason::Stop), "bar", false),
            OutputRecoveryDecision::Final {
                content: "foofoo bar".to_string(),
                visible_delta: "foo bar".to_string(),
            }
        );
    }

    #[test]
    fn whitespace_fragments_are_visible_but_do_not_hide_a_stalled_recovery() {
        let mut recovery = OutputRecovery::default();
        assert!(matches!(
            recovery.observe(Some(&FinishReason::Length), "foo", false),
            OutputRecoveryDecision::Continue { .. }
        ));
        for _ in 0..CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD {
            assert_eq!(
                recovery.observe(Some(&FinishReason::Length), "\n", false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::OutputLimit,
                    visible_delta: "\n".to_string(),
                }
            );
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::Length), "\n", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::OutputLimit)
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
                OutputRecoveryDecision::ToolRound {
                    visible_delta: String::new(),
                }
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
        assert_eq!(
            context.observe(Some(&FinishReason::ContextLimit), "partial", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ContextLimit,
                visible_delta: "partial".to_string(),
            }
        );

        let mut paused_draft = OutputRecovery::default();
        assert_eq!(
            paused_draft.observe_with_provider_state(
                Some(&FinishReason::ProviderPause),
                "draft text",
                true,
                Some("committed-provider-state"),
            ),
            OutputRecoveryDecision::RejectToolRound {
                cause: ToolRoundRejectionCause::ProviderPause,
                committed_progress: true,
            }
        );
        assert!(paused_draft.reserves_answer_channel());
    }

    #[test]
    fn repeated_resumable_text_stalls_without_duplicate_growth() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe(Some(&FinishReason::ContextLimit), "abc", false),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ContextLimit,
                visible_delta: "abc".to_string(),
            }
        );
        for _ in 0..CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD - 1 {
            assert_eq!(
                recovery.observe(Some(&FinishReason::ContextLimit), "abc", false),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::ContextLimit,
                    visible_delta: String::new(),
                }
            );
        }
        assert_eq!(
            recovery.observe(Some(&FinishReason::ContextLimit), "abc", false),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::ProtocolIncomplete)
        );
        assert_eq!(recovery.visible_prefix, "abc");
    }

    #[test]
    fn provider_pause_native_state_counts_as_progress_without_repeating_text() {
        let mut recovery = OutputRecovery::default();
        assert_eq!(
            recovery.observe_with_provider_state(
                Some(&FinishReason::ProviderPause),
                "Searching",
                false,
                Some("state-1"),
            ),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ProviderPause,
                visible_delta: "Searching".to_string(),
            }
        );
        assert_eq!(
            recovery.observe_with_provider_state(
                Some(&FinishReason::ProviderPause),
                "Searching",
                false,
                Some("state-2"),
            ),
            OutputRecoveryDecision::Continue {
                cause: OutputRecoveryCause::ProviderPause,
                visible_delta: String::new(),
            }
        );
        for _ in 0..CONSECUTIVE_NO_PROGRESS_STALL_THRESHOLD - 1 {
            assert!(matches!(
                recovery.observe_with_provider_state(
                    Some(&FinishReason::ProviderPause),
                    "Searching",
                    false,
                    Some("state-2"),
                ),
                OutputRecoveryDecision::Continue {
                    cause: OutputRecoveryCause::ProviderPause,
                    ..
                }
            ));
        }
        assert_eq!(
            recovery.observe_with_provider_state(
                Some(&FinishReason::ProviderPause),
                "Searching",
                false,
                Some("state-2"),
            ),
            OutputRecoveryDecision::Reject(OutputRecoveryFailure::ProtocolIncomplete)
        );
        assert_eq!(recovery.visible_prefix, "Searching");
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
