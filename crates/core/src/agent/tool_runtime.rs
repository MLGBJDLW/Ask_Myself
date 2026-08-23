use std::collections::HashSet;

use crate::llm::ToolCallRequest;
use crate::llm::{ProviderHostedToolEvent, ProviderHostedToolKind, ProviderHostedToolStatus};
use crate::plugins::CapabilityOwner;
use crate::tools::diff_stats::{
    changed_line_count, create_file_diff_artifact, diff_stats_artifact, diff_stats_from_diff,
    text_diff_artifact,
};
use crate::tools::{ToolRegistry, ToolRenderKind};
use crate::work_plan::MutationWorkPlan;
use serde_json::{json, Map, Value};

use super::tool_scheduler::ToolSchedulerPolicy;
use super::{ToolRunItem, ToolRunStatus};

/// Tool arguments are only a diagnostic aid in frontend events. Semantic
/// artifacts remain authoritative, so keep the cumulative raw JSON bounded
/// while the model is still producing it.
const MAX_PREPARING_ARGUMENT_PREVIEW_BYTES: usize = 32 * 1024;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_tool_run_item(
    tools: &ToolRegistry,
    call_id: &str,
    tool_name: &str,
    status: ToolRunStatus,
    arguments: Option<&str>,
    content: Option<String>,
    is_error: Option<bool>,
    artifacts: Option<serde_json::Value>,
    progress_note: Option<String>,
    duration_ms: Option<u64>,
) -> ToolRunItem {
    // Complete JSON remains the execution contract. This tolerant parser is
    // used only for display metadata and semantic previews while a provider is
    // still streaming an unfinished function-call argument string.
    let parsed_args = parse_tool_arguments_for_preview(arguments);
    let invocation = tools.build_invocation(call_id, tool_name, parsed_args.clone());
    let capabilities = invocation.capabilities.clone();
    let owner = invocation.owner.clone();
    let artifacts =
        artifacts_for_tool_run(artifacts, &invocation, &status, &parsed_args, arguments);
    let displayed_arguments = arguments.map(|arguments| {
        let audit_safe =
            crate::tool_argument_projection::audit_safe_arguments_string(tool_name, arguments);
        if matches!(status, ToolRunStatus::Preparing) {
            truncate_utf8_prefix(&audit_safe, MAX_PREPARING_ARGUMENT_PREVIEW_BYTES)
        } else {
            audit_safe
        }
    });

    ToolRunItem {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        owner,
        provider_executed: false,
        status,
        arguments: displayed_arguments,
        render_kind: capabilities.render_kind,
        capabilities,
        content,
        is_error,
        artifacts,
        progress_note,
        duration_ms,
    }
}

pub(super) fn build_provider_hosted_tool_run_item(
    tools: &ToolRegistry,
    event: &ProviderHostedToolEvent,
) -> ToolRunItem {
    let parsed_args = event
        .arguments
        .as_deref()
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .unwrap_or(Value::Null);
    let mut capabilities = tools.run_capabilities(&event.tool_name, &parsed_args);
    capabilities.render_kind = match event.kind {
        ProviderHostedToolKind::WebSearch | ProviderHostedToolKind::FileSearch => {
            ToolRenderKind::Search
        }
        ProviderHostedToolKind::CodeInterpreter | ProviderHostedToolKind::Shell => {
            ToolRenderKind::CommandExecution
        }
        ProviderHostedToolKind::ImageGeneration => ToolRenderKind::Image,
        ProviderHostedToolKind::Mcp => ToolRenderKind::Mcp,
        ProviderHostedToolKind::ComputerUse => capabilities.render_kind,
    };
    let provider_label = match event.provider_id.as_str() {
        "deepseek" => "DeepSeek",
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "google" => "Google",
        _ => event.provider_id.as_str(),
    };
    ToolRunItem {
        call_id: event.call_id.clone(),
        tool_name: event.tool_name.clone(),
        owner: CapabilityOwner {
            id: format!("provider-hosted:{}", event.provider_id),
            name: format!("{provider_label} hosted tools"),
            capability: "provider_hosted_tool".to_string(),
            description: format!("Executed remotely by {provider_label} inside the model request."),
        },
        provider_executed: true,
        status: match event.status {
            ProviderHostedToolStatus::Running => ToolRunStatus::Running,
            ProviderHostedToolStatus::Completed => ToolRunStatus::Completed,
            ProviderHostedToolStatus::Failed => ToolRunStatus::Failed,
        },
        arguments: event.arguments.clone(),
        render_kind: capabilities.render_kind,
        capabilities,
        content: event.content.clone(),
        is_error: match event.status {
            ProviderHostedToolStatus::Running => None,
            ProviderHostedToolStatus::Completed => Some(false),
            ProviderHostedToolStatus::Failed => Some(true),
        },
        artifacts: event.artifacts.clone(),
        progress_note: None,
        duration_ms: None,
    }
}

