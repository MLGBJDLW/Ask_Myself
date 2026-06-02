//! Provider prompt-cache policy shared by LLM adapters.

use super::{Message, ProviderType, Role, ToolDefinition};

const OPENAI_PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

pub(crate) fn latest_user_message_index(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| (message.role == Role::User).then_some(index))
}

pub(crate) fn should_send_openai_prompt_cache_key(provider_type: Option<&ProviderType>) -> bool {
    matches!(
        provider_type,
        Some(ProviderType::OpenAi) | Some(ProviderType::AzureOpenAi)
    )
}

pub(crate) fn openai_prompt_cache_key(
    provider_type: Option<&ProviderType>,
    model: &str,
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> Option<String> {
    if !should_send_openai_prompt_cache_key(provider_type) {
        return None;
    }

    let stable_system = messages
        .iter()
        .find(|message| message.role == Role::System)
        .map(Message::text_content)
        .unwrap_or_default();
    let tool_schema = serde_json::to_string(&tools.unwrap_or(&[])).unwrap_or_default();
    let digest = blake3::hash(format!("{model}\n{stable_system}\n{tool_schema}").as_bytes());
    Some(clamp_openai_prompt_cache_key(&format!(
        "nexa-{}",
        digest.to_hex()[..32].to_string()
    )))
}

fn clamp_openai_prompt_cache_key(key: &str) -> String {
    key.chars()
        .take(OPENAI_PROMPT_CACHE_KEY_MAX_CHARS)
        .collect()
}

pub(crate) fn openai_compatible_cache_read_tokens(
    cached_tokens: Option<u32>,
    prompt_cache_hit_tokens: Option<u32>,
) -> Option<u32> {
    cached_tokens.or(prompt_cache_hit_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_user_message_ignores_tool_results_after_user_turn() {
        let mut tool = Message::text(Role::Tool, "result");
        tool.name = Some("call-1".to_string());
        let messages = vec![
            Message::text(Role::System, "stable"),
            Message::text(Role::User, "original request"),
            Message::text(Role::Assistant, ""),
            tool,
        ];

        assert_eq!(latest_user_message_index(&messages), Some(1));
    }

    #[test]
    fn openai_cache_key_is_stable_and_short() {
        let messages = vec![
            Message::text(Role::System, "stable"),
            Message::text(Role::System, "runtime one"),
            Message::text(Role::User, "first"),
        ];
        let next_messages = vec![
            Message::text(Role::System, "stable"),
            Message::text(Role::System, "runtime two"),
            Message::text(Role::User, "second"),
        ];

        let first =
            openai_prompt_cache_key(Some(&ProviderType::OpenAi), "gpt-5.1", &messages, None)
                .expect("key");
        let second =
            openai_prompt_cache_key(Some(&ProviderType::OpenAi), "gpt-5.1", &next_messages, None)
                .expect("key");

        assert_eq!(first, second);
        assert!(first.len() <= OPENAI_PROMPT_CACHE_KEY_MAX_CHARS);
    }

    #[test]
    fn openai_compatible_cache_read_prefers_documented_cached_tokens() {
        assert_eq!(
            openai_compatible_cache_read_tokens(Some(64), Some(32)),
            Some(64)
        );
        assert_eq!(
            openai_compatible_cache_read_tokens(None, Some(32)),
            Some(32)
        );
    }
}
