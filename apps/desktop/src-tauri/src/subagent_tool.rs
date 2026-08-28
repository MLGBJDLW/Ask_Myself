use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use log::warn;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::delegation_scheduler::{
    BudgetSnapshot, DelegationLimitPolicy, DelegationScheduler as SubagentBudgetController,
};
use crate::subagent_lifecycle::{
    RegisterSubagentRequest, SubagentEventBridge, SubagentLifecycleEventKind,
    SubagentLifecycleRuntime, SubagentLifecycleStatus,
};

use nexa_core::agent::context::estimate_tool_tokens_for_model;
use nexa_core::agent::{
    llm_streaming_disabled_by_env, AgentConfig, AgentEvent, AgentExecutor, AgentRequestKind,
    AgentSteeringMessage, CancellationToken,
};
use nexa_core::conversation::memory::{
    estimate_tokens_for_model, ContextWindowAuthority, ResolvedContextWindow,
};
#[cfg(test)]
use nexa_core::conversation::memory::{model_context_window, resolve_model_context_window};
use nexa_core::conversation::{
    conversation_message_llm_context_content, conversation_message_provider_turn,
};
use nexa_core::db::Database;
use nexa_core::error::CoreError;
use nexa_core::llm::message_validation::{
    normalize_assistant_message, validate_message_sequence, InvalidAssistantHandling,
    MessageNormalizationContext, MessageSource,
};
use nexa_core::llm::{
    create_provider, provider_uses_non_streaming_fallback, CompletionRequest, ContentPart, Message,
    ProviderConfig, ProviderType, ReasoningEffort, Role, Usage,
};
use nexa_core::provider_catalog::{
    model_capabilities_from_catalog, model_limits_from_catalog,
    resolve_endpoint_model_context_window,
};
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
const SUBAGENT_INTERACTIVE_SURFACE_TOOLS: &[&str] = &[
    "browser_session",
    "computer_observe",
    "computer_control",
    "desktop_automation",
];

fn provider_catalog_key(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAi => "open_ai",
        ProviderType::OpenRouter => "openrouter",
        ProviderType::Anthropic => "anthropic",
        ProviderType::Google => "google",
        ProviderType::DeepSeek => "deep_seek",
        ProviderType::Ollama => "ollama",
        ProviderType::LmStudio => "lm_studio",
        ProviderType::AzureOpenAi => "azure_open_ai",
        ProviderType::Zhipu => "zhipu",
        ProviderType::Moonshot => "moonshot",
        ProviderType::Qwen => "qwen",
        ProviderType::AlibabaModelStudio => "alibaba_model_studio",
        ProviderType::SiliconFlow => "siliconflow",
        ProviderType::Doubao => "doubao",
        ProviderType::Yi => "yi",
        ProviderType::Baichuan => "baichuan",
        ProviderType::Custom => "custom",
    }
}

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
        name: "observe_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "wait_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "send_subagent_input",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "cancel_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "close_subagent",
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
        instructions: "Plan a narrow user-visible browser or desktop action for the supervisor to perform. Delegated workers do not receive interactive surface control or approval authority; inspect only supplied evidence, state the exact proposed action, and never infer private screen state you cannot observe.",
        default_sections: ROLE_DESKTOP_OPERATOR_SECTIONS,
        recommended_tools: &[
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

pub struct SubagentLifecycleTool {
    runtime: DelegationRuntime,
    action: SubagentLifecycleAction,
}

#[derive(Clone, Copy)]
enum SubagentLifecycleAction {
    Observe,
    Wait,
    SendInput,
    Cancel,
    Close,
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
    lifecycle: SubagentLifecycleRuntime,
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

impl SubagentLifecycleTool {
    pub fn all(runtime: DelegationRuntime) -> Vec<Self> {
        [
            SubagentLifecycleAction::Observe,
            SubagentLifecycleAction::Wait,
            SubagentLifecycleAction::SendInput,
            SubagentLifecycleAction::Cancel,
            SubagentLifecycleAction::Close,
        ]
        .into_iter()
        .map(|action| Self {
            runtime: runtime.clone(),
            action,
        })
        .collect()
    }
}

impl DelegationRuntime {
    pub fn new(
        provider_config: ProviderConfig,
        base_config: AgentConfig,
        allowed_tools: Option<Vec<String>>,
        allowed_skill_ids: Option<Vec<String>>,
        lifecycle: SubagentLifecycleRuntime,
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
            lifecycle,
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
            lifecycle: self.lifecycle.clone(),
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
            lifecycle: self.lifecycle.clone(),
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
        context_limit: Option<u32>,
        handoff_token_budget: u32,
    ) -> Arc<DelegationContextSnapshot> {
        let key = format!("{model}:{context_limit:?}:{handoff_token_budget}");
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
            handoff_token_budget,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentLifecycleArgs {
    agent_id: String,
    #[serde(default)]
    after_seq: Option<u64>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    input: Option<String>,
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
    context_limit: Option<u32>,
    handoff_token_budget: u32,
    dropped_invalid_messages: usize,
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
    preflight_failure: Option<SubagentPreflightFailure>,
    preflight: Option<SubagentPreflightReport>,
    context_snapshot: Option<serde_json::Value>,
    effective_model_budgets: Option<serde_json::Value>,
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
    context_limit: Option<u32>,
    handoff_token_budget: u32,
) -> DelegationContextSnapshot {
    // `context_limit` is model capacity; handoff is a separate parent-history
    // allocation. Never report the smaller allocation as the model window.
    let token_budget = context_limit.map_or(handoff_token_budget, |limit| {
        handoff_token_budget.min(limit)
    });
    let mut selected = Vec::new();
    let mut token_estimate = 0u32;
    let mut dropped_invalid_messages = 0usize;
    if let Some(conversation_id) = conversation_id {
        if let Ok(messages) = db.get_messages(conversation_id) {
            for message in messages.into_iter().rev() {
                // Delegate conversational intent and final assistant output, not
                // provider-specific tool protocol records. Evidence needed by a
                // child is handed off through typed evidence cards instead.
                if message.role == Role::Tool {
                    continue;
                }
                let content = conversation_message_llm_context_content(&message).to_string();
                let message_tokens = estimate_tokens_for_model(model, &content);
                if message_tokens > token_budget {
                    dropped_invalid_messages = dropped_invalid_messages.saturating_add(1);
                    continue;
                }
                if !selected.is_empty()
                    && token_estimate.saturating_add(message_tokens) > token_budget
                {
                    break;
                }
                let mut projected = Message::text(message.role.clone(), content);
                if message.role == Role::Assistant {
                    projected.reasoning_content =
                        nexa_core::conversation::conversation_message_reasoning_replay(&message);
                    if let Some(envelope) = conversation_message_provider_turn(&message) {
                        projected.set_provider_turn(envelope);
                    }
                }
                let context = MessageNormalizationContext {
                    provider: None,
                    model: Some(model),
                    conversation_id: Some(conversation_id),
                    turn_id: None,
                    message_index: selected.len(),
                    source: MessageSource::SubagentHandoff,
                    invalid_assistant: InvalidAssistantHandling::Drop,
                };
                match normalize_assistant_message(projected, &context) {
                    Ok(Some(projected)) => {
                        token_estimate = token_estimate.saturating_add(message_tokens);
                        selected.push((message.id, projected));
                    }
                    Ok(None) | Err(_) => {
                        dropped_invalid_messages = dropped_invalid_messages.saturating_add(1);
                    }
                }
            }
        }
    }
    selected.reverse();
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    match context_limit {
        Some(limit) => {
            hasher.update(&[1]);
            hasher.update(&limit.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hasher.update(&handoff_token_budget.to_le_bytes());
    for (id, message) in &selected {
        hasher.update(id.as_bytes());
        hasher.update(message.text_content().as_bytes());
        if let Some(tool_calls) = message.tool_calls.as_ref() {
            hasher.update(&serde_json::to_vec(tool_calls).unwrap_or_default());
        }
    }
    let (selected_message_ids, messages): (Vec<_>, Vec<_>) = selected.into_iter().unzip();
    DelegationContextSnapshot {
        id: hasher.finalize().to_hex().to_string(),
        selected_message_ids: Arc::from(selected_message_ids),
        messages: Arc::from(messages),
        token_estimate,
        context_limit,
        handoff_token_budget: token_budget,
        dropped_invalid_messages,
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
        Some(requested) => {
            if parent_scope.is_empty() {
                requested.to_vec()
            } else {
                let parent: BTreeSet<&str> = parent_scope.iter().map(String::as_str).collect();
                let narrowed: Vec<String> = requested
                    .iter()
                    .filter(|id| parent.contains(id.as_str()))
                    .cloned()
                    .collect();
                narrowed
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
        Some(requested) => {
            let allowed: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
            let narrowed: Vec<String> = requested
                .iter()
                .filter(|name| allowed.contains(name.as_str()))
                .cloned()
                .collect();
            narrowed
        }
        _ => base_allowed_tools.to_vec(),
    }
}

fn resolve_allowed_tools_for_role(
    base_allowed_tools: &[String],
    requested_allowed_tools: Option<&[String]>,
    role_profile: Option<&SubagentRoleProfile>,
) -> Vec<String> {
    let role_allowed_tools = match role_profile {
        Some(profile) => {
            let base: BTreeSet<&str> = base_allowed_tools.iter().map(String::as_str).collect();
            profile
                .recommended_tools
                .iter()
                .filter(|name| base.contains(**name))
                .map(|name| (*name).to_string())
                .collect()
        }
        None => base_allowed_tools.to_vec(),
    };
    resolve_allowed_tools(&role_allowed_tools, requested_allowed_tools)
}

const SUBAGENT_PREFLIGHT_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_PREFLIGHT_MARKER: &str = "NEXA_SUBAGENT_PREFLIGHT=";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SubagentPreflightStage {
    History,
    Provider,
    Policy,
    Budget,
    Timeout,
}

impl std::fmt::Display for SubagentPreflightStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::History => "history",
            Self::Provider => "provider",
            Self::Policy => "policy",
            Self::Budget => "budget",
            Self::Timeout => "timeout",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SubagentPreflightFailure {
    schema_version: u32,
    stage: SubagentPreflightStage,
    code: String,
    retryable: bool,
    message: String,
}

fn subagent_preflight_failure(
    stage: SubagentPreflightStage,
    code: &str,
    retryable: bool,
    message: impl Into<String>,
) -> CoreError {
    let failure = SubagentPreflightFailure {
        schema_version: SUBAGENT_PREFLIGHT_SCHEMA_VERSION,
        stage,
        code: code.to_string(),
        retryable,
        message: message.into(),
    };
    let encoded = serde_json::to_string(&failure).unwrap_or_else(|_| "{}".to_string());
    CoreError::InvalidInput(format!(
        "Subagent preflight failed at {} ({}): {}\n{SUBAGENT_PREFLIGHT_MARKER}{encoded}",
        failure.stage, failure.code, failure.message
    ))
}

fn subagent_preflight_failure_from_error(error: &CoreError) -> Option<SubagentPreflightFailure> {
    let CoreError::InvalidInput(message) = error else {
        return None;
    };
    let encoded = message.split_once(SUBAGENT_PREFLIGHT_MARKER)?.1.trim();
    serde_json::from_str(encoded).ok()
}

fn subagent_admission_failure(error: &CoreError) -> CoreError {
    let message = error.to_string();
    if message.contains("queue deadline") {
        subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "queue_deadline_exceeded",
            true,
            message,
        )
    } else if message.contains("cancelled while waiting") {
        subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "queue_wait_cancelled",
            false,
            message,
        )
    } else {
        subagent_preflight_failure(
            SubagentPreflightStage::Budget,
            "admission_rejected",
            false,
            message,
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubagentPreflightReport {
    schema_version: u32,
    completed_stages: Vec<SubagentPreflightStage>,
    provider_id: String,
    effective_model: String,
    inherited_tool_count: usize,
    requested_tool_count: usize,
    effective_tool_count: usize,
    requested_source_count: usize,
    effective_source_count: usize,
    context_message_count: usize,
    dropped_invalid_context_messages: usize,
    reserved_tokens: u32,
    remaining_token_budget: u32,
    remaining_call_budget: u32,
    run_deadline_ms: u64,
}

fn validate_subagent_preflight(
    args: &SpawnSubagentArgs,
    effective_model: &str,
    provider_id: &str,
    baseline_allowed_tools: &[String],
    effective_allowed_tools: &[String],
    inherited_source_scope: &[String],
    effective_source_scope: &[String],
    context_snapshot: &DelegationContextSnapshot,
) -> Result<SubagentPreflightReport, CoreError> {
    let history_context = MessageNormalizationContext {
        provider: Some(provider_id),
        model: Some(effective_model),
        conversation_id: None,
        turn_id: None,
        message_index: 0,
        source: MessageSource::SubagentHandoff,
        invalid_assistant: InvalidAssistantHandling::Reject,
    };
    validate_message_sequence(&context_snapshot.messages, history_context).map_err(|error| {
        subagent_preflight_failure(
            SubagentPreflightStage::History,
            "inherited_history_invalid",
            false,
            error.to_string(),
        )
    })?;

    if let Some(requested) = args.allowed_tools.as_deref() {
        let interactive: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|name| is_interactive_surface_tool(name))
            .collect();
        if !interactive.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "interactive_tool_requires_parent_proxy",
                false,
                format!(
                    "Delegated workers cannot directly control interactive browser or desktop surfaces: {}. Ask the parent agent to perform the approved action.",
                    interactive.join(", ")
                ),
            ));
        }
        let inherited: BTreeSet<&str> = baseline_allowed_tools.iter().map(String::as_str).collect();
        let denied: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|name| !inherited.contains(name))
            .collect();
        if !denied.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_scope_widening",
                false,
                format!(
                    "Requested tool(s) are not available to the parent: {}.",
                    denied.join(", ")
                ),
            ));
        }
        if !requested.is_empty() && effective_allowed_tools.is_empty() {
            return Err(subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_scope_empty_after_narrowing",
                false,
                "No requested tools remain after role and delegation-depth restrictions.",
            ));
        }
    }

    if let Some(requested) = args.source_ids.as_deref() {
        if !inherited_source_scope.is_empty() {
            let inherited: BTreeSet<&str> =
                inherited_source_scope.iter().map(String::as_str).collect();
            let denied: Vec<&str> = requested
                .iter()
                .map(String::as_str)
                .filter(|source_id| !inherited.contains(source_id))
                .collect();
            if !denied.is_empty() {
                return Err(subagent_preflight_failure(
                    SubagentPreflightStage::Policy,
                    "source_scope_widening",
                    false,
                    format!(
                        "Requested source(s) are outside the parent scope: {}.",
                        denied.join(", ")
                    ),
                ));
            }
        }
    }

    Ok(SubagentPreflightReport {
        schema_version: SUBAGENT_PREFLIGHT_SCHEMA_VERSION,
        completed_stages: vec![
            SubagentPreflightStage::History,
            SubagentPreflightStage::Provider,
            SubagentPreflightStage::Policy,
        ],
        provider_id: provider_id.to_string(),
        effective_model: effective_model.to_string(),
        inherited_tool_count: baseline_allowed_tools.len(),
        requested_tool_count: args.allowed_tools.as_ref().map_or(0, Vec::len),
        effective_tool_count: effective_allowed_tools.len(),
        requested_source_count: args.source_ids.as_ref().map_or(0, Vec::len),
        effective_source_count: effective_source_scope.len(),
        context_message_count: context_snapshot.messages.len(),
        dropped_invalid_context_messages: context_snapshot.dropped_invalid_messages,
        reserved_tokens: 0,
        remaining_token_budget: 0,
        remaining_call_budget: 0,
        run_deadline_ms: 0,
    })
}

fn finalize_subagent_preflight(
    report: &mut SubagentPreflightReport,
    budget: &BudgetSnapshot,
    reserved_tokens: u32,
    run_deadline_ms: u64,
) -> Result<(), CoreError> {
    if budget.remaining_calls == 0 {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Budget,
            "call_budget_exhausted",
            false,
            "No delegated call budget remains for this turn.",
        ));
    }
    report.reserved_tokens = reserved_tokens;
    report.remaining_token_budget = budget.remaining_tokens;
    report.remaining_call_budget = budget.remaining_calls;
    report.completed_stages.push(SubagentPreflightStage::Budget);

    if run_deadline_ms == 0 {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Timeout,
            "run_deadline_invalid",
            false,
            "The delegated run deadline is zero.",
        ));
    }
    report.run_deadline_ms = run_deadline_ms;
    report
        .completed_stages
        .push(SubagentPreflightStage::Timeout);
    Ok(())
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

