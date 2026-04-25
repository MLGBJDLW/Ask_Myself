//! Context management — prepare and trim messages for LLM requests.

use chrono::Utc;

use crate::conversation::memory::{
    context_safety_buffer, estimate_tokens_for_model, model_context_window, trim_to_context_window,
};
use crate::llm::{ContentPart, Message, Role, ToolDefinition};
use crate::skills::Skill;

/// Approximate character limit for the system prompt (~4 000 tokens).
const MAX_SYSTEM_PROMPT_CHARS: usize = 16_000;

/// Build a complete message list for an LLM request, trimmed to fit the
/// model's context window.
///
/// 1. Prepend the system prompt.
/// 2. Append conversation history.
/// 3. Append the new user message.
/// 4. Trim from the oldest non-system message to stay within the context window
///    minus `max_tokens_response` (reserved for the model's answer).
///
/// If `context_window_override` is `Some`, it takes priority over auto-detection.
#[allow(clippy::too_many_arguments)]
pub fn prepare_messages(
    system_prompt: &str,
    history: &[Message],
    user_parts: &[ContentPart],
    model: &str,
    max_tokens_response: u32,
    context_window_override: Option<u32>,
    skills: &[Skill],
    tool_definitions: &[ToolDefinition],
) -> Vec<Message> {
    let mut messages = Vec::with_capacity(history.len() + 2);

    let user_query = user_parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    // System message — always first, with current date/time and skills appended.
    let skills_section = crate::skills::build_skills_section_for_query(skills, &user_query);
    let mut full_prompt = format!(
        "{}\n\nCurrent date and time: {} (UTC)",
        system_prompt,
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    );
    // Inject skills before capping.
    if !skills_section.is_empty() {
        // Reserve space for the base prompt; truncate skills if they would exceed the cap.
        let remaining = MAX_SYSTEM_PROMPT_CHARS.saturating_sub(full_prompt.len());
        if remaining > 0 {
            let truncated_skills = if skills_section.len() > remaining {
                format!(
                    "{}\n...[skills truncated]",
                    &skills_section[..remaining.saturating_sub(25)]
                )
            } else {
                skills_section
            };
            full_prompt.push_str(&truncated_skills);
        }
    }
    let system_with_datetime = cap_system_prompt(full_prompt);
    messages.push(Message::text(Role::System, system_with_datetime));

    // Prior conversation turns.
    messages.extend_from_slice(history);

    // New user input (may include image parts for multimodal messages).
    messages.push(Message {
        role: Role::User,
        parts: user_parts.to_vec(),
        name: None,
        tool_calls: None,
        reasoning_content: None,
    });

    // Trim to fit context window, accounting for tool definition overhead.
    let max_context = context_window_override.unwrap_or_else(|| model_context_window(model));
    let tool_overhead = estimate_tool_tokens_for_model(model, tool_definitions);
    let effective_context = max_context
        .saturating_sub(tool_overhead)
        .saturating_sub(context_safety_buffer(max_context));
    let mut trimmed = trim_to_context_window(&messages, effective_context, max_tokens_response);

    // If messages were evicted, inject an extractive recap into the system prompt
    // so the LLM retains awareness of earlier conversation topics.
    let original_non_system = messages.iter().filter(|m| m.role != Role::System).count();
    let kept_non_system = trimmed.iter().filter(|m| m.role != Role::System).count();
    let evicted_count = original_non_system.saturating_sub(kept_non_system);

    if evicted_count > 0 {
        let evicted: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .take(evicted_count)
            .collect();

        let recap = build_evicted_recap(&evicted);
        if !recap.is_empty() {
            if let Some(sys) = trimmed.iter_mut().find(|m| m.role == Role::System) {
                if let Some(ContentPart::Text { text }) = sys.parts.first_mut() {
                    *text = cap_system_prompt(format!("{}\n\n{}", text, recap));
                }
            }
        }
    }

    trimmed
}

