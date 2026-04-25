use std::collections::BTreeSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, Semaphore};

use nexa_core::agent::context::estimate_tool_tokens_for_model;
use nexa_core::agent::{AgentConfig, AgentEvent, AgentExecutor, CancellationToken};
use nexa_core::conversation::memory::estimate_tokens_for_model;
use nexa_core::db::Database;
use nexa_core::error::CoreError;
use nexa_core::llm::{create_provider, CompletionRequest, ContentPart, ProviderConfig, Usage};
use nexa_core::search;
use nexa_core::skills::Skill;
use nexa_core::tools::{Tool, ToolRegistry, ToolResult};

const DESCRIPTION: &str = "Spawn a short-lived subagent to handle an isolated subtask, gather an independent perspective, or critique another result. You can call this tool multiple times in parallel, pass it explicit evidence and acceptance criteria, narrow its source scope or tool access, and then synthesize or adjudicate the returned results yourself.";
const BATCH_DESCRIPTION: &str = "Spawn a batch of short-lived subagents for parallel fan-out research, critique, or comparison. Use this when you want several independent delegated workers launched together under one shared budget and then synthesize or adjudicate them later.";
const JUDGE_DESCRIPTION: &str = "Adjudicate or rank multiple delegated worker results using a structured rubric. Use this after parallel subagents return when you need a separate judging pass instead of asking the main worker to merge results implicitly.";
const MAX_SUBAGENT_DELEGATION_DEPTH: u8 = 1;

struct SubagentToolSpec {
    name: &'static str,
    enabled_by_default: bool,
}

const SUBAGENT_TOOL_SPECS: &[SubagentToolSpec] = &[
    SubagentToolSpec {
        name: "search_knowledge_base",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "read_file",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "retrieve_evidence",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "manage_playbook",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "list_sources",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "list_documents",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "list_dir",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "get_chunk_context",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "fetch_url",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "write_note",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "search_playbooks",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "edit_file",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "submit_feedback",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "get_document_info",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "reindex_document",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "compare_documents",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "manage_source",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "get_statistics",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "search_by_date",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "summarize_document",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "update_plan",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "record_verification",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "spawn_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "spawn_subagent_batch",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "judge_subagent_results",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "compile_document",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "query_knowledge_graph",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "run_health_check",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "archive_output",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "get_related_concepts",
        enabled_by_default: true,
    },
];

pub struct SubagentTool {
    runtime: DelegationRuntime,
}

pub struct SubagentBatchTool {
    runtime: DelegationRuntime,
}

pub struct JudgeSubagentResultsTool {
    runtime: DelegationRuntime,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BudgetSnapshot {
    max_parallel: u32,
    max_calls_per_turn: u32,
    calls_started: u32,
    remaining_calls: u32,
    token_budget: u32,
    tokens_spent: u32,
    tokens_reserved: u32,
    remaining_tokens: u32,
}

#[derive(Debug)]
struct SubagentBudgetState {
    max_parallel: u32,
    max_calls_per_turn: u32,
    token_budget: u32,
    calls_started: u32,
    tokens_spent: u32,
    tokens_reserved: u32,
}

#[derive(Clone)]
struct SubagentBudgetController {
    semaphore: Arc<Semaphore>,
    state: Arc<Mutex<SubagentBudgetState>>,
}

#[derive(Clone)]
pub struct DelegationRuntime {
    provider_config: ProviderConfig,
    base_config: AgentConfig,
    allowed_tools: Option<Vec<String>>,
    allowed_skill_ids: Option<Vec<String>>,
    tool_registry: Arc<StdMutex<Option<ToolRegistry>>>,
    budget: SubagentBudgetController,
    cancel_token: CancellationToken,
    delegation_depth: u8,
}

impl SubagentTool {
    pub fn from_runtime(runtime: DelegationRuntime) -> Self {
        Self { runtime }
    }
}

impl SubagentBatchTool {
    pub fn from_runtime(runtime: DelegationRuntime) -> Self {
        Self { runtime }
    }
}

impl JudgeSubagentResultsTool {
    pub fn from_runtime(runtime: DelegationRuntime) -> Self {
        Self { runtime }
    }
}

impl SubagentBudgetController {
    fn new(config: &AgentConfig) -> Self {
        let max_parallel = config.subagent_max_parallel.unwrap_or(3).clamp(1, 12);
        let max_calls_per_turn = config.subagent_max_calls_per_turn.unwrap_or(6).clamp(1, 32);
        let token_budget = config
            .subagent_token_budget
            .unwrap_or(12_000)
            .clamp(256, 200_000);

        Self {
            semaphore: Arc::new(Semaphore::new(max_parallel as usize)),
            state: Arc::new(Mutex::new(SubagentBudgetState {
                max_parallel,
                max_calls_per_turn,
                token_budget,
                calls_started: 0,
                tokens_spent: 0,
                tokens_reserved: 0,
            })),
        }
    }

    async fn begin_call(
        &self,
        label: &str,
        reserved_tokens: u32,
    ) -> Result<OwnedSemaphorePermit, CoreError> {
        {
            let mut state = self.state.lock().await;
            if state.calls_started >= state.max_calls_per_turn {
                return Err(CoreError::InvalidInput(format!(
                    "Delegated execution budget exhausted: maximum {} calls per turn reached before starting {label}.",
                    state.max_calls_per_turn
                )));
            }
            if state.tokens_spent + state.tokens_reserved + reserved_tokens > state.token_budget {
                return Err(CoreError::InvalidInput(format!(
                    "Delegated execution token budget exhausted before starting {label}. Requested reserve: {reserved_tokens} tokens.",
                )));
            }
            state.calls_started += 1;
            state.tokens_reserved = state.tokens_reserved.saturating_add(reserved_tokens);
        }

        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| CoreError::Internal("delegated execution semaphore closed".into()))
    }

    async fn finish_call(&self, reserved_tokens: u32, usage: &Usage) {
        let mut state = self.state.lock().await;
        state.tokens_reserved = state.tokens_reserved.saturating_sub(reserved_tokens);
        state.tokens_spent = state.tokens_spent.saturating_add(usage.total_tokens);
    }

    async fn release_reservation(&self, reserved_tokens: u32) {
        let mut state = self.state.lock().await;
        state.tokens_reserved = state.tokens_reserved.saturating_sub(reserved_tokens);
    }

