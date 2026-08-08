//! Provider-native assistant turn capture and replay metadata.
//!
//! A provider turn is the durable unit that authorizes tool dispatch.  It
//! records the exact route and opaque replay material before any requested
//! tool is allowed to run.

use serde::{Deserialize, Serialize};

use super::reasoning_profile::{
    ReasoningApiStyle, ReasoningCaptureStatus, ReasoningProfile, ReasoningReplayPolicy,
};
use super::{CompletionRequest, ReasoningEffort, ToolCallRequest};

pub const ANTHROPIC_THINKING_SIGNATURE_PREFIX: &str = "nexa.anthropic.thinking.v1:";
pub const RESPONSES_REASONING_SIGNATURE_PREFIX: &str = "nexa.responses.reasoning.v1:";
pub const GEMINI_THOUGHT_SIGNATURE_PREFIX: &str = "nexa.gemini.thought.v1:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteSnapshot {
    pub provider_endpoint_id: String,
    pub provider_family: String,
    pub api_style: ReasoningApiStyle,
    pub model_id: String,
    pub reasoning_profile_id: String,
    pub reasoning_profile_version: u32,
    pub replay_policy: ReasoningReplayPolicy,
}

impl RouteSnapshot {
    pub fn from_profile(profile: &ReasoningProfile) -> Self {
        Self {
            provider_endpoint_id: profile.key.endpoint_id.clone(),
            provider_family: profile.key.provider_id.clone(),
            api_style: profile.key.api_style,
            model_id: profile.key.model_id.clone(),
            reasoning_profile_id: profile.id.clone(),
            reasoning_profile_version: profile.version,
            replay_policy: profile.replay_policy,
        }
    }

    pub fn from_profile_for_request(
        profile: &ReasoningProfile,
        request: &CompletionRequest,
    ) -> Self {
        let mut snapshot = Self::from_profile(profile);
        if request.reasoning_enabled == Some(false)
            || request.reasoning_effort == Some(ReasoningEffort::None)
        {
            snapshot.replay_policy = ReasoningReplayPolicy::NotRequired;
        }
        snapshot
    }

    pub fn unknown(
        provider_family: impl Into<String>,
        model_id: impl Into<String>,
        replay_policy: ReasoningReplayPolicy,
    ) -> Self {
        let provider_family = provider_family.into();
        Self {
            provider_endpoint_id: format!("{provider_family}-unknown"),
            provider_family,
            api_style: ReasoningApiStyle::Local,
            model_id: model_id.into(),
            reasoning_profile_id: "unknown-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy,
        }
    }

