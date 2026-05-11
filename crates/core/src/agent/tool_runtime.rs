use std::collections::HashSet;

use crate::llm::ToolCallRequest;
use crate::tools::ToolRegistry;

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
    let capabilities = tools.run_capabilities(tool_name, &parsed_args);
    ToolRunItem {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
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
        let capabilities = tools.run_capabilities(&tool_call.name, &scheduling.parsed_args);
        let exclusive = capabilities.destructive || !capabilities.concurrency_safe;
        let has_resource_keys = !capabilities.resource_keys.is_empty();
        let resource_conflict = if exclusive {
            capabilities
                .resource_keys
                .iter()
                .any(|key| current_resource_keys.contains(key))
        } else {
            capabilities
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
        for key in &capabilities.resource_keys {
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
