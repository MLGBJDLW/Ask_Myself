//! First-class control-plane events for a single agent turn.

use serde::{Deserialize, Serialize};

use super::route::AgentRouteKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum TurnLoopEvent {
    TurnStarted {
        route_kind: String,
        max_iterations: u32,
    },
    StepStarted {
        iteration: u32,
        remaining_iterations: u32,
    },
    ModelStepCompleted {
        iteration: u32,
        tool_call_count: usize,
        finish_reason: Option<String>,
        prompt_tokens: u32,
        completion_tokens: u32,
        context_usage_pct: f32,
    },
    ToolScheduled {
        iteration: u32,
        call_id: String,
        tool_name: String,
        timeout_secs: Option<u64>,
        policy: String,
    },
    ToolFinished {
        iteration: u32,
        call_id: String,
        tool_name: String,
        duration_ms: u64,
        is_error: bool,
    },
    CompactionStarted {
        reason: String,
        message_count: usize,
    },
    CompactionEnded {
        reason: String,
        evicted_count: usize,
        message_count: usize,
    },
    LoopGuardIntervention {
        reason: String,
        action: String,
    },
    TurnFinished {
        outcome: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct TurnLoopRecorder {
    events: Vec<TurnLoopEvent>,
}

impl TurnLoopRecorder {
    pub(crate) fn new(route_kind: AgentRouteKind, max_iterations: u32) -> Self {
        let mut recorder = Self { events: Vec::new() };
        recorder.record(TurnLoopEvent::TurnStarted {
            route_kind: route_kind.as_str().to_string(),
            max_iterations,
        });
        recorder
    }

    pub(crate) fn record(&mut self, event: TurnLoopEvent) {
        self.events.push(event);
    }

    pub(crate) fn tool_scheduled(
        &mut self,
        iteration: u32,
        call_id: &str,
        tool_name: &str,
        timeout_secs: Option<u64>,
        policy: impl Into<String>,
    ) {
        self.record(TurnLoopEvent::ToolScheduled {
            iteration,
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            timeout_secs,
            policy: policy.into(),
        });
    }

    pub(crate) fn events(&self) -> &[TurnLoopEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_keeps_route_and_events() {
        let mut recorder = TurnLoopRecorder::new(AgentRouteKind::FileOperation, 5);
        recorder.record(TurnLoopEvent::StepStarted {
            iteration: 0,
            remaining_iterations: 5,
        });
        recorder.tool_scheduled(0, "call-1", "read_file", Some(30), "execute");
        recorder.record(TurnLoopEvent::TurnFinished {
            outcome: "success".to_string(),
        });

        assert_eq!(recorder.events().len(), 4);
        assert!(matches!(
            recorder.events().first(),
            Some(TurnLoopEvent::TurnStarted { route_kind, .. }) if route_kind == "FileOperation"
        ));
    }
}
