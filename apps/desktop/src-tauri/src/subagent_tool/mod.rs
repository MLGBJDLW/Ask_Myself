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

/// Owns the durable subtask row and its parent timeline projection.
///
/// Callers still own admission-budget rollback because that is async, while
/// this guard guarantees every created row receives a terminal settlement.
struct SubtaskRecorder {
    db: Database,
    parent_run_id: Option<String>,
    subtask_run_id: Option<String>,
    call_label: String,
    settled: bool,
}

impl SubtaskRecorder {
    #[allow(clippy::too_many_arguments)]
    fn create(
        db: &Database,
        parent_run_id: Option<&str>,
        call_label: &str,
        role_label: &str,
        input: &serde_json::Value,
        reserved_tokens: u32,
        queued_label: String,
        queued_payload: serde_json::Value,
    ) -> Result<Self, CoreError> {
        let subtask_run_id = if let Some(parent_run_id) = parent_run_id {
            let subtask = db.create_agent_subtask_run(
                parent_run_id,
                call_label,
                role_label,
                Some(input),
                Some(reserved_tokens),
            )?;
            let mut payload = queued_payload;
            payload["subtaskRunId"] = serde_json::json!(&subtask.id);
            record_subtask_event(db, parent_run_id, &queued_label, "queued", Some(&payload));
            Some(subtask.id)
        } else {
            None
        };
        Ok(Self {
            db: db.clone(),
            parent_run_id: parent_run_id.map(str::to_string),
            subtask_run_id,
            call_label: call_label.to_string(),
            settled: false,
        })
    }

    fn id(&self) -> Option<&str> {
        self.subtask_run_id.as_deref()
    }

    fn record_launch_metrics(&self, metrics: &[(&str, Option<u64>, Option<&str>, &str)]) {
        let (Some(parent_run_id), Some(subtask_run_id)) =
            (self.parent_run_id.as_deref(), self.id())
        else {
            return;
        };
        for (stage, elapsed_ms, provider_invocation_id, status) in metrics {
            record_subagent_launch_metric(
                &self.db,
                parent_run_id,
                subtask_run_id,
                &self.call_label,
                stage,
                *elapsed_ms,
                *provider_invocation_id,
                status,
            );
        }
    }

    fn emit(&self, label: String, status: &str, payload: &serde_json::Value) {
        if let Some(parent_run_id) = self.parent_run_id.as_deref() {
            record_subtask_event(&self.db, parent_run_id, &label, status, Some(payload));
        }
    }

    fn mark_started(
        &self,
        run_status: &str,
        event_label: String,
        event_payload: serde_json::Value,
    ) -> Result<(), CoreError> {
        if let Some(subtask_run_id) = self.id() {
            self.db
                .mark_agent_subtask_run_started(subtask_run_id, run_status)?;
        }
        self.emit(event_label, "running", &event_payload);
        Ok(())
    }

    fn finish(
        &mut self,
        status: &str,
        output: Option<&serde_json::Value>,
        error_message: Option<&str>,
        event_label: Option<String>,
    ) {
        if self.settled {
            return;
        }
        finish_subtask_run_best_effort(&self.db, self.id(), status, output, error_message);
        if let (Some(label), Some(payload)) = (event_label, output) {
            self.emit(label, status, payload);
        }
        self.settled = true;
    }
}

impl Drop for SubtaskRecorder {
    fn drop(&mut self) {
        if self.settled || self.subtask_run_id.is_none() {
            return;
        }
        let error = "Subtask exited without an explicit terminal settlement";
        finish_subtask_run_best_effort(&self.db, self.id(), "failed", None, Some(error));
        let payload = serde_json::json!({
            "kind": "subtask_settlement_error",
            "callLabel": &self.call_label,
            "error": error,
        });
        self.emit(
            format!("Subagent failed: {}", self.call_label),
            "failed",
            &payload,
        );
        self.settled = true;
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

mod catalog;
mod event_pump;
mod judge;
mod policy;
mod preflight;
mod request;
mod tools;
mod worker;

use catalog::*;
use event_pump::*;
use policy::*;
use preflight::*;
use request::*;
use worker::*;

#[cfg(test)]
mod tests;
