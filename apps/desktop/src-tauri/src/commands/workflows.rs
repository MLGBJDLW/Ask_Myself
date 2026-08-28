use super::conversation::desktop_package_host_snapshot;
use super::*;
use crate::desktop_agent_session::resolve_desktop_pending_approvals_for_stopped_run;
use chrono::DateTime;
use nexa_core::package_host::PackageSurfaceKind;
use nexa_core::task_orchestrator::{
    workflow_automation_delivery_envelope, workflow_automation_execution_ticket,
    workflow_due_run_delivery_envelope, workflow_due_run_execution_ticket,
    workflow_due_run_queue_item, TaskOrchestratorDeliveryEnvelope, TaskOrchestratorExecutionTicket,
    TaskOrchestratorQueueItem,
};
use nexa_core::tools::{
    browser_evidence_tool::BrowserEvidenceCaptureTool,
    capability::{scheduled_workspace_tool_class, ScheduledWorkspaceToolClass},
    Tool,
};
use nexa_core::workflow_automation::{
    BrowserEvidenceCapture, InvestigationGraph, LearningGovernanceSnapshot,
    SaveWorkflowAutomationInput, TaskResumeCheckpoint, TaskResumePrompt, WorkflowAutomation,
    WorkflowAutomationApprovalPolicy, WorkflowAutomationDueRun, WorkflowAutomationRun,
    WorkflowAutomationRunStatus, WorkflowAutomationSchedulerEvent,
    WorkflowAutomationSchedulerRetryDecision, WorkflowAutomationTrigger,
};
use nexa_core::workflow_scheduler::{
    legacy_workflow_schedule_config, preview_workflow_cron_schedule,
    WorkflowAutomationExecutionPolicy, WorkflowAutomationScheduleConfig, WorkflowSchedulePreview,
    WorkflowScheduleWorkspacePolicy,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorWorkflowLaunch {
    pub ticket: TaskOrchestratorExecutionTicket,
    pub conversation_id: String,
    pub task_run_id: String,
    pub task_orchestrator_run_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOrchestratorWorkflowStartOutcome {
    Launched {
        launch: TaskOrchestratorWorkflowLaunch,
    },
    PendingApproval {
        run: WorkflowAutomationRun,
    },
    Skipped {
        reason: String,
    },
}

pub struct TaskOrchestratorSchedulerState {
    pub shutdown: Arc<AtomicBool>,
    pub tick_lock: TokioMutex<()>,
    pub poll_interval_secs: u64,
    pub max_concurrent_due_runs: usize,
}

struct DesktopTaskOrchestratorLaunchRequest<'a> {
    state: &'a AppState,
    agent_state: &'a AgentState,
    mcp_state: &'a McpManagerState,
    approval_state: &'a ApprovalState,
    app_handle: AppHandle,
    ticket: TaskOrchestratorExecutionTicket,
    launch_policy: ScheduledWorkflowLaunchPolicy,
    conversation_id: Option<String>,
    persona_id: Option<String>,
    skill_ids: Option<Vec<String>>,
    delivery_kind: &'static str,
}

#[derive(Debug, Clone)]
struct ScheduledWorkflowLaunchPolicy {
    selected_config: DbAgentConfig,
    project_id: Option<String>,
    force_workspace_isolation: bool,
    source_root_fingerprint: Option<String>,
    execution_mode: Option<String>,
    power_mode: String,
    collaboration_mode: String,
    orchestration_profile: String,
    /// Scheduled runs always carry an explicit capability boundary. An empty
    /// list intentionally means no tools, never "inherit the root registry".
    allowed_tools: Option<Vec<String>>,
    /// The occurrence approval plus immutable allowlist is the unattended
    /// grant. Hard interactive confirmations remain enforced downstream.
    tool_approval_mode: nexa_core::approval::ToolApprovalMode,
    /// Scheduled policy snapshots are per-run authorities and must not be
    /// replaced by a later Capability Registry lookup.
    agent_config_is_authoritative: bool,
}

#[derive(Debug)]
enum ScheduledWorkflowLaunchOutcome {
    Launched(TaskOrchestratorWorkflowLaunch),
    PendingApproval { run: WorkflowAutomationRun },
    Skipped { reason: String },
}

struct AuthoritativeScheduledWorkflowLaunchRequest<'a> {
    state: &'a AppState,
    agent_state: &'a AgentState,
    mcp_state: &'a McpManagerState,
    approval_state: &'a ApprovalState,
    app_handle: AppHandle,
    due: WorkflowAutomationDueRun,
    now: &'a str,
    conversation_id: Option<String>,
    summary: Option<String>,
    delivery_kind: &'static str,
}

impl ScheduledWorkflowLaunchOutcome {
    fn into_launch(self) -> Option<TaskOrchestratorWorkflowLaunch> {
        match self {
            Self::Launched(launch) => Some(launch),
            Self::PendingApproval { .. } | Self::Skipped { .. } => None,
        }
    }
}

impl From<ScheduledWorkflowLaunchOutcome> for TaskOrchestratorWorkflowStartOutcome {
    fn from(value: ScheduledWorkflowLaunchOutcome) -> Self {
        match value {
            ScheduledWorkflowLaunchOutcome::Launched(launch) => Self::Launched { launch },
            ScheduledWorkflowLaunchOutcome::PendingApproval { run } => {
                Self::PendingApproval { run }
            }
            ScheduledWorkflowLaunchOutcome::Skipped { reason } => Self::Skipped { reason },
        }
    }
}

pub(super) fn workflow_due_runs_to_queue_items(
    due_runs: &[WorkflowAutomationDueRun],
) -> Vec<TaskOrchestratorQueueItem> {
    due_runs.iter().map(workflow_due_run_queue_item).collect()
}

fn visible_desktop_workflow_template_ids(
    db: &Database,
) -> Result<std::collections::HashSet<String>, String> {
    let snapshot = desktop_package_host_snapshot(db)?;
    Ok(snapshot
        .runtime_components()
        .into_iter()
        .filter(|component| component.kind == PackageSurfaceKind::Workflow)
        .map(|component| component.id.clone())
        .collect())
}

pub(super) fn ensure_workflow_template_runtime_visible(
    db: &Database,
    workflow_template_id: &str,
) -> Result<(), String> {
    let visible_workflow_ids = visible_desktop_workflow_template_ids(db)?;
    if visible_workflow_ids.contains(workflow_template_id) {
        Ok(())
    } else {
        Err(format!(
            "Workflow template '{workflow_template_id}' is disabled or unavailable through Package Host."
        ))
    }
}

pub(super) fn filter_due_workflow_runs_by_package_host(
    db: &Database,
    due_runs: Vec<WorkflowAutomationDueRun>,
) -> Result<Vec<WorkflowAutomationDueRun>, String> {
    let visible_workflow_ids = visible_desktop_workflow_template_ids(db)?;
    Ok(due_runs
        .into_iter()
        .filter(|due| visible_workflow_ids.contains(&due.automation.workflow_template_id))
        .collect())
}

