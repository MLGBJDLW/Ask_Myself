//! Logical tool-round budgeting for one user turn.
//!
//! Provider samples are transport/runtime implementation details. They must
//! not consume the user-facing tool-round budget when Nexa is continuing an
//! output-limited answer, repairing an incomplete provider envelope, or
//! restarting a discarded sample. A finite tool budget also owns one
//! answer-only synthesis step after the last verified tool round.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// One shared ceiling across model restarts and all recovery controllers.
/// It is consumed only before starting a provider invocation, never while
/// receiving an active stream. Tool-round accounting remains independent.
#[derive(Debug, Clone)]
pub(super) struct ModelRequestBudget {
    used: Arc<AtomicU32>,
    limit: u32,
}

impl ModelRequestBudget {
    pub(super) fn new(limit: u32) -> Self {
        Self {
            used: Arc::new(AtomicU32::new(0)),
            limit,
        }
    }

    pub(super) fn acquire(&self) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.limit).then(|| used + 1)
            })
            .is_ok()
    }

    pub(super) fn limit(&self) -> u32 {
        self.limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnStepPurpose {
    Normal,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnStepMode {
    ToolsAllowed,
    FinalAnswerOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TurnStepPermit {
    pub(super) sample_index: u32,
    pub(super) mode: TurnStepMode,
    pub(super) tool_rounds_used: u32,
    pub(super) remaining_tool_rounds: Option<u32>,
}

impl TurnStepPermit {
    pub(super) fn allows_tools(self) -> bool {
        self.mode == TurnStepMode::ToolsAllowed
    }
}

/// Owns the distinction between logical tool rounds and physical model
/// samples. `u32::MAX` remains the legacy boundary encoding for no configured
/// tool-round cap; it is normalized to `None` at this seam.
#[derive(Debug)]
pub(super) struct TurnBudget {
    requests: ModelRequestBudget,
    tool_round_limit: Option<u32>,
    tool_rounds_used: u32,
    next_sample_index: u32,
    final_answer_sample_started: bool,
}

impl TurnBudget {
    pub(super) fn new(legacy_max_iterations: u32) -> Self {
        Self {
            requests: ModelRequestBudget::new(if legacy_max_iterations == u32::MAX {
                256
            } else {
                legacy_max_iterations.saturating_add(16)
            }),
            tool_round_limit: (legacy_max_iterations != u32::MAX).then_some(legacy_max_iterations),
            tool_rounds_used: 0,
            next_sample_index: 0,
            final_answer_sample_started: false,
        }
    }

    pub(super) fn permit(&mut self, purpose: TurnStepPurpose) -> Option<TurnStepPermit> {
        let final_answer_only = self
            .tool_round_limit
            .is_some_and(|limit| self.tool_rounds_used >= limit);

        if final_answer_only
            && purpose == TurnStepPurpose::Normal
            && self.final_answer_sample_started
        {
            return None;
        }
        if final_answer_only {
            self.final_answer_sample_started = true;
        }

        let permit = TurnStepPermit {
            sample_index: self.next_sample_index,
            mode: if final_answer_only {
                TurnStepMode::FinalAnswerOnly
            } else {
                TurnStepMode::ToolsAllowed
            },
            tool_rounds_used: self.tool_rounds_used,
            remaining_tool_rounds: self.remaining_tool_rounds(),
        };
        self.next_sample_index = self.next_sample_index.saturating_add(1);
        Some(permit)
    }

    pub(super) fn request_budget(&self) -> ModelRequestBudget {
        self.requests.clone()
    }

    pub(super) fn record_verified_tool_round(&mut self) {
        debug_assert!(
            self.can_dispatch_tool_round(),
            "a verified tool round must acquire budget authority before dispatch"
        );
        self.tool_rounds_used = self.tool_rounds_used.saturating_add(1);
    }

    /// Whether a controller- or model-owned tool batch may enter dispatch.
    /// Unlike [`Self::can_start_normal_step`], this never treats the reserved
    /// answer-only sample as tool authority.
    pub(super) fn can_dispatch_tool_round(&self) -> bool {
        self.tool_round_limit
            .is_none_or(|limit| self.tool_rounds_used < limit)
    }

    pub(super) fn can_start_normal_step(&self) -> bool {
        match self.tool_round_limit {
            None => true,
            Some(limit) if self.tool_rounds_used < limit => true,
            Some(_) => !self.final_answer_sample_started,
        }
    }

    pub(super) fn tool_rounds_used(&self) -> u32 {
        self.tool_rounds_used
    }

    pub(super) fn remaining_tool_rounds(&self) -> Option<u32> {
        self.tool_round_limit
            .map(|limit| limit.saturating_sub(self.tool_rounds_used))
    }

    pub(super) fn configured_tool_round_limit(&self) -> Option<u32> {
        self.tool_round_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_recovery_controllers_share_a_non_resetting_request_ceiling() {
        let turn = TurnBudget::new(0);
        for _ in 0..16 {
            assert!(turn.request_budget().acquire());
        }
        assert!(!turn.request_budget().acquire());
        assert!(!turn.request_budget().acquire());
        assert_eq!(turn.tool_rounds_used(), 0);
    }

    #[test]
    fn unconfigured_tool_rounds_still_have_a_finite_model_request_ceiling() {
        let turn = TurnBudget::new(u32::MAX);
        for _ in 0..256 {
            assert!(turn.request_budget().acquire());
        }
        assert!(!turn.request_budget().acquire());
    }

    #[test]
    fn recovery_samples_do_not_consume_a_tool_round() {
        let mut budget = TurnBudget::new(1);

        let first = budget.permit(TurnStepPurpose::Normal).unwrap();
        let recovery = budget.permit(TurnStepPurpose::Recovery).unwrap();

        assert!(first.allows_tools());
        assert!(recovery.allows_tools());
        assert_eq!(first.tool_rounds_used, 0);
        assert_eq!(recovery.tool_rounds_used, 0);
        assert_eq!(recovery.sample_index, 1);
    }

    #[test]
    fn finite_tool_budget_reserves_one_answer_only_sample() {
        let mut budget = TurnBudget::new(1);
        assert!(budget
            .permit(TurnStepPurpose::Normal)
            .unwrap()
            .allows_tools());
        budget.record_verified_tool_round();

        let final_answer = budget.permit(TurnStepPurpose::Normal).unwrap();
        assert_eq!(final_answer.mode, TurnStepMode::FinalAnswerOnly);
        assert_eq!(final_answer.remaining_tool_rounds, Some(0));
        assert!(budget.permit(TurnStepPurpose::Normal).is_none());
    }

    #[test]
    fn final_answer_output_recovery_remains_permitted_without_tools() {
        let mut budget = TurnBudget::new(0);
        assert!(!budget.can_dispatch_tool_round());
        let final_answer = budget.permit(TurnStepPurpose::Normal).unwrap();
        let continuation = budget.permit(TurnStepPurpose::Recovery).unwrap();

        assert_eq!(final_answer.mode, TurnStepMode::FinalAnswerOnly);
        assert_eq!(continuation.mode, TurnStepMode::FinalAnswerOnly);
        assert_eq!(continuation.sample_index, 1);
    }

    #[test]
    fn legacy_unlimited_budget_never_enters_answer_only_mode() {
        let mut budget = TurnBudget::new(u32::MAX);
        for _ in 0..4 {
            assert_eq!(
                budget.permit(TurnStepPurpose::Normal).unwrap().mode,
                TurnStepMode::ToolsAllowed
            );
            budget.record_verified_tool_round();
        }
        assert_eq!(budget.configured_tool_round_limit(), None);
    }

    #[test]
    fn controller_tool_batches_share_the_verified_round_budget() {
        let mut budget = TurnBudget::new(1);
        assert!(budget.can_dispatch_tool_round());
        budget.record_verified_tool_round();
        assert!(!budget.can_dispatch_tool_round());
        assert_eq!(
            budget.permit(TurnStepPurpose::Normal).unwrap().mode,
            TurnStepMode::FinalAnswerOnly
        );
    }
}
