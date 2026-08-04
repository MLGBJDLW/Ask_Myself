//! Provider prompt-cache capabilities shared by LLM adapters.
//!
//! Cache behavior is resolved from the complete provider boundary instead of
//! being guessed independently by each wire encoder.  The endpoint is reduced
//! to a privacy-safe identifier so diagnostics never persist a user URL.

use std::sync::OnceLock;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::provider_boundary::{
    endpoint_id, is_alibaba_chat_endpoint, is_anthropic_public_endpoint, is_azure_openai_endpoint,
    is_deepseek_public_endpoint, is_openai_public_endpoint, is_openrouter_public_endpoint,
    provider_id,
};
use super::{Message, ProviderType, Role, ToolDefinition};

const OPENAI_PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;
const ALIBABA_EXPLICIT_QWEN_PREFIXES: &[&str] = &["qwen3.5-", "qwen3.6-", "qwen3.7-", "qwen3.8-"];
static ROUTING_SESSION_SECRET: OnceLock<[u8; 32]> = OnceLock::new();

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PromptCacheApiStyle {
    OpenAiCompatible,
    AnthropicMessages,
    Gemini,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PromptCacheMode {
    None,
    ImplicitExactPrefix,
    ExplicitBreakpoints,
    RoutingDependent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CacheUsageDecoderId {
    None,
    OpenAiCompatible,
    DeepSeek,
    AlibabaOpenAiCompatible,
    OpenRouterNormalized,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheProfileKey {
    pub provider_id: String,
    pub endpoint_id: String,
    pub api_style: PromptCacheApiStyle,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheProfile {
    pub id: String,
    pub version: u32,
    pub key: PromptCacheProfileKey,
    pub mode: PromptCacheMode,
    pub min_cacheable_tokens: Option<u32>,
    pub max_breakpoints: Option<u8>,
    pub ttl_seconds: Option<u32>,
    pub lookback_content_blocks: Option<u32>,
    pub message_content_markers: bool,
    pub tool_definition_markers: bool,
    pub tool_definitions_are_prefix: bool,
    pub requires_stable_tool_serialization: bool,
    pub usage_decoder: CacheUsageDecoderId,
    pub routing_session_affinity: bool,
}

impl Default for PromptCacheProfile {
    fn default() -> Self {
        Self::unsupported(PromptCacheProfileKey {
            provider_id: "unknown".to_string(),
            endpoint_id: "unknown".to_string(),
            api_style: PromptCacheApiStyle::Local,
            model_id: String::new(),
        })
    }
}

impl PromptCacheProfile {
    fn unsupported(key: PromptCacheProfileKey) -> Self {
        Self {
            id: "unsupported-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::None,
            min_cacheable_tokens: None,
            max_breakpoints: None,
            ttl_seconds: None,
            lookback_content_blocks: None,
            message_content_markers: false,
            tool_definition_markers: false,
            tool_definitions_are_prefix: false,
            requires_stable_tool_serialization: false,
            usage_decoder: CacheUsageDecoderId::None,
            routing_session_affinity: false,
        }
    }

    pub(crate) fn uses_message_breakpoints(&self) -> bool {
        self.mode == PromptCacheMode::ExplicitBreakpoints && self.message_content_markers
    }

    pub(crate) fn sends_openai_prompt_cache_key(&self) -> bool {
        self.id == "openai-automatic-v1"
    }

    pub(crate) fn request_is_eligible(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> bool {
        let Some(minimum) = self.min_cacheable_tokens else {
            return self.mode != PromptCacheMode::None;
        };
        estimated_prompt_tokens(messages, tools) >= minimum
    }
}

fn is_explicit_qwen_model(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase();
    ALIBABA_EXPLICIT_QWEN_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

pub fn resolve_prompt_cache_profile(
    provider_type: ProviderType,
    base_url: Option<&str>,
    api_style: PromptCacheApiStyle,
    model: &str,
) -> PromptCacheProfile {
    let key = PromptCacheProfileKey {
        provider_id: provider_id(provider_type).to_string(),
        endpoint_id: endpoint_id(provider_type, base_url),
        api_style,
        model_id: model.to_string(),
    };

    if api_style == PromptCacheApiStyle::OpenAiCompatible
        && matches!(
            provider_type,
            ProviderType::OpenAi | ProviderType::AzureOpenAi
        )
        && ((provider_type == ProviderType::OpenAi
            && is_openai_public_endpoint(provider_type, base_url))
            || (provider_type == ProviderType::AzureOpenAi
                && is_azure_openai_endpoint(provider_type, base_url)))
    {
        return PromptCacheProfile {
            id: "openai-automatic-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::ImplicitExactPrefix,
            min_cacheable_tokens: Some(1_024),
            max_breakpoints: None,
            ttl_seconds: None,
            lookback_content_blocks: None,
            message_content_markers: false,
            tool_definition_markers: false,
            tool_definitions_are_prefix: true,
            requires_stable_tool_serialization: true,
            usage_decoder: CacheUsageDecoderId::OpenAiCompatible,
            routing_session_affinity: false,
        };
    }

    if api_style == PromptCacheApiStyle::OpenAiCompatible
        && provider_type == ProviderType::DeepSeek
        && is_deepseek_public_endpoint(provider_type, base_url)
    {
        return PromptCacheProfile {
            id: "deepseek-exact-prefix-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::ImplicitExactPrefix,
            min_cacheable_tokens: None,
            max_breakpoints: None,
            ttl_seconds: None,
            lookback_content_blocks: None,
            message_content_markers: false,
            tool_definition_markers: false,
            tool_definitions_are_prefix: true,
            requires_stable_tool_serialization: true,
            usage_decoder: CacheUsageDecoderId::DeepSeek,
            routing_session_affinity: false,
        };
    }

    if api_style == PromptCacheApiStyle::OpenAiCompatible
        && matches!(
            provider_type,
            ProviderType::Qwen | ProviderType::AlibabaModelStudio
        )
        && is_alibaba_chat_endpoint(provider_type, base_url)
        && is_explicit_qwen_model(model)
    {
        return PromptCacheProfile {
            id: "alibaba-qwen-explicit-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::ExplicitBreakpoints,
            min_cacheable_tokens: Some(1_024),
            max_breakpoints: Some(4),
            ttl_seconds: Some(300),
            lookback_content_blocks: Some(20),
            message_content_markers: true,
            tool_definition_markers: false,
            tool_definitions_are_prefix: true,
            requires_stable_tool_serialization: true,
            usage_decoder: CacheUsageDecoderId::AlibabaOpenAiCompatible,
            routing_session_affinity: false,
        };
    }

    if api_style == PromptCacheApiStyle::OpenAiCompatible
        && provider_type == ProviderType::OpenRouter
        && is_openrouter_public_endpoint(provider_type, base_url)
    {
        return PromptCacheProfile {
            id: "openrouter-routing-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::RoutingDependent,
            min_cacheable_tokens: None,
            max_breakpoints: None,
            ttl_seconds: None,
            lookback_content_blocks: None,
            message_content_markers: false,
            tool_definition_markers: false,
            tool_definitions_are_prefix: true,
            requires_stable_tool_serialization: true,
            usage_decoder: CacheUsageDecoderId::OpenRouterNormalized,
            routing_session_affinity: true,
        };
    }

    if api_style == PromptCacheApiStyle::AnthropicMessages
        && provider_type == ProviderType::Anthropic
        && is_anthropic_public_endpoint(provider_type, base_url)
    {
        return PromptCacheProfile {
            id: "anthropic-explicit-v1".to_string(),
            version: 1,
            key,
            mode: PromptCacheMode::ExplicitBreakpoints,
            min_cacheable_tokens: Some(1_024),
            max_breakpoints: Some(4),
            ttl_seconds: Some(300),
            lookback_content_blocks: Some(20),
            message_content_markers: true,
            tool_definition_markers: true,
            tool_definitions_are_prefix: true,
            requires_stable_tool_serialization: true,
            usage_decoder: CacheUsageDecoderId::Anthropic,
            routing_session_affinity: false,
        };
    }

    PromptCacheProfile::unsupported(key)
}

/// Configure the app-installation secret used to pseudonymize routing sessions.
/// The first caller wins so a library consumer cannot rotate identifiers midway
/// through a process.
pub fn configure_routing_session_secret(secret: &[u8]) -> bool {
    if secret.is_empty() {
        return false;
    }
    ROUTING_SESSION_SECRET
        .set(*blake3::hash(secret).as_bytes())
        .is_ok()
}

pub fn privacy_preserving_routing_session_id_with_secret(
    secret: &[u8],
    conversation_id: &str,
) -> Option<String> {
    let trimmed = conversation_id.trim();
    if trimmed.is_empty() || secret.is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;
    mac.update(b"nexa-openrouter-session-v2\n");
    mac.update(trimmed.as_bytes());
    let digest = mac.finalize().into_bytes();
    let encoded = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(format!("nexa-{encoded}"))
}

pub fn privacy_preserving_routing_session_id(conversation_id: &str) -> Option<String> {
    let secret = ROUTING_SESSION_SECRET.get_or_init(|| {
        // Non-desktop library consumers still get process-local unlinkability.
        // The desktop configures a persisted installation secret during setup.
        *blake3::hash(uuid::Uuid::new_v4().as_bytes()).as_bytes()
    });
    privacy_preserving_routing_session_id_with_secret(secret, conversation_id)
}

fn estimated_prompt_tokens(messages: &[Message], tools: Option<&[ToolDefinition]>) -> u32 {
    let message_chars = messages
        .iter()
        .map(|message| message.text_content().chars().count())
        .sum::<usize>();
    let tool_chars = serde_json::to_string(&tools.unwrap_or(&[]))
        .map(|value| value.chars().count())
        .unwrap_or(0);
    u32::try_from((message_chars.saturating_add(tool_chars).saturating_add(3)) / 4)
        .unwrap_or(u32::MAX)
}

pub(crate) fn openai_prompt_cache_key(
    profile: &PromptCacheProfile,
    model: &str,
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> Option<String> {
    if !profile.sends_openai_prompt_cache_key() {
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
        &digest.to_hex()[..32]
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
    fn openai_cache_key_is_stable_and_short() {
        let profile = resolve_prompt_cache_profile(
            ProviderType::OpenAi,
            None,
            PromptCacheApiStyle::OpenAiCompatible,
            "gpt-5.1",
        );
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

        let first = openai_prompt_cache_key(&profile, "gpt-5.1", &messages, None).expect("key");
        let second =
            openai_prompt_cache_key(&profile, "gpt-5.1", &next_messages, None).expect("key");

        assert_eq!(first, second);
        assert!(first.len() <= OPENAI_PROMPT_CACHE_KEY_MAX_CHARS);
    }

    #[test]
    fn profile_matrix_requires_provider_endpoint_api_and_model_match() {
        let direct = resolve_prompt_cache_profile(
            ProviderType::Qwen,
            Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"),
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.8-max-preview",
        );
        let routed = resolve_prompt_cache_profile(
            ProviderType::AlibabaModelStudio,
            Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.7-max",
        );
        let unknown_endpoint = resolve_prompt_cache_profile(
            ProviderType::Qwen,
            Some("https://example.com/v1"),
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.8-max-preview",
        );
        let unsupported_snapshot = resolve_prompt_cache_profile(
            ProviderType::AlibabaModelStudio,
            None,
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.4-max",
        );

        assert_eq!(direct.id, "alibaba-qwen-explicit-v1");
        assert_eq!(routed.id, direct.id);
        assert_eq!(direct.max_breakpoints, Some(4));
        assert_eq!(direct.ttl_seconds, Some(300));
        assert_eq!(direct.lookback_content_blocks, Some(20));
        assert!(!direct.tool_definition_markers);
        assert_eq!(unknown_endpoint.mode, PromptCacheMode::None);
        assert_eq!(unsupported_snapshot.mode, PromptCacheMode::None);

        let global = resolve_prompt_cache_profile(
            ProviderType::Qwen,
            Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.8-max",
        );
        let payg = resolve_prompt_cache_profile(
            ProviderType::AlibabaModelStudio,
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            PromptCacheApiStyle::OpenAiCompatible,
            "qwen3.8-max",
        );
        assert_ne!(direct.key.endpoint_id, global.key.endpoint_id);
        assert_ne!(direct.key.endpoint_id, payg.key.endpoint_id);
        assert_ne!(global.key.endpoint_id, payg.key.endpoint_id);

        for endpoint in [
            "http://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com:8443/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/apps/anthropic",
        ] {
            let profile = resolve_prompt_cache_profile(
                ProviderType::AlibabaModelStudio,
                Some(endpoint),
                PromptCacheApiStyle::OpenAiCompatible,
                "qwen3.8-max",
            );
            assert_eq!(profile.mode, PromptCacheMode::None);
            assert!(profile.key.endpoint_id.starts_with("custom-"));
        }
    }

    #[test]
    fn routing_session_is_stable_private_and_bounded() {
        let first = privacy_preserving_routing_session_id_with_secret(
            b"installation-one",
            "conversation-123",
        )
        .unwrap();
        let second = privacy_preserving_routing_session_id_with_secret(
            b"installation-one",
            "conversation-123",
        )
        .unwrap();
        let other = privacy_preserving_routing_session_id_with_secret(
            b"installation-one",
            "conversation-456",
        )
        .unwrap();
        let other_install = privacy_preserving_routing_session_id_with_secret(
            b"installation-two",
            "conversation-123",
        )
        .unwrap();

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_ne!(first, other_install);
        assert!(!first.contains("conversation-123"));
        assert!(first.len() <= 256);
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
