//! Deterministic quality evals for product-critical agent contracts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::behavioral_eval::{run_core_behavioral_eval, BehavioralEvalReport};
use crate::conversation::{ConversationMessage, CreateConversationInput};
use crate::db::Database;
use crate::error::CoreError;
use crate::intelligence::{
    build_task_plan, AgentTaskPlan, DelegationMode, EvidenceMode, SourceScopePolicy,
    TaskPlanningInput,
};
use crate::llm::Role;
use crate::rag;
use crate::workflow_catalog::workflow_catalog;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityEvalCheckResult {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityEvalCaseResult {
    pub id: String,
    pub label: String,
    pub severity: String,
    pub passed: bool,
    pub checks: Vec<QualityEvalCheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityEvalSuiteReport {
    pub id: String,
    pub label: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<QualityEvalCaseResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityEvalReport {
    pub status: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub suites: Vec<QualityEvalSuiteReport>,
    pub behavioral_eval: BehavioralEvalReport,
}

pub fn run_agent_quality_eval() -> QualityEvalReport {
    let (behavioral_eval, behavioral_suite) = behavioral_routing_suite();
    let suites = vec![
        behavioral_suite,
        evidence_policy_suite(),
        rag_governance_suite(),
        workflow_catalog_suite(),
        checkpoint_recovery_suite(),
    ];
    let total = suites.iter().map(|suite| suite.total).sum();
    let passed = suites.iter().map(|suite| suite.passed).sum();
    let failed = total - passed;

    QualityEvalReport {
        status: if failed == 0 { "passed" } else { "failed" }.to_string(),
        total,
        passed,
        failed,
        suites,
        behavioral_eval,
    }
}

fn behavioral_routing_suite() -> (BehavioralEvalReport, QualityEvalSuiteReport) {
    let behavioral_eval = run_core_behavioral_eval();
    let cases = behavioral_eval
        .cases
        .iter()
        .map(|case| {
            eval_case(
                &case.id,
                format!("Behavioral route: {}", case.id),
                "high",
                vec![
                    eval_check(
                        "route",
                        case.route == case.expected_route,
                        format!("route={} expected={}", case.route, case.expected_route),
                    ),
                    eval_check(
                        "evidenceMode",
                        case.expected_evidence_mode
                            .as_ref()
                            .map(|expected| expected == &case.evidence_mode)
                            .unwrap_or(true),
                        format!(
                            "evidenceMode={} expected={}",
                            case.evidence_mode,
                            case.expected_evidence_mode
                                .as_deref()
                                .unwrap_or("unspecified")
                        ),
                    ),
                    eval_check(
                        "toolRegistry",
                        case.missing_tools.is_empty(),
                        if case.missing_tools.is_empty() {
                            "required tools are exposed".to_string()
                        } else {
                            format!("missing tools: {}", case.missing_tools.join(", "))
                        },
                    ),
                    eval_check(
                        "taskPlanTools",
                        case.missing_plan_tools.is_empty(),
                        if case.missing_plan_tools.is_empty() {
                            "required plan tools are present".to_string()
                        } else {
                            format!("missing plan tools: {}", case.missing_plan_tools.join(", "))
                        },
                    ),
                    eval_check(
                        "forbiddenTools",
                        case.forbidden_tools_present.is_empty(),
                        if case.forbidden_tools_present.is_empty() {
                            "forbidden tools are absent".to_string()
                        } else {
                            format!(
                                "forbidden tools exposed: {}",
                                case.forbidden_tools_present.join(", ")
                            )
                        },
                    ),
                ],
            )
        })
        .collect::<Vec<_>>();

    (
        behavioral_eval,
        eval_suite("behavioral_routing", "Behavioral routing", cases),
    )
}

fn evidence_policy_suite() -> QualityEvalSuiteReport {
    let knowledge = build_task_plan(TaskPlanningInput {
        user_query: "What changed in my retry notes and why?",
        route_kind: "KnowledgeRetrieval",
        has_sources: true,
        source_scope_count: 2,
        collection_context: false,
    });
    let web = build_task_plan(TaskPlanningInput {
        user_query: "Fetch https://example.com and summarize the page.",
        route_kind: "WebLookup",
        has_sources: false,
        source_scope_count: 0,
        collection_context: false,
    });
    let direct = build_task_plan(TaskPlanningInput {
        user_query: "Say hello in one sentence.",
        route_kind: "DirectResponse",
        has_sources: false,
        source_scope_count: 0,
        collection_context: false,
    });

    eval_suite(
        "evidence_policy",
        "Evidence and execution policy",
        vec![
            eval_case(
                "knowledge-retrieval-requires-verification",
                "Knowledge retrieval requires grounded verification",
                "critical",
                vec![
                    eval_check(
                        "mode",
                        knowledge.evidence_policy.mode == EvidenceMode::Required,
                        format!("mode={:?}", knowledge.evidence_policy.mode),
                    ),
                    eval_check(
                        "citations",
                        knowledge.evidence_policy.require_citations
                            && knowledge.evidence_policy.require_verification
                            && knowledge.evidence_policy.contradiction_check,
                        "citations, verification, and contradiction checks are enforced",
                    ),
                    eval_check(
                        "sourceCount",
                        knowledge.evidence_policy.min_sources >= 2,
                        format!("minSources={}", knowledge.evidence_policy.min_sources),
                    ),
                    eval_check(
                        "scope",
                        knowledge.source_scope_policy == SourceScopePolicy::LinkedSourcesFirst
                            && !knowledge.evidence_policy.allow_web
                            && knowledge.evidence_policy.allow_memory,
                        format!(
                            "scope={:?} allowWeb={} allowMemory={}",
                            knowledge.source_scope_policy,
                            knowledge.evidence_policy.allow_web,
                            knowledge.evidence_policy.allow_memory
                        ),
                    ),
                    eval_check(
                        "requiredTools",
                        plan_has_tool(&knowledge, "search_knowledge_base")
                            && plan_has_tool(&knowledge, "retrieve_evidence")
                            && plan_has_tool(&knowledge, "record_verification"),
                        "search, retrieval, and verification tools are in the task plan",
                    ),
                    eval_check(
                        "ledger",
                        knowledge.ledger.sufficiency == "insufficient"
                            && knowledge.ledger.claims.iter().any(|claim| claim.required),
                        format!("ledger={}", knowledge.ledger.sufficiency),
                    ),
                    eval_check(
                        "delegationJudge",
                        knowledge.delegation.mode != DelegationMode::Disabled
                            && knowledge.delegation.judge_required,
                        format!(
                            "delegation={:?} judgeRequired={}",
                            knowledge.delegation.mode, knowledge.delegation.judge_required
                        ),
                    ),
                ],
            ),
            eval_case(
                "web-lookup-is-cited-and-isolated",
                "Web lookup uses citations without treating memory as evidence",
                "high",
                vec![
                    eval_check(
                        "mode",
                        web.evidence_policy.mode == EvidenceMode::Required,
                        format!("mode={:?}", web.evidence_policy.mode),
                    ),
                    eval_check(
                        "scope",
                        web.source_scope_policy == SourceScopePolicy::WebFirst
                            && web.evidence_policy.allow_web
                            && !web.evidence_policy.allow_memory,
                        format!(
                            "scope={:?} allowWeb={} allowMemory={}",
                            web.source_scope_policy,
                            web.evidence_policy.allow_web,
                            web.evidence_policy.allow_memory
                        ),
                    ),
                    eval_check(
                        "tools",
                        plan_has_tool(&web, "fetch_url")
                            && plan_has_tool(&web, "record_verification"),
                        "fetch and verification tools are present",
                    ),
                    eval_check(
                        "safeguards",
                        web.safeguards
                            .iter()
                            .any(|guard| guard.contains("higher-priority instructions")),
                        "web prompt-injection safeguard is present",
                    ),
                ],
            ),
            eval_case(
                "direct-response-stays-lightweight",
                "Direct response avoids unnecessary evidence and mutation work",
                "medium",
                vec![
                    eval_check(
                        "mode",
                        direct.evidence_policy.mode == EvidenceMode::NotRequired,
                        format!("mode={:?}", direct.evidence_policy.mode),
                    ),
                    eval_check(
                        "budget",
                        direct.tool_budget.max_tool_rounds == 1
                            && direct.tool_budget.prefer_direct_dispatch,
                        format!(
                            "maxToolRounds={} directDispatch={}",
                            direct.tool_budget.max_tool_rounds,
                            direct.tool_budget.prefer_direct_dispatch
                        ),
                    ),
                    eval_check(
                        "delegation",
                        direct.delegation.mode == DelegationMode::Disabled
                            && !direct.delegation.judge_required,
                        format!(
                            "delegation={:?} judgeRequired={}",
                            direct.delegation.mode, direct.delegation.judge_required
                        ),
                    ),
                    eval_check(
                        "mutationSafeguard",
                        direct
                            .safeguards
                            .iter()
                            .any(|guard| guard.contains("mutation tools")),
                        "mutation-tool safeguard is present",
                    ),
                ],
            ),
        ],
    )
}

fn rag_governance_suite() -> QualityEvalSuiteReport {
    let benchmark = rag::saved_rag_benchmark_suite();
    let source_kinds = benchmark
        .iter()
        .map(|case| case.expected_source_kind.as_str())
        .collect::<Vec<_>>();
    let has_multihop = benchmark
        .iter()
        .any(|case| case.tags.iter().any(|tag| tag == "multi_hop"));
    let sample_report = rag::build_rag_eval_report(&[
        rag::RagEvalCase {
            name: "local hit".to_string(),
            query: Some("local".to_string()),
            expected_chunk_ids: vec!["local-a".to_string()],
            retrieved_chunk_ids: vec!["local-a".to_string()],
            expected_sources: vec!["/notes/a.md".to_string()],
            retrieved_sources: vec!["/notes/a.md".to_string()],
            citation_supported: Some(true),
            top_k: 5,
            failure_notes: Vec::new(),
        },
        rag::RagEvalCase {
            name: "web miss".to_string(),
            query: Some("web".to_string()),
            expected_chunk_ids: vec!["web-a".to_string()],
            retrieved_chunk_ids: vec!["other".to_string()],
            expected_sources: vec!["https://example.com/a".to_string()],
            retrieved_sources: vec!["https://example.com/other".to_string()],
            citation_supported: Some(false),
            top_k: 5,
            failure_notes: vec!["source mismatch".to_string()],
        },
    ]);

    eval_suite(
        "rag_governance",
        "RAG evaluation governance",
        vec![
            eval_case(
                "benchmark-suite-covers-source-kinds",
                "RAG benchmark suite covers local, web, and multi-hop retrieval",
                "high",
                vec![
                    eval_check(
                        "localFile",
                        source_kinds.contains(&"local_file"),
                        "local-file benchmark case is present",
                    ),
                    eval_check(
                        "webPage",
                        source_kinds.contains(&"web_page"),
                        "web-page benchmark case is present",
                    ),
                    eval_check(
                        "multiHop",
                        has_multihop,
                        "multi-hop benchmark case is present",
                    ),
                ],
            ),
            eval_case(
                "report-tracks-rag-governance-metrics",
                "RAG eval reports track retrieval and citation governance metrics",
                "high",
                vec![
                    eval_check(
                        "hitAtK",
                        sample_report.hit_rate_at_k >= 0.0,
                        format!("hit@k={:.3}", sample_report.hit_rate_at_k),
                    ),
                    eval_check(
                        "mrr",
                        sample_report.mean_reciprocal_rank >= 0.0,
                        format!("mrr={:.3}", sample_report.mean_reciprocal_rank),
                    ),
                    eval_check(
                        "sourceAccuracy",
                        sample_report.source_accuracy < 1.0,
                        format!("sourceAccuracy={:.3}", sample_report.source_accuracy),
                    ),
                    eval_check(
                        "citationSupport",
                        sample_report.citation_support_rate < 1.0,
                        format!(
                            "citationSupportRate={:.3}",
                            sample_report.citation_support_rate
                        ),
                    ),
                    eval_check(
                        "failureNotes",
                        !sample_report.failure_notes.is_empty(),
                        format!("failureNotes={}", sample_report.failure_notes.len()),
                    ),
                ],
            ),
        ],
    )
}

fn workflow_catalog_suite() -> QualityEvalSuiteReport {
    let catalog = workflow_catalog();
    let ids = catalog
        .iter()
        .map(|template| template.id.as_str())
        .collect::<Vec<_>>();

    let required_template_ids = [
        "research_verify",
        "draft_review",
        "meeting_summary",
        "document_compare",
        "report_brief",
        "connector_background",
    ];
    let required_roles = [
        "researcher",
        "verifier",
        "critic",
        "planner",
        "writer",
        "connector",
    ];
    let roles = catalog
        .iter()
        .flat_map(|template| template.tasks.iter().map(|task| task.role_id.as_str()))
        .collect::<Vec<_>>();
    let acceptance_criteria_count = catalog
        .iter()
        .flat_map(|template| template.tasks.iter())
        .filter(|task| !task.acceptance_criteria.is_empty())
        .count();
    let task_count = catalog
        .iter()
        .map(|template| template.tasks.len())
        .sum::<usize>();

    eval_suite(
        "workflow_catalog",
        "Workflow catalog",
        vec![
            eval_case(
                "catalog-ships-core-workflows",
                "Catalog includes product-ready workflow templates",
                "high",
                required_template_ids
                    .iter()
                    .map(|template_id| {
                        eval_check(
                            *template_id,
                            ids.iter().any(|actual| actual == template_id),
                            format!("template {template_id} is present"),
                        )
                    })
                    .collect(),
            ),
            eval_case(
                "catalog-templates-are-actionable",
                "Every workflow template has actionable prompt and task metadata",
                "high",
                vec![
                    eval_check(
                        "templateCount",
                        catalog.len() >= required_template_ids.len(),
                        format!("templates={}", catalog.len()),
                    ),
                    eval_check(
                        "taskCount",
                        task_count >= catalog.len() * 3,
                        format!("tasks={task_count}"),
                    ),
                    eval_check(
                        "metadata",
                        catalog.iter().all(|template| {
                            !template.label.trim().is_empty()
                                && !template.description.trim().is_empty()
                                && !template.prompt_template.trim().is_empty()
                                && template.max_parallel >= 1
                        }),
                        "labels, descriptions, prompt templates, and parallel budgets are present",
                    ),
                    eval_check(
                        "taskMetadata",
                        catalog
                            .iter()
                            .flat_map(|template| template.tasks.iter())
                            .all(|task| {
                                !task.role_id.trim().is_empty()
                                    && !task.role_label.trim().is_empty()
                                    && !task.task.trim().is_empty()
                                    && !task.expected_output.trim().is_empty()
                                    && !task.deliverable_style.trim().is_empty()
                            }),
                        "each task has role, prompt, expected output, and deliverable style",
                    ),
                    eval_check(
                        "acceptanceCriteria",
                        acceptance_criteria_count == task_count && task_count > 0,
                        format!("tasksWithCriteria={acceptance_criteria_count}/{task_count}"),
                    ),
                ],
            ),
            eval_case(
                "catalog-covers-core-roles",
                "Workflow catalog covers the core delegation roles",
                "medium",
                required_roles
                    .iter()
                    .map(|role_id| {
                        eval_check(
                            *role_id,
                            roles.iter().any(|actual| actual == role_id),
                            format!("role {role_id} is represented"),
                        )
                    })
                    .collect(),
            ),
        ],
    )
}

fn checkpoint_recovery_suite() -> QualityEvalSuiteReport {
    let checks = match checkpoint_recovery_outcome() {
        Ok(outcome) => vec![
            eval_check(
                "branchConversation",
                outcome.branch_conversation_id != outcome.source_conversation_id,
                format!(
                    "source={} branch={}",
                    outcome.source_conversation_id, outcome.branch_conversation_id
                ),
            ),
            eval_check(
                "branchMetadata",
                outcome.branch_provider == "openai"
                    && outcome.branch_model == "gpt-4o"
                    && outcome.branch_system_prompt == "Stay grounded."
                    && outcome.branch_persona_id.as_deref() == Some("researcher")
                    && outcome.branch_title.contains("Original investigation"),
                format!(
                    "provider={} model={} persona={:?} title={}",
                    outcome.branch_provider,
                    outcome.branch_model,
                    outcome.branch_persona_id,
                    outcome.branch_title
                ),
            ),
            eval_check(
                "branchMessages",
                outcome.branch_message_count == 3
                    && outcome.branch_contents
                        == ["Old question", "Old answer", "Current follow-up"],
                format!(
                    "count={} contents={:?}",
                    outcome.branch_message_count, outcome.branch_contents
                ),
            ),
            eval_check(
                "branchSortOrder",
                outcome.branch_sort_order_valid,
                "branch messages are rewritten into contiguous order",
            ),
            eval_check(
                "restoreMessages",
                outcome.restored_contents == ["Old question", "Old answer", "Current follow-up"],
                format!("contents={:?}", outcome.restored_contents),
            ),
            eval_check(
                "restoreDropsSummary",
                !outcome.restored_contains_compaction_summary,
                "compaction summary is replaced with archived messages",
            ),
            eval_check(
                "restoreSortOrder",
                outcome.restored_sort_order_valid,
                "restored messages are rewritten into contiguous order",
            ),
        ],
        Err(err) => vec![eval_check(
            "execution",
            false,
            format!("checkpoint recovery setup failed: {err}"),
        )],
    };

    eval_suite(
        "checkpoint_recovery",
        "Checkpoint recovery and branching",
        vec![eval_case(
            "checkpoint-restore-and-branch",
            "Checkpoint restore and branch preserve recoverable context",
            "critical",
            checks,
        )],
    )
}

fn checkpoint_recovery_outcome() -> Result<CheckpointRecoveryOutcome, CoreError> {
    let db = Database::open_memory()?;
    let conversation = db.create_conversation(&CreateConversationInput {
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        system_prompt: Some("Stay grounded.".to_string()),
        collection_context: None,
        project_id: None,
        persona_id: Some("researcher".to_string()),
    })?;
    db.rename_conversation_by_user(&conversation.id, "Original investigation")?;

    let archived = vec![
        conversation_message(&conversation.id, Role::User, "Old question", 0),
        conversation_message(&conversation.id, Role::Assistant, "Old answer", 1),
    ];
    for message in &archived {
        db.add_message(message)?;
    }

    let checkpoint_id =
        db.create_checkpoint(&conversation.id, "manual", archived.len() as u32, 8)?;
    db.archive_messages(&checkpoint_id, &conversation.id, &archived)?;

    db.delete_messages(&conversation.id)?;
    db.add_message(&conversation_message(
        &conversation.id,
        Role::System,
        "## Earlier conversation context (summarized)\nOld material",
        0,
    ))?;
    db.add_message(&conversation_message(
        &conversation.id,
        Role::User,
        "Current follow-up",
        1,
    ))?;

    let branch = db.branch_checkpoint(&checkpoint_id)?;
    let branch_messages = db.get_messages(&branch.conversation.id)?;
    let restored_messages = db.restore_checkpoint_into_conversation(&checkpoint_id)?;
    let source_conversation_id = conversation.id.clone();
    let branch_conversation_id = branch.conversation.id.clone();

    Ok(CheckpointRecoveryOutcome {
        source_conversation_id: source_conversation_id.clone(),
        branch_conversation_id: branch_conversation_id.clone(),
        branch_title: branch.conversation.title,
        branch_provider: branch.conversation.provider,
        branch_model: branch.conversation.model,
        branch_system_prompt: branch.conversation.system_prompt,
        branch_persona_id: branch.conversation.persona_id,
        branch_message_count: branch.message_count,
        branch_contents: message_contents(&branch_messages),
        branch_sort_order_valid: contiguous_sort_order(&branch_messages, &branch_conversation_id),
        restored_contents: message_contents(&restored_messages),
        restored_contains_compaction_summary: restored_messages.iter().any(is_compaction_summary),
        restored_sort_order_valid: contiguous_sort_order(
            &restored_messages,
            &source_conversation_id,
        ),
    })
}

#[derive(Debug)]
struct CheckpointRecoveryOutcome {
    source_conversation_id: String,
    branch_conversation_id: String,
    branch_title: String,
    branch_provider: String,
    branch_model: String,
    branch_system_prompt: String,
    branch_persona_id: Option<String>,
    branch_message_count: usize,
    branch_contents: Vec<String>,
    branch_sort_order_valid: bool,
    restored_contents: Vec<String>,
    restored_contains_compaction_summary: bool,
    restored_sort_order_valid: bool,
}

fn conversation_message(
    conversation_id: &str,
    role: Role,
    content: &str,
    sort_order: i64,
) -> ConversationMessage {
    ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        role,
        content: content.to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: content.split_whitespace().count() as u32,
        created_at: String::new(),
        sort_order,
        thinking: None,
        image_attachments: None,
    }
}

fn message_contents(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|message| message.content.clone())
        .collect()
}

fn contiguous_sort_order(messages: &[ConversationMessage], conversation_id: &str) -> bool {
    messages.iter().enumerate().all(|(index, message)| {
        message.sort_order == index as i64 && message.conversation_id == conversation_id
    })
}

fn is_compaction_summary(message: &ConversationMessage) -> bool {
    message.role == Role::System
        && message
            .content
            .trim_start()
            .starts_with("## Earlier conversation context (summarized)")
}

fn plan_has_tool(plan: &AgentTaskPlan, tool: &str) -> bool {
    plan.steps
        .iter()
        .flat_map(|step| step.required_tools.iter())
        .any(|required_tool| required_tool == tool)
}

fn eval_check(
    id: impl Into<String>,
    passed: bool,
    detail: impl Into<String>,
) -> QualityEvalCheckResult {
    QualityEvalCheckResult {
        id: id.into(),
        passed,
        detail: detail.into(),
    }
}

fn eval_case(
    id: impl Into<String>,
    label: impl Into<String>,
    severity: impl Into<String>,
    checks: Vec<QualityEvalCheckResult>,
) -> QualityEvalCaseResult {
    let passed = checks.iter().all(|check| check.passed);
    QualityEvalCaseResult {
        id: id.into(),
        label: label.into(),
        severity: severity.into(),
        passed,
        checks,
    }
}

fn eval_suite(
    id: impl Into<String>,
    label: impl Into<String>,
    cases: Vec<QualityEvalCaseResult>,
) -> QualityEvalSuiteReport {
    let total = cases.len();
    let passed = cases.iter().filter(|case| case.passed).count();
    let failed = total - passed;
    QualityEvalSuiteReport {
        id: id.into(),
        label: label.into(),
        total,
        passed,
        failed,
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_quality_eval_passes_and_includes_core_suites() {
        let report = run_agent_quality_eval();
        let suite_ids = report
            .suites
            .iter()
            .map(|suite| suite.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            report.failed,
            0,
            "quality eval failures: {:#?}",
            report
                .suites
                .iter()
                .flat_map(|suite| suite.cases.iter())
                .filter(|case| !case.passed)
                .collect::<Vec<_>>()
        );
        assert!(suite_ids.contains(&"behavioral_routing"));
        assert!(suite_ids.contains(&"evidence_policy"));
        assert!(suite_ids.contains(&"rag_governance"));
        assert!(suite_ids.contains(&"workflow_catalog"));
        assert!(suite_ids.contains(&"checkpoint_recovery"));
        assert_eq!(report.status, "passed");
    }
}
