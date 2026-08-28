//! Integrity boundary between provider tool-call assembly and execution.
//!
//! Streaming deltas are drafts. Only a sealed batch may cross into dispatch;
//! this type makes that authorization visible in the Rust interface.

use std::collections::HashSet;

use crate::llm::message_validation::is_complete_tool_call;
use crate::llm::ToolCallRequest;
use crate::tools::MAX_TOOL_CALL_ARGUMENT_BYTES;

#[derive(Debug, Clone)]
pub(super) struct VerifiedToolCallBatch {
    calls: Vec<ToolCallRequest>,
}

#[derive(Debug, Clone)]
pub(super) struct RejectedToolCallBatch {
    pub(super) incomplete_count: usize,
    pub(super) duplicate_id_count: usize,
    pub(super) oversized_count: usize,
    pub(super) assembly_rejected: bool,
    pub(super) terminal_rejected: bool,
}

impl VerifiedToolCallBatch {
    pub(super) fn seal(
        calls: Vec<ToolCallRequest>,
        assembly_rejected: bool,
        terminal_allows_tool_dispatch: bool,
    ) -> Result<Self, RejectedToolCallBatch> {
        let calls = calls
            .into_iter()
            .map(normalize_provider_tool_call)
            .collect::<Vec<_>>();
        let incomplete_count = calls
            .iter()
            .filter(|call| !is_complete_tool_call(call))
            .count();
        let mut ids = HashSet::new();
        let duplicate_id_count = calls
            .iter()
            .filter(|call| !call.id.trim().is_empty() && !ids.insert(call.id.as_str()))
            .count();
        let oversized_count = calls
            .iter()
            .filter(|call| call.arguments.len() > MAX_TOOL_CALL_ARGUMENT_BYTES)
            .count();
        let terminal_rejected = !calls.is_empty() && !terminal_allows_tool_dispatch;
        if assembly_rejected
            || terminal_rejected
            || incomplete_count > 0
            || duplicate_id_count > 0
            || oversized_count > 0
        {
            return Err(RejectedToolCallBatch {
                incomplete_count,
                duplicate_id_count,
                oversized_count,
                assembly_rejected,
                terminal_rejected,
            });
        }
        Ok(Self { calls })
    }

    pub(super) fn as_slice(&self) -> &[ToolCallRequest] {
        &self.calls
    }
}

fn normalize_provider_tool_call(mut call: ToolCallRequest) -> ToolCallRequest {
    call.id = call.id.trim().to_string();
    call.name = call.name.trim().to_string();
    if let Some(arguments) = normalize_provider_tool_arguments(&call.arguments) {
        call.arguments = arguments;
    }
    call
}

fn normalize_provider_tool_arguments(arguments: &str) -> Option<String> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Some("{}".to_string());
    }

    let unfenced = if let Some(rest) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
    {
        rest.strip_suffix("```")?.trim()
    } else {
        trimmed
    };

    let value = serde_json::from_str::<serde_json::Value>(unfenced).ok()?;
    let object = match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(object),
        serde_json::Value::String(encoded) => {
            let decoded = serde_json::from_str::<serde_json::Value>(encoded.trim()).ok()?;
            if decoded.is_object() {
                decoded
            } else {
                return None;
            }
        }
        serde_json::Value::Null => serde_json::json!({}),
        _ => return None,
    };
    serde_json::to_string(&object).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: id.to_string(),
            name: "search".to_string(),
            arguments: arguments.to_string(),
            thought_signature: None,
        }
    }

    #[test]
    fn seal_is_the_only_transition_to_verified_calls() {
        let verified =
            VerifiedToolCallBatch::seal(vec![call("call-1", r#"{"query":"rust"}"#)], false, true)
                .unwrap();
        assert_eq!(verified.as_slice().len(), 1);
    }

    #[test]
    fn seal_normalizes_provider_empty_and_wrapped_object_arguments() {
        let verified = VerifiedToolCallBatch::seal(
            vec![
                call("call-empty", ""),
                call("call-fenced", "```json\n{\"query\":\"rust\"}\n```"),
                call("call-encoded", r#""{\"query\":\"docs\"}""#),
            ],
            false,
            true,
        )
        .expect("recoverable provider encodings should cross the execution boundary");

        assert_eq!(verified.as_slice()[0].arguments, "{}");
        assert_eq!(verified.as_slice()[1].arguments, r#"{"query":"rust"}"#);
        assert_eq!(verified.as_slice()[2].arguments, r#"{"query":"docs"}"#);
    }

    #[test]
    fn seal_rejects_incomplete_duplicate_and_ambiguous_assembly() {
        let incomplete = VerifiedToolCallBatch::seal(vec![call("call-1", "{")], false, true)
            .expect_err("invalid JSON is not executable");
        assert_eq!(incomplete.incomplete_count, 1);

        let duplicate = VerifiedToolCallBatch::seal(
            vec![call("call-1", "{}"), call("call-1", "{}")],
            false,
            true,
        )
        .expect_err("duplicate ids are not executable");
        assert_eq!(duplicate.duplicate_id_count, 1);

        let ambiguous = VerifiedToolCallBatch::seal(vec![call("call-1", "{}")], true, true)
            .expect_err("a rejected stream fragment taints the batch");
        assert!(ambiguous.assembly_rejected);

        let truncated = VerifiedToolCallBatch::seal(vec![call("call-1", "{}")], false, false)
            .expect_err("a syntactically valid call is unsafe without a trusted terminal");
        assert!(truncated.terminal_rejected);

        let oversized = VerifiedToolCallBatch::seal(
            vec![call(
                "call-1",
                &"x".repeat(MAX_TOOL_CALL_ARGUMENT_BYTES + 1),
            )],
            false,
            true,
        )
        .expect_err("oversized arguments are not executable");
        assert_eq!(oversized.oversized_count, 1);
    }
}