    async fn snapshot(&self) -> BudgetSnapshot {
        let state = self.state.lock().await;
        BudgetSnapshot {
            max_parallel: state.max_parallel,
            max_calls_per_turn: state.max_calls_per_turn,
            calls_started: state.calls_started,
            remaining_calls: state.max_calls_per_turn.saturating_sub(state.calls_started),
            token_budget: state.token_budget,
            tokens_spent: state.tokens_spent,
            tokens_reserved: state.tokens_reserved,
            remaining_tokens: state
                .token_budget
                .saturating_sub(state.tokens_spent.saturating_add(state.tokens_reserved)),
        }
    }
}

impl DelegationRuntime {
    pub fn new(
        provider_config: ProviderConfig,
        base_config: AgentConfig,
        allowed_tools: Option<Vec<String>>,
        allowed_skill_ids: Option<Vec<String>>,
        cancel_token: CancellationToken,
    ) -> Self {
        let budget = SubagentBudgetController::new(&base_config);
        Self {
            provider_config,
            base_config,
            allowed_tools,
            allowed_skill_ids,
            tool_registry: Arc::new(StdMutex::new(None)),
            budget,
            cancel_token,
            delegation_depth: 0,
        }
    }

    pub fn set_tool_registry(&self, registry: ToolRegistry) {
        if let Ok(mut slot) = self.tool_registry.lock() {
            *slot = Some(registry);
        }
    }

    fn get_tool_registry(&self) -> Result<ToolRegistry, CoreError> {
        self.tool_registry
            .lock()
            .map_err(|_| {
                CoreError::Internal("delegation runtime tool registry lock poisoned".into())
            })?
            .clone()
            .ok_or_else(|| {
                CoreError::Internal("delegation runtime tool registry not initialized".into())
            })
    }

    fn spawn_child_runtime(&self, cancel_token: CancellationToken) -> Self {
        Self {
            provider_config: self.provider_config.clone(),
            base_config: self.base_config.clone(),
            allowed_tools: self.allowed_tools.clone(),
            allowed_skill_ids: self.allowed_skill_ids.clone(),
            tool_registry: Arc::clone(&self.tool_registry),
            budget: self.budget.clone(),
            cancel_token,
            delegation_depth: self.delegation_depth.saturating_add(1),
        }
    }

    fn can_delegate_further(&self) -> bool {
        self.delegation_depth < MAX_SUBAGENT_DELEGATION_DEPTH
    }
}

