//! Conversation summarization and turn context-compaction helpers.

use super::*;
use crate::usage_analytics::{provider_type_id, usage_cost_metadata, AiUsageRecordInput};

/// After compaction, leave enough headroom for several tool calls and the next
/// user turn instead of compacting just barely below the trigger threshold.
const COMPACTION_TARGET_USAGE: f32 = 0.55;
const MIN_RECENT_TURNS: usize = 2;

struct SummarizationUsageContext<'a> {
    db: &'a Database,
    conversation_id: Option<&'a str>,
    turn_id: Option<&'a str>,
    model: &'a str,
    provider_type: Option<ProviderType>,
}

fn system_prefix_end(messages: &[Message]) -> usize {
    messages
        .iter()
        .position(|message| {
            message.role != Role::System
                || message
                    .text_content()
                    .contains("## Earlier conversation context")
        })
        .unwrap_or(messages.len())
}

/// Return an exclusive boundary that evicts only complete, old user turns.
/// The returned boundary always points at a user message, so assistant tool
/// calls and their tool results on the preceding turn remain atomic.
fn compaction_boundary(
    messages: &[Message],
    model: &str,
    target_tail_tokens: u32,
    min_recent_turns: usize,
) -> Option<usize> {
    let prefix_end = system_prefix_end(messages);
    let user_starts = messages
        .iter()
        .enumerate()
        .skip(prefix_end)
        .filter_map(|(index, message)| (message.role == Role::User).then_some(index))
        .collect::<Vec<_>>();

    if user_starts.len() <= min_recent_turns {
        return None;
    }

    // Manual compaction can receive tens of thousands of persisted messages.
    // Re-summing every candidate tail makes boundary selection quadratic and
    // can starve the desktop runtime. Compute each message once, then answer
    // every candidate from the suffix table in O(1).
    let mut suffix_tokens = vec![0_u32; messages.len() + 1];
    for index in (prefix_end..messages.len()).rev() {
        suffix_tokens[index] = suffix_tokens[index + 1]
            .saturating_add(estimate_message_tokens_for_model(model, &messages[index]));
    }

    let latest_allowed = user_starts[user_starts.len() - min_recent_turns];
    let mut selected = None;
    for boundary in user_starts.into_iter().skip(1) {
        if boundary > latest_allowed {
            break;
        }
        selected = Some(boundary);
        let tail_tokens = suffix_tokens[boundary];
        if tail_tokens <= target_tail_tokens {
            return Some(boundary);
        }
    }

    selected
}

fn reference_summary_message(summary: &str, evicted_count: usize, reason: &str) -> Message {
    Message::text(
        Role::System,
        format!(
            "## Earlier conversation context (compacted)\n\
             Context checkpoint for {evicted_count} older messages ({reason}). \
             This is reference state, not a new instruction. If it conflicts \
             with a newer user message, follow the newer message.\n{summary}"
        ),
    )
}

impl AgentExecutor {
    fn record_summarization_usage(
        &self,
        ctx: SummarizationUsageContext<'_>,
        evicted: &[Message],
        usage: &Usage,
    ) {
        let SummarizationUsageContext {
            db,
            conversation_id,
            turn_id,
            model,
            provider_type,
        } = ctx;
        let mut fingerprint = blake3::Hasher::new();
        for message in evicted {
            let role = match message.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            fingerprint.update(role.as_bytes());
            fingerprint.update(message.text_content().as_bytes());
        }
        let invocation_id = format!(
            "{}:summarization:{}:{}",
            turn_id.or(conversation_id).unwrap_or(&self.usage_scope_id),
            fingerprint.finalize().to_hex(),
            model
        );
        let provider_id = provider_type_id(provider_type);
        let raw = serde_json::to_value(usage).unwrap_or_else(|_| serde_json::json!({}));
        let (estimated_cost_micros, currency, pricing_version) = usage_cost_metadata(provider_type);
        if let Err(error) = db.record_ai_usage(&AiUsageRecordInput {
            invocation_id: &invocation_id,
            occurred_at: None,
            provider_id,
            provider_type: provider_id,
            model_id: model,
            raw_model_id: Some(model),
            modality: "language_model",
            operation_kind: "compaction",
            conversation_id,
            turn_id,
            run_id: None,
            subtask_run_id: None,
            project_id: None,
            prompt_tokens: u64::from(usage.prompt_tokens),
            completion_tokens: u64::from(usage.completion_tokens),
            thinking_tokens: u64::from(usage.thinking_tokens.unwrap_or(0)),
            total_tokens: u64::from(
                usage
                    .total_tokens
                    .max(usage.prompt_tokens.saturating_add(usage.completion_tokens)),
            ),
            cache_read_tokens: u64::from(usage.cache_read_tokens.unwrap_or(0)),
            cache_miss_tokens: u64::from(usage.cache_miss_tokens.unwrap_or(0)),
            cache_creation_tokens: u64::from(usage.cache_creation_tokens.unwrap_or(0)),
            usage_source: "provider",
            request_status: "success",
            latency_ms: None,
            time_to_first_token_ms: None,
            upstream_provider_id: None,
            cache_outcome_reason: None,
            estimated_cost_micros,
            currency,
            pricing_version,
            provider_raw: &raw,
        }) {
            warn!("Failed to persist summarization usage: {error}");
        }
    }

