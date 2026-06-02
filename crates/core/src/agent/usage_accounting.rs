//! Token usage, model-step trace, and automatic compaction accounting.

use super::context;
use super::turn_state::{TurnPhase, TurnStateMachine};
use super::*;

pub(super) struct UsageAccountingContext<'a> {
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) model: &'a str,
    pub(super) messages: &'a mut Vec<Message>,
    pub(super) context_pipeline: ContextPipeline,
    pub(super) tool_defs: &'a [ToolDefinition],
    pub(super) turn_state: &'a mut TurnStateMachine,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) total_usage: &'a mut Usage,
    pub(super) last_prompt_tokens: &'a mut u32,
    pub(super) last_context_breakdown: &'a mut Option<context::ContextUsageBreakdown>,
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
        let UsageAccountingContext {
            tx,
            model,
            messages,
            context_pipeline,
            tool_defs,
            turn_state,
            loop_recorder,
            persisted_trace_items,
            trace,
            total_usage,
            last_prompt_tokens,
            last_context_breakdown,
        } = ctx;

        let actual_prompt_tokens = chunk_usage.as_ref().map(|usage| usage.prompt_tokens);
        let context_breakdown = context::estimate_context_usage_breakdown_for_model(
            model,
            messages,
            tool_defs,
            actual_prompt_tokens,
        );
        let (prompt_tokens, completion_tokens, _has_actual_usage) =
            model_step_accounting_tokens(chunk_usage.as_ref(), context_breakdown.total_tokens);
        *last_prompt_tokens = prompt_tokens;
        *last_context_breakdown = Some(context_breakdown.clone());
        if let Some(u) = chunk_usage.as_ref() {
            total_usage.prompt_tokens += u.prompt_tokens;
            total_usage.completion_tokens += u.completion_tokens;
            total_usage.total_tokens += u.total_tokens;
            if let Some(t) = u.thinking_tokens {
                *total_usage.thinking_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = u.cache_read_tokens {
                *total_usage.cache_read_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = u.cache_miss_tokens {
                *total_usage.cache_miss_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = u.cache_creation_tokens {
                *total_usage.cache_creation_tokens.get_or_insert(0) += t;
            }
        }

        let _ = tx
            .send(AgentEvent::UsageUpdate {
                usage_total: total_usage.clone(),
                last_prompt_tokens: *last_prompt_tokens,
                context_breakdown: Some(context_breakdown.clone()),
            })
            .await;

        let mut iteration_compacted = false;
        let budget_decision = context_pipeline.budget_decision(prompt_tokens);
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
            prompt_tokens,
            completion_tokens,
            context_usage_pct: iteration_context_pct,
        };
        loop_recorder.record(completed.clone());
        append_persisted_trace_loop_event(persisted_trace_items, completed);

        if let Some(ref mut t) = trace {
            t.add_step(TraceStep {
                iteration,
                tool_name: None,
                tool_duration_ms: None,
                input_tokens: prompt_tokens as u64,
                output_tokens: completion_tokens as u64,
                cache_read_tokens: chunk_usage
                    .as_ref()
                    .and_then(|usage| usage.cache_read_tokens.map(u64::from)),
                cache_miss_tokens: chunk_usage
                    .as_ref()
                    .and_then(|usage| usage.cache_miss_tokens.map(u64::from)),
                cache_creation_tokens: chunk_usage
                    .as_ref()
                    .and_then(|usage| usage.cache_creation_tokens.map(u64::from)),
                context_usage_pct: iteration_context_pct,
                was_compacted: iteration_compacted,
            });
        }
    }
}

fn model_step_accounting_tokens(
    chunk_usage: Option<&Usage>,
    estimated_prompt_tokens: u32,
) -> (u32, u32, bool) {
    match chunk_usage {
        Some(usage) => (usage.prompt_tokens, usage.completion_tokens, true),
        None => (estimated_prompt_tokens, 0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_step_tokens_use_actual_usage_when_available() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
            ..Usage::default()
        };

        assert_eq!(
            model_step_accounting_tokens(Some(&usage), 999),
            (100, 20, true)
        );
    }

    #[test]
    fn model_step_tokens_fallback_to_estimated_prompt_tokens_without_usage() {
        assert_eq!(model_step_accounting_tokens(None, 321), (321, 0, false));
    }
}
