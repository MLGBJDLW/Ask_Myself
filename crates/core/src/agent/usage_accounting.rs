//! Token usage, model-step trace, and automatic compaction accounting.

use super::context;
use super::turn_state::{TurnPhase, TurnStateMachine};
use super::*;

pub(super) struct UsageAccountingContext<'a> {
    pub(super) db: &'a Database,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
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

#[derive(Debug, Clone, Copy)]
pub(super) struct ModelStepUsageReport {
    /// Whether context was rewritten after this request and therefore before
    /// the next model step. The current request's cache sample must not be
    /// mislabeled with a compaction that had not happened yet.
    pub(super) compacted_after_step: bool,
}

impl AgentExecutor {
    pub(super) fn record_model_step_failure(
        &self,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        model: &str,
        iteration: u32,
        error: &CoreError,
    ) {
        let operation_kind = match self.config.request_kind {
            AgentRequestKind::MainAgentStep => "agent_main",
            AgentRequestKind::SubagentWorker => "subagent",
        };
        let invocation_id = format!(
            "{}:{}:{}:{}",
            self.usage_subtask_run_id
                .as_deref()
                .or(self.usage_run_id.as_deref())
                .or(turn_id)
                .unwrap_or(&self.usage_scope_id),
            operation_kind,
            iteration,
            model
        );
        let provider_id = crate::usage_analytics::provider_type_id(self.config.provider_type);
        let raw = serde_json::json!({ "error": error.to_string() });
        let (estimated_cost_micros, currency, pricing_version) =
            crate::usage_analytics::usage_cost_metadata(self.config.provider_type);
        if let Err(record_error) = db.record_ai_usage(&crate::usage_analytics::AiUsageRecordInput {
            invocation_id: &invocation_id,
            occurred_at: None,
            provider_id,
            provider_type: provider_id,
            model_id: model,
            raw_model_id: Some(model),
            modality: "language_model",
            operation_kind,
            conversation_id,
            turn_id,
            run_id: self.usage_run_id.as_deref(),
            subtask_run_id: self.usage_subtask_run_id.as_deref(),
            project_id: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            thinking_tokens: 0,
            total_tokens: 0,
            cache_read_tokens: 0,
            cache_miss_tokens: 0,
            cache_creation_tokens: 0,
            usage_source: "unknown",
            request_status: "error",
            latency_ms: None,
            time_to_first_token_ms: None,
            upstream_provider_id: None,
            cache_outcome_reason: None,
            estimated_cost_micros,
            currency,
            pricing_version,
            provider_raw: &raw,
        }) {
            warn!("Failed to persist failed AI invocation: {record_error}");
        }
    }

