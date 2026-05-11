//! Tool scheduling policy for the agent loop.

use std::collections::HashSet;
use std::time::Duration;

use crate::llm::ToolCallRequest;
use crate::tools::{structured_tool_error_result, ToolResult};

/// Maximum characters to keep in a tool result for LLM context.
/// ~4K tokens is about 16K chars for English text; the smaller cap leaves room
/// for conversation and follow-up tool calls.
const MAX_TOOL_RESULT_CONTEXT_CHARS: usize = 4_800;

#[derive(Debug, Clone)]
pub(crate) struct ToolSchedulerPolicy {
    configured_timeout_secs: Option<u32>,
    dynamic_tool_visibility: bool,
    offered_tool_names: HashSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolSchedulingDecision {
    pub(crate) parsed_args: serde_json::Value,
    pub(crate) timeout: Option<Duration>,
    pub(crate) synthetic_result: Option<ToolResult>,
    pub(crate) policy_label: &'static str,
}

impl ToolSchedulerPolicy {
    pub(crate) fn new(
        configured_timeout_secs: Option<u32>,
        dynamic_tool_visibility: bool,
        offered_tool_names: HashSet<String>,
    ) -> Self {
        Self {
            configured_timeout_secs,
            dynamic_tool_visibility,
            offered_tool_names,
        }
    }

    pub(crate) fn decision_for(&self, call: &ToolCallRequest) -> ToolSchedulingDecision {
        let parsed_args: serde_json::Value =
            serde_json::from_str(&call.arguments).unwrap_or_default();
        let timeout = tool_timeout_for_call(self.configured_timeout_secs, &call.name, &parsed_args);

        let synthetic_result = if self.dynamic_tool_visibility
            && !self.offered_tool_names.contains(&call.name)
        {
            Some(ToolResult {
                    call_id: call.id.clone(),
                    content: format!(
                        "Tool '{}' is not available in the current tool policy for this turn. Use an offered tool or ask the user to change the request scope.",
                        call.name
                    ),
                    is_error: true,
                    artifacts: None,
                })
        } else {
            None
        };

        ToolSchedulingDecision {
            parsed_args,
            timeout,
            policy_label: if synthetic_result.is_some() {
                "blockedByToolVisibility"
            } else {
                "execute"
            },
            synthetic_result,
        }
    }
}

pub(crate) fn loop_guard_blocked_result(call: &ToolCallRequest, reason: &str) -> ToolResult {
    structured_tool_error_result(
        &call.id,
        "loop_guard_blocked",
        format!(
            "{} was blocked by the loop guard: {reason}. Do not retry the same arguments; change strategy or answer with the known limitation.",
            call.name
        ),
        serde_json::json!({
            "tool": &call.name,
            "arguments": "must differ materially from the repeated blocked call",
            "recovery": "change strategy, narrow scope, ask the user, or synthesize from existing evidence"
        }),
        true,
    )
}

pub(crate) fn tool_timeout_for_call(
    configured_timeout_secs: Option<u32>,
    tool_name: &str,
    parsed_args: &serde_json::Value,
) -> Option<Duration> {
    let base_timeout = configured_timeout_secs.unwrap_or(30) as u64;
    if base_timeout == 0 {
        return None;
    }

    let multiplier = match tool_name {
        "retrieve_evidence" => 2,
        "spawn_subagent" | "spawn_subagent_batch" => 3,
        _ => 1,
    };
    let mut timeout_secs = base_timeout.saturating_mul(multiplier);

    if tool_name == "run_shell" {
        let requested = parsed_args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);
        if requested == 0 {
            return None;
        }
        timeout_secs = timeout_secs.max(requested.saturating_add(5));
    }

    Some(Duration::from_secs(timeout_secs.max(1)))
}

pub(crate) fn compact_tool_result_for_context(tool_name: &str, content: &str) -> String {
    match tool_name {
        "run_shell" | "read_file" | "fetch_url" => summarize_lines(
            &truncate_tool_result(content, MAX_TOOL_RESULT_CONTEXT_CHARS),
            40,
            25,
            MAX_TOOL_RESULT_CONTEXT_CHARS,
        ),
        "list_dir" | "list_documents" | "list_sources" => {
            summarize_lines(content, 60, 10, MAX_TOOL_RESULT_CONTEXT_CHARS)
        }
        "search_knowledge_base" => truncate_tool_result(content, 3_500),
        "retrieve_evidence" | "search_playbooks" => truncate_tool_result(content, 6_000),
        _ => truncate_tool_result(content, MAX_TOOL_RESULT_CONTEXT_CHARS),
    }
}