fn truncate_utf8_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn parse_tool_arguments_for_preview(arguments: Option<&str>) -> Value {
    let Some(arguments) = arguments else {
        return Value::Null;
    };
    if let Ok(value) = serde_json::from_str::<Value>(arguments) {
        return value;
    }

    let partial = parse_partial_top_level_object(arguments);
    if partial.is_empty() {
        Value::Null
    } else {
        Value::Object(partial)
    }
}

/// Best-effort parser for the common function-call shape `{ "key": value }`.
/// It deliberately extracts only values that are safe to observe before the
/// JSON closes. In particular, an unfinished top-level string is decoded up to
/// the last complete escape sequence and exposed as a preview value.
fn parse_partial_top_level_object(input: &str) -> Map<String, Value> {
    let bytes = input.as_bytes();
    let mut out = Map::new();
    let Some(mut index) = bytes
        .iter()
        .position(|byte| *byte == b'{')
        .map(|idx| idx + 1)
    else {
        return out;
    };

    loop {
        skip_json_whitespace(bytes, &mut index);
        while bytes.get(index) == Some(&b',') {
            index += 1;
            skip_json_whitespace(bytes, &mut index);
        }
        if index >= bytes.len() || bytes.get(index) == Some(&b'}') {
            break;
        }

        let Some((key, next, key_closed)) = parse_partial_json_string(input, index) else {
            break;
        };
        if !key_closed {
            break;
        }
        index = next;
        skip_json_whitespace(bytes, &mut index);
        if bytes.get(index) != Some(&b':') {
            break;
        }
        index += 1;
        skip_json_whitespace(bytes, &mut index);
        let Some(next_byte) = bytes.get(index).copied() else {
            break;
        };

        match next_byte {
            b'"' => {
                let Some((value, next, closed)) = parse_partial_json_string(input, index) else {
                    break;
                };
                out.insert(key, Value::String(value));
                index = next;
                if !closed {
                    break;
                }
            }
            b'{' | b'[' => {
                let Some(end) = complete_container_end(input, index) else {
                    break;
                };
                if let Ok(value) = serde_json::from_str::<Value>(&input[index..end]) {
                    out.insert(key, value);
                }
                index = end;
            }
            _ => {
                let start = index;
                while index < bytes.len() && !matches!(bytes[index], b',' | b'}') {
                    index += 1;
                }
                let token = input[start..index].trim();
                if !token.is_empty() {
                    if let Ok(value) = serde_json::from_str::<Value>(token) {
                        out.insert(key, value);
                    }
                }
            }
        }
    }

    out
}

fn skip_json_whitespace(bytes: &[u8], index: &mut usize) {
    while bytes
        .get(*index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        *index += 1;
    }
}