#[cfg(test)]
pub(super) fn task_orchestrator_scheduler_status_is_active(status: &str) -> bool {
    let raw = status.trim().to_ascii_lowercase();
    if raw == "cancelling" {
        return true;
    }
    nexa_core::task_orchestrator::project_task_status(&raw)
        .map(|projection| {
            matches!(
                projection.state,
                nexa_core::task_orchestrator::TaskOrchestratorState::Queued
                    | nexa_core::task_orchestrator::TaskOrchestratorState::Running
                    | nexa_core::task_orchestrator::TaskOrchestratorState::WaitingApproval
                    | nexa_core::task_orchestrator::TaskOrchestratorState::Paused
                    | nexa_core::task_orchestrator::TaskOrchestratorState::Resuming
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
pub(super) fn due_workflow_run_is_scheduler_eligible(due: &WorkflowAutomationDueRun) -> bool {
    !due.automation.approval_policy.require_before_run
        && !task_orchestrator_scheduler_status_is_active(&due.automation.status)
}

#[cfg(test)]
pub(super) fn task_orchestrator_scheduler_due_runs(
    db: &Database,
    now: &str,
) -> Result<Vec<WorkflowAutomationDueRun>, String> {
    let due_runs = db
        .list_due_workflow_automations(now)
        .map_err(|err| err.to_string())?;
    let due_runs = filter_due_workflow_runs_by_package_host(db, due_runs)?;
    let mut out = Vec::new();
    for due in due_runs {
        if !due_workflow_run_is_scheduler_eligible(&due) {
            continue;
        }
        let retry_decision = db
            .workflow_automation_scheduler_retry_decision(&due.automation.id, now)
            .map_err(|err| err.to_string())?;
        if retry_decision.allowed {
            out.push(due);
        }
    }
    Ok(out)
}

fn record_task_orchestrator_scheduler_event(
    db: &Database,
    automation_id: Option<&str>,
    run_id: Option<&str>,
    event_type: &str,
    status: Option<&str>,
    summary: &str,
    payload: serde_json::Value,
) {
    if let Err(err) = db.record_workflow_automation_scheduler_event(
        automation_id,
        run_id,
        event_type,
        status,
        summary,
        Some(&payload),
    ) {
        warn!("Failed to persist Task Orchestrator scheduler event {event_type}: {err}");
    }
}

pub(super) fn task_orchestrator_scheduler_retry_skip_event(
    retry_decision: &WorkflowAutomationSchedulerRetryDecision,
) -> (&'static str, &'static str, &'static str) {
    if retry_decision.attempts_exhausted {
        (
            "skipped_retry_limit",
            "blocked",
            "Scheduler skipped due workflow because retry attempts are exhausted",
        )
    } else {
        (
            "skipped_backoff",
            "backoff",
            "Scheduler skipped due workflow until retry backoff expires",
        )
    }
}

#[cfg(test)]
pub(super) fn queue_due_workflow_automation_execution_ticket(
    db: &Database,
    id: &str,
    now: &str,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let due = find_due_workflow_automation(db, id, now)?;
    claim_due_workflow_automation_execution_ticket(db, due, now, summary)
}

fn find_due_workflow_automation(
    db: &Database,
    id: &str,
    now: &str,
) -> Result<WorkflowAutomationDueRun, String> {
    let due_runs = db
        .list_due_workflow_automations(now)
        .map_err(|err| err.to_string())?;
    let due_runs = filter_due_workflow_runs_by_package_host(db, due_runs)?;
    due_runs
        .into_iter()
        .find(|due| due.automation.id == id)
        .ok_or_else(|| format!("Workflow automation '{id}' is not currently due."))
}

fn claim_due_workflow_automation_execution_ticket(
    db: &Database,
    due: WorkflowAutomationDueRun,
    now: &str,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let claim = claim_due_workflow_automation_execution(db, due, now, summary)?;
    let run = claim
        .run
        .as_ref()
        .expect("launchable claim must include a run");
    workflow_due_run_execution_ticket(&claim.due_run, run).map_err(|err| err.to_string())
}

fn claim_due_workflow_automation_execution(
    db: &Database,
    due: WorkflowAutomationDueRun,
    now: &str,
    summary: Option<String>,
) -> Result<nexa_core::workflow_automation::WorkflowAutomationDueRunClaim, String> {
    let claim_summary = summary.unwrap_or_else(|| due.due_reason.clone());
    let claim = db
        .claim_workflow_automation_due_run_at(due, now, Some(claim_summary.as_str()))
        .map_err(|err| err.to_string())?;
    claim.run.as_ref().ok_or_else(|| {
        format!(
            "workflow_occurrence_skipped:{}",
            claim.skip_reason.as_deref().unwrap_or("not_launchable")
        )
    })?;
    Ok(claim)
}

pub(super) fn select_task_orchestrator_launch_agent_config(
    db: &Database,
    requested_config_id: Option<&str>,
) -> Result<DbAgentConfig, String> {
    let configs = db.list_agent_configs().map_err(|err| err.to_string())?;
    if let Some(id) = requested_config_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return configs
            .into_iter()
            .find(|config| config.id == id)
            .ok_or_else(|| format!("Requested agent config '{id}' was not found."));
    }

    configs
        .iter()
        .find(|config| config.is_default)
        .cloned()
        .or_else(|| configs.first().cloned())
        .ok_or_else(|| "No agent config set. Please configure an LLM provider first.".to_string())
}

fn validate_scheduled_execution_route(
    policy: &WorkflowAutomationExecutionPolicy,
    provider: &str,
    provider_endpoint_id: Option<&str>,
) -> Result<(), String> {
    if let Some(expected_provider) = policy
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider != expected_provider {
            return Err(format!(
                "Scheduled workflow provider drift: saved '{expected_provider}', agent config now uses '{}'",
                provider
            ));
        }
    }
    if let Some(expected_endpoint_id) = policy
        .provider_endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider_endpoint_id != Some(expected_endpoint_id) {
            return Err(format!(
                "Scheduled workflow endpoint drift: saved '{expected_endpoint_id}', agent config now uses '{}'",
                provider_endpoint_id.unwrap_or("unknown")
            ));
        }
    }
    Ok(())
}

pub(super) fn apply_scheduled_execution_policy(
    mut config: DbAgentConfig,
    policy: &WorkflowAutomationExecutionPolicy,
) -> Result<DbAgentConfig, String> {
    validate_scheduled_execution_route(
        policy,
        &config.provider,
        config.provider_endpoint_id.as_deref(),
    )?;
    if let Some(model) = policy
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if config.model != model {
            config.model = model.to_string();
            config.model_id = None;
            config.model_selection_resolution = None;
        }
    }
    config.context_window = policy.context_window.map(i64::from);
    Ok(config)
}

fn scheduled_workflow_launch_policy(
    config: DbAgentConfig,
    execution_policy: &WorkflowAutomationExecutionPolicy,
    approval_policy: &WorkflowAutomationApprovalPolicy,
) -> Result<ScheduledWorkflowLaunchPolicy, String> {
    let selected_config = apply_scheduled_execution_policy(config, execution_policy)?;
    let allowed_tools = Some(approval_policy.allowed_tools.clone());
    Ok(ScheduledWorkflowLaunchPolicy {
        selected_config,
        project_id: execution_policy.project_id.clone(),
        force_workspace_isolation: execution_policy.workspace_policy
            == WorkflowScheduleWorkspacePolicy::IsolatedPatch,
        source_root_fingerprint: execution_policy.source_root_fingerprint.clone(),
        execution_mode: execution_policy.execution_mode.clone(),
        power_mode: execution_policy.power_mode.clone(),
        collaboration_mode: execution_policy.collaboration_mode.clone(),
        orchestration_profile: execution_policy.orchestration_profile.clone(),
        allowed_tools,
        tool_approval_mode: nexa_core::approval::ToolApprovalMode::AllowAll,
        agent_config_is_authoritative: true,
    })
}

fn resolve_authoritative_workflow_launch_policy(
    db: &Database,
    automation: &WorkflowAutomation,
) -> Result<ScheduledWorkflowLaunchPolicy, String> {
    let execution_policy = &automation.schedule_config.execution_policy;
    if let Some(project_id) = execution_policy.project_id.as_deref() {
        let project = db.get_project(project_id).map_err(|error| {
            format!("Scheduled workflow project '{project_id}' is unavailable: {error}")
        })?;
        if project.archived {
            return Err(format!(
                "Scheduled workflow project '{}' is archived.",
                project.name
            ));
        }
        let project_sources = project.source_scope.unwrap_or_default();
        if automation
            .source_scope
            .iter()
            .any(|source_id| !project_sources.iter().any(|allowed| allowed == source_id))
        {
            return Err(
                "Scheduled workflow source scope drifted outside its saved project boundary."
                    .to_string(),
            );
        }
    }
    validate_scheduled_workspace_target(db, automation)?;
    let selected_config = select_task_orchestrator_launch_agent_config(
        db,
        execution_policy.agent_config_id.as_deref(),
    )?;
    scheduled_workflow_launch_policy(
        selected_config,
        execution_policy,
        &automation.approval_policy,
    )
}

fn interactive_workflow_launch_policy(
    selected_config: DbAgentConfig,
    execution_policy: &WorkflowAutomationExecutionPolicy,
    approval_policy: &WorkflowAutomationApprovalPolicy,
) -> Result<ScheduledWorkflowLaunchPolicy, String> {
    let selected_config = apply_scheduled_execution_policy(selected_config, execution_policy)?;
    Ok(ScheduledWorkflowLaunchPolicy {
        selected_config,
        project_id: execution_policy.project_id.clone(),
        force_workspace_isolation: false,
        source_root_fingerprint: None,
        execution_mode: execution_policy.execution_mode.clone(),
        power_mode: execution_policy.power_mode.clone(),
        collaboration_mode: execution_policy.collaboration_mode.clone(),
        orchestration_profile: execution_policy.orchestration_profile.clone(),
        allowed_tools: (!approval_policy.allowed_tools.is_empty())
            .then(|| approval_policy.allowed_tools.clone()),
        tool_approval_mode: nexa_core::approval::ToolApprovalMode::Ask,
        agent_config_is_authoritative: false,
    })
}

fn resolve_interactive_workflow_launch_policy(
    db: &Database,
    automation: &WorkflowAutomation,
) -> Result<ScheduledWorkflowLaunchPolicy, String> {
    let execution_policy = &automation.schedule_config.execution_policy;
    let selected_config = select_task_orchestrator_launch_agent_config(
        db,
        execution_policy.agent_config_id.as_deref(),
    )?;
    interactive_workflow_launch_policy(
        selected_config,
        execution_policy,
        &automation.approval_policy,
    )
}

fn snapshot_scheduled_agent_config(
    schedule_config: &mut WorkflowAutomationScheduleConfig,
    selected_config: &DbAgentConfig,
) {
    let execution_policy = &mut schedule_config.execution_policy;
    execution_policy.agent_config_id = Some(selected_config.id.clone());
    execution_policy.provider = Some(selected_config.provider.clone());
    execution_policy.provider_endpoint_id = selected_config.provider_endpoint_id.clone();
    if execution_policy
        .model
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        execution_policy.model = Some(selected_config.model.clone());
    } else {
        execution_policy.model = execution_policy
            .model
            .as_deref()
            .map(str::trim)
            .map(str::to_string);
    }
    // `None` remains an explicit Auto policy. Do not copy a stale configured
    // context ceiling into a newly scheduled run.
}

