use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use log::warn;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::delegation_scheduler::{
    BudgetSnapshot, DelegationLimitPolicy, DelegationScheduler as SubagentBudgetController,
};

use nexa_core::agent::context::estimate_tool_tokens_for_model;
use nexa_core::agent::{
    llm_streaming_disabled_by_env, AgentConfig, AgentEvent, AgentExecutor, AgentRequestKind,
    CancellationToken,
};
use nexa_core::conversation::conversation_message_llm_context_content;
use nexa_core::conversation::memory::estimate_tokens_for_model;
use nexa_core::db::Database;
use nexa_core::error::CoreError;
use nexa_core::llm::{
    create_provider, provider_uses_non_streaming_fallback, CompletionRequest, ContentPart, Message,
    ProviderConfig, Role, Usage,
};
use nexa_core::provider_catalog::model_limits_from_catalog;
use nexa_core::search;
use nexa_core::skills::Skill;
use nexa_core::task_run::AgentTaskRuntime;
use nexa_core::task_timeline::TaskTimelineEvent;
use nexa_core::tools::{Tool, ToolCategory, ToolRegistry, ToolResult};
use nexa_core::workflow_catalog::{
    workflow_template_by_id, workflow_template_id_values, WorkflowTemplateDefinition,
};
use nexa_core::workflow_ir::ModelRoutingClass;

static SPAWN_SUBAGENT_DEF: OnceLock<DelegationToolDef> = OnceLock::new();
static SPAWN_SUBAGENT_BATCH_DEF: OnceLock<DelegationToolDef> = OnceLock::new();
static JUDGE_SUBAGENT_RESULTS_DEF: OnceLock<DelegationToolDef> = OnceLock::new();

const SPAWN_SUBAGENT_JSON: &str =
    include_str!("../../../../crates/core/prompts/tools/spawn_subagent.json");
const SPAWN_SUBAGENT_BATCH_JSON: &str =
    include_str!("../../../../crates/core/prompts/tools/spawn_subagent_batch.json");
const JUDGE_SUBAGENT_RESULTS_JSON: &str =
    include_str!("../../../../crates/core/prompts/tools/judge_subagent_results.json");
const MAX_SUBAGENT_DELEGATION_DEPTH: u8 = 1;
const DEFAULT_SUBAGENT_MAX_TOKENS: u32 = 8_192;
const CONSERVATIVE_SUBAGENT_MAX_TOKENS: u32 = 65_536;

struct DelegationToolDef {
    description: String,
    parameters: serde_json::Value,
}

fn delegation_tool_def<'a>(
    lock: &'a OnceLock<DelegationToolDef>,
    json_str: &str,
) -> &'a DelegationToolDef {
    lock.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(json_str).expect("invalid delegated tool JSON definition");
        DelegationToolDef {
            description: value["description"]
                .as_str()
                .expect("delegated tool JSON missing description")
                .to_string(),
            parameters: value["parameters"].clone(),
        }
    })
}

fn spawn_subagent_parameters_schema() -> serde_json::Value {
    let mut schema = delegation_tool_def(&SPAWN_SUBAGENT_DEF, SPAWN_SUBAGENT_JSON)
        .parameters
        .clone();
    schema["properties"]["role_id"]["enum"] = serde_json::json!(role_id_values());
    schema
}

fn spawn_subagent_batch_parameters_schema() -> serde_json::Value {
    let mut schema = delegation_tool_def(&SPAWN_SUBAGENT_BATCH_DEF, SPAWN_SUBAGENT_BATCH_JSON)
        .parameters
        .clone();
    let role_ids = serde_json::json!(role_id_values());
    schema["properties"]["tasks"]["items"]["properties"]["role_id"]["enum"] = role_ids;
    schema["properties"]["workflow_template"]["enum"] =
        serde_json::json!(workflow_template_id_values());
    schema
}

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
        name: "tool_search",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "read_file",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "read_files",
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
        name: "glob_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "search_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "grep_files",
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
        name: "web_search",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "web_research_context",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "browser_evidence_capture",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "desktop_automation",
        enabled_by_default: false,
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
        name: "multi_edit",
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
        name: "observe_subagent_batch",
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

struct SubagentRoleProfile {
    id: &'static str,
    label: &'static str,
    instructions: &'static str,
    default_sections: &'static [&'static str],
    recommended_tools: &'static [&'static str],
    default_max_iterations: u32,
    default_timeout_secs: u32,
}

const ROLE_RESEARCHER_SECTIONS: &[&str] =
    &["Conclusion", "Evidence gathered", "Gaps or uncertainty"];
const ROLE_VERIFIER_SECTIONS: &[&str] =
    &["Verdict", "Checks performed", "Unverified or risky claims"];
const ROLE_CRITIC_SECTIONS: &[&str] = &["Main concerns", "Failure modes", "Suggested fixes"];
const ROLE_PLANNER_SECTIONS: &[&str] = &["Plan", "Dependencies", "Verification gates"];
const ROLE_WRITER_SECTIONS: &[&str] = &["Draft", "Assumptions", "Follow-up edits"];
const ROLE_CONNECTOR_SECTIONS: &[&str] = &["Connector options", "Setup risks", "Recommended path"];
const ROLE_DESKTOP_OPERATOR_SECTIONS: &[&str] =
    &["Action result", "Observed state", "Next safe action"];