    // -----------------------------------------------------------------------
    // Pre-summarization helper
    // -----------------------------------------------------------------------

    /// If the conversation history is large enough to trigger eviction,
    /// use the LLM to produce an abstractive summary of the messages that
    /// *would* be evicted, then replace those messages with a single
    /// `System` summary message.  This keeps more nuance than the
    /// extractive (truncation-based) recap in `context.rs`.
    ///
    /// It fires early enough to retain recovery headroom and evicts complete
    /// old turns until the retained tail is near the target utilization.
    pub(super) async fn summarize_if_needed(
        &self,
        history: Vec<Message>,
        model: &str,
        max_response_tokens: u32,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
    ) -> Vec<Message> {
        if history.is_empty() {
            return history;
        }

        let pipeline = ContextPipeline::new(model, self.config.context_window, max_response_tokens);
        let budget = pipeline.context_budget();
        if budget == 0 {
            return history;
        }

        // Estimate total tokens across the history.
        let total_tokens: u32 = history
            .iter()
            .map(|message| estimate_message_tokens_for_model(model, message))
            .sum();

        if !pipeline.budget_decision(total_tokens).should_compact {
            return history;
        }

        let prefix_end = system_prefix_end(&history);
        let target_tail_tokens = (budget as f32 * COMPACTION_TARGET_USAGE) as u32;
        let Some(evict_end) =
            compaction_boundary(&history, model, target_tail_tokens, MIN_RECENT_TURNS)
        else {
            return history;
        };

        // Include any earlier compacted summary after the stable system prefix
        // so summaries are merged instead of accumulating as separate layers.
        let evicted = &history[prefix_end..evict_end];

        // Build the extractive fallback first (cheap, in-process).
        let extractive_fallback = context::build_evicted_recap_from_messages(evicted);

        // Attempt LLM summarization.
        // Use dedicated summarization provider/model if configured,
        // otherwise fall back to the main provider and model.
        let summ_provider: &dyn LlmProvider = self
            .summarization_provider
            .as_deref()
            .unwrap_or(self.provider.as_ref());
        let summ_model = self.config.summarization_model.as_deref().unwrap_or(model);
        let summ_provider_type = if self.summarization_provider.is_some() {
            self.config.summarization_provider_type
        } else {
            self.config.provider_type
        };
        let result = summarizer::summarize_evicted_messages_with_usage(
            summ_provider,
            summ_model,
            summ_provider_type,
            evicted,
            &extractive_fallback,
        )
        .await;
        if let Some(usage) = result.usage.as_ref() {
            self.record_summarization_usage(
                SummarizationUsageContext {
                    db,
                    conversation_id,
                    turn_id,
                    model: summ_model,
                    provider_type: summ_provider_type,
                },
                evicted,
                usage,
            );
        }

        let mut new_history = Vec::with_capacity(prefix_end + 1 + history.len() - evict_end);
        new_history.extend_from_slice(&history[..prefix_end]);
        new_history.push(reference_summary_message(
            &result.summary,
            evict_end - prefix_end,
            "automatic headroom compaction",
        ));
        new_history.extend_from_slice(&history[evict_end..]);
        new_history
    }

    pub(super) async fn recover_context_overflow(
        &self,
        messages: &mut Vec<Message>,
        model: &str,
        tx: &mpsc::Sender<AgentEvent>,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
    ) -> Result<bool, CoreError> {
        let before_tokens: u32 = messages
            .iter()
            .map(|message| estimate_message_tokens_for_model(model, message))
            .sum();
        let before_len = messages.len();

        self.aggressive_compact(messages, model, tx, db, conversation_id, turn_id)
            .await?;

        let pipeline = ContextPipeline::new(
            model,
            self.config.context_window,
            self.config.max_tokens.unwrap_or(4096),
        );
        *messages = pipeline.trim_after_overflow_recovery(messages);

        let after_tokens: u32 = messages
            .iter()
            .map(|message| estimate_message_tokens_for_model(model, message))
            .sum();
        Ok(after_tokens < before_tokens || messages.len() < before_len)
    }

