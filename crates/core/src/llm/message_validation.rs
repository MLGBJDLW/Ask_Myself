//! Provider-neutral message canonicalization and sequence validation.
//!
//! Provider adapters must never serialize an assistant message that contains
//! neither visible content nor a complete tool call. Historical and recovery
//! paths may drop legacy-invalid records, while live/provider boundaries fail
//! closed with privacy-safe diagnostics.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ContentPart, Message, Role, ToolCallRequest};
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    Persisted,
    Stream,
    Recovery,
    SubagentHandoff,
    ProviderBoundary,
}

impl fmt::Display for MessageSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Persisted => "persisted",
            Self::Stream => "stream",
            Self::Recovery => "recovery",
            Self::SubagentHandoff => "subagent_handoff",
            Self::ProviderBoundary => "provider_boundary",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidAssistantHandling {
    Drop,
    Reject,
}

#[derive(Debug, Clone)]
pub struct MessageNormalizationContext<'a> {
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub message_index: usize,
    pub source: MessageSource,
    pub invalid_assistant: InvalidAssistantHandling,
}

impl<'a> MessageNormalizationContext<'a> {
    pub fn provider_boundary(provider: &'a str, model: &'a str, message_index: usize) -> Self {
        Self {
            provider: Some(provider),
            model: Some(model),
            conversation_id: None,
            turn_id: None,
            message_index,
            source: MessageSource::ProviderBoundary,
            invalid_assistant: InvalidAssistantHandling::Reject,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageValidationDiagnostic {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
    pub message_index: usize,
    pub role: String,
    pub has_content: bool,
    pub tool_call_count: usize,
    pub source: MessageSource,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct MessageValidationError {
    pub diagnostic: Box<MessageValidationDiagnostic>,
}

impl fmt::Display for MessageValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostic = &self.diagnostic;
        write!(
            formatter,
            "invalid {} message at index {} (source={}, provider={}, model={}, conversation_id={}, turn_id={}, has_content={}, tool_call_count={}): {}",
            diagnostic.role,
            diagnostic.message_index,
            diagnostic.source,
            diagnostic.provider.as_deref().unwrap_or("unknown"),
            diagnostic.model.as_deref().unwrap_or("unknown"),
            diagnostic.conversation_id.as_deref().unwrap_or("unknown"),
            diagnostic.turn_id.as_deref().unwrap_or("unknown"),
            diagnostic.has_content,
            diagnostic.tool_call_count,
            diagnostic.reason,
        )
    }
}

impl std::error::Error for MessageValidationError {}

#[derive(Debug, Clone)]
pub struct MessageValidationReport {
    pub messages: Vec<Message>,
    pub dropped: Vec<MessageValidationDiagnostic>,
}

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn has_visible_content(message: &Message) -> bool {
    message.parts.iter().any(|part| match part {
        ContentPart::Text { text } => !text.trim().is_empty(),
        ContentPart::Image { data, .. } => !data.is_empty(),
    })
}

fn complete_tool_call(call: &ToolCallRequest) -> bool {
    !call.id.trim().is_empty()
        && !call.name.trim().is_empty()
        && serde_json::from_str::<serde_json::Value>(&call.arguments)
            .is_ok_and(|arguments| arguments.is_object())
}

fn diagnostic(
    context: &MessageNormalizationContext<'_>,
    message: &Message,
    has_content: bool,
    tool_call_count: usize,
    reason: impl Into<String>,
) -> MessageValidationDiagnostic {
    MessageValidationDiagnostic {
        provider: context.provider.map(str::to_string),
        model: context.model.map(str::to_string),
        conversation_id: context.conversation_id.map(str::to_string),
        turn_id: context.turn_id.map(str::to_string),
        message_index: context.message_index,
        role: role_name(&message.role).to_string(),
        has_content,
        tool_call_count,
        source: context.source,
        reason: reason.into(),
    }
}

pub fn normalize_assistant_message(
    mut message: Message,
    context: &MessageNormalizationContext<'_>,
) -> Result<Option<Message>, MessageValidationError> {
    if message.role != Role::Assistant {
        return Ok(Some(message));
    }

    let has_content = has_visible_content(&message);
    let original_tool_call_count = message.tool_calls.as_ref().map_or(0, Vec::len);
    let invalid_tool_calls = message
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .filter(|call| !complete_tool_call(call))
                .count()
        })
        .unwrap_or_default();