#[derive(Debug, Deserialize)]
struct SpawnSubagentArgs {
    task: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    expected_output: Option<String>,
    #[serde(default)]
    max_iterations: Option<u32>,
    #[serde(default)]
    timeout_secs: Option<u32>,
    #[serde(default)]
    acceptance_criteria: Option<Vec<String>>,
    #[serde(default)]
    evidence_chunk_ids: Option<Vec<String>>,
    #[serde(default)]
    source_ids: Option<Vec<String>>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    parallel_group: Option<String>,
    #[serde(default)]
    deliverable_style: Option<String>,
    #[serde(default)]
    return_sections: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BatchSubagentTaskArgs {
    #[serde(default)]
    id: Option<String>,
    task: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    expected_output: Option<String>,
    #[serde(default)]
    max_iterations: Option<u32>,
    #[serde(default)]
    timeout_secs: Option<u32>,
    #[serde(default)]
    acceptance_criteria: Option<Vec<String>>,
    #[serde(default)]
    evidence_chunk_ids: Option<Vec<String>>,
    #[serde(default)]
    source_ids: Option<Vec<String>>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    parallel_group: Option<String>,
    #[serde(default)]
    deliverable_style: Option<String>,
    #[serde(default)]
    return_sections: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct SpawnSubagentBatchArgs {
    tasks: Vec<BatchSubagentTaskArgs>,
    #[serde(default)]
    batch_goal: Option<String>,
    #[serde(default)]
    parallel_group: Option<String>,
    #[serde(default)]
    max_parallel: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JudgeCandidateArgs {
    id: String,
    #[serde(default)]
    label: Option<String>,
    result: String,
    #[serde(default)]
    evidence_summary: Option<String>,
    #[serde(default)]
    concerns: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct JudgeSubagentResultsArgs {
    candidates: Vec<JudgeCandidateArgs>,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    rubric: Option<Vec<String>>,
    #[serde(default)]
    decision_mode: Option<String>,
    #[serde(default)]
    required_winner_count: Option<u32>,
    #[serde(default)]
    expected_output: Option<String>,
    #[serde(default)]
    parallel_group: Option<String>,
}

#[derive(Default)]
struct EventCapture {
    usage_total: Usage,
    finish_reason: Option<String>,
    tool_events: Vec<serde_json::Value>,
    thinking: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct EvidenceHandoffItem {
    chunk_id: String,
    path: String,
    title: String,
    excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedSkillRef {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubagentRunArtifact {
    id: String,
    status: String,
    task: String,
    role: Option<String>,
    expected_output: Option<String>,
    acceptance_criteria: Option<Vec<String>>,
    evidence_chunk_ids: Option<Vec<String>>,
    evidence_handoff: Vec<EvidenceHandoffItem>,
    requested_source_scope: Option<Vec<String>>,
    effective_source_scope: Vec<String>,
    requested_allowed_tools: Option<Vec<String>>,
    allowed_tools: Vec<String>,
    allowed_skills: Vec<AppliedSkillRef>,
    parallel_group: Option<String>,
    deliverable_style: Option<String>,
    return_sections: Option<Vec<String>>,
    result: String,
    finish_reason: Option<String>,
    usage_total: Usage,
    tool_events: Vec<serde_json::Value>,
    thinking: Option<Vec<String>>,
    source_scope_applied: bool,
    is_error: bool,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JudgeDecisionArtifact {
    kind: &'static str,
    task: Option<String>,
    rubric: Option<Vec<String>>,
    decision_mode: String,
    expected_output: Option<String>,
    parallel_group: Option<String>,
    winner_ids: Vec<String>,
    confidence: Option<String>,
    summary: String,
    rationale: Option<String>,
    raw_response: String,
    candidates: Vec<JudgeCandidateArgs>,
    usage_total: Usage,
    budget: BudgetSnapshot,
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn normalize_string_list(value: Option<Vec<String>>, limit: usize) -> Option<Vec<String>> {
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();

    for item in value.unwrap_or_default() {
        let trimmed = item.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        normalized.push(trimmed.to_string());
        if normalized.len() >= limit {
            break;
        }
    }

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn truncate_excerpt(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let mut cut = max_chars;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let trimmed = content[..cut].trim_end();
    format!("{trimmed}...[truncated]")
}

fn applied_skills(skills: &[Skill]) -> Vec<AppliedSkillRef> {
    skills
        .iter()
        .map(|skill| AppliedSkillRef {
            id: skill.id.clone(),
            name: skill.name.clone(),
        })
        .collect()
}

fn filter_enabled_skills(skills: Vec<Skill>, allowed_skill_ids: Option<&[String]>) -> Vec<Skill> {
    match allowed_skill_ids {
        Some(ids) => {
            let allowed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            skills
                .into_iter()
                .filter(|skill| allowed.contains(skill.id.as_str()))
                .collect()
        }
        None => skills,
    }
}

fn build_subagent_system_prompt(base_prompt: &str, role: Option<&str>) -> String {
    let mut prompt = base_prompt.trim().to_string();
    prompt.push_str("\n\n## Subagent Instructions\n\n");
    prompt.push_str(
        "You are a short-lived worker spawned by another agent. Focus only on the delegated subtask. Keep your work scoped, use tools only when they materially help, and return a compact result for the supervisor agent rather than addressing the end user directly.",
    );
    prompt.push_str(
        "\n\nTreat supervisor-provided acceptance criteria as requirements. If explicit evidence handoff is provided, ground your answer in that evidence before doing broader retrieval. If you are one of several parallel workers, produce an independent result instead of speculating about what sibling workers might find.",
    );

    if let Some(role) = role.map(str::trim).filter(|value| !value.is_empty()) {
        prompt.push_str("\n\n## Assigned Role\n\n");
        prompt.push_str(role);
    }

    prompt
}

fn build_return_sections(args: &SpawnSubagentArgs) -> Vec<String> {
    normalize_string_list(args.return_sections.clone(), 8).unwrap_or_else(|| {
        vec![
            "Conclusion".to_string(),
            "Key evidence or reasoning".to_string(),
            "Risks or open questions".to_string(),
        ]
    })
}

fn resolve_source_scope(
    parent_scope: &[String],
    requested_scope: Option<&[String]>,
) -> Vec<String> {
    match requested_scope {
        Some(requested) if !requested.is_empty() => {
            if parent_scope.is_empty() {
                requested.to_vec()
            } else {
                let parent: BTreeSet<&str> = parent_scope.iter().map(String::as_str).collect();
                let narrowed: Vec<String> = requested
                    .iter()
                    .filter(|id| parent.contains(id.as_str()))
                    .cloned()
                    .collect();
                if narrowed.is_empty() {
                    parent_scope.to_vec()
                } else {
                    narrowed
                }
            }
        }
        _ => parent_scope.to_vec(),
    }
}

fn resolve_allowed_tools(
    base_allowed_tools: &[String],
    requested_allowed_tools: Option<&[String]>,
) -> Vec<String> {
    match requested_allowed_tools {
        Some(requested) if !requested.is_empty() => {
            let allowed: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
            let narrowed: Vec<String> = requested
                .iter()
                .filter(|name| allowed.contains(name.as_str()))
                .cloned()
                .collect();
            if narrowed.is_empty() {
                base_allowed_tools.to_vec()
            } else {
                narrowed
            }
        }
        _ => base_allowed_tools.to_vec(),
    }
}

fn build_evidence_handoff(db: &Database, chunk_ids: Option<&[String]>) -> Vec<EvidenceHandoffItem> {
    chunk_ids
        .unwrap_or(&[])
        .iter()
        .take(8)
        .filter_map(|chunk_id| {
            let card = search::get_evidence_card(db, chunk_id).ok()?;
            Some(EvidenceHandoffItem {
                chunk_id: card.chunk_id.to_string(),
                path: card.document_path,
                title: card.document_title,
                excerpt: truncate_excerpt(&card.content, 1400),
            })
        })
        .collect()
}

fn build_subagent_request(
    args: &SpawnSubagentArgs,
    effective_source_scope: &[String],
    effective_allowed_tools: &[String],
    allowed_skills: &[AppliedSkillRef],
    evidence_handoff: &[EvidenceHandoffItem],
) -> String {
    let sections = build_return_sections(args);
    let mut request = String::from(
        "Complete the delegated task below. If information is missing, make the smallest reasonable assumption, state it briefly, and continue.\n\n## Supervisor Handoff Packet\n",
    );
    request.push_str("```json\n");
    request.push_str(
        &serde_json::to_string_pretty(&serde_json::json!({
            "task": args.task.trim(),
            "role": args.role,
            "parallelGroup": args.parallel_group,
            "expectedOutput": args.expected_output,
            "deliverableStyle": args.deliverable_style,
            "requiredSections": sections,
            "acceptanceCriteria": args.acceptance_criteria,
            "sourceScope": effective_source_scope,
            "allowedTools": effective_allowed_tools,
            "allowedSkills": allowed_skills,
            "evidenceChunkIds": args.evidence_chunk_ids,
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    request.push_str("\n```\n\n## Delegated Task\n");
    request.push_str(args.task.trim());

    if let Some(role) = args
        .role
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\nRequested perspective:\n");
        request.push_str(role);
    }

    if let Some(group) = args
        .parallel_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\nParallel group:\n");
        request.push_str(group);
        request.push_str(
            "\nTreat this as an independent branch of work. Do not assume what sibling workers will conclude.",
        );
    }

    if let Some(context) = args
        .context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Supervisor Context\n");
        request.push_str(&truncate_excerpt(context, 4_000));
    }

    if let Some(expected_output) = args
        .expected_output
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Desired Output\n");
        request.push_str(expected_output);
    }

    if let Some(style) = args
        .deliverable_style
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("\n\n## Deliverable Style\n");
        request.push_str(style);
    }

    if let Some(criteria) = args
        .acceptance_criteria
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        request.push_str("\n\n## Acceptance Criteria\n");
        for item in criteria {
            request.push_str("- ");
            request.push_str(item);
            request.push('\n');
        }
    }

    if !effective_source_scope.is_empty() {
        request.push_str("\n## Source Scope Restriction\n");
        for source_id in effective_source_scope {
            request.push_str("- ");
            request.push_str(source_id);
            request.push('\n');
        }
    }

    if !effective_allowed_tools.is_empty() {
        request.push_str("\n## Delegated Tool Access\n");
        for tool_name in effective_allowed_tools {
            request.push_str("- ");
            request.push_str(tool_name);
            request.push('\n');
        }
    }

    if !allowed_skills.is_empty() {
        request.push_str("\n## Delegated Skills\n");
        for skill in allowed_skills {
            request.push_str("- ");
            request.push_str(&skill.name);
            request.push_str(" (");
            request.push_str(&skill.id);
            request.push_str(")\n");
        }
    }

    if !evidence_handoff.is_empty() {
        request.push_str("\n## Evidence Handoff\n");
        for evidence in evidence_handoff {
            request.push_str(&format!(
                "\n--- Evidence ---\n[chunk_id: {}]\nPath: {}\nTitle: {}\nExcerpt:\n{}\n",
                evidence.chunk_id, evidence.path, evidence.title, evidence.excerpt
            ));
        }
    }

    request.push_str("\n\n## Response Contract\nReturn a concise result with these sections:\n");
    for (index, section) in sections.iter().enumerate() {
        request.push_str(&format!("{}. {}\n", index + 1, section));
    }
    request.push_str(
        "\nGround claims in the handed-off evidence or retrieved data. If source scope or tool access prevents certainty, state that plainly instead of guessing.",
    );

    request
}

fn normalize_spawn_args(mut args: SpawnSubagentArgs) -> Result<SpawnSubagentArgs, CoreError> {
    args.task = args.task.trim().to_string();
    if args.task.is_empty() {
        return Err(CoreError::InvalidInput(
            "spawn_subagent requires a non-empty task".into(),
        ));
    }

    args.role = trim_optional(args.role);
    args.context = trim_optional(args.context);
    args.expected_output = trim_optional(args.expected_output);
    args.parallel_group = trim_optional(args.parallel_group);
    args.deliverable_style = trim_optional(args.deliverable_style);
    args.timeout_secs = args.timeout_secs.map(|value| value.clamp(15, 180));
    args.acceptance_criteria = normalize_string_list(args.acceptance_criteria.take(), 8);
    args.evidence_chunk_ids = normalize_string_list(args.evidence_chunk_ids.take(), 8);
    args.source_ids = normalize_string_list(args.source_ids.take(), 16);
    args.allowed_tools = normalize_string_list(args.allowed_tools.take(), 16);
    args.return_sections = normalize_string_list(args.return_sections.take(), 8);
    Ok(args)
}

fn normalize_batch_task_args(
    task: BatchSubagentTaskArgs,
) -> Result<(Option<String>, SpawnSubagentArgs), CoreError> {
    let worker_id = trim_optional(task.id);
    let args = normalize_spawn_args(SpawnSubagentArgs {
        task: task.task,
        role: task.role,
        context: task.context,
        expected_output: task.expected_output,
        max_iterations: task.max_iterations,
        timeout_secs: task.timeout_secs,
        acceptance_criteria: task.acceptance_criteria,
        evidence_chunk_ids: task.evidence_chunk_ids,
        source_ids: task.source_ids,
        allowed_tools: task.allowed_tools,
        parallel_group: task.parallel_group,
        deliverable_style: task.deliverable_style,
        return_sections: task.return_sections,
    })?;
    Ok((worker_id, args))
}

async fn run_subagent_once(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
) -> Result<SubagentRunArtifact, CoreError> {
    if runtime.delegation_depth >= MAX_SUBAGENT_DELEGATION_DEPTH {
        return Err(CoreError::InvalidInput(format!(
            "Recursive delegated execution is blocked beyond depth {}.",
            MAX_SUBAGENT_DELEGATION_DEPTH
        )));
    }

    let worker_cancel_token = runtime.cancel_token.child_token();

    let provider = create_provider(runtime.provider_config.clone())
        .map_err(|e| CoreError::Llm(e.to_string()))?;

    let mut config = runtime.base_config.clone();
    config.max_iterations = args.max_iterations.unwrap_or(3).clamp(1, 6);
    config.max_tokens = Some(config.max_tokens.unwrap_or(2048).min(2048));
    let timeout_secs = estimate_subagent_timeout_secs(&runtime, &args);
    config.agent_timeout_secs = Some(timeout_secs as u32);
    config.system_prompt =
        build_subagent_system_prompt(&config.system_prompt, args.role.as_deref());

    let available_tool_names = runtime.get_tool_registry()?.tool_names();
    let baseline_allowed_tools =
        normalize_allowed_tools(runtime.allowed_tools.as_deref(), &available_tool_names);
    let mut effective_allowed_tools =
        resolve_allowed_tools(&baseline_allowed_tools, args.allowed_tools.as_deref());
    if !runtime.can_delegate_further() {
        effective_allowed_tools.retain(|name| !is_subagent_tool_name(name));
    }
    let effective_source_scope =
        resolve_source_scope(&inherited_source_scope, args.source_ids.as_deref());
    let evidence_handoff = build_evidence_handoff(&db, args.evidence_chunk_ids.as_deref());
    let selected_skill_query = format!(
        "{}\n{}",
        args.task,
        args.context.clone().unwrap_or_default()
    );
    let enabled_skills = nexa_core::skills::select_skills_from_pool(
        filter_enabled_skills(
            {
                let mut combined = nexa_core::skills::load_builtin_skills();
                combined.extend(db.get_enabled_skills().unwrap_or_default());
                combined
            },
            runtime.allowed_skill_ids.as_deref(),
        ),
        &selected_skill_query,
        5,
    );
    let applied_skill_refs = applied_skills(&enabled_skills);
    let tools =
        build_subagent_executor_tools(&runtime, &effective_allowed_tools, &worker_cancel_token)?;
    let request_text = build_subagent_request(
        &args,
        &effective_source_scope,
        &effective_allowed_tools,
        &applied_skill_refs,
        &evidence_handoff,
    );
    let reserved_tokens = estimate_reserved_tokens(&config, &request_text, &tools);
    let _permit = runtime
        .budget
        .begin_call(&call_label, reserved_tokens)
        .await?;

    let executor = AgentExecutor::new(provider, tools, config)
        .with_cancel_token(worker_cancel_token.clone())
        .with_skills_override(enabled_skills);

    let (tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let event_task = tokio::spawn(async move {
        let mut capture = EventCapture::default();

        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::ToolCallStart {
                    call_id,
                    tool_name,
                    arguments,
                } => capture.tool_events.push(serde_json::json!({
                    "phase": "start",
                    "callId": call_id,
                    "toolName": tool_name,
                    "arguments": arguments,
                })),
                AgentEvent::ToolCallResult {
                    call_id,
                    tool_name,
                    content,
                    is_error,
                    artifacts,
                } => capture.tool_events.push(serde_json::json!({
                    "phase": "result",
                    "callId": call_id,
                    "toolName": tool_name,
                    "content": content,
                    "isError": is_error,
                    "artifacts": artifacts,
                })),
                AgentEvent::Thinking { content } => {
                    if !content.trim().is_empty() {
                        capture.thinking.push(content);
                    }
                }
                AgentEvent::Status { content, tone } => {
                    if !content.trim().is_empty() {
                        capture.tool_events.push(serde_json::json!({
                            "phase": "status",
                            "content": content,
                            "tone": tone,
                        }));
                    }
                }
                AgentEvent::UsageUpdate { usage_total, .. } => {
                    capture.usage_total = usage_total;
                }
                AgentEvent::Done {
                    usage_total,
                    finish_reason,
                    ..
                } => {
                    capture.usage_total = usage_total;
                    capture.finish_reason = finish_reason;
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::Error { .. }
                | AgentEvent::AutoCompacted { .. }
                | AgentEvent::ToolCallArgsDelta { .. }
                | AgentEvent::ToolCallProgress { .. }
                | AgentEvent::ApprovalRequested { .. }
                | AgentEvent::ApprovalResolved { .. } => {}
            }
        }

        capture
    });

    let final_result = tokio::select! {
        _ = worker_cancel_token.cancelled() => Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' was cancelled by the parent turn."
        ))),
        result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            executor.run_with_source_scope(
                Vec::new(),
                vec![ContentPart::Text { text: request_text }],
                &db,
                None,
                None,
                Some(effective_source_scope.clone()),
                tx,
                0,
            )
        ) => match result {
            Ok(run) => run,
            Err(_) => {
                worker_cancel_token.cancel();
                Err(CoreError::Agent(format!(
                    "Delegated execution '{call_label}' timed out after {timeout_secs}s."
                )))
            }
        }
    };

    let capture = event_task.await.unwrap_or_default();
    runtime
        .budget
        .finish_call(reserved_tokens, &capture.usage_total)
        .await;
    let final_message = final_result?;

    let result_text = final_message.text_content().trim().to_string();
    let result_text = if result_text.is_empty() {
        "(Subagent returned no text.)".to_string()
    } else {
        result_text
    };
    let source_scope_applied = !inherited_source_scope.is_empty()
        || args
            .source_ids
            .as_deref()
            .is_some_and(|ids| !ids.is_empty());

    Ok(SubagentRunArtifact {
        id: worker_id.unwrap_or(call_label),
        status: "done".to_string(),
        task: args.task,
        role: args.role,
        expected_output: args.expected_output,
        acceptance_criteria: args.acceptance_criteria,
        evidence_chunk_ids: args.evidence_chunk_ids,
        evidence_handoff,
        requested_source_scope: args.source_ids,
        effective_source_scope,
        requested_allowed_tools: args.allowed_tools,
        allowed_tools: effective_allowed_tools,
        allowed_skills: applied_skill_refs,
        parallel_group: args.parallel_group,
        deliverable_style: args.deliverable_style,
        return_sections: args.return_sections,
        result: result_text,
        finish_reason: capture.finish_reason,
        usage_total: capture.usage_total,
        tool_events: capture.tool_events,
        thinking: if capture.thinking.is_empty() {
            None
        } else {
            Some(capture.thinking)
        },
        source_scope_applied,
        is_error: false,
        error_message: None,
    })
}

fn summarize_subagent_run(run: &SubagentRunArtifact) -> String {
    let role_suffix = run
        .role
        .as_deref()
        .map(|role| format!(" ({role})"))
        .unwrap_or_default();
    format!(
        "{}{}: {}",
        run.task,
        role_suffix,
        truncate_excerpt(&run.result, 220)
    )
}

fn extract_json_block(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))?
        .trim();
    fenced.strip_suffix("```").map(str::trim)
}

fn build_judge_system_prompt(base_prompt: &str) -> String {
    let mut prompt = base_prompt.trim().to_string();
    prompt.push_str("\n\n## Adjudicator Instructions\n\n");
    prompt.push_str(
        "You are an adjudicator reviewing delegated worker outputs. Compare candidates strictly against the supplied rubric and return a compact, structured judgement. Do not invent evidence beyond the candidate content you were given.",
    );
    prompt
}

fn build_judge_request(args: &JudgeSubagentResultsArgs) -> String {
    let mut request = String::new();
    if let Some(task) = args
        .task
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Adjudication task:\n");
        request.push_str(task);
        request.push_str("\n\n");
    }
    request.push_str("Decision mode:\n");
    request.push_str(
        args.decision_mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("single_best"),
    );
    request.push_str("\n\n");

    if let Some(expected_output) = args
        .expected_output
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Expected output:\n");
        request.push_str(expected_output);
        request.push_str("\n\n");
    }

