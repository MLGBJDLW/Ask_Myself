use serde::{Deserialize, Serialize};

use crate::evidence_verifier::EvidenceSignals;
use crate::tools::{ToolRegistry, ToolRenderKind, ToolRunCapabilities};

use super::route::AgentRouteKind;
use super::turn_events::TurnLoopEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PersistedTraceToolCall {
    call_id: String,
    tool_name: String,
    arguments: String,
    status: String,
    #[serde(rename = "renderKind")]
    render_kind: ToolRenderKind,
    capabilities: ToolRunCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifacts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum PersistedTraceItem {
    Thinking { text: String },
    Tool { tool_call: PersistedTraceToolCall },
    Status { text: String, tone: String },
    Loop { event: TurnLoopEvent },
}

pub(super) fn append_persisted_trace_thinking(items: &mut Vec<PersistedTraceItem>, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    items.push(PersistedTraceItem::Thinking {
        text: trimmed.to_string(),
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_persisted_trace_tool(
    items: &mut Vec<PersistedTraceItem>,
    tools: &ToolRegistry,
    tool_name: &str,
    arguments: &str,
    call_id: &str,
    status: &str,
    content: Option<String>,
    is_error: Option<bool>,
    artifacts: Option<serde_json::Value>,
) {
    let parsed_args = serde_json::from_str::<serde_json::Value>(arguments).ok();
    let capabilities = tools.run_capabilities(
        tool_name,
        parsed_args.as_ref().unwrap_or(&serde_json::Value::Null),
    );
    items.push(PersistedTraceItem::Tool {
        tool_call: PersistedTraceToolCall {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: arguments.to_string(),
            status: status.to_string(),
            render_kind: capabilities.render_kind,
            capabilities,
            content,
            is_error,
            artifacts,
        },
    });
}

pub(super) fn append_persisted_trace_status(
    items: &mut Vec<PersistedTraceItem>,
    text: &str,
    tone: &str,
) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    items.push(PersistedTraceItem::Status {
        text: trimmed.to_string(),
        tone: tone.to_string(),
    });
}

pub(super) fn append_persisted_trace_loop_event(
    items: &mut Vec<PersistedTraceItem>,
    event: TurnLoopEvent,
) {
    items.push(PersistedTraceItem::Loop { event });
}

pub(super) fn build_trace_artifacts(items: &[PersistedTraceItem]) -> Option<serde_json::Value> {
    if items.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "kind": "traceTimeline",
        "version": 1,
        "items": items,
    }))
}

pub(super) fn build_turn_trace(
    route_kind: AgentRouteKind,
    items: &[PersistedTraceItem],
) -> serde_json::Value {
    build_turn_trace_with_verification(route_kind, items, None)
}

pub(super) fn build_turn_trace_with_verification(
    route_kind: AgentRouteKind,
    items: &[PersistedTraceItem],
    verification: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut trace = serde_json::json!({
        "kind": "turnTrace",
        "routeKind": format!("{route_kind:?}"),
        "items": items,
    });
    if let Some(verification) = verification {
        trace["verification"] = verification.clone();
    }
    trace
}

pub(super) fn evidence_signals_from_trace(items: &[PersistedTraceItem]) -> EvidenceSignals {
    let mut successful_evidence_tool_calls = 0usize;
    let mut verification_tool_recorded = false;

    for item in items {
        let PersistedTraceItem::Tool { tool_call } = item else {
            continue;
        };
        let ok = tool_call.status == "done" && tool_call.is_error != Some(true);
        if ok && is_evidence_oriented_tool(&tool_call.tool_name) {
            successful_evidence_tool_calls += 1;
        }
        if ok && tool_call.tool_name == "record_verification" {
            verification_tool_recorded = true;
        }
    }

    EvidenceSignals {
        successful_evidence_tool_calls,
        verification_tool_recorded,
    }
}

fn is_evidence_oriented_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "search_knowledge_base"
            | "retrieve_evidence"
            | "search_playbooks"
            | "compare_documents"
            | "summarize_document"
            | "query_knowledge_graph"
            | "fetch_url"
            | "read_file"
            | "read_files"
            | "get_document_info"
            | "search_sessions"
    )
}

pub(super) fn build_task_run_artifacts(verification: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentTaskArtifacts",
        "version": 1,
        "verification": verification,
    })
}