    if invalid_tool_calls > 0 && context.invalid_assistant == InvalidAssistantHandling::Reject {
        return Err(MessageValidationError {
            diagnostic: Box::new(diagnostic(
                context,
                &message,
                has_content,
                original_tool_call_count,
                format!("{invalid_tool_calls} incomplete tool call(s)"),
            )),
        });
    }

    if let Some(calls) = message.tool_calls.as_mut() {
        calls.retain(complete_tool_call);
        if calls.is_empty() {
            message.tool_calls = None;
        }
    }
    let tool_call_count = message.tool_calls.as_ref().map_or(0, Vec::len);

    if has_content || tool_call_count > 0 {
        return Ok(Some(message));
    }

    let reason = if original_tool_call_count > 0 {
        "assistant message contained only incomplete tool calls"
    } else if message
        .reasoning_content
        .as_deref()
        .is_some_and(|reasoning| !reasoning.trim().is_empty())
    {
        "assistant message contained reasoning without visible content or tool calls"
    } else {
        "assistant message contained no visible content or tool calls"
    };
    let error = MessageValidationError {
        diagnostic: Box::new(diagnostic(
            context,
            &message,
            false,
            tool_call_count,
            reason,
        )),
    };
    match context.invalid_assistant {
        InvalidAssistantHandling::Drop => Ok(None),
        InvalidAssistantHandling::Reject => Err(error),
    }
}

pub fn validate_message_sequence(
    messages: &[Message],
    mut context: MessageNormalizationContext<'_>,
) -> Result<MessageValidationReport, MessageValidationError> {
    let mut normalized = Vec::with_capacity(messages.len());
    let mut dropped = Vec::new();
    for (index, message) in messages.iter().cloned().enumerate() {
        context.message_index = index;
        match normalize_assistant_message(message.clone(), &context)? {
            Some(message) => normalized.push(message),
            None => dropped.push(diagnostic(
                &context,
                &message,
                has_visible_content(&message),
                message.tool_calls.as_ref().map_or(0, Vec::len),
                "legacy-invalid assistant message removed before provider dispatch",
            )),
        }
    }
    Ok(MessageValidationReport {
        messages: normalized,
        dropped,
    })
}

pub fn validate_provider_request(
    messages: &[Message],
    provider: &str,
    model: &str,
) -> Result<(), CoreError> {
    let context = MessageNormalizationContext::provider_boundary(provider, model, 0);
    let report = validate_message_sequence(messages, context.clone())
        .map_err(|error| CoreError::Llm(format!("message validation failed: {error}")))?;
    validate_tool_call_sequence(&report.messages, context)
        .map_err(|error| CoreError::Llm(format!("message validation failed: {error}")))
}

