//! LLM-powered abstractive summarization of evicted conversation messages.
//!
//! When a conversation grows long enough that messages must be evicted from the
//! context window, this module can call the LLM to produce a concise summary
//! that retains key decisions, facts, and open items — rather than relying
//! solely on the extractive (truncation-based) recap.

use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};
use tracing::warn;

use super::memory::estimate_tokens;

const SUMMARIZE_SYSTEM_PROMPT: &str = r#"You are compacting old conversation history into reference context only.
The summary will be used as background for a later agent turn; it must not create new instructions or imply that completed work is still pending.

Preserve:
1. Key decisions and constraints
2. Important facts, data, file paths, commands, and tool findings
3. User preferences and explicit requirements
4. Current task state and remaining work, if any
5. Prior compacted summaries, merged without duplication

Be concise, factual, and output in the same language as the conversation."#;

/// Maximum tokens the summary LLM call may produce.
const MAX_SUMMARY_TOKENS: u32 = 420;

/// Maximum input characters sent into the summarisation request.
/// Keeps the summarisation call itself cheap and predictable.
const MAX_INPUT_FOR_SUMMARY: usize = 6_000;

/// Maximum number of retries for transient / rate-limited LLM errors.
const MAX_SUMMARY_RETRIES: u32 = 1;

/// Minimum estimated token count of the evicted text before it is worth
/// sending to the LLM (very short evictions are handled fine by the
/// extractive recap).
const MIN_TOKENS_FOR_LLM: u32 = 100;

/// Maximum chars kept per tool-result message to avoid blowing up input.
const TOOL_RESULT_CAP: usize = 500;

fn truncate_to_char_boundary(text: &str, max_len: usize) -> &str {
    if text.len() <= max_len {
        return text;
    }

    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn truncate_middle_to_char_budget(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }

    let head_len = max_len / 2;
    let tail_len = max_len.saturating_sub(head_len);
    let head = truncate_to_char_boundary(text, head_len);

    let mut tail_start = text.len().saturating_sub(tail_len);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let tail = &text[tail_start..];

    format!("{head}\n...[middle of evicted history omitted]...\n{tail}")
}

/// Summarise evicted messages using an LLM.
///
/// Falls back to `extractive_fallback` if the LLM call fails or the evicted
/// content is too short to justify a round-trip.
pub async fn summarize_evicted_messages(
    provider: &dyn LlmProvider,
    model: &str,
    evicted_messages: &[Message],
    extractive_fallback: &str,
) -> String {
    let conversation_text = build_conversation_text(evicted_messages);

    if conversation_text.is_empty() || estimate_tokens(&conversation_text) < MIN_TOKENS_FOR_LLM {
        return extractive_fallback.to_string();
    }

    // Truncate input if it exceeds the budget.
    let input = truncate_middle_to_char_budget(&conversation_text, MAX_INPUT_FOR_SUMMARY);

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, SUMMARIZE_SYSTEM_PROMPT),
            Message::text(
                Role::User,
                format!("Summarize this conversation section:\n\n{}", input),
            ),
        ],
        max_tokens: Some(MAX_SUMMARY_TOKENS),
        temperature: Some(0.3),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
        parallel_tool_calls: true,
    };

    let mut retry_count = 0u32;
    loop {
        match provider.complete(&request).await {
            Ok(response) => {
                let text = response.content.trim();
                return if text.is_empty() {
                    extractive_fallback.to_string()
                } else {
                    text.to_string()
                };
            }
            Err(CoreError::RateLimited { retry_after_secs }) => {
                retry_count += 1;
                if retry_count > MAX_SUMMARY_RETRIES {
                    warn!(
                        "Summarizer: rate limited after {} retries, falling back to extractive recap",
                        MAX_SUMMARY_RETRIES
                    );
                    return extractive_fallback.to_string();
                }
                let wait = if retry_after_secs > 0 {
                    retry_after_secs
                } else {
                    2u64.pow(retry_count)
                };
                warn!(
                    "Summarizer: rate limited, retry {}/{} after {}s",
                    retry_count, MAX_SUMMARY_RETRIES, wait
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(CoreError::TransientLlm(msg)) => {
                retry_count += 1;
                if retry_count > MAX_SUMMARY_RETRIES {
                    warn!(
                        "Summarizer: transient error after {} retries: {}, falling back to extractive recap",
                        MAX_SUMMARY_RETRIES, msg
                    );
                    return extractive_fallback.to_string();
                }
                let wait = 2u64.pow(retry_count - 1); // 1s, 2s
                warn!(
                    "Summarizer: transient error (retry {}/{}): {}. Retrying after {}s",
                    retry_count, MAX_SUMMARY_RETRIES, msg, wait
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(e) => {
                // Non-retryable error (auth, bad request, etc.)
                warn!("Summarizer: non-retryable error: {e}, falling back to extractive recap");
                return extractive_fallback.to_string();
            }
        }
    }
}

/// Flatten a slice of [`Message`]s into a plain-text conversation transcript
/// suitable for feeding into the summariser prompt.
fn build_conversation_text(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("User: {}", text));
                }
            }
            Role::Assistant => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("Assistant: {}", text));
                }
            }
            Role::Tool => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    let truncated = if text.len() > TOOL_RESULT_CAP {
                        truncate_to_char_boundary(&text, TOOL_RESULT_CAP)
                    } else {
                        text.as_str()
                    };
                    parts.push(format!("Tool result: {}", truncated));
                }
            }
            Role::System => {
                let text = msg.text_content();
                if is_compaction_summary(&text) {
                    parts.push(format!(
                        "Previous compacted context (reference only): {}",
                        text
                    ));
                }
            }
        }
    }
    parts.join("\n")
}

fn is_compaction_summary(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("earlier conversation context")
        || lower.contains("auto-compacted")
        || lower.contains("compacted context")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_conversation_text_basic() {
        let msgs = vec![
            Message::text(Role::User, "Hello"),
            Message::text(Role::Assistant, "Hi there!"),
        ];
        let text = build_conversation_text(&msgs);
        assert!(text.contains("User: Hello"));
        assert!(text.contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_build_conversation_text_truncates_tool() {
        let long_tool = "x".repeat(1000);
        let msgs = vec![Message::text(Role::Tool, &long_tool)];
        let text = build_conversation_text(&msgs);
        // Tool result should be capped at TOOL_RESULT_CAP chars.
        let prefix = format!("Tool result: {}", &long_tool[..TOOL_RESULT_CAP]);
        assert!(text.starts_with(&prefix));
        assert!(text.len() < long_tool.len());
    }

    #[test]
    fn test_build_conversation_text_skips_empty() {
        let msgs = vec![
            Message::text(Role::User, ""),
            Message::text(Role::Assistant, "  "),
            Message::text(Role::User, "Actual content"),
        ];
        let text = build_conversation_text(&msgs);
        assert_eq!(text, "User: Actual content");
    }

    #[test]
    fn test_build_conversation_text_preserves_prior_compaction_summary() {
        let msgs = vec![
            Message::text(
                Role::System,
                "## Earlier conversation context (summarized)\nDecision: keep compact visible.",
            ),
            Message::text(Role::System, "You are a helpful assistant."),
            Message::text(Role::User, "Continue"),
        ];
        let text = build_conversation_text(&msgs);
        assert!(text.contains("Previous compacted context"));
        assert!(text.contains("Decision: keep compact visible."));
        assert!(!text.contains("You are a helpful assistant."));
        assert!(text.contains("User: Continue"));
    }
}