    if let Some(group) = args
        .parallel_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.push_str("Parallel group:\n");
        request.push_str(group);
        request.push_str("\n\n");
    }

    if let Some(rubric) = args.rubric.as_ref().filter(|items| !items.is_empty()) {
        request.push_str("Rubric:\n");
        for item in rubric {
            request.push_str("- ");
            request.push_str(item);
            request.push('\n');
        }
        request.push('\n');
    }

    request.push_str("Candidates:\n");
    for candidate in &args.candidates {
        request.push_str(&format!(
            "\n--- Candidate {} ---\n",
            candidate.label.as_deref().unwrap_or(&candidate.id)
        ));
        request.push_str(&format!("id: {}\n", candidate.id));
        request.push_str("result:\n");
        request.push_str(candidate.result.trim());
        request.push('\n');
        if let Some(evidence_summary) = candidate
            .evidence_summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request.push_str("evidence summary:\n");
            request.push_str(evidence_summary);
            request.push('\n');
        }
        if let Some(concerns) = candidate
            .concerns
            .as_ref()
            .filter(|items| !items.is_empty())
        {
            request.push_str("concerns:\n");
            for concern in concerns {
                request.push_str("- ");
                request.push_str(concern);
                request.push('\n');
            }
        }
    }

    let required_winners = args.required_winner_count.unwrap_or(1).clamp(1, 4);
    request.push_str(
        "\nReturn ONLY JSON with this shape:\n{\"winnerIds\":[\"candidate-id\"],\"confidence\":\"high|medium|low\",\"summary\":\"short final recommendation\",\"rationale\":\"why these candidates won\"}\n",
    );
    request.push_str(&format!(
        "Select exactly {required_winners} winner id(s) unless the evidence clearly supports a tie."
    ));
    request
}

