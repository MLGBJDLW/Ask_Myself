//! Lightweight prompt-cache stability diagnostics for agent model requests.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tracing::{debug, warn};

use super::*;

const MIN_CACHE_BREAK_TOKEN_DROP: u32 = 1_000;
const MAX_STABLE_CACHE_READ_RATIO: f32 = 0.95;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptCacheSnapshot {
    model: String,
    stable_system_hash: u64,
    tool_schema_hash: u64,
    tool_count: usize,
    tool_names: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct PromptCacheTracker {
    previous_snapshot: Option<PromptCacheSnapshot>,
    previous_cache_read_tokens: Option<u32>,
    pending_changes: Vec<String>,
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn stable_system_text(messages: &[Message]) -> String {
    messages
        .iter()
        .find(|message| message.role == Role::System)
        .map(Message::text_content)
        .unwrap_or_default()
}

fn tool_schema_hash(tools: &[ToolDefinition]) -> u64 {
    let serialized = serde_json::to_string(tools).unwrap_or_default();
    hash_text(&serialized)
}

fn snapshot_for(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> PromptCacheSnapshot {
    PromptCacheSnapshot {
        model: model.to_string(),
        stable_system_hash: hash_text(&stable_system_text(messages)),
        tool_schema_hash: tool_schema_hash(tools),
        tool_count: tools.len(),
        tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
    }
}

fn diff_snapshots(previous: &PromptCacheSnapshot, next: &PromptCacheSnapshot) -> Vec<String> {
    let mut changes = Vec::new();
    if previous.model != next.model {
        changes.push(format!(
            "model changed ({} -> {})",
            previous.model, next.model
        ));
    }
    if previous.stable_system_hash != next.stable_system_hash {
        changes.push("stable system prompt changed".to_string());
    }
    if previous.tool_schema_hash != next.tool_schema_hash {
        if previous.tool_count == next.tool_count {
            changes.push("tool schemas changed with same count".to_string());
        } else {
            changes.push(format!(
                "tool count changed ({} -> {})",
                previous.tool_count, next.tool_count
            ));
        }
    }
    changes
}

impl PromptCacheTracker {
    fn begin(&mut self, model: &str, messages: &[Message], tools: &[ToolDefinition]) {
        let next = snapshot_for(model, messages, tools);
        self.pending_changes = self
            .previous_snapshot
            .as_ref()
            .map(|previous| diff_snapshots(previous, &next))
            .unwrap_or_default();
        self.previous_snapshot = Some(next);
    }

    fn complete(&mut self, cache_read_tokens: Option<u32>, cache_creation_tokens: Option<u32>) {
        let Some(cache_read_tokens) = cache_read_tokens else {
            self.pending_changes.clear();
            return;
        };
        let previous = self.previous_cache_read_tokens;
        self.previous_cache_read_tokens = Some(cache_read_tokens);
        let Some(previous_cache_read_tokens) = previous else {
            debug!(
                cache_read_tokens,
                cache_creation_tokens, "prompt cache baseline recorded"
            );
            self.pending_changes.clear();
            return;
        };

        let token_drop = previous_cache_read_tokens.saturating_sub(cache_read_tokens);
        let ratio = if previous_cache_read_tokens == 0 {
            1.0
        } else {
            cache_read_tokens as f32 / previous_cache_read_tokens as f32
        };
        if token_drop >= MIN_CACHE_BREAK_TOKEN_DROP && ratio < MAX_STABLE_CACHE_READ_RATIO {
            let reason = if self.pending_changes.is_empty() {
                "prompt unchanged; likely provider-side TTL/routing/eviction".to_string()
            } else {
                self.pending_changes.join(", ")
            };
            warn!(
                previous_cache_read_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                reason,
                "provider prompt cache read dropped"
            );
        }
        self.pending_changes.clear();
    }
}

impl AgentExecutor {
    pub(super) fn begin_prompt_cache_observation(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) {
        if let Ok(mut tracker) = self.prompt_cache_tracker.lock() {
            tracker.begin(model, messages, tools);
        }
    }

    pub(super) fn complete_prompt_cache_observation(&self, usage: Option<&Usage>) {
        let Some(usage) = usage else {
            return;
        };
        if let Ok(mut tracker) = self.prompt_cache_tracker.lock() {
            tracker.complete(usage.cache_read_tokens, usage.cache_creation_tokens);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, text: &str) -> Message {
        Message::text(role, text)
    }

    #[test]
    fn stable_system_hash_ignores_runtime_system_blocks() {
        let first = vec![msg(Role::System, "stable"), msg(Role::System, "date one")];
        let second = vec![msg(Role::System, "stable"), msg(Role::System, "date two")];
        assert_eq!(
            snapshot_for("m", &first, &[]).stable_system_hash,
            snapshot_for("m", &second, &[]).stable_system_hash
        );
    }

    #[test]
    fn snapshot_diff_reports_tool_schema_changes() {
        let tool_a = ToolDefinition {
            name: "search".into(),
            description: "Search".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let mut tool_b = tool_a.clone();
        tool_b.description = "Search docs".into();
        let previous = snapshot_for("m", &[msg(Role::System, "stable")], &[tool_a]);
        let next = snapshot_for("m", &[msg(Role::System, "stable")], &[tool_b]);
        assert_eq!(
            diff_snapshots(&previous, &next),
            vec!["tool schemas changed with same count"]
        );
    }
}
