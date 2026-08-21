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

#[derive(Debug, Clone)]
pub struct MessageRepairReport {
    pub messages: Vec<Message>,
    pub repairs: Vec<MessageValidationDiagnostic>,
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
        ContentPart::ProviderTurn { .. } => false,
    })
}

pub fn is_complete_tool_call(call: &ToolCallRequest) -> bool {
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
                .filter(|call| !is_complete_tool_call(call))
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
        calls.retain(is_complete_tool_call);
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

/// Repair legacy persisted history into provider-safe assistant/tool replay
/// units. A tool round is atomic: every call must be complete and uniquely
/// identified, and it must be followed immediately by exactly one non-empty
/// result. Invalid units lose their tool envelope as a whole; genuine visible
/// assistant text is preserved, while tool-only fragments are quarantined.
pub fn repair_persisted_message_history(
    messages: Vec<Message>,
    conversation_id: Option<&str>,
) -> MessageRepairReport {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut repairs = Vec::new();
    let mut index = 0usize;

    while index < messages.len() {
        let message = &messages[index];
        if message.role == Role::Tool {
            let context = MessageNormalizationContext {
                provider: None,
                model: None,
                conversation_id,
                turn_id: None,
                message_index: index,
                source: MessageSource::Persisted,
                invalid_assistant: InvalidAssistantHandling::Drop,
            };
            repairs.push(diagnostic(
                &context,
                message,
                has_visible_content(message),
                0,
                "orphan persisted tool result removed",
            ));
            index += 1;
            continue;
        }

        if message.role != Role::Assistant {
            repaired.push(message.clone());
            index += 1;
            continue;
        }

        let calls = message.tool_calls.as_deref().unwrap_or_default();
        if calls.is_empty() {
            let mut normalized = message.clone();
            normalized.tool_calls = None;
            if has_visible_content(&normalized) {
                repaired.push(normalized);
            } else {
                let context = MessageNormalizationContext {
                    provider: None,
                    model: None,
                    conversation_id,
                    turn_id: None,
                    message_index: index,
                    source: MessageSource::Persisted,
                    invalid_assistant: InvalidAssistantHandling::Drop,
                };
                repairs.push(diagnostic(
                    &context,
                    message,
                    false,
                    0,
                    "empty persisted assistant message removed",
                ));
            }
            index += 1;
            continue;
        }

        let mut result_end = index + 1;
        while result_end < messages.len() && messages[result_end].role == Role::Tool {
            result_end += 1;
        }
        let results = &messages[index + 1..result_end];

        let mut call_ids = HashSet::new();
        let calls_are_complete = calls
            .iter()
            .all(|call| is_complete_tool_call(call) && call_ids.insert(call.id.clone()));
        let mut result_ids = HashSet::new();
        let results_are_complete = results.len() == calls.len()
            && results.iter().all(|result| {
                let call_id = result.name.as_deref().map(str::trim).unwrap_or_default();
                !call_id.is_empty()
                    && has_visible_content(result)
                    && result_ids.insert(call_id.to_string())
                    && call_ids.contains(call_id)
            });
        let unit_is_complete = calls_are_complete
            && results_are_complete
            && call_ids.len() == result_ids.len()
            && call_ids == result_ids;

        if unit_is_complete {
            repaired.push(message.clone());
            repaired.extend(results.iter().cloned());
        } else {
            let context = MessageNormalizationContext {
                provider: None,
                model: None,
                conversation_id,
                turn_id: None,
                message_index: index,
                source: MessageSource::Persisted,
                invalid_assistant: InvalidAssistantHandling::Drop,
            };
            repairs.push(diagnostic(
                &context,
                message,
                has_visible_content(message),
                calls.len(),
                format!(
                    "invalid persisted assistant/tool replay unit repaired (calls={}, results={})",
                    calls.len(),
                    results.len()
                ),
            ));
            if has_visible_content(message) {
                let mut visible_assistant = message.clone();
                visible_assistant.tool_calls = None;
                visible_assistant.clear_provider_turn();
                repaired.push(visible_assistant);
            }
        }
        index = result_end;
    }

    MessageRepairReport {
        messages: repaired,
        repairs,
    }
}

pub fn validate_provider_request(
    messages: &[Message],
    provider: &str,
    model: &str,
) -> Result<(), CoreError> {
    validate_provider_request_with_context(messages, provider, model, None, None)
}

pub fn validate_provider_request_with_context(
    messages: &[Message],
    provider: &str,
    model: &str,
    conversation_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<(), CoreError> {
    let mut context = MessageNormalizationContext::provider_boundary(provider, model, 0);
    context.conversation_id = conversation_id;
    context.turn_id = turn_id;
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
    let mut resolved_in_round: HashSet<String> = HashSet::new();

    for (index, message) in messages.iter().enumerate() {
        context.message_index = index;
        if message.role != Role::Tool {
            if !pending.is_empty() {
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
            resolved_in_round.clear();
        }

        match message.role {
            Role::Assistant => {
                for call in message.tool_calls.as_deref().unwrap_or_default() {
                    if pending.contains_key(&call.id) {
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
                if resolved_in_round.contains(call_id) {
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
                resolved_in_round.insert(call_id.to_string());
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
    fn provider_boundary_diagnostics_include_privacy_safe_request_context() {
        let incomplete = assistant_with_calls(Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: "{".to_string(),
            thought_signature: None,
        }]));

        let error = validate_provider_request_with_context(
            &[incomplete],
            "openai",
            "deepseek-v4-pro",
            Some("nexa-private-routing-key"),
            Some("turn-1"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("conversation_id=nexa-private-routing-key"));
        assert!(error.contains("turn_id=turn-1"));
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

    #[test]
    fn persisted_history_repair_quarantines_incomplete_tool_unit_and_keeps_text() {
        let mut assistant = Message::text(Role::Assistant, "Visible progress remains useful.");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-broken".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"unfinished""#.to_string(),
            thought_signature: None,
        }]);
        let tool = Message::text_with_name(Role::Tool, "not executed", "call-broken");

        let report = repair_persisted_message_history(
            vec![assistant, tool, Message::text(Role::User, "Continue")],
            Some("conversation-1"),
        );

        assert_eq!(report.repairs.len(), 1);
        assert_eq!(report.messages.len(), 2);
        assert_eq!(
            report.messages[0].text_content(),
            "Visible progress remains useful."
        );
        assert!(report.messages[0].tool_calls.is_none());
        assert!(report
            .messages
            .iter()
            .all(|message| message.role != Role::Tool));
        validate_provider_request(&report.messages, "openai", "deepseek-v4-pro").unwrap();
    }

    #[test]
    fn persisted_history_repair_preserves_complete_atomic_tool_unit() {
        let call = ToolCallRequest {
            id: "call-complete".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        };
        let assistant = assistant_with_calls(Some(vec![call]));
        let tool = Message::text_with_name(Role::Tool, "result", "call-complete");

        let report =
            repair_persisted_message_history(vec![assistant, tool], Some("conversation-1"));

        assert!(report.repairs.is_empty());
        assert_eq!(report.messages.len(), 2);
        validate_provider_request(&report.messages, "openai", "deepseek-v4-pro").unwrap();
    }

    #[test]
    fn completed_tool_rounds_may_reuse_provider_call_ids() {
        let call = || ToolCallRequest {
            id: "call_0".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        };
        let messages = vec![
            assistant_with_calls(Some(vec![call()])),
            Message::text_with_name(Role::Tool, "first result", "call_0"),
            assistant_with_calls(Some(vec![call()])),
            Message::text_with_name(Role::Tool, "second result", "call_0"),
        ];

        validate_provider_request(&messages, "openai", "local-compatible").unwrap();
        let report = repair_persisted_message_history(messages, Some("conversation-1"));
        assert!(report.repairs.is_empty());
        assert_eq!(report.messages.len(), 4);
    }
}