fn default_subagent_tool_names() -> Vec<String> {
    SUBAGENT_TOOL_SPECS
        .iter()
        .filter(|spec| spec.enabled_by_default)
        .map(|spec| spec.name.to_string())
        .collect()
}

fn canonical_tool_name(name: &str) -> &str {
    match name {
        "compare" => "compare_documents",
        "date_search" => "search_by_date",
        other => other,
    }
}

fn normalize_allowed_tools(
    allowed_tools: Option<&[String]>,
    available_tool_names: &[String],
) -> Vec<String> {
    let available: BTreeSet<&str> = available_tool_names.iter().map(String::as_str).collect();
    match allowed_tools {
        Some(names) => names
            .iter()
            .filter_map(|name| {
                let trimmed = canonical_tool_name(name.trim());
                available.contains(trimmed).then(|| trimmed.to_string())
            })
            .collect(),
        None => default_subagent_tool_names()
            .into_iter()
            .filter(|name| available.contains(name.as_str()))
            .collect(),
    }
}

fn is_subagent_tool_name(name: &str) -> bool {
    matches!(
        name,
        "spawn_subagent" | "spawn_subagent_batch" | "judge_subagent_results"
    )
}

fn resolve_delegation_timeout_secs(config: &AgentConfig, requested: Option<u32>) -> u64 {
    requested.unwrap_or_else(|| {
        let tool_timeout = config.tool_timeout_secs.unwrap_or(30);
        let turn_timeout = config.agent_timeout_secs.unwrap_or(120);
        tool_timeout
            .saturating_mul(2)
            .min(turn_timeout)
            .clamp(15, 120)
    }) as u64
}

