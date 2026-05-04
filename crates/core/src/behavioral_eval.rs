//! Deterministic behavioral evals for agent routing and tool exposure.
//!
//! These evals deliberately avoid model calls. They lock product-critical
//! behavior that should not drift during refactors: when to retrieve evidence,
//! when to treat a turn as a file operation, and which tools are safe to offer.

use serde::{Deserialize, Serialize};

use crate::agent::route_name_for_behavioral_eval;
use crate::intelligence::{build_task_plan, EvidenceMode, TaskPlanningInput};
use crate::tools::default_tool_registry;

#[derive(Debug, Clone)]
struct BehavioralEvalCase {
    id: &'static str,
    query: &'static str,
    system_prompt: &'static str,
    has_sources: bool,
    expected_route: &'static str,
    expected_evidence_mode: Option<EvidenceMode>,
    required_tools: &'static [&'static str],
    required_plan_tools: &'static [&'static str],
    forbidden_tools: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BehavioralEvalCaseResult {
    pub id: String,
    pub passed: bool,
    pub route: String,
    pub expected_route: String,
    pub evidence_mode: String,
    pub expected_evidence_mode: Option<String>,
    pub missing_tools: Vec<String>,
    pub missing_plan_tools: Vec<String>,
    pub forbidden_tools_present: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BehavioralEvalReport {
    pub status: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<BehavioralEvalCaseResult>,
}

fn cases() -> Vec<BehavioralEvalCase> {
    vec![
        BehavioralEvalCase {
            id: "knowledge-question-searches-first",
            query: "What changed in my retry notes and why?",
            system_prompt: "",
            has_sources: true,
            expected_route: "KnowledgeRetrieval",
            expected_evidence_mode: Some(EvidenceMode::Required),
            required_tools: &[
                "search_knowledge_base",
                "retrieve_evidence",
                "compare_documents",
                "summarize_document",
            ],
            required_plan_tools: &[
                "search_knowledge_base",
                "retrieve_evidence",
                "record_verification",
            ],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "collection-context-stays-collection-focused",
            query: "Summarize this collection and its evidence.",
            system_prompt:
                "## Collection Context\nTitle: Launch Notes\nSaved evidence: chunk-a, chunk-b",
            has_sources: true,
            expected_route: "CollectionFocused",
            expected_evidence_mode: Some(EvidenceMode::Required),
            required_tools: &["manage_playbook", "search_playbooks", "retrieve_evidence"],
            required_plan_tools: &["search_playbooks", "retrieve_evidence"],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "office-generation-is-file-operation",
            query: "请创建一份 Word 商业计划书",
            system_prompt: "",
            has_sources: false,
            expected_route: "FileOperation",
            expected_evidence_mode: Some(EvidenceMode::Prefer),
            required_tools: &["read_file", "create_file", "edit_file", "run_shell"],
            required_plan_tools: &["read_file", "create_file", "edit_file", "run_shell"],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "file-move-is-file-operation",
            query: "Move notes/today.md to notes/archive/today.md",
            system_prompt: "",
            has_sources: true,
            expected_route: "FileOperation",
            expected_evidence_mode: Some(EvidenceMode::Prefer),
            required_tools: &["list_dir", "read_file", "run_shell"],
            required_plan_tools: &["list_dir", "read_file", "run_shell"],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "source-management-is-operational",
            query: "Reindex this source after I changed files.",
            system_prompt: "",
            has_sources: true,
            expected_route: "SourceManagement",
            expected_evidence_mode: Some(EvidenceMode::NotRequired),
            required_tools: &["manage_source", "reindex_document"],
            required_plan_tools: &["manage_source", "reindex_document"],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "web-url-uses-url-tools",
            query: "Fetch https://example.com and summarize the page.",
            system_prompt: "",
            has_sources: false,
            expected_route: "WebLookup",
            expected_evidence_mode: Some(EvidenceMode::Required),
            required_tools: &["fetch_url"],
            required_plan_tools: &["fetch_url", "record_verification"],
            forbidden_tools: &[],
        },
        BehavioralEvalCase {
            id: "casual-chat-does-not-offer-file-mutation-tools",
            query: "Tell me a quick joke.",
            system_prompt: "",
            has_sources: false,
            expected_route: "DirectResponse",
            expected_evidence_mode: Some(EvidenceMode::NotRequired),
            required_tools: &[],
            required_plan_tools: &[],
            forbidden_tools: &["create_file", "edit_file", "write_note", "run_shell"],
        },
        BehavioralEvalCase {
            id: "persona-text-does-not-pretend-collection-context",
            query: "Say hello in one sentence.",
            system_prompt: "## Active Persona\nInstructions: Prefer saved evidence when it exists.",
            has_sources: false,
            expected_route: "DirectResponse",
            expected_evidence_mode: Some(EvidenceMode::NotRequired),
            required_tools: &[],
            required_plan_tools: &[],
            forbidden_tools: &["create_file", "edit_file", "write_note", "run_shell"],
        },
    ]
}

pub fn run_core_behavioral_eval() -> BehavioralEvalReport {
    let registry = default_tool_registry();
    let mut results = Vec::new();

    for case in cases() {
        let route =
            route_name_for_behavioral_eval(case.query, case.system_prompt, case.has_sources);
        let tools = registry
            .select_tools(case.query, case.has_sources)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let plan = build_task_plan(TaskPlanningInput {
            user_query: case.query,
            route_kind: route,
            has_sources: case.has_sources,
            source_scope_count: if case.has_sources { 1 } else { 0 },
            collection_context: case.system_prompt.contains("## Collection Context"),
        });

        let missing_tools = case
            .required_tools
            .iter()
            .filter(|tool| !tools.iter().any(|actual| actual == **tool))
            .map(|tool| (*tool).to_string())
            .collect::<Vec<_>>();
        let plan_tools = plan
            .steps
            .iter()
            .flat_map(|step| step.required_tools.iter())
            .cloned()
            .collect::<Vec<_>>();
        let missing_plan_tools = case
            .required_plan_tools
            .iter()
            .filter(|tool| !plan_tools.iter().any(|actual| actual == **tool))
            .map(|tool| (*tool).to_string())
            .collect::<Vec<_>>();
        let forbidden_tools_present = case
            .forbidden_tools
            .iter()
            .filter(|tool| tools.iter().any(|actual| actual == **tool))
            .map(|tool| (*tool).to_string())
            .collect::<Vec<_>>();
        let evidence_mode_matches = match case.expected_evidence_mode.as_ref() {
            Some(expected) => *expected == plan.evidence_policy.mode,
            None => true,
        };
        let passed = route == case.expected_route
            && missing_tools.is_empty()
            && missing_plan_tools.is_empty()
            && evidence_mode_matches
            && forbidden_tools_present.is_empty();

        results.push(BehavioralEvalCaseResult {
            id: case.id.to_string(),
            passed,
            route: route.to_string(),
            expected_route: case.expected_route.to_string(),
            evidence_mode: format!("{:?}", plan.evidence_policy.mode),
            expected_evidence_mode: case
                .expected_evidence_mode
                .as_ref()
                .map(|mode| format!("{mode:?}")),
            missing_tools,
            missing_plan_tools,
            forbidden_tools_present,
        });
    }

    let total = results.len();
    let passed = results.iter().filter(|result| result.passed).count();
    let failed = total - passed;

    BehavioralEvalReport {
        status: if failed == 0 { "passed" } else { "failed" }.to_string(),
        total,
        passed,
        failed,
        cases: results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_behavioral_eval_passes() {
        let report = run_core_behavioral_eval();
        assert_eq!(
            report.failed,
            0,
            "behavioral eval failures: {:#?}",
            report
                .cases
                .iter()
                .filter(|case| !case.passed)
                .collect::<Vec<_>>()
        );
    }
}