async fn emit_subagent_lifecycle_event(
    bridge: Option<&SubagentEventBridge>,
    kind: SubagentLifecycleEventKind,
    detail: serde_json::Value,
) {
    let Some(bridge) = bridge else {
        return;
    };
    if let Err(error) = bridge.emit(kind, detail).await {
        warn!("Failed to bridge subagent lifecycle event {kind:?}: {error}");
    }
}

async fn flush_subagent_deltas(
    bridge: Option<&SubagentEventBridge>,
    pending_thinking: &mut String,
    pending_output: &mut String,
) {
    if !pending_thinking.is_empty() {
        let delta = std::mem::take(pending_thinking);
        emit_subagent_lifecycle_event(
            bridge,
            SubagentLifecycleEventKind::ThinkingDelta,
            serde_json::json!({ "delta": delta }),
        )
        .await;
    }
    if !pending_output.is_empty() {
        let delta = std::mem::take(pending_output);
        emit_subagent_lifecycle_event(
            bridge,
            SubagentLifecycleEventKind::OutputDelta,
            serde_json::json!({ "delta": delta }),
        )
        .await;
    }
}

async fn run_subagent_once(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    steering_rx: Option<mpsc::UnboundedReceiver<AgentSteeringMessage>>,
    lifecycle_events: Option<SubagentEventBridge>,
) -> Result<SubagentRunArtifact, CoreError> {
    let launch_started = Instant::now();
    if runtime.delegation_depth >= MAX_SUBAGENT_DELEGATION_DEPTH {
        return Err(subagent_preflight_failure(
            SubagentPreflightStage::Policy,
            "recursion_depth_exceeded",
            false,
            format!(
                "Recursive delegated execution is blocked beyond depth {}.",
                MAX_SUBAGENT_DELEGATION_DEPTH
            ),
        ));
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
    apply_nexus_worker_reasoning_policy(&mut config, role_profile);
    let effective_model = config.model.clone();
    let effective_model_id = effective_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| {
            subagent_preflight_failure(
                SubagentPreflightStage::Provider,
                "model_unresolved",
                false,
                "No effective model was resolved.",
            )
        })?
        .to_string();
    let provider = create_provider(runtime.provider_config.clone()).map_err(|error| {
        subagent_preflight_failure(
            SubagentPreflightStage::Provider,
            "provider_configuration_invalid",
            false,
            error.to_string(),
        )
    })?;
    let provider_id = provider.name().to_string();
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
    let resolved_model_context = resolve_endpoint_model_context_window(
        provider_catalog_key(runtime.provider_config.provider_type),
        runtime.provider_config.base_url.as_deref(),
        &effective_model_id,
        None,
    );
    let context_authority = apply_delegated_model_limits(
        &mut config,
        delegation_limits.input_context_policy,
        delegation_limits.max_output_tokens_per_worker,
        resolved_model_context,
        catalog_limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens),
        runtime.base_config.delegation_limits_v2.is_some(),
    );
    config.context_window_resolution = Some(ResolvedContextWindow {
        capacity_tokens: config.context_window,
        authority: context_authority,
    });
    if let Some(worker_limit) = delegation_limits
        .max_actual_tokens_per_worker
        .and_then(|limit| u32::try_from(limit).ok())
    {
        config.max_tokens = Some(config.max_tokens.unwrap_or(worker_limit).min(worker_limit));
        config.max_actual_tokens_per_run = Some(worker_limit);
    }
    let model_context_limit = config.context_window;
    let handoff_budget_snapshot = runtime.budget.snapshot().await;
    let fair_share_divisor = handoff_budget_snapshot
        .max_parallel
        .min(handoff_budget_snapshot.remaining_calls)
        .max(1);
    let fair_share = handoff_budget_snapshot.remaining_tokens / fair_share_divisor;
    let control_lane_role = matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    );
    let automatic_handoff_budget = if control_lane_role {
        delegation_limits
            .max_actual_tokens_per_worker
            .and_then(|limit| u32::try_from(limit).ok())
            .unwrap_or(fair_share)
            .saturating_mul(3)
            / 5
    } else {
        fair_share.saturating_mul(3) / 5
    };
    let mut handoff_token_budget = delegation_limits
        .handoff_context_tokens_per_worker
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(automatic_handoff_budget);
    if let Some(model_context_limit) = model_context_limit {
        handoff_token_budget = handoff_token_budget
            .min(model_context_limit.saturating_mul(3) / 5)
            .max(1.min(model_context_limit));
    } else {
        handoff_token_budget = handoff_token_budget.max(1);
    }
    let context_snapshot = runtime.context_snapshot(
        &db,
        effective_model.as_deref().unwrap_or("default"),
        model_context_limit,
        handoff_token_budget,
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

    let available_tool_names = runtime
        .get_tool_registry()
        .map_err(|error| {
            subagent_preflight_failure(
                SubagentPreflightStage::Policy,
                "tool_registry_unavailable",
                false,
                error.to_string(),
            )
        })?
        .tool_names();
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
    // Interactive browser/computer control is parent-scoped until delegated
    // workers have a parent approval proxy and a surface capability lease.
    // `conversation_id=None` must never become a shared tenant key.
    effective_allowed_tools.retain(|name| !is_interactive_surface_tool(name));
    let effective_source_scope =
        resolve_source_scope(&inherited_source_scope, args.source_ids.as_deref());
    let mut preflight = validate_subagent_preflight(
        &args,
        &effective_model_id,
        &provider_id,
        &baseline_allowed_tools,
        &effective_allowed_tools,
        &inherited_source_scope,
        &effective_source_scope,
        &context_snapshot,
    )?;
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
        build_subagent_executor_tools(&runtime, &effective_allowed_tools, &worker_cancel_token)
            .map_err(|error| {
                subagent_preflight_failure(
                    SubagentPreflightStage::Policy,
                    "tool_registry_construction_failed",
                    false,
                    error.to_string(),
                )
            })?;
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
    let inherited_skill_tokens = enabled_skills.iter().fold(0_u32, |total, skill| {
        total.saturating_add(estimate_tokens_for_model(
            &effective_model_id,
            &skill.content,
        ))
    });
    let reserved_tokens = estimate_reserved_tokens(
        &config,
        &request_text,
        &tools,
        context_snapshot.token_estimate,
        inherited_skill_tokens,
        initial_output_credit,
    );
    let budget_snapshot = runtime.budget.snapshot().await;
    finalize_subagent_preflight(
        &mut preflight,
        &budget_snapshot,
        reserved_tokens,
        run_deadline_ms,
    )?;
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
    subtask_input["inheritedSkillTokens"] = serde_json::json!(inherited_skill_tokens);
    subtask_input["skillIndexGeneration"] = serde_json::json!(&skill_index.generation);
    subtask_input["preflight"] =
        serde_json::to_value(&preflight).unwrap_or_else(|_| serde_json::json!({}));
    let context_snapshot_artifact = serde_json::json!({
        "id": &context_snapshot.id,
        "selectedMessageIds": &context_snapshot.selected_message_ids,
        "tokenEstimate": context_snapshot.token_estimate,
        "contextCapacity": context_snapshot.context_limit,
        "contextAuthority": context_authority,
        "handoffTokenBudget": context_snapshot.handoff_token_budget,
        "droppedInvalidMessages": context_snapshot.dropped_invalid_messages,
    });
    let output_authority = match delegation_limits.max_output_tokens_per_worker {
        DelegationLimitPolicy::Explicit(_) => "user_override",
        DelegationLimitPolicy::Auto
            if catalog_limits
                .as_ref()
                .and_then(|limits| limits.max_output_tokens)
                .is_some() =>
        {
            "catalog_ceiling"
        }
        DelegationLimitPolicy::Auto => "safe_default",
    };
    let effective_model_budgets = serde_json::json!({
        "contextCapacity": config.context_window,
        "parentHistoryHandoff": context_snapshot.handoff_token_budget,
        "maxOutputPerStep": config.max_tokens,
        "maxActualTokensPerWorker": config.max_actual_tokens_per_run,
        "contextAuthority": context_authority,
        "outputAuthority": output_authority,
    });
    subtask_input["contextSnapshot"] = context_snapshot_artifact.clone();
    subtask_input["effectiveModelBudgets"] = effective_model_budgets.clone();
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
            let err = subagent_admission_failure(&err);
            let output = serde_json::json!({
                "kind": "subagent_run_error",
                "callLabel": &call_label,
                "error": err.to_string(),
                "preflight": subagent_preflight_failure_from_error(&err),
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
                let error = subagent_preflight_failure(
                    SubagentPreflightStage::Timeout,
                    "batch_queue_deadline_exceeded",
                    true,
                    error.to_string(),
                );
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
    let estimated_cost_micros =
        nexa_core::usage_analytics::usage_cost_metadata(Some(effective_provider_type)).0;
    let non_streaming_completion = llm_streaming_disabled_by_env()
        || provider_uses_non_streaming_fallback(
            effective_provider_type,
            effective_model.as_deref().unwrap_or_default(),
        );

    let mut executor = AgentExecutor::new(provider, tools, config)
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
    if let Some(steering_rx) = steering_rx {
        executor = executor.with_steering_receiver(steering_rx);
    }

    let (tx, event_rx) = mpsc::channel::<AgentEvent>(64);
    let (fatal_error_tx, mut fatal_error_rx) = mpsc::unbounded_channel::<String>();
    let (provider_connected_tx, mut provider_connected_rx) = mpsc::channel::<()>(1);
    let (first_response_tx, mut first_response_rx) = mpsc::channel::<()>(1);
    let capture_cancel_token = worker_cancel_token.clone();
    let worker_actual_token_limit = delegation_limits
        .max_actual_tokens_per_worker
        .and_then(|limit| u32::try_from(limit).ok());
    let telemetry_db = db.clone();
    let telemetry_identity = parent_task_run_id.clone().zip(subtask_run_id.clone());
    let telemetry_call_label = call_label.clone();
    let lifecycle_capture = lifecycle_events.clone();
    let mut event_task = tokio::spawn(async move {
        let mut event_rx = event_rx;
        let mut capture = EventCapture::default();
        let mut provider_invocation_index = 0_u32;
        let mut active_provider_invocation_id: Option<String> = None;
        let mut first_provider_output_recorded = false;
        let mut pending_thinking = String::new();
        let mut pending_output = String::new();
        let mut last_delta_flush = Instant::now();
        let mut worker_token_limit_exceeded = false;

        loop {
            let event =
                match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => {
                        flush_subagent_deltas(
                            lifecycle_capture.as_ref(),
                            &mut pending_thinking,
                            &mut pending_output,
                        )
                        .await;
                        break;
                    }
                    Err(_) => {
                        flush_subagent_deltas(
                            lifecycle_capture.as_ref(),
                            &mut pending_thinking,
                            &mut pending_output,
                        )
                        .await;
                        last_delta_flush = Instant::now();
                        continue;
                    }
                };
            let provider_connected = matches!(
                &event,
                AgentEvent::ControllerStatus { code, .. } if code == "provider_connected"
            );
            if provider_connected {
                emit_subagent_lifecycle_event(
                    lifecycle_capture.as_ref(),
                    SubagentLifecycleEventKind::Connected,
                    serde_json::json!({ "providerConnected": true }),
                )
                .await;
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
            let should_flush_before_event = match &event {
                AgentEvent::Thinking { .. } => !pending_output.is_empty(),
                AgentEvent::TextDelta { .. } => !pending_thinking.is_empty(),
                _ => !pending_thinking.is_empty() || !pending_output.is_empty(),
            };
            if should_flush_before_event {
                flush_subagent_deltas(
                    lifecycle_capture.as_ref(),
                    &mut pending_thinking,
                    &mut pending_output,
                )
                .await;
                last_delta_flush = Instant::now();
            }
            match event {
                AgentEvent::ToolCallStart {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    let capture_detail = serde_json::json!({
                        "phase": "start",
                        "callId": &call_id,
                        "toolName": &tool_name,
                        "arguments": &arguments,
                    });
                    capture.tool_events.push(capture_detail);
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::ToolStarted,
                        serde_json::json!({
                            "phase": "start",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": arguments.len(),
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallResult {
                    call_id,
                    tool_name,
                    content,
                    is_error,
                    artifacts,
                } => {
                    let capture_detail = serde_json::json!({
                        "phase": "result",
                        "callId": &call_id,
                        "toolName": &tool_name,
                        "content": &content,
                        "isError": is_error,
                        "artifacts": &artifacts,
                    });
                    capture.tool_events.push(capture_detail);
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "result",
                            "callId": call_id,
                            "toolName": tool_name,
                            "contentBytes": content.len(),
                            "isError": is_error,
                            "hasArtifacts": artifacts.is_some(),
                        }),
                    )
                    .await;
                }
                AgentEvent::Thinking { content } => {
                    if !content.trim().is_empty() {
                        pending_thinking.push_str(&content);
                        capture.thinking.push(content);
                    }
                }
                AgentEvent::Status { content, tone } => {
                    if !content.trim().is_empty() {
                        let detail = serde_json::json!({
                            "phase": "status",
                            "content": content,
                            "tone": tone,
                        });
                        capture.tool_events.push(detail.clone());
                        emit_subagent_lifecycle_event(
                            lifecycle_capture.as_ref(),
                            SubagentLifecycleEventKind::Progress,
                            detail,
                        )
                        .await;
                    }
                }
                AgentEvent::ConnectionState { state } => {
                    let detail = serde_json::json!({
                        "phase": "connection",
                        "state": state,
                    });
                    capture.tool_events.push(detail.clone());
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        detail,
                    )
                    .await;
                }
                AgentEvent::Steering { content } => {
                    if !content.trim().is_empty() {
                        capture.tool_events.push(serde_json::json!({
                            "phase": "steering",
                            "content": content,
                        }));
                        emit_subagent_lifecycle_event(
                            lifecycle_capture.as_ref(),
                            SubagentLifecycleEventKind::InputApplied,
                            serde_json::json!({
                                "bytes": content.len(),
                                "state": "applied_at_model_boundary",
                            }),
                        )
                        .await;
                    }
                }
                AgentEvent::UsageUpdate { usage_total, .. } => {
                    capture.usage_total = usage_total;
                    if !worker_token_limit_exceeded
                        && worker_actual_token_limit
                            .is_some_and(|limit| capture.usage_total.total_tokens > limit)
                    {
                        worker_token_limit_exceeded = true;
                        capture_cancel_token.cancel();
                        let _ = fatal_error_tx.send(format!(
                            "worker actual token limit exceeded: {} > {}",
                            capture.usage_total.total_tokens,
                            worker_actual_token_limit.unwrap_or_default(),
                        ));
                    }
                }
                AgentEvent::Done {
                    usage_total,
                    finish_reason,
                    ..
                } => {
                    flush_subagent_deltas(
                        lifecycle_capture.as_ref(),
                        &mut pending_thinking,
                        &mut pending_output,
                    )
                    .await;
                    capture.usage_total = usage_total;
                    if !worker_token_limit_exceeded
                        && worker_actual_token_limit
                            .is_some_and(|limit| capture.usage_total.total_tokens > limit)
                    {
                        worker_token_limit_exceeded = true;
                        capture_cancel_token.cancel();
                        let _ = fatal_error_tx.send(format!(
                            "worker actual token limit exceeded: {} > {}",
                            capture.usage_total.total_tokens,
                            worker_actual_token_limit.unwrap_or_default(),
                        ));
                    }
                    capture.finish_reason = finish_reason;
                }
                AgentEvent::Error { message } => {
                    flush_subagent_deltas(
                        lifecycle_capture.as_ref(),
                        &mut pending_thinking,
                        &mut pending_output,
                    )
                    .await;
                    capture.error_message = Some(message.clone());
                    capture.tool_events.push(serde_json::json!({
                        "phase": "error",
                        "message": &message,
                    }));
                    let _ = fatal_error_tx.send(message);
                    capture_cancel_token.cancel();
                    break;
                }
                AgentEvent::TextDelta { delta } => {
                    pending_output.push_str(&delta);
                }
                AgentEvent::ToolRunStarted { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::ToolStarted,
                        serde_json::json!({ "phase": "runStarted", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolRunUpdated { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "phase": "runUpdated", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolRunCompleted { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "phase": "runCompleted", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolCallPreparing {
                    call_id,
                    tool_name,
                    args_bytes,
                    index,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolPreparing",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": args_bytes,
                            "index": index,
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallArgsDelta {
                    call_id,
                    tool_name,
                    arguments_delta,
                    index,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolArgsDelta",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": arguments_delta.len(),
                            "index": index,
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallProgress {
                    call_id,
                    tool_name,
                    note,
                    activity,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolProgress",
                            "callId": call_id,
                            "toolName": tool_name,
                            "note": note,
                            "activity": activity,
                        }),
                    )
                    .await;
                }
                AgentEvent::ControllerStatus {
                    code,
                    content,
                    tone,
                } if code != "provider_connected" => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "controller",
                            "code": code,
                            "content": content,
                            "tone": tone,
                        }),
                    )
                    .await;
                }
                AgentEvent::StreamBlockDelta { .. }
                | AgentEvent::StreamReset { .. }
                | AgentEvent::AutoCompacted { .. }
                | AgentEvent::ApprovalRequested { .. }
                | AgentEvent::ApprovalResolved { .. }
                | AgentEvent::ControllerStatus { .. }
                | AgentEvent::PlanUpdated { .. } => {}
            }
            if last_delta_flush.elapsed() >= Duration::from_millis(100)
                && (!pending_thinking.is_empty() || !pending_output.is_empty())
            {
                flush_subagent_deltas(
                    lifecycle_capture.as_ref(),
                    &mut pending_thinking,
                    &mut pending_output,
                )
                .await;
                last_delta_flush = Instant::now();
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

    let mut capture = match tokio::time::timeout(Duration::from_millis(500), &mut event_task).await
    {
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
    if final_result.is_err() && capture.usage_total.total_tokens == 0 {
        // Providers commonly omit usage when a long reasoning sample is
        // cancelled. Charge a conservative prompt/reservation floor so
        // repeated timeouts cannot bypass the aggregate soft budget.
        capture.usage_total.prompt_tokens = reserved_tokens.saturating_sub(initial_output_credit);
        capture.usage_total.total_tokens = reserved_tokens;
    }
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
        preflight_failure: None,
        preflight: Some(preflight),
        context_snapshot: Some(context_snapshot_artifact),
        effective_model_budgets: Some(effective_model_budgets),
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

fn isolated_subagent_runtime() -> Result<tokio::runtime::Runtime, CoreError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CoreError::Internal(format!(
                "Failed to build isolated subagent runtime: {error}"
            ))
        })
}

#[allow(clippy::too_many_arguments)]
async fn run_subagent_isolated(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    steering_rx: Option<mpsc::UnboundedReceiver<AgentSteeringMessage>>,
    lifecycle_events: Option<SubagentEventBridge>,
) -> Result<SubagentRunArtifact, CoreError> {
    let isolated_runtime = isolated_subagent_runtime()?;
    let (result_tx, result_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("nexa-subagent-worker".to_string())
        .spawn(move || {
            let result = isolated_runtime.block_on(run_subagent_once(
                runtime,
                db,
                inherited_source_scope,
                call_label,
                worker_id,
                args,
                batch_slots,
                steering_rx,
                lifecycle_events,
            ));
            let _ = result_tx.send(result);
        })
        .map_err(|error| {
            CoreError::Internal(format!("Failed to start isolated subagent thread: {error}"))
        })?;
    result_rx.await.map_err(|_| {
        CoreError::Agent("Isolated subagent thread exited without a result".to_string())
    })?
}

#[allow(clippy::too_many_arguments)]
async fn run_registered_subagent_isolated(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    call_label: String,
    worker_id: Option<String>,
    args: SpawnSubagentArgs,
    batch_slots: Option<Arc<tokio::sync::Semaphore>>,
    registration: crate::subagent_lifecycle::SubagentWorkerRegistration,
) -> Result<SubagentRunArtifact, CoreError> {
    let lifecycle = runtime.lifecycle.clone();
    let agent_id = registration.agent_id.clone();
    let cancellation = registration.cancel_token.clone();
    if let Err(error) = registration.events.start().await {
        let _ = lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Failed);
        return Err(error);
    }
    lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Running)?;

    let outcome = run_subagent_isolated(
        runtime.scoped_to_worker(registration.cancel_token),
        db,
        inherited_source_scope,
        call_label,
        worker_id,
        args,
        batch_slots,
        Some(registration.steering_rx),
        Some(registration.events),
    )
    .await;
    match &outcome {
        Ok(run) => {
            if let Err(error) = lifecycle
                .finish(
                    &agent_id,
                    SubagentLifecycleStatus::Completed,
                    serde_json::to_value(run).ok(),
                    None,
                )
                .await
            {
                warn!("Failed to complete lifecycle for batch subagent {agent_id}: {error}");
            }
        }
        Err(error) => {
            let status = if cancellation.is_cancelled() {
                SubagentLifecycleStatus::Cancelled
            } else {
                SubagentLifecycleStatus::Failed
            };
            if let Err(lifecycle_error) = lifecycle
                .finish(&agent_id, status, None, Some(error.to_string()))
                .await
            {
                warn!(
                    "Failed to record lifecycle failure for batch subagent {agent_id}: {lifecycle_error}"
                );
            }
        }
    }
    outcome
}

fn launch_detached_subagent(
    runtime: DelegationRuntime,
    db: Database,
    inherited_source_scope: Vec<String>,
    args: SpawnSubagentArgs,
    registration: crate::subagent_lifecycle::SubagentWorkerRegistration,
) -> Result<(), CoreError> {
    let isolated_runtime = isolated_subagent_runtime()?;
    let lifecycle = runtime.lifecycle.clone();
    let agent_id = registration.agent_id.clone();
    let cancellation = registration.cancel_token.clone();
    let worker_runtime = runtime.scoped_to_worker(registration.cancel_token);
    std::thread::Builder::new()
        .name("nexa-subagent-worker".to_string())
        .spawn(move || {
            isolated_runtime.block_on(async move {
                if let Err(error) = registration.events.start().await {
                    warn!("Failed to start lifecycle for subagent {agent_id}: {error}");
                    let _ = lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Failed);
                    return;
                }
                let _ = lifecycle.set_status(&agent_id, SubagentLifecycleStatus::Running);
                let outcome = run_subagent_once(
                    worker_runtime,
                    db,
                    inherited_source_scope,
                    agent_id.clone(),
                    Some(agent_id.clone()),
                    args,
                    None,
                    Some(registration.steering_rx),
                    Some(registration.events),
                )
                .await;
                match outcome {
                    Ok(run) => {
                        let result = serde_json::to_value(&run).ok();
                        if let Err(error) = lifecycle
                            .finish(
                                &agent_id,
                                SubagentLifecycleStatus::Completed,
                                result,
                                None,
                            )
                            .await
                        {
                            warn!("Failed to complete lifecycle for subagent {agent_id}: {error}");
                        }
                    }
                    Err(error) => {
                        let status = if cancellation.is_cancelled() {
                            SubagentLifecycleStatus::Cancelled
                        } else {
                            SubagentLifecycleStatus::Failed
                        };
                        if let Err(lifecycle_error) = lifecycle
                            .finish(
                                &agent_id,
                                status,
                                None,
                                Some(error.to_string()),
                            )
                            .await
                        {
                            warn!(
                                "Failed to record lifecycle failure for subagent {agent_id}: {lifecycle_error}"
                            );
                        }
                    }
                }
            });
        })
        .map_err(|error| {
            CoreError::Internal(format!("Failed to start detached subagent thread: {error}"))
        })?;
    Ok(())
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
        preflight_failure: subagent_preflight_failure_from_error(error),
        preflight: None,
        context_snapshot: None,
        effective_model_budgets: None,
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
            | "observe_subagent"
            | "wait_subagent"
            | "send_subagent_input"
            | "cancel_subagent"
            | "close_subagent"
    )
}

fn is_interactive_surface_tool(name: &str) -> bool {
    SUBAGENT_INTERACTIVE_SURFACE_TOOLS.contains(&name)
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

fn apply_nexus_worker_reasoning_policy(
    config: &mut AgentConfig,
    role_profile: Option<&SubagentRoleProfile>,
) {
    if !config.power_mode.is_nexus() {
        return;
    }
    let (Some(provider), Some(model)) = (config.provider_type, config.model.as_deref()) else {
        return;
    };
    let desired = if matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    ) {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::Low
    };
    let desired_budget: u32 = if matches!(
        role_profile.map(|profile| profile.id),
        Some("verifier" | "critic")
    ) {
        16_384
    } else {
        4_096
    };
    let Some(reasoning) = model_capabilities_from_catalog(provider, model)
        .and_then(|capabilities| capabilities.reasoning)
    else {
        // Unknown/local/custom endpoints must not inherit an unbounded parent
        // reasoning contract. Preserve an explicit off setting; otherwise
        // clamp only the control family already in use. Provider adapters still
        // decide whether that family is valid on the wire.
        if config.reasoning_enabled == Some(false)
            || config.reasoning_effort == Some(ReasoningEffort::None)
        {
            config.reasoning_enabled = Some(false);
            config.reasoning_effort = Some(ReasoningEffort::None);
            config.thinking_budget = None;
        } else if let Some(current_budget) = config.thinking_budget {
            let bounded = current_budget.min(desired_budget);
            config.reasoning_enabled = Some(bounded > 0);
            config.reasoning_effort = None;
            config.thinking_budget = Some(bounded);
        } else if config.reasoning_effort.is_some() || config.reasoning_enabled == Some(true) {
            config.reasoning_enabled = Some(true);
            config.reasoning_effort = Some(desired);
            config.thinking_budget = None;
        }
        return;
    };
    let effort_rank = |effort: &ReasoningEffort| match effort {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal => 1,
        ReasoningEffort::Low => 2,
        ReasoningEffort::Medium => 3,
        ReasoningEffort::High => 4,
        ReasoningEffort::XHigh => 5,
        ReasoningEffort::Max => 6,
    };
    let supported = reasoning
        .effort_levels
        .iter()
        .filter_map(|level| ReasoningEffort::from_wire(level))
        .filter(|effort| *effort != ReasoningEffort::None)
        .collect::<Vec<_>>();
    let selected = supported
        .iter()
        .filter(|effort| effort_rank(effort) >= effort_rank(&desired))
        .min_by_key(|effort| effort_rank(effort))
        .cloned()
        .or_else(|| {
            supported
                .iter()
                .max_by_key(|effort| effort_rank(effort))
                .cloned()
        });
    if let Some(selected) = selected {
        config.reasoning_enabled = Some(true);
        config.reasoning_effort = Some(selected);
        config.thinking_budget = None;
    } else if let Some(budget) = reasoning.thinking_budget.filter(|budget| budget.enabled) {
        let bounded = desired_budget
            .max(budget.min_tokens.unwrap_or_default())
            .min(budget.max_tokens.unwrap_or(desired_budget));
        config.reasoning_enabled = Some(bounded > 0 || reasoning.mode.as_deref() == Some("always"));
        config.reasoning_effort = None;
        config.thinking_budget = Some(bounded);
    }
}

fn apply_judge_recovery_controls(request: &mut CompletionRequest) {
    let Some(provider) = request.provider_type else {
        request.reasoning_enabled = Some(false);
        request.reasoning_effort = None;
        request.thinking_budget = None;
        return;
    };
    let reasoning = model_capabilities_from_catalog(provider, &request.model)
        .and_then(|capabilities| capabilities.reasoning);
    if reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.mode.as_deref())
        != Some("always")
    {
        request.reasoning_enabled = Some(false);
        request.reasoning_effort = (provider == ProviderType::OpenRouter
            && reasoning.as_ref().is_some_and(|reasoning| {
                reasoning.effort_levels.iter().any(|level| level == "none")
            }))
        .then_some(ReasoningEffort::None);
        request.thinking_budget = None;
        return;
    }
    request.reasoning_enabled = Some(true);
    request.reasoning_effort = reasoning
        .into_iter()
        .flat_map(|reasoning| reasoning.effort_levels)
        .filter_map(|level| ReasoningEffort::from_wire(&level))
        .find(|effort| *effort != ReasoningEffort::None);
    request.thinking_budget = None;
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
    inherited_context_tokens: u32,
    inherited_skill_tokens: u32,
    initial_output_credit: u32,
) -> u32 {
    let model = config.model.as_deref().unwrap_or("gpt-4o-mini");
    estimate_tokens_for_model(model, &config.system_prompt)
        .saturating_add(estimate_tokens_for_model(model, request_text))
        .saturating_add(estimate_tool_tokens_for_model(model, &tools.definitions()))
        .saturating_add(inherited_context_tokens)
        .saturating_add(inherited_skill_tokens)
        .saturating_add(initial_output_credit)
}

