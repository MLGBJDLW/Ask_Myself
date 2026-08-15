//! Endpoint-scoped reasoning capabilities for OpenAI-compatible providers.
//!
//! The same model family can use different request fields when it is served
//! directly, through Alibaba Model Studio, or through a subscription endpoint.
//! Resolve that boundary once and let the wire adapter consume the profile.

use serde::{Deserialize, Serialize};

use super::provider_boundary::{
    endpoint_id, is_alibaba_chat_endpoint, is_anthropic_public_endpoint, is_azure_openai_endpoint,
    is_deepseek_anthropic_endpoint, is_deepseek_public_endpoint, is_google_public_endpoint,
    is_minimax_public_endpoint, is_mistral_public_endpoint, is_moonshot_public_endpoint,
    is_openai_public_endpoint, is_openrouter_public_endpoint, is_siliconflow_public_endpoint,
    is_xai_public_endpoint, is_zhipu_model_api_endpoint, provider_id,
};
use super::{ProviderType, ReasoningEffort};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningApiStyle {
    OpenAiChatCompletions,
    OpenAiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingModeControl {
    Unsupported,
    ProviderDefault,
    AlwaysOn,
    AlwaysOnThinkingType,
    EnableThinking,
    ThinkingType,
    ThinkingTypeWithKeep,
    AdaptiveThinking,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningEffortField {
    None,
    TopLevel,
    NestedReasoning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningBudgetField {
    None,
    ThinkingBudget,
    NestedReasoning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningEffortMapping {
    Exact,
    OpenAiCompatible,
    Qwen38Chat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityConfidence {
    Verified,
    ConflictingDocs,
    CuratedCompatibility,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningHistoryEncoding {
    ReasoningContent,
    ThinkTags,
    MistralContentChunks,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningReplayPolicy {
    NotRequired,
    RequiredOnToolCall,
    RequiredAlways,
    OpaqueSignature,
    Forbidden,
    #[default]
    Unknown,
}

impl ReasoningReplayPolicy {
    pub fn requires_tool_call_payload(self) -> bool {
        matches!(
            self,
            Self::RequiredOnToolCall | Self::RequiredAlways | Self::OpaqueSignature
        )
    }

    pub fn authorizes_tool_call(self, payload_present: bool) -> bool {
        match self {
            Self::NotRequired => true,
            Self::RequiredOnToolCall | Self::RequiredAlways | Self::OpaqueSignature => {
                payload_present
            }
            Self::Forbidden | Self::Unknown => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningCaptureStatus {
    Captured,
    NotRequested,
    NotRequired,
    OmittedByProvider,
    MissingFromLegacyHistory,
    Interrupted,
    Truncated,
    Redacted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_payload: Option<serde_json::Value>,
    pub status: ReasoningCaptureStatus,
    pub required_for_replay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_field: Option<String>,
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningProfileKey {
    pub provider_id: String,
    pub endpoint_id: String,
    pub api_style: ReasoningApiStyle,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningProfile {
    pub id: String,
    pub version: u32,
    pub key: ReasoningProfileKey,
    pub mode_control: ThinkingModeControl,
    pub effort_field: ReasoningEffortField,
    pub effort_mapping: ReasoningEffortMapping,
    pub accepted_efforts: Vec<ReasoningEffort>,
    pub default_effort: Option<ReasoningEffort>,
    pub budget_field: ReasoningBudgetField,
    pub min_budget_tokens: Option<u32>,
    pub max_budget_tokens: Option<u32>,
    pub effort_budget_exclusive: bool,
    pub preserve_reasoning_history: bool,
    pub reasoning_history_encoding: ReasoningHistoryEncoding,
    pub replay_policy: ReasoningReplayPolicy,
    pub send_preserve_thinking: bool,
    pub omit_temperature_when_reasoning: bool,
    #[serde(default)]
    pub omit_stop_when_reasoning: bool,
    pub use_max_completion_tokens: bool,
    pub confidence: CapabilityConfidence,
}

impl ReasoningProfile {
    fn unsupported(key: ReasoningProfileKey) -> Self {
        Self {
            id: "unsupported-v1".to_string(),
            version: 1,
            key,
            mode_control: ThinkingModeControl::Unsupported,
            effort_field: ReasoningEffortField::None,
            effort_mapping: ReasoningEffortMapping::Exact,
            accepted_efforts: Vec::new(),
            default_effort: None,
            budget_field: ReasoningBudgetField::None,
            min_budget_tokens: None,
            max_budget_tokens: None,
            effort_budget_exclusive: false,
            preserve_reasoning_history: false,
            reasoning_history_encoding: ReasoningHistoryEncoding::ReasoningContent,
            replay_policy: ReasoningReplayPolicy::Unknown,
            send_preserve_thinking: false,
            omit_temperature_when_reasoning: false,
            omit_stop_when_reasoning: false,
            use_max_completion_tokens: false,
            confidence: CapabilityConfidence::Unknown,
        }
    }

    pub(crate) fn requested_mode(
        &self,
        enabled: Option<bool>,
        effort: Option<&ReasoningEffort>,
        budget: Option<u32>,
    ) -> Option<bool> {
        if matches!(
            self.mode_control,
            ThinkingModeControl::AlwaysOn | ThinkingModeControl::AlwaysOnThinkingType
        ) {
            return Some(true);
        }
        if effort == Some(&ReasoningEffort::None) {
            return Some(false);
        }
        enabled
            .or_else(|| effort.map(|_| true))
            .or_else(|| budget.map(|_| true))
    }

    pub(crate) fn wire_effort(&self, effort: Option<&ReasoningEffort>) -> Option<String> {
        let effort = effort?;
        let normalized = match self.effort_mapping {
            ReasoningEffortMapping::Qwen38Chat => match effort {
                ReasoningEffort::Minimal | ReasoningEffort::Low => ReasoningEffort::Low,
                ReasoningEffort::Medium => ReasoningEffort::Medium,
                ReasoningEffort::High | ReasoningEffort::Max | ReasoningEffort::XHigh => {
                    ReasoningEffort::XHigh
                }
                ReasoningEffort::None => return None,
            },
            ReasoningEffortMapping::OpenAiCompatible | ReasoningEffortMapping::Exact => {
                effort.clone()
            }
        };
        self.accepted_efforts
            .contains(&normalized)
            .then(|| normalized.to_string())
    }

    pub(crate) fn wire_budget(&self, budget: Option<u32>, has_wire_effort: bool) -> Option<u32> {
        if self.budget_field == ReasoningBudgetField::None
            || (self.effort_budget_exclusive && has_wire_effort)
        {
            return None;
        }
        budget.map(|value| {
            let with_min = self.min_budget_tokens.map_or(value, |min| value.max(min));
            self.max_budget_tokens
                .map_or(with_min, |max| with_min.min(max))
        })
    }

    pub(crate) fn should_replay_reasoning(&self, requested_mode: Option<bool>) -> bool {
        self.preserve_reasoning_history && requested_mode != Some(false)
    }
}

fn profile(
    key: ReasoningProfileKey,
    id: &str,
    mode_control: ThinkingModeControl,
    effort_field: ReasoningEffortField,
    effort_mapping: ReasoningEffortMapping,
    effort_policy: (&[ReasoningEffort], Option<ReasoningEffort>),
    budget_field: ReasoningBudgetField,
) -> ReasoningProfile {
    let (accepted_efforts, default_effort) = effort_policy;
    ReasoningProfile {
        id: id.to_string(),
        version: 1,
        key,
        mode_control,
        effort_field,
        effort_mapping,
        accepted_efforts: accepted_efforts.to_vec(),
        default_effort,
        budget_field,
        min_budget_tokens: None,
        max_budget_tokens: None,
        effort_budget_exclusive: false,
        preserve_reasoning_history: false,
        reasoning_history_encoding: ReasoningHistoryEncoding::ReasoningContent,
        replay_policy: ReasoningReplayPolicy::NotRequired,
        send_preserve_thinking: false,
        omit_temperature_when_reasoning: false,
        omit_stop_when_reasoning: false,
        use_max_completion_tokens: false,
        confidence: CapabilityConfidence::Verified,
    }
}

pub fn resolve_reasoning_profile(
    provider: ProviderType,
    base_url: Option<&str>,
    api_style: ReasoningApiStyle,
    model: &str,
) -> ReasoningProfile {
    let key = ReasoningProfileKey {
        provider_id: provider_id(provider).to_string(),
        endpoint_id: endpoint_id(provider, base_url),
        api_style,
        model_id: model.to_string(),
    };
    if api_style == ReasoningApiStyle::OpenAiResponses {
        let mut value = ReasoningProfile::unsupported(key);
        let trusted_codec = match provider {
            ProviderType::DeepSeek => is_deepseek_public_endpoint(provider, base_url),
            ProviderType::OpenAi => is_openai_public_endpoint(provider, base_url),
            _ => false,
        };
        if !trusted_codec {
            return value;
        }
        value.id = match provider {
            ProviderType::DeepSeek => "deepseek-responses-replay-v1",
            _ => "openai-responses-replay-v1",
        }
        .to_string();
        value.preserve_reasoning_history = true;
        value.replay_policy = ReasoningReplayPolicy::OpaqueSignature;
        value.confidence = CapabilityConfidence::Verified;
        return value;
    }
    if api_style == ReasoningApiStyle::AnthropicMessages {
        let mut value = ReasoningProfile::unsupported(key);
        let is_deepseek_compat = is_deepseek_anthropic_endpoint(provider, base_url);
        if !is_anthropic_public_endpoint(provider, base_url) && !is_deepseek_compat {
            return value;
        }
        value.id = if is_deepseek_compat {
            "deepseek-anthropic-signed-thinking-v1"
        } else {
            "anthropic-signed-thinking-v1"
        }
        .to_string();
        value.mode_control = ThinkingModeControl::ThinkingType;
        value.preserve_reasoning_history = true;
        value.replay_policy = ReasoningReplayPolicy::OpaqueSignature;
        value.confidence = CapabilityConfidence::Verified;
        return value;
    }
    if api_style == ReasoningApiStyle::GeminiGenerateContent {
        let mut value = ReasoningProfile::unsupported(key);
        if !is_google_public_endpoint(provider, base_url) {
            return value;
        }
        value.id = "gemini-thought-signature-v1".to_string();
        value.mode_control = ThinkingModeControl::ProviderDefault;
        value.preserve_reasoning_history = true;
        value.replay_policy = ReasoningReplayPolicy::OpaqueSignature;
        value.confidence = CapabilityConfidence::Verified;
        return value;
    }
    if api_style != ReasoningApiStyle::OpenAiChatCompletions {
        return ReasoningProfile::unsupported(key);
    }

    let model = model.trim().to_ascii_lowercase();
    if (provider == ProviderType::OpenAi && is_openai_public_endpoint(provider, base_url))
        || (provider == ProviderType::AzureOpenAi && is_azure_openai_endpoint(provider, base_url))
    {
        let mut value = profile(
            key,
            "openai-reasoning-v1",
            ThinkingModeControl::ProviderDefault,
            ReasoningEffortField::TopLevel,
            ReasoningEffortMapping::OpenAiCompatible,
            (
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                    ReasoningEffort::XHigh,
                ],
                None,
            ),
            ReasoningBudgetField::None,
        );
        value.omit_temperature_when_reasoning = true;
        value.use_max_completion_tokens = true;
        return value;
    }

    if is_xai_public_endpoint(provider, base_url) {
        return match model.as_str() {
            "grok-4.6" => {
                let mut value = profile(
                    key,
                    "xai-grok-4.6-reasoning-v1",
                    ThinkingModeControl::AlwaysOn,
                    ReasoningEffortField::TopLevel,
                    ReasoningEffortMapping::Exact,
                    (
                        &[
                            ReasoningEffort::Low,
                            ReasoningEffort::Medium,
                            ReasoningEffort::High,
                            ReasoningEffort::XHigh,
                        ],
                        Some(ReasoningEffort::High),
                    ),
                    ReasoningBudgetField::None,
                );
                value.omit_stop_when_reasoning = true;
                value
            }
            "grok-4.5" => {
                let mut value = profile(
                    key,
                    "xai-grok-4.5-reasoning-v1",
                    ThinkingModeControl::AlwaysOn,
                    ReasoningEffortField::TopLevel,
                    ReasoningEffortMapping::Exact,
                    (
                        &[
                            ReasoningEffort::Low,
                            ReasoningEffort::Medium,
                            ReasoningEffort::High,
                        ],
                        Some(ReasoningEffort::High),
                    ),
                    ReasoningBudgetField::None,
                );
                value.omit_stop_when_reasoning = true;
                value
            }
            "grok-4.3" => profile(
                key,
                "xai-grok-4.3-reasoning-v1",
                ThinkingModeControl::ProviderDefault,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (
                    &[
                        ReasoningEffort::None,
                        ReasoningEffort::Low,
                        ReasoningEffort::Medium,
                        ReasoningEffort::High,
                    ],
                    Some(ReasoningEffort::Medium),
                ),
                ReasoningBudgetField::None,
            ),
            "grok-build-0.1" | "grok-4.20-0309-reasoning" => profile(
                key,
                "xai-native-reasoning-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            ),
            // The direct multi-agent model is Responses-only. This Chat
            // Completions adapter must not translate its nested control.
            _ => ReasoningProfile::unsupported(key),
        };
    }

    if is_minimax_public_endpoint(provider, base_url) && model.starts_with("minimax-m") {
        let mut value = profile(
            key,
            "minimax-native-reasoning-v1",
            ThinkingModeControl::AlwaysOn,
            ReasoningEffortField::None,
            ReasoningEffortMapping::Exact,
            (&[], None),
            ReasoningBudgetField::None,
        );
        value.preserve_reasoning_history = true;
        value.reasoning_history_encoding = ReasoningHistoryEncoding::ThinkTags;
        return value;
    }

    if is_mistral_public_endpoint(provider, base_url) {
        let mut value = match model.as_str() {
            "mistral-medium-3-5" => profile(
                key,
                "mistral-adjustable-reasoning-v1",
                ThinkingModeControl::ProviderDefault,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (
                    &[
                        ReasoningEffort::None,
                        ReasoningEffort::Minimal,
                        ReasoningEffort::Low,
                        ReasoningEffort::Medium,
                        ReasoningEffort::High,
                        ReasoningEffort::XHigh,
                    ],
                    Some(ReasoningEffort::Medium),
                ),
                ReasoningBudgetField::None,
            ),
            "magistral-medium-2509" => profile(
                key,
                "mistral-native-reasoning-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            ),
            _ => return ReasoningProfile::unsupported(key),
        };
        value.preserve_reasoning_history = true;
        value.reasoning_history_encoding = ReasoningHistoryEncoding::MistralContentChunks;
        return value;
    }

    if provider == ProviderType::OpenRouter && is_openrouter_public_endpoint(provider, base_url) {
        let mut value = profile(
            key,
            "openrouter-normalized-reasoning-v1",
            ThinkingModeControl::ProviderDefault,
            ReasoningEffortField::NestedReasoning,
            ReasoningEffortMapping::OpenAiCompatible,
            (
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                    ReasoningEffort::XHigh,
                ],
                None,
            ),
            ReasoningBudgetField::NestedReasoning,
        );
        value.effort_budget_exclusive = true;
        value.confidence = CapabilityConfidence::CuratedCompatibility;
        return value;
    }

    if provider == ProviderType::DeepSeek
        && is_deepseek_public_endpoint(provider, base_url)
        && matches!(model.as_str(), "deepseek-v4-pro" | "deepseek-v4-flash")
    {
        let mut value = profile(
            key,
            "deepseek-direct-thinking-v1",
            ThinkingModeControl::ThinkingType,
            ReasoningEffortField::TopLevel,
            ReasoningEffortMapping::Exact,
            (
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ],
                Some(ReasoningEffort::High),
            ),
            ReasoningBudgetField::None,
        );
        value.preserve_reasoning_history = true;
        value.replay_policy = ReasoningReplayPolicy::RequiredOnToolCall;
        value.omit_temperature_when_reasoning = true;
        return value;
    }

    if provider == ProviderType::Moonshot && is_moonshot_public_endpoint(provider, base_url) {
        let mut value = match model.as_str() {
            "kimi-k3" => profile(
                key,
                "moonshot-kimi-k3-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (
                    &[
                        ReasoningEffort::Low,
                        ReasoningEffort::High,
                        ReasoningEffort::Max,
                    ],
                    Some(ReasoningEffort::Max),
                ),
                ReasoningBudgetField::None,
            ),
            "kimi-k2.7-code" | "kimi-k2.7-code-highspeed" => profile(
                key,
                "moonshot-kimi-k2.7-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            ),
            "kimi-k2.6" => profile(
                key,
                "moonshot-kimi-k2.6-v1",
                ThinkingModeControl::ThinkingTypeWithKeep,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            ),
            "kimi-k2.5" => profile(
                key,
                "moonshot-kimi-k2.5-v1",
                ThinkingModeControl::ThinkingType,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            ),
            _ => return ReasoningProfile::unsupported(key),
        };
        value.preserve_reasoning_history = true;
        value.omit_temperature_when_reasoning = true;
        return value;
    }

    if is_zhipu_model_api_endpoint(provider, base_url) {
        if model != "glm-5.3" {
            return ReasoningProfile::unsupported(key);
        }
        let mut value = profile(
            key,
            "zhipu-glm53-model-api-v1",
            ThinkingModeControl::AlwaysOnThinkingType,
            ReasoningEffortField::TopLevel,
            ReasoningEffortMapping::Exact,
            (
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ],
                Some(ReasoningEffort::Max),
            ),
            ReasoningBudgetField::None,
        );
        value.preserve_reasoning_history = true;
        value.omit_temperature_when_reasoning = true;
        return value;
    }

    if is_alibaba_chat_endpoint(provider, base_url) {
        if matches!(model.as_str(), "qwen3.8-max" | "qwen3.8-max-preview") {
            let mode_control = if model == "qwen3.8-max-preview" {
                ThinkingModeControl::AlwaysOn
            } else {
                ThinkingModeControl::EnableThinking
            };
            let mut value = profile(
                key,
                "alibaba-qwen3.8-chat-v1",
                mode_control,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Qwen38Chat,
                (
                    &[
                        ReasoningEffort::Low,
                        ReasoningEffort::Medium,
                        ReasoningEffort::XHigh,
                    ],
                    Some(ReasoningEffort::XHigh),
                ),
                ReasoningBudgetField::ThinkingBudget,
            );
            value.min_budget_tokens = Some(0);
            value.max_budget_tokens = Some(262_144);
            value.effort_budget_exclusive = true;
            value.preserve_reasoning_history = true;
            return value;
        }

        if model.starts_with("qwen3.5-")
            || model.starts_with("qwen3.6-")
            || model.starts_with("qwen3.7-")
        {
            let mut value = profile(
                key,
                "alibaba-qwen-hybrid-chat-v1",
                ThinkingModeControl::EnableThinking,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::ThinkingBudget,
            );
            value.preserve_reasoning_history = true;
            return value;
        }

        if model == "kimi/kimi-k3" {
            let mut value = profile(
                key,
                "alibaba-moonshot-kimi-k3-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (&[ReasoningEffort::Max], Some(ReasoningEffort::Max)),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.send_preserve_thinking = true;
            value.omit_temperature_when_reasoning = true;
            return value;
        }

        if matches!(
            model.as_str(),
            "kimi/kimi-k2.7-code" | "kimi/kimi-k2.7-code-highspeed" | "kimi-k2.7-code"
        ) {
            let mut value = profile(
                key,
                "alibaba-kimi-k2.7-v1",
                ThinkingModeControl::AlwaysOn,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.send_preserve_thinking = model.contains('/');
            value.omit_temperature_when_reasoning = true;
            return value;
        }

        if matches!(model.as_str(), "kimi/kimi-k2.6" | "kimi/kimi-k2.5") {
            let mut value = profile(
                key,
                "alibaba-moonshot-kimi-hybrid-v1",
                ThinkingModeControl::EnableThinking,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.send_preserve_thinking = model.ends_with("k2.6");
            value.omit_temperature_when_reasoning = true;
            value.confidence = CapabilityConfidence::ConflictingDocs;
            return value;
        }

        if matches!(model.as_str(), "kimi-k2.6" | "kimi-k2.5") {
            let mut value = profile(
                key,
                "alibaba-hosted-kimi-hybrid-v1",
                ThinkingModeControl::EnableThinking,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::ThinkingBudget,
            );
            value.preserve_reasoning_history = true;
            value.omit_temperature_when_reasoning = true;
            return value;
        }

        if matches!(model.as_str(), "deepseek-v4-pro" | "deepseek-v4-flash") {
            let mut value = profile(
                key,
                "alibaba-deepseek-v4-v1",
                ThinkingModeControl::EnableThinking,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (
                    &[
                        ReasoningEffort::Low,
                        ReasoningEffort::Medium,
                        ReasoningEffort::High,
                        ReasoningEffort::XHigh,
                        ReasoningEffort::Max,
                    ],
                    Some(ReasoningEffort::High),
                ),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.omit_temperature_when_reasoning = true;
            return value;
        }

        if matches!(
            model.as_str(),
            "glm-5.2" | "glm-5.2-fast-preview" | "glm-5.1" | "glm-5"
        ) {
            let efforts = if matches!(model.as_str(), "glm-5.2" | "glm-5.2-fast-preview") {
                vec![
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                    ReasoningEffort::Max,
                ]
            } else {
                vec![
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ]
            };
            let mut value = profile(
                key,
                "alibaba-glm-effort-v1",
                ThinkingModeControl::EnableThinking,
                ReasoningEffortField::TopLevel,
                ReasoningEffortMapping::Exact,
                (&efforts, None),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.omit_temperature_when_reasoning = true;
            return value;
        }

        if model == "minimax/minimax-m3" {
            let mut value = profile(
                key,
                "alibaba-minimax-m3-adaptive-v1",
                ThinkingModeControl::AdaptiveThinking,
                ReasoningEffortField::None,
                ReasoningEffortMapping::Exact,
                (&[], None),
                ReasoningBudgetField::None,
            );
            value.preserve_reasoning_history = true;
            value.omit_temperature_when_reasoning = true;
            return value;
        }
    }

    if provider == ProviderType::SiliconFlow && is_siliconflow_public_endpoint(provider, base_url) {
        let mut value = profile(
            key,
            "siliconflow-compatible-budget-v1",
            ThinkingModeControl::EnableThinking,
            ReasoningEffortField::None,
            ReasoningEffortMapping::Exact,
            (&[], None),
            ReasoningBudgetField::ThinkingBudget,
        );
        value.confidence = CapabilityConfidence::CuratedCompatibility;
        return value;
    }

    ReasoningProfile::unsupported(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_model_matrix_does_not_leak_reasoning_encoders() {
        let direct = resolve_reasoning_profile(
            ProviderType::Moonshot,
            Some("https://api.moonshot.ai/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "kimi-k3",
        );
        let routed = resolve_reasoning_profile(
            ProviderType::AlibabaModelStudio,
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "kimi/kimi-k3",
        );
        let unknown = resolve_reasoning_profile(
            ProviderType::AlibabaModelStudio,
            Some("https://example.com/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "kimi/kimi-k3",
        );

        assert_eq!(direct.id, "moonshot-kimi-k3-v1");
        assert_eq!(
            direct.accepted_efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::High,
                ReasoningEffort::Max
            ]
        );
        assert_eq!(routed.accepted_efforts, vec![ReasoningEffort::Max]);
        assert_eq!(unknown.mode_control, ThinkingModeControl::Unsupported);
    }

    #[test]
    fn qwen38_aliases_and_budget_exclusivity_follow_chat_contract() {
        let value = resolve_reasoning_profile(
            ProviderType::Qwen,
            Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "qwen3.8-max",
        );

        assert_eq!(
            value
                .wire_effort(Some(&ReasoningEffort::Minimal))
                .as_deref(),
            Some("low")
        );
        assert_eq!(
            value.wire_effort(Some(&ReasoningEffort::Max)).as_deref(),
            Some("xhigh")
        );
        assert_eq!(value.wire_budget(Some(300_000), false), Some(262_144));
        assert_eq!(value.wire_budget(Some(16_384), true), None);
    }

    #[test]
    fn non_https_and_non_chat_paths_are_not_trusted() {
        for endpoint in [
            "http://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com:8443/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/apps/anthropic",
        ] {
            let value = resolve_reasoning_profile(
                ProviderType::AlibabaModelStudio,
                Some(endpoint),
                ReasoningApiStyle::OpenAiChatCompletions,
                "qwen3.8-max",
            );
            assert_eq!(value.mode_control, ThinkingModeControl::Unsupported);
        }
    }

    #[test]
    fn curated_openai_compatible_endpoints_keep_only_their_exact_controls() {
        let grok46 = resolve_reasoning_profile(
            ProviderType::OpenAi,
            Some("https://api.x.ai/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "grok-4.6",
        );
        assert_eq!(grok46.default_effort, Some(ReasoningEffort::High));
        assert_eq!(
            grok46.wire_effort(Some(&ReasoningEffort::XHigh)).as_deref(),
            Some("xhigh")
        );

        let xai = resolve_reasoning_profile(
            ProviderType::OpenAi,
            Some("https://api.x.ai/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "grok-4.3",
        );
        assert_eq!(xai.key.endpoint_id, "xai-public");
        assert_eq!(
            xai.wire_effort(Some(&ReasoningEffort::None)).as_deref(),
            Some("none")
        );

        let minimax = resolve_reasoning_profile(
            ProviderType::OpenAi,
            Some("https://api.minimax.io/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "MiniMax-M3",
        );
        assert_eq!(minimax.mode_control, ThinkingModeControl::AlwaysOn);
        assert_eq!(
            minimax.reasoning_history_encoding,
            ReasoningHistoryEncoding::ThinkTags
        );

        let mistral = resolve_reasoning_profile(
            ProviderType::OpenAi,
            Some("https://api.mistral.ai/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "mistral-medium-3-5",
        );
        assert_eq!(
            mistral.wire_effort(Some(&ReasoningEffort::High)).as_deref(),
            Some("high")
        );

        for endpoint in [
            "https://api.x.ai/v2",
            "http://api.minimax.io/v1",
            "https://api.mistral.ai:8443/v1",
        ] {
            let value = resolve_reasoning_profile(
                ProviderType::OpenAi,
                Some(endpoint),
                ReasoningApiStyle::OpenAiChatCompletions,
                "grok-4.3",
            );
            assert_eq!(value.mode_control, ThinkingModeControl::Unsupported);
        }
    }

    #[test]
    fn responses_only_xai_multi_agent_is_not_encoded_as_chat_completions() {
        let value = resolve_reasoning_profile(
            ProviderType::OpenAi,
            Some("https://api.x.ai/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "grok-4.20-multi-agent-0309",
        );
        assert_eq!(value.mode_control, ThinkingModeControl::Unsupported);
    }

    #[test]
    fn glm53_model_api_is_always_on_and_endpoint_scoped() {
        for endpoint in [
            "https://open.bigmodel.cn/api/paas/v4",
            "https://api.z.ai/api/paas/v4",
        ] {
            let value = resolve_reasoning_profile(
                ProviderType::Zhipu,
                Some(endpoint),
                ReasoningApiStyle::OpenAiChatCompletions,
                "glm-5.3",
            );
            assert_eq!(value.id, "zhipu-glm53-model-api-v1");
            assert_eq!(
                value.mode_control,
                ThinkingModeControl::AlwaysOnThinkingType
            );
            assert_eq!(value.effort_field, ReasoningEffortField::TopLevel);
            assert_eq!(
                value.accepted_efforts,
                [
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ]
            );
            assert_eq!(value.default_effort, Some(ReasoningEffort::Max));
        }

        for endpoint in [
            "https://open.bigmodel.cn/api/coding/paas/v4",
            "https://open.bigmodel.cn/api/paas/v5",
            "https://api.z.ai:8443/api/paas/v4",
        ] {
            let value = resolve_reasoning_profile(
                ProviderType::Zhipu,
                Some(endpoint),
                ReasoningApiStyle::OpenAiChatCompletions,
                "glm-5.3",
            );
            assert_eq!(value.mode_control, ThinkingModeControl::Unsupported);
        }
    }

    #[test]
    fn provider_native_replay_codecs_require_exact_official_endpoints() {
        let deepseek_anthropic = resolve_reasoning_profile(
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com/anthropic"),
            ReasoningApiStyle::AnthropicMessages,
            "deepseek-v4",
        );
        assert_eq!(
            deepseek_anthropic.id,
            "deepseek-anthropic-signed-thinking-v1"
        );
        assert_eq!(
            deepseek_anthropic.replay_policy,
            ReasoningReplayPolicy::OpaqueSignature
        );
        assert_eq!(
            deepseek_anthropic.key.endpoint_id,
            "deepseek-anthropic-public"
        );

        let cases = [
            (
                ProviderType::OpenAi,
                ReasoningApiStyle::OpenAiResponses,
                "https://proxy.example.com/v1",
                "gpt-5",
            ),
            (
                ProviderType::Anthropic,
                ReasoningApiStyle::AnthropicMessages,
                "https://api.anthropic.com:8443",
                "claude-sonnet-4-5",
            ),
            (
                ProviderType::Google,
                ReasoningApiStyle::GeminiGenerateContent,
                "http://generativelanguage.googleapis.com/v1beta",
                "gemini-3-flash",
            ),
        ];

        for (provider, api_style, endpoint, model) in cases {
            let profile = resolve_reasoning_profile(provider, Some(endpoint), api_style, model);
            assert_eq!(profile.replay_policy, ReasoningReplayPolicy::Unknown);
            assert_eq!(profile.confidence, CapabilityConfidence::Unknown);
        }
    }

    #[test]
    fn deepseek_replay_requirement_is_scoped_to_the_exact_public_endpoint() {
        let direct = resolve_reasoning_profile(
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "deepseek-v4-pro",
        );
        let custom = resolve_reasoning_profile(
            ProviderType::DeepSeek,
            Some("https://deepseek.example.com/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "deepseek-v4",
        );
        let alibaba = resolve_reasoning_profile(
            ProviderType::AlibabaModelStudio,
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "deepseek-v4-pro",
        );

        assert_eq!(
            direct.replay_policy,
            ReasoningReplayPolicy::RequiredOnToolCall
        );
        assert_eq!(custom.replay_policy, ReasoningReplayPolicy::Unknown);
        assert_eq!(alibaba.replay_policy, ReasoningReplayPolicy::NotRequired);

        for unknown_model in [
            "deepseek-v4",
            "deepseek-v4-pro-preview",
            "deepseek-reasoner",
        ] {
            let unknown = resolve_reasoning_profile(
                ProviderType::DeepSeek,
                Some("https://api.deepseek.com/v1"),
                ReasoningApiStyle::OpenAiChatCompletions,
                unknown_model,
            );
            assert_eq!(unknown.replay_policy, ReasoningReplayPolicy::Unknown);
        }

        let flash = resolve_reasoning_profile(
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com/v1"),
            ReasoningApiStyle::OpenAiChatCompletions,
            "deepseek-v4-flash",
        );
        assert_eq!(
            flash.replay_policy,
            ReasoningReplayPolicy::RequiredOnToolCall
        );
    }
}