fn parse_partial_json_string(input: &str, start: usize) -> Option<(String, usize, bool)> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }

    let mut output = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((output, index + 1, true)),
            b'\\' => {
                let Some(escape) = bytes.get(index + 1).copied() else {
                    return Some((output, bytes.len(), false));
                };
                match escape {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'/' => output.push('/'),
                    b'b' => output.push('\u{0008}'),
                    b'f' => output.push('\u{000c}'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    b'u' => {
                        let Some(unit) = parse_hex_quad(bytes, index + 2) else {
                            return Some((output, bytes.len(), false));
                        };
                        if (0xD800..=0xDBFF).contains(&unit) {
                            let low_escape_start = index + 6;
                            if bytes.get(low_escape_start) != Some(&b'\\')
                                || bytes.get(low_escape_start + 1) != Some(&b'u')
                            {
                                return Some((output, bytes.len(), false));
                            }
                            let Some(low) = parse_hex_quad(bytes, low_escape_start + 2) else {
                                return Some((output, bytes.len(), false));
                            };
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Some((output, bytes.len(), false));
                            }
                            let scalar = 0x1_0000
                                + (((unit as u32) - 0xD800) << 10)
                                + ((low as u32) - 0xDC00);
                            if let Some(character) = char::from_u32(scalar) {
                                output.push(character);
                            }
                            index += 12;
                            continue;
                        }
                        if (0xDC00..=0xDFFF).contains(&unit) {
                            return Some((output, bytes.len(), false));
                        }
                        if let Some(character) = char::from_u32(unit as u32) {
                            output.push(character);
                        }
                        index += 6;
                        continue;
                    }
                    _ => return Some((output, bytes.len(), false)),
                }
                index += 2;
            }
            byte if byte < 0x20 => return Some((output, index, false)),
            _ => {
                let character = input[index..].chars().next()?;
                output.push(character);
                index += character.len_utf8();
            }
        }
    }

    Some((output, index, false))
}

fn parse_hex_quad(bytes: &[u8], start: usize) -> Option<u16> {
    let digits = bytes.get(start..start + 4)?;
    let mut value = 0u16;
    for digit in digits {
        value = (value << 4)
            | match digit {
                b'0'..=b'9' => (digit - b'0') as u16,
                b'a'..=b'f' => (digit - b'a' + 10) as u16,
                b'A'..=b'F' => (digit - b'A' + 10) as u16,
                _ => return None,
            };
    }
    Some(value)
}

fn complete_container_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let first = *bytes.get(start)?;
    let mut stack = vec![match first {
        b'{' => b'}',
        b'[' => b']',
        _ => return None,
    }];
    let mut index = start + 1;
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' if stack.last() == Some(&byte) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn artifacts_for_tool_run(
    artifacts: Option<Value>,
    invocation: &crate::tools::ToolInvocation,
    status: &ToolRunStatus,
    parsed_args: &Value,
    raw_arguments: Option<&str>,
) -> Option<Value> {
    let mut artifacts = if artifacts.is_none() && matches!(status, ToolRunStatus::Preparing) {
        streaming_file_change_preview_artifact(&invocation.tool_name, parsed_args)
    } else {
        artifacts
    };

    if matches!(status, ToolRunStatus::Preparing) {
        if let Some(Value::Object(map)) = artifacts.as_mut() {
            map.insert(
                "inputProgress".to_string(),
                json!({
                    "receivedBytes": raw_arguments.map(str::len).unwrap_or(0),
                    "argumentsComplete": raw_arguments
                        .is_some_and(|arguments| serde_json::from_str::<Value>(arguments).is_ok()),
                }),
            );
        }
    }

    artifacts_with_work_plan(artifacts, invocation)
}

fn artifacts_with_work_plan(
    artifacts: Option<Value>,
    invocation: &crate::tools::ToolInvocation,
) -> Option<Value> {
    let Some(work_plan) = MutationWorkPlan::from_tool_invocation(invocation) else {
        return artifacts;
    };
    let work_plan = serde_json::to_value(work_plan).ok()?;
    let mut map = match artifacts {
        Some(Value::Object(map)) => map,
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert("result".to_string(), value);
            map
        }
        None => serde_json::Map::new(),
    };
    map.entry("workPlan".to_string()).or_insert(work_plan);
    Some(Value::Object(map))
}

fn string_arg<'a>(args: &'a Value, fields: &[&str]) -> Option<&'a str> {
    fields.iter().find_map(|field| args.get(*field)?.as_str())
}

fn non_empty_string_arg<'a>(args: &'a Value, fields: &[&str]) -> Option<&'a str> {
    string_arg(args, fields).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn preview_artifact_from_diff(diff: Value, replacements: Option<usize>) -> Value {
    json!({
        "kind": "fileChangePreview",
        "preview": true,
        "diffStats": diff_stats_from_diff(&diff, replacements),
        "diff": diff,
    })
}