    pub fn api_style_id(&self) -> &'static str {
        match self.api_style {
            ReasoningApiStyle::OpenAiChatCompletions => "openAiChatCompletions",
            ReasoningApiStyle::OpenAiResponses => "openAiResponses",
            ReasoningApiStyle::AnthropicMessages => "anthropicMessages",
            ReasoningApiStyle::GeminiGenerateContent => "geminiGenerateContent",
            ReasoningApiStyle::Local => "local",
        }
    }

    /// Whether two samples were sent through the same provider protocol seam.
    /// Recovery is allowed to turn reasoning off, which changes replay policy
    /// but must never silently switch endpoint, dialect, profile, or model.
    pub fn same_route_identity(&self, other: &Self) -> bool {
        self.provider_endpoint_id == other.provider_endpoint_id
            && self.provider_family == other.provider_family
            && self.api_style == other.api_style
            && self.model_id == other.model_id
            && self.reasoning_profile_id == other.reasoning_profile_id
            && self.reasoning_profile_version == other.reasoning_profile_version
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicThinkingBlock {
    Thinking { thinking: String, signature: String },
    RedactedThinking { data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThoughtSignature {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_part_index: Option<usize>,
    pub signature: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum ProviderReplayPayload {
    DeepSeekReasoningContent(String),
    DeepSeekResponseItems(Vec<serde_json::Value>),
    AnthropicThinkingBlocks(Vec<AnthropicThinkingBlock>),
    OpenAiResponseItems(Vec<serde_json::Value>),
    GeminiThoughtSignatures(Vec<GeminiThoughtSignature>),
    OpenAiCompatibleReasoningContent {
        source_field: String,
        content: String,
    },
    #[default]
    None,
}

impl ProviderReplayPayload {
    pub fn is_present(&self) -> bool {
        match self {
            Self::DeepSeekReasoningContent(content)
            | Self::OpenAiCompatibleReasoningContent { content, .. } => !content.trim().is_empty(),
            Self::AnthropicThinkingBlocks(blocks) => !blocks.is_empty(),
            Self::DeepSeekResponseItems(items) | Self::OpenAiResponseItems(items) => {
                !items.is_empty()
            }
            Self::GeminiThoughtSignatures(signatures) => !signatures.is_empty(),
            Self::None => false,
        }
    }

    pub fn reasoning_content(&self) -> Option<String> {
        match self {
            Self::DeepSeekReasoningContent(content)
            | Self::OpenAiCompatibleReasoningContent { content, .. } => {
                (!content.trim().is_empty()).then(|| content.clone())
            }
            _ => None,
        }
    }

    pub fn capture(
        route: &RouteSnapshot,
        reasoning_content: Option<&str>,
        tool_calls: &[ToolCallRequest],
    ) -> Self {
        let reasoning_content = reasoning_content
            .map(str::trim)
            .filter(|value| !value.is_empty());

        match route.api_style {
            ReasoningApiStyle::AnthropicMessages => tool_calls
                .iter()
                .filter_map(|call| call.thought_signature.as_deref())
                .find_map(decode_anthropic_thinking_blocks)
                .filter(|blocks| !blocks.is_empty())
                .map(Self::AnthropicThinkingBlocks)
                .unwrap_or(Self::None),
            ReasoningApiStyle::OpenAiResponses => tool_calls
                .iter()
                .filter_map(|call| call.thought_signature.as_deref())
                .find_map(decode_responses_reasoning_items)
                .filter(|items| !items.is_empty())
                .map(|items| {
                    if route.provider_family == "deepseek" {
                        Self::DeepSeekResponseItems(items)
                    } else {
                        Self::OpenAiResponseItems(items)
                    }
                })
                .unwrap_or(Self::None),
            ReasoningApiStyle::GeminiGenerateContent => {
                let signatures = tool_calls
                    .iter()
                    .filter_map(|call| {
                        call.thought_signature
                            .as_deref()
                            .map(str::trim)
                            .filter(|signature| !signature.is_empty())
                            .map(|signature| {
                                let mut captured = decode_gemini_thought_signature(signature)
                                    .unwrap_or_else(|| GeminiThoughtSignature {
                                        tool_call_id: call.id.clone(),
                                        model_part_index: None,
                                        signature: signature.to_string(),
                                    });
                                captured.tool_call_id = call.id.clone();
                                captured
                            })
                    })
                    .collect::<Vec<_>>();
                if signatures.is_empty() {
                    Self::None
                } else {
                    Self::GeminiThoughtSignatures(signatures)
                }
            }
            ReasoningApiStyle::OpenAiChatCompletions => {
                let Some(content) = reasoning_content else {
                    return Self::None;
                };
                if route.provider_family == "deepseek" {
                    Self::DeepSeekReasoningContent(content.to_string())
                } else {
                    Self::OpenAiCompatibleReasoningContent {
                        source_field: "reasoning_content".to_string(),
                        content: content.to_string(),
                    }
                }
            }
            ReasoningApiStyle::Local => Self::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum ProviderReplayItem {
    DeepSeekReasoningContent(String),
    DeepSeekResponseItem(serde_json::Value),
    AnthropicThinkingBlock(AnthropicThinkingBlock),
    OpenAiResponseItem(serde_json::Value),
    GeminiThoughtSignature(GeminiThoughtSignature),
    OpenAiCompatibleReasoningContent {
        source_field: String,
        content: String,
    },
}

impl ProviderReplayItem {
    fn from_payload(payload: &ProviderReplayPayload) -> Vec<Self> {
        match payload {
            ProviderReplayPayload::DeepSeekReasoningContent(content) => {
                vec![Self::DeepSeekReasoningContent(content.clone())]
            }
            ProviderReplayPayload::AnthropicThinkingBlocks(blocks) => blocks
                .iter()
                .cloned()
                .map(Self::AnthropicThinkingBlock)
                .collect(),
            ProviderReplayPayload::DeepSeekResponseItems(items) => items
                .iter()
                .cloned()
                .map(Self::DeepSeekResponseItem)
                .collect(),
            ProviderReplayPayload::OpenAiResponseItems(items) => items
                .iter()
                .cloned()
                .map(Self::OpenAiResponseItem)
                .collect(),
            ProviderReplayPayload::GeminiThoughtSignatures(signatures) => signatures
                .iter()
                .cloned()
                .map(Self::GeminiThoughtSignature)
                .collect(),
            ProviderReplayPayload::OpenAiCompatibleReasoningContent {
                source_field,
                content,
            } => vec![Self::OpenAiCompatibleReasoningContent {
                source_field: source_field.clone(),
                content: content.clone(),
            }],
            ProviderReplayPayload::None => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTurnEnvelope {
    pub turn_item_id: String,
    pub sample_id: String,
    pub route: RouteSnapshot,
    pub visible_content: String,
    pub provider_items: Vec<ProviderReplayItem>,
    pub replay_payload: ProviderReplayPayload,
    pub tool_calls: Vec<ToolCallRequest>,
    pub capture_status: ReasoningCaptureStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub raw_response_digest: String,
}

impl ProviderTurnEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        turn_item_id: impl Into<String>,
        sample_id: impl Into<String>,
        route: RouteSnapshot,
        visible_content: impl Into<String>,
        display_reasoning: Option<&str>,
        replay_reasoning: Option<&str>,
        tool_calls: Vec<ToolCallRequest>,
        reasoning_was_requested: bool,
    ) -> Self {
        let visible_content = visible_content.into();
        let replay_payload = ProviderReplayPayload::capture(&route, replay_reasoning, &tool_calls);
        let required_for_replay =
            !tool_calls.is_empty() && route.replay_policy.requires_tool_call_payload();
        let capture_status = if replay_payload.is_present() {
            ReasoningCaptureStatus::Captured
        } else if required_for_replay {
            ReasoningCaptureStatus::OmittedByProvider
        } else if !reasoning_was_requested {
            ReasoningCaptureStatus::NotRequested
        } else if display_reasoning.is_some() {
            ReasoningCaptureStatus::Redacted
        } else {
            ReasoningCaptureStatus::NotRequired
        };
        let provider_items = ProviderReplayItem::from_payload(&replay_payload);
        let digest_input = serde_json::json!({
            "route": &route,
            "visibleContent": &visible_content,
            "providerItems": &provider_items,
            "toolCalls": &tool_calls,
        });
        let raw_response_digest = blake3::hash(
            serde_json::to_vec(&digest_input)
                .unwrap_or_default()
                .as_slice(),
        )
        .to_hex()
        .to_string();

        Self {
            turn_item_id: turn_item_id.into(),
            sample_id: sample_id.into(),
            route,
            visible_content,
            provider_items,
            replay_payload,
            tool_calls,
            capture_status,
            request_id: None,
            response_id: None,
            raw_response_digest,
        }
    }

    pub fn authorizes_tool_dispatch(&self) -> bool {
        self.tool_calls.is_empty()
            || !self.route.replay_policy.requires_tool_call_payload()
            || self.replay_payload.is_present()
    }

    pub fn is_compatible_with(&self, route: &RouteSnapshot) -> bool {
        self.route == *route
    }
}

pub fn encode_anthropic_thinking_blocks(blocks: &[AnthropicThinkingBlock]) -> Option<String> {
    (!blocks.is_empty()).then(|| {
        format!(
            "{ANTHROPIC_THINKING_SIGNATURE_PREFIX}{}",
            serde_json::to_string(blocks).unwrap_or_else(|_| "[]".to_string())
        )
    })
}

pub fn decode_anthropic_thinking_blocks(signature: &str) -> Option<Vec<AnthropicThinkingBlock>> {
    signature
        .strip_prefix(ANTHROPIC_THINKING_SIGNATURE_PREFIX)
        .and_then(|payload| serde_json::from_str(payload).ok())
}

pub fn encode_responses_reasoning_items(items: &[serde_json::Value]) -> Option<String> {
    (!items.is_empty()).then(|| {
        format!(
            "{RESPONSES_REASONING_SIGNATURE_PREFIX}{}",
            serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
        )
    })
}

pub fn decode_responses_reasoning_items(signature: &str) -> Option<Vec<serde_json::Value>> {
    signature
        .strip_prefix(RESPONSES_REASONING_SIGNATURE_PREFIX)
        .and_then(|payload| serde_json::from_str(payload).ok())
}

pub fn encode_gemini_thought_signature(signature: &GeminiThoughtSignature) -> Option<String> {
    (!signature.signature.trim().is_empty()).then(|| {
        format!(
            "{GEMINI_THOUGHT_SIGNATURE_PREFIX}{}",
            serde_json::to_string(signature).unwrap_or_default()
        )
    })
}

pub fn decode_gemini_thought_signature(signature: &str) -> Option<GeminiThoughtSignature> {
    signature
        .strip_prefix(GEMINI_THOUGHT_SIGNATURE_PREFIX)
        .and_then(|payload| serde_json::from_str(payload).ok())
}

pub fn raw_gemini_thought_signature(signature: &str) -> String {
    decode_gemini_thought_signature(signature)
        .map(|captured| captured.signature)
        .unwrap_or_else(|| signature.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(api_style: ReasoningApiStyle, family: &str) -> RouteSnapshot {
        RouteSnapshot {
            provider_endpoint_id: format!("{family}-public"),
            provider_family: family.to_string(),
            api_style,
            model_id: "model".to_string(),
            reasoning_profile_id: "profile-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy: ReasoningReplayPolicy::OpaqueSignature,
        }
    }

    #[test]
    fn captures_each_provider_native_payload_shape() {
        let deepseek = ProviderReplayPayload::capture(
            &RouteSnapshot {
                replay_policy: ReasoningReplayPolicy::RequiredOnToolCall,
                ..route(ReasoningApiStyle::OpenAiChatCompletions, "deepseek")
            },
            Some("reasoning"),
            &[],
        );
        assert!(matches!(
            deepseek,
            ProviderReplayPayload::DeepSeekReasoningContent(_)
        ));

        let anthropic_blocks = vec![
            AnthropicThinkingBlock::Thinking {
                thinking: "private".to_string(),
                signature: "signed".to_string(),
            },
            AnthropicThinkingBlock::RedactedThinking {
                data: "opaque-redacted".to_string(),
            },
        ];
        let anthropic_call = ToolCallRequest {
            id: "call-a".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: encode_anthropic_thinking_blocks(&anthropic_blocks),
        };
        assert_eq!(
            ProviderReplayPayload::capture(
                &route(ReasoningApiStyle::AnthropicMessages, "anthropic"),
                Some("private"),
                &[anthropic_call],
            ),
            ProviderReplayPayload::AnthropicThinkingBlocks(anthropic_blocks)
        );

        let response_items = vec![serde_json::json!({
            "type": "reasoning",
            "encrypted_content": "opaque"
        })];
        let response_call = ToolCallRequest {
            id: "call-o".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: encode_responses_reasoning_items(&response_items),
        };
        assert_eq!(
            ProviderReplayPayload::capture(
                &route(ReasoningApiStyle::OpenAiResponses, "openai"),
                None,
                &[response_call],
            ),
            ProviderReplayPayload::OpenAiResponseItems(response_items)
        );

        let gemini_call = ToolCallRequest {
            id: "call-g".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("thought-signature".to_string()),
        };
        assert!(matches!(
            ProviderReplayPayload::capture(
                &route(ReasoningApiStyle::GeminiGenerateContent, "google"),
                None,
                &[gemini_call],
            ),
            ProviderReplayPayload::GeminiThoughtSignatures(_)
        ));
    }

    #[test]
    fn missing_opaque_payload_never_authorizes_tools() {
        let envelope = ProviderTurnEnvelope::capture(
            "item",
            "sample",
            route(ReasoningApiStyle::AnthropicMessages, "anthropic"),
            "",
            None,
            None,
            vec![ToolCallRequest {
                id: "call".to_string(),
                name: "side_effect".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            }],
            true,
        );
        assert_eq!(
            envelope.capture_status,
            ReasoningCaptureStatus::OmittedByProvider
        );
        assert!(!envelope.authorizes_tool_dispatch());
    }
}
