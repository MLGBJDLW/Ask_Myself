use serde::{Deserialize, Serialize};

use crate::agent_run::{AgentRunDisplayKind, AgentRunEventVisibility};
use crate::evidence_verifier::{EvidenceSignals, RuntimeVerificationSignals};
use crate::skills::Skill;
use crate::tool_visibility_policy::ToolVisibilityDecision;
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
#[serde(rename_all = "camelCase")]
pub(super) struct PersistedTraceSkillRef {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    activated: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl PersistedTraceSkillRef {
    fn from_skill(skill: &Skill, activated: bool) -> Self {
        let display_name = skill.interface.display_name.trim();
        Self {
            id: skill.id.clone(),
            name: skill.name.clone(),
            display_name: (!display_name.is_empty()).then(|| display_name.to_string()),
            builtin: skill.builtin,
            source_path: skill.source_path.clone(),
            activated,
        }
    }
}

impl From<&Skill> for PersistedTraceSkillRef {
    fn from(skill: &Skill) -> Self {
        PersistedTraceSkillRef::from_skill(skill, false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum PersistedTraceItem {
    Thinking {
        text: String,
    },
    Tool {
        tool_call: PersistedTraceToolCall,
    },
    SkillSelection {
        skills: Vec<PersistedTraceSkillRef>,
    },
    Status {
        text: String,
        tone: String,
        visibility: AgentRunEventVisibility,
        #[serde(rename = "displayKind")]
        display_kind: AgentRunDisplayKind,
    },
    Loop {
        event: TurnLoopEvent,
    },
    ToolVisibility {
        decision: ToolVisibilityDecision,
    },
    PromptCache {
        observation: serde_json::Value,
    },
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

#[cfg(test)]
pub(super) fn append_persisted_trace_skill_selection(
    items: &mut Vec<PersistedTraceItem>,
    skills: &[Skill],
) {
    if skills.is_empty() {
        return;
    }

    items.push(PersistedTraceItem::SkillSelection {
        skills: skills.iter().map(PersistedTraceSkillRef::from).collect(),
    });
}

pub(super) fn append_persisted_trace_loaded_skills(
    items: &mut Vec<PersistedTraceItem>,
    skills: &[Skill],
) {
    if skills.is_empty() {
        return;
    }

    items.push(PersistedTraceItem::SkillSelection {
        skills: skills
            .iter()
            .map(|skill| PersistedTraceSkillRef::from_skill(skill, true))
            .collect(),
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
        visibility: AgentRunEventVisibility::User,
        display_kind: AgentRunDisplayKind::Status,
    });
}

pub(super) fn append_internal_persisted_trace_status(
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
        visibility: AgentRunEventVisibility::Internal,
        display_kind: AgentRunDisplayKind::Status,
    });
}

pub(super) fn append_persisted_trace_loop_event(
    items: &mut Vec<PersistedTraceItem>,
    event: TurnLoopEvent,
) {
    items.push(PersistedTraceItem::Loop { event });
}

pub(super) fn append_persisted_trace_visibility(
    items: &mut Vec<PersistedTraceItem>,
    decision: &ToolVisibilityDecision,
) {
    items.push(PersistedTraceItem::ToolVisibility {
        decision: decision.clone(),
    });
}

pub(super) fn append_persisted_trace_prompt_cache(
    items: &mut Vec<PersistedTraceItem>,
    observation: serde_json::Value,
) {
    items.push(PersistedTraceItem::PromptCache { observation });
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
    let mut verification_artifact_status: Option<String> = None;
    let mut runtime_verification_reasons: Vec<String> = Vec::new();

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
            if let Some(status) = verification_status_from_artifacts(tool_call.artifacts.as_ref()) {
                verification_artifact_status = Some(status.to_string());
            }
        }
        if ok {
            push_runtime_verification_reason(tool_call, &mut runtime_verification_reasons);
        }
    }

