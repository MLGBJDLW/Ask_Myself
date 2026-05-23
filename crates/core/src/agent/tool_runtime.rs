use std::collections::HashSet;

use crate::llm::ToolCallRequest;
use crate::tools::diff_stats::{
    changed_line_count, create_file_diff_artifact, diff_stats_artifact, diff_stats_from_diff,
    text_diff_artifact,
};
use crate::tools::ToolRegistry;
use crate::work_plan::MutationWorkPlan;
use serde_json::{json, Value};

use super::tool_scheduler::ToolSchedulerPolicy;
use super::{ToolRunItem, ToolRunStatus};

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
    let parsed_args = arguments
        .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
        .unwrap_or(serde_json::Value::Null);
    let invocation = tools.build_invocation(call_id, tool_name, parsed_args.clone());
    let capabilities = invocation.capabilities.clone();
    let plugin = invocation.plugin.clone();
    let artifacts = artifacts_for_tool_run(artifacts, &invocation, &status, &parsed_args);
    ToolRunItem {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        plugin,
        status,
        arguments: arguments.map(ToString::to_string),
        render_kind: capabilities.render_kind,
        capabilities,
        content,
        is_error,
        artifacts,
        progress_note,
        duration_ms,
    }
}

fn artifacts_for_tool_run(
    artifacts: Option<Value>,
    invocation: &crate::tools::ToolInvocation,
    status: &ToolRunStatus,
    parsed_args: &Value,
) -> Option<Value> {
    let artifacts = if artifacts.is_none() && matches!(status, ToolRunStatus::Preparing) {
        streaming_file_change_preview_artifact(&invocation.tool_name, parsed_args)
    } else {
        artifacts
    };
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

    let diff = if matches!(action, "create") || old_content.unwrap_or("").is_empty() {
        create_file_diff_artifact(path, new_content)
    } else {
        text_diff_artifact(path, "str_replace", old_content.unwrap_or(""), new_content)
    };
    Some(preview_artifact_from_diff(
        diff,
        Some(usize::from(!matches!(action, "create"))),
    ))
}

fn create_file_preview_artifact(args: &Value) -> Option<Value> {
    let path = non_empty_string_arg(args, &["path", "file_path", "filePath"])?;
    let content = string_arg(args, &["content", "new_str", "new_string"])?;
    let operation = if args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "overwrite"
    } else {
        "create"
    };
    let diff = if operation == "create" {
        create_file_diff_artifact(path, content)
    } else {
        text_diff_artifact(path, operation, "", content)
    };
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
    let diff = if mode == "create" {
        create_file_diff_artifact(filename, content)
    } else {
        text_diff_artifact(filename, mode, "", content)
    };
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
        let scheduling = tool_policy.decision_for(tool_call);
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
}
