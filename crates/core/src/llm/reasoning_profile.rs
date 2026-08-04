//! Endpoint-scoped reasoning capabilities for OpenAI-compatible providers.
//!
//! The same model family can use different request fields when it is served
//! directly, through Alibaba Model Studio, or through a subscription endpoint.
//! Resolve that boundary once and let the wire adapter consume the profile.

use serde::{Deserialize, Serialize};

use super::provider_boundary::{
    endpoint_id, is_alibaba_chat_endpoint, is_azure_openai_endpoint, is_deepseek_public_endpoint,
    is_minimax_public_endpoint, is_mistral_public_endpoint, is_moonshot_public_endpoint,
    is_openai_public_endpoint, is_openrouter_public_endpoint, is_siliconflow_public_endpoint,
    is_xai_public_endpoint, provider_id,
};
use super::{ProviderType, ReasoningEffort};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningApiStyle {
    OpenAiChatCompletions,
    OpenAiResponses,
    AnthropicMessages,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingModeControl {
    Unsupported,
    ProviderDefault,
    AlwaysOn,
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
    pub synthesize_missing_reasoning_history: bool,
    pub send_preserve_thinking: bool,
    pub omit_temperature_when_reasoning: bool,
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
            synthesize_missing_reasoning_history: false,
            send_preserve_thinking: false,
            omit_temperature_when_reasoning: false,
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
        if self.mode_control == ThinkingModeControl::AlwaysOn {
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
        synthesize_missing_reasoning_history: false,
        send_preserve_thinking: false,
        omit_temperature_when_reasoning: false,
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
            "grok-4.5" => profile(
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
            ),
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
        && (model.contains("reasoner") || model.contains("r1") || model.contains("v4"))
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
        value.synthesize_missing_reasoning_history = true;
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
}
