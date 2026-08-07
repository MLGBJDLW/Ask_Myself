use super::conversation::desktop_package_host_snapshot;
use super::*;
use nexa_core::package_host::PackageSurfaceKind;
use nexa_core::task_orchestrator::{
    workflow_automation_delivery_envelope, workflow_automation_execution_ticket,
    workflow_due_run_delivery_envelope, workflow_due_run_execution_ticket,
    workflow_due_run_queue_item, TaskOrchestratorDeliveryEnvelope, TaskOrchestratorExecutionTicket,
    TaskOrchestratorQueueItem,
};
use nexa_core::tools::{browser_evidence_tool::BrowserEvidenceCaptureTool, Tool};
use nexa_core::workflow_automation::{
    BrowserEvidenceCapture, InvestigationGraph, LearningGovernanceSnapshot,
    SaveWorkflowAutomationInput, TaskResumeCheckpoint, TaskResumePrompt, WorkflowAutomation,
    WorkflowAutomationApprovalPolicy, WorkflowAutomationDueRun, WorkflowAutomationRun,
    WorkflowAutomationSchedulerEvent, WorkflowAutomationSchedulerRetryDecision,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorWorkflowLaunch {
    pub ticket: TaskOrchestratorExecutionTicket,
    pub conversation_id: String,
    pub task_run_id: String,
    pub task_orchestrator_run_id: String,
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
    selected_config: DbAgentConfig,
    conversation_id: Option<String>,
    persona_id: Option<String>,
    skill_ids: Option<Vec<String>>,
    execution_mode: Option<String>,
    delivery_kind: &'static str,
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

pub(super) fn queue_due_workflow_automation_execution_ticket(
    db: &Database,
    id: &str,
    now: &str,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let due_runs = db
        .list_due_workflow_automations(now)
        .map_err(|err| err.to_string())?;
    let due_runs = filter_due_workflow_runs_by_package_host(db, due_runs)?;
    let due = due_runs
        .into_iter()
        .find(|due| due.automation.id == id)
        .ok_or_else(|| format!("Workflow automation '{id}' is not currently due."))?;
    let claim_summary = summary.unwrap_or_else(|| due.due_reason.clone());
    let claim = db
        .claim_workflow_automation_due_run(due, Some(claim_summary.as_str()))
        .map_err(|err| err.to_string())?;
    workflow_due_run_execution_ticket(&claim.due_run, &claim.run).map_err(|err| err.to_string())
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
        selected_config,
        conversation_id,
        persona_id,
        skill_ids,
        execution_mode,
        delivery_kind,
    } = request;
    let conversation_id = match conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => {
            state
                .db
                .get_conversation(id)
                .map_err(|err| err.to_string())?;
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
                    project_id: None,
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
        persona_id,
        skill_ids,
        execution_mode,
        power_mode: Some("standard".to_string()),
        collaboration_mode: Some("direct".to_string()),
        moa_preset: Some("fastReview".to_string()),
        orchestration_profile: Some("balanced".to_string()),
        custom_orchestration: None,
        vision_turn_override: None,
        user_artifacts: Some(serde_json::json!({
            "kind": "taskOrchestratorLaunch",
            "version": 1,
            "delivery": delivery_kind,
            "queueId": queue_id,
            "workflowRunId": workflow_run_id,
        })),
        task_orchestrator_run_id: Some(workflow_run_id.clone()),
        idempotency_key: format!("workflow:{queue_id}"),
    })
    .await;

    let launch = match launch_result {
        Ok(launch) => launch,
        Err(err) => {
            if let Err(transition_err) = state.db.transition_workflow_automation_run(
                &workflow_run_id,
                "cancelled",
                Some("Task Orchestrator launch failed before agent start"),
            ) {
                warn!(
                    "Failed to mark Task Orchestrator run {workflow_run_id} cancelled after launch failure: {transition_err}"
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

#[tauri::command]
pub async fn save_workflow_automation_cmd(
    state: tauri::State<'_, AppState>,
    input: SaveWorkflowAutomationInput,
) -> Result<WorkflowAutomation, String> {
    state
        .db
        .save_workflow_automation(&input)
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
        .save_workflow_automation(&SaveWorkflowAutomationInput {
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
        })
        .map_err(|err| err.to_string())
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
    ensure_workflow_template_runtime_visible(state.db.as_ref(), &automation.workflow_template_id)?;
    let prompt = state
        .db
        .preview_workflow_automation_prompt(&id)
        .map_err(|err| err.to_string())?;
    let delivery =
        workflow_automation_delivery_envelope(&automation, prompt, "manual run requested");
    let run = state
        .db
        .record_workflow_automation_run(
            &automation.id,
            None,
            "queued",
            summary
                .as_deref()
                .or(Some(delivery.queue_item.due_reason.as_str())),
        )
        .map_err(|err| err.to_string())?;
    workflow_automation_execution_ticket(&automation, &run, delivery).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn queue_due_workflow_automation_delivery_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    now: Option<String>,
    summary: Option<String>,
) -> Result<TaskOrchestratorExecutionTicket, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    queue_due_workflow_automation_execution_ticket(state.db.as_ref(), &id, &now, summary)
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
    agent_config_id: Option<String>,
    persona_id: Option<String>,
    skill_ids: Option<Vec<String>>,
    execution_mode: Option<String>,
    summary: Option<String>,
) -> Result<TaskOrchestratorWorkflowLaunch, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    let selected_config = select_task_orchestrator_launch_agent_config(
        state.db.as_ref(),
        agent_config_id.as_deref(),
    )?;
    let ticket =
        queue_due_workflow_automation_execution_ticket(state.db.as_ref(), &id, &now, summary)?;
    launch_task_orchestrator_execution_ticket(DesktopTaskOrchestratorLaunchRequest {
        state: state.inner(),
        agent_state: agent_state.inner(),
        mcp_state: mcp_state.inner(),
        approval_state: approval_state.inner(),
        app_handle,
        ticket,
        selected_config,
        conversation_id,
        persona_id,
        skill_ids,
        execution_mode,
        delivery_kind: "direct",
    })
    .await
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

    let selected_config =
        match select_task_orchestrator_launch_agent_config(app_state.db.as_ref(), None) {
            Ok(config) => config,
            Err(err) => {
                warn!("Task Orchestrator scheduler skipped due runs: {err}");
                record_task_orchestrator_scheduler_event(
                    app_state.db.as_ref(),
                    None,
                    None,
                    "skipped_no_agent_config",
                    Some("blocked"),
                    "Scheduler skipped due workflows because no agent config is available",
                    serde_json::json!({ "error": err }),
                );
                return Ok(Vec::new());
            }
        };

    let now = Utc::now().to_rfc3339();
    let visible_due_runs = app_state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())
        .and_then(|due_runs| {
            filter_due_workflow_runs_by_package_host(app_state.db.as_ref(), due_runs)
        })?;
    let mut due_runs = Vec::new();
    for due in visible_due_runs {
        let automation_id = due.automation.id.clone();
        if due.automation.approval_policy.require_before_run {
            let due_reason = due.due_reason.clone();
            let trigger_kind = due.automation.trigger_kind.clone();
            let workflow_template_id = due.automation.workflow_template_id.clone();
            let risk_level = due.automation.approval_policy.risk_level.clone();
            record_task_orchestrator_scheduler_event(
                app_state.db.as_ref(),
                Some(&automation_id),
                None,
                "skipped_pre_run_approval",
                Some("waiting_approval"),
                "Scheduler skipped due workflow because pre-run approval is required",
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": trigger_kind,
                    "workflowTemplateId": workflow_template_id,
                    "riskLevel": risk_level,
                }),
            );
            continue;
        }
        if task_orchestrator_scheduler_status_is_active(&due.automation.status) {
            let due_reason = due.due_reason.clone();
            let trigger_kind = due.automation.trigger_kind.clone();
            let workflow_template_id = due.automation.workflow_template_id.clone();
            let status = due.automation.status.clone();
            record_task_orchestrator_scheduler_event(
                app_state.db.as_ref(),
                Some(&automation_id),
                None,
                "skipped_active",
                Some(status.as_str()),
                "Scheduler skipped due workflow because it is already active",
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": trigger_kind,
                    "workflowTemplateId": workflow_template_id,
                    "status": status,
                }),
            );
            continue;
        }
        let retry_decision = app_state
            .db
            .workflow_automation_scheduler_retry_decision(&automation_id, &now)
            .map_err(|err| err.to_string())?;
        if !retry_decision.allowed {
            let due_reason = due.due_reason.clone();
            let trigger_kind = due.automation.trigger_kind.clone();
            let workflow_template_id = due.automation.workflow_template_id.clone();
            let (event_type, status, summary) =
                task_orchestrator_scheduler_retry_skip_event(&retry_decision);
            record_task_orchestrator_scheduler_event(
                app_state.db.as_ref(),
                Some(&automation_id),
                None,
                event_type,
                Some(status),
                summary,
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": trigger_kind,
                    "workflowTemplateId": workflow_template_id,
                    "retryDecision": retry_decision,
                }),
            );
            continue;
        }
        due_runs.push(due);
    }
    let mut launches = Vec::new();
    for due in due_runs
        .into_iter()
        .take(scheduler_state.max_concurrent_due_runs.max(1))
    {
        let automation_id = due.automation.id.clone();
        let due_reason = due.due_reason.clone();
        let summary = Some(format!("scheduler: {}", due.due_reason));
        let ticket = match queue_due_workflow_automation_execution_ticket(
            app_state.db.as_ref(),
            &automation_id,
            &now,
            summary,
        ) {
            Ok(ticket) => ticket,
            Err(err) => {
                warn!(
                    "Task Orchestrator scheduler could not claim due workflow {automation_id}: {err}"
                );
                record_task_orchestrator_scheduler_event(
                    app_state.db.as_ref(),
                    Some(&automation_id),
                    None,
                    "claim_failed",
                    Some("failed"),
                    "Scheduler failed to claim due workflow",
                    serde_json::json!({
                        "dueReason": due_reason,
                        "error": err,
                    }),
                );
                continue;
            }
        };
        let run_id = ticket.run.run_id.clone();
        let queue_id = ticket.delivery.queue_item.queue_id.clone();
        record_task_orchestrator_scheduler_event(
            app_state.db.as_ref(),
            Some(&automation_id),
            Some(&run_id),
            "claimed",
            Some(ticket.run.status.raw_status.as_str()),
            "Scheduler claimed due workflow",
            serde_json::json!({
                "queueId": queue_id.clone(),
                "dueReason": due_reason,
            }),
        );
        match launch_task_orchestrator_execution_ticket(DesktopTaskOrchestratorLaunchRequest {
            state: app_state.inner(),
            agent_state: agent_state.inner(),
            mcp_state: mcp_state.inner(),
            approval_state: approval_state.inner(),
            app_handle: app_handle.clone(),
            ticket,
            selected_config: selected_config.clone(),
            conversation_id: None,
            persona_id: None,
            skill_ids: None,
            execution_mode: None,
            delivery_kind: "scheduler",
        })
        .await
        {
            Ok(launch) => {
                record_task_orchestrator_scheduler_event(
                    app_state.db.as_ref(),
                    Some(&automation_id),
                    Some(&run_id),
                    "launch_succeeded",
                    Some("running"),
                    "Scheduler launched due workflow",
                    serde_json::json!({
                        "queueId": queue_id,
                        "conversationId": launch.conversation_id.clone(),
                        "taskRunId": launch.task_run_id.clone(),
                    }),
                );
                launches.push(launch);
            }
            Err(err) => {
                warn!(
                    "Task Orchestrator scheduler failed to launch due workflow {automation_id}: {err}"
                );
                record_task_orchestrator_scheduler_event(
                    app_state.db.as_ref(),
                    Some(&automation_id),
                    Some(&run_id),
                    "launch_failed",
                    Some("failed"),
                    "Scheduler failed to launch due workflow",
                    serde_json::json!({
                        "queueId": queue_id,
                        "error": err,
                    }),
                );
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
    app_handle: AppHandle,
    run_id: String,
) -> Result<TaskResumeCheckpoint, String> {
    let run = state
        .db
        .get_agent_task_run(&run_id)
        .map_err(|err| err.to_string())?;
    let checkpoint = state
        .db
        .pause_task_run_with_checkpoint(&run_id, "user_pause")
        .map_err(|err| err.to_string())?;

    if let Some(task_state) = agent_state.sessions.take(&run.conversation_id).await {
        if task_state.handle.run_id == run_id {
            let stream_event_seq = Arc::clone(&task_state.event_sequencer);
            let run_event = emit_agent_frontend_event_with_presentation(
                &app_handle,
                stream_event_seq.as_ref(),
                &run.conversation_id,
                &run_id,
                Some(&task_state.handle.turn_id),
                AgentEvent::Status {
                    content: "Pause checkpoint saved".to_string(),
                    tone: Some("muted".to_string()),
                },
                AgentRunEventVisibility::Internal,
                AgentRunDisplayKind::Status,
                AgentRunEventImportance::Low,
            );
            persist_durable_run_event(&state.db, &run_event);
            emit_agent_task_run_update(&state.db, &app_handle, &run.conversation_id, &run_id);
            task_state.cancel_token.cancel();
            let abort_task = task_state.task;
            let db = state.db.clone();
            let handle = app_handle.clone();
            let conv_id = run.conversation_id.clone();
            let turn_id = task_state.handle.turn_id.clone();
            let checkpoint_id = checkpoint.id.clone();
            let resume_prompt = checkpoint.resume_prompt.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                if !abort_task.is_finished() {
                    abort_task.abort();
                    let artifacts = serde_json::json!({
                        "kind": "resumeCheckpoint",
                        "checkpointId": checkpoint_id,
                        "resumePrompt": resume_prompt,
                    });
                    let _ = db.finish_agent_task_run(
                        &run_id,
                        "paused",
                        Some("Paused with a resumable checkpoint"),
                        None,
                        Some(&artifacts),
                    );
                    let run_event = AgentRunEvent::terminal_status(
                        &run_id,
                        Some(&turn_id),
                        stream_event_seq.next(),
                        "Paused with a resumable checkpoint",
                        "paused",
                        Some(&artifacts),
                    );
                    emit_agent_run_frontend_event(&handle, &conv_id, &run_event);
                    persist_durable_run_event(&db, &run_event);
                    emit_agent_task_run_update(&db, &handle, &conv_id, &run_id);
                }
            });
        } else {
            agent_state.sessions.register(task_state).await;
        }
    }

    Ok(checkpoint)
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
