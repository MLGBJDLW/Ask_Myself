//! Context window management for conversation history.

use crate::llm::{Message, Role};

/// Approximate token count using character-based heuristics.
///
/// - ASCII-heavy (English, code, JSON): ~4 chars per token
/// - CJK-heavy: ~1.5 chars per token (most CJK chars = 1 token)
/// - Mixed: weighted average
/// - Adds overhead for message formatting (~4 tokens per message)
pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }

    let mut ascii_chars: u32 = 0;
    let mut cjk_chars: u32 = 0;
    let mut other_chars: u32 = 0;

    for ch in text.chars() {
        if ch.is_ascii() {
            ascii_chars += 1;
        } else if is_cjk(ch) {
            cjk_chars += 1;
        } else {
            other_chars += 1;
        }
    }

    // ASCII text: ~4 chars per token
    // CJK text: ~1.5 chars per token (most CJK characters = 1 token in GPT tokenizers)
    // Other (emoji, accented, etc.): ~2 chars per token
    let ascii_tokens = (ascii_chars as f64 / 4.0).ceil() as u32;
    let cjk_tokens = (cjk_chars as f64 / 1.5).ceil() as u32;
    let other_tokens = (other_chars as f64 / 2.0).ceil() as u32;

    // Add per-message overhead (role label, formatting)
    ascii_tokens + cjk_tokens + other_tokens + 4
}

/// Check if a character is in the CJK Unified Ideographs range.
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
}

/// Estimate the total token cost of a message, including tool_calls and images.
pub fn estimate_message_tokens(msg: &Message) -> u32 {
    let mut tokens = estimate_tokens(&msg.text_content());

    // Estimate tokens for image parts.
    // OpenAI vision: 85 base + 170 per 512x512 tile. Since we don't know
    // resolution, use base64 data length as a rough proxy:
    //   max(258, data_len / 1500)
    // 258 ≈ low-res mode (85 + 170 for one tile + overhead).
    for part in &msg.parts {
        if let crate::llm::ContentPart::Image { data, .. } = part {
            let estimated = (data.len() / 1500) as u32;
            tokens += estimated.max(258);
        }
    }

    if let Some(ref calls) = msg.tool_calls {
        for tc in calls {
            // Each tool call: id + name + arguments JSON
            tokens += estimate_tokens(&tc.id);
            tokens += estimate_tokens(&tc.name);
            tokens += estimate_tokens(&tc.arguments);
            tokens += 4; // overhead per tool call
        }
    }
    tokens
}