fn normalize_scheduled_config_agent_snapshot(
    db: &Database,
    mut schedule_config: WorkflowAutomationScheduleConfig,
) -> Result<WorkflowAutomationScheduleConfig, String> {
    if let Some(project_id) = schedule_config
        .execution_policy
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let project = db.get_project(project_id).map_err(|error| {
            format!("Scheduled workflow project '{project_id}' is unavailable: {error}")
        })?;
        if project.archived {
            return Err(format!(
                "Scheduled workflow project '{}' is archived.",
                project.name
            ));
        }
        schedule_config.execution_policy.project_id = Some(project_id.to_string());
    } else {
        schedule_config.execution_policy.project_id = None;
    }
    let requested_config_id = schedule_config
        .execution_policy
        .agent_config_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(config_id) = requested_config_id {
        let selected_config = select_task_orchestrator_launch_agent_config(db, Some(&config_id))?;
        snapshot_scheduled_agent_config(&mut schedule_config, &selected_config);
    }
    Ok(schedule_config)
}

#[derive(Debug)]
struct ScheduledWorkspaceTarget {
    source_scope: Vec<String>,
    source_root_fingerprint: Option<String>,
}

fn scheduled_source_root_fingerprint(db: &Database, source_id: &str) -> Result<String, String> {
    let source = db.get_source(source_id).map_err(|error| {
        format!("Scheduled workflow source '{source_id}' is unavailable: {error}")
    })?;
    let canonical = std::fs::canonicalize(&source.root_path).map_err(|error| {
        format!(
            "Scheduled workflow source '{}' cannot be canonicalized: {error}",
            source.root_path
        )
    })?;
    let normalized = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let normalized = normalized.to_ascii_lowercase();
    Ok(format!(
        "blake3:{}",
        blake3::hash(normalized.as_bytes()).to_hex()
    ))
}

fn validate_scheduled_allowed_tools(
    allowed_tools: &[String],
    workspace_policy: WorkflowScheduleWorkspacePolicy,
) -> Result<(), String> {
    let mut has_isolatable_write = false;
    for tool in allowed_tools {
        let normalized = tool.trim();
        let delegates = matches!(
            normalized,
            "spawn_subagent"
                | "spawn_subagent_batch"
                | "observe_subagent"
                | "observe_subagent_batch"
                | "wait_subagent"
                | "send_subagent_input"
                | "cancel_subagent"
                | "close_subagent"
                | "judge_subagent_results"
        );
        if delegates && workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch {
            return Err(format!(
                "Scheduled tool '{normalized}' cannot yet inherit the isolated patch sandbox."
            ));
        }
        match scheduled_workspace_tool_class(tool) {
            ScheduledWorkspaceToolClass::Independent => {}
            ScheduledWorkspaceToolClass::IsolatableWrite
                if workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch =>
            {
                has_isolatable_write = true;
            }
            ScheduledWorkspaceToolClass::IsolatableWrite => {
                return Err(format!(
                    "Scheduled tool '{}' can write a workspace; select isolated_patch or remove it.",
                    tool.trim()
                ));
            }
            ScheduledWorkspaceToolClass::Unsupported => {
                return Err(format!(
                    "Scheduled tool '{}' is not supported for unattended execution.",
                    tool.trim()
                ));
            }
        }
    }
    if workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch && !has_isolatable_write {
        return Err(
            "Isolated scheduled patches require at least one isolation-safe write tool."
                .to_string(),
        );
    }
    Ok(())
}

fn resolve_scheduled_workspace_target(
    db: &Database,
    input: &SaveWorkflowAutomationInput,
    schedule_config: &WorkflowAutomationScheduleConfig,
) -> Result<ScheduledWorkspaceTarget, String> {
    let mut source_scope = input.source_scope.clone();
    let policy = schedule_config.execution_policy.workspace_policy;
    let is_schedule = matches!(&input.trigger, WorkflowAutomationTrigger::Schedule { .. });
    if !is_schedule {
        return Ok(ScheduledWorkspaceTarget {
            source_scope,
            source_root_fingerprint: None,
        });
    }
    validate_scheduled_allowed_tools(&input.approval_policy.allowed_tools, policy)?;
    if source_scope.is_empty() && policy != WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        if let Some(project_id) = schedule_config.execution_policy.project_id.as_deref() {
            source_scope = db
                .get_project(project_id)
                .map_err(|error| error.to_string())?
                .source_scope
                .unwrap_or_default();
        }
    }
    for source_id in &source_scope {
        db.get_source(source_id).map_err(|error| {
            format!("Scheduled workflow source '{source_id}' is unavailable: {error}")
        })?;
    }
    if let Some(project_id) = schedule_config.execution_policy.project_id.as_deref() {
        let project_sources = db
            .get_project(project_id)
            .map_err(|error| error.to_string())?
            .source_scope
            .unwrap_or_default();
        if source_scope
            .iter()
            .any(|source_id| !project_sources.iter().any(|allowed| allowed == source_id))
        {
            return Err(
                "Scheduled source scope must remain inside the selected project's source boundary."
                    .to_string(),
            );
        }
    }
    let source_root_fingerprint = if policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        if source_scope.len() != 1 {
            return Err(format!(
                "Isolated scheduled patches require exactly one explicit Source; found {}.",
                source_scope.len()
            ));
        }
        Some(scheduled_source_root_fingerprint(db, &source_scope[0])?)
    } else {
        None
    };
    Ok(ScheduledWorkspaceTarget {
        source_scope,
        source_root_fingerprint,
    })
}

fn validate_scheduled_workspace_target(
    db: &Database,
    automation: &WorkflowAutomation,
) -> Result<(), String> {
    let policy = automation.schedule_config.execution_policy.workspace_policy;
    validate_scheduled_allowed_tools(&automation.approval_policy.allowed_tools, policy)?;
    if policy != WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        return Ok(());
    }
    if automation.source_scope.len() != 1 {
        return Err(format!(
            "Isolated scheduled patches require exactly one snapshotted Source; found {}.",
            automation.source_scope.len()
        ));
    }
    let current = scheduled_source_root_fingerprint(db, &automation.source_scope[0])?;
    let expected = automation
        .schedule_config
        .execution_policy
        .source_root_fingerprint
        .as_deref()
        .ok_or_else(|| "Isolated scheduled patch is missing its Source fingerprint.".to_string())?;
    if current != expected {
        return Err(
            "Scheduled Source root changed after this definition was saved; review and save it again."
                .to_string(),
        );
    }
    Ok(())
}

fn schedule_config_for_save(
    db: &Database,
    input: &SaveWorkflowAutomationInput,
    schedule_config: Option<WorkflowAutomationScheduleConfig>,
) -> Result<WorkflowAutomationScheduleConfig, String> {
    let schedule_config = match (&input.trigger, schedule_config) {
        (WorkflowAutomationTrigger::Schedule { cron }, None) => {
            let legacy = legacy_workflow_schedule_config(cron);
            if legacy.legacy_needs_review {
                return Err(format!(
                    "Legacy schedule '{cron}' requires review with an explicit timezone and scheduleConfig before it can be saved."
                ));
            }
            legacy
        }
        (_, Some(config)) => config,
        (_, None) => WorkflowAutomationScheduleConfig::default(),
    };
    normalize_scheduled_config_agent_snapshot(db, schedule_config)
}

