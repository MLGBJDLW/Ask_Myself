use std::collections::HashSet;

use crate::llm::ToolCallRequest;
use crate::tools::ToolRegistry;
use crate::work_plan::MutationWorkPlan;

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
    let invocation = tools.build_invocation(call_id, tool_name, parsed_args);
    let capabilities = invocation.capabilities.clone();
    let plugin = invocation.plugin.clone();
    let artifacts = artifacts_with_work_plan(artifacts, &invocation);
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

fn artifacts_with_work_plan(
    artifacts: Option<serde_json::Value>,
    invocation: &crate::tools::ToolInvocation,
) -> Option<serde_json::Value> {
    let Some(work_plan) = MutationWorkPlan::from_tool_invocation(invocation) else {
        return artifacts;
    };
    let work_plan = serde_json::to_value(work_plan).ok()?;
    let mut map = match artifacts {
        Some(serde_json::Value::Object(map)) => map,
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert("result".to_string(), value);
            map
        }
        None => serde_json::Map::new(),
    };
    map.entry("workPlan".to_string()).or_insert(work_plan);
    Some(serde_json::Value::Object(map))
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
