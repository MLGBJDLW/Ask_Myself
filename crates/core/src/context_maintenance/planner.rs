use crate::agent::context::build_evicted_recap_from_messages;
use crate::conversation::memory::{estimate_tokens_for_model, model_context_window};
use crate::conversation::{conversation_message_llm_context_content, ConversationMessage};
use crate::llm::{Message, Role};

const TARGET_USAGE: f32 = 0.55;
const TOOL_RESULT_CAP: usize = 1_500;
const MESSAGE_CONTENT_CAP: usize = 6_000;
const TOOL_CALL_CAP: usize = 2_000;

#[derive(Debug)]
pub(crate) enum PlanOutcome {
    Noop {
        messages_before: usize,
        tokens_before: u32,
    },
    Planned(CompactionPlan),
}

#[derive(Debug)]
pub(crate) struct CompactionPlan {
    pub snapshot_high_watermark: i64,
    pub source_message_ids: Vec<String>,
    pub source_start_sort_order: i64,
    pub source_boundary_sort_order: i64,
    pub source_digest: String,
    pub expected_checkpoint_generation: u64,
    pub summary_messages: Vec<Message>,
    pub extractive_fallback: String,
    pub retained_tail_message_ids: Vec<String>,
    pub retained_start_sort_order: i64,
    pub messages_before: usize,
    pub evicted_messages: usize,
    pub tokens_before: u32,
    pub retained_tokens: u32,
}

pub(crate) fn plan_compaction(
    messages: Vec<ConversationMessage>,
    model: &str,
    configured_context_window: Option<u32>,
    max_response_tokens: u32,
    expected_checkpoint_generation: u64,
) -> PlanOutcome {
    let messages_before = messages.len();
    let tokens = messages
        .iter()
        .map(|message| estimated_message_tokens(message, model))
        .collect::<Vec<_>>();
    let tokens_before = tokens.iter().copied().fold(0_u32, u32::saturating_add);
    if messages.is_empty() {
        return PlanOutcome::Noop {
            messages_before,
            tokens_before,
        };
    }

    let context_window = configured_context_window.unwrap_or_else(|| model_context_window(model));
    let budget = context_window.saturating_sub(max_response_tokens);
    if budget == 0 {
        return PlanOutcome::Noop {
            messages_before,
            tokens_before,
        };
    }

    let prefix_end = messages
        .iter()
        .position(|message| {
            message.role != Role::System || is_existing_compaction_summary(&message.content)
        })
        .unwrap_or(messages.len());
    let user_starts = messages
        .iter()
        .enumerate()
        .skip(prefix_end)
        .filter_map(|(index, message)| (message.role == Role::User).then_some(index))
        .collect::<Vec<_>>();
    if user_starts.len() <= 1 {
        return PlanOutcome::Noop {
            messages_before,
            tokens_before,
        };
    }

    let mut suffix_tokens = vec![0_u32; messages.len() + 1];
    for index in (prefix_end..messages.len()).rev() {
        suffix_tokens[index] = suffix_tokens[index + 1].saturating_add(tokens[index]);
    }
    let target = (budget as f32 * TARGET_USAGE) as u32;
    let latest_allowed = *user_starts.last().expect("at least two user turns");
    let mut boundary = None;
    for candidate in user_starts.into_iter().skip(1) {
        if candidate > latest_allowed {
            break;
        }
        boundary = Some(candidate);
        if suffix_tokens[candidate] <= target {
            break;
        }
    }
    let Some(evict_end) = boundary else {
        return PlanOutcome::Noop {
            messages_before,
            tokens_before,
        };
    };
    if evict_end <= prefix_end || evict_end >= messages.len() {
        return PlanOutcome::Noop {
            messages_before,
            tokens_before,
        };
    }

    let summary_messages = messages[prefix_end..evict_end]
        .iter()
        .map(summary_message)
        .collect::<Vec<_>>();
    let extractive_fallback = build_evicted_recap_from_messages(&summary_messages);
    let retained_tail_message_ids = messages[evict_end..]
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let retained_start_sort_order = messages[evict_end].sort_order;
    let retained_tokens = tokens[evict_end..]
        .iter()
        .copied()
        .fold(0_u32, u32::saturating_add);

    let source_messages = &messages[prefix_end..evict_end];

    PlanOutcome::Planned(CompactionPlan {
        snapshot_high_watermark: messages
            .iter()
            .map(|message| message.sort_order)
            .max()
            .unwrap_or_default(),
        source_message_ids: source_messages
            .iter()
            .map(|message| message.id.clone())
            .collect(),
        source_start_sort_order: source_messages
            .first()
            .map(|message| message.sort_order)
            .unwrap_or_default(),
        source_boundary_sort_order: source_messages
            .last()
            .map(|message| message.sort_order)
            .unwrap_or_default(),
        source_digest: source_digest(source_messages),
        expected_checkpoint_generation,
        summary_messages,
        extractive_fallback,
        retained_tail_message_ids,
        retained_start_sort_order,
        messages_before,
        evicted_messages: evict_end.saturating_sub(prefix_end),
        tokens_before,
        retained_tokens,
    })
}