fn streaming_file_change_preview_artifact(tool_name: &str, args: &Value) -> Option<Value> {
    match tool_name {
        "edit_file" => edit_file_preview_artifact(args),
        "create_file" => create_file_preview_artifact(args),
        "multi_edit" => multi_edit_preview_artifact(args),
        "write_note" => write_note_preview_artifact(args),
        _ => None,
    }
}

fn edit_file_preview_artifact(args: &Value) -> Option<Value> {
    let path = non_empty_string_arg(args, &["path", "file_path", "filePath"])?;
    let old_content = string_arg(args, &["old_str", "old_string"]);
    let new_content = string_arg(args, &["new_str", "new_string", "content"])?;
    let action = string_arg(args, &["action"]).unwrap_or_else(|| {
        if old_content.is_some_and(|value| !value.is_empty()) {
            "str_replace"
        } else {
            "create"
        }
    });

    let diff = if matches!(action, "create" | "append") || old_content.unwrap_or("").is_empty() {
        let mut diff = create_file_diff_artifact(path, new_content);
        if let Some(map) = diff.as_object_mut() {
            map.insert("operation".to_string(), Value::String(action.to_string()));
        }
        diff
    } else {
        text_diff_artifact(path, "str_replace", old_content.unwrap_or(""), new_content)
    };
    Some(preview_artifact_from_diff(
        diff,
        Some(usize::from(!matches!(action, "create" | "append"))),
    ))
}

fn create_file_preview_artifact(args: &Value) -> Option<Value> {
    let path = non_empty_string_arg(args, &["path", "file_path", "filePath"])?;
    let content = string_arg(args, &["content", "new_str", "new_string"])?;
    let operation = string_arg(args, &["mode"])
        .filter(|mode| matches!(*mode, "create" | "overwrite" | "append"))
        .unwrap_or_else(|| {
            if args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "overwrite"
            } else {
                "create"
            }
        });
    let mut diff = if operation == "overwrite" {
        text_diff_artifact(path, operation, "", content)
    } else {
        create_file_diff_artifact(path, content)
    };
    if let Some(map) = diff.as_object_mut() {
        map.insert(
            "operation".to_string(),
            Value::String(operation.to_string()),
        );
    }
    Some(preview_artifact_from_diff(diff, Some(0)))
}

fn multi_edit_preview_artifact(args: &Value) -> Option<Value> {
    let path = non_empty_string_arg(args, &["path", "file_path", "filePath"])?;
    let edits = args.get("edits")?.as_array()?;
    if edits.is_empty() {
        return None;
    }

    let mut diffs = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut replacements = 0usize;

    for edit in edits {
        let old_content = string_arg(edit, &["old_str", "old_string"]).unwrap_or("");
        let new_content = string_arg(edit, &["new_str", "new_string", "content"]).unwrap_or("");
        if old_content.is_empty() && new_content.is_empty() {
            continue;
        }
        diffs.push(text_diff_artifact(
            path,
            "multi_edit",
            old_content,
            new_content,
        ));
        additions += changed_line_count(new_content);
        deletions += changed_line_count(old_content);
        replacements += 1;
    }

    if diffs.is_empty() {
        return None;
    }

    Some(json!({
        "kind": "fileChangePreview",
        "preview": true,
        "diffStats": diff_stats_artifact(
            path,
            "multi_edit",
            additions,
            deletions,
            diffs.len(),
            Some(replacements),
        ),
        "diffs": diffs,
    }))
}

fn write_note_preview_artifact(args: &Value) -> Option<Value> {
    let filename = non_empty_string_arg(args, &["filename", "path"])?;
    let content = string_arg(args, &["content"])?;
    let mode = string_arg(args, &["mode"]).unwrap_or("create");
    let mut diff = if mode == "create" {
        create_file_diff_artifact(filename, content)
    } else {
        text_diff_artifact(filename, mode, "", content)
    };
    if let Some(map) = diff.as_object_mut() {
        map.insert("operation".to_string(), Value::String(mode.to_string()));
    }
    Some(preview_artifact_from_diff(diff, Some(0)))
}

