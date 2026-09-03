use super::provider_turn::RouteSnapshot;
use super::reasoning_profile::ReasoningReplayPolicy;
use super::{Message, Role};

pub const LEGACY_MISSING_REASONING_SENTINEL: &str =
    "[reasoning content unavailable in local history]";

const MAX_BOUNDARY_FIELD_CHARS: usize = 1_500;

#[derive(Debug, Clone)]
pub struct ReasoningReplayProjection {
    pub messages: Vec<Message>,
    pub omitted_units: usize,
}

pub fn sanitize_reasoning_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != LEGACY_MISSING_REASONING_SENTINEL)
        .map(str::to_string)
}

fn bounded_visible_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_BOUNDARY_FIELD_CHARS {
        return trimmed.to_string();
    }
    let mut bounded = trimmed
        .chars()
        .take(MAX_BOUNDARY_FIELD_CHARS)
        .collect::<String>();
    bounded.push_str("… [truncated at replay boundary]");
    bounded
}

/// Keep only the user-visible conclusion from a provider-incompatible tool
/// unit. Tool names, tool receipts, and replay diagnostics are control-plane
/// data; projecting them as an assistant-authored summary teaches the next
/// model to repeat Nexa internals in its visible answer.
fn compact_visible_conclusion(unit: &[Message]) -> Option<Message> {
    unit.iter()
        .rev()
        .find(|message| {
            message.role == Role::Assistant
                && message
                    .tool_calls
                    .as_ref()
                    .is_none_or(|calls| calls.is_empty())
                && !message.text_content().trim().is_empty()
        })
        .map(|message| {
            Message::text(
                Role::Assistant,
                bounded_visible_text(&message.text_content()),
            )
        })
}

/// Return the exclusive end of one assistant/tool replay chain.
///
/// A chain starts with an assistant tool call and includes every following
/// tool result, subsequent assistant tool call, and the final assistant
/// response. Keeping this boundary shared prevents trimming and replay repair
/// from retaining only a dependent suffix of a multi-tool turn.
pub(crate) fn atomic_tool_replay_unit_end(messages: &[Message], start: usize) -> usize {
    debug_assert!(
        messages.get(start).is_some_and(|message| {
            message.role == Role::Assistant
                && message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        }),
        "atomic replay units must start at an assistant tool call"
    );

    let mut end = start + 1;
    loop {
        while end < messages.len() && messages[end].role == Role::Tool {
            end += 1;
        }
        let Some(assistant) = messages
            .get(end)
            .filter(|message| message.role == Role::Assistant)
        else {
            break;
        };
        end += 1;
        if assistant
            .tool_calls
            .as_ref()
            .is_none_or(|calls| calls.is_empty())
        {
            break;
        }
    }
    end
}

pub fn prepare_reasoning_replay_history(
    messages: &[Message],
    policy: ReasoningReplayPolicy,
) -> ReasoningReplayProjection {
    let mut normalized = messages.to_vec();
    for message in &mut normalized {
        message.reasoning_content = sanitize_reasoning_text(message.reasoning_content.as_deref());
        if policy == ReasoningReplayPolicy::Forbidden {
            message.reasoning_content = None;
        }
    }

    if !policy.requires_tool_call_payload() {
        return ReasoningReplayProjection {
            messages: normalized,
            omitted_units: 0,
        };
    }

    let mut projected = Vec::with_capacity(normalized.len());
    let mut omitted_units = 0;
    let mut index = 0;
    while index < normalized.len() {
        let message = &normalized[index];
        let starts_tool_unit = message.role == Role::Assistant
            && message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| !calls.is_empty());
        if starts_tool_unit {
            let end = atomic_tool_replay_unit_end(&normalized, index);
            let missing_required_payload = normalized[index..end].iter().any(|message| {
                message.role == Role::Assistant
                    && message
                        .tool_calls
                        .as_ref()
                        .is_some_and(|calls| !calls.is_empty())
                    && message.reasoning_content.is_none()
            });
            if missing_required_payload {
                omitted_units += 1;
                if let Some(conclusion) = compact_visible_conclusion(&normalized[index..end]) {
                    projected.push(conclusion);
                }
                index = end;
                continue;
            }
            projected.extend_from_slice(&normalized[index..end]);
            index = end;
            continue;
        }
        projected.push(message.clone());
        index += 1;
    }

    ReasoningReplayProjection {
        messages: projected,
        omitted_units,
    }
}

