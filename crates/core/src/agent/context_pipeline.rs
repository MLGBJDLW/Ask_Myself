//! Context-window policy for the agent loop.

use crate::conversation::memory::{
    context_safety_buffer, model_context_window, trim_to_context_window,
};
use crate::llm::Message;

/// Start compacting before the provider's hard limit is close enough to make
/// one large tool result turn an otherwise healthy run into an overflow retry.
const AUTO_COMPACT_THRESHOLD: f32 = 0.78;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextPipeline {
    context_window: u32,
    max_response_tokens: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextBudgetDecision {
    pub(crate) budget_tokens: u32,
    pub(crate) usage_pct: f32,
    pub(crate) should_compact: bool,
}

impl ContextPipeline {
    pub(crate) fn new(
        model: &str,
        context_window_override: Option<u32>,
        max_response_tokens: u32,
    ) -> Self {
        Self {
            context_window: context_window_override.unwrap_or_else(|| model_context_window(model)),
            max_response_tokens,
        }
    }

    pub(crate) fn context_budget(self) -> u32 {
        self.context_window
            .saturating_sub(self.max_response_tokens)
            .saturating_sub(context_safety_buffer(self.context_window))
    }

    pub(crate) fn budget_decision(self, prompt_tokens: u32) -> ContextBudgetDecision {
        let budget = self.context_budget();
        let usage_pct = if budget == 0 {
            0.0
        } else {
            (prompt_tokens as f32 / budget as f32) * 100.0
        };
        ContextBudgetDecision {
            budget_tokens: budget,
            usage_pct,
            should_compact: budget > 0
                && prompt_tokens > (budget as f64 * AUTO_COMPACT_THRESHOLD as f64) as u32,
        }
    }

    pub(crate) fn trim_after_tool_results(self, messages: &[Message]) -> Vec<Message> {
        trim_to_context_window(
            messages,
            self.context_window
                .saturating_sub(context_safety_buffer(self.context_window)),
            self.max_response_tokens,
        )
    }

    pub(crate) fn trim_after_overflow_recovery(self, messages: &[Message]) -> Vec<Message> {
        let extra_safety = context_safety_buffer(self.context_window).saturating_mul(2);
        trim_to_context_window(
            messages,
            self.context_window.saturating_sub(extra_safety),
            self.max_response_tokens,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[test]
    fn compact_decision_tracks_budget_usage() {
        let pipeline = ContextPipeline::new("test-model", Some(1_000), 100);
        let decision = pipeline.budget_decision(800);
        assert!(decision.budget_tokens < 1_000);
        assert!(decision.usage_pct > 80.0);
        assert!(decision.should_compact);
    }

    #[test]
    fn trim_after_tool_results_keeps_system_message() {
        let pipeline = ContextPipeline::new("test-model", Some(200), 20);
        let messages = vec![
            Message::text(Role::System, "system"),
            Message::text(Role::User, "old ".repeat(200)),
            Message::text(Role::Tool, "tool result"),
        ];
        let trimmed = pipeline.trim_after_tool_results(&messages);
        assert_eq!(trimmed.first().unwrap().role, Role::System);
    }
}