async fn launch_task_orchestrator_execution_ticket(
    request: DesktopTaskOrchestratorLaunchRequest<'_>,
) -> Result<TaskOrchestratorWorkflowLaunch, String> {
    let DesktopTaskOrchestratorLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        app_handle,
        ticket,
        launch_policy,
        conversation_id,
        persona_id,
        skill_ids,
        delivery_kind,
    } = request;
    let ScheduledWorkflowLaunchPolicy {
        selected_config,
        project_id,
        force_workspace_isolation,
        source_root_fingerprint,
        execution_mode,
        power_mode,
        collaboration_mode,
        orchestration_profile,
        allowed_tools,
        tool_approval_mode,
        agent_config_is_authoritative,
    } = launch_policy;
    let conversation_id = match conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => {
            let conversation = state
                .db
                .get_conversation(id)
                .map_err(|err| err.to_string())?;
            if conversation.project_id != project_id {
                return Err(format!(
                    "Scheduled workflow project drift: saved '{}', conversation uses '{}'.",
                    project_id.as_deref().unwrap_or("none"),
                    conversation.project_id.as_deref().unwrap_or("none")
                ));
            }
            id.to_string()
        }
        None => {
            state
                .db
                .create_conversation(&CreateConversationInput {
                    provider: selected_config.provider.clone(),
                    model: selected_config.model.clone(),
                    system_prompt: None,
                    collection_context: None,
                    project_id: project_id.clone(),
                    persona_id: persona_id.clone(),
                })
                .map_err(|err| err.to_string())?
                .id
        }
    };

    state
        .db
        .set_conversation_sources(
            &conversation_id,
            &ticket.delivery.queue_item.ownership.source_scope,
        )
        .map_err(|err| err.to_string())?;

    let queue_id = ticket.delivery.queue_item.queue_id.clone();
    let workflow_run_id = ticket.run.run_id.clone();
    let workflow_run_snapshot = state
        .db
        .get_workflow_automation_run(&workflow_run_id)
        .map_err(|error| error.to_string())?;
    let launch_result = launch_desktop_agent_chat_turn(DesktopAgentChatLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        terminal_state: None,
        browser_state: app_handle
            .state::<crate::browser::BrowserState>()
            .inner()
            .clone(),
        app_handle,
        conversation_id,
        message: ticket.delivery.prompt.clone(),
        attachments: None,
        agent_config_id: Some(selected_config.id.clone()),
        agent_config_override: Some(selected_config.clone()),
        agent_config_override_is_authoritative: agent_config_is_authoritative,
        persona_id,
        skill_ids,
        execution_mode,
        power_mode: Some(power_mode),
        collaboration_mode: Some(collaboration_mode),
        moa_preset: Some("fastReview".to_string()),
        orchestration_profile: Some(orchestration_profile),
        custom_orchestration: None,
        vision_turn_override: None,
        root_allowed_tools: allowed_tools.clone(),
        tool_approval_mode_override: Some(tool_approval_mode),
        force_workspace_isolation,
        user_artifacts: Some(serde_json::json!({
            "kind": "taskOrchestratorLaunch",
            "version": 1,
            "delivery": delivery_kind,
            "queueId": queue_id,
            "workflowRunId": workflow_run_id,
            "occurrenceId": workflow_run_snapshot.occurrence_id,
            "scheduledFor": workflow_run_snapshot.scheduled_for,
            "definitionRevision": workflow_run_snapshot.definition_revision,
            "attempt": workflow_run_snapshot.attempt,
            "scheduledAllowedTools": allowed_tools,
            "scheduledToolGrant": {
                "authority": "savedAllowlist",
                "approvalMode": "allow_all_within_saved_allowlist",
                "hardInteractiveConfirmationsRemainRequired": true,
            },
            "scheduledRoute": {
                "authority": "scheduledPolicy",
                "projectId": project_id,
                "workspacePolicy": if force_workspace_isolation { "isolated_patch" } else { "deny_writes" },
                "sourceRootFingerprint": source_root_fingerprint,
                "agentConfigId": selected_config.id,
                "provider": selected_config.provider,
                "providerEndpointId": selected_config.provider_endpoint_id,
                "model": selected_config.model,
                "contextWindow": selected_config.context_window,
            },
        })),
        task_orchestrator_run_id: Some(workflow_run_id.clone()),
        resume_checkpoint_id: None,
        retry_from_message_id: None,
        idempotency_key: format!("workflow:{queue_id}"),
    })
    .await;

    let launch = match launch_result {
        Ok(launch) => launch,
        Err(err) => {
            if let Err(transition_err) = state.db.mark_workflow_automation_launch_failed_for_retry(
                &workflow_run_id,
                &err,
                &Utc::now().to_rfc3339(),
            ) {
                warn!(
                    "Failed to schedule Task Orchestrator run {workflow_run_id} retry after launch failure: {transition_err}"
                );
            }
            return Err(err);
        }
    };

    Ok(TaskOrchestratorWorkflowLaunch {
        ticket,
        conversation_id: launch.conversation_id,
        task_run_id: launch.task_run_id,
        task_orchestrator_run_id: workflow_run_id,
    })
}

async fn launch_authoritative_scheduled_workflow(
    request: AuthoritativeScheduledWorkflowLaunchRequest<'_>,
) -> Result<ScheduledWorkflowLaunchOutcome, String> {
    let AuthoritativeScheduledWorkflowLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        app_handle,
        due,
        now,
        conversation_id,
        summary,
        delivery_kind,
    } = request;
    let automation_id = due.automation.id.clone();
    let due_reason = due.due_reason.clone();
    let uses_durable_occurrence = due.automation.trigger_kind == "schedule";

    if !uses_durable_occurrence {
        let retry_decision = state
            .db
            .workflow_automation_scheduler_retry_decision(&automation_id, now)
            .map_err(|error| error.to_string())?;
        if !retry_decision.allowed {
            let (event_type, status, event_summary) =
                task_orchestrator_scheduler_retry_skip_event(&retry_decision);
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                None,
                event_type,
                Some(status),
                event_summary,
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": due.automation.trigger_kind,
                    "workflowTemplateId": due.automation.workflow_template_id,
                    "retryDecision": retry_decision,
                }),
            );
            return Ok(ScheduledWorkflowLaunchOutcome::Skipped {
                reason: if retry_decision.attempts_exhausted {
                    "retry_exhausted".into()
                } else {
                    "retry_backoff".into()
                },
            });
        }
        // Folder triggers do not have schedule occurrences. Preserve the
        // pre-claim approval boundary so the matching file change and
        // `last_run_at` are not consumed before an operator can act.
        if requires_preclaim_approval_skip(
            &due.automation.trigger_kind,
            due.automation.approval_policy.require_before_run,
        ) {
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                None,
                "skipped_pre_run_approval",
                Some("waiting_approval"),
                "Scheduler skipped due workflow because pre-run approval is required",
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": due.automation.trigger_kind,
                    "workflowTemplateId": due.automation.workflow_template_id,
                    "riskLevel": due.automation.approval_policy.risk_level,
                }),
            );
            return Ok(ScheduledWorkflowLaunchOutcome::Skipped {
                reason: "approval_required_before_claim".into(),
            });
        }
    }

    let claim = match claim_due_workflow_automation_execution(
        state.db.as_ref(),
        due,
        now,
        summary.or_else(|| Some(format!("scheduler: {due_reason}"))),
    ) {
        Ok(ticket) => ticket,
        Err(error) => {
            if let Some(reason) = error.strip_prefix("workflow_occurrence_skipped:") {
                if !matches!(
                    reason,
                    "already_claimed_live" | "already_consumed" | "retry_backoff"
                ) {
                    record_task_orchestrator_scheduler_event(
                        state.db.as_ref(),
                        Some(&automation_id),
                        None,
                        "occurrence_skipped",
                        Some("skipped"),
                        "Scheduler applied the saved occurrence policy",
                        serde_json::json!({
                            "dueReason": due_reason,
                            "skipReason": reason,
                        }),
                    );
                }
                return Ok(ScheduledWorkflowLaunchOutcome::Skipped {
                    reason: reason.to_string(),
                });
            }
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                None,
                "claim_failed",
                Some("failed"),
                "Scheduler failed to claim due workflow",
                serde_json::json!({
                    "dueReason": due_reason,
                    "error": error,
                }),
            );
            return Err(error);
        }
    };
    let claimed_run = claim
        .run
        .as_ref()
        .expect("launchable workflow claim must include a run");
    let run_id = claimed_run.id.clone();
    if uses_durable_occurrence {
        let occurrence_id = claimed_run.occurrence_id.as_deref().ok_or_else(|| {
            "Scheduled workflow claim did not retain an occurrence id.".to_string()
        })?;
        let occurrence_approval_state = state
            .db
            .workflow_automation_occurrence_approval_state(occurrence_id)
            .map_err(|error| error.to_string())?;
        if claim.due_run.automation.approval_policy.require_before_run
            && occurrence_approval_state
                != nexa_core::workflow_automation::WorkflowAutomationApprovalState::Approved
        {
            let requested = state
                .db
                .mark_workflow_automation_run_waiting_approval(&run_id)
                .map_err(|error| error.to_string())?;
            let waiting_run = state
                .db
                .get_workflow_automation_run(&run_id)
                .map_err(|error| error.to_string())?;
            if requested || waiting_run.status == WorkflowAutomationRunStatus::WaitingApproval {
                return Ok(ScheduledWorkflowLaunchOutcome::PendingApproval { run: waiting_run });
            }
            return Ok(ScheduledWorkflowLaunchOutcome::Skipped {
                reason: "approval_request_not_actionable".into(),
            });
        }
    }
    let launch_policy = match resolve_authoritative_workflow_launch_policy(
        state.db.as_ref(),
        &claim.due_run.automation,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                Some(&run_id),
                "skipped_no_agent_config",
                Some("blocked"),
                "Scheduler could not resolve the workflow's saved agent policy",
                serde_json::json!({ "error": error }),
            );
            if let Err(transition_error) = state
                .db
                .mark_workflow_automation_launch_failed_for_retry(&run_id, &error, now)
            {
                warn!("Failed to fence unresolved scheduled route {run_id}: {transition_error}");
            }
            return Err(error);
        }
    };
    let ticket = workflow_due_run_execution_ticket(&claim.due_run, claimed_run)
        .map_err(|error| error.to_string())?;
    let queue_id = ticket.delivery.queue_item.queue_id.clone();
    record_task_orchestrator_scheduler_event(
        state.db.as_ref(),
        Some(&automation_id),
        Some(&run_id),
        "claimed",
        Some(ticket.run.status.raw_status.as_str()),
        "Scheduler claimed due workflow",
        serde_json::json!({
            "queueId": queue_id.clone(),
            "dueReason": due_reason,
            "occurrenceId": claimed_run.occurrence_id,
            "scheduledFor": claimed_run.scheduled_for,
            "definitionRevision": claimed_run.definition_revision,
            "attempt": claimed_run.attempt,
        }),
    );

    match launch_task_orchestrator_execution_ticket(DesktopTaskOrchestratorLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        app_handle,
        ticket,
        launch_policy,
        conversation_id,
        persona_id: None,
        skill_ids: None,
        delivery_kind,
    })
    .await
    {
        Ok(launch) => {
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                Some(&run_id),
                "launch_succeeded",
                Some("running"),
                "Scheduler launched due workflow",
                serde_json::json!({
                    "queueId": queue_id,
                    "conversationId": launch.conversation_id,
                    "taskRunId": launch.task_run_id,
                }),
            );
            Ok(ScheduledWorkflowLaunchOutcome::Launched(launch))
        }
        Err(error) => {
            record_task_orchestrator_scheduler_event(
                state.db.as_ref(),
                Some(&automation_id),
                Some(&run_id),
                "launch_failed",
                Some("failed"),
                "Scheduler failed to launch due workflow",
                serde_json::json!({
                    "queueId": queue_id,
                    "error": error,
                }),
            );
            Err(error)
        }
    }
}

