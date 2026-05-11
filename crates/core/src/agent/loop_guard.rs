//! Runtime guardrails for unproductive agent loops.

use serde_json::Value;

use crate::llm::ToolCallRequest;

const REPEATED_TOOL_SIGNATURE_THRESHOLD: u32 = 3;
const REPEATED_TEXT_FINGERPRINT_THRESHOLD: u32 = 3;
const CONSECUTIVE_TOOL_ERROR_THRESHOLD: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopGuardAction {
    BlockToolCalls,
    ChangeStrategy,
}

impl LoopGuardAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LoopGuardAction::BlockToolCalls => "blockToolCalls",
            LoopGuardAction::ChangeStrategy => "changeStrategy",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LoopGuardIntervention {
    pub(crate) reason: String,
    pub(crate) action: LoopGuardAction,
    pub(crate) prompt: String,
}

#[derive(Debug, Default)]
pub(crate) struct AgentLoopGuard {
    last_tool_signature: Option<String>,
    repeated_tool_signature_count: u32,
    last_text_fingerprint: Option<String>,
    repeated_text_fingerprint_count: u32,
    consecutive_tool_errors: u32,
}

impl AgentLoopGuard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn observe_model_step(
        &mut self,
        assistant_text: &str,
        tool_calls: &[ToolCallRequest],
    ) -> Option<LoopGuardIntervention> {
        if !tool_calls.is_empty() {
            let signature = tool_call_batch_signature(tool_calls);
            if self.last_tool_signature.as_deref() == Some(signature.as_str()) {
                self.repeated_tool_signature_count =
                    self.repeated_tool_signature_count.saturating_add(1);
            } else {
                self.last_tool_signature = Some(signature);
                self.repeated_tool_signature_count = 1;
            }

            if self.repeated_tool_signature_count >= REPEATED_TOOL_SIGNATURE_THRESHOLD {
                return Some(LoopGuardIntervention {
                    reason: format!(
                        "The model requested the same tool call batch {} times without visible progress.",
                        self.repeated_tool_signature_count
                    ),
                    action: LoopGuardAction::BlockToolCalls,
                    prompt: "## Loop Guard\nThe previous tool-call batch was blocked because it repeated the same tool names and arguments without progress. Do not retry the same calls. Summarize what is known, choose a different tool/argument strategy, or ask the user for the missing information.".to_string(),
                });
            }

            self.last_text_fingerprint = None;
            self.repeated_text_fingerprint_count = 0;
            return None;
        }

        let fingerprint = text_fingerprint(assistant_text)?;
        if self.last_text_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            self.repeated_text_fingerprint_count =
                self.repeated_text_fingerprint_count.saturating_add(1);
        } else {
            self.last_text_fingerprint = Some(fingerprint);
            self.repeated_text_fingerprint_count = 1;
        }

        if self.repeated_text_fingerprint_count >= REPEATED_TEXT_FINGERPRINT_THRESHOLD {
            return Some(LoopGuardIntervention {
                reason: "The model produced the same final-text shape repeatedly without taking a new action."
                    .to_string(),
                action: LoopGuardAction::ChangeStrategy,
                prompt: "## Loop Guard\nThe last draft repeated an earlier answer shape without new progress. Make a concrete decision now: either provide the final answer with explicit uncertainty, use a different tool strategy, or ask one focused question.".to_string(),
            });
        }

        None
    }

    pub(crate) fn observe_tool_result(&mut self, is_error: bool) -> Option<LoopGuardIntervention> {
        if is_error {
            self.consecutive_tool_errors = self.consecutive_tool_errors.saturating_add(1);
        } else {
            self.consecutive_tool_errors = 0;
        }

        if self.consecutive_tool_errors >= CONSECUTIVE_TOOL_ERROR_THRESHOLD {
            return Some(LoopGuardIntervention {
                reason: format!(
                    "Observed {} consecutive tool errors.",
                    self.consecutive_tool_errors
                ),
                action: LoopGuardAction::ChangeStrategy,
                prompt: "## Loop Guard\nSeveral tool calls failed in a row. Stop retrying the same approach. Inspect the latest error, reduce scope, change arguments, or answer with the recoverable limitation if the task cannot proceed.".to_string(),
            });
        }

        None
    }
}

fn tool_call_batch_signature(tool_calls: &[ToolCallRequest]) -> String {
    tool_calls
        .iter()
        .map(|call| {
            format!(
                "{}:{}",
                call.name,
                canonical_json_text(&call.arguments).unwrap_or_else(|| call.arguments.clone())
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn canonical_json_text(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    serde_json::to_string(&canonicalize_value(value)).ok()
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_value).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_value(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

fn text_fingerprint(text: &str) -> Option<String> {
    let normalized = text
        .split_whitespace()
        .take(80)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if normalized.len() < 80 {
        return None;
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "call".to_string(),
            name: "read_file".to_string(),
            arguments: arguments.to_string(),
            thought_signature: None,
        }
    }

    #[test]
    fn detects_repeated_tool_signature_with_reordered_json() {
        let mut guard = AgentLoopGuard::new();
        assert!(guard
            .observe_model_step("", &[call(r#"{"b":2,"a":1}"#)])
            .is_none());
        assert!(guard
            .observe_model_step("", &[call(r#"{"a":1,"b":2}"#)])
            .is_none());
        let intervention = guard
            .observe_model_step("", &[call(r#"{"a":1,"b":2}"#)])
            .expect("third identical call should be blocked");

        assert_eq!(intervention.action, LoopGuardAction::BlockToolCalls);
    }

    #[test]
    fn detects_consecutive_tool_errors() {
        let mut guard = AgentLoopGuard::new();
        assert!(guard.observe_tool_result(true).is_none());
        assert!(guard.observe_tool_result(true).is_none());
        assert!(guard.observe_tool_result(true).is_none());
        assert_eq!(
            guard.observe_tool_result(true).unwrap().action,
            LoopGuardAction::ChangeStrategy
        );
        assert!(guard.observe_tool_result(false).is_none());
    }
}