fn summarize_lines(text: &str, head_lines: usize, tail_lines: usize, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head_lines + tail_lines + 3 {
        return truncate_tool_result(text, max_chars);
    }
    let omitted = lines.len().saturating_sub(head_lines + tail_lines);
    let mut compact: Vec<String> = lines
        .iter()
        .take(head_lines)
        .map(|line| (*line).to_string())
        .collect();
    compact.push(format!("[... {} lines omitted ...]", omitted));
    compact.extend(
        lines
            .iter()
            .skip(lines.len().saturating_sub(tail_lines))
            .map(|line| (*line).to_string()),
    );
    let rendered = compact.join("\n");
    if rendered.len() <= max_chars {
        rendered
    } else {
        truncate_tool_result(&rendered, max_chars)
    }
}

fn truncate_tool_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    if let Some(compressed) = try_smart_compress(content, max_chars) {
        if compressed.len() <= max_chars {
            return compressed;
        }
    }

    let head_len = max_chars * 3 / 4;
    let tail_len = max_chars / 4;

    let mut head_end = head_len.min(content.len());
    while head_end > 0 && !content.is_char_boundary(head_end) {
        head_end -= 1;
    }

    let mut tail_start = content.len().saturating_sub(tail_len);
    while tail_start < content.len() && !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }

    format!(
        "{}\n\n[... truncated {} chars ...]\n\n{}",
        &content[..head_end],
        content
            .len()
            .saturating_sub(head_end + (content.len() - tail_start)),
        &content[tail_start..]
    )
}

fn try_smart_compress(content: &str, max_chars: usize) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(compressed) = compress_json_value(&value, max_chars) {
            return Some(compressed);
        }
    }

    for separator in ["\n---\n", "\n===\n", "\n## "] {
        if content.contains(separator) {
            if let Some(compressed) = compress_sections(content, separator, max_chars) {
                return Some(compressed);
            }
        }
    }

    None
}

fn compress_json_value(value: &serde_json::Value, max_chars: usize) -> Option<String> {
    let mut cloned = value.clone();
    truncate_json_strings(&mut cloned, 500);

    if let serde_json::Value::Array(arr) = &mut cloned {
        let keep = arr.len().min(20);
        if arr.len() > keep {
            let omitted = arr.len() - keep;
            arr.truncate(keep);
            arr.push(serde_json::json!({
                "_truncated": format!("{} additional items omitted", omitted)
            }));
        }
    }

    let rendered = serde_json::to_string_pretty(&cloned).ok()?;
    if rendered.len() < value.to_string().len() && rendered.len() <= max_chars * 2 {
        Some(rendered)
    } else {
        None
    }
}

fn truncate_json_strings(value: &mut serde_json::Value, max_string_len: usize) {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max_string_len {
                let mut cut = max_string_len;
                while cut > 0 && !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                *s = format!("{}... [truncated]", &s[..cut]);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                truncate_json_strings(item, max_string_len);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values_mut() {
                truncate_json_strings(value, max_string_len);
            }
        }
        _ => {}
    }
}

fn compress_sections(text: &str, separator: &str, max_chars: usize) -> Option<String> {
    let sections: Vec<&str> = text.split(separator).collect();
    if sections.len() <= 3 {
        return None;
    }

    let keep_chars = max_chars / sections.len().min(10);
    let mut result = Vec::new();

    for (i, section) in sections.iter().enumerate() {
        if i == 0 || i == sections.len() - 1 {
            result.push((*section).to_string());
        } else {
            let trimmed = section.trim();
            if trimmed.len() > keep_chars {
                let mut cut = keep_chars;
                while cut > 0 && !trimmed.is_char_boundary(cut) {
                    cut -= 1;
                }
                result.push(format!("{}...", &trimmed[..cut]));
            } else {
                result.push(trimmed.to_string());
            }
        }
    }

    let compressed = result.join(&format!("\n{}\n", separator));
    if compressed.len() < text.len() {
        Some(compressed)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_zero_disables_outer_timeout() {
        assert_eq!(
            tool_timeout_for_call(Some(0), "read_file", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn timeout_extends_for_long_shell_timeout() {
        let timeout = tool_timeout_for_call(
            Some(30),
            "run_shell",
            &serde_json::json!({ "timeout_secs": 120 }),
        );
        assert_eq!(timeout, Some(Duration::from_secs(125)));
    }

    #[test]
    fn dynamic_visibility_blocks_unoffered_tools() {
        let policy =
            ToolSchedulerPolicy::new(Some(30), true, HashSet::from(["read_file".to_string()]));
        let decision = policy.decision_for(&ToolCallRequest {
            id: "call-1".to_string(),
            name: "run_shell".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        });
        assert!(decision.synthetic_result.unwrap().is_error);
        assert_eq!(decision.policy_label, "blockedByToolVisibility");
    }
}
