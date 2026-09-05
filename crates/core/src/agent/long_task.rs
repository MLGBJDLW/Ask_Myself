//! Long-running task resilience helpers.

use super::context;
use super::*;
use crate::workflow_ir::WorkflowIr;

const CHECKPOINT_EVERY_TOOL_ROUNDS: u32 = 3;

#[derive(Debug, Default)]
pub(super) struct LongTaskState {
    last_checkpoint_iteration: Option<u32>,
}

pub(super) struct LongTaskCompactionContext<'a> {
    pub(super) db: &'a Database,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) model: &'a str,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) context_pipeline: ContextPipeline,
    pub(super) tool_defs: &'a [ToolDefinition],
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) total_usage: &'a mut Usage,
}

impl LongTaskState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn should_checkpoint_after_tool_round(&self, iteration: u32) -> bool {
        let completed_rounds = iteration.saturating_add(1);
        completed_rounds.is_multiple_of(CHECKPOINT_EVERY_TOOL_ROUNDS)
            && self.last_checkpoint_iteration != Some(iteration)
    }

    pub(super) fn record_checkpoint(&mut self, iteration: u32) {
        self.last_checkpoint_iteration = Some(iteration);
    }

    pub(super) fn checkpoint_live_state(
        &self,
        plan: &AgentTaskPlan,
        workflow_ir: Option<&WorkflowIr>,
        completed_tool_rounds: u32,
        max_iterations: u32,
        loop_recorder: &TurnLoopRecorder,
    ) -> serde_json::Value {
        let mut loop_events_tail = loop_recorder
            .events()
            .iter()
            .rev()
            .take(12)
            .cloned()
            .collect::<Vec<_>>();
        loop_events_tail.reverse();
        let current_step = plan
            .steps
            .iter()
            .find(|step| step.status != crate::intelligence::PlanStepStatus::Completed);

        serde_json::json!({
            "kind": "longTaskLiveState",
            // Retain the legacy zero-based projection while making the
            // authoritative completed-round count explicit. Callers operate
            // on counts, so a pause before the first tool round cannot
            // manufacture a completed iteration.
            "iteration": completed_tool_rounds.saturating_sub(1),
            "completedToolRounds": completed_tool_rounds,
            "maxIterations": max_iterations,
            "remainingIterations": max_iterations.saturating_sub(completed_tool_rounds),
            "lastCheckpointIteration": self.last_checkpoint_iteration,
            "taskPlan": plan,
            "workflowIr": workflow_ir,
            "currentStep": current_step,
            "evidenceSufficiency": &plan.ledger.sufficiency,
            "openQuestions": &plan.ledger.open_questions,
            "loopEventsTail": loop_events_tail,
        })
    }
}

impl AgentExecutor {
    pub(super) async fn compact_before_model_step_if_needed(
        &self,
        ctx: LongTaskCompactionContext<'_>,
    ) -> bool {
        let LongTaskCompactionContext {
            db,
            conversation_id,
            turn_id,
            tx,
            model,
            messages,
            context_pipeline,
            tool_defs,
            loop_recorder,
            persisted_trace_items,
            total_usage,
        } = ctx;

        let estimated =
            context::estimate_context_usage_breakdown_for_model(model, messages, tool_defs, None);
        let budget_decision = context_pipeline.budget_decision(estimated.total_tokens);
        if !budget_decision.should_compact {
            return false;
        }

        let before_message_count = messages.len();
        let before_messages = prompt_cache::message_sequence_fingerprint(messages);
        let started = TurnLoopEvent::CompactionStarted {
            reason: "pre_model_estimate".to_string(),
            message_count: before_message_count,
        };
        loop_recorder.record(started.clone());
        append_persisted_trace_loop_event(persisted_trace_items, started);
        append_persisted_trace_status(
            persisted_trace_items,
            "Proactively compacting context before the next model step.",
            "info",
        );

        let actual_tokens_remaining = self
            .config
            .max_actual_tokens_per_run
            .map(|limit| limit.saturating_sub(total_usage.total_tokens));
        let compacted = match self
            .aggressive_compact(
                messages,
                model,
                tx,
                context_compaction::CompactionRunContext {
                    db,
                    conversation_id,
                    turn_id,
                },
                actual_tokens_remaining,
            )
            .await
        {
            Ok(compaction_usage) => {
                super::usage_accounting::accumulate_usage(total_usage, &compaction_usage);
                let after_messages = prompt_cache::message_sequence_fingerprint(messages);
                let compacted = before_messages != after_messages;
                let evicted_count = before_message_count.saturating_sub(messages.len());
                let ended = TurnLoopEvent::CompactionEnded {
                    reason: "pre_model_estimate".to_string(),
                    evicted_count,
                    message_count: messages.len(),
                };
                loop_recorder.record(ended.clone());
                append_persisted_trace_loop_event(persisted_trace_items, ended);
                compacted
            }
            Err(err) => {
                warn!("Pre-model context compaction failed: {err}");
                append_persisted_trace_status(
                    persisted_trace_items,
                    &format!("Pre-model context compaction failed: {err}"),
                    "warning",
                );
                false
            }
        };
        compacted
    }
}

