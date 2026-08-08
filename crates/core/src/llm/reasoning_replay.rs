use super::reasoning_profile::ReasoningReplayPolicy;
use super::{Message, Role};

pub const LEGACY_MISSING_REASONING_SENTINEL: &str =
    "[reasoning content unavailable in local history]";

const REPLAY_BOUNDARY_NOTE: &str = "## Provider replay boundary\nA legacy assistant/tool replay unit was omitted because its provider reasoning payload was not retained. Continue from the remaining verified conversation history.";

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
        let missing_required_payload = message.role == Role::Assistant
            && message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| !calls.is_empty())
            && message.reasoning_content.is_none();
        if missing_required_payload {
            omitted_units += 1;
            index += 1;
            while index < normalized.len() && normalized[index].role == Role::Tool {
                index += 1;
            }
            continue;
        }
        projected.push(message.clone());
        index += 1;
    }

    if omitted_units > 0 {
        let insertion_index = projected
            .iter()
            .take_while(|message| message.role == Role::System)
            .count();
        projected.insert(
            insertion_index,
            Message::text(Role::System, REPLAY_BOUNDARY_NOTE),
        );
    }

    ReasoningReplayProjection {
        messages: projected,
        omitted_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(projection
            .messages
            .iter()
            .any(|message| message.text_content().contains("Provider replay boundary")));
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
    fn forbidden_policy_strips_reasoning_without_dropping_messages() {
        let projection = prepare_reasoning_replay_history(
            &[tool_call_message(Some("private"))],
            ReasoningReplayPolicy::Forbidden,
        );

        assert_eq!(projection.omitted_units, 0);
        assert_eq!(projection.messages.len(), 1);
        assert!(projection.messages[0].reasoning_content.is_none());
    }
}
