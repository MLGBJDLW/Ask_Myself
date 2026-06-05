//! Long-running task resilience helpers.

use super::context;
use super::turn_state::{TurnPhase, TurnStateMachine};
use super::*;

const CHECKPOINT_EVERY_TOOL_ROUNDS: u32 = 3;
const LONG_TASK_RECITATION_PREFIX: &str = "## Long Task Control State";

#[derive(Debug, Default)]
pub(super) struct LongTaskState {
    last_checkpoint_iteration: Option<u32>,
}

pub(super) struct LongTaskCompactionContext<'a> {
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) model: &'a str,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) context_pipeline: ContextPipeline,
    pub(super) tool_defs: &'a [ToolDefinition],
    pub(super) turn_state: &'a mut TurnStateMachine,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
}

impl LongTaskState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn refresh_plan_recitation(
        &self,
        messages: &mut Vec<Message>,
        plan: &AgentTaskPlan,
        iteration: u32,
        max_iterations: u32,
        preserve_prefix: bool,
    ) {
        if iteration == 0 {
            return;
        }
        if !preserve_prefix {
            messages.retain(|message| {
                message.role != Role::System
                    || !message
                        .text_content()
                        .starts_with(LONG_TASK_RECITATION_PREFIX)
            });
        }
        if let Some(message) = prompt_ir::controller_state_message(self.plan_recitation(
            plan,
            iteration,
            max_iterations,
        )) {
            messages.push(message);
        }
    }

    pub(super) fn should_checkpoint_after_tool_round(&self, iteration: u32) -> bool {
        let completed_rounds = iteration.saturating_add(1);
        completed_rounds % CHECKPOINT_EVERY_TOOL_ROUNDS == 0
            && self.last_checkpoint_iteration != Some(iteration)
    }

    pub(super) fn record_checkpoint(&mut self, iteration: u32) {
        self.last_checkpoint_iteration = Some(iteration);
    }

    pub(super) fn checkpoint_live_state(
        &self,
        plan: &AgentTaskPlan,
        iteration: u32,
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
            "iteration": iteration,
            "completedToolRounds": iteration.saturating_add(1),
            "maxIterations": max_iterations,
            "remainingIterations": max_iterations.saturating_sub(iteration.saturating_add(1)),
            "lastCheckpointIteration": self.last_checkpoint_iteration,
            "taskPlan": plan,
            "currentStep": current_step,
            "evidenceSufficiency": &plan.ledger.sufficiency,
            "openQuestions": &plan.ledger.open_questions,
            "loopEventsTail": loop_events_tail,
        })
    }

    fn plan_recitation(&self, plan: &AgentTaskPlan, iteration: u32, max_iterations: u32) -> String {
        let completed = plan
            .steps
            .iter()
            .filter(|step| step.status == crate::intelligence::PlanStepStatus::Completed)
            .count();
        let current_step = plan
            .steps
            .iter()
            .find(|step| step.status != crate::intelligence::PlanStepStatus::Completed)
            .map(|step| format!("{} ({:?})", step.title, step.status))
            .unwrap_or_else(|| "all planned steps completed".to_string());
        let open_questions = compact_items(&plan.ledger.open_questions, "none");
        let safeguards = compact_items(&plan.safeguards, "none");

        format!(
            "{LONG_TASK_RECITATION_PREFIX}\n\
             This rolling control note keeps the long task anchored. Treat the original user request and active plan as still in force.\n\n\
             Objective: {}\n\
             Iteration: {}/{}\n\
             Plan progress: {}/{} steps completed; current: {}\n\
             Evidence sufficiency: {}; open questions: {}\n\
             Safeguards: {}\n\n\
             Rules:\n\
             - Continue from completed work; do not repeat successful tool calls unless evidence is stale, missing, or contradicted.\n\
             - Spend the next tool round on the smallest unfinished step that can change the answer.\n\
             - Before the final answer, verify required evidence and name any remaining gaps.",
            plan.objective,
            iteration.saturating_add(1),
            max_iterations,
            completed,
            plan.steps.len(),
            current_step,
            plan.ledger.sufficiency,
            open_questions,
            safeguards
        )
    }
}