fn validate_tool_call_sequence(
    messages: &[Message],
    mut context: MessageNormalizationContext<'_>,
) -> Result<(), MessageValidationError> {
    let mut pending: HashMap<String, usize> = HashMap::new();
    let mut resolved: HashSet<String> = HashSet::new();

    for (index, message) in messages.iter().enumerate() {
        context.message_index = index;
        if message.role != Role::Tool && !pending.is_empty() {
            return Err(MessageValidationError {
                diagnostic: Box::new(diagnostic(
                    &context,
                    message,
                    has_visible_content(message),
                    message.tool_calls.as_ref().map_or(0, Vec::len),
                    format!(
                        "{} tool call(s) have no result before the next non-tool message",
                        pending.len()
                    ),
                )),
            });
        }

        match message.role {
            Role::Assistant => {
                for call in message.tool_calls.as_deref().unwrap_or_default() {
                    if pending.contains_key(&call.id) || resolved.contains(&call.id) {
                        return Err(MessageValidationError {
                            diagnostic: Box::new(diagnostic(
                                &context,
                                message,
                                has_visible_content(message),
                                message.tool_calls.as_ref().map_or(0, Vec::len),
                                "duplicate tool call id",
                            )),
                        });
                    }
                    pending.insert(call.id.clone(), index);
                }
            }
            Role::Tool => {
                let call_id = message.name.as_deref().map(str::trim).unwrap_or_default();
                if call_id.is_empty() {
                    return Err(MessageValidationError {
                        diagnostic: Box::new(diagnostic(
                            &context,
                            message,
                            has_visible_content(message),
                            0,
                            "tool result has no tool call id",
                        )),
                    });
                }
                if resolved.contains(call_id) {
                    return Err(MessageValidationError {
                        diagnostic: Box::new(diagnostic(
                            &context,
                            message,
                            has_visible_content(message),
                            0,
                            "duplicate tool result",
                        )),
                    });
                }
                if pending.remove(call_id).is_none() {
                    return Err(MessageValidationError {
                        diagnostic: Box::new(diagnostic(
                            &context,
                            message,
                            has_visible_content(message),
                            0,
                            "orphan tool result",
                        )),
                    });
                }
                if !has_visible_content(message) {
                    return Err(MessageValidationError {
                        diagnostic: Box::new(diagnostic(
                            &context,
                            message,
                            false,
                            0,
                            "tool result has no content",
                        )),
                    });
                }
                resolved.insert(call_id.to_string());
            }
            Role::System | Role::User => {}
        }
    }

    if !pending.is_empty() {
        context.message_index = messages.len().saturating_sub(1);
        let message = messages
            .last()
            .expect("pending calls require an assistant message");
        return Err(MessageValidationError {
            diagnostic: Box::new(diagnostic(
                &context,
                message,
                has_visible_content(message),
                message.tool_calls.as_ref().map_or(0, Vec::len),
                format!("{} tool call(s) have no result", pending.len()),
            )),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_with_calls(calls: Option<Vec<ToolCallRequest>>) -> Message {
        Message {
            role: Role::Assistant,
            parts: Vec::new(),
            name: None,
            tool_calls: calls,
            reasoning_content: None,
            prompt_cache_hint: None,
        }
    }

    fn recovery_context() -> MessageNormalizationContext<'static> {
        MessageNormalizationContext {
            provider: Some("openai"),
            model: Some("test-model"),
            conversation_id: Some("conversation-1"),
            turn_id: Some("turn-1"),
            message_index: 0,
            source: MessageSource::Recovery,
            invalid_assistant: InvalidAssistantHandling::Drop,
        }
    }

    #[test]
    fn accepts_text_tool_calls_and_both() {
        let text = Message::text(Role::Assistant, "answer");
        let call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        };
        let tools = assistant_with_calls(Some(vec![call.clone()]));
        let mut both = Message::text(Role::Assistant, "working");
        both.tool_calls = Some(vec![call]);

        for message in [text, tools, both] {
            assert!(normalize_assistant_message(message, &recovery_context())
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn drops_legacy_empty_and_reasoning_only_assistant_messages() {
        let empty = assistant_with_calls(None);
        let mut reasoning_only = assistant_with_calls(None);
        reasoning_only.reasoning_content = Some("private reasoning".to_string());

        assert!(normalize_assistant_message(empty, &recovery_context())
            .unwrap()
            .is_none());
        assert!(
            normalize_assistant_message(reasoning_only, &recovery_context())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn provider_boundary_rejects_empty_and_incomplete_tool_calls() {
        let empty = assistant_with_calls(Some(Vec::new()));
        let incomplete = assistant_with_calls(Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"unterminated""#.to_string(),
            thought_signature: None,
        }]));
        let context = MessageNormalizationContext::provider_boundary("openai", "gpt-test", 0);

        assert!(normalize_assistant_message(empty, &context).is_err());
        let error = normalize_assistant_message(incomplete, &context).unwrap_err();
        assert_eq!(error.diagnostic.source, MessageSource::ProviderBoundary);
        assert!(error.to_string().contains("incomplete tool call"));
    }

    #[test]
    fn provider_boundary_accepts_complete_tool_call_and_result_sequence() {
        let call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        };
        let tool_call = assistant_with_calls(Some(vec![call]));
        let tool_result = Message::text_with_name(Role::Tool, "result", "call-1");

        validate_provider_request(&[tool_call, tool_result], "openai", "gpt-test").unwrap();
    }

    #[test]
    fn provider_boundary_rejects_missing_orphan_and_duplicate_tool_results() {
        let call = ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        };
        let tool_call = assistant_with_calls(Some(vec![call]));
        let tool_result = Message::text_with_name(Role::Tool, "result", "call-1");

        assert!(
            validate_provider_request(std::slice::from_ref(&tool_call), "openai", "gpt-test")
                .unwrap_err()
                .to_string()
                .contains("no result")
        );
        assert!(validate_provider_request(
            std::slice::from_ref(&tool_result),
            "openai",
            "gpt-test"
        )
        .unwrap_err()
        .to_string()
        .contains("orphan"));
        assert!(validate_provider_request(
            &[tool_call, tool_result.clone(), tool_result],
            "openai",
            "gpt-test"
        )
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    }
}
