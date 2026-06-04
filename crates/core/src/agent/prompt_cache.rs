//! Lightweight prompt-cache stability diagnostics for agent model requests.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tracing::{debug, warn};

use super::*;
use crate::conversation::memory::estimate_message_tokens_for_model;

const MIN_CACHE_BREAK_TOKEN_DROP: u32 = 1_000;
const MAX_STABLE_CACHE_READ_RATIO: f32 = 0.95;
const PREFIX_HASH_TOKEN_WINDOWS: [u32; 3] = [1_024, 4_096, 16_384];

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptCacheSnapshot {
    provider_type: Option<ProviderType>,
    model: String,
    stable_system_hash: u64,
    tool_schema_hash: u64,
    tool_count: usize,
    tool_names: Vec<String>,
    message_hashes: Vec<u64>,
    prefix_hashes: [u64; 3],
    system_message_positions: Vec<usize>,
    system_message_hashes: Vec<u64>,
    dynamic_system_tokens: u32,
    tool_result_tokens: u32,
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

fn role_label(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn serialized_message_for_hash(message: &Message) -> String {
    let tool_calls = serde_json::to_string(&message.tool_calls).unwrap_or_default();
    format!(
        "role={};name={};text={};tool_calls={};reasoning={}",
        role_label(&message.role),
        message.name.as_deref().unwrap_or_default(),
        message.text_content(),
        tool_calls,
        message.reasoning_content.as_deref().unwrap_or_default()
    )
}

fn message_hash(message: &Message) -> u64 {
    hash_text(&serialized_message_for_hash(message))
}

fn prefix_hash_for_token_budget(model: &str, messages: &[Message], token_budget: u32) -> u64 {
    let mut used = 0u32;
    let mut serialized = String::new();

    for message in messages {
        if used >= token_budget {
            break;
        }
        let message_tokens = estimate_message_tokens_for_model(model, message);
        let message_text = serialized_message_for_hash(message);
        if used.saturating_add(message_tokens) <= token_budget {
            serialized.push_str(&message_text);
            used = used.saturating_add(message_tokens);
            continue;
        }

        let remaining = token_budget.saturating_sub(used);
        let keep_chars = remaining.saturating_mul(4) as usize;
        serialized.extend(message_text.chars().take(keep_chars));
        break;
    }

    hash_text(&serialized)
}

fn system_message_positions(messages: &[Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == Role::System).then_some(index))
        .collect()
}

fn system_message_hashes(messages: &[Message]) -> Vec<u64> {
    messages
        .iter()
        .filter(|message| message.role == Role::System)
        .map(message_hash)
        .collect()
}

fn dynamic_system_tokens(model: &str, messages: &[Message]) -> u32 {
    messages
        .iter()
        .enumerate()
        .filter(|(index, message)| *index > 0 && message.role == Role::System)
        .map(|(_, message)| estimate_message_tokens_for_model(model, message))
        .sum()
}

fn tool_result_tokens(model: &str, messages: &[Message]) -> u32 {
    messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .map(|message| estimate_message_tokens_for_model(model, message))
        .sum()
}

fn first_changed_message_index(previous: &[u64], next: &[u64]) -> Option<usize> {
    let max_len = previous.len().max(next.len());
    (0..max_len).find(|index| previous.get(*index) != next.get(*index))
}

fn snapshot_for(
    provider_type: Option<ProviderType>,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> PromptCacheSnapshot {
    PromptCacheSnapshot {
        provider_type,
        model: model.to_string(),
        stable_system_hash: hash_text(&stable_system_text(messages)),
        tool_schema_hash: tool_schema_hash(tools),
        tool_count: tools.len(),
        tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
        message_hashes: messages.iter().map(message_hash).collect(),
        prefix_hashes: PREFIX_HASH_TOKEN_WINDOWS
            .map(|window| prefix_hash_for_token_budget(model, messages, window)),
        system_message_positions: system_message_positions(messages),
        system_message_hashes: system_message_hashes(messages),
        dynamic_system_tokens: dynamic_system_tokens(model, messages),
        tool_result_tokens: tool_result_tokens(model, messages),
    }
}

fn diff_snapshots(previous: &PromptCacheSnapshot, next: &PromptCacheSnapshot) -> Vec<String> {
    let mut changes = Vec::new();
    if previous.provider_type != next.provider_type {
        changes.push(format!(
            "provider type changed ({:?} -> {:?})",
            previous.provider_type, next.provider_type
        ));
    }
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
    if let Some(index) = first_changed_message_index(&previous.message_hashes, &next.message_hashes)
    {
        changes.push(format!("first changed message index {index}"));
    }
    if previous.prefix_hashes != next.prefix_hashes {
        let changed = PREFIX_HASH_TOKEN_WINDOWS
            .iter()
            .zip(previous.prefix_hashes.iter().zip(next.prefix_hashes.iter()))
            .filter_map(|(window, (previous_hash, next_hash))| {
                (previous_hash != next_hash).then_some(format!("{window}t"))
            })
            .collect::<Vec<_>>()
            .join(", ");
        changes.push(format!("serialized prefix hash changed ({changed})"));
    }
    if previous.system_message_positions != next.system_message_positions {
        changes.push(format!(
            "system message positions changed ({:?} -> {:?})",
            previous.system_message_positions, next.system_message_positions
        ));
    }
    if previous.system_message_hashes != next.system_message_hashes {
        changes.push("system message hashes changed".to_string());
    }
    if previous.dynamic_system_tokens != next.dynamic_system_tokens {
        changes.push(format!(
            "dynamic system tokens changed ({} -> {})",
            previous.dynamic_system_tokens, next.dynamic_system_tokens
        ));
    }
    if previous.tool_result_tokens != next.tool_result_tokens {
        changes.push(format!(
            "tool result tokens changed ({} -> {})",
            previous.tool_result_tokens, next.tool_result_tokens
        ));
    }
    changes
}

impl PromptCacheTracker {
    fn begin(
        &mut self,
        provider_type: Option<ProviderType>,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) {
        let next = snapshot_for(provider_type, model, messages, tools);
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
        let provider_type = self
            .previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.provider_type);
        let model = self
            .previous_snapshot
            .as_ref()
            .map(|snapshot| snapshot.model.as_str())
            .unwrap_or_default();
        let previous = self.previous_cache_read_tokens;
        self.previous_cache_read_tokens = Some(cache_read_tokens);
        let Some(previous_cache_read_tokens) = previous else {
            debug!(
                ?provider_type,
                model, cache_read_tokens, cache_creation_tokens, "prompt cache baseline recorded"
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
                ?provider_type,
                model,
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
            tracker.begin(self.config.provider_type, model, messages, tools);
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
            snapshot_for(None, "m", &first, &[]).stable_system_hash,
            snapshot_for(None, "m", &second, &[]).stable_system_hash
        );
    }

    #[test]
    fn snapshot_diff_reports_runtime_system_prefix_changes() {
        let first = snapshot_for(
            Some(ProviderType::DeepSeek),
            "m",
            &[msg(Role::System, "stable"), msg(Role::System, "date one")],
            &[],
        );
        let second = snapshot_for(
            Some(ProviderType::DeepSeek),
            "m",
            &[msg(Role::System, "stable"), msg(Role::System, "date two")],
            &[],
        );

        let diff = diff_snapshots(&first, &second);

        assert!(diff
            .iter()
            .any(|change| change == "first changed message index 1"));
        assert!(diff
            .iter()
            .any(|change| change == "system message hashes changed"));
        assert!(diff
            .iter()
            .any(|change| change.starts_with("serialized prefix hash changed")));
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
        let previous = snapshot_for(None, "m", &[msg(Role::System, "stable")], &[tool_a]);
        let next = snapshot_for(None, "m", &[msg(Role::System, "stable")], &[tool_b]);
        assert_eq!(
            diff_snapshots(&previous, &next),
            vec!["tool schemas changed with same count"]
        );
    }

    #[test]
    fn snapshot_diff_reports_provider_type_changes() {
        let previous = snapshot_for(
            Some(ProviderType::OpenAi),
            "m",
            &[msg(Role::System, "stable")],
            &[],
        );
        let next = snapshot_for(
            Some(ProviderType::DeepSeek),
            "m",
            &[msg(Role::System, "stable")],
            &[],
        );

        assert_eq!(
            diff_snapshots(&previous, &next),
            vec!["provider type changed (Some(OpenAi) -> Some(DeepSeek))"]
        );
    }
}
