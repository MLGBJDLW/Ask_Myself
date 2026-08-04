//! Lightweight prompt-cache stability diagnostics for agent model requests.

use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::*;
use crate::conversation::memory::estimate_message_tokens_for_model;
use crate::db::Database;
use crate::llm::prompt_cache::{PromptCacheMode, PromptCacheProfile};

const MIN_CACHE_BREAK_TOKEN_DROP: u32 = 1_000;
const MAX_STABLE_CACHE_READ_RATIO: f32 = 0.95;
const PREFIX_HASH_TOKEN_WINDOWS: [u32; 3] = [1_024, 4_096, 16_384];
const DEEPSEEK_CACHE_SETTLE_RISK_MS: u64 = 2_000;
const CACHE_RATIO_BASIS_POINTS: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromptCacheMessageFingerprint {
    role: String,
    text_hash: u64,
    tool_calls_hash: u64,
    reasoning_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromptCacheSnapshot {
    provider_type: Option<ProviderType>,
    model: String,
    #[serde(default)]
    cache_profile: PromptCacheProfile,
    stable_system_hash: u64,
    tool_schema_hash: u64,
    tool_count: usize,
    tool_names: Vec<String>,
    message_hashes: Vec<u64>,
    message_fingerprints: Vec<PromptCacheMessageFingerprint>,
    prefix_hashes: [u64; 3],
    system_message_positions: Vec<usize>,
    system_message_hashes: Vec<u64>,
    dynamic_system_tokens: u32,
    tool_result_tokens: u32,
    /// Estimated token cost for each message, aligned with `message_hashes`.
    /// Defaults keep snapshots written before diagnostics v2 readable.
    #[serde(default)]
    estimated_message_tokens: Vec<u32>,
    #[serde(default)]
    estimated_tool_tokens: u32,
    #[serde(default)]
    estimated_prompt_tokens: u32,
    /// Per-tool hashes aligned with `tool_names`, used to name schema drift
    /// instead of reporting only an opaque aggregate hash change.
    #[serde(default)]
    tool_schema_hashes: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PromptCacheSnapshotSource {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromptCacheTraceObservation {
    version: u32,
    request_kind: String,
    snapshot: PromptCacheSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_snapshot_source: Option<PromptCacheSnapshotSource>,
    /// `coldStart`, `warmAppend`, `warmReplay`, or the concrete prefix
    /// invalidation class. Cold starts are intentionally not mixed with warm
    /// append-only samples when diagnosing provider cache quality.
    sample_kind: String,
    prefix_changed: bool,
    changes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_changed_message_index: Option<usize>,
    common_prefix_message_count: usize,
    estimated_request_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_reusable_prefix_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_changed_suffix_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_reuse_ratio_bps: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_names_added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_names_removed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_schemas_changed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_miss_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_cache_hit_rate_bps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    was_compacted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_step_interval_ms: Option<u64>,
    fast_cache_settle_risk: bool,
    usage_coverage: String,
    cache_outcome_reason: String,
}

#[derive(Debug, Clone)]
struct PendingPromptCacheObservation {
    request_kind: String,
    snapshot: PromptCacheSnapshot,
    previous_snapshot_source: Option<PromptCacheSnapshotSource>,
    diff: PromptCacheDiff,
    model_step_interval_ms: Option<u64>,
    fast_cache_settle_risk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptCacheDiff {
    sample_kind: String,
    prefix_changed: bool,
    changes: Vec<String>,
    first_changed_message_index: Option<usize>,
    common_prefix_message_count: usize,
    estimated_request_tokens: u32,
    estimated_reusable_prefix_tokens: Option<u32>,
    estimated_changed_suffix_tokens: Option<u32>,
    estimated_reuse_ratio_bps: Option<u32>,
    tool_names_added: Vec<String>,
    tool_names_removed: Vec<String>,
    tool_schemas_changed: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct PromptCacheTracker {
    previous_snapshot: Option<PromptCacheSnapshot>,
    previous_snapshot_source: Option<PromptCacheSnapshotSource>,
    previous_cache_read_tokens: Option<u32>,
    pending_observation: Option<PendingPromptCacheObservation>,
    previous_begin_at: Option<Instant>,
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

fn individual_tool_schema_hashes(tools: &[ToolDefinition]) -> Vec<u64> {
    tools
        .iter()
        .map(|tool| hash_text(&serde_json::to_string(tool).unwrap_or_default()))
        .collect()
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
    let base = format!(
        "role={};name={};text={};tool_calls={};reasoning={}",
        role_label(&message.role),
        message.name.as_deref().unwrap_or_default(),
        message.text_content(),
        tool_calls,
        message.reasoning_content.as_deref().unwrap_or_default()
    );
    let image_fingerprints = message
        .parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Image { media_type, data } => Some((media_type.as_str(), hash_text(data))),
            ContentPart::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    if image_fingerprints.is_empty() {
        base
    } else {
        format!(
            "{base};images={}",
            serde_json::to_string(&image_fingerprints).unwrap_or_default()
        )
    }
}

fn estimate_prompt_cache_message_tokens(model: &str, message: &Message) -> u32 {
    let base = estimate_message_tokens_for_model(model, message);
    message
        .reasoning_content
        .as_deref()
        .map(|reasoning| base.saturating_add(estimate_tokens_for_model(model, reasoning)))
        .unwrap_or(base)
}

fn message_hash(message: &Message) -> u64 {
    hash_text(&serialized_message_for_hash(message))
}

/// A compact structural fingerprint used to detect whether a context
/// compaction actually rewrote the message sequence. Unlike serializing the
/// entire history, this avoids materializing another copy of large image data.
pub(super) fn message_sequence_fingerprint(messages: &[Message]) -> u64 {
    let mut hasher = DefaultHasher::new();
    messages.len().hash(&mut hasher);
    for message in messages {
        message_hash(message).hash(&mut hasher);
    }
    hasher.finish()
}

fn message_fingerprint(message: &Message) -> PromptCacheMessageFingerprint {
    PromptCacheMessageFingerprint {
        role: role_label(&message.role).to_string(),
        text_hash: hash_text(&message.text_content()),
        tool_calls_hash: hash_text(&serde_json::to_string(&message.tool_calls).unwrap_or_default()),
        reasoning_hash: hash_text(message.reasoning_content.as_deref().unwrap_or_default()),
    }
}

fn prefix_hash_for_token_budget(model: &str, messages: &[Message], token_budget: u32) -> u64 {
    let mut used = 0u32;
    let mut serialized = String::new();

    for message in messages {
        if used >= token_budget {
            break;
        }
        let message_tokens = estimate_prompt_cache_message_tokens(model, message);
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

#[cfg(test)]
fn snapshot_for(
    provider_type: Option<ProviderType>,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> PromptCacheSnapshot {
    let provider = provider_type.unwrap_or(ProviderType::Custom);
    let api_style = if provider == ProviderType::Anthropic {
        crate::llm::prompt_cache::PromptCacheApiStyle::AnthropicMessages
    } else {
        crate::llm::prompt_cache::PromptCacheApiStyle::OpenAiCompatible
    };
    let profile =
        crate::llm::prompt_cache::resolve_prompt_cache_profile(provider, None, api_style, model);
    snapshot_for_profile(provider_type, profile, model, messages, tools)
}

fn snapshot_for_profile(
    provider_type: Option<ProviderType>,
    cache_profile: PromptCacheProfile,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> PromptCacheSnapshot {
    let estimated_message_tokens = messages
        .iter()
        .map(|message| estimate_prompt_cache_message_tokens(model, message))
        .collect::<Vec<_>>();
    let estimated_tool_tokens = context::estimate_tool_tokens_for_model(model, tools);
    let estimated_prompt_tokens = estimated_message_tokens
        .iter()
        .copied()
        .sum::<u32>()
        .saturating_add(estimated_tool_tokens);
    PromptCacheSnapshot {
        provider_type,
        model: model.to_string(),
        cache_profile,
        stable_system_hash: hash_text(&stable_system_text(messages)),
        tool_schema_hash: tool_schema_hash(tools),
        tool_count: tools.len(),
        tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
        message_hashes: messages.iter().map(message_hash).collect(),
        message_fingerprints: messages.iter().map(message_fingerprint).collect(),
        prefix_hashes: PREFIX_HASH_TOKEN_WINDOWS
            .map(|window| prefix_hash_for_token_budget(model, messages, window)),
        system_message_positions: system_message_positions(messages),
        system_message_hashes: system_message_hashes(messages),
        dynamic_system_tokens: dynamic_system_tokens(model, messages),
        tool_result_tokens: tool_result_tokens(model, messages),
        estimated_message_tokens,
        estimated_tool_tokens,
        estimated_prompt_tokens,
        tool_schema_hashes: individual_tool_schema_hashes(tools),
    }
}

fn common_prefix_message_count(previous: &[u64], next: &[u64]) -> usize {
    previous
        .iter()
        .zip(next.iter())
        .take_while(|(previous, next)| previous == next)
        .count()
}

fn tool_name_changes(
    previous: &PromptCacheSnapshot,
    next: &PromptCacheSnapshot,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let previous_names = previous.tool_names.iter().cloned().collect::<BTreeSet<_>>();
    let next_names = next.tool_names.iter().cloned().collect::<BTreeSet<_>>();
    let added = next_names
        .difference(&previous_names)
        .cloned()
        .collect::<Vec<_>>();
    let removed = previous_names
        .difference(&next_names)
        .cloned()
        .collect::<Vec<_>>();

    let hashes_for = |snapshot: &PromptCacheSnapshot| {
        if snapshot.tool_names.len() != snapshot.tool_schema_hashes.len() {
            return BTreeMap::new();
        }
        snapshot
            .tool_names
            .iter()
            .cloned()
            .zip(snapshot.tool_schema_hashes.iter().copied())
            .collect::<BTreeMap<_, _>>()
    };
    let previous_hashes = hashes_for(previous);
    let next_hashes = hashes_for(next);
    let changed = previous_names
        .intersection(&next_names)
        .filter(|name| {
            matches!(
                (previous_hashes.get(*name), next_hashes.get(*name)),
                (Some(previous), Some(next)) if previous != next
            )
        })
        .cloned()
        .collect::<Vec<_>>();

    (added, removed, changed)
}

fn ratio_basis_points(numerator: u32, denominator: u32) -> Option<u32> {
    (denominator > 0).then(|| {
        let ratio =
            u64::from(numerator).saturating_mul(CACHE_RATIO_BASIS_POINTS) / u64::from(denominator);
        u32::try_from(ratio.min(CACHE_RATIO_BASIS_POINTS)).unwrap_or(10_000)
    })
}

fn cold_start_diff(next: &PromptCacheSnapshot) -> PromptCacheDiff {
    PromptCacheDiff {
        sample_kind: "coldStart".to_string(),
        prefix_changed: false,
        changes: Vec::new(),
        first_changed_message_index: None,
        common_prefix_message_count: 0,
        estimated_request_tokens: next.estimated_prompt_tokens,
        estimated_reusable_prefix_tokens: None,
        estimated_changed_suffix_tokens: None,
        estimated_reuse_ratio_bps: None,
        tool_names_added: Vec::new(),
        tool_names_removed: Vec::new(),
        tool_schemas_changed: Vec::new(),
    }
}

fn diff_snapshots(previous: &PromptCacheSnapshot, next: &PromptCacheSnapshot) -> PromptCacheDiff {
    let mut changes = Vec::new();
    let provider_changed = previous.provider_type != next.provider_type;
    let model_changed = previous.model != next.model;
    let stable_system_changed = previous.stable_system_hash != next.stable_system_hash;
    let tool_surface_changed = previous.tool_schema_hash != next.tool_schema_hash;
    let common_prefix_message_count =
        common_prefix_message_count(&previous.message_hashes, &next.message_hashes);
    let previous_messages_are_prefix = common_prefix_message_count == previous.message_hashes.len();
    let messages_equal =
        previous_messages_are_prefix && previous.message_hashes.len() == next.message_hashes.len();
    let (tool_names_added, tool_names_removed, tool_schemas_changed) =
        tool_name_changes(previous, next);

    if provider_changed {
        changes.push(format!(
            "provider type changed ({:?} -> {:?})",
            previous.provider_type, next.provider_type
        ));
    }
    if model_changed {
        changes.push(format!(
            "model changed ({} -> {})",
            previous.model, next.model
        ));
    }
    if stable_system_changed {
        changes.push("stable system prompt changed".to_string());
    }
    if tool_surface_changed {
        if previous.tool_count == next.tool_count {
            changes.push("tool schemas changed with same count".to_string());
        } else {
            changes.push(format!(
                "tool count changed ({} -> {})",
                previous.tool_count, next.tool_count
            ));
        }
        if !tool_names_added.is_empty() {
            changes.push(format!("tools added: {}", tool_names_added.join(", ")));
        }
        if !tool_names_removed.is_empty() {
            changes.push(format!("tools removed: {}", tool_names_removed.join(", ")));
        }
        if !tool_schemas_changed.is_empty() {
            changes.push(format!(
                "tool schemas changed: {}",
                tool_schemas_changed.join(", ")
            ));
        }
    }

    // Appending a new assistant/tool/user tail is the healthy cache path and
    // must not be diagnosed as a prefix mutation. Only explain message-level
    // drift when bytes inside the prior request were removed or rewritten.
    let first_changed_message_index = (!previous_messages_are_prefix)
        .then(|| first_changed_message_index(&previous.message_hashes, &next.message_hashes))
        .flatten();
    if let Some(index) = first_changed_message_index {
        changes.push(format!("first changed message index {index}"));
        match (
            previous.message_fingerprints.get(index),
            next.message_fingerprints.get(index),
        ) {
            (Some(previous), Some(next)) => {
                if previous.text_hash != next.text_hash {
                    changes.push(format!("message {index} text hash changed"));
                }
                if previous.tool_calls_hash != next.tool_calls_hash {
                    changes.push(format!("message {index} tool calls hash changed"));
                }
                if previous.reasoning_hash != next.reasoning_hash {
                    changes.push(format!("message {index} reasoning hash changed"));
                }
            }
            (None, Some(_)) => changes.push(format!("message {index} was added")),
            (Some(_), None) => changes.push(format!("message {index} was removed")),
            (None, None) => {}
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
    }

    let static_prefix_compatible =
        !provider_changed && !model_changed && !stable_system_changed && !tool_surface_changed;
    let estimated_reusable_prefix_tokens = if static_prefix_compatible
        && next.estimated_message_tokens.len() == next.message_hashes.len()
    {
        Some(
            next.estimated_message_tokens
                .iter()
                .take(common_prefix_message_count)
                .copied()
                .sum::<u32>()
                .saturating_add(next.estimated_tool_tokens),
        )
    } else {
        Some(0)
    };
    let estimated_changed_suffix_tokens = estimated_reusable_prefix_tokens
        .map(|reusable| next.estimated_prompt_tokens.saturating_sub(reusable));
    let estimated_reuse_ratio_bps = estimated_reusable_prefix_tokens
        .and_then(|reusable| ratio_basis_points(reusable, next.estimated_prompt_tokens));

    let sample_kind = if provider_changed {
        "providerChanged"
    } else if model_changed {
        "modelChanged"
    } else if stable_system_changed {
        "systemPromptChanged"
    } else if tool_surface_changed {
        "toolSurfaceChanged"
    } else if !previous_messages_are_prefix {
        "prefixRewrite"
    } else if messages_equal {
        "warmReplay"
    } else {
        "warmAppend"
    };
    let prefix_changed = !matches!(sample_kind, "warmAppend" | "warmReplay");

    PromptCacheDiff {
        sample_kind: sample_kind.to_string(),
        prefix_changed,
        changes,
        first_changed_message_index,
        common_prefix_message_count,
        estimated_request_tokens: next.estimated_prompt_tokens,
        estimated_reusable_prefix_tokens,
        estimated_changed_suffix_tokens,
        estimated_reuse_ratio_bps,
        tool_names_added,
        tool_names_removed,
        tool_schemas_changed,
    }
}

impl PromptCacheTracker {
    #[cfg(test)]
    fn begin(
        &mut self,
        request_kind: &str,
        provider_type: Option<ProviderType>,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) {
        let provider = provider_type.unwrap_or(ProviderType::Custom);
        let api_style = if provider == ProviderType::Anthropic {
            crate::llm::prompt_cache::PromptCacheApiStyle::AnthropicMessages
        } else {
            crate::llm::prompt_cache::PromptCacheApiStyle::OpenAiCompatible
        };
        let profile = crate::llm::prompt_cache::resolve_prompt_cache_profile(
            provider, None, api_style, model,
        );
        self.begin_with_profile(request_kind, provider_type, profile, model, messages, tools);
    }

    fn begin_with_profile(
        &mut self,
        request_kind: &str,
        provider_type: Option<ProviderType>,
        cache_profile: PromptCacheProfile,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) {
        let now = Instant::now();
        let model_step_interval_ms = self.previous_begin_at.map(|previous| {
            u64::try_from(now.duration_since(previous).as_millis()).unwrap_or(u64::MAX)
        });
        self.previous_begin_at = Some(now);
        let next = snapshot_for_profile(provider_type, cache_profile, model, messages, tools);
        let diff = self
            .previous_snapshot
            .as_ref()
            .map(|previous| diff_snapshots(previous, &next))
            .unwrap_or_else(|| cold_start_diff(&next));
        let previous_snapshot_source = self.previous_snapshot_source.clone();
        let fast_cache_settle_risk = matches!(provider_type, Some(ProviderType::DeepSeek))
            && model_step_interval_ms
                .is_some_and(|elapsed| elapsed < DEEPSEEK_CACHE_SETTLE_RISK_MS);
        self.pending_observation = Some(PendingPromptCacheObservation {
            request_kind: request_kind.to_string(),
            snapshot: next.clone(),
            previous_snapshot_source,
            diff,
            model_step_interval_ms,
            fast_cache_settle_risk,
        });
        self.previous_snapshot = Some(next);
        self.previous_snapshot_source = Some(PromptCacheSnapshotSource {
            kind: "currentTurnPreviousStep".to_string(),
            turn_id: None,
        });
    }

    fn seed_previous_turn_snapshot(
        &mut self,
        turn_id: String,
        snapshot: PromptCacheSnapshot,
        cache_read_tokens: Option<u32>,
    ) {
        self.previous_snapshot = Some(snapshot);
        self.previous_snapshot_source = Some(PromptCacheSnapshotSource {
            kind: "previousConversationTurn".to_string(),
            turn_id: Some(turn_id),
        });
        self.previous_cache_read_tokens = cache_read_tokens;
        self.pending_observation = None;
    }

    fn complete(
        &mut self,
        usage: Option<&Usage>,
        was_compacted: Option<bool>,
    ) -> Option<PromptCacheTraceObservation> {
        let provider_type = self
            .previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.provider_type);
        let model = self
            .previous_snapshot
            .as_ref()
            .map(|snapshot| snapshot.model.as_str())
            .unwrap_or_default();

        let cache_read_tokens = usage.and_then(|usage| usage.cache_read_tokens);
        let cache_miss_tokens = usage
            .and_then(|usage| usage_accounting::normalized_cache_miss_tokens(provider_type, usage));
        let cache_creation_tokens = usage.and_then(|usage| usage.cache_creation_tokens);
        let usage_coverage = match usage {
            Some(usage) if usage.provider_raw.is_some() => "providerRawAndNormalized",
            Some(_) => "normalizedOnly",
            None => "notReported",
        }
        .to_string();
        let actual_cache_hit_rate_bps = cache_read_tokens.and_then(|read| {
            let denominator = cache_miss_tokens
                .map(|miss| read.saturating_add(miss))
                .or_else(|| usage.map(|usage| usage.prompt_tokens))
                .unwrap_or(0);
            ratio_basis_points(read, denominator)
        });
        if let Some(cache_read_tokens) = cache_read_tokens {
            let previous = self.previous_cache_read_tokens;
            self.previous_cache_read_tokens = Some(cache_read_tokens);
            if let Some(previous_cache_read_tokens) = previous {
                let token_drop = previous_cache_read_tokens.saturating_sub(cache_read_tokens);
                let ratio = if previous_cache_read_tokens == 0 {
                    1.0
                } else {
                    cache_read_tokens as f32 / previous_cache_read_tokens as f32
                };
                if token_drop >= MIN_CACHE_BREAK_TOKEN_DROP && ratio < MAX_STABLE_CACHE_READ_RATIO {
                    let changes = self
                        .pending_observation
                        .as_ref()
                        .map(|pending| pending.diff.changes.as_slice())
                        .unwrap_or_default();
                    let reason = if changes.is_empty() {
                        "prompt unchanged; likely provider-side TTL/routing/eviction".to_string()
                    } else {
                        changes.join(", ")
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
            } else {
                debug!(
                    ?provider_type,
                    model,
                    cache_read_tokens,
                    cache_creation_tokens,
                    "prompt cache baseline recorded"
                );
            }
        }

        self.pending_observation.take().map(|pending| {
            let diff = pending.diff;
            let profile = &pending.snapshot.cache_profile;
            let eligible = profile
                .min_cacheable_tokens
                .is_none_or(|minimum| pending.snapshot.estimated_prompt_tokens >= minimum);
            let cache_outcome_reason = if profile.mode == PromptCacheMode::None {
                "unsupported_profile"
            } else if !eligible {
                "ineligible_below_minimum"
            } else if cache_read_tokens.is_some_and(|tokens| tokens > 0) {
                "hit_reported"
            } else if cache_creation_tokens.is_some_and(|tokens| tokens > 0) {
                "cold_create_reported"
            } else if cache_miss_tokens.is_some_and(|tokens| tokens > 0) {
                "miss_reported"
            } else if diff.tool_names_added.len()
                + diff.tool_names_removed.len()
                + diff.tool_schemas_changed.len()
                > 0
            {
                "tool_surface_changed"
            } else if diff.prefix_changed {
                "prefix_changed"
            } else if usage.is_none() {
                "usage_not_reported"
            } else {
                "usage_schema_unknown"
            }
            .to_string();
            PromptCacheTraceObservation {
                version: 3,
                request_kind: pending.request_kind,
                snapshot: pending.snapshot,
                previous_snapshot_source: pending.previous_snapshot_source,
                sample_kind: diff.sample_kind,
                prefix_changed: diff.prefix_changed,
                changes: diff.changes,
                first_changed_message_index: diff.first_changed_message_index,
                common_prefix_message_count: diff.common_prefix_message_count,
                estimated_request_tokens: diff.estimated_request_tokens,
                estimated_reusable_prefix_tokens: diff.estimated_reusable_prefix_tokens,
                estimated_changed_suffix_tokens: diff.estimated_changed_suffix_tokens,
                estimated_reuse_ratio_bps: diff.estimated_reuse_ratio_bps,
                tool_names_added: diff.tool_names_added,
                tool_names_removed: diff.tool_names_removed,
                tool_schemas_changed: diff.tool_schemas_changed,
                cache_read_tokens,
                cache_miss_tokens,
                cache_creation_tokens,
                actual_cache_hit_rate_bps,
                was_compacted,
                model_step_interval_ms: pending.model_step_interval_ms,
                fast_cache_settle_risk: pending.fast_cache_settle_risk,
                usage_coverage,
                cache_outcome_reason,
            }
        })
    }
}

impl AgentExecutor {
    pub(super) fn seed_prompt_cache_from_previous_turn(
        &self,
        db: &Database,
        conversation_id: Option<&str>,
        current_turn_id: Option<&str>,
    ) {
        let Some(conversation_id) = conversation_id else {
            return;
        };
        let Ok(turns) = db.get_conversation_turns(conversation_id) else {
            return;
        };
        let Some((previous_turn_id, seed)) = turns
            .iter()
            .rev()
            .filter(|turn| Some(turn.id.as_str()) != current_turn_id)
            .filter(|turn| turn.finished_at.is_some())
            .find_map(|turn| {
                turn.trace
                    .as_ref()
                    .and_then(prompt_cache_seed_from_turn_trace)
                    .map(|seed| (turn.id.clone(), seed))
            })
        else {
            return;
        };
        if let Ok(mut tracker) = self.prompt_cache_tracker.lock() {
            tracker.seed_previous_turn_snapshot(
                previous_turn_id,
                seed.snapshot,
                seed.cache_read_tokens,
            );
        }
    }

    pub(super) fn begin_prompt_cache_observation(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) {
        if let Ok(mut tracker) = self.prompt_cache_tracker.lock() {
            let cache_profile = self.provider.prompt_cache_profile(model);
            tracker.begin_with_profile(
                self.config.request_kind.as_str(),
                self.config.provider_type,
                cache_profile,
                model,
                messages,
                tools,
            );
        }
    }

    pub(super) fn complete_prompt_cache_observation(
        &self,
        usage: Option<&Usage>,
        was_compacted: Option<bool>,
    ) -> Option<PromptCacheTraceObservation> {
        if let Ok(mut tracker) = self.prompt_cache_tracker.lock() {
            return tracker.complete(usage, was_compacted);
        }
        None
    }
}

pub(super) fn prompt_cache_observation_to_value(
    observation: &PromptCacheTraceObservation,
    was_compacted: bool,
) -> Option<serde_json::Value> {
    let mut observation = observation.clone();
    observation.was_compacted = Some(was_compacted);
    serde_json::to_value(observation).ok()
}

struct PromptCacheSeed {
    snapshot: PromptCacheSnapshot,
    cache_read_tokens: Option<u32>,
}

fn prompt_cache_seed_from_turn_trace(trace: &serde_json::Value) -> Option<PromptCacheSeed> {
    let items = trace.get("items")?.as_array()?;
    items
        .iter()
        .rev()
        .filter(|item| item.get("kind").and_then(serde_json::Value::as_str) == Some("promptCache"))
        .filter_map(|item| item.get("observation"))
        .find_map(|observation| {
            let snapshot = observation
                .get("snapshot")
                .and_then(|snapshot| serde_json::from_value(snapshot.clone()).ok())?;
            let cache_read_tokens = observation
                .get("cacheReadTokens")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());
            Some(PromptCacheSeed {
                snapshot,
                cache_read_tokens,
            })
        })
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

        assert_eq!(diff.sample_kind, "prefixRewrite");
        assert!(diff.prefix_changed);
        assert_eq!(diff.first_changed_message_index, Some(1));
        assert!(diff
            .changes
            .iter()
            .any(|change| change == "first changed message index 1"));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.starts_with("serialized prefix hash changed")));
        assert!(diff.estimated_reusable_prefix_tokens.unwrap_or(0) > 0);
        assert!(diff.estimated_changed_suffix_tokens.unwrap_or(0) > 0);
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
        let diff = diff_snapshots(&previous, &next);

        assert_eq!(diff.sample_kind, "toolSurfaceChanged");
        assert!(diff.prefix_changed);
        assert_eq!(diff.tool_schemas_changed, vec!["search"]);
        assert_eq!(diff.estimated_reusable_prefix_tokens, Some(0));
        assert!(diff
            .changes
            .iter()
            .any(|change| change == "tool schemas changed with same count"));
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

        let diff = diff_snapshots(&previous, &next);

        assert_eq!(diff.sample_kind, "providerChanged");
        assert_eq!(diff.estimated_reusable_prefix_tokens, Some(0));
        assert_eq!(
            diff.changes,
            vec!["provider type changed (Some(OpenAi) -> Some(DeepSeek))"]
        );
    }

    #[test]
    fn snapshot_diff_reports_reasoning_hash_changes() {
        let mut previous_message = msg(Role::Assistant, "same answer");
        previous_message.reasoning_content = Some("first reasoning".to_string());
        let mut next_message = msg(Role::Assistant, "same answer");
        next_message.reasoning_content = Some("second reasoning".to_string());
        let previous = snapshot_for(None, "m", &[previous_message], &[]);
        let next = snapshot_for(None, "m", &[next_message], &[]);

        let diff = diff_snapshots(&previous, &next);

        assert!(diff
            .changes
            .iter()
            .any(|change| change == "message 0 reasoning hash changed"));
    }

    #[test]
    fn snapshot_hash_detects_image_content_changes() {
        let message_with_image = |data: &str| Message {
            role: Role::User,
            parts: vec![
                ContentPart::Text {
                    text: "inspect this image".to_string(),
                },
                ContentPart::Image {
                    media_type: "image/png".to_string(),
                    data: data.to_string(),
                },
            ],
            name: None,
            tool_calls: None,
            reasoning_content: None,
        };
        let previous = snapshot_for(None, "m", &[message_with_image("first-image")], &[]);
        let next = snapshot_for(None, "m", &[message_with_image("second-image")], &[]);

        let diff = diff_snapshots(&previous, &next);

        assert_eq!(diff.sample_kind, "prefixRewrite");
        assert_eq!(diff.first_changed_message_index, Some(0));
    }

    #[test]
    fn snapshot_token_estimate_includes_replayed_reasoning_content() {
        let plain = msg(Role::Assistant, "answer");
        let mut with_reasoning = plain.clone();
        with_reasoning.reasoning_content = Some("reasoning ".repeat(2_000));

        let plain_snapshot = snapshot_for(
            Some(ProviderType::DeepSeek),
            "deepseek-reasoner",
            &[plain],
            &[],
        );
        let reasoning_snapshot = snapshot_for(
            Some(ProviderType::DeepSeek),
            "deepseek-reasoner",
            &[with_reasoning],
            &[],
        );

        assert!(
            reasoning_snapshot.estimated_prompt_tokens > plain_snapshot.estimated_prompt_tokens
        );
    }

    #[test]
    fn persisted_seed_restores_snapshot_and_cache_read_baseline() {
        let snapshot = snapshot_for(
            Some(ProviderType::Anthropic),
            "claude-sonnet-4-6",
            &[msg(Role::System, "stable"), msg(Role::User, "first")],
            &[],
        );
        let trace = serde_json::json!({
            "items": [{
                "kind": "promptCache",
                "observation": {
                    "snapshot": snapshot,
                    "cacheReadTokens": 8192
                }
            }]
        });

        let seed = prompt_cache_seed_from_turn_trace(&trace).expect("persisted cache seed");
        assert_eq!(seed.cache_read_tokens, Some(8192));

        let mut tracker = PromptCacheTracker::default();
        tracker.seed_previous_turn_snapshot(
            "previous-turn".to_string(),
            seed.snapshot,
            seed.cache_read_tokens,
        );
        assert_eq!(tracker.previous_cache_read_tokens, Some(8192));
        assert_eq!(
            tracker
                .previous_snapshot_source
                .as_ref()
                .and_then(|source| source.turn_id.as_deref()),
            Some("previous-turn")
        );
    }

    #[test]
    fn snapshot_diff_classifies_append_only_growth_as_warm_without_false_changes() {
        let previous_messages = vec![
            msg(Role::System, "stable"),
            msg(Role::User, "first request"),
        ];
        let next_messages = vec![
            msg(Role::System, "stable"),
            msg(Role::User, "first request"),
            msg(Role::Assistant, "first answer"),
            msg(Role::User, "next request"),
        ];
        let previous = snapshot_for(
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &previous_messages,
            &[],
        );
        let next = snapshot_for(
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &next_messages,
            &[],
        );

        let diff = diff_snapshots(&previous, &next);

        assert_eq!(diff.sample_kind, "warmAppend");
        assert!(!diff.prefix_changed);
        assert!(diff.changes.is_empty());
        assert_eq!(diff.first_changed_message_index, None);
        assert_eq!(diff.common_prefix_message_count, previous_messages.len());
        assert!(diff.estimated_reusable_prefix_tokens.unwrap_or(0) > 0);
        assert!(diff.estimated_changed_suffix_tokens.unwrap_or(0) > 0);
        assert!(diff
            .estimated_reuse_ratio_bps
            .is_some_and(|ratio| ratio < 10_000));
    }

    #[test]
    fn tracker_separates_cold_start_from_warm_append_and_normalizes_miss_tokens() {
        let mut tracker = PromptCacheTracker::default();
        tracker.begin(
            "mainAgentStep",
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &[msg(Role::System, "stable"), msg(Role::User, "first")],
            &[],
        );
        let cold = tracker
            .complete(
                Some(&Usage {
                    prompt_tokens: 100,
                    cache_read_tokens: Some(0),
                    ..Usage::default()
                }),
                Some(false),
            )
            .expect("cold observation");
        assert_eq!(cold.sample_kind, "coldStart");
        assert_eq!(cold.cache_miss_tokens, Some(100));
        assert_eq!(cold.actual_cache_hit_rate_bps, Some(0));
        assert_eq!(cold.snapshot.cache_profile.id, "deepseek-exact-prefix-v1");
        assert_eq!(cold.cache_outcome_reason, "miss_reported");
        assert_eq!(cold.usage_coverage, "normalizedOnly");

        tracker.begin(
            "mainAgentStep",
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &[
                msg(Role::System, "stable"),
                msg(Role::User, "first"),
                msg(Role::Assistant, "answer"),
            ],
            &[],
        );
        let warm = tracker
            .complete(
                Some(&Usage {
                    prompt_tokens: 125,
                    cache_read_tokens: Some(100),
                    ..Usage::default()
                }),
                Some(false),
            )
            .expect("warm observation");
        assert_eq!(warm.sample_kind, "warmAppend");
        assert!(!warm.prefix_changed);
        assert_eq!(warm.cache_miss_tokens, Some(25));
        assert_eq!(warm.actual_cache_hit_rate_bps, Some(8_000));
        assert_eq!(warm.cache_outcome_reason, "hit_reported");
    }

    #[test]
    fn tracker_normalizes_anthropic_disjoint_cache_counters() {
        let mut tracker = PromptCacheTracker::default();
        tracker.begin(
            "mainAgentStep",
            Some(ProviderType::Anthropic),
            "claude-sonnet-4-6",
            &[msg(Role::System, "stable"), msg(Role::User, "first")],
            &[],
        );

        let observation = tracker
            .complete(
                Some(&Usage {
                    prompt_tokens: 100,
                    cache_read_tokens: Some(900),
                    cache_creation_tokens: Some(50),
                    ..Usage::default()
                }),
                Some(false),
            )
            .expect("anthropic observation");

        assert_eq!(observation.cache_miss_tokens, Some(150));
        assert_eq!(observation.actual_cache_hit_rate_bps, Some(8_571));
    }

    #[test]
    fn tracker_records_deepseek_fast_cache_settle_risk_bucket() {
        let mut tracker = PromptCacheTracker::default();
        tracker.begin(
            "mainAgentStep",
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &[msg(Role::System, "stable"), msg(Role::User, "first")],
            &[],
        );
        let first = tracker
            .complete(None, Some(false))
            .expect("first observation");
        assert_eq!(first.model_step_interval_ms, None);
        assert!(!first.fast_cache_settle_risk);

        tracker.begin(
            "mainAgentStep",
            Some(ProviderType::DeepSeek),
            "deepseek-chat",
            &[
                msg(Role::System, "stable"),
                msg(Role::User, "first"),
                msg(Role::Assistant, "answer"),
            ],
            &[],
        );
        let second = tracker
            .complete(None, Some(false))
            .expect("second observation");
        assert!(second
            .model_step_interval_ms
            .is_some_and(|elapsed| elapsed < DEEPSEEK_CACHE_SETTLE_RISK_MS));
        assert!(second.fast_cache_settle_risk);
    }

    #[test]
    fn tracker_records_request_kind() {
        let mut tracker = PromptCacheTracker::default();
        tracker.begin("subagentWorker", None, "m", &[msg(Role::User, "work")], &[]);

        let observation = tracker
            .complete(None, Some(false))
            .expect("prompt cache observation");

        assert_eq!(observation.request_kind, "subagentWorker");
    }
}