/// Known model context windows, mapped by exact API model ID.
/// Falls back to prefix/substring matching for unknown variants.
///
/// IMPORTANT: Only verified model IDs are listed here. For unlisted models,
/// users can set context_window in AgentConfig. Update this list by checking
/// provider API documentation.
pub fn model_context_window(model: &str) -> u32 {
    let m = model.to_lowercase();
    let m = m.as_str();

    // ── Exact matches for verified model IDs (highest priority) ────
    match m {
        // OpenAI GPT-5 series (400K)
        "gpt-5.2" | "gpt-5.2-codex" | "gpt-5.2-pro" => 400_000,
        "gpt-5.1" | "gpt-5.1-codex" => 400_000,
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" => 400_000,
        // OpenAI GPT-4.1 series (1,047,576)
        "gpt-4.1" | "gpt-4.1-2025-04-14" => 1_047_576,
        "gpt-4.1-mini" | "gpt-4.1-mini-2025-04-14" => 1_047_576,
        "gpt-4.1-nano" | "gpt-4.1-nano-2025-04-14" => 1_047_576,
        // OpenAI GPT-4.5 (128K)
        "gpt-4.5-preview" | "gpt-4.5-preview-2025-02-27" => 128_000,
        // OpenAI GPT-4o (128K)
        "gpt-4o" | "gpt-4o-mini" | "gpt-4o-audio-preview" | "chatgpt-4o-latest" => 128_000,
        "gpt-4o-2024-11-20" | "gpt-4o-2024-08-06" | "gpt-4o-2024-05-13" => 128_000,
        "gpt-4o-mini-2024-07-18" => 128_000,
        // OpenAI Legacy
        "gpt-4-turbo" | "gpt-4-turbo-2024-04-09" => 128_000,
        "gpt-4-0613" => 8_192,
        "gpt-3.5-turbo" | "gpt-3.5-turbo-0125" => 16_385,
        // OpenAI o-series reasoning (200K)
        "o4-mini" | "o4-mini-2025-04-16" => 200_000,
        "o3" | "o3-2025-04-16" => 200_000,
        "o3-mini" | "o3-mini-2025-01-31" => 200_000,
        "o3-pro" | "o3-pro-2025-06-10" => 200_000,
        "o1" | "o1-2024-12-17" => 200_000,
        "o1-mini" | "o1-mini-2024-09-12" => 128_000,
        "o1-pro" | "o1-pro-2025-03-19" => 200_000,
        // OpenAI Codex
        "codex-mini-latest" => 200_000,

        // Anthropic Claude 4.x (200K)
        "claude-opus-4-6" => 200_000,
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => 200_000,
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => 200_000,
        "claude-opus-4-20250514" => 200_000,
        "claude-sonnet-4-20250514" => 200_000,
        // Anthropic Claude 3.x
        "claude-3-7-sonnet-20250219" | "claude-3-7-sonnet-latest" => 200_000,
        "claude-3-5-sonnet-20241022" | "claude-3-5-sonnet-latest" => 200_000,
        "claude-3-5-haiku-20241022" | "claude-3-5-haiku-latest" => 200_000,
        "claude-3-opus-20240229" | "claude-3-opus-latest" => 200_000,
        "claude-3-sonnet-20240229" => 200_000,
        "claude-3-haiku-20240307" => 200_000,
        // Anthropic Claude 2.x
        "claude-2.1" => 200_000,
        "claude-2.0" => 100_000,

        // Google Gemini 3.x (preview)
        "gemini-3-pro-preview" => 1_048_576,
        "gemini-3-flash" => 1_048_576,
        // Google Gemini 2.5
        "gemini-2.5-pro" | "gemini-2.5-pro-preview-05-06" => 1_048_576,
        "gemini-2.5-flash" | "gemini-2.5-flash-preview-04-17" => 1_048_576,
        "gemini-2.5-flash-lite" => 1_048_576,
        // Google Gemini 2.0
        "gemini-2.0-flash" | "gemini-2.0-flash-lite" => 1_048_576,
        // Google Gemini 1.5
        "gemini-1.5-pro" | "gemini-1.5-pro-latest" => 2_097_152,
        "gemini-1.5-flash" | "gemini-1.5-flash-latest" => 1_048_576,

        // DeepSeek
        "deepseek-chat" | "deepseek-reasoner" => 128_000,

        // xAI Grok
        "grok-4" | "grok-4-0709" => 256_000,
        "grok-4-1-fast" | "grok-4-1-fast-reasoning" => 2_000_000,
        "grok-3" | "grok-3-latest" | "grok-3-mini" => 131_072,
        "grok-2" | "grok-2-latest" => 131_072,

        // Mistral
        "mistral-large-2512" => 256_000,
        "magistral-medium-2509" | "mistral-small-2506" => 128_000,
        "codestral-2508" | "devstral-2-2512" => 256_000,

        _ => return prefix_model_context_window(m),
    }
}