fn resolve_delegated_max_output(config: &AgentConfig, catalog_limit: Option<u64>) -> u32 {
    let fallback_limit = u64::from(CONSERVATIVE_SUBAGENT_MAX_TOKENS);
    let effective_limit = catalog_limit
        .unwrap_or(fallback_limit)
        .min(u64::from(u32::MAX)) as u32;
    let requested_limit = config
        .max_tokens
        .unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS)
        .max(256);
    requested_limit.min(effective_limit.max(1))
}

fn apply_delegated_model_limits(
    config: &mut AgentConfig,
    input_context_policy: DelegationLimitPolicy,
    max_output_policy: DelegationLimitPolicy,
    resolved_context: ResolvedContextWindow,
    catalog_output_limit: Option<u64>,
    independent_v2_limits: bool,
) -> ContextWindowAuthority {
    let existing_context_window = config.context_window;
    let context_authority = match input_context_policy {
        DelegationLimitPolicy::Explicit(_) => ContextWindowAuthority::UserOverride,
        DelegationLimitPolicy::Auto
            if !independent_v2_limits && existing_context_window.is_some() =>
        {
            ContextWindowAuthority::UserOverride
        }
        DelegationLimitPolicy::Auto => resolved_context.authority,
    };
    config.context_window = match input_context_policy {
        // An explicit delegated window is authoritative. In particular, never
        // clamp it to an endpoint-agnostic model-name fallback.
        DelegationLimitPolicy::Explicit(limit) => u32::try_from(limit).ok(),
        DelegationLimitPolicy::Auto if independent_v2_limits => resolved_context.capacity_tokens,
        DelegationLimitPolicy::Auto => config.context_window.or(resolved_context.capacity_tokens),
    };

    match max_output_policy {
        DelegationLimitPolicy::Explicit(limit) => {
            config.max_tokens = u32::try_from(limit).ok();
        }
        DelegationLimitPolicy::Auto if independent_v2_limits => {
            config.max_tokens = Some(
                catalog_output_limit
                    .map(|limit| {
                        limit
                            .min(u64::from(CONSERVATIVE_SUBAGENT_MAX_TOKENS))
                            .min(u64::from(u32::MAX)) as u32
                    })
                    .unwrap_or(DEFAULT_SUBAGENT_MAX_TOKENS),
            );
        }
        DelegationLimitPolicy::Auto => {}
    }

    let mut resolved_output = resolve_delegated_max_output(config, catalog_output_limit);
    if let Some(context_window) = config.context_window {
        let prompt_reserve = (context_window / 10).max(1_024).min(context_window);
        resolved_output =
            resolved_output.min(context_window.saturating_sub(prompt_reserve).max(256));
    }
    config.max_tokens = Some(resolved_output);
    context_authority
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
        .without_names(SUBAGENT_INTERACTIVE_SURFACE_TOOLS)
        .without_names(&[
            "spawn_subagent",
            "spawn_subagent_batch",
            "judge_subagent_results",
            "observe_subagent_batch",
            "observe_subagent",
            "wait_subagent",
            "send_subagent_input",
            "cancel_subagent",
            "close_subagent",
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
            child_runtime.clone(),
        )));
    }
    for lifecycle_tool in SubagentLifecycleTool::all(child_runtime.clone()) {
        if allowed_tool_names
            .iter()
            .any(|name| name == lifecycle_tool.name())
        {
            registry.register(Box::new(lifecycle_tool));
        }
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
            conversation_id,
            turn_id,
            activity_runtime,
            event_tx,
            ..
        } = context;
        let args: SpawnSubagentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid spawn_subagent arguments: {e}"))
        })?;
        let args = normalize_spawn_args(args)?;
        let agent_id = format!("subagent-{}", uuid::Uuid::new_v4());
        let registration = self.runtime.lifecycle.register(RegisterSubagentRequest {
            agent_id: agent_id.clone(),
            parent_call_id: call_id.to_string(),
            task: args.task.clone(),
            role_id: args.role_id.clone(),
            role: args.role.clone(),
            conversation_id: conversation_id.map(str::to_string),
            turn_id: turn_id.map(str::to_string),
            task_run_id: self.runtime.parent_task_run_id.clone(),
            cancel_token: self.runtime.cancel_token.child_token(),
            activity_runtime: activity_runtime.cloned().unwrap_or_default(),
            event_tx: event_tx.map(|sender| sender.downgrade()),
        })?;
        if let Err(error) = launch_detached_subagent(
            self.runtime.clone(),
            db.clone(),
            source_scope.to_vec(),
            args.clone(),
            registration,
        ) {
            let _ = self
                .runtime
                .lifecycle
                .set_status(&agent_id, SubagentLifecycleStatus::Failed);
            let _ = self.runtime.lifecycle.close(&agent_id);
            return Err(error);
        }

        let content = format!(
            "Subagent {agent_id} spawned and is running. Use observe_subagent for incremental events, wait_subagent before consuming its final result, send_subagent_input to steer it, or cancel_subagent to stop it."
        );

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "subagent_result",
                "id": agent_id,
                "sessionId": args.task_id,
                "status": "running",
                "task": args.task,
                "roleId": args.role_id,
                "role": args.role,
                "expectedOutput": args.expected_output,
                "acceptanceCriteria": args.acceptance_criteria,
                "parallelGroup": args.parallel_group,
                "result": "",
                "isError": false,
                "lifecycleTools": {
                    "observe": "observe_subagent",
                    "wait": "wait_subagent",
                    "sendInput": "send_subagent_input",
                    "cancel": "cancel_subagent",
                    "close": "close_subagent",
                },
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
            conversation_id,
            turn_id,
            activity_runtime,
            event_tx,
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
        let parent_conversation_id = conversation_id.map(str::to_string);
        let parent_turn_id = turn_id.map(str::to_string);
        let activity_runtime = activity_runtime.cloned().unwrap_or_default();
        let event_tx = event_tx.map(|sender| sender.downgrade());
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
        let mut lifecycle_workers = Vec::with_capacity(worker_count);
        let mut pending = FuturesUnordered::new();
        for (index, (worker_id, task_args)) in normalized_tasks.into_iter().enumerate() {
            let db = db.clone();
            let inherited_source_scope = inherited_source_scope.clone();
            let batch_parallel_group = batch_parallel_group.clone();
            let worker_cancel = runtime.cancel_token.child_token();
            let batch_runtime = runtime.clone();
            let batch_slots = Arc::clone(&batch_slots);
            worker_cancel_tokens.push(worker_cancel.clone());
            runtime.add_batch_cancel_token(&batch_id, worker_cancel.clone());
            let worker_batch_id = batch_id.clone();
            let detached_label = worker_id
                .clone()
                .unwrap_or_else(|| format!("{}-{}", call_id, index + 1));
            let detached_fallback = task_args.clone();
            let detached_parallel_group = batch_parallel_group.clone();
            let batch_call_id = call_id.to_string();
            let lifecycle_agent_id = format!("subagent-{}", uuid::Uuid::new_v4());
            let registration = runtime.lifecycle.register(RegisterSubagentRequest {
                agent_id: lifecycle_agent_id.clone(),
                parent_call_id: call_id.to_string(),
                task: task_args.task.clone(),
                role_id: task_args.role_id.clone(),
                role: task_args.role.clone(),
                conversation_id: parent_conversation_id.clone(),
                turn_id: parent_turn_id.clone(),
                task_run_id: self.runtime.parent_task_run_id.clone(),
                cancel_token: worker_cancel,
                activity_runtime: activity_runtime.clone(),
                event_tx: event_tx.clone(),
            })?;
            lifecycle_workers.push(serde_json::json!({
                "agentId": &lifecycle_agent_id,
                "workerId": &worker_id,
                "task": &task_args.task,
                "roleId": &task_args.role_id,
                "role": &task_args.role,
            }));
            let lifecycle_for_join = runtime.lifecycle.clone();
            let lifecycle_agent_id_for_join = lifecycle_agent_id.clone();
            let worker_task = tokio::spawn(async move {
                let label = worker_id
                    .clone()
                    .unwrap_or_else(|| format!("{}-{}", batch_call_id, index + 1));
                let fallback = task_args.clone();
                let run = match run_registered_subagent_isolated(
                    batch_runtime.clone(),
                    db,
                    inherited_source_scope,
                    label.clone(),
                    worker_id,
                    task_args,
                    Some(batch_slots),
                    registration,
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
                match worker_task.await {
                    Ok(result) => result,
                    Err(join_error) => {
                        let error = CoreError::Agent(format!(
                            "Delegated worker task terminated unexpectedly: {join_error}"
                        ));
                        let _ = lifecycle_for_join
                            .finish(
                                &lifecycle_agent_id_for_join,
                                SubagentLifecycleStatus::Failed,
                                None,
                                Some(error.to_string()),
                            )
                            .await;
                        (
                            index,
                            failed_subagent_run_artifact(
                                detached_label,
                                detached_fallback,
                                detached_parallel_group,
                                &error,
                            ),
                        )
                    }
                }
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
                "lifecycleWorkers": lifecycle_workers,
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
                    "maximum": 2500,
                    "description": "One steering-friendly wait quantum for another supplemental result"
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

        let wait_ms = args.wait_ms.unwrap_or(0).min(2_500);
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
impl Tool for SubagentLifecycleTool {
    fn name(&self) -> &str {
        match self.action {
            SubagentLifecycleAction::Observe => "observe_subagent",
            SubagentLifecycleAction::Wait => "wait_subagent",
            SubagentLifecycleAction::SendInput => "send_subagent_input",
            SubagentLifecycleAction::Cancel => "cancel_subagent",
            SubagentLifecycleAction::Close => "close_subagent",
        }
    }

    fn description(&self) -> &str {
        match self.action {
            SubagentLifecycleAction::Observe => {
                "Read a spawned subagent's current state and incremental lifecycle events without blocking the parent turn."
            }
            SubagentLifecycleAction::Wait => {
                "Wait for a spawned subagent to settle, up to a bounded timeout, and return its authoritative result snapshot."
            }
            SubagentLifecycleAction::SendInput => {
                "Steer an active spawned subagent with additional user-authored input."
            }
            SubagentLifecycleAction::Cancel => {
                "Request cooperative cancellation of an active spawned subagent."
            }
            SubagentLifecycleAction::Close => {
                "Release a terminal subagent handle after its result has been consumed."
            }
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::json!({
            "agentId": {
                "type": "string",
                "description": "Stable agent id returned by spawn_subagent"
            }
        });
        let required = if matches!(self.action, SubagentLifecycleAction::SendInput) {
            properties["input"] = serde_json::json!({
                "type": "string",
                "description": "Additional instruction to inject at the next safe model boundary"
            });
            vec!["agentId", "input"]
        } else {
            if matches!(self.action, SubagentLifecycleAction::Observe) {
                properties["afterSeq"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "description": "Return only lifecycle events after this cursor"
                });
                properties["waitMs"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "description": "Optional long-poll duration for new events"
                });
            } else if matches!(self.action, SubagentLifecycleAction::Wait) {
                properties["waitMs"] = serde_json::json!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "default": 2500,
                    "description": "One steering-friendly wait quantum for terminal state"
                });
            }
            vec!["agentId"]
        };
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }

    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let args: SubagentLifecycleArgs =
            serde_json::from_str(context.arguments).map_err(|error| {
                CoreError::InvalidInput(format!("Invalid {} arguments: {error}", self.name()))
            })?;
        let agent_id = args.agent_id.trim();
        if agent_id.is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "{} requires agentId",
                self.name()
            )));
        }
        self.runtime
            .lifecycle
            .ensure_conversation(agent_id, self.runtime.parent_conversation_id.as_deref())?;

        let (content, artifacts) = match self.action {
            SubagentLifecycleAction::Observe => {
                let observation = self
                    .runtime
                    .lifecycle
                    .observe(
                        agent_id,
                        args.after_seq.unwrap_or(0),
                        Duration::from_millis(args.wait_ms.unwrap_or(0).min(2_500)),
                    )
                    .await?;
                let content = format!(
                    "Subagent {agent_id} is {:?}; received {} lifecycle event(s), cursor {}.",
                    observation.worker.status,
                    observation.events.len(),
                    observation.cursor,
                );
                (
                    content,
                    serde_json::json!({
                        "kind": "subagent_observation",
                        "observation": observation,
                    }),
                )
            }
            SubagentLifecycleAction::Wait => {
                let wait_result = self
                    .runtime
                    .lifecycle
                    .wait(
                        agent_id,
                        Duration::from_millis(args.wait_ms.unwrap_or(2_500).min(2_500)),
                    )
                    .await?;
                let result_text = wait_result
                    .worker
                    .result
                    .as_ref()
                    .and_then(|value| value.get("result"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let mut content = format!(
                    "Subagent {agent_id} is {:?}{}.",
                    wait_result.worker.status,
                    if wait_result.timed_out {
                        " (wait timed out; worker remains active)"
                    } else {
                        ""
                    }
                );
                if !result_text.is_empty() {
                    content.push_str("\n\n");
                    content.push_str(result_text);
                }
                (
                    content,
                    serde_json::json!({
                        "kind": "subagent_wait_result",
                        "worker": wait_result.worker,
                        "timedOut": wait_result.timed_out,
                    }),
                )
            }
            SubagentLifecycleAction::SendInput => {
                let input = args
                    .input
                    .as_deref()
                    .map(str::trim)
                    .filter(|input| !input.is_empty())
                    .ok_or_else(|| {
                        CoreError::InvalidInput("send_subagent_input requires input".into())
                    })?;
                let bridge = self
                    .runtime
                    .lifecycle
                    .send_input(agent_id, input.to_string())?;
                bridge
                    .emit(
                        SubagentLifecycleEventKind::InputQueued,
                        serde_json::json!({
                            "bytes": input.len(),
                            "state": "queued",
                            "acknowledgement": "channel_enqueue_only",
                        }),
                    )
                    .await?;
                (
                    format!("Input queued for subagent {agent_id}; wait for an inputApplied lifecycle event to confirm it reached a model boundary."),
                    serde_json::json!({
                        "kind": "subagent_input_queued",
                        "agentId": agent_id,
                        "state": "queued",
                    }),
                )
            }
            SubagentLifecycleAction::Cancel => {
                let bridge = self.runtime.lifecycle.cancel(agent_id)?;
                bridge
                    .emit(
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "status": "cancelling" }),
                    )
                    .await?;
                (
                    format!("Cancellation requested for subagent {agent_id}."),
                    serde_json::json!({
                        "kind": "subagent_cancellation",
                        "agentId": agent_id,
                        "status": "cancelling",
                    }),
                )
            }
            SubagentLifecycleAction::Close => {
                let snapshot = self.runtime.lifecycle.close(agent_id)?;
                (
                    format!("Closed terminal subagent handle {agent_id}."),
                    serde_json::json!({
                        "kind": "subagent_closed",
                        "worker": snapshot,
                    }),
                )
            }
        };

        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(artifacts),
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
        let mut judge_config = self.runtime.base_config.clone();
        judge_config.model = Some(model.clone());
        apply_nexus_worker_reasoning_policy(&mut judge_config, role_profile_by_id("verifier"));
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
        let mut request = CompletionRequest {
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
            thinking_budget: judge_config.thinking_budget,
            reasoning_enabled: judge_config.reasoning_enabled,
            reasoning_effort: judge_config.reasoning_effort.clone(),
            provider_type: judge_config.provider_type,
            routing_session_id: None,
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
        let judge_sample_deadline_ms = judge_limits
            .first_token_deadline_ms
            .min(judge_timeout_ms)
            .max(1_000);
        let judge_deadline = tokio::time::Instant::now() + Duration::from_millis(judge_timeout_ms);
        let judge_response = async {
            let first = tokio::time::timeout(
                Duration::from_millis(judge_sample_deadline_ms),
                provider.complete(&request),
            )
            .await;
            match first {
                Ok(result) => result.map(|response| (response, 0_u32)),
                Err(_) => {
                    let remaining =
                        judge_deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(CoreError::Agent(format!(
                            "Delegated adjudication timed out after {judge_timeout_ms}ms."
                        )));
                    }
                    apply_judge_recovery_controls(&mut request);
                    request.messages.push(Message::text(
                        Role::System,
                        "The previous adjudication sample exceeded its progress deadline. Do not continue private analysis. Return the requested compact judgement JSON now.",
                    ));
                    tokio::time::timeout(
                        remaining.min(Duration::from_millis(60_000)),
                        provider.complete(&request),
                    )
                    .await
                    .map_err(|_| {
                        CoreError::Agent(
                            "Delegated adjudication remained reasoning-only after one bounded recovery."
                                .to_string(),
                        )
                    })?
                    .map(|response| (response, reserved_tokens))
                }
            }
        };
        tokio::pin!(judge_response);
        let judge_failure_usage = Usage {
            prompt_tokens: reserved_tokens.saturating_sub(1_200),
            total_tokens: reserved_tokens,
            ..Usage::default()
        };
        let response = tokio::select! {
            _ = judge_cancel_token.cancelled() => {
                self.runtime
                    .budget
                    .finish_call(reserved_tokens, &judge_failure_usage, judge_cost_micros)
                    .await;
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
            result = &mut judge_response => match result {
                Ok((response, discarded_tokens)) => {
                    let mut accounted_usage = response.usage.clone();
                    accounted_usage.prompt_tokens = accounted_usage
                        .prompt_tokens
                        .saturating_add(discarded_tokens);
                    accounted_usage.total_tokens = accounted_usage
                        .total_tokens
                        .saturating_add(discarded_tokens);
                    self.runtime
                        .budget
                        .finish_call(reserved_tokens, &accounted_usage, judge_cost_micros)
                        .await;
                    response
                }
                Err(err) => {
                    self.runtime
                        .budget
                        .finish_call(reserved_tokens, &judge_failure_usage, judge_cost_micros)
                        .await;
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
            time_to_first_token_ms: None,
            upstream_provider_id: None,
            cache_outcome_reason: None,
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
            SubagentLifecycleRuntime::default(),
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
    fn test_explicit_request_cannot_widen_role_tool_policy() {
        let base_tools = vec!["web_search".to_string(), "desktop_automation".to_string()];
        let verifier = role_profile_by_id("verifier").unwrap();

        let tools = resolve_allowed_tools_for_role(
            &base_tools,
            Some(&["desktop_automation".to_string()]),
            Some(verifier),
        );

        assert!(tools.is_empty());
    }

    #[test]
    fn test_explicit_tool_scope_never_falls_back_to_parent_permissions() {
        let base_tools = vec!["read_file".to_string(), "web_search".to_string()];

        assert!(resolve_allowed_tools(&base_tools, Some(&[])).is_empty());
        assert!(
            resolve_allowed_tools(&base_tools, Some(&["desktop_automation".to_string()]))
                .is_empty()
        );
    }

    #[test]
    fn test_explicit_source_scope_never_falls_back_to_parent_scope() {
        let parent_scope = vec!["source-a".to_string()];

        assert!(resolve_source_scope(&parent_scope, Some(&[])).is_empty());
        assert!(resolve_source_scope(&parent_scope, Some(&["source-b".to_string()])).is_empty());
    }

    #[test]
    fn test_preflight_rejects_tools_outside_parent_capabilities() {
        let args = SpawnSubagentArgs {
            task: "Inspect the repository".into(),
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
            allowed_tools: Some(vec!["edit_file".into()]),
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        };
        let snapshot = DelegationContextSnapshot {
            id: "snapshot".into(),
            selected_message_ids: Arc::from(Vec::<String>::new()),
            messages: Arc::from(Vec::<Message>::new()),
            token_estimate: 0,
            context_limit: Some(128_000),
            handoff_token_budget: 64_000,
            dropped_invalid_messages: 0,
        };
        let error = validate_subagent_preflight(
            &args,
            "test-model",
            "openai",
            &["read_file".into()],
            &[],
            &[],
            &[],
            &snapshot,
        )
        .unwrap_err();

        let failure = subagent_preflight_failure_from_error(&error).unwrap();
        assert_eq!(failure.schema_version, 1);
        assert_eq!(failure.stage, SubagentPreflightStage::Policy);
        assert_eq!(failure.code, "tool_scope_widening");
        assert!(!failure.retryable);
        assert!(error.to_string().contains("edit_file"));
    }

    #[test]
    fn test_preflight_rejects_interactive_surface_tools_even_when_parent_has_them() {
        let args = SpawnSubagentArgs {
            task: "Click the visible button".into(),
            task_id: None,
            role_id: Some("desktop_operator".into()),
            role: None,
            model_policy: None,
            context: None,
            expected_output: None,
            max_iterations: None,
            timeout_secs: None,
            acceptance_criteria: None,
            evidence_chunk_ids: None,
            source_ids: None,
            allowed_tools: Some(vec!["computer_control".into(), "browser_session".into()]),
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        };
        let snapshot = DelegationContextSnapshot {
            id: "snapshot".into(),
            selected_message_ids: Arc::from(Vec::<String>::new()),
            messages: Arc::from(Vec::<Message>::new()),
            token_estimate: 0,
            context_limit: Some(128_000),
            handoff_token_budget: 64_000,
            dropped_invalid_messages: 0,
        };
        let error = validate_subagent_preflight(
            &args,
            "test-model",
            "openai",
            &["computer_control".into(), "browser_session".into()],
            &[],
            &[],
            &[],
            &snapshot,
        )
        .unwrap_err();

        let failure = subagent_preflight_failure_from_error(&error).unwrap();
        assert_eq!(failure.stage, SubagentPreflightStage::Policy);
        assert_eq!(failure.code, "interactive_tool_requires_parent_proxy");
        assert!(error.to_string().contains("parent agent"));
    }

    #[test]
    fn test_preflight_classifies_invalid_inherited_history() {
        let args = SpawnSubagentArgs {
            task: "Inspect the repository".into(),
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
        };
        let invalid_assistant = Message {
            role: Role::Assistant,
            parts: Vec::new(),
            name: None,
            tool_calls: None,
            reasoning_content: Some("private reasoning".into()),
            prompt_cache_hint: None,
        };
        let snapshot = DelegationContextSnapshot {
            id: "snapshot".into(),
            selected_message_ids: Arc::from(vec!["message-1".to_string()]),
            messages: Arc::from(vec![invalid_assistant]),
            token_estimate: 1,
            context_limit: Some(128_000),
            handoff_token_budget: 64_000,
            dropped_invalid_messages: 0,
        };

        let error = validate_subagent_preflight(
            &args,
            "test-model",
            "openai",
            &[],
            &[],
            &[],
            &[],
            &snapshot,
        )
        .unwrap_err();
        let failure = subagent_preflight_failure_from_error(&error).unwrap();

        assert_eq!(failure.stage, SubagentPreflightStage::History);
        assert_eq!(failure.code, "inherited_history_invalid");
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
    fn delegated_output_helper_honors_explicit_value_and_catalog_ceiling() {
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

    #[tokio::test]
    async fn nexus_control_lanes_remain_admissible_after_exploration_soft_limit() {
        let config = AgentConfig {
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                total_actual_tokens_soft_limit: Some(256),
                max_parallel: Some(3),
                max_calls_per_turn: Some(4),
                ..Default::default()
            }),
            subagent_verification_reserve_percent: Some(25),
            ..Default::default()
        };
        let budget = SubagentBudgetController::new(&config);
        let cancel = CancellationToken::new();
        let explorer = budget
            .begin_call("explorer", 100, false, &cancel)
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
        drop(explorer);

        let verifier = budget
            .begin_call("verifier", 32, true, &cancel)
            .await
            .expect("verification lane survives exploration token exhaustion");
        let judge = budget
            .begin_judge_call("judge", 32, &cancel)
            .await
            .expect("judge lane survives exploration token exhaustion");
        drop(verifier);
        drop(judge);
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
            ResolvedContextWindow {
                capacity_tokens: Some(1_000_000),
                authority: ContextWindowAuthority::Catalog,
            },
            Some(65_536),
            true,
        );

        assert_eq!(config.context_window, Some(1_000_000));
        assert_eq!(config.max_tokens, Some(65_536));
    }

    #[test]
    fn nexus_long_reasoning_workers_use_interactive_effort_instead_of_parent_max() {
        let mut qwen = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(ProviderType::Qwen),
            model: Some("qwen3.8-max".to_string()),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(&mut qwen, role_profile_by_id("researcher"));
        assert_eq!(qwen.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(qwen.thinking_budget, None);

        apply_nexus_worker_reasoning_policy(&mut qwen, role_profile_by_id("verifier"));
        assert_eq!(qwen.reasoning_effort, Some(ReasoningEffort::Medium));

        let mut direct_kimi = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(ProviderType::Moonshot),
            model: Some("kimi-k3".to_string()),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(&mut direct_kimi, role_profile_by_id("verifier"));
        assert_eq!(direct_kimi.reasoning_effort, Some(ReasoningEffort::High));

        let mut routed_kimi = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(ProviderType::AlibabaModelStudio),
            model: Some("kimi/kimi-k3".to_string()),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(&mut routed_kimi, role_profile_by_id("researcher"));
        assert_eq!(routed_kimi.reasoning_effort, Some(ReasoningEffort::Max));

        let mut qwen_request = CompletionRequest {
            model: "qwen3.8-max".to_string(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: Some(4_000),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Medium),
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };
        apply_judge_recovery_controls(&mut qwen_request);
        assert_eq!(qwen_request.reasoning_enabled, Some(false));

        let mut kimi_request = CompletionRequest {
            model: "kimi-k3".to_string(),
            provider_type: Some(ProviderType::Moonshot),
            ..qwen_request
        };
        apply_judge_recovery_controls(&mut kimi_request);
        assert_eq!(kimi_request.reasoning_enabled, Some(true));
        assert_eq!(kimi_request.reasoning_effort, Some(ReasoningEffort::Low));
    }

    #[test]
    fn nexus_reasoning_policy_is_catalog_driven_across_provider_families() {
        for (provider, model) in [
            (ProviderType::OpenAi, "gpt-5.6"),
            (ProviderType::Anthropic, "claude-fable-5"),
            (ProviderType::Google, "gemini-3.7-flash"),
            (ProviderType::DeepSeek, "deepseek-v4-pro"),
            (ProviderType::Moonshot, "kimi-k3"),
            (ProviderType::Qwen, "qwen3.8-max"),
            (ProviderType::AlibabaModelStudio, "qwen3.8-max"),
            (ProviderType::Zhipu, "glm-5.3"),
            (ProviderType::OpenRouter, "moonshotai/kimi-k3"),
        ] {
            let reasoning = model_capabilities_from_catalog(provider, model)
                .and_then(|capabilities| capabilities.reasoning)
                .unwrap_or_else(|| panic!("missing reasoning profile for {provider:?}:{model}"));
            let mut config = AgentConfig {
                power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
                provider_type: Some(provider),
                model: Some(model.to_string()),
                reasoning_enabled: Some(true),
                reasoning_effort: Some(ReasoningEffort::Max),
                thinking_budget: Some(262_144),
                ..Default::default()
            };
            apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("researcher"));
            if reasoning.effort_levels.is_empty() {
                assert!(config.thinking_budget.is_some_and(|budget| budget <= 4_096));
            } else if reasoning.effort_levels.iter().any(|level| level == "low") {
                assert_eq!(config.reasoning_effort, Some(ReasoningEffort::Low));
                assert_eq!(config.thinking_budget, None);
            } else {
                assert!(config.reasoning_effort.is_some());
            }
        }
    }

    #[test]
    fn nexus_unknown_model_reasoning_is_bounded_for_every_provider_type() {
        for provider in [
            ProviderType::OpenAi,
            ProviderType::OpenRouter,
            ProviderType::Anthropic,
            ProviderType::Google,
            ProviderType::DeepSeek,
            ProviderType::Ollama,
            ProviderType::LmStudio,
            ProviderType::AzureOpenAi,
            ProviderType::Zhipu,
            ProviderType::Moonshot,
            ProviderType::Qwen,
            ProviderType::AlibabaModelStudio,
            ProviderType::SiliconFlow,
            ProviderType::Doubao,
            ProviderType::Yi,
            ProviderType::Baichuan,
            ProviderType::Custom,
        ] {
            let mut config = AgentConfig {
                power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
                provider_type: Some(provider),
                model: Some("private-unknown-reasoner".to_string()),
                reasoning_enabled: Some(true),
                reasoning_effort: Some(ReasoningEffort::Max),
                ..Default::default()
            };
            apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("researcher"));
            assert_eq!(
                config.reasoning_effort,
                Some(ReasoningEffort::Low),
                "unknown model inherited parent max for {provider:?}"
            );
            apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("verifier"));
            assert_eq!(config.reasoning_effort, Some(ReasoningEffort::Medium));
        }

        let mut budget_controlled = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(ProviderType::Custom),
            model: Some("private-budget-reasoner".to_string()),
            reasoning_enabled: Some(true),
            thinking_budget: Some(262_144),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(
            &mut budget_controlled,
            role_profile_by_id("researcher"),
        );
        assert_eq!(budget_controlled.thinking_budget, Some(4_096));
        assert_eq!(budget_controlled.reasoning_effort, None);
    }

    #[test]
    fn independent_auto_output_uses_safe_8k_fallback_without_catalog_data() {
        let mut config = AgentConfig {
            max_tokens: Some(8_192),
            ..Default::default()
        };

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Auto,
            ResolvedContextWindow {
                capacity_tokens: None,
                authority: ContextWindowAuthority::ProviderManaged,
            },
            None,
            true,
        );

        assert_eq!(config.max_tokens, Some(DEFAULT_SUBAGENT_MAX_TOKENS));
    }

    #[test]
    fn independent_auto_output_uses_catalog_as_ceiling_not_kimi_allocation() {
        let mut config = AgentConfig {
            context_window: Some(128_000),
            max_tokens: Some(8_192),
            ..Default::default()
        };

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Auto,
            ResolvedContextWindow {
                capacity_tokens: Some(1_048_576),
                authority: ContextWindowAuthority::Catalog,
            },
            Some(1_048_576),
            true,
        );

        assert_eq!(config.context_window, Some(1_048_576));
        assert_eq!(config.max_tokens, Some(CONSERVATIVE_SUBAGENT_MAX_TOKENS));
        assert!(config.max_tokens.unwrap() < config.context_window.unwrap());
        assert_eq!(model_context_window("moonshotai/kimi-k3:free"), 1_048_576);
        assert_eq!(model_context_window("qwen3.8-max-latest"), 1_000_000);
    }

    #[test]
    fn delegated_fallback_contract_covers_local_compatible_and_unknown_providers() {
        for (provider, model, expected_context) in [
            (ProviderType::Ollama, "qwen3.8-max", Some(1_000_000)),
            (ProviderType::LmStudio, "openai/gpt-5.6", Some(1_050_000)),
            (
                ProviderType::SiliconFlow,
                "deepseek/deepseek-v4-pro",
                Some(1_000_000),
            ),
            (
                ProviderType::Doubao,
                "doubao-seed-1-6-thinking",
                Some(256_000),
            ),
            (ProviderType::Yi, "yi-large", Some(128_000)),
            (ProviderType::Baichuan, "baichuan-m3", Some(32_000)),
            (ProviderType::Custom, "unknown-private-model", None),
        ] {
            let mut config = AgentConfig {
                provider_type: Some(provider),
                model: Some(model.to_string()),
                context_window: None,
                max_tokens: None,
                ..Default::default()
            };
            apply_delegated_model_limits(
                &mut config,
                DelegationLimitPolicy::Auto,
                DelegationLimitPolicy::Auto,
                resolve_model_context_window(model),
                None,
                true,
            );
            assert_eq!(
                config.context_window, expected_context,
                "fallback context mismatch for {provider:?}:{model}"
            );
            assert_eq!(config.max_tokens, Some(DEFAULT_SUBAGENT_MAX_TOKENS));
        }
    }

    #[test]
    fn explicit_worker_context_is_not_clamped_by_an_inferred_capacity() {
        let mut config = AgentConfig {
            context_window: Some(32_000),
            ..Default::default()
        };
        let authority = apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Explicit(750_000),
            DelegationLimitPolicy::Auto,
            ResolvedContextWindow {
                capacity_tokens: Some(32_000),
                authority: ContextWindowAuthority::ModelProfile,
            },
            None,
            true,
        );

        assert_eq!(config.context_window, Some(750_000));
        assert_eq!(authority, ContextWindowAuthority::UserOverride);
    }

    #[test]
    fn explicit_worker_output_cap_below_legacy_minimum_is_preserved() {
        let mut config = AgentConfig {
            max_tokens: Some(8_192),
            ..Default::default()
        };

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Explicit(512),
            ResolvedContextWindow {
                capacity_tokens: None,
                authority: ContextWindowAuthority::ProviderManaged,
            },
            Some(65_536),
            true,
        );

        assert_eq!(config.max_tokens, Some(512));

        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Explicit(512),
            ResolvedContextWindow {
                capacity_tokens: None,
                authority: ContextWindowAuthority::ProviderManaged,
            },
            Some(400),
            true,
        );

        assert_eq!(config.max_tokens, Some(400));
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

        let ordinary = SubagentBudgetController::new(&AgentConfig {
            model: Some("ordinary-model".to_string()),
            provider_type: Some(ProviderType::OpenAi),
            ..Default::default()
        })
        .limits()
        .await;
        assert_eq!(ordinary.connect_deadline_ms, 15_000);
        assert_eq!(ordinary.first_token_deadline_ms, 45_000);
        assert_eq!(ordinary.run_deadline_ms, 180_000);

        let qwen = SubagentBudgetController::new(&AgentConfig {
            model: Some("qwen3.8-max".to_string()),
            provider_type: Some(ProviderType::Qwen),
            ..Default::default()
        })
        .limits()
        .await;
        assert_eq!(qwen.connect_deadline_ms, 90_000);
        assert_eq!(qwen.first_token_deadline_ms, 150_000);
        assert_eq!(qwen.run_deadline_ms, 360_000);

        for (provider, model) in [
            (ProviderType::OpenAi, "gpt-5.6"),
            (ProviderType::Anthropic, "claude-fable-5"),
            (ProviderType::Google, "gemini-3.7-flash"),
            (ProviderType::DeepSeek, "deepseek-v4-pro"),
            (ProviderType::Zhipu, "glm-5.3"),
        ] {
            let profiled = SubagentBudgetController::new(&AgentConfig {
                model: Some(model.to_string()),
                provider_type: Some(provider),
                ..Default::default()
            })
            .limits()
            .await;
            assert_eq!(
                profiled.connect_deadline_ms, 90_000,
                "catalog long-prefill profile missing for {provider:?}:{model}"
            );
            assert_eq!(profiled.first_token_deadline_ms, 150_000);
        }
    }

    #[tokio::test]
    async fn delegation_limits_v2_overrides_legacy_dimensions_and_deadlines() {
        let config = AgentConfig {
            provider_type: Some(ProviderType::Ollama),
            subagent_max_parallel: Some(2),
            subagent_token_budget: Some(12_000),
            delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
                input_context_limit: Some(1_000_000),
                handoff_context_tokens_per_worker: Some(40_000),
                max_output_tokens_per_step: None,
                max_output_tokens_per_worker: Some(65_536),
                max_actual_tokens_per_worker: Some(96_000),
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
            Some(1_048_576),
            64_000,
        );
        let second = load_delegation_context_snapshot(
            &db,
            Some(&conversation.id),
            "gemini-2.5-pro",
            Some(1_048_576),
            64_000,
        );

        assert_eq!(first.id, second.id);
        assert_eq!(first.selected_message_ids.as_ref(), &["parent-message"]);
        assert_eq!(
            first.messages[0].text_content(),
            "Parent context that the delegated worker needs"
        );
        assert_eq!(first.context_limit, Some(1_048_576));
        assert_eq!(first.handoff_token_budget, 64_000);
    }

    #[test]
    fn oversized_parent_message_cannot_overrun_worker_handoff_budget() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "qwen".to_string(),
                model: "qwen3.8-max".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        db.add_message(&ConversationMessage {
            id: "oversized-parent".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "large parent context ".repeat(20_000),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 100_000,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        })
        .unwrap();

        let snapshot = load_delegation_context_snapshot(
            &db,
            Some(&conversation.id),
            "qwen3.8-max",
            Some(1_000_000),
            10_000,
        );
        assert!(snapshot.token_estimate <= 10_000);
        assert!(snapshot.messages.is_empty());
        assert_eq!(snapshot.dropped_invalid_messages, 1);
        assert_eq!(snapshot.context_limit, Some(1_000_000));
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