impl AgentExecutor {
    pub(super) async fn compact_before_model_step_if_needed(
        &self,
        ctx: LongTaskCompactionContext<'_>,
    ) {
        let LongTaskCompactionContext {
            tx,
            model,
            messages,
            context_pipeline,
            tool_defs,
            turn_state,
            loop_recorder,
            persisted_trace_items,
        } = ctx;

        let estimated =
            context::estimate_context_usage_breakdown_for_model(model, messages, tool_defs, None);
        let budget_decision = context_pipeline.budget_decision(estimated.total_tokens);
        if !budget_decision.should_compact {
            return;
        }

        let before_message_count = messages.len();
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
        turn_state.transition_to(TurnPhase::Compacting);

        match self.aggressive_compact(messages, model, tx).await {
            Ok(()) => {
                let evicted_count = before_message_count.saturating_sub(messages.len());
                let ended = TurnLoopEvent::CompactionEnded {
                    reason: "pre_model_estimate".to_string(),
                    evicted_count,
                    message_count: messages.len(),
                };
                loop_recorder.record(ended.clone());
                append_persisted_trace_loop_event(persisted_trace_items, ended);
            }
            Err(err) => {
                warn!("Pre-model context compaction failed: {err}");
                append_persisted_trace_status(
                    persisted_trace_items,
                    &format!("Pre-model context compaction failed: {err}"),
                    "warning",
                );
            }
        }
        turn_state.transition_to(TurnPhase::ModelStep);
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

fn compact_items(items: &[String], fallback: &str) -> String {
    let selected = items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .take(3)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        fallback.to_string()
    } else {
        selected.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::{build_task_plan, TaskPlanningInput};

    #[test]
    fn recitation_is_replaced_not_accumulated() {
        let state = LongTaskState::new();
        let plan = build_task_plan(TaskPlanningInput {
            user_query: "compare local notes",
            route_kind: "FileOperation",
            has_sources: true,
            source_scope_count: 2,
            collection_context: false,
        });
        let mut messages = vec![Message::text(Role::System, "root system")];

        state.refresh_plan_recitation(&mut messages, &plan, 1, 10, false);
        state.refresh_plan_recitation(&mut messages, &plan, 2, 10, false);

        let recitations = messages
            .iter()
            .filter(|message| {
                message
                    .text_content()
                    .starts_with(LONG_TASK_RECITATION_PREFIX)
            })
            .collect::<Vec<_>>();
        assert_eq!(recitations.len(), 1);
        let recitation = recitations[0].text_content();
        assert!(recitation.contains("Iteration: 3/10"));
        assert!(recitation.contains(&plan.objective));
    }

    #[test]
    fn recitation_can_preserve_prefix_by_appending() {
        let state = LongTaskState::new();
        let plan = build_task_plan(TaskPlanningInput {
            user_query: "compare local notes",
            route_kind: "FileOperation",
            has_sources: true,
            source_scope_count: 2,
            collection_context: false,
        });
        let mut messages = vec![Message::text(Role::System, "root system")];

        state.refresh_plan_recitation(&mut messages, &plan, 1, 10, true);
        let first = messages
            .iter()
            .map(|message| (message.role.clone(), message.text_content()))
            .collect::<Vec<_>>();
        state.refresh_plan_recitation(&mut messages, &plan, 2, 10, true);
        let prefix = messages
            .iter()
            .take(first.len())
            .map(|message| (message.role.clone(), message.text_content()))
            .collect::<Vec<_>>();

        assert_eq!(messages.len(), first.len() + 1);
        assert_eq!(prefix, first);
    }

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
        let plan = build_task_plan(TaskPlanningInput {
            user_query: "audit a codebase",
            route_kind: "CodebaseOperation",
            has_sources: false,
            source_scope_count: 0,
            collection_context: false,
        });
        let mut recorder = TurnLoopRecorder::new(AgentRouteKind::CodebaseOperation, 8);
        recorder.record(TurnLoopEvent::StepStarted {
            iteration: 1,
            remaining_iterations: 7,
        });

        let live_state = state.checkpoint_live_state(&plan, 1, 8, &recorder);

        assert_eq!(live_state["kind"].as_str(), Some("longTaskLiveState"));
        assert_eq!(live_state["remainingIterations"].as_u64(), Some(6));
        assert_eq!(
            live_state["taskPlan"]["objective"].as_str(),
            Some(plan.objective.as_str())
        );
        assert!(live_state["loopEventsTail"]
            .as_array()
            .is_some_and(|items| items.len() == 2));
    }
}