/// Project history for one concrete provider route. Required replay payloads
/// must come from a compatible typed envelope; ambiguous legacy strings form
/// a boundary instead of being reinterpreted as another provider dialect.
pub fn prepare_provider_replay_history(
    messages: &[Message],
    route: &RouteSnapshot,
) -> ReasoningReplayProjection {
    let required_payload = route.replay_policy.requires_tool_call_payload();
    if !required_payload && route.replay_policy != ReasoningReplayPolicy::NotRequired {
        return prepare_reasoning_replay_history(messages, route.replay_policy);
    }

    let mut normalized = messages.to_vec();
    for message in &mut normalized {
        message.reasoning_content = sanitize_reasoning_text(message.reasoning_content.as_deref());
        if let Some(envelope) = message.provider_turn() {
            message.reasoning_content = envelope.replay_payload.reasoning_content();
        }
    }

    let mut projected = Vec::with_capacity(normalized.len());
    let mut omitted_units = 0;
    let mut index = 0;
    while index < normalized.len() {
        let message = &normalized[index];
        let starts_tool_unit = message.role == Role::Assistant
            && message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| !calls.is_empty());
        if starts_tool_unit {
            let end = atomic_tool_replay_unit_end(&normalized, index);
            let invalid_envelope = normalized[index..end].iter().any(|message| {
                if message.role != Role::Assistant
                    || message
                        .tool_calls
                        .as_ref()
                        .is_none_or(|calls| calls.is_empty())
                {
                    return false;
                }
                if required_payload {
                    message.provider_turn().is_none_or(|envelope| {
                        !envelope.is_compatible_with(route) || !envelope.authorizes_tool_dispatch()
                    })
                } else {
                    message.provider_turn().is_some_and(|envelope| {
                        !matches!(
                            &envelope.replay_payload,
                            super::provider_turn::ProviderReplayPayload::None
                        ) && (!envelope.is_compatible_with(route)
                            || !envelope.authorizes_tool_dispatch())
                    })
                }
            });
            if invalid_envelope {
                omitted_units += 1;
                if let Some(conclusion) = compact_visible_conclusion(&normalized[index..end]) {
                    projected.push(conclusion);
                }
                index = end;
                continue;
            }
            projected.extend_from_slice(&normalized[index..end]);
            index = end;
            continue;
        }
        projected.push(message.clone());
        index += 1;
    }

    ReasoningReplayProjection {
        messages: projected,
        omitted_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider_turn::{ProviderTurnEnvelope, RouteSnapshot};
    use crate::llm::reasoning_profile::ReasoningApiStyle;
    use crate::llm::ToolCallRequest;

    fn tool_call_message(reasoning: Option<&str>) -> Message {
        let mut message = Message::text(Role::Assistant, "");
        message.reasoning_content = reasoning.map(str::to_string);
        message.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        message
    }

    #[test]
    fn required_tool_reasoning_omits_the_whole_legacy_replay_unit() {
        let mut tool = Message::text(Role::Tool, "result");
        tool.name = Some("call-1".to_string());
        let projection = prepare_reasoning_replay_history(
            &[
                Message::text(Role::System, "policy"),
                Message::text(Role::User, "old question"),
                tool_call_message(Some(LEGACY_MISSING_REASONING_SENTINEL)),
                tool,
                Message::text(Role::Assistant, "old answer"),
                Message::text(Role::User, "new question"),
            ],
            ReasoningReplayPolicy::RequiredOnToolCall,
        );

        assert_eq!(projection.omitted_units, 1);
        assert!(projection.messages.iter().any(|message| {
            message.role == Role::Assistant && message.text_content() == "old answer"
        }));
        assert!(!projection.messages.iter().any(|message| {
            message.role == Role::Tool
                || message
                    .reasoning_content
                    .as_deref()
                    .is_some_and(|value| value == LEGACY_MISSING_REASONING_SENTINEL)
        }));
        assert_eq!(
            projection
                .messages
                .last()
                .map(Message::text_content)
                .as_deref(),
            Some("new question")
        );
    }

    #[test]
    fn real_reasoning_keeps_the_atomic_tool_replay_unit() {
        let mut tool = Message::text(Role::Tool, "result");
        tool.name = Some("call-1".to_string());
        let projection = prepare_reasoning_replay_history(
            &[tool_call_message(Some("captured")), tool],
            ReasoningReplayPolicy::RequiredOnToolCall,
        );

        assert_eq!(projection.omitted_units, 0);
        assert_eq!(projection.messages.len(), 2);
        assert_eq!(
            projection.messages[0].reasoning_content.as_deref(),
            Some("captured")
        );
    }

    #[test]
    fn missing_payload_drops_the_entire_multi_tool_chain() {
        let first_tool = Message::text_with_name(Role::Tool, "first result", "call-1");
        let second_tool = Message::text_with_name(Role::Tool, "second result", "call-1");
        let projection = prepare_reasoning_replay_history(
            &[
                Message::text(Role::User, "old question"),
                tool_call_message(Some("captured first step")),
                first_tool,
                tool_call_message(None),
                second_tool,
                Message::text(Role::Assistant, "dependent final answer"),
                Message::text(Role::User, "new question"),
            ],
            ReasoningReplayPolicy::RequiredOnToolCall,
        );

        assert_eq!(projection.omitted_units, 1);
        assert!(!projection
            .messages
            .iter()
            .any(|message| message.role == Role::Tool));
        assert_eq!(
            projection
                .messages
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "old question".to_string(),
                "dependent final answer".to_string(),
                "new question".to_string(),
            ]
        );
        assert!(projection.messages.iter().all(|message| {
            let text = message.text_content();
            !text.contains("Verified legacy visible-history summary")
                && !text.contains("Provider replay boundary")
                && !text.contains("first result")
                && !text.contains("second result")
        }));
    }

    #[test]
    fn forbidden_policy_strips_reasoning_without_dropping_messages() {
        let projection = prepare_reasoning_replay_history(
            &[tool_call_message(Some("private"))],
            ReasoningReplayPolicy::Forbidden,
        );

        assert_eq!(projection.omitted_units, 0);
        assert_eq!(projection.messages.len(), 1);
        assert!(projection.messages[0].reasoning_content.is_none());
    }

    fn deepseek_route(model: &str) -> RouteSnapshot {
        RouteSnapshot {
            provider_endpoint_id: "deepseek-public".to_string(),
            provider_family: "deepseek".to_string(),
            api_style: ReasoningApiStyle::OpenAiChatCompletions,
            model_id: model.to_string(),
            reasoning_profile_id: "deepseek-chat-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy: ReasoningReplayPolicy::RequiredOnToolCall,
        }
    }

    fn typed_tool_call_message(route: RouteSnapshot) -> Message {
        let mut message = tool_call_message(Some("legacy display copy"));
        let envelope = ProviderTurnEnvelope::capture(
            "turn-item",
            "sample",
            route,
            "",
            Some("display reasoning"),
            Some("native replay reasoning"),
            message.tool_calls.clone().unwrap_or_default(),
            true,
        );
        message.set_provider_turn(envelope);
        message
    }

    #[test]
    fn typed_envelope_replays_only_on_the_exact_route() {
        let route = deepseek_route("deepseek-reasoner");
        let tool = Message::text_with_name(Role::Tool, "result", "call-1");
        let projection = prepare_provider_replay_history(
            &[typed_tool_call_message(route.clone()), tool],
            &route,
        );

        assert_eq!(projection.omitted_units, 0);
        assert_eq!(
            projection.messages[0].reasoning_content.as_deref(),
            Some("native replay reasoning")
        );
    }

    #[test]
    fn route_mismatch_omits_the_entire_typed_tool_unit() {
        let captured_route = deepseek_route("deepseek-reasoner");
        let requested_route = deepseek_route("deepseek-chat");
        let tool = Message::text_with_name(Role::Tool, "result", "call-1");
        let projection = prepare_provider_replay_history(
            &[typed_tool_call_message(captured_route), tool],
            &requested_route,
        );

        assert_eq!(projection.omitted_units, 1);
        assert!(projection.messages.iter().all(|message| {
            message.role != Role::Tool
                && !message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        }));
    }

    #[test]
    fn optional_but_invalid_provider_payload_crosses_a_replay_boundary() {
        let route = RouteSnapshot {
            provider_endpoint_id: "google-public".to_string(),
            provider_family: "google".to_string(),
            api_style: ReasoningApiStyle::GeminiGenerateContent,
            model_id: "gemini-2.5-flash".to_string(),
            reasoning_profile_id: "gemini-thought-signature-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy: ReasoningReplayPolicy::NotRequired,
        };
        let payload = crate::llm::provider_turn::GeminiThoughtSignatureSet {
            signatures: vec![crate::llm::provider_turn::GeminiThoughtSignature {
                tool_call_id: "call-1".to_string(),
                model_part_index: Some(1),
                signature: "moved".to_string(),
            }],
            content_parts: vec![serde_json::json!({
                "functionCall": {"id": "call-1", "name": "lookup", "args": {}},
                "thoughtSignature": "moved"
            })],
        };
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: crate::llm::provider_turn::encode_gemini_thought_signatures(
                &payload,
            ),
        }]);
        assistant.set_provider_turn(ProviderTurnEnvelope::capture(
            "optional-item",
            "optional-sample",
            route.clone(),
            "",
            None,
            None,
            assistant.tool_calls.clone().unwrap_or_default(),
            false,
        ));

        let projection = prepare_provider_replay_history(
            &[
                assistant,
                Message::text_with_name(Role::Tool, "result", "call-1"),
            ],
            &route,
        );

        assert_eq!(projection.omitted_units, 1);
        assert!(projection
            .messages
            .iter()
            .all(|message| message.role != Role::Tool));
    }
}
