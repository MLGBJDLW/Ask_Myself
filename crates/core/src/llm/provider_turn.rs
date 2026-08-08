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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThoughtSignatureSet {
    pub signatures: Vec<GeminiThoughtSignature>,
    /// Exact ordered provider-native `Content.parts` for this model turn.
    pub content_parts: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesReplayPayload {
    pub response_status: String,
    pub items: Vec<serde_json::Value>,
}

impl ResponsesReplayPayload {
    fn completed_item(item: &serde_json::Value) -> bool {
        item.get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
            && item.get("status").and_then(serde_json::Value::as_str) == Some("completed")
    }

    fn is_structurally_complete(&self, encrypted_reasoning_required: bool) -> bool {
        if self.response_status != "completed" || self.items.is_empty() {
            return false;
        }
        let mut saw_reasoning = false;
        let mut saw_function_call = false;
        for item in &self.items {
            match item.get("type").and_then(serde_json::Value::as_str) {
                Some("reasoning") => {
                    let has_state = if encrypted_reasoning_required {
                        item.get("encrypted_content")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|content| !content.trim().is_empty())
                    } else {
                        item.get("content")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|content| !content.is_empty())
                    };
                    if !Self::completed_item(item) || !has_state {
                        return false;
                    }
                    saw_reasoning = true;
                }
                Some("web_search_call") => {
                    if !Self::completed_item(item) {
                        return false;
                    }
                }
                Some("message") => {
                    if !Self::completed_item(item)
                        || !item
                            .get("content")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|content| !content.is_empty())
                    {
                        return false;
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|call_id| !call_id.trim().is_empty());
                    let name = item
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| !name.trim().is_empty());
                    let arguments = item
                        .get("arguments")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|arguments| {
                            serde_json::from_str::<serde_json::Value>(arguments).ok()
                        })
                        .is_some_and(|arguments| arguments.is_object());
                    if !saw_reasoning
                        || !Self::completed_item(item)
                        || !call_id
                        || !name
                        || !arguments
                    {
                        return false;
                    }
                    saw_function_call = true;
                }
                _ => return false,
            }
        }
        saw_reasoning && saw_function_call
    }

    fn authorizes_tool_calls(
        &self,
        tool_calls: &[ToolCallRequest],
        encrypted_reasoning_required: bool,
    ) -> bool {
        if !self.is_structurally_complete(encrypted_reasoning_required) {
            return false;
        }
        let provider_calls = self
            .items
            .iter()
            .filter(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
            })
            .collect::<Vec<_>>();
        provider_calls.len() == tool_calls.len()
            && provider_calls
                .iter()
                .zip(tool_calls)
                .all(|(provider_call, tool_call)| {
                    provider_call
                        .get("call_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(tool_call.id.as_str())
                        && provider_call
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            == Some(tool_call.name.as_str())
                        && provider_call
                            .get("arguments")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|arguments| {
                                serde_json::from_str::<serde_json::Value>(arguments).ok()
                            })
                            == serde_json::from_str::<serde_json::Value>(&tool_call.arguments).ok()
                })
    }
}