    EvidenceSignals {
        successful_evidence_tool_calls,
        verification_tool_recorded,
        runtime_verification: RuntimeVerificationSignals {
            required: !runtime_verification_reasons.is_empty(),
            reasons: runtime_verification_reasons,
            verification_artifact_status,
        },
    }
}

fn push_runtime_verification_reason(tool_call: &PersistedTraceToolCall, reasons: &mut Vec<String>) {
    let reason = match tool_call.tool_name.as_str() {
        "edit_file" | "multi_edit" => {
            Some(format!("{} modified source files", tool_call.tool_name))
        }
        "create_file" => Some("create_file created or overwrote files".to_string()),
        "write_note" => Some("write_note created or updated local files".to_string()),
        "run_shell" if artifact_has_file_changes(tool_call.artifacts.as_ref()) => {
            Some("run_shell changed files".to_string())
        }
        _ => None,
    };

    if let Some(reason) = reason {
        if !reasons.iter().any(|existing| existing == &reason) {
            reasons.push(reason);
        }
    }
}

fn verification_status_from_artifacts(artifacts: Option<&serde_json::Value>) -> Option<&str> {
    let artifacts = artifacts?;
    if artifacts.get("kind").and_then(|value| value.as_str()) == Some("verification") {
        return artifacts
            .get("overallStatus")
            .and_then(|value| value.as_str());
    }
    artifacts
        .get("verification")
        .and_then(|value| value.get("overallStatus"))
        .and_then(|value| value.as_str())
}

fn artifact_has_file_changes(artifacts: Option<&serde_json::Value>) -> bool {
    let Some(artifacts) = artifacts else {
        return false;
    };
    artifacts.get("kind").and_then(|value| value.as_str()) == Some("fileChangeSet")
        || artifacts
            .get("fileChanges")
            .and_then(|value| value.as_array())
            .is_some_and(|changes| !changes.is_empty())
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
            | "web_search"
            | "web_research_context"
            | "fetch_url"
            | "read_file"
            | "read_files"
            | "glob_files"
            | "search_files"
            | "grep_files"
            | "code_intelligence"
            | "project_tool"
            | "get_document_info"
            | "search_sessions"
    )
}