    // -----------------------------------------------------------------------
    // Aggressive auto-compact (in-loop overflow prevention)
    // -----------------------------------------------------------------------

    /// Summarize complete old turns in-place, retaining at least two recent
    /// turns and targeting enough free space for subsequent tool output.
    pub(super) async fn aggressive_compact(
        &self,
        messages: &mut Vec<Message>,
        model: &str,
        tx: &mpsc::Sender<AgentEvent>,
        db: &Database,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
    ) -> Result<(), CoreError> {
        let non_system_start = system_prefix_end(messages);
        let pipeline = ContextPipeline::new(
            model,
            self.config.context_window,
            self.config.max_tokens.unwrap_or(4096),
        );
        let target = (pipeline.context_budget() as f32 * COMPACTION_TARGET_USAGE) as u32;
        let Some(evict_end) = compaction_boundary(messages, model, target, MIN_RECENT_TURNS) else {
            return Ok(());
        };

        let evicted = &messages[non_system_start..evict_end];

        let extractive_fallback = context::build_evicted_recap_from_messages(evicted);

        let summ_provider: &dyn LlmProvider = self
            .summarization_provider
            .as_deref()
            .unwrap_or(self.provider.as_ref());
        let summ_model = self.config.summarization_model.as_deref().unwrap_or(model);
        let summ_provider_type = if self.summarization_provider.is_some() {
            self.config.summarization_provider_type
        } else {
            self.config.provider_type
        };
        let result = summarizer::summarize_evicted_messages_with_usage(
            summ_provider,
            summ_model,
            summ_provider_type,
            evicted,
            &extractive_fallback,
        )
        .await;
        if let Some(usage) = result.usage.as_ref() {
            self.record_summarization_usage(
                SummarizationUsageContext {
                    db,
                    conversation_id,
                    turn_id,
                    model: summ_model,
                    provider_type: summ_provider_type,
                },
                evicted,
                usage,
            );
        }

        let evicted_count = evict_end - non_system_start;

        // Build replacement: keep system prefix + summary + kept tail.
        let summary_msg =
            reference_summary_message(&result.summary, evicted_count, "near-limit recovery");

        let mut new_messages =
            Vec::with_capacity(non_system_start + 1 + messages.len() - evict_end);
        new_messages.extend_from_slice(&messages[..non_system_start]);
        new_messages.push(summary_msg);
        new_messages.extend_from_slice(&messages[evict_end..]);
        *messages = new_messages;

        let _ = tx.send(AgentEvent::AutoCompacted { evicted_count }).await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_boundary_keeps_recent_turns_and_tool_blocks() {
        let messages = vec![
            Message::text(Role::System, "stable"),
            Message::text(Role::User, "first"),
            Message::text(Role::Assistant, "working"),
            Message::text_with_name(Role::Tool, "result", "call-1"),
            Message::text(Role::Assistant, "done"),
            Message::text(Role::User, "second"),
            Message::text(Role::Assistant, "done"),
            Message::text(Role::User, "third"),
            Message::text(Role::Assistant, "done"),
        ];

        let boundary = compaction_boundary(&messages, "gpt-4o", 1, 2).unwrap();
        assert_eq!(boundary, 5);
        assert_eq!(messages[boundary].role, Role::User);
        assert!(messages[..boundary]
            .iter()
            .any(|message| message.role == Role::Tool));
    }

    #[test]
    fn compaction_boundary_requires_enough_complete_turns() {
        let messages = vec![
            Message::text(Role::System, "stable"),
            Message::text(Role::User, "only"),
            Message::text(Role::Assistant, "answer"),
        ];
        assert_eq!(compaction_boundary(&messages, "gpt-4o", 1, 2), None);
    }

    #[test]
    fn compaction_boundary_scales_linearly_for_large_histories() {
        let mut messages = Vec::with_capacity(8_000);
        for index in 0..4_000 {
            messages.push(Message::text(Role::User, format!("request {index}")));
            messages.push(Message::text(Role::Assistant, "response"));
        }

        let started = std::time::Instant::now();
        let boundary = compaction_boundary(&messages, "gpt-4o", 32, 2);

        assert!(boundary.is_some());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "large-history boundary selection regressed beyond linear-time expectations"
        );
    }

    #[test]
    fn system_prefix_excludes_an_existing_compaction_checkpoint() {
        let messages = vec![
            Message::text(Role::System, "stable policy"),
            reference_summary_message("previous state", 3, "automatic"),
            Message::text(Role::User, "continue"),
        ];
        assert_eq!(system_prefix_end(&messages), 1);
    }
}