fn estimate_subagent_timeout_secs(runtime: &DelegationRuntime, args: &SpawnSubagentArgs) -> u64 {
    resolve_delegation_timeout_secs(&runtime.base_config, args.timeout_secs)
}

fn estimate_reserved_tokens(config: &AgentConfig, request_text: &str, tools: &ToolRegistry) -> u32 {
    let model = config.model.as_deref().unwrap_or("gpt-4o-mini");
    estimate_tokens_for_model(model, &config.system_prompt)
        .saturating_add(estimate_tokens_for_model(model, request_text))
        .saturating_add(estimate_tool_tokens_for_model(model, &tools.definitions()))
        .saturating_add(config.max_tokens.unwrap_or(2048))
}

fn build_subagent_executor_tools(
    runtime: &DelegationRuntime,
    allowed_tool_names: &[String],
    worker_cancel_token: &CancellationToken,
) -> Result<ToolRegistry, CoreError> {
    let filtered = runtime
        .get_tool_registry()?
        .filtered(allowed_tool_names)
        .without_names(&[
            "spawn_subagent",
            "spawn_subagent_batch",
            "judge_subagent_results",
        ]);

    if !runtime.can_delegate_further() {
        return Ok(filtered);
    }

    let child_runtime = runtime.spawn_child_runtime(worker_cancel_token.child_token());
    let mut registry = filtered;
    if allowed_tool_names
        .iter()
        .any(|name| name == "spawn_subagent")
    {
        registry.register(Box::new(SubagentTool::from_runtime(child_runtime.clone())));
    }
    if allowed_tool_names
        .iter()
        .any(|name| name == "spawn_subagent_batch")
    {
        registry.register(Box::new(SubagentBatchTool::from_runtime(
            child_runtime.clone(),
        )));
    }
    if allowed_tool_names
        .iter()
        .any(|name| name == "judge_subagent_results")
    {
        registry.register(Box::new(JudgeSubagentResultsTool::from_runtime(
            child_runtime,
        )));
    }
    Ok(registry)
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The concrete subtask for the delegated agent to complete."
                },
                "role": {
                    "type": "string",
                    "description": "Optional perspective or specialization, for example researcher, critic, planner, or implementer."
                },
                "context": {
                    "type": "string",
                    "description": "Optional context from the supervisor, such as another agent's draft, constraints, or evidence to critique."
                },
                "expected_output": {
                    "type": "string",
                    "description": "Optional description of the format or deliverable you want back."
                },
                "acceptance_criteria": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional checklist the subagent should satisfy before returning."
                },
                "evidence_chunk_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional chunk IDs that should be treated as handed-off evidence from the supervisor."
                },
                "source_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional narrower source scope for this subagent. When omitted, it inherits the supervisor scope."
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional narrower tool whitelist for this subagent. Tool names must be from the delegated allowlist."
                },
                "parallel_group": {
                    "type": "string",
                    "description": "Optional label used when several subagents are exploring sibling branches in parallel."
                },
                "deliverable_style": {
                    "type": "string",
                    "description": "Optional style hint such as critique, plan, comparison, or verification report."
                },
                "return_sections": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional ordered section titles the subagent should use in its response."
                },
                "max_iterations": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 6,
                    "description": "Optional round budget for the subagent. Keep this small."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 15,
                    "maximum": 180,
                    "description": "Optional hard timeout for this delegated worker in seconds."
                }
            },
            "required": ["task"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SpawnSubagentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent arguments: {e}"))
        })?;
        let args = normalize_spawn_args(args)?;
        let run = run_subagent_once(
            self.runtime.clone(),
            db.clone(),
            source_scope.to_vec(),
            call_id.to_string(),
            None,
            args,
        )
        .await?;

        let mut content = String::from("Subagent result");
        if let Some(role) = run.role.as_deref() {
            content.push_str(&format!(" ({role})"));
        }
        content.push_str(":\n");
        content.push_str(&run.result);

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_result",
                "task": run.task,
                "role": run.role,
                "expectedOutput": run.expected_output,
                "acceptanceCriteria": run.acceptance_criteria,
                "evidenceChunkIds": run.evidence_chunk_ids,
                "evidenceHandoff": run.evidence_handoff,
                "requestedSourceScope": run.requested_source_scope,
                "effectiveSourceScope": run.effective_source_scope,
                "requestedAllowedTools": run.requested_allowed_tools,
                "parallelGroup": run.parallel_group,
                "deliverableStyle": run.deliverable_style,
                "returnSections": run.return_sections,
                "result": run.result,
                "finishReason": run.finish_reason,
                "usageTotal": run.usage_total,
                "toolEvents": run.tool_events,
                "thinking": run.thinking,
                "sourceScopeApplied": run.source_scope_applied,
                "allowedTools": run.allowed_tools,
                "allowedSkills": run.allowed_skills,
            })),
        })
    }
}

