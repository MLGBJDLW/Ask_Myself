//! Explicit state machine for one agent turn.

use tracing::debug;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum TurnPhase {
    Created,
    PreparingContext,
    Planning,
    DirectDispatch,
    CacheLookup,
    PreSearch,
    ModelStep,
    ToolDispatch,
    Compacting,
    Finalizing,
    Finished,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum TurnOutcome {
    Success,
    Cached,
    DirectDispatch,
    Cancelled,
    MaxIterations,
}

#[derive(Debug)]
pub(crate) struct TurnStateMachine {
    phase: TurnPhase,
    iteration: Option<u32>,
    outcome: Option<TurnOutcome>,
}

impl TurnStateMachine {
    pub(crate) fn new() -> Self {
        Self {
            phase: TurnPhase::Created,
            iteration: None,
            outcome: None,
        }
    }

    pub(crate) fn transition_to(&mut self, next: TurnPhase) {
        if self.phase == next {
            return;
        }
        debug!(
            "Agent turn state transition: {:?} -> {:?} (iteration={:?}, outcome={:?})",
            self.phase, next, self.iteration, self.outcome
        );
        self.phase = next;
    }

    pub(crate) fn start_iteration(&mut self, iteration: u32) {
        self.iteration = Some(iteration);
        self.transition_to(TurnPhase::ModelStep);
    }

    pub(crate) fn finish(&mut self, outcome: TurnOutcome) {
        self.outcome = Some(outcome);
        self.transition_to(TurnPhase::Finished);
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> TurnPhase {
        self.phase
    }

    #[cfg(test)]
    pub(crate) fn outcome(&self) -> Option<TurnOutcome> {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_phase_and_outcome() {
        let mut state = TurnStateMachine::new();
        assert_eq!(state.phase(), TurnPhase::Created);

        state.transition_to(TurnPhase::Planning);
        assert_eq!(state.phase(), TurnPhase::Planning);

        state.start_iteration(0);
        assert_eq!(state.phase(), TurnPhase::ModelStep);

        state.finish(TurnOutcome::Success);
        assert_eq!(state.phase(), TurnPhase::Finished);
        assert_eq!(state.outcome(), Some(TurnOutcome::Success));
    }
}