pub(super) fn build_task_run_artifacts(
    previous_artifacts: Option<serde_json::Value>,
    verification: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = match previous_artifacts {
        Some(serde_json::Value::Object(map)) => map,
        Some(previous) => {
            let mut map = serde_json::Map::new();
            map.insert("previous".to_string(), previous);
            map
        }
        None => serde_json::Map::new(),
    };
    merged.insert(
        "kind".to_string(),
        serde_json::Value::String("agentTaskArtifacts".to_string()),
    );
    merged.insert(
        "version".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    merged.insert("verification".to_string(), verification.clone());
    serde_json::Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done_tool(tool_name: &str) -> PersistedTraceItem {
        done_tool_with_artifacts(tool_name, None)
    }

    fn done_tool_with_artifacts(
        tool_name: &str,
        artifacts: Option<serde_json::Value>,
    ) -> PersistedTraceItem {
        PersistedTraceItem::Tool {
            tool_call: PersistedTraceToolCall {
                call_id: format!("{tool_name}-1"),
                tool_name: tool_name.to_string(),
                arguments: "{}".to_string(),
                status: "done".to_string(),
                render_kind: ToolRenderKind::Generic,
                capabilities: ToolRunCapabilities {
                    input_streaming: crate::tools::ToolInputStreamingMode::None,
                    render_kind: ToolRenderKind::Generic,
                    read_only: true,
                    destructive: false,
                    concurrency_safe: true,
                    interrupt_behavior: crate::tools::ToolInterruptBehavior::Cancel,
                    resource_keys: Vec::new(),
                },
                content: Some("ok".to_string()),
                is_error: Some(false),
                artifacts,
            },
        }
    }

    fn test_skill(id: &str, name: &str, display_name: &str) -> Skill {
        Skill {
            id: id.to_string(),
            name: name.to_string(),
            description: "Use for test work".to_string(),
            content: "Do test work.".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: true,
            interface: crate::skills::SkillInterfaceMetadata {
                display_name: display_name.to_string(),
                short_description: String::new(),
                icon_small: None,
                icon_large: None,
                default_prompt: None,
            },
            dependencies: crate::skills::SkillDependencies::default(),
            policy: crate::skills::SkillPolicy::default(),
            source_path: Some("bundled/test/SKILL.md".to_string()),
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }
    }

    #[test]
    fn skill_selection_trace_serializes_selected_skill_refs() {
        let mut items = Vec::new();
        append_persisted_trace_skill_selection(
            &mut items,
            &[test_skill(
                "builtin-fiction-writing",
                "fiction-writing",
                "Fiction Writing",
            )],
        );

        let artifacts = build_trace_artifacts(&items).expect("trace artifacts");

        assert_eq!(artifacts["items"][0]["kind"], "skillSelection");
        assert_eq!(
            artifacts["items"][0]["skills"][0]["id"],
            "builtin-fiction-writing"
        );
        assert_eq!(
            artifacts["items"][0]["skills"][0]["displayName"],
            "Fiction Writing"
        );
        assert!(
            artifacts["items"][0]["skills"][0]
                .get("activated")
                .is_none(),
            "plain skill index selections are not activation records"
        );
    }

    #[test]
    fn loaded_skill_trace_marks_skill_refs_activated() {
        let mut items = Vec::new();
        append_persisted_trace_loaded_skills(
            &mut items,
            &[test_skill(
                "builtin-fiction-writing",
                "fiction-writing",
                "Fiction Writing",
            )],
        );

        let artifacts = build_trace_artifacts(&items).expect("trace artifacts");

        assert_eq!(artifacts["items"][0]["kind"], "skillSelection");
        assert_eq!(
            artifacts["items"][0]["skills"][0]["id"],
            "builtin-fiction-writing"
        );
        assert_eq!(artifacts["items"][0]["skills"][0]["activated"], true);
    }

    #[test]
    fn persisted_statuses_carry_semantic_presentation() {
        let mut items = Vec::new();
        append_persisted_trace_status(&mut items, "Visible status", "info");
        append_internal_persisted_trace_status(&mut items, "Internal status", "muted");

        let artifacts = build_trace_artifacts(&items).expect("trace artifacts");

        assert_eq!(artifacts["items"][0]["visibility"], "user");
        assert_eq!(artifacts["items"][0]["displayKind"], "status");
        assert_eq!(artifacts["items"][1]["visibility"], "internal");
        assert_eq!(artifacts["items"][1]["displayKind"], "status");
    }

    #[test]
    fn task_run_artifacts_preserve_selected_skills_when_adding_verification() {
        let previous = serde_json::json!({
            "kind": "agentTaskArtifacts",
            "version": 1,
            "selectedSkills": {
                "kind": "selectedSkills",
                "skills": [{ "id": "builtin-fiction-writing", "name": "fiction-writing" }]
            }
        });
        let verification = serde_json::json!({
            "kind": "verification",
            "overallStatus": "passed",
            "checks": []
        });

        let merged = build_task_run_artifacts(Some(previous), &verification);

        assert_eq!(merged["kind"], "agentTaskArtifacts");
        assert_eq!(
            merged["selectedSkills"]["skills"][0]["id"],
            "builtin-fiction-writing"
        );
        assert_eq!(merged["verification"]["overallStatus"], "passed");
    }

    #[test]
    fn evidence_signals_count_codebase_discovery_and_project_tools() {
        let items = vec![
            done_tool("code_intelligence"),
            done_tool("search_files"),
            done_tool("project_tool"),
        ];

        let signals = evidence_signals_from_trace(&items);

        assert_eq!(signals.successful_evidence_tool_calls, 3);
        assert!(!signals.verification_tool_recorded);
        assert!(!signals.runtime_verification.required);
    }

    #[test]
    fn evidence_signals_require_runtime_verification_for_file_mutations() {
        let items = vec![done_tool("edit_file")];

        let signals = evidence_signals_from_trace(&items);

        assert!(signals.runtime_verification.required);
        assert!(signals
            .runtime_verification
            .reasons
            .iter()
            .any(|reason| reason.contains("edit_file")));
    }

    #[test]
    fn evidence_signals_require_runtime_verification_for_shell_file_changes() {
        let items = vec![done_tool_with_artifacts(
            "run_shell",
            Some(serde_json::json!({
                "kind": "fileChangeSet",
                "fileChanges": [{ "path": "deck.pptx", "operation": "create" }]
            })),
        )];

        let signals = evidence_signals_from_trace(&items);

        assert!(signals.runtime_verification.required);
        assert_eq!(
            signals.runtime_verification.reasons,
            vec!["run_shell changed files".to_string()]
        );
    }

    #[test]
    fn evidence_signals_capture_explicit_verification_artifact_status() {
        let items = vec![done_tool_with_artifacts(
            "record_verification",
            Some(serde_json::json!({
                "kind": "verification",
                "overallStatus": "passed",
                "checks": [{ "name": "tests", "status": "passed" }]
            })),
        )];

        let signals = evidence_signals_from_trace(&items);

        assert!(signals.verification_tool_recorded);
        assert_eq!(
            signals.runtime_verification.verification_artifact_status,
            Some("passed".to_string())
        );
    }
}