/// Fallback matching for model variants not in the exact list.
fn prefix_model_context_window(m: &str) -> u32 {
    match m {
        // OpenAI
        _ if m.starts_with("gpt-5") => 400_000,
        _ if m.starts_with("gpt-4.1") => 1_047_576,
        _ if m.starts_with("gpt-4.5") => 128_000,
        _ if m.starts_with("gpt-4o") => 128_000,
        _ if m.starts_with("gpt-4") => 128_000,
        _ if m.starts_with("gpt-3.5") => 16_385,
        _ if m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4") =>
        {
            200_000
        }
        _ if m.starts_with("codex") => 200_000,

        // Anthropic
        _ if m.contains("claude") => 200_000,

        // Google
        _ if m.contains("gemini") => 1_048_576,

        // DeepSeek
        _ if m.contains("deepseek") => 128_000,

        // xAI Grok
        _ if m.starts_with("grok-4") => 256_000,
        _ if m.contains("grok") => 131_072,

        // Mistral
        _ if m.contains("codestral") => 256_000,
        _ if m.contains("devstral") => 256_000,
        _ if m.contains("magistral") => 128_000,
        _ if m.contains("mistral") || m.contains("mixtral") => 128_000,

        // Meta Llama
        _ if m.contains("llama") => 128_000,

        // Qwen
        _ if m.contains("qwen") => 128_000,

        // Others
        _ if m.contains("phi") => 128_000,
        _ if m.contains("command") => 128_000,
        _ if m.contains("yi") => 128_000,
        _ if m.contains("starcoder") => 16_384,

        // Default for completely unknown models
        _ => 32_000,
    }
}

/// A contiguous group of messages that must be kept or dropped together.
struct MessageBlock {
    messages: Vec<Message>,
    token_cost: u32,
}

/// Trim conversation history to fit within context window.
///
/// Keeps the system prompt (first message if role == System) plus the
/// newest messages that fit within `max_tokens - reserved_for_response`.
///
/// Tool-call pairs (an assistant message with `tool_calls` and its
/// subsequent `Tool` result messages) are treated as atomic blocks and
/// will never be split.
pub fn trim_to_context_window(
    messages: &[Message],
    max_tokens: u32,
    reserved_for_response: u32,
) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }

    let budget = max_tokens.saturating_sub(reserved_for_response);
    let mut result: Vec<Message> = Vec::new();
    let mut used: u32 = 0;

    // Separate system messages from the rest.
    let (system_msgs, conversation): (Vec<&Message>, Vec<&Message>) =
        messages.iter().partition(|m| m.role == Role::System);

    // Always include system messages first — they are non-negotiable.
    for msg in &system_msgs {
        let cost = estimate_message_tokens(msg);
        used = used.saturating_add(cost);
        result.push((*msg).clone());
    }

    // If system messages alone exceed budget, return just them.
    if used >= budget {
        return result;
    }

    // Group conversation into atomic blocks
    let blocks = build_message_blocks(&conversation);

    let remaining_budget = budget.saturating_sub(used);
    let mut kept_blocks: Vec<&MessageBlock> = Vec::new();
    let mut tail_tokens: u32 = 0;

    // Walk from newest block to oldest
    for block in blocks.iter().rev() {
        if tail_tokens.saturating_add(block.token_cost) > remaining_budget {
            break;
        }
        tail_tokens = tail_tokens.saturating_add(block.token_cost);
        kept_blocks.push(block);
    }

    // Reverse to restore chronological order
    kept_blocks.reverse();
    for block in kept_blocks {
        result.extend(block.messages.iter().cloned());
    }

    result
}