fn estimated_message_tokens(message: &ConversationMessage, model: &str) -> u32 {
    let content_tokens = if message.token_count > 0 {
        message.token_count
    } else {
        estimate_tokens_for_model(model, conversation_message_llm_context_content(message))
    };
    let thinking_tokens = message
        .thinking
        .as_deref()
        .map(|thinking| estimate_tokens_for_model(model, thinking))
        .unwrap_or(0);
    let tool_call_tokens = message
        .tool_calls
        .iter()
        .map(|call| estimate_tokens_for_model(model, &call.arguments))
        .fold(0_u32, u32::saturating_add);
    content_tokens
        .saturating_add(thinking_tokens)
        .saturating_add(tool_call_tokens)
}

fn summary_message(message: &ConversationMessage) -> Message {
    let raw = conversation_message_llm_context_content(message);
    let cap = if message.role == Role::Tool {
        TOOL_RESULT_CAP
    } else {
        MESSAGE_CONTENT_CAP
    };
    let mut content = bounded_reference(raw, cap);
    if message.role == Role::Assistant && !message.tool_calls.is_empty() {
        let calls = message
            .tool_calls
            .iter()
            .map(|call| format!("{}({})", call.name, call.arguments))
            .collect::<Vec<_>>()
            .join("\n");
        content.push_str("\nAssistant tool calls: ");
        content.push_str(&bounded_reference(&calls, TOOL_CALL_CAP));
    }
    if message.role == Role::Assistant && content.trim().is_empty() {
        if let Some(thinking) = message.thinking.as_deref() {
            content = format!(
                "Assistant reasoning: {}",
                bounded_reference(thinking, TOOL_CALL_CAP)
            );
        }
    }
    Message::text(message.role.clone(), content)
}

fn bounded_reference(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let digest = blake3::hash(text.as_bytes()).to_hex();
    format!(
        "{}\n[content referenced: {} bytes, blake3:{}]",
        &text[..end],
        text.len(),
        digest
    )
}

pub(crate) fn source_digest(messages: &[ConversationMessage]) -> String {
    let mut hash = blake3::Hasher::new();
    for message in messages {
        let tool_calls_json = serde_json::to_string(&message.tool_calls).unwrap_or_default();
        hash_source_message(
            &mut hash,
            &message.id,
            message.sort_order,
            role_label(&message.role),
            conversation_message_llm_context_content(message),
            message.tool_call_id.as_deref(),
            &tool_calls_json,
        );
    }
    hash.finalize().to_hex().to_string()
}

pub(crate) fn hash_source_message(
    hash: &mut blake3::Hasher,
    id: &str,
    sort_order: i64,
    role: &str,
    canonical_content: &str,
    tool_call_id: Option<&str>,
    tool_calls_json: &str,
) {
    hash_field(hash, id.as_bytes());
    hash_field(hash, &sort_order.to_le_bytes());
    hash_field(hash, role.as_bytes());
    hash_field(hash, canonical_content.as_bytes());
    hash_field(hash, tool_call_id.unwrap_or_default().as_bytes());
    hash_field(hash, tool_calls_json.as_bytes());
}

pub(crate) fn hash_field(hash: &mut blake3::Hasher, value: &[u8]) {
    hash.update(&(value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn role_label(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn is_existing_compaction_summary(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("earlier conversation context") || lower.contains("compacted context")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(index: usize, role: Role, content: String) -> ConversationMessage {
        ConversationMessage {
            id: format!("message-{index}"),
            conversation_id: "conversation-1".to_string(),
            role,
            content,
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 4,
            created_at: String::new(),
            sort_order: index as i64,
            thinking: None,
            image_attachments: None,
        }
    }

    #[test]
    fn plans_fifty_thousand_messages_without_quadratic_scans() {
        let mut messages = Vec::with_capacity(50_000);
        for index in 0..25_000 {
            messages.push(message(index * 2, Role::User, format!("request {index}")));
            messages.push(message(
                index * 2 + 1,
                Role::Assistant,
                "response".to_string(),
            ));
        }
        let started = std::time::Instant::now();
        let outcome = plan_compaction(messages, "gpt-4o", Some(16_000), 4_096, 0);
        assert!(matches!(outcome, PlanOutcome::Planned(_)));
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
    }

    #[test]
    fn large_tool_results_are_referenced_before_summary_input_is_cloned() {
        let messages = vec![
            message(0, Role::User, "first".to_string()),
            message(1, Role::Tool, "x".repeat(2_000_000)),
            message(2, Role::Assistant, "done".to_string()),
            message(3, Role::User, "latest".to_string()),
        ];
        let PlanOutcome::Planned(plan) = plan_compaction(messages, "gpt-4o", Some(8_000), 1_000, 0)
        else {
            panic!("expected compaction plan");
        };
        let tool = plan
            .summary_messages
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool summary entry")
            .text_content();
        assert!(tool.len() < 2_000);
        assert!(tool.contains("content referenced"));
    }
}