fn requires_preclaim_approval_skip(trigger_kind: &str, require_before_run: bool) -> bool {
    trigger_kind != "schedule" && require_before_run
}

#[tauri::command]
pub async fn save_workflow_automation_cmd(
    state: tauri::State<'_, AppState>,
    mut input: SaveWorkflowAutomationInput,
    schedule_config: Option<WorkflowAutomationScheduleConfig>,
) -> Result<WorkflowAutomation, String> {
    let mut schedule_config = schedule_config_for_save(state.db.as_ref(), &input, schedule_config)?;
    let workspace_target =
        resolve_scheduled_workspace_target(state.db.as_ref(), &input, &schedule_config)?;
    input.source_scope = workspace_target.source_scope;
    schedule_config.execution_policy.source_root_fingerprint =
        workspace_target.source_root_fingerprint;
    state
        .db
        .save_workflow_automation_with_schedule_config(&input, &schedule_config)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_workflow_automations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WorkflowAutomation>, String> {
    state
        .db
        .list_workflow_automations()
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn delete_workflow_automation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .delete_workflow_automation(&id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn set_workflow_automation_enabled_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<WorkflowAutomation, String> {
    let existing = state
        .db
        .get_workflow_automation(&id)
        .map_err(|err| err.to_string())?;
    state
        .db
        .save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: Some(existing.id),
                name: existing.name,
                description: existing.description,
                workflow_template_id: existing.workflow_template_id,
                prompt: existing.prompt,
                trigger: existing.trigger,
                source_scope: existing.source_scope,
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: existing.approval_policy.require_before_run,
                    allowed_tools: existing.approval_policy.allowed_tools,
                    risk_level: existing.approval_policy.risk_level,
                },
                enabled,
            },
            &existing.schedule_config,
        )
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn preview_workflow_automation_schedule_cmd(
    cron: String,
    timezone: String,
    after: Option<String>,
    limit: Option<usize>,
) -> Result<WorkflowSchedulePreview, String> {
    let after = after
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|error| format!("Invalid schedule preview timestamp: {error}"))?
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    preview_workflow_cron_schedule(&cron, &timezone, after, limit.unwrap_or(5))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_due_workflow_automations_cmd(
    state: tauri::State<'_, AppState>,
    now: Option<String>,
) -> Result<Vec<WorkflowAutomationDueRun>, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let due_runs = state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())?;
    filter_due_workflow_runs_by_package_host(state.db.as_ref(), due_runs)
}

#[tauri::command]
pub async fn list_due_task_orchestrator_queue_cmd(
    state: tauri::State<'_, AppState>,
    now: Option<String>,
) -> Result<Vec<TaskOrchestratorQueueItem>, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let due_runs = state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())?;
    let due_runs = filter_due_workflow_runs_by_package_host(state.db.as_ref(), due_runs)?;
    Ok(workflow_due_runs_to_queue_items(&due_runs))
}

#[tauri::command]
pub async fn preview_workflow_automation_prompt_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    state
        .db
        .preview_workflow_automation_prompt(&id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn prepare_workflow_automation_delivery_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<TaskOrchestratorDeliveryEnvelope, String> {
    let automation = state
        .db
        .get_workflow_automation(&id)
        .map_err(|err| err.to_string())?;
    ensure_workflow_template_runtime_visible(state.db.as_ref(), &automation.workflow_template_id)?;
    let prompt = state
        .db
        .preview_workflow_automation_prompt(&id)
        .map_err(|err| err.to_string())?;
    Ok(workflow_automation_delivery_envelope(
        &automation,
        prompt,
        "manual run requested",
    ))
}

#[tauri::command]
pub async fn prepare_due_workflow_automation_delivery_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    now: Option<String>,
) -> Result<TaskOrchestratorDeliveryEnvelope, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let due_runs = state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())?;
    let due_runs = filter_due_workflow_runs_by_package_host(state.db.as_ref(), due_runs)?;
    let due = due_runs
        .into_iter()
        .find(|due| due.automation.id == id)
        .ok_or_else(|| format!("Workflow automation '{id}' is not currently due."))?;
    Ok(workflow_due_run_delivery_envelope(&due))
}

#[tauri::command]
pub async fn queue_workflow_automation_delivery_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let automation = state
        .db
        .get_workflow_automation(&id)
        .map_err(|err| err.to_string())?;
    if automation.trigger_kind == "schedule" {
        return Err(
            "Scheduled definitions must use start_workflow_automation_run_cmd so the saved execution and approval policy is enforced."
                .to_string(),
        );
    }
    queue_manual_workflow_automation_execution_ticket(state.db.as_ref(), &automation, summary)
}

fn queue_manual_workflow_automation_execution_ticket(
    db: &Database,
    automation: &WorkflowAutomation,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    ensure_workflow_template_runtime_visible(db, &automation.workflow_template_id)?;
    let prompt = db
        .preview_workflow_automation_prompt(&automation.id)
        .map_err(|err| err.to_string())?;
    let delivery =
        workflow_automation_delivery_envelope(automation, prompt, "manual run requested");
    let run = db
        .record_workflow_automation_run(
            &automation.id,
            None,
            "queued",
            summary
                .as_deref()
                .or(Some(delivery.queue_item.due_reason.as_str())),
        )
        .map_err(|err| err.to_string())?;
    workflow_automation_execution_ticket(automation, &run, delivery).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn start_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    app_handle: AppHandle,
    id: String,
    conversation_id: Option<String>,
    summary: Option<String>,
) -> Result<TaskOrchestratorWorkflowStartOutcome, String> {
    let automation = state
        .db
        .get_workflow_automation(&id)
        .map_err(|error| error.to_string())?;
    if automation.trigger_kind == "schedule" {
        ensure_workflow_template_runtime_visible(
            state.db.as_ref(),
            &automation.workflow_template_id,
        )?;
        let now = Utc::now().to_rfc3339();
        let due = state
            .db
            .workflow_automation_run_now_due_at(&id, &now)
            .map_err(|error| error.to_string())?;
        return launch_authoritative_scheduled_workflow(
            AuthoritativeScheduledWorkflowLaunchRequest {
                state: state.inner(),
                agent_state: agent_state.inner(),
                mcp_state: mcp_state.inner(),
                approval_state: approval_state.inner(),
                app_handle,
                due,
                now: &now,
                conversation_id,
                summary,
                delivery_kind: "manual_run_now",
            },
        )
        .await
        .map(Into::into);
    }
    if automation.approval_policy.require_before_run {
        return Err("Workflow requires approval before it can run.".to_string());
    }
    let launch_policy = resolve_interactive_workflow_launch_policy(state.db.as_ref(), &automation)?;
    let ticket =
        queue_manual_workflow_automation_execution_ticket(state.db.as_ref(), &automation, summary)?;
    launch_task_orchestrator_execution_ticket(DesktopTaskOrchestratorLaunchRequest {
        state: state.inner(),
        agent_state: agent_state.inner(),
        mcp_state: mcp_state.inner(),
        approval_state: approval_state.inner(),
        app_handle,
        ticket,
        launch_policy,
        conversation_id,
        persona_id: None,
        skill_ids: None,
        delivery_kind: "manual",
    })
    .await
    .map(|launch| TaskOrchestratorWorkflowStartOutcome::Launched { launch })
}

#[tauri::command]
pub async fn queue_due_workflow_automation_delivery_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    now: Option<String>,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let due = find_due_workflow_automation(state.db.as_ref(), &id, &now)?;
    if due.automation.trigger_kind == "schedule" {
        return Err(
            "Scheduled occurrences must use start_due_workflow_automation_run_cmd so the saved execution and approval policy is enforced."
                .to_string(),
        );
    }
    claim_due_workflow_automation_execution_ticket(state.db.as_ref(), due, &now, summary)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_due_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    app_handle: AppHandle,
    id: String,
    now: Option<String>,
    conversation_id: Option<String>,
    summary: Option<String>,
) -> Result<TaskOrchestratorWorkflowStartOutcome, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let due = find_due_workflow_automation(state.db.as_ref(), &id, &now)?;
    launch_authoritative_scheduled_workflow(AuthoritativeScheduledWorkflowLaunchRequest {
        state: state.inner(),
        agent_state: agent_state.inner(),
        mcp_state: mcp_state.inner(),
        approval_state: approval_state.inner(),
        app_handle,
        due,
        now: &now,
        conversation_id,
        summary,
        delivery_kind: "manual_due",
    })
    .await
    .map(Into::into)
}