#[async_trait]
impl Tool for SubagentBatchTool {
    fn name(&self) -> &str {
        "spawn_subagent_batch"
    }

    fn description(&self) -> &str {
        BATCH_DESCRIPTION
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "task": { "type": "string" },
                            "role": { "type": "string" },
                            "context": { "type": "string" },
                            "expected_output": { "type": "string" },
                            "acceptance_criteria": { "type": "array", "items": { "type": "string" } },
                            "evidence_chunk_ids": { "type": "array", "items": { "type": "string" } },
                            "source_ids": { "type": "array", "items": { "type": "string" } },
                            "allowed_tools": { "type": "array", "items": { "type": "string" } },
                            "parallel_group": { "type": "string" },
                            "deliverable_style": { "type": "string" },
                            "return_sections": { "type": "array", "items": { "type": "string" } },
                            "max_iterations": { "type": "integer", "minimum": 1, "maximum": 6 },
                            "timeout_secs": { "type": "integer", "minimum": 15, "maximum": 180 }
                        },
                        "required": ["task"],
                        "additionalProperties": false
                    }
                },
                "batch_goal": { "type": "string" },
                "parallel_group": { "type": "string" },
                "max_parallel": { "type": "integer", "minimum": 1, "maximum": 8 }
            },
            "required": ["tasks"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let mut args: SpawnSubagentBatchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent_batch arguments: {e}"))
        })?;
        args.batch_goal = trim_optional(args.batch_goal);
        args.parallel_group = trim_optional(args.parallel_group);
        if args.tasks.is_empty() {
            return Err(CoreError::InvalidInput(
                "spawn_subagent_batch requires at least one task".into(),
            ));
        }

        let normalized_tasks: Vec<(Option<String>, SpawnSubagentArgs)> = args
            .tasks
            .into_iter()
            .take(8)
            .enumerate()
            .map(|(index, mut task)| {
                if task.parallel_group.is_none() {
                    task.parallel_group = args.parallel_group.clone();
                }
                if task.id.is_none() {
                    task.id = Some(format!("{}-{}", call_id, index + 1));
                }
                normalize_batch_task_args(task)
            })
            .collect::<Result<_, _>>()?;

        let budget_before = self.runtime.budget.snapshot().await;
        let requested_parallel = args
            .max_parallel
            .unwrap_or(budget_before.max_parallel)
            .clamp(1, 8);
        let effective_parallel = requested_parallel.min(budget_before.max_parallel).max(1) as usize;

        let runtime = self.runtime.clone();
        let db = db.clone();
        let inherited_source_scope = source_scope.to_vec();
        let batch_parallel_group = args.parallel_group.clone();
        let runs: Vec<SubagentRunArtifact> = stream::iter(normalized_tasks.into_iter().enumerate())
            .map(|entry: (usize, (Option<String>, SpawnSubagentArgs))| {
                let (index, (worker_id, task_args)) = entry;
                let runtime = runtime.clone();
                let db = db.clone();
                let inherited_source_scope = inherited_source_scope.clone();
                let batch_parallel_group = batch_parallel_group.clone();
                async move {
                    let label = worker_id
                        .clone()
                        .unwrap_or_else(|| format!("{}-{}", call_id, index + 1));
                    let fallback_task = task_args.task.clone();
                    match run_subagent_once(
                        runtime,
                        db,
                        inherited_source_scope,
                        label.clone(),
                        worker_id,
                        task_args,
                    )
                    .await
                    {
                        Ok(run) => run,
                        Err(err) => SubagentRunArtifact {
                            id: label,
                            status: "error".to_string(),
                            task: fallback_task,
                            role: None,
                            expected_output: None,
                            acceptance_criteria: None,
                            evidence_chunk_ids: None,
                            evidence_handoff: Vec::new(),
                            requested_source_scope: None,
                            effective_source_scope: Vec::new(),
                            requested_allowed_tools: None,
                            allowed_tools: Vec::new(),
                            allowed_skills: Vec::new(),
                            parallel_group: batch_parallel_group.clone(),
                            deliverable_style: None,
                            return_sections: None,
                            result: format!("Subagent failed: {err}"),
                            finish_reason: None,
                            usage_total: Usage::default(),
                            tool_events: Vec::new(),
                            thinking: None,
                            source_scope_applied: false,
                            is_error: true,
                            error_message: Some(err.to_string()),
                        },
                    }
                }
            })
            .buffer_unordered(effective_parallel)
            .collect()
            .await;

        let budget_after = self.runtime.budget.snapshot().await;
        let completed_runs = runs.iter().filter(|run| !run.is_error).count();
        let failed_runs = runs.len().saturating_sub(completed_runs);
        let mut content = format!("Completed {} delegated worker(s) in batch", runs.len());
        if let Some(goal) = args.batch_goal.as_deref() {
            content.push_str(&format!(" for: {goal}"));
        }
        content.push_str(".\n\n");
        for run in &runs {
            content.push_str("- ");
            content.push_str(&summarize_subagent_run(run));
            content.push('\n');
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: failed_runs > 0 && completed_runs == 0,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_batch_result",
                "batchGoal": args.batch_goal,
                "parallelGroup": args.parallel_group,
                "requestedMaxParallel": requested_parallel,
                "effectiveMaxParallel": effective_parallel,
                "completedRuns": completed_runs,
                "failedRuns": failed_runs,
                "budgetBefore": budget_before,
                "budgetAfter": budget_after,
                "runs": runs,
            })),
        })
    }
}

#[async_trait]
impl Tool for JudgeSubagentResultsTool {
    fn name(&self) -> &str {
        "judge_subagent_results"
    }