pub(super) fn create_task_checkpoint_for_turn(
    db: &Database,
    turn_id: Option<&str>,
    reason: &str,
) -> Result<Option<String>, CoreError> {
    create_task_checkpoint_for_turn_with_state(db, turn_id, reason, None)
}

pub(super) fn create_task_checkpoint_for_turn_with_state(
    db: &Database,
    turn_id: Option<&str>,
    reason: &str,
    live_state: Option<&serde_json::Value>,
) -> Result<Option<String>, CoreError> {
    let Some(tid) = turn_id else {
        return Ok(None);
    };
    let Some(task_run) = db.get_agent_task_run_by_turn(tid)? else {
        return Ok(None);
    };
    let checkpoint =
        db.create_task_resume_checkpoint_with_state(&task_run.id, reason, live_state)?;
    Ok(Some(checkpoint.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::{build_task_plan, TaskPlanningInput};

    #[test]
    fn checkpoint_cadence_tracks_completed_tool_rounds() {
        let mut state = LongTaskState::new();

        assert!(!state.should_checkpoint_after_tool_round(0));
        assert!(!state.should_checkpoint_after_tool_round(1));
        assert!(state.should_checkpoint_after_tool_round(2));

        state.record_checkpoint(2);
        assert!(!state.should_checkpoint_after_tool_round(2));
        assert!(!state.should_checkpoint_after_tool_round(3));
        assert!(!state.should_checkpoint_after_tool_round(4));
        assert!(state.should_checkpoint_after_tool_round(5));
    }

    #[test]
    fn live_state_carries_plan_and_recent_loop_events() {
        let state = LongTaskState::new();
        let plan = build_task_plan(TaskPlanningInput::for_route(
            "audit a codebase",
            "CodebaseOperation",
            false,
            0,
        ));
        let mut recorder = TurnLoopRecorder::new(AgentRouteKind::CodebaseOperation, 8);
        recorder.record(TurnLoopEvent::StepStarted {
            iteration: 1,
            remaining_iterations: 7,
        });

        let live_state = state.checkpoint_live_state(&plan, None, 1, 8, &recorder);

        assert_eq!(live_state["kind"].as_str(), Some("longTaskLiveState"));
        assert_eq!(live_state["completedToolRounds"].as_u64(), Some(1));
        assert_eq!(live_state["remainingIterations"].as_u64(), Some(7));
        assert_eq!(
            live_state["taskPlan"]["objective"].as_str(),
            Some(plan.objective.as_str())
        );
        assert!(live_state["loopEventsTail"]
            .as_array()
            .is_some_and(|items| items.len() == 2));

        let before_first_round = state.checkpoint_live_state(&plan, None, 0, 8, &recorder);
        assert_eq!(before_first_round["completedToolRounds"].as_u64(), Some(0));
        assert_eq!(before_first_round["remainingIterations"].as_u64(), Some(8));
    }
}