#[tauri::command]
pub async fn list_workflow_automation_approvals_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WorkflowAutomationRun>, String> {
    state
        .db
        .list_workflow_automation_runs_waiting_approval()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn approve_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    app_handle: AppHandle,
    run_id: String,
    conversation_id: Option<String>,
) -> Result<TaskOrchestratorWorkflowLaunch, String> {
    let now = Utc::now().to_rfc3339();
    let waiting_run = state
        .db
        .get_workflow_automation_run(&run_id)
        .map_err(|error| error.to_string())?;
    let waiting_automation = state
        .db
        .get_workflow_automation(&waiting_run.automation_id)
        .map_err(|error| error.to_string())?;
    ensure_workflow_template_runtime_visible(
        state.db.as_ref(),
        &waiting_automation.workflow_template_id,
    )?;
    let launch_policy =
        resolve_authoritative_workflow_launch_policy(state.db.as_ref(), &waiting_automation)?;
    let claim = state
        .db
        .approve_workflow_automation_run_at(&run_id, &now)
        .map_err(|error| error.to_string())?;
    let run = claim
        .run
        .as_ref()
        .expect("approved workflow claim must include its run");
    let ticket = workflow_due_run_execution_ticket(&claim.due_run, run)
        .map_err(|error| error.to_string())?;
    launch_task_orchestrator_execution_ticket(DesktopTaskOrchestratorLaunchRequest {
        state: state.inner(),
        agent_state: agent_state.inner(),
        mcp_state: mcp_state.inner(),
        approval_state: approval_state.inner(),
        app_handle,
        ticket,
        launch_policy,
        conversation_id,
        persona_id: None,
        skill_ids: None,
        delivery_kind: "approved_schedule",
    })
    .await
}

#[tauri::command]
pub async fn deny_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<WorkflowAutomationRun, String> {
    state
        .db
        .deny_workflow_automation_run_at(&run_id, &Utc::now().to_rfc3339())
        .map_err(|error| error.to_string())
}

pub fn init_task_orchestrator_scheduler(app_handle: AppHandle) {
    if app_handle
        .try_state::<TaskOrchestratorSchedulerState>()
        .is_some()
    {
        return;
    }

    let scheduler_state = TaskOrchestratorSchedulerState {
        shutdown: Arc::new(AtomicBool::new(false)),
        tick_lock: TokioMutex::new(()),
        poll_interval_secs: 60,
        max_concurrent_due_runs: 1,
    };
    let shutdown = Arc::clone(&scheduler_state.shutdown);
    let poll_interval_secs = scheduler_state.poll_interval_secs;
    app_handle.manage(scheduler_state);

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(poll_interval_secs.max(5)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            if let Err(err) = run_task_orchestrator_scheduler_tick(handle.clone()).await {
                warn!("Task Orchestrator scheduler tick failed: {err}");
            }
            interval.tick().await;
        }
    });
}

pub fn shutdown_task_orchestrator_scheduler(state: &TaskOrchestratorSchedulerState) {
    state.shutdown.store(true, Ordering::SeqCst);
}

pub async fn run_task_orchestrator_scheduler_tick(
    app_handle: AppHandle,
) -> Result<Vec<TaskOrchestratorWorkflowLaunch>, String> {
    let scheduler_state = app_handle
        .try_state::<TaskOrchestratorSchedulerState>()
        .ok_or_else(|| "Task Orchestrator scheduler state is not initialized.".to_string())?;
    if scheduler_state.shutdown.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let _tick_guard = match scheduler_state.tick_lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => return Ok(Vec::new()),
    };

    let app_state = app_handle
        .try_state::<AppState>()
        .ok_or_else(|| "App state is not initialized.".to_string())?;
    let agent_state = app_handle
        .try_state::<AgentState>()
        .ok_or_else(|| "Agent state is not initialized.".to_string())?;
    let mcp_state = app_handle
        .try_state::<McpManagerState>()
        .ok_or_else(|| "MCP manager state is not initialized.".to_string())?;
    let approval_state = app_handle
        .try_state::<ApprovalState>()
        .ok_or_else(|| "Approval state is not initialized.".to_string())?;

    let now = Utc::now().to_rfc3339();
    let visible_due_runs = app_state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())
        .and_then(|due_runs| {
            filter_due_workflow_runs_by_package_host(app_state.db.as_ref(), due_runs)
        })?;
    let mut launches = Vec::new();
    let launch_limit = scheduler_state.max_concurrent_due_runs.max(1);
    for due in visible_due_runs {
        if launches.len() >= launch_limit {
            break;
        }
        let automation_id = due.automation.id.clone();
        match launch_authoritative_scheduled_workflow(AuthoritativeScheduledWorkflowLaunchRequest {
            state: app_state.inner(),
            agent_state: agent_state.inner(),
            mcp_state: mcp_state.inner(),
            approval_state: approval_state.inner(),
            app_handle: app_handle.clone(),
            due,
            now: &now,
            conversation_id: None,
            summary: None,
            delivery_kind: "scheduler",
        })
        .await
        {
            Ok(outcome) => {
                if let Some(launch) = outcome.into_launch() {
                    launches.push(launch);
                }
            }
            Err(error) => {
                warn!("Scheduled workflow {automation_id} was not launched: {error}");
            }
        }
    }

    Ok(launches)
}