    fn description(&self) -> &str {
        JUDGE_DESCRIPTION
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "candidates": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "label": { "type": "string" },
                            "result": { "type": "string" },
                            "evidence_summary": { "type": "string" },
                            "concerns": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["id", "result"],
                        "additionalProperties": false
                    }
                },
                "task": { "type": "string" },
                "rubric": { "type": "array", "items": { "type": "string" } },
                "decision_mode": { "type": "string" },
                "required_winner_count": { "type": "integer", "minimum": 1, "maximum": 4 },
                "expected_output": { "type": "string" },
                "parallel_group": { "type": "string" }
            },
            "required": ["candidates"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let mut args: JudgeSubagentResultsArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid judge_subagent_results arguments: {e}"))
        })?;
        if args.candidates.len() < 2 {
            return Err(CoreError::InvalidInput(
                "judge_subagent_results requires at least two candidates".into(),
            ));
        }
        args.task = trim_optional(args.task);
        args.expected_output = trim_optional(args.expected_output);
        args.parallel_group = trim_optional(args.parallel_group);
        args.decision_mode = trim_optional(args.decision_mode);
        args.rubric = normalize_string_list(args.rubric.take(), 8);

        let provider = create_provider(self.runtime.provider_config.clone())
            .map_err(|e| CoreError::Llm(e.to_string()))?;
        let model = self
            .runtime
            .base_config
            .summarization_model
            .clone()
            .or_else(|| self.runtime.base_config.model.clone())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let system_prompt = build_judge_system_prompt(&self.runtime.base_config.system_prompt);
        let user_prompt = build_judge_request(&args);
        let reserved_tokens = estimate_tokens_for_model(&model, &system_prompt)
            .saturating_add(estimate_tokens_for_model(&model, &user_prompt))
            .saturating_add(1_200);
        let _permit = self
            .runtime
            .budget
            .begin_call("judge_subagent_results", reserved_tokens)
            .await?;
        let request = CompletionRequest {
            model: model.clone(),
            messages: vec![
                nexa_core::llm::Message::text(nexa_core::llm::Role::System, system_prompt),
                nexa_core::llm::Message::text(nexa_core::llm::Role::User, user_prompt),
            ],
            temperature: Some(0.1),
            max_tokens: Some(1200),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: self.runtime.base_config.provider_type,
            parallel_tool_calls: true,
        };
        let judge_cancel_token = self.runtime.cancel_token.child_token();
        let timeout_secs = resolve_delegation_timeout_secs(&self.runtime.base_config, None);
        let response = tokio::select! {
            _ = judge_cancel_token.cancelled() => {
                self.runtime.budget.release_reservation(reserved_tokens).await;
                return Err(CoreError::Agent(
                    "Delegated adjudication was cancelled by the parent turn.".into()
                ));
            }
            result = tokio::time::timeout(Duration::from_secs(timeout_secs), provider.complete(&request)) => match result {
                Ok(Ok(response)) => {
                    self.runtime.budget.finish_call(reserved_tokens, &response.usage).await;
                    response
                }
                Ok(Err(err)) => {
                    self.runtime.budget.release_reservation(reserved_tokens).await;
                    return Err(err);
                }
                Err(_) => {
                    judge_cancel_token.cancel();
                    self.runtime.budget.release_reservation(reserved_tokens).await;
                    return Err(CoreError::Agent(format!(
                        "Delegated adjudication timed out after {timeout_secs}s."
                    )));
                }
            }
        };

        let raw_response = response.content.trim().to_string();
        let parsed = extract_json_block(&raw_response)
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .unwrap_or_else(|| serde_json::json!({ "summary": raw_response }));

        let winner_ids = parsed
            .get("winnerIds")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let summary = parsed
            .get("summary")
            .and_then(|value| value.as_str())
            .unwrap_or(raw_response.as_str())
            .to_string();
        let rationale = parsed
            .get("rationale")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let confidence = parsed
            .get("confidence")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let budget = self.runtime.budget.snapshot().await;

        let artifact = JudgeDecisionArtifact {
            kind: "subagent_judgement",
            task: args.task,
            rubric: args.rubric,
            decision_mode: args
                .decision_mode
                .unwrap_or_else(|| "single_best".to_string()),
            expected_output: args.expected_output,
            parallel_group: args.parallel_group,
            winner_ids,
            confidence,
            summary: summary.clone(),
            rationale,
            raw_response: raw_response.clone(),
            candidates: args.candidates,
            usage_total: response.usage,
            budget,
        };

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: summary,
            is_error: false,
            artifacts: Some(serde_json::to_value(artifact).unwrap_or_default()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::llm::ProviderType;

    fn test_runtime() -> DelegationRuntime {
        DelegationRuntime::new(
            ProviderConfig {
                provider_type: ProviderType::OpenAi,
                base_url: None,
                api_key: None,
                org_id: None,
                timeout_secs: None,
            },
            AgentConfig::default(),
            None,
            None,
            CancellationToken::new(),
        )
    }

    #[test]
    fn test_normalize_spawn_args_clamps_timeout() {
        let args = normalize_spawn_args(SpawnSubagentArgs {
            task: "Investigate".into(),
            role: None,
            context: None,
            expected_output: None,
            max_iterations: None,
            timeout_secs: Some(999),
            acceptance_criteria: None,
            evidence_chunk_ids: None,
            source_ids: None,
            allowed_tools: None,
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        })
        .unwrap();

        assert_eq!(args.timeout_secs, Some(180));
    }

    #[test]
    fn test_child_runtime_blocks_recursive_delegation() {
        let runtime = test_runtime();
        assert!(runtime.can_delegate_further());

        let child = runtime.spawn_child_runtime(CancellationToken::new());
        assert!(!child.can_delegate_further());
    }

    #[tokio::test]
    async fn test_budget_reservation_prevents_overcommit() {
        let config = AgentConfig {
            subagent_token_budget: Some(256),
            ..Default::default()
        };

        let budget = SubagentBudgetController::new(&config);
        let permit = budget.begin_call("worker-a", 220).await.unwrap();
        let snapshot = budget.snapshot().await;
        assert_eq!(snapshot.tokens_reserved, 220);
        assert_eq!(snapshot.remaining_tokens, 36);

        let second = budget.begin_call("worker-b", 50).await;
        assert!(
            second.is_err(),
            "reservation should block over-budget fanout"
        );

        drop(permit);
        budget.release_reservation(220).await;
        assert_eq!(budget.snapshot().await.tokens_reserved, 0);
    }
}