/// Build an extractive recap of evicted conversation messages.
///
/// Only includes `User` and `Assistant` messages (skips tool-call
/// intermediaries). The output is capped at ~800 characters (~200 tokens)
/// to avoid eating too much context budget.
fn build_evicted_recap(evicted: &[&Message]) -> String {
    const MAX_RECAP_CHARS: usize = 800;
    let mut parts: Vec<String> = Vec::new();
    let mut total_chars: usize = 0;

    for msg in evicted {
        if total_chars >= MAX_RECAP_CHARS {
            break;
        }

        match msg.role {
            Role::User => {
                let text = msg.text_content();
                let summary = if text.trim().is_empty() {
                    if msg.has_images() {
                        "[image]".to_string()
                    } else {
                        continue;
                    }
                } else {
                    let label = if msg.has_images() { "[image] " } else { "" };
                    format!("{}{}", label, truncate_text(&text, 100))
                };
                let line = format!("- User asked: {}", summary);
                total_chars += line.len();
                parts.push(line);
            }
            Role::Assistant => {
                // Skip tool-call intermediary messages.
                if msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
                    continue;
                }
                let text = msg.text_content();
                let summary = if text.trim().is_empty() {
                    continue;
                } else {
                    truncate_text(&text, 80)
                };
                let line = format!("- You answered: {}", summary);
                total_chars += line.len();
                parts.push(line);
            }
            _ => {} // Skip Tool and System messages.
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    format!(
        "## Earlier conversation context (summarized)\n\
         These topics were discussed earlier but trimmed for context space:\n{}",
        parts.join("\n")
    )
}

/// Public entry-point for building an extractive recap from owned messages.
///
/// This is used by `AgentExecutor::summarize_if_needed` as the extractive
/// fallback string that gets passed to the LLM summariser.
pub fn build_evicted_recap_from_messages(evicted: &[Message]) -> String {
    let refs: Vec<&Message> = evicted.iter().collect();
    build_evicted_recap(&refs)
}

/// Estimate tokens occupied by tool definitions in the LLM request.
pub fn estimate_tool_tokens(tools: &[ToolDefinition]) -> u32 {
    estimate_tool_tokens_for_model("gpt-4o", tools)
}

pub fn estimate_tool_tokens_for_model(model: &str, tools: &[ToolDefinition]) -> u32 {
    let mut total = 0u32;
    for tool in tools {
        let tool_text = format!("{} {} {}", tool.name, tool.description, tool.parameters);
        total += estimate_tokens_for_model(model, &tool_text);
        total += 10; // overhead per tool (formatting, type annotations)
    }
    total
}

/// Enforce `MAX_SYSTEM_PROMPT_CHARS` on the system prompt.
///
/// If the prompt exceeds the limit it is truncated on a word boundary and
/// a `...[truncated]` marker is appended so the LLM can see signalling.
fn cap_system_prompt(text: String) -> String {
    if text.len() <= MAX_SYSTEM_PROMPT_CHARS {
        return text;
    }
    let truncated = &text[..MAX_SYSTEM_PROMPT_CHARS];
    let cut = truncated
        .rfind('\n')
        .or_else(|| truncated.rfind(' '))
        .unwrap_or(MAX_SYSTEM_PROMPT_CHARS);
    format!("{}\n...[truncated]", &text[..cut])
}

/// Truncate text to `max_chars` on a word boundary, appending "..." if truncated.
fn truncate_text(text: &str, max_chars: usize) -> String {
    let clean = text.replace('\n', " ");
    let clean = clean.trim();
    if clean.len() <= max_chars {
        return clean.to_string();
    }
    let truncated = &clean[..max_chars];
    let cut = truncated.rfind(' ').unwrap_or(max_chars);
    format!("{}...", &clean[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> Message {
        Message::text(role, content)
    }

    #[test]
    fn test_prepare_messages_basic() {
        let history = vec![msg(Role::User, "Hi"), msg(Role::Assistant, "Hello!")];
        let result = prepare_messages(
            "System prompt",
            &history,
            &[ContentPart::Text {
                text: "What's up?".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
        );

        // System is first, with datetime appended.
        assert_eq!(result[0].role, Role::System);
        assert!(result[0]
            .text_content()
            .starts_with("System prompt\n\nCurrent date and time:"));

        // Last message is the new user input.
        assert_eq!(result.last().unwrap().text_content(), "What's up?");
        assert_eq!(result.last().unwrap().role, Role::User);
    }

    #[test]
    fn test_prepare_messages_trims_when_needed() {
        // Build a history that exceeds a small context window.
        // Alternate User/Assistant so the recap has both sides.
        // Use varied words instead of repeated characters so tokenizer-backed
        // counting cannot compress the fixture below the trim threshold.
        let history: Vec<Message> = (0..200)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                let padding = (0..80)
                    .map(|n| format!("token{i}_{n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                msg(role, &format!("Message number {i} {padding}"))
            })
            .collect();
        // Force a small context window (8192) so trimming is guaranteed.
        let result = prepare_messages(
            "Sys",
            &history,
            &[ContentPart::Text {
                text: "New".to_string(),
            }],
            "some-model",
            512,
            Some(8192),
            &[],
            &[],
        );

        // System message must survive.
        assert_eq!(result[0].role, Role::System);
        // Total input is 202 messages. With 7680 token budget and ~59 tok/msg, only ~130 fit.
        assert!(
            result.len() < 202,
            "expected trimming, got {} messages",
            result.len()
        );
        assert!(result.len() > 2, "expected more than just sys+user");
        // Last message is the new user input.
        assert_eq!(result.last().unwrap().text_content(), "New");

        // System message should contain the evicted recap.
        assert!(
            result[0]
                .text_content()
                .contains("Earlier conversation context"),
            "System message should contain evicted recap"
        );
    }

    #[test]
    fn test_no_recap_when_nothing_evicted() {
        let history = vec![msg(Role::User, "Hi"), msg(Role::Assistant, "Hello!")];
        let result = prepare_messages(
            "Sys",
            &history,
            &[ContentPart::Text {
                text: "What's up?".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
        );

        // No trimming happened, so no recap.
        assert!(!result[0]
            .text_content()
            .contains("Earlier conversation context"));
    }

    #[test]
    fn test_prepare_messages_empty_history() {
        let result = prepare_messages(
            "Sys",
            &[],
            &[ContentPart::Text {
                text: "Hello".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[1].role, Role::User);
        assert_eq!(result[1].text_content(), "Hello");
    }

    #[test]
    fn test_prepare_messages_with_skills() {
        let skills = vec![Skill {
            id: "1".into(),
            name: "Be Concise".into(),
            description: "Always favor brevity".into(),
            content: "Always answer briefly.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }];
        let result = prepare_messages(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Hi".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &skills,
            &[],
        );
        let sys_text = result[0].text_content();
        assert!(
            sys_text.contains("Active Skills"),
            "Skills should be in system prompt"
        );
        assert!(sys_text.contains("Be Concise"));
    }

    #[test]
    fn test_estimate_tool_tokens() {
        let tools = vec![ToolDefinition {
            name: "search".into(),
            description: "Search the knowledge base".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let tokens = estimate_tool_tokens(&tools);
        assert!(tokens > 10, "Tool tokens should be non-trivial");
    }
}