pub(super) fn tool_call_execution_batches(
    tools: &ToolRegistry,
    tool_policy: &ToolSchedulerPolicy,
    tool_calls: &[ToolCallRequest],
) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current_parallel_batch: Vec<usize> = Vec::new();
    let mut current_resource_keys: HashSet<String> = HashSet::new();
    let mut current_exclusive_resource_keys: HashSet<String> = HashSet::new();

    for (index, tool_call) in tool_calls.iter().enumerate() {
        let scheduling = tool_policy.decision_for(tools, tool_call);
        let invocation =
            tools.build_invocation(&tool_call.id, &tool_call.name, scheduling.parsed_args);
        if invocation.wait_for_previous && !current_parallel_batch.is_empty() {
            batches.push(std::mem::take(&mut current_parallel_batch));
            current_resource_keys.clear();
            current_exclusive_resource_keys.clear();
        }
        let exclusive =
            invocation.capabilities.destructive || !invocation.capabilities.concurrency_safe;
        let has_resource_keys = !invocation.capabilities.resource_keys.is_empty();
        let resource_conflict = if exclusive {
            invocation
                .capabilities
                .resource_keys
                .iter()
                .any(|key| current_resource_keys.contains(key))
        } else {
            invocation
                .capabilities
                .resource_keys
                .iter()
                .any(|key| current_exclusive_resource_keys.contains(key))
        };
        let unkeyed_exclusive = exclusive && !has_resource_keys;

        if !current_parallel_batch.is_empty() && (unkeyed_exclusive || resource_conflict) {
            batches.push(std::mem::take(&mut current_parallel_batch));
            current_resource_keys.clear();
            current_exclusive_resource_keys.clear();
        }

        current_parallel_batch.push(index);
        for key in &invocation.capabilities.resource_keys {
            current_resource_keys.insert(key.clone());
            if exclusive {
                current_exclusive_resource_keys.insert(key.clone());
            }
        }

        if unkeyed_exclusive {
            batches.push(std::mem::take(&mut current_parallel_batch));
            current_resource_keys.clear();
            current_exclusive_resource_keys.clear();
        }
    }

    if !current_parallel_batch.is_empty() {
        batches.push(current_parallel_batch);
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::default_tool_registry;

    #[test]
    fn named_provider_hosted_mcp_tool_keeps_mcp_render_kind() {
        let tools = default_tool_registry();
        let run = build_provider_hosted_tool_run_item(
            &tools,
            &ProviderHostedToolEvent {
                call_id: "mcp-1".to_string(),
                tool_name: "remote_lookup".to_string(),
                kind: ProviderHostedToolKind::Mcp,
                provider_id: "openai".to_string(),
                status: ProviderHostedToolStatus::Completed,
                arguments: Some("{}".to_string()),
                content: Some("ok".to_string()),
                artifacts: None,
            },
        );

        assert_eq!(run.tool_name, "remote_lookup");
        assert_eq!(run.render_kind, ToolRenderKind::Mcp);
        assert_eq!(run.capabilities.render_kind, ToolRenderKind::Mcp);
    }

    #[test]
    fn mutation_tool_run_item_includes_work_plan_artifact() {
        let tools = default_tool_registry();
        let run = build_tool_run_item(
            &tools,
            "call-1",
            "edit_file",
            ToolRunStatus::Preparing,
            Some(r#"{"path":"notes.md","old_str":"before","new_str":"after"}"#),
            None,
            None,
            None,
            None,
            None,
        );

        let artifacts = run.artifacts.expect("work plan artifact");
        assert_eq!(artifacts["workPlan"]["toolName"], "edit_file");
        assert_eq!(artifacts["workPlan"]["steps"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn preparing_edit_file_run_item_includes_streaming_diff_preview() {
        let tools = default_tool_registry();
        let run = build_tool_run_item(
            &tools,
            "call-1",
            "edit_file",
            ToolRunStatus::Preparing,
            Some(r#"{"path":"notes.md","old_str":"before","new_str":"after\nextra"}"#),
            None,
            None,
            None,
            None,
            None,
        );

        let artifacts = run.artifacts.expect("preview artifacts");
        assert_eq!(artifacts["kind"], "fileChangePreview");
        assert_eq!(artifacts["preview"], true);
        assert_eq!(artifacts["diff"]["path"], "notes.md");
        assert_eq!(artifacts["diffStats"]["kind"], "diffStats");
        assert_eq!(artifacts["diffStats"]["additions"], 2);
        assert_eq!(artifacts["diffStats"]["deletions"], 1);
        assert_eq!(artifacts["workPlan"]["toolName"], "edit_file");
    }

    #[test]
    fn partial_create_file_json_produces_live_diff_preview() {
        let tools = default_tool_registry();
        let arguments = r#"{"path":"notes.md","mode":"append","content":"first\nsecond"#;
        let run = build_tool_run_item(
            &tools,
            "call-live",
            "create_file",
            ToolRunStatus::Preparing,
            Some(arguments),
            None,
            None,
            None,
            None,
            None,
        );

        let artifacts = run.artifacts.expect("partial preview artifacts");
        assert_eq!(artifacts["diff"]["path"], "notes.md");
        assert_eq!(artifacts["diff"]["operation"], "append");
        assert_eq!(artifacts["diffStats"]["additions"], 2);
        assert_eq!(artifacts["inputProgress"]["receivedBytes"], arguments.len());
        assert_eq!(artifacts["inputProgress"]["argumentsComplete"], false);
    }

    #[test]
    fn partial_json_string_decodes_unicode_and_stops_before_incomplete_escape() {
        let parsed = parse_tool_arguments_for_preview(Some(
            r#"{"path":"notes.md","content":"\u4F60\u597D \uD83D\uDE03 tail\"#,
        ));
        assert_eq!(parsed["path"], "notes.md");
        assert_eq!(parsed["content"], "你好 😃 tail");
    }

    #[test]
    fn preparing_arguments_are_bounded_without_losing_semantic_preview() {
        let tools = default_tool_registry();
        let content = "x".repeat(MAX_PREPARING_ARGUMENT_PREVIEW_BYTES + 4096);
        let arguments = serde_json::json!({
            "path": "large.txt",
            "content": content,
        })
        .to_string();
        let run = build_tool_run_item(
            &tools,
            "call-large",
            "create_file",
            ToolRunStatus::Preparing,
            Some(&arguments),
            None,
            None,
            None,
            None,
            None,
        );

        assert!(run.arguments.as_deref().unwrap().len() <= MAX_PREPARING_ARGUMENT_PREVIEW_BYTES);
        assert_eq!(
            run.artifacts.as_ref().unwrap()["inputProgress"]["receivedBytes"],
            arguments.len()
        );
        assert_eq!(run.artifacts.as_ref().unwrap()["diffStats"]["additions"], 1);
    }

    #[test]
    fn readonly_tool_run_item_does_not_create_work_plan_artifact() {
        let tools = default_tool_registry();
        let run = build_tool_run_item(
            &tools,
            "call-1",
            "search_knowledge_base",
            ToolRunStatus::Preparing,
            Some(r#"{"query":"notes"}"#),
            None,
            None,
            None,
            None,
            None,
        );

        assert!(run.artifacts.is_none());
    }

    #[test]
    fn readonly_tool_run_item_preserves_existing_artifacts() {
        let tools = default_tool_registry();
        let run = build_tool_run_item(
            &tools,
            "call-1",
            "search_knowledge_base",
            ToolRunStatus::Completed,
            Some(r#"{"query":"notes"}"#),
            None,
            None,
            Some(serde_json::json!({ "kind": "searchResults" })),
            None,
            None,
        );

        assert_eq!(run.artifacts.unwrap()["kind"], "searchResults");
    }

    #[test]
    fn computer_control_tool_run_never_exposes_typed_text() {
        let tools = default_tool_registry();
        let sentinel = "tool-run-secret-f231";
        let arguments = serde_json::json!({
            "action": "type_text",
            "observation_id": "observation",
            "window_id": 42,
            "text": sentinel
        })
        .to_string();
        let run = build_tool_run_item(
            &tools,
            "call-sensitive",
            "computer_control",
            ToolRunStatus::Running,
            Some(&arguments),
            None,
            None,
            None,
            None,
            None,
        );
        let displayed = run.arguments.expect("audit-safe arguments");
        assert!(!displayed.contains(sentinel));
        assert!(displayed.contains("charCount"));
    }
}