impl GeminiThoughtSignatureSet {
    fn has_valid_signature_positions(&self) -> bool {
        let signed_parts = self
            .content_parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                part.get("thoughtSignature")
                    .and_then(serde_json::Value::as_str)
                    .filter(|signature| !signature.trim().is_empty())
                    .map(|signature| (index, signature))
            })
            .collect::<Vec<_>>();
        !self.content_parts.is_empty()
            && signed_parts.len() == self.signatures.len()
            && signed_parts.iter().all(|(index, signature)| {
                self.signatures.iter().any(|captured| {
                    captured.model_part_index == Some(*index) && captured.signature == *signature
                })
            })
    }

    fn authorizes_tool_calls(
        &self,
        tool_calls: &[ToolCallRequest],
        require_first_function_signature: bool,
    ) -> bool {
        if !self.has_valid_signature_positions() {
            return false;
        }
        let provider_calls = self
            .content_parts
            .iter()
            .filter_map(|part| part.get("functionCall"))
            .collect::<Vec<_>>();
        if require_first_function_signature {
            let signed_function_calls = self
                .content_parts
                .iter()
                .filter(|part| part.get("functionCall").is_some())
                .map(|part| {
                    part.get("thoughtSignature")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|signature| !signature.trim().is_empty())
                })
                .collect::<Vec<_>>();
            if signed_function_calls.first() != Some(&true)
                || signed_function_calls.iter().skip(1).any(|signed| *signed)
            {
                return false;
            }
        }
        provider_calls.len() == tool_calls.len()
            && provider_calls
                .iter()
                .zip(tool_calls)
                .all(|(provider_call, tool_call)| {
                    provider_call
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        == Some(tool_call.name.as_str())
                        && provider_call.get("args")
                            == serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                                .ok()
                                .as_ref()
                        && provider_call
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|provider_id| provider_id == tool_call.id)
                })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum ProviderReplayPayload {
    DeepSeekReasoningContent(String),
    DeepSeekResponseItems(ResponsesReplayPayload),
    AnthropicThinkingBlocks(Vec<AnthropicThinkingBlock>),
    OpenAiResponseItems(ResponsesReplayPayload),
    GeminiThoughtSignatures(GeminiThoughtSignatureSet),
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
            Self::DeepSeekResponseItems(payload) => payload.is_structurally_complete(false),
            Self::OpenAiResponseItems(payload) => payload.is_structurally_complete(true),
            Self::GeminiThoughtSignatures(payload) => {
                !payload.signatures.is_empty() && payload.has_valid_signature_positions()
            }
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

    fn authorizes_tool_calls(&self, route: &RouteSnapshot, tool_calls: &[ToolCallRequest]) -> bool {
        match self {
            Self::DeepSeekResponseItems(payload) => {
                payload.authorizes_tool_calls(tool_calls, false)
            }
            Self::OpenAiResponseItems(payload) => payload.authorizes_tool_calls(tool_calls, true),
            Self::GeminiThoughtSignatures(payload) => payload.authorizes_tool_calls(
                tool_calls,
                route
                    .model_id
                    .strip_prefix("models/")
                    .unwrap_or(&route.model_id)
                    .to_ascii_lowercase()
                    .starts_with("gemini-3"),
            ),
            _ => self.is_present(),
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
                .filter(|payload| !payload.items.is_empty())
                .map(|payload| {
                    if route.provider_family == "deepseek" {
                        Self::DeepSeekResponseItems(payload)
                    } else {
                        Self::OpenAiResponseItems(payload)
                    }
                })
                .unwrap_or(Self::None),
            ReasoningApiStyle::GeminiGenerateContent => {
                if let Some(payload) = tool_calls
                    .iter()
                    .filter_map(|call| call.thought_signature.as_deref())
                    .find_map(decode_gemini_thought_signatures)
                {
                    return Self::GeminiThoughtSignatures(payload);
                }
                let signatures = tool_calls
                    .iter()
                    .filter_map(|call| {
                        call.thought_signature
                            .as_deref()
                            .map(str::trim)
                            .filter(|signature| !signature.is_empty())
                            .map(|signature| GeminiThoughtSignature {
                                tool_call_id: call.id.clone(),
                                model_part_index: None,
                                signature: signature.to_string(),
                            })
                    })
                    .collect::<Vec<_>>();
                if signatures.is_empty() {
                    Self::None
                } else {
                    // Legacy signatures did not retain their ordered provider
                    // parts and therefore cannot authorize required replay.
                    Self::GeminiThoughtSignatures(GeminiThoughtSignatureSet {
                        signatures,
                        content_parts: Vec::new(),
                    })
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
    GeminiContentPart(serde_json::Value),
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
            ProviderReplayPayload::DeepSeekResponseItems(payload) => payload
                .items
                .iter()
                .cloned()
                .map(Self::DeepSeekResponseItem)
                .collect(),
            ProviderReplayPayload::OpenAiResponseItems(payload) => payload
                .items
                .iter()
                .cloned()
                .map(Self::OpenAiResponseItem)
                .collect(),
            ProviderReplayPayload::GeminiThoughtSignatures(payload) => payload
                .content_parts
                .iter()
                .cloned()
                .map(Self::GeminiContentPart)
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
        if self.tool_calls.is_empty() {
            return true;
        }
        let payload_supplied = !matches!(self.replay_payload, ProviderReplayPayload::None);
        let payload_valid = self
            .replay_payload
            .authorizes_tool_calls(&self.route, &self.tool_calls);
        if self.route.replay_policy == ReasoningReplayPolicy::NotRequired && payload_supplied {
            return payload_valid;
        }
        self.route.replay_policy.authorizes_tool_call(payload_valid)
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

pub fn encode_responses_reasoning_items(payload: &ResponsesReplayPayload) -> Option<String> {
    (!payload.items.is_empty()).then(|| {
        format!(
            "{RESPONSES_REASONING_SIGNATURE_PREFIX}{}",
            serde_json::to_string(payload).unwrap_or_default()
        )
    })
}

pub fn decode_responses_reasoning_items(signature: &str) -> Option<ResponsesReplayPayload> {
    signature
        .strip_prefix(RESPONSES_REASONING_SIGNATURE_PREFIX)
        .and_then(|payload| {
            serde_json::from_str(payload).ok().or_else(|| {
                serde_json::from_str::<Vec<serde_json::Value>>(payload)
                    .ok()
                    .map(|items| ResponsesReplayPayload {
                        response_status: "legacy_unknown".to_string(),
                        items,
                    })
            })
        })
}

pub fn encode_gemini_thought_signatures(payload: &GeminiThoughtSignatureSet) -> Option<String> {
    (!payload.content_parts.is_empty()).then(|| {
        format!(
            "{GEMINI_THOUGHT_SIGNATURE_PREFIX}{}",
            serde_json::to_string(payload).unwrap_or_default()
        )
    })
}

pub fn decode_gemini_thought_signatures(signature: &str) -> Option<GeminiThoughtSignatureSet> {
    signature
        .strip_prefix(GEMINI_THOUGHT_SIGNATURE_PREFIX)
        .and_then(|payload| serde_json::from_str(payload).ok())
}

pub fn raw_gemini_thought_signature(signature: &str) -> String {
    decode_gemini_thought_signatures(signature)
        .and_then(|captured| captured.signatures.into_iter().next())
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

        let response_payload = ResponsesReplayPayload {
            response_status: "completed".to_string(),
            items: vec![
                serde_json::json!({
                    "type": "reasoning",
                    "id": "rs-o",
                    "status": "completed",
                    "encrypted_content": "opaque"
                }),
                serde_json::json!({
                    "type": "function_call",
                    "id": "fc-o",
                    "status": "completed",
                    "call_id": "call-o",
                    "name": "lookup",
                    "arguments": "{}"
                }),
            ],
        };
        let response_call = ToolCallRequest {
            id: "call-o".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: encode_responses_reasoning_items(&response_payload),
        };
        assert_eq!(
            ProviderReplayPayload::capture(
                &route(ReasoningApiStyle::OpenAiResponses, "openai"),
                None,
                &[response_call],
            ),
            ProviderReplayPayload::OpenAiResponseItems(response_payload)
        );

        let gemini_payload = GeminiThoughtSignatureSet {
            signatures: vec![GeminiThoughtSignature {
                tool_call_id: "call-g".to_string(),
                model_part_index: Some(1),
                signature: "thought-signature".to_string(),
            }],
            content_parts: vec![
                serde_json::json!({"text": "working", "thought": true}),
                serde_json::json!({
                    "functionCall": {"id": "call-g", "name": "lookup", "args": {}},
                    "thoughtSignature": "thought-signature"
                }),
            ],
        };
        let gemini_call = ToolCallRequest {
            id: "call-g".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: encode_gemini_thought_signatures(&gemini_payload),
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

        let unknown = ProviderTurnEnvelope::capture(
            "unknown-item",
            "unknown-sample",
            RouteSnapshot {
                replay_policy: ReasoningReplayPolicy::Unknown,
                ..route(ReasoningApiStyle::OpenAiChatCompletions, "custom")
            },
            "",
            None,
            None,
            vec![ToolCallRequest {
                id: "unknown-call".to_string(),
                name: "side_effect".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            }],
            false,
        );
        assert!(!unknown.authorizes_tool_dispatch());
    }

    #[test]
    fn moved_or_mismatched_gemini_parts_never_authorize_tools() {
        let payload = GeminiThoughtSignatureSet {
            signatures: vec![GeminiThoughtSignature {
                tool_call_id: "call-g".to_string(),
                model_part_index: Some(0),
                signature: "opaque".to_string(),
            }],
            content_parts: vec![serde_json::json!({
                "functionCall": {"id": "call-g", "name": "write_file", "args": {"path": "a"}},
                "thoughtSignature": "opaque"
            })],
        };
        let tool_call = ToolCallRequest {
            id: "call-g".to_string(),
            name: "write_file".to_string(),
            arguments: r#"{"path":"a"}"#.to_string(),
            thought_signature: encode_gemini_thought_signatures(&payload),
        };
        let envelope = ProviderTurnEnvelope::capture(
            "gemini-item",
            "gemini-sample",
            route(ReasoningApiStyle::GeminiGenerateContent, "google"),
            "",
            None,
            None,
            vec![tool_call],
            true,
        );
        assert!(envelope.authorizes_tool_dispatch());

        let ProviderReplayPayload::GeminiThoughtSignatures(mut moved) =
            envelope.replay_payload.clone()
        else {
            panic!("Gemini replay payload");
        };
        moved.signatures[0].model_part_index = Some(1);
        let mut moved_envelope = envelope.clone();
        moved_envelope.replay_payload = ProviderReplayPayload::GeminiThoughtSignatures(moved);
        assert!(!moved_envelope.authorizes_tool_dispatch());

        let ProviderReplayPayload::GeminiThoughtSignatures(mut mismatched) =
            envelope.replay_payload.clone()
        else {
            panic!("Gemini replay payload");
        };
        mismatched.content_parts[0]["functionCall"]["name"] =
            serde_json::Value::String("different_tool".to_string());
        let mut mismatched_envelope = envelope;
        mismatched_envelope.replay_payload =
            ProviderReplayPayload::GeminiThoughtSignatures(mismatched);
        assert!(!mismatched_envelope.authorizes_tool_dispatch());

        let mut optional_route = route(ReasoningApiStyle::GeminiGenerateContent, "google");
        optional_route.model_id = "gemini-2.5-flash".to_string();
        optional_route.replay_policy = ReasoningReplayPolicy::NotRequired;
        moved_envelope.route = optional_route;
        assert!(!moved_envelope.authorizes_tool_dispatch());
    }

    #[test]
    fn gemini_three_parallel_calls_reject_signatures_after_the_first_call() {
        let payload = GeminiThoughtSignatureSet {
            signatures: vec![
                GeminiThoughtSignature {
                    tool_call_id: "call-1".to_string(),
                    model_part_index: Some(0),
                    signature: "first".to_string(),
                },
                GeminiThoughtSignature {
                    tool_call_id: "call-2".to_string(),
                    model_part_index: Some(1),
                    signature: "second".to_string(),
                },
            ],
            content_parts: vec![
                serde_json::json!({
                    "functionCall": {"id": "call-1", "name": "read_file", "args": {}},
                    "thoughtSignature": "first"
                }),
                serde_json::json!({
                    "functionCall": {"id": "call-2", "name": "list_files", "args": {}},
                    "thoughtSignature": "second"
                }),
            ],
        };
        let mut route = route(ReasoningApiStyle::GeminiGenerateContent, "google");
        route.model_id = "gemini-3-flash".to_string();
        let envelope = ProviderTurnEnvelope::capture(
            "parallel-item",
            "parallel-sample",
            route,
            "",
            None,
            None,
            vec![
                ToolCallRequest {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                    thought_signature: encode_gemini_thought_signatures(&payload),
                },
                ToolCallRequest {
                    id: "call-2".to_string(),
                    name: "list_files".to_string(),
                    arguments: "{}".to_string(),
                    thought_signature: None,
                },
            ],
            true,
        );

        assert!(!envelope.authorizes_tool_dispatch());
    }

    #[test]
    fn incomplete_or_reasoning_missing_responses_payload_never_authorizes_tools() {
        let payload = ResponsesReplayPayload {
            response_status: "completed".to_string(),
            items: vec![
                serde_json::json!({
                    "type": "reasoning",
                    "id": "rs-1",
                    "status": "completed",
                    "encrypted_content": "opaque"
                }),
                serde_json::json!({
                    "type": "function_call",
                    "id": "fc-1",
                    "status": "completed",
                    "call_id": "call-1",
                    "name": "write_file",
                    "arguments": "{\"path\":\"a\"}"
                }),
            ],
        };
        let tool_call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "write_file".to_string(),
            arguments: r#"{"path":"a"}"#.to_string(),
            thought_signature: encode_responses_reasoning_items(&payload),
        };
        let envelope = ProviderTurnEnvelope::capture(
            "responses-item",
            "responses-sample",
            RouteSnapshot {
                replay_policy: ReasoningReplayPolicy::RequiredOnToolCall,
                ..route(ReasoningApiStyle::OpenAiResponses, "openai")
            },
            "",
            None,
            None,
            vec![tool_call],
            true,
        );
        assert!(envelope.authorizes_tool_dispatch());

        for mutate in [
            |payload: &mut ResponsesReplayPayload| {
                payload.response_status = "incomplete".to_string();
            },
            |payload: &mut ResponsesReplayPayload| {
                payload.items[0]
                    .as_object_mut()
                    .unwrap()
                    .remove("encrypted_content");
            },
            |payload: &mut ResponsesReplayPayload| {
                payload.items[1]["status"] = serde_json::Value::String("in_progress".to_string());
            },
        ] {
            let ProviderReplayPayload::OpenAiResponseItems(mut invalid) =
                envelope.replay_payload.clone()
            else {
                panic!("OpenAI Responses payload");
            };
            mutate(&mut invalid);
            let mut rejected = envelope.clone();
            rejected.replay_payload = ProviderReplayPayload::OpenAiResponseItems(invalid);
            assert!(!rejected.authorizes_tool_dispatch());
        }
    }
}