/// Group messages into atomic blocks.
///
/// An assistant message with `tool_calls` + its following `Tool` messages
/// form one indivisible block. Everything else is its own block.
fn build_message_blocks(conversation: &[&Message]) -> Vec<MessageBlock> {
    let mut blocks: Vec<MessageBlock> = Vec::new();
    let mut i = 0;

    while i < conversation.len() {
        let msg = conversation[i];

        // Check if this is an assistant message with tool calls
        if msg.role == Role::Assistant
            && msg.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty())
        {
            let mut block_msgs = vec![msg.clone()];
            let mut cost = estimate_message_tokens(msg);

            // Collect following tool result messages
            let mut j = i + 1;
            while j < conversation.len() && conversation[j].role == Role::Tool {
                cost += estimate_message_tokens(conversation[j]);
                block_msgs.push(conversation[j].clone());
                j += 1;
            }

            blocks.push(MessageBlock {
                messages: block_msgs,
                token_cost: cost,
            });
            i = j;
        } else {
            // Standalone message
            blocks.push(MessageBlock {
                messages: vec![msg.clone()],
                token_cost: estimate_message_tokens(msg),
            });
            i += 1;
        }
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCallRequest, ContentPart};

    fn msg(role: Role, content: &str) -> Message {
        Message::text(role, content)
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        // 4 ASCII chars → ceil(4/4) + 4 overhead = 5
        assert_eq!(estimate_tokens("abcd"), 5);
        // 5 ASCII chars → ceil(5/4) + 4 overhead = 6
        assert_eq!(estimate_tokens("abcde"), 6);
    }

    #[test]
    fn test_estimate_tokens_cjk() {
        // "你好世界" = 4 CJK chars → ceil(4/1.5) + 4 = 3 + 4 = 7
        let tokens = estimate_tokens("你好世界");
        assert_eq!(tokens, 7);
        assert!(tokens > 4, "CJK should produce more tokens than naive byte/4");
    }

    #[test]
    fn test_estimate_message_tokens_with_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_123".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query": "test"}"#.to_string(),
            }]),
        };
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 10, "Tool calls should contribute to token count");
    }

    #[test]
    fn test_model_context_window_exact_match() {
        // OpenAI GPT-5
        assert_eq!(model_context_window("gpt-5.2"), 400_000);
        assert_eq!(model_context_window("gpt-5"), 400_000);
        // OpenAI GPT-4.1
        assert_eq!(model_context_window("gpt-4.1"), 1_047_576);
        assert_eq!(model_context_window("gpt-4.1-mini"), 1_047_576);
        assert_eq!(model_context_window("gpt-4.1-nano"), 1_047_576);
        // OpenAI GPT-4.5
        assert_eq!(model_context_window("gpt-4.5-preview"), 128_000);
        assert_eq!(model_context_window("gpt-4.5-preview-2025-02-27"), 128_000);
        // OpenAI GPT-4o
        assert_eq!(model_context_window("gpt-4o"), 128_000);
        assert_eq!(model_context_window("chatgpt-4o-latest"), 128_000);
        // OpenAI Legacy
        assert_eq!(model_context_window("gpt-4-turbo"), 128_000);
        assert_eq!(model_context_window("gpt-4-0613"), 8_192);
        assert_eq!(model_context_window("gpt-3.5-turbo"), 16_385);
        // OpenAI o-series
        assert_eq!(model_context_window("o1"), 200_000);
        assert_eq!(model_context_window("o1-mini"), 128_000);
        assert_eq!(model_context_window("o3-mini"), 200_000);
        assert_eq!(model_context_window("o3-pro"), 200_000);
        assert_eq!(model_context_window("o4-mini"), 200_000);
        assert_eq!(model_context_window("codex-mini-latest"), 200_000);
        // Anthropic
        assert_eq!(model_context_window("claude-opus-4-6"), 200_000);
        assert_eq!(model_context_window("claude-sonnet-4-5"), 200_000);
        assert_eq!(model_context_window("claude-3-7-sonnet-20250219"), 200_000);
        assert_eq!(model_context_window("claude-2.1"), 200_000);
        assert_eq!(model_context_window("claude-2.0"), 100_000);
        // Google Gemini
        assert_eq!(model_context_window("gemini-3-pro-preview"), 1_048_576);
        assert_eq!(model_context_window("gemini-2.0-flash"), 1_048_576);
        assert_eq!(model_context_window("gemini-1.5-pro"), 2_097_152);
        assert_eq!(model_context_window("gemini-1.5-flash"), 1_048_576);
        // DeepSeek
        assert_eq!(model_context_window("deepseek-chat"), 128_000);
        assert_eq!(model_context_window("deepseek-reasoner"), 128_000);
        // xAI Grok
        assert_eq!(model_context_window("grok-4"), 256_000);
        assert_eq!(model_context_window("grok-4-1-fast"), 2_000_000);
        assert_eq!(model_context_window("grok-3"), 131_072);
        assert_eq!(model_context_window("grok-2"), 131_072);
        // Mistral
        assert_eq!(model_context_window("mistral-large-2512"), 256_000);
        assert_eq!(model_context_window("codestral-2508"), 256_000);
    }

    #[test]
    fn test_model_context_window_prefix_fallback() {
        // These hit prefix/substring matching, not exact match
        assert_eq!(model_context_window("gpt-5-future"), 400_000);
        assert_eq!(model_context_window("gpt-4o-something"), 128_000);
        assert_eq!(model_context_window("claude-3-opus"), 200_000);
        assert_eq!(model_context_window("gemini-2.5-future"), 1_048_576);
        assert_eq!(model_context_window("deepseek-something"), 128_000);
        assert_eq!(model_context_window("grok-4-future"), 256_000);
        assert_eq!(model_context_window("grok-3-beta"), 131_072);
        assert_eq!(model_context_window("llama-3-70b"), 128_000);
        assert_eq!(model_context_window("codex-future"), 200_000);
    }

    #[test]
    fn test_model_context_window_case_insensitive() {
        assert_eq!(model_context_window("GPT-5.2"), 400_000);
        assert_eq!(model_context_window("Claude-Opus-4-6"), 200_000);
        assert_eq!(model_context_window("GEMINI-2.5-PRO"), 1_048_576);
    }

    #[test]
    fn test_model_context_window_unknown() {
        assert_eq!(model_context_window("some-custom-model"), 32_000);
    }

    #[test]
    fn test_trim_empty() {
        let result = trim_to_context_window(&[], 4096, 512);
        assert!(result.is_empty());
    }

    #[test]
    fn test_trim_keeps_system_and_newest() {
        let messages = vec![
            msg(Role::System, "You are helpful."),
            msg(Role::User, "Hello"),
            msg(Role::Assistant, "Hi there!"),
            msg(Role::User, "How are you?"),
        ];
        // With +4 overhead per message, system ≈ 8, each msg ≈ 5-7
        // Budget of 20 should fit system + only newest
        let result = trim_to_context_window(&messages, 30, 5);
        assert_eq!(result[0].role, Role::System);
        // Last message should be present
        assert_eq!(result.last().unwrap().text_content(), "How are you?");
    }

    #[test]
    fn test_trim_all_fit() {
        let messages = vec![
            msg(Role::System, "Sys"),
            msg(Role::User, "Hi"),
            msg(Role::Assistant, "Hey"),
        ];
        let result = trim_to_context_window(&messages, 100_000, 512);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_trim_preserves_tool_call_pairs() {
        let messages = vec![
            msg(Role::System, "You are helpful."),
            msg(Role::User, "first question"),
            Message {
                role: Role::Assistant,
                parts: vec![ContentPart::Text { text: "Let me search.".to_string() }],
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "tc1".to_string(),
                    name: "search".to_string(),
                    arguments: "{}".to_string(),
                }]),
            },
            Message::text_with_name(Role::Tool, "Result: found something", "tc1"),
            msg(Role::Assistant, "Based on the search, here is the answer."),
            msg(Role::User, "second question"),
            msg(Role::Assistant, "Here is the second answer."),
        ];

        // Give enough budget for system + last 2 messages but NOT the tool call block
        let result = trim_to_context_window(&messages, 60, 10);

        // Verify that if the tool call assistant is present, the tool result is too
        let has_tool_call_assistant = result.iter().any(|m| {
            m.role == Role::Assistant
                && m.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty())
        });
        let has_tool_result = result.iter().any(|m| m.role == Role::Tool);

        // Either both present or both absent — never split
        assert_eq!(
            has_tool_call_assistant, has_tool_result,
            "Tool call assistant and tool result must be kept or dropped together"
        );
    }
}