const SUBAGENT_ROLE_PROFILES: &[SubagentRoleProfile] = &[
    SubagentRoleProfile {
        id: "researcher",
        label: "Researcher",
        instructions: "Find and summarize relevant evidence. Prefer retrieval before synthesis, distinguish direct evidence from inference, and return only material useful to the supervisor.",
        default_sections: ROLE_RESEARCHER_SECTIONS,
        recommended_tools: &[
            "search_knowledge_base",
            "retrieve_evidence",
            "read_file",
            "read_files",
            "list_sources",
            "list_documents",
            "list_dir",
            "glob_files",
            "search_files",
            "grep_files",
            "get_chunk_context",
            "fetch_url",
            "web_search",
            "web_research_context",
            "search_playbooks",
            "get_document_info",
            "search_by_date",
            "summarize_document",
            "query_knowledge_graph",
            "get_related_concepts",
            "record_verification",
        ],
        default_max_iterations: 3,
        default_timeout_secs: 90,
    },
    SubagentRoleProfile {
        id: "verifier",
        label: "Verifier",
        instructions: "Check whether a proposed answer or plan is supported. Look for missing evidence, stale assumptions, contradictions, and unverifiable claims. Prefer concise pass/fail findings.",
        default_sections: ROLE_VERIFIER_SECTIONS,
        recommended_tools: &[
            "search_knowledge_base",
            "retrieve_evidence",
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "fetch_url",
            "web_search",
            "web_research_context",
            "compare_documents",
            "get_document_info",
            "run_health_check",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 75,
    },
    SubagentRoleProfile {
        id: "critic",
        label: "Critic",
        instructions: "Stress-test the proposed approach. Identify brittle reasoning, missing edge cases, UX or trust risks, and places where the supervisor should simplify or narrow scope.",
        default_sections: ROLE_CRITIC_SECTIONS,
        recommended_tools: &[
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "compare_documents",
            "search_knowledge_base",
            "retrieve_evidence",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 60,
    },
    SubagentRoleProfile {
        id: "planner",
        label: "Planner",
        instructions: "Turn the goal into a practical sequence with dependencies, risk controls, and verification gates. Keep the plan executable and avoid speculative work.",
        default_sections: ROLE_PLANNER_SECTIONS,
        recommended_tools: &[
            "update_plan",
            "search_playbooks",
            "search_knowledge_base",
            "list_sources",
            "list_documents",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 60,
    },
    SubagentRoleProfile {
        id: "writer",
        label: "Writer",
        instructions: "Produce a clean draft or synthesis for the supervisor to adapt. Keep the output grounded in supplied context and note assumptions rather than silently inventing details.",
        default_sections: ROLE_WRITER_SECTIONS,
        recommended_tools: &[
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "retrieve_evidence",
            "search_knowledge_base",
            "search_playbooks",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 75,
    },
    SubagentRoleProfile {
        id: "connector",
        label: "Connector Specialist",
        instructions: "Evaluate external connector or MCP options. Focus on tool availability, lifecycle, credentials, timeout behavior, and safe defaults before recommending setup.",
        default_sections: ROLE_CONNECTOR_SECTIONS,
        recommended_tools: &[
            "list_sources",
            "search_playbooks",
            "search_knowledge_base",
            "fetch_url",
            "web_search",
            "web_research_context",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 75,
    },
    SubagentRoleProfile {
        id: "desktop_operator",
        label: "Desktop Operator",
        instructions: "Plan and perform only narrow user-visible browser or desktop actions. Prefer one small approved action at a time, report what was launched, and never infer private screen state you cannot observe.",
        default_sections: ROLE_DESKTOP_OPERATOR_SECTIONS,
        recommended_tools: &[
            "desktop_automation",
            "fetch_url",
            "read_file",
            "list_dir",
            "record_verification",
        ],
        default_max_iterations: 2,
        default_timeout_secs: 60,
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

pub struct ObserveSubagentBatchTool {
    runtime: DelegationRuntime,
}

struct DelegationBatchState {
    expected_workers: usize,
    results: BTreeMap<usize, SubagentRunArtifact>,
    cancel_tokens: Vec<CancellationToken>,
}

#[derive(Clone)]
pub struct DelegationRuntime {
    provider_config: ProviderConfig,
    base_config: AgentConfig,
    allowed_tools: Option<Vec<String>>,
    allowed_skill_ids: Option<Vec<String>>,
    parent_task_run_id: Option<String>,
    parent_conversation_id: Option<String>,
    tool_registry: Arc<StdMutex<Option<ToolRegistry>>>,
    sessions: Arc<StdMutex<HashMap<String, SubagentSessionSnapshot>>>,
    skill_index: Arc<OnceLock<SkillIndexSnapshot>>,
    context_snapshots: Arc<StdMutex<HashMap<String, Arc<DelegationContextSnapshot>>>>,
    batches: Arc<StdMutex<HashMap<String, DelegationBatchState>>>,
    batch_notify: Arc<tokio::sync::Notify>,
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

impl ObserveSubagentBatchTool {
    pub fn from_runtime(runtime: DelegationRuntime) -> Self {
        Self { runtime }
    }
}

impl DelegationRuntime {
    pub fn new(
        provider_config: ProviderConfig,
        base_config: AgentConfig,
        allowed_tools: Option<Vec<String>>,
        allowed_skill_ids: Option<Vec<String>>,
        cancel_token: CancellationToken,
        parent_task_run_id: Option<String>,
        parent_conversation_id: Option<String>,
    ) -> Self {
        let budget = SubagentBudgetController::new(&base_config);
        Self {
            provider_config,
            base_config,
            allowed_tools,
            allowed_skill_ids,
            parent_task_run_id,
            parent_conversation_id,
            tool_registry: Arc::new(StdMutex::new(None)),
            sessions: Arc::new(StdMutex::new(HashMap::new())),
            skill_index: Arc::new(OnceLock::new()),
            context_snapshots: Arc::new(StdMutex::new(HashMap::new())),
            batches: Arc::new(StdMutex::new(HashMap::new())),
            batch_notify: Arc::new(tokio::sync::Notify::new()),
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
            parent_task_run_id: self.parent_task_run_id.clone(),
            parent_conversation_id: self.parent_conversation_id.clone(),
            tool_registry: Arc::clone(&self.tool_registry),
            sessions: Arc::clone(&self.sessions),
            skill_index: Arc::clone(&self.skill_index),
            context_snapshots: Arc::clone(&self.context_snapshots),
            batches: Arc::clone(&self.batches),
            batch_notify: Arc::clone(&self.batch_notify),
            budget: self.budget.clone(),
            cancel_token,
            delegation_depth: self.delegation_depth.saturating_add(1),
        }
    }

    fn scoped_to_worker(&self, cancel_token: CancellationToken) -> Self {
        Self {
            provider_config: self.provider_config.clone(),
            base_config: self.base_config.clone(),
            allowed_tools: self.allowed_tools.clone(),
            allowed_skill_ids: self.allowed_skill_ids.clone(),
            parent_task_run_id: self.parent_task_run_id.clone(),
            parent_conversation_id: self.parent_conversation_id.clone(),
            tool_registry: Arc::clone(&self.tool_registry),
            sessions: Arc::clone(&self.sessions),
            skill_index: Arc::clone(&self.skill_index),
            context_snapshots: Arc::clone(&self.context_snapshots),
            batches: Arc::clone(&self.batches),
            batch_notify: Arc::clone(&self.batch_notify),
            budget: self.budget.clone(),
            cancel_token,
            delegation_depth: self.delegation_depth,
        }
    }

    fn can_delegate_further(&self) -> bool {
        self.delegation_depth < MAX_SUBAGENT_DELEGATION_DEPTH
    }

    fn get_session_snapshot(&self, task_id: &str) -> Option<SubagentSessionSnapshot> {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(task_id).cloned())
    }

    fn save_session_snapshot(&self, snapshot: SubagentSessionSnapshot) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(snapshot.task_id.clone(), snapshot);
        }
    }

    fn register_batch(&self, batch_id: &str, expected_workers: usize) {
        if let Ok(mut batches) = self.batches.lock() {
            batches.insert(
                batch_id.to_string(),
                DelegationBatchState {
                    expected_workers,
                    results: BTreeMap::new(),
                    cancel_tokens: Vec::with_capacity(expected_workers),
                },
            );
        }
    }

    fn add_batch_cancel_token(&self, batch_id: &str, token: CancellationToken) {
        if let Ok(mut batches) = self.batches.lock() {
            if let Some(batch) = batches.get_mut(batch_id) {
                batch.cancel_tokens.push(token);
            }
        }
    }

    fn record_batch_result(&self, batch_id: &str, index: usize, run: SubagentRunArtifact) {
        if let Ok(mut batches) = self.batches.lock() {
            if let Some(batch) = batches.get_mut(batch_id) {
                batch.results.insert(index, run);
            }
        }
        self.batch_notify.notify_waiters();
    }

    fn batch_snapshot(&self, batch_id: &str) -> Option<(usize, Vec<SubagentRunArtifact>)> {
        self.batches.lock().ok().and_then(|batches| {
            batches.get(batch_id).map(|batch| {
                (
                    batch.expected_workers,
                    batch.results.values().cloned().collect(),
                )
            })
        })
    }

    fn cancel_batch(&self, batch_id: &str) -> bool {
        let Some(tokens) = self.batches.lock().ok().and_then(|batches| {
            batches
                .get(batch_id)
                .map(|batch| batch.cancel_tokens.clone())
        }) else {
            return false;
        };
        for token in tokens {
            token.cancel();
        }
        true
    }

    fn context_snapshot(
        &self,
        db: &Database,
        model: &str,
        context_limit: u32,
    ) -> Arc<DelegationContextSnapshot> {
        let key = format!("{model}:{context_limit}");
        if let Some(snapshot) = self
            .context_snapshots
            .lock()
            .ok()
            .and_then(|snapshots| snapshots.get(&key).cloned())
        {
            return snapshot;
        }
        let snapshot = Arc::new(load_delegation_context_snapshot(
            db,
            self.parent_conversation_id.as_deref(),
            model,
            context_limit,
        ));
        if let Ok(mut snapshots) = self.context_snapshots.lock() {
            return snapshots
                .entry(key)
                .or_insert_with(|| Arc::clone(&snapshot))
                .clone();
        }
        snapshot
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SpawnSubagentArgs {
    task: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    role_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    model_policy: Option<ModelRoutingClass>,
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
    #[serde(default)]
    task_id: Option<String>,
    task: String,
    #[serde(default)]
    role_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    model_policy: Option<ModelRoutingClass>,
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
    #[serde(default)]
    tasks: Vec<BatchSubagentTaskArgs>,
    #[serde(default)]
    batch_goal: Option<String>,
    #[serde(default)]
    workflow_template: Option<String>,
    #[serde(default)]
    parallel_group: Option<String>,
    #[serde(default)]
    max_parallel: Option<u32>,
    #[serde(default)]
    completion_policy: Option<String>,
    #[serde(default)]
    quorum: Option<u32>,
    #[serde(default)]
    deadline_ms: Option<u64>,
    #[serde(default)]
    cancel_remaining: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObserveSubagentBatchArgs {
    batch_id: String,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    cancel_remaining: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum DelegationCompletionPolicy {
    All,
    Quorum { required: usize },
    FirstSuccess,
    Deadline { deadline_ms: u64 },
    ParentDecides,
}

impl DelegationCompletionPolicy {
    fn resolve(args: &SpawnSubagentBatchArgs, worker_count: usize) -> Result<Self, CoreError> {
        let mode = args
            .completion_policy
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("all")
            .to_ascii_lowercase();
        match mode.as_str() {
            "all" => Ok(Self::All),
            "quorum" => {
                let required = args.quorum.unwrap_or_else(|| {
                    u32::try_from(worker_count.saturating_div(2).saturating_add(1))
                        .unwrap_or(u32::MAX)
                }) as usize;
                if required == 0 || required > worker_count {
                    return Err(CoreError::InvalidInput(format!(
                        "spawn_subagent_batch quorum must be between 1 and {worker_count}"
                    )));
                }
                Ok(Self::Quorum { required })
            }
            "first_success" | "firstsuccess" => Ok(Self::FirstSuccess),
            "deadline" => Ok(Self::Deadline {
                deadline_ms: args.deadline_ms.unwrap_or(60_000).clamp(250, 180_000),
            }),
            "parent_decides" | "parentdecides" => Ok(Self::ParentDecides),
            _ => Err(CoreError::InvalidInput(format!(
                "Unsupported spawn_subagent_batch completion_policy '{mode}'"
            ))),
        }
    }

    fn is_satisfied(&self, runs: &[SubagentRunArtifact], pending: usize) -> bool {
        let successes = runs.iter().filter(|run| !run.is_error).count();
        match self {
            Self::All | Self::Deadline { .. } => pending == 0,
            Self::Quorum { required } => successes >= *required,
            Self::FirstSuccess => successes > 0,
            // Return after the first settled result. The parent can then wait
            // for more evidence or cancel residual workers through the
            // observe_subagent_batch decision channel.
            Self::ParentDecides => !runs.is_empty() || pending == 0,
        }
    }
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

#[derive(Debug, Deserialize, Serialize)]
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
    error_message: Option<String>,
}

#[derive(Clone)]
struct SkillIndexSnapshot {
    generation: String,
    skills: Arc<[Skill]>,
}

#[derive(Clone)]
struct DelegationContextSnapshot {
    id: String,
    selected_message_ids: Arc<[String]>,
    messages: Arc<[Message]>,
    token_estimate: u32,
    context_limit: u32,
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
struct SubagentSessionSnapshot {
    task_id: String,
    last_run_id: String,
    task: String,
    role_id: Option<String>,
    role_name: Option<String>,
    result: String,
    finish_reason: Option<String>,
    usage_total: Usage,
    tool_event_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubagentRunArtifact {
    id: String,
    session_id: String,
    resumed_from_task_id: Option<String>,
    previous_session: Option<SubagentSessionSnapshot>,
    status: String,
    task: String,
    role_id: Option<String>,
    role_name: Option<String>,
    role: Option<String>,
    model_policy: Option<ModelRoutingClass>,
    effective_model: Option<String>,
    model_route_fallback: bool,
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

fn subtask_role_label(
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
    fallback: &str,
) -> String {
    role_profile
        .map(|profile| profile.label.to_string())
        .or_else(|| args.role.as_ref().map(|role| role.trim().to_string()))
        .filter(|role| !role.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[allow(clippy::too_many_arguments)]
fn subtask_input_payload(
    kind: &str,
    call_label: &str,
    worker_id: Option<&str>,
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
    effective_source_scope: &[String],
    effective_allowed_tools: &[String],
    applied_skill_refs: &[AppliedSkillRef],
    reserved_tokens: u32,
    timeout_secs: u64,
) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "callLabel": call_label,
        "workerId": worker_id,
        "taskId": &args.task_id,
        "task": &args.task,
        "roleId": role_profile.map(|profile| profile.id),
        "roleName": role_profile.map(|profile| profile.label),
        "role": &args.role,
        "modelPolicy": &args.model_policy,
        "context": &args.context,
        "expectedOutput": &args.expected_output,
        "acceptanceCriteria": &args.acceptance_criteria,
        "evidenceChunkIds": &args.evidence_chunk_ids,
        "requestedSourceScope": &args.source_ids,
        "effectiveSourceScope": effective_source_scope,
        "requestedAllowedTools": &args.allowed_tools,
        "allowedTools": effective_allowed_tools,
        "allowedSkills": applied_skill_refs,
        "parallelGroup": &args.parallel_group,
        "deliverableStyle": &args.deliverable_style,
        "returnSections": &args.return_sections,
        "maxIterations": args.max_iterations,
        "timeoutSecs": timeout_secs,
        "reservedTokens": reserved_tokens,
    })
}

fn record_subtask_event(
    db: &Database,
    parent_run_id: &str,
    label: &str,
    status: &str,
    payload: Option<&serde_json::Value>,
) {
    let timeline_event = TaskTimelineEvent::subtask(label, status, payload);
    if let Err(err) =
        AgentTaskRuntime::new(db).record_timeline_event(parent_run_id, &timeline_event)
    {
        warn!("Failed to record subtask event for {parent_run_id}: {err}");
    }
}

#[allow(clippy::too_many_arguments)]
fn record_subagent_launch_metric(
    db: &Database,
    parent_run_id: &str,
    subtask_run_id: &str,
    call_label: &str,
    stage: &str,
    elapsed_ms: Option<u64>,
    provider_invocation_id: Option<&str>,
    measurement_status: &str,
) {
    record_subtask_event(
        db,
        parent_run_id,
        &format!("Subagent telemetry {stage}: {call_label}"),
        "telemetry",
        Some(&serde_json::json!({
            "kind": "turnLaunchMetric",
            "scope": if provider_invocation_id.is_some() { "provider" } else { "subagent" },
            "stage": stage,
            "elapsedMs": elapsed_ms,
            "measurementStatus": measurement_status,
            "subtaskRunId": subtask_run_id,
            "callLabel": call_label,
            "providerInvocationId": provider_invocation_id,
        })),
    );
}

fn instant_elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn signal_progress_latch(sender: &mpsc::Sender<()>) {
    // Provider-connect and first-response signals are edge-triggered latches,
    // not event logs. Coalesce repeated stream deltas into the single pending
    // slot so a long response cannot grow an unbounded notification queue.
    let _ = sender.try_send(());
}

async fn acquire_batch_slot(
    batch_slots: Arc<tokio::sync::Semaphore>,
    cancel_token: &CancellationToken,
    call_label: &str,
    queue_started: Instant,
    queue_deadline_ms: u64,
) -> Result<tokio::sync::OwnedSemaphorePermit, CoreError> {
    let elapsed_ms = u64::try_from(queue_started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let remaining_ms = queue_deadline_ms.saturating_sub(elapsed_ms);
    if remaining_ms == 0 {
        return Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' exceeded its {queue_deadline_ms}ms queue deadline while waiting for a batch slot."
        )));
    }

    match Arc::clone(&batch_slots).try_acquire_owned() {
        Ok(permit) => return Ok(permit),
        Err(tokio::sync::TryAcquireError::Closed) => {
            return Err(CoreError::Internal(
                "delegated batch semaphore closed".into(),
            ));
        }
        Err(tokio::sync::TryAcquireError::NoPermits) => {}
    }

    tokio::select! {
        _ = cancel_token.cancelled() => Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' was cancelled while waiting for its batch slot."
        ))),
        result = tokio::time::timeout(
            Duration::from_millis(remaining_ms),
            batch_slots.acquire_owned(),
        ) => match result {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(CoreError::Internal(
                "delegated batch semaphore closed".into()
            )),
            Err(_) => Err(CoreError::Agent(format!(
                "Delegated execution '{call_label}' exceeded its {queue_deadline_ms}ms queue deadline while waiting for a batch slot."
            ))),
        },
    }
}

fn finish_subtask_run_best_effort(
    db: &Database,
    subtask_run_id: Option<&str>,
    status: &str,
    output: Option<&serde_json::Value>,
    error_message: Option<&str>,
) {
    if let Some(id) = subtask_run_id {
        if let Err(err) = db.finish_agent_subtask_run(id, status, output, error_message) {
            warn!("Failed to finish subtask run {id}: {err}");
        }
    }
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

fn filter_enabled_skills(skills: &[Skill], allowed_skill_ids: Option<&[String]>) -> Vec<Skill> {
    match allowed_skill_ids {
        Some(ids) => {
            let allowed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            skills
                .iter()
                .filter(|skill| allowed.contains(skill.id.as_str()))
                .cloned()
                .collect()
        }
        None => skills.to_vec(),
    }
}

fn load_skill_index_snapshot(db: &Database) -> SkillIndexSnapshot {
    let mut skills = nexa_core::skills::load_builtin_skills();
    skills.extend(db.get_enabled_skills().unwrap_or_default());
    let encoded = serde_json::to_vec(&skills).unwrap_or_default();
    SkillIndexSnapshot {
        generation: blake3::hash(&encoded).to_hex().to_string(),
        skills: Arc::from(skills),
    }
}

fn load_delegation_context_snapshot(
    db: &Database,
    conversation_id: Option<&str>,
    model: &str,
    context_limit: u32,
) -> DelegationContextSnapshot {
    let token_budget = context_limit.saturating_mul(60) / 100;
    let mut selected = Vec::new();
    let mut token_estimate = 0u32;
    if let Some(conversation_id) = conversation_id {
        if let Ok(messages) = db.get_messages(conversation_id) {
            for message in messages.into_iter().rev() {
                let content = conversation_message_llm_context_content(&message).to_string();
                let message_tokens = estimate_tokens_for_model(model, &content);
                if !selected.is_empty()
                    && token_estimate.saturating_add(message_tokens) > token_budget
                {
                    break;
                }
                token_estimate = token_estimate.saturating_add(message_tokens);
                let is_tool_result = message.role == Role::Tool;
                let role = match &message.role {
                    Role::Tool => Role::User,
                    role => role.clone(),
                };
                let content = if is_tool_result {
                    format!("[Prior tool result]\n{content}")
                } else {
                    content
                };
                let mut projected = Message::text(role, content);
                if message.role == Role::Assistant {
                    projected.reasoning_content = message.thinking;
                }
                selected.push((message.id, projected));
            }
        }
    }
    selected.reverse();
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(&context_limit.to_le_bytes());
    for (id, message) in &selected {
        hasher.update(id.as_bytes());
        hasher.update(message.text_content().as_bytes());
    }
    let (selected_message_ids, messages): (Vec<_>, Vec<_>) = selected.into_iter().unzip();
    DelegationContextSnapshot {
        id: hasher.finalize().to_hex().to_string(),
        selected_message_ids: Arc::from(selected_message_ids),
        messages: Arc::from(messages),
        token_estimate,
        context_limit,
    }
}

fn normalize_role_id(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn role_profile_by_id(role_id: &str) -> Option<&'static SubagentRoleProfile> {
    let normalized = normalize_role_id(role_id);
    SUBAGENT_ROLE_PROFILES
        .iter()
        .find(|profile| profile.id == normalized)
}

fn infer_role_profile(role: Option<&str>) -> Option<&'static SubagentRoleProfile> {
    let text = role?.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    SUBAGENT_ROLE_PROFILES.iter().find(|profile| {
        text.contains(profile.id) || text.contains(&profile.label.to_ascii_lowercase())
    })
}

fn resolve_role_profile(
    role_id: Option<&str>,
    role: Option<&str>,
) -> Result<Option<&'static SubagentRoleProfile>, CoreError> {
    if let Some(raw_id) = role_id.map(str::trim).filter(|value| !value.is_empty()) {
        return role_profile_by_id(raw_id).map(Some).ok_or_else(|| {
            let allowed = SUBAGENT_ROLE_PROFILES
                .iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>()
                .join(", ");
            CoreError::InvalidInput(format!(
                "Unknown subagent role_id '{raw_id}'. Allowed role_id values: {allowed}."
            ))
        });
    }
    Ok(infer_role_profile(role))
}

fn role_id_values() -> Vec<&'static str> {
    SUBAGENT_ROLE_PROFILES
        .iter()
        .map(|profile| profile.id)
        .collect()
}

fn normalize_workflow_template_id(value: Option<String>) -> Result<Option<String>, CoreError> {
    let Some(raw_template) = trim_optional(value) else {
        return Ok(None);
    };
    let normalized = normalize_role_id(&raw_template);
    if workflow_template_by_id(&normalized).is_some() {
        return Ok(Some(normalized));
    }
    let allowed = workflow_template_id_values().join(", ");
    Err(CoreError::InvalidInput(format!(
        "Unknown workflow_template '{raw_template}'. Allowed workflow_template values: {allowed}."
    )))
}

fn expand_workflow_template_tasks(
    template: &WorkflowTemplateDefinition,
    batch_goal: &str,
    parallel_group: Option<&str>,
) -> Vec<BatchSubagentTaskArgs> {
    let shared_context = format!(
        "Workflow template: {} ({})\n{}\n\nOverall batch goal:\n{}",
        template.label,
        template.id,
        template.description,
        batch_goal.trim()
    );
    let group = parallel_group
        .map(str::to_string)
        .unwrap_or_else(|| template.id.to_string());

    template
        .tasks
        .iter()
        .map(|task_template| {
            let profile = role_profile_by_id(task_template.role_id);
            let mut acceptance_criteria = task_template
                .acceptance_criteria
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>();
            acceptance_criteria.push(
                "Tie the result back to the overall batch goal and state unresolved uncertainty."
                    .to_string(),
            );

            BatchSubagentTaskArgs {
                id: Some(format!("{}-{}", template.id, task_template.id)),
                task_id: None,
                task: format!(
                    "Overall goal:\n{}\n\nTemplate step:\n{}",
                    batch_goal.trim(),
                    task_template.task
                ),
                role_id: Some(task_template.role_id.to_string()),
                role: profile.map(|profile| profile.label.to_string()),
                model_policy: None,
                context: Some(shared_context.clone()),
                expected_output: Some(task_template.expected_output.to_string()),
                max_iterations: None,
                timeout_secs: None,
                acceptance_criteria: Some(acceptance_criteria),
                evidence_chunk_ids: None,
                source_ids: None,
                allowed_tools: None,
                parallel_group: Some(group.clone()),
                deliverable_style: Some(task_template.deliverable_style.to_string()),
                return_sections: Some(role_sections(profile)),
            }
        })
        .collect()
}

fn role_sections(profile: Option<&SubagentRoleProfile>) -> Vec<String> {
    profile
        .map(|profile| {
            profile
                .default_sections
                .iter()
                .map(|section| (*section).to_string())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "Conclusion".to_string(),
                "Key evidence or reasoning".to_string(),
                "Risks or open questions".to_string(),
            ]
        })
}

fn build_subagent_system_prompt(
    base_prompt: &str,
    role: Option<&str>,
    role_profile: Option<&SubagentRoleProfile>,
) -> String {
    let mut prompt = base_prompt.trim().to_string();
    prompt.push_str("\n\n## Subagent Instructions\n\n");
    prompt.push_str(
        "You are a short-lived worker spawned by another agent. Focus only on the delegated subtask. Keep your work scoped, use tools only when they materially help, and return a compact result for the supervisor agent rather than addressing the end user directly.",
    );
    prompt.push_str(
        "\n\nTreat supervisor-provided acceptance criteria as requirements. If explicit evidence handoff is provided, ground your answer in that evidence before doing broader retrieval. If you are one of several parallel workers, produce an independent result instead of speculating about what sibling workers might find.",
    );

    if let Some(profile) = role_profile {
        prompt.push_str("\n\n## Role Profile\n\n");
        prompt.push_str("- id: ");
        prompt.push_str(profile.id);
        prompt.push_str("\n- label: ");
        prompt.push_str(profile.label);
        prompt.push_str("\n- instructions: ");
        prompt.push_str(profile.instructions);
        prompt.push_str(
            "\n\nFollow this profile even when the free-form role text is vague. If the profile and free-form role conflict, prefer the profile.",
        );
    }

    if let Some(role) = role.map(str::trim).filter(|value| !value.is_empty()) {
        prompt.push_str("\n\n## Assigned Role\n\n");
        prompt.push_str(role);
    }

    prompt
}

fn build_return_sections(
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
) -> Vec<String> {
    normalize_string_list(args.return_sections.clone(), 8)
        .unwrap_or_else(|| role_sections(role_profile))
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

fn resolve_allowed_tools_for_role(
    base_allowed_tools: &[String],
    requested_allowed_tools: Option<&[String]>,
    role_profile: Option<&SubagentRoleProfile>,
) -> Vec<String> {
    if requested_allowed_tools.is_some() {
        return resolve_allowed_tools(base_allowed_tools, requested_allowed_tools);
    }

    let Some(profile) = role_profile else {
        return base_allowed_tools.to_vec();
    };
    let base: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
    let narrowed: Vec<String> = profile
        .recommended_tools
        .iter()
        .filter(|name| base.contains(**name))
        .map(|name| (*name).to_string())
        .collect();
    if narrowed.is_empty() {
        base_allowed_tools.to_vec()
    } else {
        narrowed
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
    role_profile: Option<&SubagentRoleProfile>,
    effective_source_scope: &[String],
    effective_allowed_tools: &[String],
    allowed_skills: &[AppliedSkillRef],
    evidence_handoff: &[EvidenceHandoffItem],
    previous_session: Option<&SubagentSessionSnapshot>,
) -> String {
    let sections = build_return_sections(args, role_profile);
    let mut request = String::from(
        "Complete the delegated task below. If information is missing, make the smallest reasonable assumption, state it briefly, and continue.\n\n## Supervisor Handoff Packet\n",
    );
    request.push_str("```json\n");
    request.push_str(
        &serde_json::to_string_pretty(&serde_json::json!({
            "task": args.task.trim(),
            "roleId": role_profile.map(|profile| profile.id),
            "roleName": role_profile.map(|profile| profile.label),
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
            "taskId": args.task_id,
            "resumingTaskId": previous_session.map(|snapshot| snapshot.task_id.as_str()),
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    request.push_str("\n```\n\n## Delegated Task\n");
    request.push_str(args.task.trim());

    if let Some(profile) = role_profile {
        request.push_str("\n\nAssigned role profile:\n");
        request.push_str(profile.label);
        request.push_str(" (");
        request.push_str(profile.id);
        request.push_str(")\n");
        request.push_str(profile.instructions);
    }

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

    if let Some(snapshot) = previous_session {
        request.push_str("\n\n## Resumed Subagent Session\n");
        request.push_str("You are continuing a previous delegated session with task_id `");
        request.push_str(&snapshot.task_id);
        request.push_str("`. Treat the prior result as context, not as final truth.\n\n");
        request.push_str("Previous task:\n");
        request.push_str(&truncate_excerpt(&snapshot.task, 1_000));
        request.push_str("\n\nPrevious result:\n");
        request.push_str(&truncate_excerpt(&snapshot.result, 4_000));
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

    args.role_id = trim_optional(args.role_id).map(|role_id| normalize_role_id(&role_id));
    resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())?;
    args.role = trim_optional(args.role);
    args.task_id = trim_optional(args.task_id);
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
        task_id: task.task_id,
        role_id: task.role_id,
        role: task.role,
        model_policy: task.model_policy,
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
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
) -> Result<SubagentRunArtifact, CoreError> {
    let launch_started = Instant::now();
    if runtime.delegation_depth >= MAX_SUBAGENT_DELEGATION_DEPTH {
        return Err(CoreError::InvalidInput(format!(
            "Recursive delegated execution is blocked beyond depth {}.",
            MAX_SUBAGENT_DELEGATION_DEPTH
        )));
    }

    let worker_cancel_token = runtime.cancel_token.child_token();

    let role_profile = resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())?;
    let requested_task_id = args.task_id.clone();
    let session_id = requested_task_id
        .clone()
        .unwrap_or_else(|| worker_id.clone().unwrap_or_else(|| call_label.clone()));
    let history_load_started = Instant::now();
    let previous_session = requested_task_id
        .as_deref()
        .and_then(|task_id| runtime.get_session_snapshot(task_id));
    let history_load_ms = instant_elapsed_ms(history_load_started);

    let context_build_started = Instant::now();
    let mut config = runtime.base_config.clone();
    let model_route_fallback = apply_delegated_model_policy(
        &mut config,
        &runtime.provider_config,
        args.model_policy.as_ref(),
    );
    let effective_model = config.model.clone();
    let effective_provider_type = config
        .provider_type
        .unwrap_or(runtime.provider_config.provider_type);
    let catalog_limits = effective_model
        .as_deref()
        .and_then(|model| model_limits_from_catalog(effective_provider_type, model));
    let delegation_limits = runtime.budget.limits().await;
    config.max_iterations = args
        .max_iterations
        .unwrap_or_else(|| {
            role_profile
                .map(|profile| profile.default_max_iterations)
                .unwrap_or(3)
        })
        .clamp(1, 6);
    let catalog_context_limit = catalog_limits
        .as_ref()
        .and_then(|limits| limits.context_tokens)
        .and_then(|limit| u32::try_from(limit).ok());
    apply_delegated_model_limits(
        &mut config,
        delegation_limits.input_context_policy,
        delegation_limits.max_output_tokens_per_worker,
        catalog_context_limit,
        catalog_limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens),
        runtime.base_config.delegation_limits_v2.is_some(),
    );
    let context_snapshot = runtime.context_snapshot(
        &db,
        effective_model.as_deref().unwrap_or("default"),
        config.context_window.unwrap_or(128_000),
    );
    let context_build_ms = instant_elapsed_ms(context_build_started);
    let timeout_secs = estimate_subagent_timeout_secs(&runtime, &args, role_profile);
    let run_deadline_ms = resolve_delegation_run_deadline_ms(
        &runtime.base_config,
        args.timeout_secs,
        timeout_secs,
        delegation_limits.run_deadline_ms,
    );
    config.agent_timeout_secs = Some(
        u32::try_from(run_deadline_ms.div_ceil(1_000))
            .unwrap_or(u32::MAX)
            .max(1),
    );
    config.request_kind = AgentRequestKind::SubagentWorker;
    config.system_prompt =
        build_subagent_system_prompt(&config.system_prompt, args.role.as_deref(), role_profile);

    let available_tool_names = runtime.get_tool_registry()?.tool_names();
    let baseline_allowed_tools =
        normalize_allowed_tools(runtime.allowed_tools.as_deref(), &available_tool_names);
    let mut effective_allowed_tools = resolve_allowed_tools_for_role(
        &baseline_allowed_tools,
        args.allowed_tools.as_deref(),
        role_profile,
    );
    if !runtime.can_delegate_further() {
        effective_allowed_tools.retain(|name| !is_subagent_tool_name(name));
    }
    let effective_source_scope =
        resolve_source_scope(&inherited_source_scope, args.source_ids.as_deref());
    let evidence_handoff = build_evidence_handoff(&db, args.evidence_chunk_ids.as_deref());
    let skill_select_started = Instant::now();
    let selected_skill_query = format!(
        "{}\n{}",
        args.task,
        args.context.clone().unwrap_or_default()
    );
    let skill_index = runtime
        .skill_index
        .get_or_init(|| load_skill_index_snapshot(&db));
    let enabled_skills = nexa_core::skills::select_available_skills_from_pool(
        filter_enabled_skills(&skill_index.skills, runtime.allowed_skill_ids.as_deref()),
        &selected_skill_query,
    );
    let applied_skill_refs = applied_skills(&enabled_skills);
    let skill_select_ms = instant_elapsed_ms(skill_select_started);
    let tool_registry_started = Instant::now();
    let tools =
        build_subagent_executor_tools(&runtime, &effective_allowed_tools, &worker_cancel_token)?;
    let tool_registry_ms = instant_elapsed_ms(tool_registry_started);
    let request_build_started = Instant::now();
    let request_text = build_subagent_request(
        &args,
        role_profile,
        &effective_source_scope,
        &effective_allowed_tools,
        &applied_skill_refs,
        &evidence_handoff,
        previous_session.as_ref(),
    );
    let request_build_ms = instant_elapsed_ms(request_build_started);
    let initial_output_credit = initial_output_credit(role_profile, &args, &config);
    let reserved_tokens =
        estimate_reserved_tokens(&config, &request_text, &tools, initial_output_credit);
    let mut subtask_input = subtask_input_payload(
        "subagent_run",
        &call_label,
        worker_id.as_deref(),
        &args,
        role_profile,
        &effective_source_scope,
        &effective_allowed_tools,
        &applied_skill_refs,
        reserved_tokens,
        timeout_secs,
    );
    subtask_input["delegationLimitsV2"] =
        serde_json::to_value(&delegation_limits).unwrap_or_else(|_| serde_json::json!({}));
    subtask_input["initialOutputCredit"] = serde_json::json!(initial_output_credit);
    subtask_input["skillIndexGeneration"] = serde_json::json!(&skill_index.generation);
    subtask_input["contextSnapshot"] = serde_json::json!({
        "id": &context_snapshot.id,
        "selectedMessageIds": &context_snapshot.selected_message_ids,
        "tokenEstimate": context_snapshot.token_estimate,
        "contextLimit": context_snapshot.context_limit,
    });
    let parent_task_run_id = runtime.parent_task_run_id.clone();
    let subtask_run_id = if let Some(parent_run_id) = parent_task_run_id.as_deref() {
        let role_label = subtask_role_label(&args, role_profile, "Subagent");
        let subtask = db.create_agent_subtask_run(
            parent_run_id,
            &call_label,
            &role_label,
            Some(&subtask_input),
            Some(reserved_tokens),
        )?;
        record_subtask_event(
            &db,
            parent_run_id,
            &format!("Subagent queued: {call_label}"),
            "queued",
            Some(&serde_json::json!({
                "subtaskRunId": &subtask.id,
                "callLabel": &call_label,
                "role": role_label,
                "task": &args.task,
                "modelPolicy": &args.model_policy,
                "effectiveModel": &effective_model,
                "modelRouteFallback": model_route_fallback,
                "reservedTokens": reserved_tokens,
            })),
        );
        Some(subtask.id)
    } else {
        None
    };
    if let (Some(parent_run_id), Some(subtask_run_id)) =
        (parent_task_run_id.as_deref(), subtask_run_id.as_deref())
    {
        for (stage, elapsed_ms, status) in [
            (
                "launch_ack_ms",
                instant_elapsed_ms(launch_started),
                "measured",
            ),
            ("history_load_ms", history_load_ms, "measured"),
            ("context_build_ms", context_build_ms, "measured"),
            ("skill_select_ms", skill_select_ms, "measured"),
            ("mcp_sync_ms", 0, "shared_snapshot"),
            ("tool_registry_ms", tool_registry_ms, "measured"),
            ("attachment_prepare_ms", 0, "not_applicable"),
            ("request_build_ms", request_build_ms, "measured"),
        ] {
            record_subagent_launch_metric(
                &db,
                parent_run_id,
                subtask_run_id,
                &call_label,
                stage,
                Some(elapsed_ms),
                None,
                status,
            );
        }
    }
    let queue_started = Instant::now();
    let is_verification = role_profile.is_some_and(|profile| profile.id == "verifier");
    let _permit = match runtime
        .budget
        .begin_call(
            &call_label,
            reserved_tokens,
            is_verification,
            &worker_cancel_token,
        )
        .await
    {
        Ok(permit) => permit,
        Err(err) => {
            let output = serde_json::json!({
                "kind": "subagent_run_error",
                "callLabel": &call_label,
                "error": err.to_string(),
            });
            finish_subtask_run_best_effort(
                &db,
                subtask_run_id.as_deref(),
                "failed",
                Some(&output),
                Some(&err.to_string()),
            );
            if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                record_subtask_event(
                    &db,
                    parent_run_id,
                    &format!("Subagent failed: {call_label}"),
                    "failed",
                    Some(&output),
                );
            }
            return Err(err);
        }
    };
    // Acquire the batch-local cap only after the role-aware global scheduler
    // has granted a lane. Explorers queued on their lane must never occupy
    // generic batch slots and starve the dedicated verifier lane.
    let _batch_permit = if let Some(batch_slots) = batch_slots {
        match acquire_batch_slot(
            batch_slots,
            &worker_cancel_token,
            &call_label,
            queue_started,
            delegation_limits.queue_deadline_ms,
        )
        .await
        {
            Ok(permit) => Some(permit),
            Err(error) => {
                runtime
                    .budget
                    .rollback_unstarted_worker(reserved_tokens, is_verification)
                    .await;
                finish_subtask_run_best_effort(
                    &db,
                    subtask_run_id.as_deref(),
                    "failed",
                    None,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        }
    } else {
        None
    };

    if let Some(subtask_id) = subtask_run_id.as_deref() {
        if let Err(err) = db.mark_agent_subtask_run_started(subtask_id, "running") {
            runtime
                .budget
                .rollback_unstarted_worker(reserved_tokens, is_verification)
                .await;
            finish_subtask_run_best_effort(
                &db,
                Some(subtask_id),
                "failed",
                None,
                Some(&err.to_string()),
            );
            return Err(err);
        }
    }
    if let Some(parent_run_id) = parent_task_run_id.as_deref() {
        record_subtask_event(
            &db,
            parent_run_id,
            &format!("Subagent started: {call_label}"),
            "running",
            Some(&serde_json::json!({
                "subtaskRunId": &subtask_run_id,
                "callLabel": &call_label,
                "reservedTokens": reserved_tokens,
                "queueWaitMs": u64::try_from(queue_started.elapsed().as_millis()).unwrap_or(u64::MAX),
            })),
        );
    }

    if let Some(parent_run_id) = parent_task_run_id.as_deref() {
        record_subtask_event(
            &db,
            parent_run_id,
            &format!("Subagent connecting: {call_label}"),
            "connecting",
            Some(&serde_json::json!({
                "subtaskRunId": &subtask_run_id,
                "callLabel": &call_label,
            })),
        );
    }
    let provider = match create_provider(runtime.provider_config.clone()) {
        Ok(provider) => provider,
        Err(error) => {
            runtime.budget.release_reservation(reserved_tokens).await;
            let error = CoreError::Llm(error.to_string());
            finish_subtask_run_best_effort(
                &db,
                subtask_run_id.as_deref(),
                "failed",
                None,
                Some(&error.to_string()),
            );
            if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                record_subtask_event(
                    &db,
                    parent_run_id,
                    &format!("Subagent failed: {call_label}"),
                    "failed",
                    Some(&serde_json::json!({
                        "subtaskRunId": &subtask_run_id,
                        "callLabel": &call_label,
                        "error": error.to_string(),
                        "phase": "connecting",
                    })),
                );
            }
            return Err(error);
        }
    };
    let estimated_cost_micros =
        nexa_core::usage_analytics::usage_cost_metadata(Some(effective_provider_type)).0;
    let non_streaming_completion = llm_streaming_disabled_by_env()
        || provider_uses_non_streaming_fallback(
            effective_provider_type,
            effective_model.as_deref().unwrap_or_default(),
        );

    let executor = AgentExecutor::new(provider, tools, config)
        .with_usage_identity(
            format!(
                "subagent:{}",
                subtask_run_id.as_deref().unwrap_or(&session_id)
            ),
            parent_task_run_id.clone(),
            subtask_run_id.clone(),
        )
        .with_cancel_token(worker_cancel_token.clone())
        .with_skills_override(enabled_skills);

    let (tx, event_rx) = mpsc::channel::<AgentEvent>(64);
    let (fatal_error_tx, mut fatal_error_rx) = mpsc::unbounded_channel::<String>();
    let (provider_connected_tx, mut provider_connected_rx) = mpsc::channel::<()>(1);
    let (first_response_tx, mut first_response_rx) = mpsc::channel::<()>(1);
    let capture_cancel_token = worker_cancel_token.clone();
    let telemetry_db = db.clone();
    let telemetry_identity = parent_task_run_id.clone().zip(subtask_run_id.clone());
    let telemetry_call_label = call_label.clone();
    let mut event_task = tokio::spawn(async move {
        let mut event_rx = event_rx;
        let mut capture = EventCapture::default();
        let mut provider_invocation_index = 0_u32;
        let mut active_provider_invocation_id: Option<String> = None;
        let mut first_provider_output_recorded = false;

        while let Some(event) = event_rx.recv().await {
            let provider_connected = matches!(
                &event,
                AgentEvent::ControllerStatus { code, .. } if code == "provider_connected"
            );
            if provider_connected {
                provider_invocation_index = provider_invocation_index.saturating_add(1);
                let invocation_id = format!(
                    "subagent-provider:{}:{}",
                    telemetry_identity
                        .as_ref()
                        .map(|(_, subtask_id)| subtask_id.as_str())
                        .unwrap_or("detached"),
                    provider_invocation_index
                );
                active_provider_invocation_id = Some(invocation_id.clone());
                first_provider_output_recorded = false;
                if let Some((parent_run_id, subtask_run_id)) = telemetry_identity.as_ref() {
                    record_subagent_launch_metric(
                        &telemetry_db,
                        parent_run_id,
                        subtask_run_id,
                        &telemetry_call_label,
                        "provider_connect_ms",
                        Some(instant_elapsed_ms(launch_started)),
                        Some(&invocation_id),
                        if non_streaming_completion {
                            "completion_boundary"
                        } else {
                            "measured"
                        },
                    );
                }
                signal_progress_latch(&provider_connected_tx);
            }
            let is_provider_output = matches!(
                &event,
                AgentEvent::TextDelta { .. }
                    | AgentEvent::Thinking { .. }
                    | AgentEvent::ToolCallPreparing { .. }
                    | AgentEvent::ToolCallArgsDelta { .. }
                    | AgentEvent::ToolCallStart { .. }
                    | AgentEvent::Done { .. }
            );
            if is_provider_output && active_provider_invocation_id.is_some() {
                signal_progress_latch(&first_response_tx);
                if !first_provider_output_recorded {
                    first_provider_output_recorded = true;
                    if let (Some((parent_run_id, subtask_run_id)), Some(provider_invocation_id)) = (
                        telemetry_identity.as_ref(),
                        active_provider_invocation_id.as_deref(),
                    ) {
                        let elapsed_ms = instant_elapsed_ms(launch_started);
                        record_subagent_launch_metric(
                            &telemetry_db,
                            parent_run_id,
                            subtask_run_id,
                            &telemetry_call_label,
                            "first_sse_byte_ms",
                            (!non_streaming_completion).then_some(elapsed_ms),
                            Some(provider_invocation_id),
                            if non_streaming_completion {
                                "not_applicable_completion_mode"
                            } else {
                                "measured"
                            },
                        );
                        record_subagent_launch_metric(
                            &telemetry_db,
                            parent_run_id,
                            subtask_run_id,
                            &telemetry_call_label,
                            "first_visible_token_ms",
                            Some(elapsed_ms),
                            Some(provider_invocation_id),
                            "measured",
                        );
                        record_subagent_launch_metric(
                            &telemetry_db,
                            parent_run_id,
                            subtask_run_id,
                            &telemetry_call_label,
                            "frontend_first_paint_ms",
                            None,
                            Some(provider_invocation_id),
                            "not_applicable_background_worker",
                        );
                    }
                }
            }
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
                AgentEvent::Steering { content } => {
                    if !content.trim().is_empty() {
                        capture.tool_events.push(serde_json::json!({
                            "phase": "steering",
                            "content": content,
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
                AgentEvent::Error { message } => {
                    capture.error_message = Some(message.clone());
                    capture.tool_events.push(serde_json::json!({
                        "phase": "error",
                        "message": &message,
                    }));
                    let _ = fatal_error_tx.send(message);
                    capture_cancel_token.cancel();
                    break;
                }
                AgentEvent::TextDelta { .. }
                | AgentEvent::StreamBlockDelta { .. }
                | AgentEvent::StreamReset { .. }
                | AgentEvent::AutoCompacted { .. }
                | AgentEvent::ToolRunStarted { .. }
                | AgentEvent::ToolRunUpdated { .. }
                | AgentEvent::ToolRunCompleted { .. }
                | AgentEvent::ToolCallPreparing { .. }
                | AgentEvent::ToolCallArgsDelta { .. }
                | AgentEvent::ToolCallProgress { .. }
                | AgentEvent::ApprovalRequested { .. }
                | AgentEvent::ApprovalResolved { .. }
                | AgentEvent::ControllerStatus { .. }
                | AgentEvent::PlanUpdated { .. } => {}
            }
        }

        capture
    });

    let run_deadline = tokio::time::Instant::now() + Duration::from_millis(run_deadline_ms);
    let provider_wait_started = Instant::now();
    let connect_deadline = if non_streaming_completion {
        // `complete()` cannot expose a connection boundary: its
        // provider_connected event is emitted only after the full response.
        // Let the overall run deadline govern this mode.
        run_deadline
    } else {
        std::cmp::min(
            run_deadline,
            tokio::time::Instant::now()
                + Duration::from_millis(delegation_limits.connect_deadline_ms),
        )
    };
    let run_future = executor.run_with_source_scope(
        context_snapshot.messages.as_ref().to_vec(),
        vec![ContentPart::Text { text: request_text }],
        &db,
        None,
        None,
        Some(effective_source_scope.clone()),
        tx,
        0,
    );
    tokio::pin!(run_future);

    let connect_stage = tokio::select! {
        biased;
        error = fatal_error_rx.recv() => Some(Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' failed: {}",
            error.unwrap_or_else(|| "worker emitted an unspecified fatal error".to_string())
        )))),
        _ = worker_cancel_token.cancelled() => Some(Err(CoreError::Agent(format!(
            "Delegated execution '{call_label}' was cancelled by the parent turn."
        )))),
        result = &mut run_future => Some(result),
        _ = provider_connected_rx.recv() => {
            if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                record_subtask_event(
                    &db,
                    parent_run_id,
                    &format!("Subagent connected to provider: {call_label}"),
                    "connected",
                    Some(&serde_json::json!({
                        "subtaskRunId": &subtask_run_id,
                        "callLabel": &call_label,
                        "connectMs": u64::try_from(provider_wait_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    })),
                );
            }
            None
        },
        _ = tokio::time::sleep_until(connect_deadline) => {
            worker_cancel_token.cancel();
            Some(Err(CoreError::Agent(if non_streaming_completion {
                format!("Delegated execution '{call_label}' timed out after {run_deadline_ms}ms.")
            } else {
                format!(
                    "Delegated execution '{call_label}' exceeded its {}ms provider-connect deadline.",
                    delegation_limits.connect_deadline_ms
                )
            })))
        }
    };
    let first_stage = match connect_stage {
        Some(result) => Some(result),
        None => {
            let first_response_deadline = std::cmp::min(
                run_deadline,
                tokio::time::Instant::now()
                    + Duration::from_millis(delegation_limits.first_token_deadline_ms),
            );
            tokio::select! {
                biased;
                error = fatal_error_rx.recv() => Some(Err(CoreError::Agent(format!(
                    "Delegated execution '{call_label}' failed: {}",
                    error.unwrap_or_else(|| "worker emitted an unspecified fatal error".to_string())
                )))),
                _ = worker_cancel_token.cancelled() => Some(Err(CoreError::Agent(format!(
                    "Delegated execution '{call_label}' was cancelled by the parent turn."
                )))),
                result = &mut run_future => Some(result),
                _ = first_response_rx.recv() => {
            if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                record_subtask_event(
                    &db,
                    parent_run_id,
                    &format!("Subagent received first token: {call_label}"),
                    "first_token",
                    Some(&serde_json::json!({
                        "subtaskRunId": &subtask_run_id,
                        "callLabel": &call_label,
                        "firstTokenMs": u64::try_from(provider_wait_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    })),
                );
            }
            None
                },
                _ = tokio::time::sleep_until(first_response_deadline) => {
                    worker_cancel_token.cancel();
                    Some(Err(CoreError::Agent(format!(
                        "Delegated execution '{call_label}' exceeded its {}ms first-token deadline.",
                        delegation_limits.first_token_deadline_ms
                    ))))
                }
            }
        }
    };
    let final_result = match first_stage {
        Some(result) => result,
        None => tokio::select! {
            biased;
            error = fatal_error_rx.recv() => Err(CoreError::Agent(format!(
                "Delegated execution '{call_label}' failed: {}",
                error.unwrap_or_else(|| "worker emitted an unspecified fatal error".to_string())
            ))),
            _ = worker_cancel_token.cancelled() => Err(CoreError::Agent(format!(
                "Delegated execution '{call_label}' was cancelled by the parent turn."
            ))),
            result = &mut run_future => result,
            _ = tokio::time::sleep_until(run_deadline) => {
                worker_cancel_token.cancel();
                Err(CoreError::Agent(format!(
                    "Delegated execution '{call_label}' timed out after {run_deadline_ms}ms."
                )))
            }
        },
    };

    let capture = match tokio::time::timeout(Duration::from_millis(500), &mut event_task).await {
        Ok(Ok(capture)) => capture,
        Ok(Err(error)) => {
            warn!("Subagent event collector failed for {call_label}: {error}");
            EventCapture::default()
        }
        Err(_) => {
            event_task.abort();
            let _ = event_task.await;
            warn!("Subagent event collector exceeded its 500ms shutdown deadline for {call_label}");
            EventCapture::default()
        }
    };
    runtime
        .budget
        .finish_call(reserved_tokens, &capture.usage_total, estimated_cost_micros)
        .await;
    let final_message = match final_result {
        Ok(message) => message,
        Err(err) => {
            let error_text = err.to_string();
            let failure_status = delegated_failure_status(&error_text);
            let output = serde_json::json!({
                "kind": "subagent_run_error",
                "callLabel": &call_label,
                "error": &error_text,
                "emittedError": capture.error_message,
                "usageTotal": capture.usage_total,
                "toolEvents": capture.tool_events,
            });
            finish_subtask_run_best_effort(
                &db,
                subtask_run_id.as_deref(),
                failure_status,
                Some(&output),
                Some(&error_text),
            );
            if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                record_subtask_event(
                    &db,
                    parent_run_id,
                    &format!("Subagent {failure_status}: {call_label}"),
                    failure_status,
                    Some(&output),
                );
            }
            return Err(err);
        }
    };

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

    let run = SubagentRunArtifact {
        id: worker_id.unwrap_or_else(|| call_label.clone()),
        session_id: session_id.clone(),
        resumed_from_task_id: requested_task_id
            .clone()
            .filter(|_| previous_session.is_some()),
        previous_session: previous_session.clone(),
        status: "done".to_string(),
        task: args.task,
        role_id: role_profile.map(|profile| profile.id.to_string()),
        role_name: role_profile.map(|profile| profile.label.to_string()),
        role: args.role,
        model_policy: args.model_policy,
        effective_model,
        model_route_fallback,
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
    };
    runtime.save_session_snapshot(SubagentSessionSnapshot {
        task_id: session_id.clone(),
        last_run_id: run.id.clone(),
        task: run.task.clone(),
        role_id: run.role_id.clone(),
        role_name: run.role_name.clone(),
        result: run.result.clone(),
        finish_reason: run.finish_reason.clone(),
        usage_total: run.usage_total.clone(),
        tool_event_count: run.tool_events.len(),
    });
    let output = serde_json::json!({
        "kind": "subagent_run",
        "run": &run,
    });
    finish_subtask_run_best_effort(
        &db,
        subtask_run_id.as_deref(),
        "completed",
        Some(&output),
        None,
    );
    if let Some(parent_run_id) = parent_task_run_id.as_deref() {
        record_subtask_event(
            &db,
            parent_run_id,
            &format!("Subagent completed: {}", run.id),
            "completed",
            Some(&output),
        );
    }

    Ok(run)
}

fn failed_subagent_run_artifact(
    label: String,
    fallback: SpawnSubagentArgs,
    parallel_group: Option<String>,
    error: &CoreError,
) -> SubagentRunArtifact {
    SubagentRunArtifact {
        id: label.clone(),
        session_id: label,
        resumed_from_task_id: None,
        previous_session: None,
        status: "error".to_string(),
        task: fallback.task,
        role_id: fallback.role_id.clone(),
        role_name: resolve_role_profile(fallback.role_id.as_deref(), fallback.role.as_deref())
            .ok()
            .flatten()
            .map(|profile| profile.label.to_string()),
        role: fallback.role,
        model_policy: fallback.model_policy,
        effective_model: None,
        model_route_fallback: false,
        expected_output: fallback.expected_output,
        acceptance_criteria: fallback.acceptance_criteria,
        evidence_chunk_ids: fallback.evidence_chunk_ids,
        evidence_handoff: Vec::new(),
        requested_source_scope: fallback.source_ids,
        effective_source_scope: Vec::new(),
        requested_allowed_tools: fallback.allowed_tools,
        allowed_tools: Vec::new(),
        allowed_skills: Vec::new(),
        parallel_group,
        deliverable_style: fallback.deliverable_style,
        return_sections: fallback.return_sections,
        result: format!("Subagent failed: {error}"),
        finish_reason: None,
        usage_total: Usage::default(),
        tool_events: Vec::new(),
        thinking: None,
        source_scope_applied: false,
        is_error: true,
        error_message: Some(error.to_string()),
    }
}

fn summarize_subagent_run(run: &SubagentRunArtifact) -> String {
    let role_suffix = run
        .role_name
        .as_deref()
        .or(run.role.as_deref())
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
        "spawn_subagent"
            | "spawn_subagent_batch"
            | "judge_subagent_results"
            | "observe_subagent_batch"
    )
}

fn compatible_auxiliary_model(
    config: &AgentConfig,
    provider_config: &ProviderConfig,
) -> Option<String> {
    let provider_matches = config
        .summarization_provider_type
        .is_none_or(|provider_type| provider_type == provider_config.provider_type);
    provider_matches
        .then(|| config.summarization_model.as_deref().map(str::trim))
        .flatten()
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// Route delegated phases without ever sending a model identifier to credentials
/// or an endpoint configured for a different provider. A missing compatible
/// auxiliary model is an explicit fallback to the parent model.
fn apply_delegated_model_policy(
    config: &mut AgentConfig,
    provider_config: &ProviderConfig,
    policy: Option<&ModelRoutingClass>,
) -> bool {
    if !matches!(
        policy,
        Some(ModelRoutingClass::Fast | ModelRoutingClass::IndependentReviewer)
    ) {
        return false;
    }
    if let Some(model) = compatible_auxiliary_model(config, provider_config) {
        config.model = Some(model);
        false
    } else {
        true
    }
}

fn resolve_delegation_timeout_secs(config: &AgentConfig, requested: Option<u32>) -> u64 {
    requested.unwrap_or_else(|| {
        let tool_timeout = config
            .tool_timeout_secs
            .filter(|timeout| *timeout > 0)
            .unwrap_or(60);
        let turn_timeout = config
            .agent_timeout_secs
            .filter(|timeout| *timeout > 0)
            .unwrap_or(180);
        tool_timeout
            .saturating_mul(2)
            .min(turn_timeout)
            .clamp(15, 180)
    }) as u64
}

fn resolve_delegation_run_deadline_ms(
    config: &AgentConfig,
    requested_timeout_secs: Option<u32>,
    legacy_timeout_secs: u64,
    configured_run_deadline_ms: u64,
) -> u64 {
    if config.delegation_limits_v2.is_some() {
        requested_timeout_secs
            .map(|requested| configured_run_deadline_ms.min(u64::from(requested) * 1_000))
            .unwrap_or(configured_run_deadline_ms)
    } else {
        configured_run_deadline_ms.min(legacy_timeout_secs.saturating_mul(1_000))
    }
}

fn delegated_failure_status(error_text: &str) -> &'static str {
    if error_text.contains("timed out")
        || error_text.contains("provider-connect deadline")
        || error_text.contains("first-token deadline")
        || error_text.contains("queue deadline")
    {
        "timed_out"
    } else if error_text.contains("cancelled") {
        "cancelled"
    } else {
        "failed"
    }
}

fn estimate_subagent_timeout_secs(
    runtime: &DelegationRuntime,
    args: &SpawnSubagentArgs,
    role_profile: Option<&SubagentRoleProfile>,
) -> u64 {
    match args.timeout_secs {
        Some(requested) => resolve_delegation_timeout_secs(&runtime.base_config, Some(requested)),
        None => {
            let base = resolve_delegation_timeout_secs(&runtime.base_config, None);
            role_profile
                .map(|profile| base.min(profile.default_timeout_secs as u64).max(15))
                .unwrap_or(base)
        }
    }
}

fn estimate_reserved_tokens(
    config: &AgentConfig,
    request_text: &str,
    tools: &ToolRegistry,
    initial_output_credit: u32,
) -> u32 {
    let model = config.model.as_deref().unwrap_or("gpt-4o-mini");
    estimate_tokens_for_model(model, &config.system_prompt)
        .saturating_add(estimate_tokens_for_model(model, request_text))
        .saturating_add(estimate_tool_tokens_for_model(model, &tools.definitions()))
        .saturating_add(initial_output_credit)
}

fn resolve_delegated_max_output(config: &AgentConfig, catalog_limit: Option<u64>) -> u32 {
    let fallback_limit = u64::from(CONSERVATIVE_SUBAGENT_MAX_TOKENS);
    let effective_limit = catalog_limit
        .unwrap_or(fallback_limit)
        .min(u64::from(u32::MAX)) as u32;
    config
        .max_tokens
        .unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS)
        .clamp(1_024, effective_limit.max(1_024))
}

fn apply_delegated_model_limits(
    config: &mut AgentConfig,
    input_context_policy: DelegationLimitPolicy,
    max_output_policy: DelegationLimitPolicy,
    catalog_context_limit: Option<u32>,
    catalog_output_limit: Option<u64>,
    independent_v2_limits: bool,
) {
    config.context_window = match input_context_policy {
        DelegationLimitPolicy::Explicit(limit) => u32::try_from(limit)
            .ok()
            .map(|limit| catalog_context_limit.map_or(limit, |catalog| limit.min(catalog))),
        DelegationLimitPolicy::Auto if independent_v2_limits => {
            catalog_context_limit.or(config.context_window)
        }
        DelegationLimitPolicy::Auto => config.context_window.or(catalog_context_limit),
    };

    match max_output_policy {
        DelegationLimitPolicy::Explicit(limit) => {
            config.max_tokens = u32::try_from(limit).ok();
        }
        DelegationLimitPolicy::Auto if independent_v2_limits => {
            config.max_tokens = Some(
                catalog_output_limit
                    .map(|limit| limit.min(u64::from(u32::MAX)) as u32)
                    .unwrap_or(CONSERVATIVE_SUBAGENT_MAX_TOKENS),
            );
        }
        DelegationLimitPolicy::Auto => {}
    }

    config.max_tokens = Some(resolve_delegated_max_output(config, catalog_output_limit));
}

fn initial_output_credit(
    role_profile: Option<&SubagentRoleProfile>,
    args: &SpawnSubagentArgs,
    config: &AgentConfig,
) -> u32 {
    let role_credit = match role_profile.map(|profile| profile.id) {
        Some("critic" | "verifier") => 4_096,
        Some("writer") => 16_384,
        Some("researcher" | "planner") => 8_192,
        _ => 8_192,
    };
    let explicit_long_form = args.deliverable_style.as_deref().is_some_and(|style| {
        let style = style.to_ascii_lowercase();
        style.contains("long") || style.contains("comprehensive")
    });
    let requested_credit = if explicit_long_form {
        32_768
    } else {
        role_credit
    };
    requested_credit.min(config.max_tokens.unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS))
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
            "observe_subagent_batch",
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
        .any(|name| name == "observe_subagent_batch")
    {
        registry.register(Box::new(ObserveSubagentBatchTool::from_runtime(
            child_runtime.clone(),
        )));
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
        &delegation_tool_def(&SPAWN_SUBAGENT_DEF, SPAWN_SUBAGENT_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        spawn_subagent_parameters_schema()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }

    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let args: SpawnSubagentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent arguments: {e}"))
        })?;
        let args = normalize_spawn_args(args)?;
        let fallback_task = args.task.clone();
        let fallback_role_id = args.role_id.clone();
        let fallback_role = args.role.clone();
        let run = match run_subagent_once(
            self.runtime.clone(),
            db.clone(),
            source_scope.to_vec(),
            call_id.to_string(),
            None,
            args,
            None,
        )
        .await
        {
            Ok(run) => run,
            Err(err) => {
                let error_message = err.to_string();
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Subagent failed: {error_message}"),
                    is_error: true,
                    artifacts: Some(serde_json::json!({
                        "kind": "subagent_result",
                        "id": call_id,
                        "status": "error",
                        "task": fallback_task,
                        "roleId": fallback_role_id,
                        "role": fallback_role,
                        "result": format!("Subagent failed: {error_message}"),
                        "isError": true,
                        "errorMessage": error_message,
                    })),
                });
            }
        };

        let mut content = String::from("Subagent result");
        if let Some(role) = run.role_name.as_deref().or(run.role.as_deref()) {
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
                "id": run.id,
                "sessionId": run.session_id,
                "status": run.status,
                "task": run.task,
                "roleId": run.role_id,
                "roleName": run.role_name,
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
                "isError": run.is_error,
                "errorMessage": run.error_message,
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
        &delegation_tool_def(&SPAWN_SUBAGENT_BATCH_DEF, SPAWN_SUBAGENT_BATCH_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        spawn_subagent_batch_parameters_schema()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }

    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let mut args: SpawnSubagentBatchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent_batch arguments: {e}"))
        })?;
        args.batch_goal = trim_optional(args.batch_goal);
        args.parallel_group = trim_optional(args.parallel_group);
        args.workflow_template = normalize_workflow_template_id(args.workflow_template)?;
        let workflow_template = args
            .workflow_template
            .as_deref()
            .and_then(workflow_template_by_id);

        if args.tasks.is_empty() {
            let Some(template) = workflow_template else {
                return Err(CoreError::InvalidInput(
                    "spawn_subagent_batch requires either explicit tasks or workflow_template plus batch_goal".into(),
                ));
            };
            let Some(batch_goal) = args.batch_goal.clone() else {
                return Err(CoreError::InvalidInput(
                    "spawn_subagent_batch workflow_template expansion requires a non-empty batch_goal".into(),
                ));
            };
            if args.parallel_group.is_none() {
                args.parallel_group = Some(template.id.to_string());
            }
            args.tasks = expand_workflow_template_tasks(
                template,
                &batch_goal,
                args.parallel_group.as_deref(),
            );
        }

        let batch_goal = args.batch_goal.clone();
        let workflow_template_id = args.workflow_template.clone();
        let workflow_template_label = workflow_template.map(|template| template.label);
        let workflow_template_description = workflow_template.map(|template| template.description);
        let parallel_group = args.parallel_group.clone();
        let completion_policy =
            DelegationCompletionPolicy::resolve(&args, args.tasks.len().min(8))?;
        let requested_max_parallel = args.max_parallel;
        let cancel_remaining = args.cancel_remaining.unwrap_or(false);
        let normalized_tasks: Vec<(Option<String>, SpawnSubagentArgs)> = args
            .tasks
            .into_iter()
            .take(8)
            .enumerate()
            .map(|(index, mut task)| {
                if task.parallel_group.is_none() {
                    task.parallel_group = parallel_group.clone();
                }
                if task.id.is_none() {
                    task.id = Some(format!("{}-{}", call_id, index + 1));
                }
                normalize_batch_task_args(task)
            })
            .collect::<Result<_, _>>()?;

        let budget_before = self.runtime.budget.snapshot().await;
        let requested_parallel = requested_max_parallel
            .unwrap_or_else(|| {
                workflow_template
                    .map(|template| template.max_parallel)
                    .unwrap_or(budget_before.max_parallel)
            })
            .clamp(1, 8);
        let effective_parallel = requested_parallel.min(budget_before.max_parallel).max(1) as usize;

        let runtime = self.runtime.clone();
        let db = db.clone();
        let inherited_source_scope = source_scope.to_vec();
        let batch_parallel_group = parallel_group.clone();
        let worker_count = normalized_tasks.len();
        let batch_id = format!(
            "{}:{}",
            self.runtime
                .parent_task_run_id
                .as_deref()
                .unwrap_or("detached"),
            call_id
        );
        runtime.register_batch(&batch_id, worker_count);
        let batch_slots = Arc::new(tokio::sync::Semaphore::new(effective_parallel));
        let mut worker_cancel_tokens = Vec::with_capacity(worker_count);
        let mut pending = FuturesUnordered::new();
        for (index, (worker_id, task_args)) in normalized_tasks.into_iter().enumerate() {
            let db = db.clone();
            let inherited_source_scope = inherited_source_scope.clone();
            let batch_parallel_group = batch_parallel_group.clone();
            let worker_cancel = runtime.cancel_token.child_token();
            let worker_runtime = runtime.scoped_to_worker(worker_cancel.clone());
            let batch_runtime = runtime.clone();
            let batch_slots = Arc::clone(&batch_slots);
            worker_cancel_tokens.push(worker_cancel.clone());
            runtime.add_batch_cancel_token(&batch_id, worker_cancel);
            let worker_batch_id = batch_id.clone();
            let detached_label = worker_id
                .clone()
                .unwrap_or_else(|| format!("{}-{}", call_id, index + 1));
            let detached_fallback = task_args.clone();
            let detached_parallel_group = batch_parallel_group.clone();
            let batch_call_id = call_id.to_string();
            let worker_task = tokio::spawn(async move {
                let label = worker_id
                    .clone()
                    .unwrap_or_else(|| format!("{}-{}", batch_call_id, index + 1));
                let fallback = task_args.clone();
                let run = match run_subagent_once(
                    worker_runtime,
                    db,
                    inherited_source_scope,
                    label.clone(),
                    worker_id,
                    task_args,
                    Some(batch_slots),
                )
                .await
                {
                    Ok(run) => run,
                    Err(err) => {
                        failed_subagent_run_artifact(label, fallback, batch_parallel_group, &err)
                    }
                };
                batch_runtime.record_batch_result(&worker_batch_id, index, run.clone());
                (index, run)
            });
            pending.push(async move {
                worker_task.await.unwrap_or_else(|error| {
                    let error = CoreError::Agent(format!(
                        "Delegated worker task terminated unexpectedly: {error}"
                    ));
                    (
                        index,
                        failed_subagent_run_artifact(
                            detached_label,
                            detached_fallback,
                            detached_parallel_group,
                            &error,
                        ),
                    )
                })
            });
        }
        let policy_deadline = match &completion_policy {
            DelegationCompletionPolicy::Deadline { deadline_ms } => {
                Some(tokio::time::Instant::now() + Duration::from_millis(*deadline_ms))
            }
            _ => None,
        };
        let mut indexed_runs = Vec::with_capacity(worker_count);
        let mut policy_deadline_reached = false;
        while !pending.is_empty() {
            let next = if let Some(deadline) = policy_deadline {
                match tokio::time::timeout_at(deadline, pending.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        policy_deadline_reached = true;
                        None
                    }
                }
            } else {
                pending.next().await
            };
            let Some((index, run)) = next else {
                break;
            };
            indexed_runs.push((index, run));
            let completed_runs = indexed_runs
                .iter()
                .map(|(_, run)| run.clone())
                .collect::<Vec<_>>();
            if completion_policy.is_satisfied(&completed_runs, pending.len()) {
                break;
            }
        }

        let policy_satisfied = policy_deadline_reached
            || completion_policy.is_satisfied(
                &indexed_runs
                    .iter()
                    .map(|(_, run)| run.clone())
                    .collect::<Vec<_>>(),
                pending.len(),
            );
        let pending_at_policy_completion = pending.len();
        let continuing_workers = if !pending.is_empty() && !cancel_remaining {
            // Each entry owns a Tokio JoinHandle. Dropping the collector
            // detaches those tasks rather than cancelling them; their normal
            // completion path persists a supplemental subtask timeline event.
            pending.len()
        } else {
            0
        };
        if !pending.is_empty() && cancel_remaining {
            // Dropping a future is not cancellation. Signal every worker first,
            // then provide a bounded settlement window for durable final state.
            for token in &worker_cancel_tokens {
                token.cancel();
            }
            let settle_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            while !pending.is_empty() {
                match tokio::time::timeout_at(settle_deadline, pending.next()).await {
                    Ok(Some((index, run))) => indexed_runs.push((index, run)),
                    _ => break,
                }
            }
        }
        let unsettled_workers = if cancel_remaining { pending.len() } else { 0 };
        drop(pending);
        indexed_runs.sort_by_key(|(index, _)| *index);
        let runs = indexed_runs
            .into_iter()
            .map(|(_, run)| run)
            .collect::<Vec<_>>();

        let budget_after = self.runtime.budget.snapshot().await;
        let completed_runs = runs.iter().filter(|run| !run.is_error).count();
        let failed_runs = runs.len().saturating_sub(completed_runs);
        let mut content = format!("Completed {} delegated worker(s) in batch", runs.len());
        if let Some(goal) = batch_goal.as_deref() {
            content.push_str(&format!(" for: {goal}"));
        }
        if let Some(template_label) = workflow_template_label {
            content.push_str(&format!(" using {template_label}"));
        }
        if pending_at_policy_completion > 0 {
            content.push_str(&format!(
                "; completion policy released the parent with {pending_at_policy_completion} worker(s) still settling"
            ));
            content.push_str(&format!(
                ". Call observe_subagent_batch with batchId '{batch_id}' before final synthesis to receive supplemental evidence, wait for more results, or cancel residual workers"
            ));
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
                "batchId": &batch_id,
                "batchGoal": batch_goal,
                "workflowTemplate": workflow_template_id,
                "workflowTemplateLabel": workflow_template_label,
                "workflowTemplateDescription": workflow_template_description,
                "parallelGroup": parallel_group,
                "requestedMaxParallel": requested_parallel,
                "effectiveMaxParallel": effective_parallel,
                "completionPolicy": completion_policy,
                "completionPolicySatisfied": policy_satisfied,
                "pendingAtPolicyCompletion": pending_at_policy_completion,
                "unsettledWorkers": unsettled_workers,
                "continuingWorkers": continuing_workers,
                "supplementalEvidenceTool": (continuing_workers > 0).then_some("observe_subagent_batch"),
                "cancelRemaining": cancel_remaining,
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
impl Tool for ObserveSubagentBatchTool {
    fn name(&self) -> &str {
        "observe_subagent_batch"
    }

    fn description(&self) -> &str {
        "Observe supplemental results from a delegated batch after quorum, first-success, deadline, or parent-decides released the parent. Optionally wait for more results or cancel residual workers."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "batchId": {
                    "type": "string",
                    "description": "batchId returned by spawn_subagent_batch"
                },
                "waitMs": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 120000,
                    "description": "Wait up to this duration for another supplemental result"
                },
                "cancelRemaining": {
                    "type": "boolean",
                    "default": false,
                    "description": "Cancel workers that have not yet settled"
                }
            },
            "required": ["batchId"],
            "additionalProperties": false
        })
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }

    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let args: ObserveSubagentBatchArgs =
            serde_json::from_str(context.arguments).map_err(|error| {
                CoreError::InvalidInput(format!(
                    "Invalid observe_subagent_batch arguments: {error}"
                ))
            })?;
        let batch_id = args.batch_id.trim();
        if batch_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "observe_subagent_batch requires batchId".into(),
            ));
        }
        if args.cancel_remaining {
            self.runtime.cancel_batch(batch_id);
        }

        let wait_ms = args.wait_ms.unwrap_or(0).min(120_000);
        let baseline_count = self
            .runtime
            .batch_snapshot(batch_id)
            .ok_or_else(|| CoreError::NotFound(format!("Delegated batch {batch_id}")))?
            .1
            .len();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
        let (expected_workers, runs) = loop {
            let notified = self.runtime.batch_notify.notified();
            tokio::pin!(notified);
            // Register before reading the snapshot so a completion between
            // the read and the await cannot be lost by notify_waiters().
            notified.as_mut().enable();
            let Some((expected, runs)) = self.runtime.batch_snapshot(batch_id) else {
                return Err(CoreError::NotFound(format!("Delegated batch {batch_id}")));
            };
            if runs.len() >= expected
                || runs.len() > baseline_count
                || wait_ms == 0
                || tokio::time::Instant::now() >= deadline
            {
                break (expected, runs);
            }
            if tokio::time::timeout_at(deadline, &mut notified)
                .await
                .is_err()
            {
                break self
                    .runtime
                    .batch_snapshot(batch_id)
                    .unwrap_or((expected, runs));
            }
        };
        let completed_workers = runs.len();
        let pending_workers = expected_workers.saturating_sub(completed_workers);
        let mut content = format!(
            "Delegated batch {batch_id}: {completed_workers}/{expected_workers} worker(s) settled"
        );
        if pending_workers > 0 {
            content.push_str(&format!("; {pending_workers} still running"));
        }
        content.push_str(".\n\n");
        for run in &runs {
            content.push_str("- ");
            content.push_str(&summarize_subagent_run(run));
            content.push('\n');
        }

        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_batch_observation",
                "batchId": batch_id,
                "expectedWorkers": expected_workers,
                "completedWorkers": completed_workers,
                "pendingWorkers": pending_workers,
                "cancelRequested": args.cancel_remaining,
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
        &delegation_tool_def(&JUDGE_SUBAGENT_RESULTS_DEF, JUDGE_SUBAGENT_RESULTS_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        delegation_tool_def(&JUDGE_SUBAGENT_RESULTS_DEF, JUDGE_SUBAGENT_RESULTS_JSON)
            .parameters
            .clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }

    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let launch_started = Instant::now();
        let nexa_core::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            conversation_id,
            ..
        } = context;
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
        let model =
            compatible_auxiliary_model(&self.runtime.base_config, &self.runtime.provider_config)
                .or_else(|| self.runtime.base_config.model.clone())
                .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let system_prompt = build_judge_system_prompt(&self.runtime.base_config.system_prompt);
        let user_prompt = build_judge_request(&args);
        let reserved_tokens = estimate_tokens_for_model(&model, &system_prompt)
            .saturating_add(estimate_tokens_for_model(&model, &user_prompt))
            .saturating_add(1_200);
        let subtask_input = serde_json::json!({
            "kind": "subagent_judgement",
            "callLabel": call_id,
            "task": &args.task,
            "rubric": &args.rubric,
            "decisionMode": &args.decision_mode,
            "requiredWinnerCount": args.required_winner_count,
            "expectedOutput": &args.expected_output,
            "parallelGroup": &args.parallel_group,
            "candidateCount": args.candidates.len(),
            "candidateIds": args.candidates.iter().map(|candidate| candidate.id.as_str()).collect::<Vec<_>>(),
            "model": &model,
            "reservedTokens": reserved_tokens,
        });
        let parent_task_run_id = self.runtime.parent_task_run_id.clone();
        let subtask_run_id = if let Some(parent_run_id) = parent_task_run_id.as_deref() {
            let subtask = db.create_agent_subtask_run(
                parent_run_id,
                call_id,
                "Adjudicator",
                Some(&subtask_input),
                Some(reserved_tokens),
            )?;
            record_subtask_event(
                db,
                parent_run_id,
                &format!("Subagent judge queued: {call_id}"),
                "queued",
                Some(&serde_json::json!({
                    "subtaskRunId": &subtask.id,
                    "callLabel": call_id,
                    "candidateCount": args.candidates.len(),
                    "reservedTokens": reserved_tokens,
                })),
            );
            Some(subtask.id)
        } else {
            None
        };
        if let (Some(parent_run_id), Some(subtask_run_id)) =
            (parent_task_run_id.as_deref(), subtask_run_id.as_deref())
        {
            for (stage, status) in [
                ("launch_ack_ms", "measured"),
                ("history_load_ms", "not_applicable"),
                ("context_build_ms", "measured"),
                ("skill_select_ms", "not_applicable"),
                ("mcp_sync_ms", "shared_snapshot"),
                ("tool_registry_ms", "not_applicable"),
                ("attachment_prepare_ms", "not_applicable"),
                ("request_build_ms", "measured"),
            ] {
                record_subagent_launch_metric(
                    db,
                    parent_run_id,
                    subtask_run_id,
                    call_id,
                    stage,
                    Some(instant_elapsed_ms(launch_started)),
                    None,
                    status,
                );
            }
        }
        let _permit = match self
            .runtime
            .budget
            .begin_judge_call(
                "judge_subagent_results",
                reserved_tokens,
                &self.runtime.cancel_token,
            )
            .await
        {
            Ok(permit) => {
                if let Some(subtask_id) = subtask_run_id.as_deref() {
                    if let Err(err) = db.mark_agent_subtask_run_started(subtask_id, "adjudicating")
                    {
                        self.runtime
                            .budget
                            .rollback_unstarted_judge(reserved_tokens)
                            .await;
                        finish_subtask_run_best_effort(
                            db,
                            Some(subtask_id),
                            "failed",
                            None,
                            Some(&err.to_string()),
                        );
                        return Err(err);
                    }
                }
                if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                    record_subtask_event(
                        db,
                        parent_run_id,
                        &format!("Subagent judge started: {call_id}"),
                        "running",
                        Some(&serde_json::json!({
                            "subtaskRunId": &subtask_run_id,
                            "callLabel": call_id,
                            "reservedTokens": reserved_tokens,
                        })),
                    );
                }
                permit
            }
            Err(err) => {
                let output = serde_json::json!({
                    "kind": "subagent_judgement_error",
                    "callLabel": call_id,
                    "error": err.to_string(),
                });
                finish_subtask_run_best_effort(
                    db,
                    subtask_run_id.as_deref(),
                    "failed",
                    Some(&output),
                    Some(&err.to_string()),
                );
                if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                    record_subtask_event(
                        db,
                        parent_run_id,
                        &format!("Subagent judge failed: {call_id}"),
                        "failed",
                        Some(&output),
                    );
                }
                return Err(err);
            }
        };
        let request = CompletionRequest {
            model: model.clone(),
            messages: vec![
                nexa_core::llm::Message::text(nexa_core::llm::Role::System, system_prompt),
                nexa_core::llm::Message::text(nexa_core::llm::Role::User, user_prompt),
            ],
            temperature: Some(0.1),
            max_tokens: Some(if self.runtime.base_config.power_mode.is_nexus() {
                self.runtime
                    .base_config
                    .max_tokens
                    .unwrap_or(4_000)
                    .clamp(1_200, 4_000)
            } else {
                1_200
            }),
            tools: None,
            stop: None,
            thinking_budget: if self.runtime.base_config.power_mode.is_nexus() {
                self.runtime.base_config.thinking_budget
            } else {
                None
            },
            reasoning_effort: if self.runtime.base_config.power_mode.is_nexus() {
                self.runtime.base_config.reasoning_effort.clone()
            } else {
                None
            },
            provider_type: self.runtime.base_config.provider_type,
            parallel_tool_calls: true,
        };
        let judge_cancel_token = self.runtime.cancel_token.child_token();
        let timeout_secs = resolve_delegation_timeout_secs(&self.runtime.base_config, None);
        let judge_limits = self.runtime.budget.limits().await;
        let judge_timeout_ms = resolve_delegation_run_deadline_ms(
            &self.runtime.base_config,
            None,
            timeout_secs,
            judge_limits.run_deadline_ms,
        );
        let judge_cost_micros =
            nexa_core::usage_analytics::usage_cost_metadata(self.runtime.base_config.provider_type)
                .0;
        let invocation_id = format!(
            "judge:{}:{}",
            subtask_run_id
                .as_deref()
                .or(parent_task_run_id.as_deref())
                .or(conversation_id)
                .unwrap_or("detached"),
            call_id
        );
        let response = tokio::select! {
            _ = judge_cancel_token.cancelled() => {
                self.runtime.budget.release_reservation(reserved_tokens).await;
                let err = CoreError::Agent(
                    "Delegated adjudication was cancelled by the parent turn.".into()
                );
                let output = serde_json::json!({
                    "kind": "subagent_judgement_error",
                    "callLabel": call_id,
                    "error": err.to_string(),
                });
                finish_subtask_run_best_effort(
                    db,
                    subtask_run_id.as_deref(),
                    "failed",
                    Some(&output),
                    Some(&err.to_string()),
                );
                if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                    record_subtask_event(
                        db,
                        parent_run_id,
                        &format!("Subagent judge failed: {call_id}"),
                        "failed",
                        Some(&output),
                    );
                }
                return Err(err);
            }
            result = tokio::time::timeout(Duration::from_millis(judge_timeout_ms), provider.complete(&request)) => match result {
                Ok(Ok(response)) => {
                    self.runtime
                        .budget
                        .finish_call(reserved_tokens, &response.usage, judge_cost_micros)
                        .await;
                    response
                }
                Ok(Err(err)) => {
                    self.runtime.budget.release_reservation(reserved_tokens).await;
                    let output = serde_json::json!({
                        "kind": "subagent_judgement_error",
                        "callLabel": call_id,
                        "error": err.to_string(),
                    });
                    finish_subtask_run_best_effort(
                        db,
                        subtask_run_id.as_deref(),
                        "failed",
                        Some(&output),
                        Some(&err.to_string()),
                    );
                    if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                        record_subtask_event(
                            db,
                            parent_run_id,
                            &format!("Subagent judge failed: {call_id}"),
                            "failed",
                            Some(&output),
                        );
                    }
                    return Err(err);
                }
                Err(_) => {
                    judge_cancel_token.cancel();
                    self.runtime.budget.release_reservation(reserved_tokens).await;
                    let err = CoreError::Agent(format!(
                        "Delegated adjudication timed out after {judge_timeout_ms}ms."
                    ));
                    let output = serde_json::json!({
                        "kind": "subagent_judgement_error",
                        "callLabel": call_id,
                        "error": err.to_string(),
                    });
                    finish_subtask_run_best_effort(
                        db,
                        subtask_run_id.as_deref(),
                        "failed",
                        Some(&output),
                        Some(&err.to_string()),
                    );
                    if let Some(parent_run_id) = parent_task_run_id.as_deref() {
                        record_subtask_event(
                            db,
                            parent_run_id,
                            &format!("Subagent judge failed: {call_id}"),
                            "failed",
                            Some(&output),
                        );
                    }
                    return Err(err);
                }
            }
        };
        if let (Some(parent_run_id), Some(subtask_run_id)) =
            (parent_task_run_id.as_deref(), subtask_run_id.as_deref())
        {
            let elapsed_ms = instant_elapsed_ms(launch_started);
            for (stage, value, status) in [
                (
                    "provider_connect_ms",
                    Some(elapsed_ms),
                    "completion_boundary",
                ),
                ("first_sse_byte_ms", None, "not_applicable_completion_mode"),
                ("first_visible_token_ms", Some(elapsed_ms), "measured"),
                (
                    "frontend_first_paint_ms",
                    None,
                    "not_applicable_background_worker",
                ),
            ] {
                record_subagent_launch_metric(
                    db,
                    parent_run_id,
                    subtask_run_id,
                    call_id,
                    stage,
                    value,
                    Some(&invocation_id),
                    status,
                );
            }
        }

        let provider_type = self.runtime.base_config.provider_type;
        let provider_id = nexa_core::usage_analytics::provider_type_id(provider_type);
        let (estimated_cost_micros, currency, pricing_version) =
            nexa_core::usage_analytics::usage_cost_metadata(provider_type);
        let raw_usage =
            serde_json::to_value(&response.usage).unwrap_or_else(|_| serde_json::json!({}));
        if let Err(error) = db.record_ai_usage(&nexa_core::usage_analytics::AiUsageRecordInput {
            invocation_id: &invocation_id,
            occurred_at: None,
            provider_id,
            provider_type: provider_id,
            model_id: &model,
            raw_model_id: Some(&model),
            modality: "language_model",
            operation_kind: "judge",
            conversation_id,
            turn_id: None,
            run_id: parent_task_run_id.as_deref(),
            subtask_run_id: subtask_run_id.as_deref(),
            project_id: None,
            prompt_tokens: u64::from(response.usage.prompt_tokens),
            completion_tokens: u64::from(response.usage.completion_tokens),
            thinking_tokens: u64::from(response.usage.thinking_tokens.unwrap_or(0)),
            total_tokens: u64::from(
                response.usage.total_tokens.max(
                    response
                        .usage
                        .prompt_tokens
                        .saturating_add(response.usage.completion_tokens),
                ),
            ),
            cache_read_tokens: u64::from(response.usage.cache_read_tokens.unwrap_or(0)),
            cache_miss_tokens: u64::from(response.usage.cache_miss_tokens.unwrap_or(0)),
            cache_creation_tokens: u64::from(response.usage.cache_creation_tokens.unwrap_or(0)),
            usage_source: "provider",
            request_status: "success",
            latency_ms: None,
            estimated_cost_micros,
            currency,
            pricing_version,
            provider_raw: &raw_usage,
        }) {
            warn!("Failed to persist judge usage: {error}");
        }

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
        let artifact_value = serde_json::to_value(&artifact).unwrap_or_default();
        let output = serde_json::json!({
            "kind": "subagent_judgement",
            "judgement": &artifact,
        });
        finish_subtask_run_best_effort(
            db,
            subtask_run_id.as_deref(),
            "completed",
            Some(&output),
            None,
        );
        if let Some(parent_run_id) = parent_task_run_id.as_deref() {
            record_subtask_event(
                db,
                parent_run_id,
                &format!("Subagent judge completed: {call_id}"),
                "completed",
                Some(&output),
            );
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: summary,
            is_error: false,
            artifacts: Some(artifact_value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::conversation::{ConversationMessage, CreateConversationInput};
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
            None,
            None,
        )
    }

    fn observed_batch_run(id: &str) -> SubagentRunArtifact {
        failed_subagent_run_artifact(
            id.to_string(),
            SpawnSubagentArgs {
                task: format!("task-{id}"),
                task_id: None,
                role_id: None,
                role: None,
                model_policy: None,
                context: None,
                expected_output: None,
                max_iterations: None,
                timeout_secs: None,
                acceptance_criteria: None,
                evidence_chunk_ids: None,
                source_ids: None,
                allowed_tools: None,
                parallel_group: None,
                deliverable_style: None,
                return_sections: None,
            },
            None,
            &CoreError::Agent(format!("settled-{id}")),
        )
    }

    #[tokio::test]
    async fn observe_batch_returns_after_one_new_supplemental_result() {
        let runtime = test_runtime();
        runtime.register_batch("batch-1", 3);
        runtime.record_batch_result("batch-1", 0, observed_batch_run("first"));
        let tool = ObserveSubagentBatchTool::from_runtime(runtime.clone());
        let db = Database::open_memory().unwrap();
        let arguments = serde_json::json!({
            "batchId": "batch-1",
            "waitMs": 120_000,
        })
        .to_string();
        let source_scope = Vec::new();
        let observe = tool.execute(nexa_core::tools::ToolExecutionContext::new(
            "observe-1",
            &arguments,
            &db,
            &source_scope,
        ));
        let complete_next = async {
            tokio::task::yield_now().await;
            runtime.record_batch_result("batch-1", 1, observed_batch_run("second"));
        };

        let (result, ()) = tokio::time::timeout(Duration::from_millis(250), async {
            tokio::join!(observe, complete_next)
        })
        .await
        .expect("observation returns after one new result");
        let artifacts = result.unwrap().artifacts.unwrap();

        assert_eq!(artifacts["completedWorkers"], 2);
        assert_eq!(artifacts["pendingWorkers"], 1);
    }

    #[test]
    fn test_normalize_spawn_args_clamps_timeout() {
        let args = normalize_spawn_args(SpawnSubagentArgs {
            task: "Investigate".into(),
            task_id: Some("  worker-1  ".into()),
            role_id: None,
            role: None,
            model_policy: None,
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
        assert_eq!(args.task_id.as_deref(), Some("worker-1"));
    }

    #[test]
    fn test_delegation_timeout_treats_unlimited_parent_as_default_budget() {
        let mut config = AgentConfig::default();
        config.tool_timeout_secs = Some(0);
        config.agent_timeout_secs = Some(0);

        assert_eq!(resolve_delegation_timeout_secs(&config, None), 120);
    }

    #[test]
    fn test_model_policy_routes_only_to_same_provider_auxiliary_model() {
        let mut config = AgentConfig {
            model: Some("gpt-5".into()),
            summarization_model: Some("gpt-5-mini".into()),
            summarization_provider_type: Some(ProviderType::OpenAi),
            ..AgentConfig::default()
        };
        let openai = ProviderConfig {
            provider_type: ProviderType::OpenAi,
            base_url: None,
            api_key: None,
            org_id: None,
            timeout_secs: None,
        };
        assert!(!apply_delegated_model_policy(
            &mut config,
            &openai,
            Some(&ModelRoutingClass::Fast)
        ));
        assert_eq!(config.model.as_deref(), Some("gpt-5-mini"));

        config.model = Some("claude-opus".into());
        config.summarization_model = Some("gpt-5-mini".into());
        let anthropic = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            base_url: None,
            api_key: None,
            org_id: None,
            timeout_secs: None,
        };
        assert!(apply_delegated_model_policy(
            &mut config,
            &anthropic,
            Some(&ModelRoutingClass::IndependentReviewer)
        ));
        assert_eq!(config.model.as_deref(), Some("claude-opus"));
    }

    #[tokio::test]
    async fn test_budget_uses_realistic_default_token_budget() {
        let budget = SubagentBudgetController::new(&AgentConfig::default());
        let snapshot = budget.snapshot().await;

        assert_eq!(snapshot.token_budget, 32_000);
    }

    #[test]
    fn test_default_subagent_tools_include_read_only_web_research() {
        let tools = default_subagent_tool_names();

        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"web_research_context".to_string()));
        assert!(tools.contains(&"browser_evidence_capture".to_string()));
        assert!(!tools.contains(&"desktop_automation".to_string()));
        assert!(!tools.contains(&"edit_file".to_string()));
        assert!(!tools.contains(&"multi_edit".to_string()));
    }

    #[test]
    fn test_normalize_spawn_args_accepts_structured_role_id() {
        let args = normalize_spawn_args(SpawnSubagentArgs {
            task: "Check the draft".into(),
            task_id: None,
            role_id: Some("Verifier".into()),
            role: None,
            model_policy: None,
            context: None,
            expected_output: None,
            max_iterations: None,
            timeout_secs: None,
            acceptance_criteria: None,
            evidence_chunk_ids: None,
            source_ids: None,
            allowed_tools: None,
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        })
        .unwrap();

        assert_eq!(args.role_id.as_deref(), Some("verifier"));
        let profile = resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())
            .unwrap()
            .unwrap();
        assert_eq!(profile.label, "Verifier");
        assert_eq!(
            build_return_sections(&args, Some(profile)),
            vec![
                "Verdict".to_string(),
                "Checks performed".to_string(),
                "Unverified or risky claims".to_string()
            ]
        );
    }

    #[test]
    fn test_unknown_role_id_is_rejected() {
        let err = normalize_spawn_args(SpawnSubagentArgs {
            task: "Check the draft".into(),
            task_id: None,
            role_id: Some("wizard".into()),
            role: None,
            model_policy: None,
            context: None,
            expected_output: None,
            max_iterations: None,
            timeout_secs: None,
            acceptance_criteria: None,
            evidence_chunk_ids: None,
            source_ids: None,
            allowed_tools: None,
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        })
        .unwrap_err();

        assert!(err.to_string().contains("Unknown subagent role_id"));
    }

    #[test]
    fn test_role_profile_narrows_default_tools() {
        let base_tools = vec![
            "search_knowledge_base".to_string(),
            "web_search".to_string(),
            "web_research_context".to_string(),
            "desktop_automation".to_string(),
            "run_shell".to_string(),
            "record_verification".to_string(),
        ];
        let verifier = role_profile_by_id("verifier").unwrap();
        let tools = resolve_allowed_tools_for_role(&base_tools, None, Some(verifier));

        assert!(tools.contains(&"search_knowledge_base".to_string()));
        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"web_research_context".to_string()));
        assert!(tools.contains(&"record_verification".to_string()));
        assert!(!tools.contains(&"desktop_automation".to_string()));
        assert!(!tools.contains(&"run_shell".to_string()));
    }

    #[test]
    fn test_runtime_saves_subagent_session_snapshot() {
        let runtime = test_runtime();
        runtime.save_session_snapshot(SubagentSessionSnapshot {
            task_id: "worker-1".to_string(),
            last_run_id: "run-1".to_string(),
            task: "Investigate".to_string(),
            role_id: Some("researcher".to_string()),
            role_name: Some("Researcher".to_string()),
            result: "Prior result".to_string(),
            finish_reason: Some("stop".to_string()),
            usage_total: Usage::default(),
            tool_event_count: 2,
        });

        let snapshot = runtime
            .get_session_snapshot("worker-1")
            .expect("snapshot should be saved");
        assert_eq!(snapshot.last_run_id, "run-1");
        assert_eq!(snapshot.result, "Prior result");
        assert_eq!(snapshot.tool_event_count, 2);
    }

    #[test]
    fn test_workflow_template_expands_role_based_tasks() {
        let template = workflow_template_by_id("research_verify").unwrap();
        let tasks = expand_workflow_template_tasks(
            template,
            "Decide whether the proposal is supported",
            None,
        );

        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].role_id.as_deref(), Some("researcher"));
        assert_eq!(tasks[1].role_id.as_deref(), Some("verifier"));
        assert_eq!(tasks[2].role_id.as_deref(), Some("critic"));
        assert_eq!(tasks[0].parallel_group.as_deref(), Some("research_verify"));
        assert!(tasks[0]
            .task
            .contains("Decide whether the proposal is supported"));
        assert!(tasks[0]
            .return_sections
            .as_ref()
            .is_some_and(|sections| sections.iter().any(|section| section == "Conclusion")));
    }

    #[test]
    fn test_child_runtime_blocks_recursive_delegation() {
        let runtime = test_runtime();
        assert!(runtime.can_delegate_further());

        let child = runtime.spawn_child_runtime(CancellationToken::new());
        assert!(!child.can_delegate_further());
    }

    #[tokio::test]
    async fn test_budget_reservations_are_soft_for_parallel_fanout() {
        let config = AgentConfig {
            subagent_token_budget: Some(256),
            ..Default::default()
        };

        let budget = SubagentBudgetController::new(&config);
        let cancel_token = CancellationToken::new();
        let permit = budget
            .begin_call("worker-a", 220, false, &cancel_token)
            .await
            .unwrap();
        let snapshot = budget.snapshot().await;
        assert_eq!(snapshot.tokens_reserved, 220);
        assert_eq!(snapshot.remaining_tokens, 36);

        let second = budget
            .begin_call("worker-b", 50, false, &cancel_token)
            .await;
        assert!(second.is_ok(), "estimated reservations are a soft budget");
        drop(second);

        drop(permit);
        budget.release_reservation(220).await;
        budget.release_reservation(50).await;
        assert_eq!(budget.snapshot().await.tokens_reserved, 0);
    }

    #[tokio::test]
    async fn test_cancelled_worker_queue_releases_budget_reservation() {
        let config = AgentConfig {
            subagent_max_parallel: Some(1),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let active_cancel = CancellationToken::new();
        let active_permit = budget
            .begin_call("worker-a", 200, false, &active_cancel)
            .await
            .unwrap();

        let queued_budget = budget.clone();
        let queued_cancel = CancellationToken::new();
        let queued_cancel_for_task = queued_cancel.clone();
        let queued = tokio::spawn(async move {
            queued_budget
                .begin_call("worker-b", 300, false, &queued_cancel_for_task)
                .await
        });
        tokio::task::yield_now().await;

        let queued_snapshot = budget.snapshot().await;
        assert_eq!(
            queued_snapshot.calls_started, 1,
            "queued admission must not consume call count before a worker slot exists"
        );
        assert_eq!(
            queued_snapshot.tokens_reserved, 200,
            "queued admission must not reserve output credit before a worker slot exists"
        );
        queued_cancel.cancel();

        assert!(queued.await.unwrap().is_err());
        let snapshot = budget.snapshot().await;
        assert_eq!(snapshot.calls_started, 1);
        assert_eq!(snapshot.tokens_reserved, 200);

        drop(active_permit);
        budget.release_reservation(200).await;
    }

    #[tokio::test]
    async fn test_nexus_preserves_tokens_and_a_call_for_verification() {
        let config = AgentConfig {
            subagent_max_calls_per_turn: Some(3),
            subagent_token_budget: Some(1_000),
            subagent_verification_reserve_percent: Some(25),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel_token = CancellationToken::new();

        let worker = budget
            .begin_call("worker-a", 700, false, &cancel_token)
            .await
            .unwrap();
        assert!(budget
            .begin_call("worker-b", 100, false, &cancel_token)
            .await
            .is_err());
        let verifier = budget
            .begin_call("verifier", 300, true, &cancel_token)
            .await;
        assert!(verifier.is_ok());

        drop(verifier);
        drop(worker);
        budget.release_reservation(700).await;
        budget.release_reservation(300).await;
        let snapshot = budget.snapshot().await;
        assert_eq!(snapshot.verification_reserve_tokens, 0);
        assert_eq!(snapshot.exploration_lane_slots, 1);
        assert_eq!(snapshot.verification_lane_slots, 1);
        assert_eq!(snapshot.judge_lane_slots, 1);
        assert_eq!(snapshot.calls_started, 2);
    }

    #[tokio::test]
    async fn test_nexus_verifier_cannot_consume_the_reserved_judge_call() {
        let config = AgentConfig {
            subagent_max_calls_per_turn: Some(3),
            subagent_verification_reserve_percent: Some(25),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();

        let worker = budget
            .begin_call("worker", 100, false, &cancel)
            .await
            .unwrap();
        let verifier = budget
            .begin_call("verifier", 100, true, &cancel)
            .await
            .unwrap();
        assert!(budget
            .begin_call("second-verifier", 100, true, &cancel)
            .await
            .is_err());
        let judge = budget
            .begin_judge_call("judge", 100, &cancel)
            .await
            .expect("judge keeps its reserved call admission");

        drop((worker, verifier, judge));
        for _ in 0..3 {
            budget.release_reservation(100).await;
        }
        assert_eq!(budget.snapshot().await.calls_started, 3);
    }

    #[tokio::test]
    async fn test_small_custom_call_budget_keeps_exploration_admissible() {
        let config = AgentConfig {
            subagent_max_parallel: Some(3),
            subagent_max_calls_per_turn: Some(2),
            subagent_verification_reserve_percent: Some(25),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();

        let first = budget
            .begin_call("worker-a", 100, false, &cancel)
            .await
            .expect("a small custom call budget must still admit exploration");
        let second = budget
            .begin_call("worker-b", 100, false, &cancel)
            .await
            .expect("all explicitly configured calls remain usable without control lanes");

        drop((first, second));
        assert_eq!(budget.snapshot().await.calls_started, 2);
    }

    #[tokio::test]
    async fn test_worker_queue_has_an_independent_deadline() {
        let config = AgentConfig {
            subagent_max_parallel: Some(1),
            ..Default::default()
        };
        let budget =
            SubagentBudgetController::new_with_queue_deadline(&config, Duration::from_millis(10));
        let cancel = CancellationToken::new();
        let active = budget
            .begin_call("worker-a", 100, false, &cancel)
            .await
            .unwrap();

        let error = budget
            .begin_call("worker-b", 100, false, &cancel)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("queue deadline"));
        assert_eq!(budget.snapshot().await.calls_started, 1);
        drop(active);
        budget.release_reservation(100).await;
    }

    #[test]
    fn test_delegated_output_is_not_hard_clamped_to_32k() {
        let config = AgentConfig {
            max_tokens: Some(50_000),
            ..Default::default()
        };

        assert_eq!(resolve_delegated_max_output(&config, None), 50_000);
        assert_eq!(resolve_delegated_max_output(&config, Some(40_000)), 40_000);
    }

    #[test]
    fn progress_latch_coalesces_unbounded_stream_deltas() {
        let (sender, mut receiver) = mpsc::channel(1);

        for _ in 0..100_000 {
            signal_progress_latch(&sender);
        }

        assert_eq!(receiver.len(), 1);
        assert_eq!(receiver.try_recv(), Ok(()));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn provider_connect_does_not_consume_an_already_queued_first_response() {
        let (connected_tx, mut connected_rx) = mpsc::channel(1);
        let (response_tx, mut response_rx) = mpsc::channel(1);

        signal_progress_latch(&connected_tx);
        signal_progress_latch(&response_tx);

        assert_eq!(connected_rx.recv().await, Some(()));
        assert_eq!(response_rx.recv().await, Some(()));
    }

    #[tokio::test]
    async fn batch_slot_wait_shares_the_global_queue_deadline() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let _occupied = Arc::clone(&slots).acquire_owned().await.unwrap();
        let cancel = CancellationToken::new();

        let error = acquire_batch_slot(slots, &cancel, "queued-worker", Instant::now(), 20)
            .await
            .expect_err("batch-local admission must remain bounded");

        assert!(error.to_string().contains("20ms queue deadline"));
    }

    #[tokio::test]
    async fn batch_queue_failure_rolls_back_unstarted_call_and_token_credit() {
        let config = AgentConfig {
            subagent_max_parallel: Some(1),
            subagent_max_calls_per_turn: Some(2),
            subagent_token_budget: Some(1_000),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();
        let permit = budget
            .begin_call("queued", 100, false, &cancel)
            .await
            .unwrap();

        budget.rollback_unstarted_worker(100, false).await;
        let snapshot = budget.snapshot().await;

        assert_eq!(snapshot.calls_started, 0);
        assert_eq!(snapshot.tokens_reserved, 0);
        drop(permit);
    }

    #[tokio::test]
    async fn judge_startup_failure_rolls_back_global_and_judge_admission() {
        let config = AgentConfig {
            subagent_max_parallel: Some(3),
            subagent_max_calls_per_turn: Some(3),
            subagent_token_budget: Some(10_000),
            subagent_verification_reserve_percent: Some(25),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();
        let failed_judge = budget
            .begin_judge_call("failed-judge", 100, &cancel)
            .await
            .unwrap();

        budget.rollback_unstarted_judge(100).await;
        drop(failed_judge);
        let snapshot = budget.snapshot().await;
        assert_eq!(snapshot.calls_started, 0);
        assert_eq!(snapshot.tokens_reserved, 0);

        drop(
            budget
                .begin_call("explorer", 100, false, &cancel)
                .await
                .unwrap(),
        );
        drop(
            budget
                .begin_call("verifier", 100, true, &cancel)
                .await
                .unwrap(),
        );
        let error = budget
            .begin_call("extra-verifier", 100, true, &cancel)
            .await
            .expect_err("judge call credit must be reserved again after rollback");
        assert!(error.to_string().contains("remain reserved"));
    }

    #[test]
    fn v2_run_deadline_replaces_legacy_role_default_unless_call_is_explicitly_shorter() {
        let config = AgentConfig {
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                run_deadline_ms: Some(240_000),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            resolve_delegation_run_deadline_ms(&config, None, 60, 240_000),
            240_000
        );
        assert_eq!(
            resolve_delegation_run_deadline_ms(&config, Some(30), 60, 240_000),
            30_000
        );
        assert_eq!(
            resolve_delegation_run_deadline_ms(&AgentConfig::default(), None, 60, 180_000),
            60_000
        );
    }

    #[tokio::test]
    async fn unknown_remote_pricing_keeps_cost_limit_advisory_instead_of_blocking_workers() {
        let config = AgentConfig {
            provider_type: Some(ProviderType::OpenAi),
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                total_cost_soft_limit_micros: Some(1_000),
                max_parallel: Some(1),
                max_calls_per_turn: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();

        let permit = budget
            .begin_call("remote-worker", 100, false, &cancel)
            .await
            .expect("unknown pricing must not disable remote delegation");
        let snapshot = budget.snapshot().await;

        assert!(!snapshot.cost_accounting_available);
        assert_eq!(snapshot.cost_soft_limit_micros, Some(1_000));
        drop(permit);
    }

    #[tokio::test]
    async fn token_soft_limit_blocks_new_calls_while_residual_workers_are_running() {
        let config = AgentConfig {
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                total_actual_tokens_soft_limit: Some(256),
                max_parallel: Some(3),
                max_calls_per_turn: Some(4),
                ..Default::default()
            }),
            subagent_verification_reserve_percent: Some(0),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();
        let first = budget
            .begin_call("first", 100, false, &cancel)
            .await
            .unwrap();
        let residual = budget
            .begin_call("residual", 100, false, &cancel)
            .await
            .unwrap();
        budget
            .finish_call(
                100,
                &Usage {
                    total_tokens: 300,
                    ..Default::default()
                },
                None,
            )
            .await;
        drop(first);

        let error = budget
            .begin_call("new-worker", 100, false, &cancel)
            .await
            .expect_err("actual usage over the soft limit must stop new admission");

        assert!(error.to_string().contains("token soft limit exhausted"));
        drop(residual);
    }

    #[test]
    fn independent_auto_limits_prefer_model_catalog_over_parent_limits() {
        let mut config = AgentConfig {
            context_window: Some(128_000),
            max_tokens: Some(8_192),
            ..Default::default()
        };

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Auto,
            Some(1_000_000),
            Some(65_536),
            true,
        );

        assert_eq!(config.context_window, Some(1_000_000));
        assert_eq!(config.max_tokens, Some(65_536));
    }

    #[test]
    fn independent_auto_output_uses_conservative_fallback_without_catalog_data() {
        let mut config = AgentConfig {
            max_tokens: Some(8_192),
            ..Default::default()
        };

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Auto,
            None,
            None,
            true,
        );

        assert_eq!(config.max_tokens, Some(CONSERVATIVE_SUBAGENT_MAX_TOKENS));
    }

    #[test]
    fn test_delegated_failure_status_preserves_deadline_and_error_semantics() {
        for message in [
            "exceeded its 30000ms provider-connect deadline",
            "exceeded its 45000ms first-token deadline",
            "exceeded its 15000ms queue deadline",
            "timed out after 60s",
        ] {
            assert_eq!(delegated_failure_status(message), "timed_out");
        }
        assert_eq!(
            delegated_failure_status("was cancelled by the parent turn"),
            "cancelled"
        );
        assert_eq!(
            delegated_failure_status("authentication failed with status 401"),
            "failed"
        );
    }

    #[tokio::test]
    async fn test_delegation_runtime_uses_distinct_connection_and_first_token_deadlines() {
        let limits = SubagentBudgetController::new(&AgentConfig::default())
            .limits()
            .await;

        assert!(limits.connect_deadline_ms > 0);
        assert!(limits.first_token_deadline_ms > limits.connect_deadline_ms);
    }

    #[tokio::test]
    async fn delegation_limits_v2_overrides_legacy_dimensions_and_deadlines() {
        let config = AgentConfig {
            provider_type: Some(ProviderType::Ollama),
            subagent_max_parallel: Some(2),
            subagent_token_budget: Some(12_000),
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                input_context_limit: Some(1_000_000),
                max_output_tokens_per_worker: Some(65_536),
                total_actual_tokens_soft_limit: Some(240_000),
                total_cost_soft_limit_micros: Some(1_000),
                max_parallel: Some(6),
                max_calls_per_turn: Some(12),
                queue_deadline_ms: Some(5_000),
                connect_deadline_ms: Some(20_000),
                first_token_deadline_ms: Some(60_000),
                run_deadline_ms: Some(240_000),
            }),
            ..Default::default()
        };

        let limits = SubagentBudgetController::new(&config).limits().await;

        assert_eq!(limits.max_parallel, 6);
        assert_eq!(limits.max_calls_per_turn, 12);
        assert_eq!(
            limits.input_context_policy,
            DelegationLimitPolicy::Explicit(1_000_000)
        );
        assert_eq!(
            limits.max_output_tokens_per_worker,
            DelegationLimitPolicy::Explicit(65_536)
        );
        assert_eq!(limits.total_actual_tokens_soft_limit, Some(240_000));
        assert_eq!(limits.total_cost_soft_limit_micros, Some(1_000));
        assert!(limits.cost_accounting_available);
        assert_eq!(limits.queue_deadline_ms, 5_000);
        assert_eq!(limits.connect_deadline_ms, 20_000);
        assert_eq!(limits.first_token_deadline_ms, 60_000);
        assert_eq!(limits.run_deadline_ms, 240_000);
    }

    #[test]
    fn test_context_snapshot_reuses_authorized_parent_history() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "google".to_string(),
                model: "gemini-2.5-pro".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        db.add_message(&ConversationMessage {
            id: "parent-message".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Parent context that the delegated worker needs".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 10,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        })
        .unwrap();

        let first = load_delegation_context_snapshot(
            &db,
            Some(&conversation.id),
            "gemini-2.5-pro",
            1_048_576,
        );
        let second = load_delegation_context_snapshot(
            &db,
            Some(&conversation.id),
            "gemini-2.5-pro",
            1_048_576,
        );

        assert_eq!(first.id, second.id);
        assert_eq!(first.selected_message_ids.as_ref(), &["parent-message"]);
        assert_eq!(
            first.messages[0].text_content(),
            "Parent context that the delegated worker needs"
        );
        assert_eq!(first.context_limit, 1_048_576);
    }

    #[test]
    fn test_batch_completion_policy_resolves_quorum_and_deadline() {
        let quorum_args = SpawnSubagentBatchArgs {
            tasks: Vec::new(),
            batch_goal: None,
            workflow_template: None,
            parallel_group: None,
            max_parallel: None,
            completion_policy: Some("quorum".to_string()),
            quorum: Some(3),
            deadline_ms: None,
            cancel_remaining: None,
        };
        assert_eq!(
            DelegationCompletionPolicy::resolve(&quorum_args, 4).unwrap(),
            DelegationCompletionPolicy::Quorum { required: 3 }
        );

        let deadline_args = SpawnSubagentBatchArgs {
            completion_policy: Some("deadline".to_string()),
            deadline_ms: Some(2_500),
            ..quorum_args
        };
        assert_eq!(
            DelegationCompletionPolicy::resolve(&deadline_args, 4).unwrap(),
            DelegationCompletionPolicy::Deadline { deadline_ms: 2_500 }
        );

        let parent_args = SpawnSubagentBatchArgs {
            completion_policy: Some("parent_decides".to_string()),
            ..deadline_args
        };
        let parent_policy = DelegationCompletionPolicy::resolve(&parent_args, 4).unwrap();
        assert_eq!(parent_policy, DelegationCompletionPolicy::ParentDecides);
        assert!(!parent_policy.is_satisfied(&[], 4));
        assert!(!parent_policy.is_satisfied(&[], 1));
        assert!(parent_policy.is_satisfied(&[observed_batch_run("decision")], 3));
        assert!(parent_policy.is_satisfied(&[], 0));

        let schema = spawn_subagent_batch_parameters_schema();
        assert_eq!(schema["properties"]["cancel_remaining"]["type"], "boolean");
    }
}
