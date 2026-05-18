//! Token usage, model-step trace, and automatic compaction accounting.

use super::turn_state::{TurnPhase, TurnStateMachine};
use super::*;

pub(super) struct UsageAccountingContext<'a> {
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) model: &'a str,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) context_pipeline: ContextPipeline,
    pub(super) turn_state: &'a mut TurnStateMachine,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) total_usage: &'a mut Usage,
    pub(super) last_prompt_tokens: &'a mut u32,
}

impl AgentExecutor {
    pub(super) async fn record_model_step_usage(
        &self,
        ctx: UsageAccountingContext<'_>,
        iteration: u32,
        tool_call_count: usize,
        finish_reason: Option<String>,
        chunk_usage: Option<Usage>,
    ) {
        let Some(u) = chunk_usage else {
            return;
        };
        let UsageAccountingContext {
            tx,
            model,
            messages,
            context_pipeline,
            turn_state,
            loop_recorder,
            persisted_trace_items,
            trace,
            total_usage,
            last_prompt_tokens,
        } = ctx;

        *last_prompt_tokens = u.prompt_tokens;
        total_usage.prompt_tokens += u.prompt_tokens;
        total_usage.completion_tokens += u.completion_tokens;
        total_usage.total_tokens += u.total_tokens;
        if let Some(t) = u.thinking_tokens {
            *total_usage.thinking_tokens.get_or_insert(0) += t;
        }

        let _ = tx
            .send(AgentEvent::UsageUpdate {
                usage_total: total_usage.clone(),
                last_prompt_tokens: *last_prompt_tokens,
            })
            .await;

        let mut iteration_compacted = false;
        let budget_decision = context_pipeline.budget_decision(u.prompt_tokens);
        let _budget_tokens = budget_decision.budget_tokens;
        let iteration_context_pct = budget_decision.usage_pct;
        if budget_decision.should_compact {
            let before_message_count = messages.len();
            let started = TurnLoopEvent::CompactionStarted {
                reason: "auto".to_string(),
                message_count: before_message_count,
            };
            loop_recorder.record(started.clone());
            append_persisted_trace_loop_event(persisted_trace_items, started);
            turn_state.transition_to(TurnPhase::Compacting);
            if let Err(e) = self.aggressive_compact(messages, model, tx).await {
                warn!("Auto-compact failed: {e}");
            } else {
                iteration_compacted = true;
                let evicted_count = before_message_count.saturating_sub(messages.len());
                let ended = TurnLoopEvent::CompactionEnded {
                    reason: "auto".to_string(),
                    evicted_count,
                    message_count: messages.len(),
                };
                loop_recorder.record(ended.clone());
                append_persisted_trace_loop_event(persisted_trace_items, ended);
            }
            turn_state.transition_to(TurnPhase::ModelStep);
        }

        let completed = TurnLoopEvent::ModelStepCompleted {
            iteration,
            tool_call_count,
            finish_reason,
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            context_usage_pct: iteration_context_pct,
        };
        loop_recorder.record(completed.clone());
        append_persisted_trace_loop_event(persisted_trace_items, completed);

        if let Some(ref mut t) = trace {
            t.add_step(TraceStep {
                iteration,
                tool_name: None,
                tool_duration_ms: None,
                input_tokens: u.prompt_tokens as u64,
                output_tokens: u.completion_tokens as u64,
                context_usage_pct: iteration_context_pct,
                was_compacted: iteration_compacted,
            });
        }
    }
}