    pub(super) async fn record_model_step_usage(
        &self,
        ctx: UsageAccountingContext<'_>,
        iteration: u32,
        tool_call_count: usize,
        finish_reason: Option<String>,
        chunk_usage: Option<Usage>,
        request_latency_ms: u64,
        time_to_first_token_ms: Option<u64>,
        cache_outcome_reason: Option<&str>,
    ) -> ModelStepUsageReport {
        let UsageAccountingContext {
            db,
            conversation_id,
            turn_id,
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
        let normalized_cache_miss_tokens = chunk_usage
            .as_ref()
            .and_then(|usage| normalized_cache_miss_tokens(self.config.provider_type, usage));
        let context_breakdown = context::estimate_context_usage_breakdown_for_model(
            model,
            messages,
            tool_defs,
            actual_prompt_tokens,
        );
        let (prompt_tokens, completion_tokens, _has_actual_usage) =
            model_step_accounting_tokens(chunk_usage.as_ref(), context_breakdown.total_tokens);
        let operation_kind = match self.config.request_kind {
            AgentRequestKind::MainAgentStep => "agent_main",
            AgentRequestKind::SubagentWorker => "subagent",
        };
        if let Err(error) = db.record_model_step_usage(
            conversation_id,
            turn_id,
            Some(&self.usage_scope_id),
            self.usage_run_id.as_deref(),
            self.usage_subtask_run_id.as_deref(),
            iteration,
            self.config.provider_type,
            model,
            operation_kind,
            chunk_usage.as_ref(),
            context_breakdown.total_tokens,
            normalized_cache_miss_tokens,
            Some(request_latency_ms),
            time_to_first_token_ms,
            cache_outcome_reason,
        ) {
            warn!("Failed to persist canonical AI usage: {error}");
        }
        *last_prompt_tokens = prompt_tokens;
        *last_context_breakdown = Some(context_breakdown.clone());
        if let Some(u) = chunk_usage.as_ref() {
            total_usage.prompt_tokens += u.prompt_tokens;
            total_usage.completion_tokens += u.completion_tokens;
            total_usage.total_tokens += u.total_tokens;
            if let Some(t) = u.thinking_tokens {
                *total_usage.thinking_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = u.tool_prompt_tokens {
                *total_usage.tool_prompt_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = u.cache_read_tokens {
                *total_usage.cache_read_tokens.get_or_insert(0) += t;
            }
            if let Some(t) = normalized_cache_miss_tokens {
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
            let before_messages = prompt_cache::message_sequence_fingerprint(messages);
            let started = TurnLoopEvent::CompactionStarted {
                reason: "auto".to_string(),
                message_count: before_message_count,
            };
            loop_recorder.record(started.clone());
            append_persisted_trace_loop_event(persisted_trace_items, started);
            turn_state.transition_to(TurnPhase::Compacting);
            if let Err(e) = self
                .aggressive_compact(messages, model, tx, db, conversation_id, turn_id)
                .await
            {
                warn!("Auto-compact failed: {e}");
            } else {
                let after_messages = prompt_cache::message_sequence_fingerprint(messages);
                iteration_compacted = before_messages != after_messages;
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
                request_kind: self.config.request_kind.as_str().to_string(),
                tool_name: None,
                tool_duration_ms: None,
                input_tokens: prompt_tokens as u64,
                output_tokens: completion_tokens as u64,
                cache_read_tokens: chunk_usage
                    .as_ref()
                    .and_then(|usage| usage.cache_read_tokens.map(u64::from)),
                cache_miss_tokens: normalized_cache_miss_tokens.map(u64::from),
                cache_creation_tokens: chunk_usage
                    .as_ref()
                    .and_then(|usage| usage.cache_creation_tokens.map(u64::from)),
                context_usage_pct: iteration_context_pct,
                // Preserve the persisted TraceStep contract: this flag records
                // compaction performed after this model step. Prompt-cache
                // observations carry the separate pre-request attribution.
                was_compacted: iteration_compacted,
            });
        }

        ModelStepUsageReport {
            compacted_after_step: iteration_compacted,
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

pub(super) fn normalized_cache_miss_tokens(
    provider_type: Option<ProviderType>,
    usage: &Usage,
) -> Option<u32> {
    usage.cache_miss_tokens.or_else(|| {
        if matches!(provider_type, Some(ProviderType::Anthropic))
            && (usage.cache_read_tokens.is_some() || usage.cache_creation_tokens.is_some())
        {
            // Anthropic reports uncached input, cache reads, and cache writes as
            // disjoint counters. Cache writes are misses for hit-rate purposes.
            Some(
                usage
                    .prompt_tokens
                    .saturating_add(usage.cache_creation_tokens.unwrap_or(0)),
            )
        } else {
            usage
                .cache_read_tokens
                .map(|read| usage.prompt_tokens.saturating_sub(read))
        }
    })
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

    #[test]
    fn usage_cache_miss_prefers_provider_value() {
        let usage = Usage {
            prompt_tokens: 1_000,
            cache_read_tokens: Some(900),
            cache_miss_tokens: Some(25),
            ..Usage::default()
        };

        assert_eq!(
            normalized_cache_miss_tokens(Some(ProviderType::OpenAi), &usage),
            Some(25)
        );
    }

    #[test]
    fn usage_cache_miss_falls_back_to_prompt_minus_cache_read() {
        let usage = Usage {
            prompt_tokens: 1_000,
            cache_read_tokens: Some(920),
            cache_miss_tokens: None,
            ..Usage::default()
        };

        assert_eq!(
            normalized_cache_miss_tokens(Some(ProviderType::OpenAi), &usage),
            Some(80)
        );
    }

    #[test]
    fn usage_cache_miss_saturates_when_cache_read_exceeds_prompt() {
        let usage = Usage {
            prompt_tokens: 100,
            cache_read_tokens: Some(128),
            cache_miss_tokens: None,
            ..Usage::default()
        };

        assert_eq!(
            normalized_cache_miss_tokens(Some(ProviderType::OpenAi), &usage),
            Some(0)
        );
    }

    #[test]
    fn anthropic_cache_miss_uses_disjoint_input_and_creation_counters() {
        let usage = Usage {
            prompt_tokens: 100,
            cache_read_tokens: Some(900),
            cache_miss_tokens: None,
            cache_creation_tokens: Some(50),
            ..Usage::default()
        };

        assert_eq!(
            normalized_cache_miss_tokens(Some(ProviderType::Anthropic), &usage),
            Some(150)
        );
    }
}