#[tauri::command]
pub async fn record_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    automation_id: String,
    task_run_id: Option<String>,
    status: String,
    summary: Option<String>,
) -> Result<WorkflowAutomationRun, String> {
    state
        .db
        .record_workflow_automation_run(
            &automation_id,
            task_run_id.as_deref(),
            &status,
            summary.as_deref(),
        )
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_workflow_automation_scheduler_events_cmd(
    state: tauri::State<'_, AppState>,
    automation_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<WorkflowAutomationSchedulerEvent>, String> {
    state
        .db
        .list_workflow_automation_scheduler_events(automation_id.as_deref(), limit.unwrap_or(100))
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_workflow_automation_scheduler_events_for_task_run_cmd(
    state: tauri::State<'_, AppState>,
    task_run_id: String,
    limit: Option<usize>,
) -> Result<Vec<WorkflowAutomationSchedulerEvent>, String> {
    state
        .db
        .list_workflow_automation_scheduler_events_for_task_run(&task_run_id, limit.unwrap_or(100))
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn export_workflow_automation_trajectory_cmd(
    state: tauri::State<'_, AppState>,
    workflow_run_id: String,
    redaction_profile: Option<nexa_core::trajectory::TrajectoryRedactionProfile>,
) -> Result<nexa_core::trajectory::Trajectory, String> {
    nexa_core::trajectory::export_workflow_automation_run_trajectory(
        state.db.as_ref(),
        &workflow_run_id,
        redaction_profile
            .unwrap_or(nexa_core::trajectory::TrajectoryRedactionProfile::FullLocalPrivate),
    )
    .map_err(|err| err.to_string())
}

#[cfg(test)]
mod scheduled_execution_policy_tests {
    use super::*;

    #[test]
    fn only_non_occurrence_triggers_skip_approval_before_claim() {
        assert!(requires_preclaim_approval_skip("folder", true));
        assert!(!requires_preclaim_approval_skip("folder", false));
        assert!(!requires_preclaim_approval_skip("schedule", true));
    }

    fn test_agent_config() -> DbAgentConfig {
        DbAgentConfig {
            id: "scheduled-agent".into(),
            name: "Scheduled agent".into(),
            provider: "alibaba_model_studio".into(),
            api_key: "test-key".into(),
            base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".into()),
            model: "qwen3.5-plus".into(),
            temperature: Some(0.2),
            max_tokens: None,
            context_window: Some(262_144),
            is_default: true,
            reasoning_enabled: Some(true),
            thinking_budget: None,
            reasoning_effort: Some("high".into()),
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            delegation_limits_v2: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
            provider_endpoint_id: Some("text:qwen-workspace-a".into()),
            model_id: Some("qwen3.5-plus".into()),
            model_selection_resolution: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn saved_provider_endpoint_drift_fails_closed() {
        let policy = WorkflowAutomationExecutionPolicy {
            provider: Some("alibaba_model_studio".into()),
            provider_endpoint_id: Some("text:qwen-workspace-a".into()),
            ..WorkflowAutomationExecutionPolicy::default()
        };
        assert!(validate_scheduled_execution_route(
            &policy,
            "alibaba_model_studio",
            Some("text:qwen-workspace-a")
        )
        .is_ok());
        assert!(validate_scheduled_execution_route(
            &policy,
            "alibaba_model_studio",
            Some("text:qwen-workspace-b")
        )
        .is_err());
        assert!(validate_scheduled_execution_route(
            &policy,
            "open_ai",
            Some("text:qwen-workspace-a")
        )
        .is_err());
    }

    #[test]
    fn scheduled_launch_policy_carries_every_saved_runtime_restriction() {
        let config = test_agent_config();
        let policy = WorkflowAutomationExecutionPolicy {
            project_id: Some("project-scheduled".into()),
            workspace_policy: WorkflowScheduleWorkspacePolicy::IsolatedPatch,
            source_root_fingerprint: Some("blake3:test".into()),
            agent_config_id: Some(config.id.clone()),
            provider: Some(config.provider.clone()),
            provider_endpoint_id: config.provider_endpoint_id.clone(),
            model: Some("qwen3.5-max".into()),
            context_window: None,
            power_mode: "nexus".into(),
            orchestration_profile: "researchUltra".into(),
            collaboration_mode: "delegated".into(),
            execution_mode: Some("plan".into()),
        };
        let approval = WorkflowAutomationApprovalPolicy {
            require_before_run: false,
            allowed_tools: vec!["read_file".into(), "web_search".into()],
            risk_level: "medium".into(),
        };

        let resolved = scheduled_workflow_launch_policy(config, &policy, &approval)
            .expect("resolve saved scheduled policy");

        assert_eq!(resolved.selected_config.model, "qwen3.5-max");
        assert_eq!(resolved.selected_config.context_window, None);
        assert_eq!(resolved.project_id.as_deref(), Some("project-scheduled"));
        assert!(resolved.force_workspace_isolation);
        assert_eq!(resolved.power_mode, "nexus");
        assert_eq!(resolved.orchestration_profile, "researchUltra");
        assert_eq!(resolved.collaboration_mode, "delegated");
        assert_eq!(resolved.execution_mode.as_deref(), Some("plan"));
        assert_eq!(
            resolved.allowed_tools,
            Some(vec!["read_file".into(), "web_search".into()])
        );
        assert_eq!(
            resolved.tool_approval_mode,
            nexa_core::approval::ToolApprovalMode::AllowAll
        );
        assert!(resolved.agent_config_is_authoritative);
    }

    #[test]
    fn interactive_workflows_keep_normal_approval_and_empty_allowlist_inheritance() {
        let config = test_agent_config();
        let execution = WorkflowAutomationExecutionPolicy::default();
        let empty = WorkflowAutomationApprovalPolicy::default();
        let inherited = interactive_workflow_launch_policy(config.clone(), &execution, &empty)
            .expect("resolve interactive workflow policy");
        assert_eq!(inherited.allowed_tools, None);
        assert_eq!(
            inherited.tool_approval_mode,
            nexa_core::approval::ToolApprovalMode::Ask
        );
        assert!(!inherited.force_workspace_isolation);
        assert!(!inherited.agent_config_is_authoritative);

        let mut writes = empty;
        writes.allowed_tools = vec!["create_file".into(), "run_shell".into()];
        let explicit = interactive_workflow_launch_policy(config, &execution, &writes)
            .expect("interactive writes remain subject to normal approval");
        assert_eq!(
            explicit.allowed_tools,
            Some(vec!["create_file".into(), "run_shell".into()])
        );
        assert_eq!(
            explicit.tool_approval_mode,
            nexa_core::approval::ToolApprovalMode::Ask
        );
    }

    #[test]
    fn scheduled_workspace_tools_snapshot_project_sources_and_fail_closed_without_scope() {
        let db = Database::open_memory().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "nexa-scheduled-workspace-policy-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let source = db
            .add_source(nexa_core::sources::CreateSourceInput {
                root_path: directory.to_string_lossy().into_owned(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: false,
            })
            .unwrap();
        let project = db
            .create_project(&nexa_core::project::CreateProjectInput {
                name: "Scheduled project".into(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: Some(vec![source.id.clone()]),
            })
            .unwrap();
        let input = SaveWorkflowAutomationInput {
            id: None,
            name: "Scoped schedule".into(),
            description: String::new(),
            workflow_template_id: "report_brief".into(),
            prompt: "Inspect the project.".into(),
            trigger: WorkflowAutomationTrigger::Schedule {
                cron: "0 9 * * *".into(),
            },
            source_scope: vec![source.id.clone()],
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: false,
                allowed_tools: vec!["run_shell".into()],
                risk_level: "high".into(),
            },
            enabled: true,
        };
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.project_id = Some(project.id);
        config.execution_policy.workspace_policy = WorkflowScheduleWorkspacePolicy::IsolatedPatch;
        config.execution_policy.orchestration_profile = "codeUltra".into();

        assert_eq!(
            resolve_scheduled_workspace_target(&db, &input, &config)
                .unwrap()
                .source_scope,
            vec![source.id]
        );
        config.execution_policy.workspace_policy = WorkflowScheduleWorkspacePolicy::DenyWrites;
        assert!(resolve_scheduled_workspace_target(&db, &input, &config)
            .unwrap_err()
            .contains("select isolated_patch"));
        std::fs::remove_dir(&directory).unwrap();
    }

    #[test]
    fn backend_snapshots_agent_route_identity_without_fabricating_context() {
        let selected = test_agent_config();
        let mut schedule = WorkflowAutomationScheduleConfig::default();
        schedule.execution_policy.agent_config_id = Some(selected.id.clone());

        snapshot_scheduled_agent_config(&mut schedule, &selected);

        assert_eq!(
            schedule.execution_policy.provider.as_deref(),
            Some("alibaba_model_studio")
        );
        assert_eq!(
            schedule.execution_policy.provider_endpoint_id.as_deref(),
            Some("text:qwen-workspace-a")
        );
        assert_eq!(
            schedule.execution_policy.model.as_deref(),
            Some("qwen3.5-plus")
        );
        assert_eq!(schedule.execution_policy.context_window, None);

        schedule.execution_policy.model = Some(" qwen3.5-max ".into());
        snapshot_scheduled_agent_config(&mut schedule, &selected);
        assert_eq!(
            schedule.execution_policy.model.as_deref(),
            Some("qwen3.5-max")
        );
    }

    #[test]
    fn save_normalization_loads_the_authoritative_agent_config_snapshot() {
        let db = Database::open_memory().expect("open memory database");
        let saved = db
            .save_agent_config(&SaveAgentConfigInput {
                id: Some("scheduled-save-agent".into()),
                name: "Scheduled save agent".into(),
                provider: "open_ai".into(),
                api_key: "test-key".into(),
                base_url: None,
                model: "gpt-5.4".into(),
                temperature: None,
                max_tokens: None,
                context_window: Some(200_000),
                is_default: true,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                image_generation_model: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                delegation_limits_v2: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
                provider_endpoint_id: None,
                model_id: None,
            })
            .expect("save agent config");
        let mut schedule = WorkflowAutomationScheduleConfig::default();
        schedule.execution_policy.agent_config_id = Some(saved.id.clone());
        schedule.execution_policy.provider = None;
        schedule.execution_policy.provider_endpoint_id = None;
        schedule.execution_policy.model = None;

        let normalized = normalize_scheduled_config_agent_snapshot(&db, schedule)
            .expect("normalize scheduled snapshot");

        assert_eq!(normalized.execution_policy.provider, Some(saved.provider));
        assert_eq!(
            normalized.execution_policy.provider_endpoint_id,
            saved.provider_endpoint_id
        );
        assert_eq!(normalized.execution_policy.model, Some(saved.model));
        assert_eq!(normalized.execution_policy.context_window, None);
    }

    #[test]
    fn omitted_schedule_config_preserves_only_safe_legacy_daily_utc_semantics() {
        let db = Database::open_memory().expect("open memory database");
        let input = |cron: &str| SaveWorkflowAutomationInput {
            id: None,
            name: "Legacy schedule".into(),
            description: String::new(),
            workflow_template_id: "report_brief".into(),
            prompt: "Run safely".into(),
            trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
            source_scope: Vec::new(),
            approval_policy: WorkflowAutomationApprovalPolicy::default(),
            enabled: true,
        };

        let safe = schedule_config_for_save(&db, &input("0 9 * * *"), None)
            .expect("safe legacy daily UTC schedule");
        assert_eq!(safe.version, 2);
        assert_eq!(safe.timezone, "UTC");
        assert!(!safe.legacy_needs_review);

        let unsafe_error = schedule_config_for_save(&db, &input("* * * * *"), None)
            .expect_err("legacy wildcard schedule must require review");
        assert!(unsafe_error.contains("requires review"));
    }

    #[test]
    fn skipped_outcome_does_not_claim_scheduler_launch_capacity() {
        assert!(ScheduledWorkflowLaunchOutcome::Skipped {
            reason: "retry_backoff".into(),
        }
        .into_launch()
        .is_none());
    }
}

#[tauri::command]
pub async fn list_task_resume_checkpoints_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<TaskResumeCheckpoint>, String> {
    state
        .db
        .list_task_resume_checkpoints(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_task_resume_prompt_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<TaskResumePrompt, String> {
    state
        .db
        .build_task_resume_prompt(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn pause_agent_task_run_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    approval_state: tauri::State<'_, ApprovalState>,
    run_id: String,
) -> Result<TaskResumeCheckpoint, String> {
    pause_agent_task_run(
        state.db.as_ref(),
        &state.run_event_outboxes,
        &agent_state.sessions,
        &approval_state.pending,
        &run_id,
    )
    .await
}

async fn pause_agent_task_run(
    db: &Database,
    run_event_outboxes: &AgentRunEventOutboxes,
    sessions: &nexa_core::runtime::AgentSessionManager,
    pending_approvals: &PendingToolApprovals,
    run_id: &str,
) -> Result<TaskResumeCheckpoint, String> {
    let _run_lifecycle_guard = sessions.acquire_run_lifecycle(run_id).await;
    let initial_run = db
        .get_agent_task_run(&run_id)
        .map_err(|err| err.to_string())?;
    if !matches!(
        initial_run.status.as_str(),
        "queued" | "running" | "waiting_approval"
    ) {
        return Err(format!(
            "Agent task run {run_id} cannot be paused from status '{}'",
            initial_run.status
        ));
    }

    let mut event_outbox = None;
    let mut event_turn_id = initial_run.turn_id.clone();
    if let Some(task_state) = sessions
        .take_for_run(&initial_run.conversation_id, &run_id)
        .await
        .map_err(|error| error.to_string())?
    {
        event_turn_id = task_state.handle.turn_id.clone();
        event_outbox = Some(Arc::clone(&task_state.event_outbox));
        // Stop the old producer before deciding whether this is still a
        // checkpoint-pausable run. Awaiting the aborted owner closes the race
        // where it could otherwise establish a user-input barrier after our
        // first status read.
        task_state.task.abort();
        task_state.cancel_token.cancel();
        let _ = task_state.task.await;
    }

    let event_outbox = match event_outbox {
        Some(outbox) => outbox,
        None => run_event_outboxes
            .open(&initial_run.conversation_id, &run_id)
            .await
            .map_err(|error| error.to_string())?,
    };
    // Resolve approvals and drain everything accepted before the producer
    // stopped, then make the pause decision from the durable state.
    resolve_desktop_pending_approvals_for_stopped_run(
        db,
        event_outbox.as_ref(),
        run_id,
        &event_turn_id,
        pending_approvals,
    )
    .await
    .map_err(|error| error.to_string())?;
    let run = db
        .get_agent_task_run(&run_id)
        .map_err(|err| err.to_string())?;
    let has_unresolved_interactions = db
        .agent_run_has_unresolved_interactions(&run_id)
        .map_err(|err| err.to_string())?;
    if run.status == "awaiting_user_input" || has_unresolved_interactions {
        return Err(format!(
            "Agent task run {run_id} is waiting for required user input and cannot be checkpoint-paused"
        ));
    }
    if !matches!(
        run.status.as_str(),
        "queued" | "running" | "waiting_approval"
    ) {
        return Err(format!(
            "Agent task run {run_id} cannot be paused from status '{}'",
            run.status
        ));
    }

    event_outbox
        .pause_with_checkpoint(&event_turn_id, "user_pause")
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod pause_tests {
    use super::*;
    use nexa_core::agent::CancellationToken;
    use nexa_core::conversation::{AgentTaskRun, ConversationMessage, CreateConversationInput};
    use nexa_core::interaction::{
        CreateInteractionRequest, InteractionKind, InteractionQuestion, InteractionQuestionKind,
    };
    use nexa_core::llm::Role;
    use nexa_core::run_event_outbox::AgentRunEventDelivery;
    use nexa_core::runtime::{ActiveAgentTurn, AgentTurnHandle};
    use std::sync::atomic::AtomicBool;

    struct NoopDelivery;

    impl AgentRunEventDelivery for NoopDelivery {
        fn deliver_run_event(&self, _conversation_id: &str, _event: &AgentRunEvent) {}

        fn deliver_task_run_snapshot(&self, _conversation_id: &str, _snapshot: AgentTaskRun) {}
    }

    struct SuspendForInteractionOnDrop {
        db: Database,
        request: Option<CreateInteractionRequest>,
    }

    impl Drop for SuspendForInteractionOnDrop {
        fn drop(&mut self) {
            let request = self.request.take().expect("interaction request");
            let created = self
                .db
                .create_interaction_request(&request)
                .expect("create interaction while producer stops");
            self.db
                .suspend_agent_turn_for_interaction(&created.request.interaction_id)
                .expect("suspend run for interaction while producer stops");
        }
    }

    #[tokio::test]
    async fn pause_command_commits_checkpoint_event_and_both_projections() {
        let db = Database::open_memory().expect("open memory database");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Pause this task".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).expect("add user message");
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("create turn");
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &message.id,
                "Pause this task",
                Some("test"),
                Some("test-model"),
            )
            .expect("create run");
        db.mark_agent_task_run_started(&run.id, "responding")
            .expect("start run");
        let executor = DatabaseExecutor::new(db.clone(), 8).expect("database executor");
        let outboxes = AgentRunEventOutboxes::new(executor, Arc::new(NoopDelivery));
        let sessions = nexa_core::runtime::AgentSessionManager::new();

        let pending_approvals = Arc::new(TokioMutex::new(HashMap::new()));
        let checkpoint =
            pause_agent_task_run(&db, &outboxes, &sessions, &pending_approvals, &run.id)
                .await
                .expect("pause task run");

        assert_eq!(
            db.get_agent_task_run(&run.id).expect("paused task").status,
            "paused"
        );
        let paused_turn = db.get_conversation_turn(&turn.id).expect("paused turn");
        assert_eq!(paused_turn.status, "paused");
        assert!(paused_turn.finished_at.is_none());
        assert_eq!(
            db.list_task_resume_checkpoints(&run.id)
                .expect("checkpoint ledger")
                .len(),
            1
        );
        let events = db.list_agent_run_events(&run.id).expect("run event ledger");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].phase, AgentRunPhase::Paused);
        assert_eq!(events[0].payload["checkpointId"], checkpoint.id);
    }

    #[tokio::test]
    async fn producer_entering_user_input_during_pause_cannot_create_a_pause_checkpoint() {
        let db = Database::open_memory().expect("open memory database");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Ask before continuing".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).expect("add user message");
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("create turn");
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &message.id,
                "Ask before continuing",
                Some("test"),
                Some("test-model"),
            )
            .expect("create run");
        db.mark_agent_task_run_started(&run.id, "responding")
            .expect("start run");

        let executor = DatabaseExecutor::new(db.clone(), 8).expect("database executor");
        let outboxes = AgentRunEventOutboxes::new(executor, Arc::new(NoopDelivery));
        let outbox = outboxes
            .open(&conversation.id, &run.id)
            .await
            .expect("open run event outbox");
        let sessions = nexa_core::runtime::AgentSessionManager::new();
        let cancellation = CancellationToken::new();
        let (steering_tx, _steering_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let producer_db = db.clone();
        let producer_conversation_id = conversation.id.clone();
        let producer_turn_id = turn.id.clone();
        let producer = tokio::spawn(async move {
            let _suspend_on_drop = SuspendForInteractionOnDrop {
                db: producer_db,
                request: Some(CreateInteractionRequest {
                    conversation_id: producer_conversation_id,
                    turn_id: producer_turn_id,
                    tool_call_id: Some("call-pause-race".to_string()),
                    idempotency_key: "pause-race-interaction".to_string(),
                    kind: InteractionKind::UserInput,
                    title: "Input required".to_string(),
                    description: None,
                    questions: vec![InteractionQuestion {
                        id: "scope".to_string(),
                        header: "Scope".to_string(),
                        question: "Which scope should continue?".to_string(),
                        kind: InteractionQuestionKind::Short,
                        options: Vec::new(),
                        placeholder: None,
                        why: None,
                    }],
                    required: true,
                    expires_at: None,
                }),
            };
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        });
        ready_rx.await.expect("producer reached pause race");
        sessions
            .register(ActiveAgentTurn {
                handle: AgentTurnHandle::running(
                    conversation.id.clone(),
                    run.id.clone(),
                    turn.id.clone(),
                ),
                cancel_token: cancellation,
                task: producer,
                steering_tx,
                event_outbox: Arc::clone(&outbox),
                orchestrator_run_id: None,
                frontend_paint_recorded: AtomicBool::new(false),
            })
            .await;

        let pending_approvals = Arc::new(TokioMutex::new(HashMap::new()));
        let error = pause_agent_task_run(&db, &outboxes, &sessions, &pending_approvals, &run.id)
            .await
            .expect_err("user-input barrier must win the pause race");

        assert!(error.contains("waiting for required user input"));
        assert_eq!(
            db.get_agent_task_run(&run.id)
                .expect("load task run")
                .status,
            "awaiting_user_input"
        );
        assert!(db
            .agent_run_has_unresolved_interactions(&run.id)
            .expect("check interaction barrier"));
        assert!(db
            .list_task_resume_checkpoints(&run.id)
            .expect("list resume checkpoints")
            .is_empty());
        assert!(!db
            .list_agent_run_events(&run.id)
            .expect("list run events")
            .iter()
            .any(|event| {
                event.phase == AgentRunPhase::Paused || event.status.as_deref() == Some("paused")
            }));
    }
}

#[tauri::command]
pub async fn get_investigation_graph_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<InvestigationGraph, String> {
    state
        .db
        .build_investigation_graph(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_learning_governance_snapshot_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<LearningGovernanceSnapshot, String> {
    state
        .db
        .learning_governance_snapshot()
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn capture_browser_evidence_cmd(
    state: tauri::State<'_, AppState>,
    url: String,
    max_length: Option<usize>,
    mode: Option<String>,
) -> Result<BrowserEvidenceCapture, String> {
    let args = serde_json::json!({
        "url": url,
        "max_length": max_length.unwrap_or(6000),
        "mode": mode.unwrap_or_else(|| "auto".to_string()),
    })
    .to_string();
    let tool = BrowserEvidenceCaptureTool;
    let result = tool
        .execute(nexa_core::tools::ToolExecutionContext::new(
            "manual-browser-evidence-capture",
            &args,
            &state.db,
            &[],
        ))
        .await
        .map_err(|err| err.to_string())?;
    if result.is_error {
        return Err(result.content);
    }
    let output = result.output_channels();
    output
        .data
        .and_then(|value| serde_json::from_value::<BrowserEvidenceCapture>(value).ok())
        .ok_or_else(|| "Browser evidence capture did not return structured data.".to_string())
}
