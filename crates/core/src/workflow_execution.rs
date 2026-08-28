//! Core-owned policy for saving and launching unattended workflows.
//!
//! Desktop adapters provide runtime-only launch capabilities after this module
//! has resolved the durable occurrence, approval, workspace, and agent policy.

use tracing::warn;

use crate::approval::ToolApprovalMode;
use crate::conversation::AgentConfig as DbAgentConfig;
use crate::db::Database;
use crate::error::CoreError;
use crate::task_orchestrator::{
    workflow_due_run_execution_ticket, TaskOrchestratorExecutionTicket,
};
use crate::tools::capability::{
    scheduled_workspace_tool_class, tool_delegates_to_subagent, ScheduledWorkspaceToolClass,
};
use crate::workflow_automation::{
    SaveWorkflowAutomationInput, WorkflowAutomation, WorkflowAutomationApprovalPolicy,
    WorkflowAutomationApprovalState, WorkflowAutomationDueRun, WorkflowAutomationRun,
    WorkflowAutomationRunStatus, WorkflowAutomationSchedulerRetryDecision,
    WorkflowAutomationTrigger, WorkflowSchedulerEventType,
};
use crate::workflow_scheduler::{
    legacy_workflow_schedule_config, WorkflowAutomationExecutionPolicy,
    WorkflowAutomationScheduleConfig, WorkflowScheduleWorkspacePolicy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowLaunchMode {
    Interactive,
    AuthoritativeScheduled,
}

#[derive(Debug, Clone)]
pub struct WorkflowLaunchPolicy {
    pub selected_config: DbAgentConfig,
    pub project_id: Option<String>,
    pub force_workspace_isolation: bool,
    pub source_root_fingerprint: Option<String>,
    pub execution_mode: Option<String>,
    pub power_mode: String,
    pub collaboration_mode: String,
    pub orchestration_profile: String,
    pub allowed_tools: Option<Vec<String>>,
    pub tool_approval_mode: ToolApprovalMode,
    pub agent_config_is_authoritative: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedWorkflowAutomationSave {
    pub input: SaveWorkflowAutomationInput,
    pub schedule_config: WorkflowAutomationScheduleConfig,
}

#[derive(Debug)]
pub enum ScheduledWorkflowLaunchPreparation {
    Ready {
        ticket: TaskOrchestratorExecutionTicket,
        policy: WorkflowLaunchPolicy,
    },
    PendingApproval {
        run: WorkflowAutomationRun,
    },
    Skipped {
        reason: String,
    },
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidInput(message.into())
}

pub fn select_workflow_launch_agent_config(
    db: &Database,
    requested_config_id: Option<&str>,
) -> Result<DbAgentConfig, CoreError> {
    let configs = db.list_agent_configs()?;
    if let Some(id) = requested_config_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return configs
            .into_iter()
            .find(|config| config.id == id)
            .ok_or_else(|| invalid(format!("Requested agent config '{id}' was not found.")));
    }

    configs
        .iter()
        .find(|config| config.is_default)
        .cloned()
        .or_else(|| configs.first().cloned())
        .ok_or_else(|| invalid("No agent config set. Please configure an LLM provider first."))
}

fn validate_scheduled_execution_route(
    policy: &WorkflowAutomationExecutionPolicy,
    provider: &str,
    provider_endpoint_id: Option<&str>,
) -> Result<(), CoreError> {
    if let Some(expected_provider) = policy
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider != expected_provider {
            return Err(invalid(format!(
                "Scheduled workflow provider drift: saved '{expected_provider}', agent config now uses '{provider}'"
            )));
        }
    }
    if let Some(expected_endpoint_id) = policy
        .provider_endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider_endpoint_id != Some(expected_endpoint_id) {
            return Err(invalid(format!(
                "Scheduled workflow endpoint drift: saved '{expected_endpoint_id}', agent config now uses '{}'",
                provider_endpoint_id.unwrap_or("unknown")
            )));
        }
    }
    Ok(())
}

pub fn apply_scheduled_execution_policy(
    mut config: DbAgentConfig,
    policy: &WorkflowAutomationExecutionPolicy,
) -> Result<DbAgentConfig, CoreError> {
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

fn build_launch_policy(
    config: DbAgentConfig,
    execution_policy: &WorkflowAutomationExecutionPolicy,
    approval_policy: &WorkflowAutomationApprovalPolicy,
    mode: WorkflowLaunchMode,
) -> Result<WorkflowLaunchPolicy, CoreError> {
    let selected_config = apply_scheduled_execution_policy(config, execution_policy)?;
    let authoritative = mode == WorkflowLaunchMode::AuthoritativeScheduled;
    Ok(WorkflowLaunchPolicy {
        selected_config,
        project_id: execution_policy.project_id.clone(),
        force_workspace_isolation: authoritative
            && execution_policy.workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch,
        source_root_fingerprint: authoritative
            .then(|| execution_policy.source_root_fingerprint.clone())
            .flatten(),
        execution_mode: execution_policy.execution_mode.clone(),
        power_mode: execution_policy.power_mode.clone(),
        collaboration_mode: execution_policy.collaboration_mode.clone(),
        orchestration_profile: execution_policy.orchestration_profile.clone(),
        allowed_tools: if authoritative {
            Some(approval_policy.allowed_tools.clone())
        } else {
            (!approval_policy.allowed_tools.is_empty())
                .then(|| approval_policy.allowed_tools.clone())
        },
        tool_approval_mode: if authoritative {
            ToolApprovalMode::AllowAll
        } else {
            ToolApprovalMode::Ask
        },
        agent_config_is_authoritative: authoritative,
    })
}

pub fn resolve_workflow_launch_policy(
    db: &Database,
    automation: &WorkflowAutomation,
    mode: WorkflowLaunchMode,
) -> Result<WorkflowLaunchPolicy, CoreError> {
    let execution_policy = &automation.schedule_config.execution_policy;
    if let Some(project_id) = execution_policy.project_id.as_deref() {
        let project = db.get_project(project_id).map_err(|error| {
            invalid(format!(
                "Scheduled workflow project '{project_id}' is unavailable: {error}"
            ))
        })?;
        if project.archived {
            return Err(invalid(format!(
                "Scheduled workflow project '{}' is archived.",
                project.name
            )));
        }
        if mode == WorkflowLaunchMode::AuthoritativeScheduled {
            let project_sources = project.source_scope.unwrap_or_default();
            if automation
                .source_scope
                .iter()
                .any(|source_id| !project_sources.iter().any(|allowed| allowed == source_id))
            {
                return Err(invalid(
                    "Scheduled workflow source scope drifted outside its saved project boundary.",
                ));
            }
        }
    }
    if mode == WorkflowLaunchMode::AuthoritativeScheduled {
        validate_scheduled_workspace_target(db, automation)?;
    }
    let selected_config =
        select_workflow_launch_agent_config(db, execution_policy.agent_config_id.as_deref())?;
    build_launch_policy(
        selected_config,
        execution_policy,
        &automation.approval_policy,
        mode,
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
    execution_policy.model = execution_policy
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| Some(selected_config.model.clone()));
}

fn normalize_scheduled_config_agent_snapshot(
    db: &Database,
    mut schedule_config: WorkflowAutomationScheduleConfig,
) -> Result<WorkflowAutomationScheduleConfig, CoreError> {
    if let Some(project_id) = schedule_config
        .execution_policy
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let project = db.get_project(project_id).map_err(|error| {
            invalid(format!(
                "Scheduled workflow project '{project_id}' is unavailable: {error}"
            ))
        })?;
        if project.archived {
            return Err(invalid(format!(
                "Scheduled workflow project '{}' is archived.",
                project.name
            )));
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
        let selected_config = select_workflow_launch_agent_config(db, Some(&config_id))?;
        snapshot_scheduled_agent_config(&mut schedule_config, &selected_config);
    }
    Ok(schedule_config)
}

fn scheduled_source_root_fingerprint(db: &Database, source_id: &str) -> Result<String, CoreError> {
    let source = db.get_source(source_id).map_err(|error| {
        invalid(format!(
            "Scheduled workflow source '{source_id}' is unavailable: {error}"
        ))
    })?;
    let canonical = std::fs::canonicalize(&source.root_path).map_err(|error| {
        invalid(format!(
            "Scheduled workflow source '{}' cannot be canonicalized: {error}",
            source.root_path
        ))
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
) -> Result<(), CoreError> {
    let mut has_isolatable_write = false;
    for tool in allowed_tools {
        let normalized = tool.trim();
        if tool_delegates_to_subagent(normalized)
            && workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch
        {
            return Err(invalid(format!(
                "Scheduled tool '{normalized}' cannot yet inherit the isolated patch sandbox."
            )));
        }
        match scheduled_workspace_tool_class(tool) {
            ScheduledWorkspaceToolClass::Independent => {}
            ScheduledWorkspaceToolClass::IsolatableWrite
                if workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch =>
            {
                has_isolatable_write = true;
            }
            ScheduledWorkspaceToolClass::IsolatableWrite => {
                return Err(invalid(format!(
                    "Scheduled tool '{}' can write a workspace; select isolated_patch or remove it.",
                    tool.trim()
                )));
            }
            ScheduledWorkspaceToolClass::Unsupported => {
                return Err(invalid(format!(
                    "Scheduled tool '{}' is not supported for unattended execution.",
                    tool.trim()
                )));
            }
        }
    }
    if workspace_policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch && !has_isolatable_write {
        return Err(invalid(
            "Isolated scheduled patches require at least one isolation-safe write tool.",
        ));
    }
    Ok(())
}

fn prepare_workspace_target(
    db: &Database,
    input: &mut SaveWorkflowAutomationInput,
    schedule_config: &mut WorkflowAutomationScheduleConfig,
) -> Result<(), CoreError> {
    let mut source_scope = input.source_scope.clone();
    let policy = schedule_config.execution_policy.workspace_policy;
    let is_schedule = matches!(&input.trigger, WorkflowAutomationTrigger::Schedule { .. });
    if !is_schedule {
        schedule_config.execution_policy.source_root_fingerprint = None;
        return Ok(());
    }
    validate_scheduled_allowed_tools(&input.approval_policy.allowed_tools, policy)?;
    if source_scope.is_empty() && policy != WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        if let Some(project_id) = schedule_config.execution_policy.project_id.as_deref() {
            source_scope = db.get_project(project_id)?.source_scope.unwrap_or_default();
        }
    }
    for source_id in &source_scope {
        db.get_source(source_id).map_err(|error| {
            invalid(format!(
                "Scheduled workflow source '{source_id}' is unavailable: {error}"
            ))
        })?;
    }
    if let Some(project_id) = schedule_config.execution_policy.project_id.as_deref() {
        let project_sources = db.get_project(project_id)?.source_scope.unwrap_or_default();
        if source_scope
            .iter()
            .any(|source_id| !project_sources.iter().any(|allowed| allowed == source_id))
        {
            return Err(invalid(
                "Scheduled source scope must remain inside the selected project's source boundary.",
            ));
        }
    }
    let source_root_fingerprint = if policy == WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        if source_scope.len() != 1 {
            return Err(invalid(format!(
                "Isolated scheduled patches require exactly one explicit Source; found {}.",
                source_scope.len()
            )));
        }
        Some(scheduled_source_root_fingerprint(db, &source_scope[0])?)
    } else {
        None
    };
    input.source_scope = source_scope;
    schedule_config.execution_policy.source_root_fingerprint = source_root_fingerprint;
    Ok(())
}

fn validate_scheduled_workspace_target(
    db: &Database,
    automation: &WorkflowAutomation,
) -> Result<(), CoreError> {
    let policy = automation.schedule_config.execution_policy.workspace_policy;
    validate_scheduled_allowed_tools(&automation.approval_policy.allowed_tools, policy)?;
    if policy != WorkflowScheduleWorkspacePolicy::IsolatedPatch {
        return Ok(());
    }
    if automation.source_scope.len() != 1 {
        return Err(invalid(format!(
            "Isolated scheduled patches require exactly one snapshotted Source; found {}.",
            automation.source_scope.len()
        )));
    }
    let current = scheduled_source_root_fingerprint(db, &automation.source_scope[0])?;
    let expected = automation
        .schedule_config
        .execution_policy
        .source_root_fingerprint
        .as_deref()
        .ok_or_else(|| invalid("Isolated scheduled patch is missing its Source fingerprint."))?;
    if current != expected {
        return Err(invalid(
            "Scheduled Source root changed after this definition was saved; review and save it again.",
        ));
    }
    Ok(())
}

pub fn prepare_workflow_automation_save(
    db: &Database,
    mut input: SaveWorkflowAutomationInput,
    schedule_config: Option<WorkflowAutomationScheduleConfig>,
) -> Result<PreparedWorkflowAutomationSave, CoreError> {
    let schedule_config = match (&input.trigger, schedule_config) {
        (WorkflowAutomationTrigger::Schedule { cron }, None) => {
            let legacy = legacy_workflow_schedule_config(cron);
            if legacy.legacy_needs_review {
                return Err(invalid(format!(
                    "Legacy schedule '{cron}' requires review with an explicit timezone and scheduleConfig before it can be saved."
                )));
            }
            legacy
        }
        (_, Some(config)) => config,
        (_, None) => WorkflowAutomationScheduleConfig::default(),
    };
    let mut schedule_config = normalize_scheduled_config_agent_snapshot(db, schedule_config)?;
    prepare_workspace_target(db, &mut input, &mut schedule_config)?;
    Ok(PreparedWorkflowAutomationSave {
        input,
        schedule_config,
    })
}

fn record_scheduler_event_best_effort(
    db: &Database,
    automation_id: Option<&str>,
    run_id: Option<&str>,
    event_type: WorkflowSchedulerEventType,
    status: Option<&str>,
    summary: &str,
    payload: serde_json::Value,
) {
    if let Err(error) = db.record_workflow_automation_scheduler_event(
        automation_id,
        run_id,
        event_type,
        status,
        summary,
        Some(&payload),
    ) {
        warn!(event_type = event_type.as_str(), %error, "failed to persist scheduler event");
    }
}

pub fn scheduler_retry_skip_event(
    retry_decision: &WorkflowAutomationSchedulerRetryDecision,
) -> (WorkflowSchedulerEventType, &'static str, &'static str) {
    if retry_decision.attempts_exhausted {
        (
            WorkflowSchedulerEventType::SkippedRetryLimit,
            "blocked",
            "Scheduler skipped due workflow because retry attempts are exhausted",
        )
    } else {
        (
            WorkflowSchedulerEventType::SkippedBackoff,
            "backoff",
            "Scheduler skipped due workflow until retry backoff expires",
        )
    }
}

pub fn scheduler_status_is_active(status: &str) -> bool {
    let raw = status.trim().to_ascii_lowercase();
    if raw == "cancelling" {
        return true;
    }
    crate::task_orchestrator::project_task_status(&raw)
        .map(|projection| {
            matches!(
                projection.state,
                crate::task_orchestrator::TaskOrchestratorState::Queued
                    | crate::task_orchestrator::TaskOrchestratorState::Running
                    | crate::task_orchestrator::TaskOrchestratorState::WaitingApproval
                    | crate::task_orchestrator::TaskOrchestratorState::Paused
                    | crate::task_orchestrator::TaskOrchestratorState::Resuming
            )
        })
        .unwrap_or(false)
}

pub fn due_workflow_run_is_scheduler_eligible(due: &WorkflowAutomationDueRun) -> bool {
    !due.automation.approval_policy.require_before_run
        && !scheduler_status_is_active(&due.automation.status)
}

pub fn requires_preclaim_approval_skip(trigger_kind: &str, require_before_run: bool) -> bool {
    trigger_kind != "schedule" && require_before_run
}

pub fn prepare_scheduled_workflow_launch(
    db: &Database,
    due: WorkflowAutomationDueRun,
    now: &str,
    summary: Option<String>,
) -> Result<ScheduledWorkflowLaunchPreparation, CoreError> {
    let automation_id = due.automation.id.clone();
    let due_reason = due.due_reason.clone();
    let uses_durable_occurrence = due.automation.trigger_kind == "schedule";

    if !uses_durable_occurrence {
        let retry_decision =
            db.workflow_automation_scheduler_retry_decision(&automation_id, now)?;
        if !retry_decision.allowed {
            let (event_type, status, event_summary) = scheduler_retry_skip_event(&retry_decision);
            record_scheduler_event_best_effort(
                db,
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
            return Ok(ScheduledWorkflowLaunchPreparation::Skipped {
                reason: if retry_decision.attempts_exhausted {
                    "retry_exhausted".into()
                } else {
                    "retry_backoff".into()
                },
            });
        }
        if requires_preclaim_approval_skip(
            &due.automation.trigger_kind,
            due.automation.approval_policy.require_before_run,
        ) {
            record_scheduler_event_best_effort(
                db,
                Some(&automation_id),
                None,
                WorkflowSchedulerEventType::SkippedPreRunApproval,
                Some("waiting_approval"),
                "Scheduler skipped due workflow because pre-run approval is required",
                serde_json::json!({
                    "dueReason": due_reason,
                    "triggerKind": due.automation.trigger_kind,
                    "workflowTemplateId": due.automation.workflow_template_id,
                    "riskLevel": due.automation.approval_policy.risk_level,
                }),
            );
            return Ok(ScheduledWorkflowLaunchPreparation::Skipped {
                reason: "approval_required_before_claim".into(),
            });
        }
    }

    let claim_summary = summary.unwrap_or_else(|| format!("scheduler: {due_reason}"));
    let claim =
        match db.claim_workflow_automation_due_run_at(due, now, Some(claim_summary.as_str())) {
            Ok(claim) => claim,
            Err(error) => {
                record_scheduler_event_best_effort(
                    db,
                    Some(&automation_id),
                    None,
                    WorkflowSchedulerEventType::ClaimFailed,
                    Some("failed"),
                    "Scheduler failed to claim due workflow",
                    serde_json::json!({ "dueReason": due_reason, "error": error.to_string() }),
                );
                return Err(error);
            }
        };
    let Some(claimed_run) = claim.run.as_ref() else {
        let reason = claim.skip_reason.as_deref().unwrap_or("not_launchable");
        if !matches!(
            reason,
            "already_claimed_live" | "already_consumed" | "retry_backoff"
        ) {
            record_scheduler_event_best_effort(
                db,
                Some(&automation_id),
                None,
                WorkflowSchedulerEventType::OccurrenceSkipped,
                Some("skipped"),
                "Scheduler applied the saved occurrence policy",
                serde_json::json!({ "dueReason": due_reason, "skipReason": reason }),
            );
        }
        return Ok(ScheduledWorkflowLaunchPreparation::Skipped {
            reason: reason.to_string(),
        });
    };
    let run_id = claimed_run.id.clone();
    if uses_durable_occurrence {
        let occurrence_id = claimed_run
            .occurrence_id
            .as_deref()
            .ok_or_else(|| invalid("Scheduled workflow claim did not retain an occurrence id."))?;
        let approval_state = db.workflow_automation_occurrence_approval_state(occurrence_id)?;
        if claim.due_run.automation.approval_policy.require_before_run
            && approval_state != WorkflowAutomationApprovalState::Approved
        {
            let requested = db.mark_workflow_automation_run_waiting_approval(&run_id)?;
            let waiting_run = db.get_workflow_automation_run(&run_id)?;
            if requested || waiting_run.status == WorkflowAutomationRunStatus::WaitingApproval {
                return Ok(ScheduledWorkflowLaunchPreparation::PendingApproval {
                    run: waiting_run,
                });
            }
            return Ok(ScheduledWorkflowLaunchPreparation::Skipped {
                reason: "approval_request_not_actionable".into(),
            });
        }
    }

    let policy = match resolve_workflow_launch_policy(
        db,
        &claim.due_run.automation,
        WorkflowLaunchMode::AuthoritativeScheduled,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            record_scheduler_event_best_effort(
                db,
                Some(&automation_id),
                Some(&run_id),
                WorkflowSchedulerEventType::SkippedNoAgentConfig,
                Some("blocked"),
                "Scheduler could not resolve the workflow's saved agent policy",
                serde_json::json!({ "error": error.to_string() }),
            );
            if let Err(transition_error) = db.mark_workflow_automation_launch_failed_for_retry(
                &run_id,
                &error.to_string(),
                now,
            ) {
                warn!(%transition_error, "failed to fence unresolved scheduled route");
            }
            return Err(error);
        }
    };
    let ticket = workflow_due_run_execution_ticket(&claim.due_run, claimed_run)
        .map_err(|error| invalid(error.to_string()))?;
    record_scheduler_event_best_effort(
        db,
        Some(&automation_id),
        Some(&run_id),
        WorkflowSchedulerEventType::Claimed,
        Some(ticket.run.status.raw_status.as_str()),
        "Scheduler claimed due workflow",
        serde_json::json!({
            "queueId": ticket.delivery.queue_item.queue_id,
            "dueReason": due_reason,
            "occurrenceId": claimed_run.occurrence_id,
            "scheduledFor": claimed_run.scheduled_for,
            "definitionRevision": claimed_run.definition_revision,
            "attempt": claimed_run.attempt,
        }),
    );
    Ok(ScheduledWorkflowLaunchPreparation::Ready { ticket, policy })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::SaveAgentConfigInput;
    use crate::project::CreateProjectInput;
    use crate::sources::CreateSourceInput;

    fn save_agent(db: &Database) -> DbAgentConfig {
        db.save_agent_config(&SaveAgentConfigInput {
            id: Some("scheduled-agent".into()),
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
            provider_streaming: Default::default(),
        })
        .expect("save agent config")
    }

    fn schedule_input(cron: &str) -> SaveWorkflowAutomationInput {
        SaveWorkflowAutomationInput {
            id: None,
            name: "Scheduled workflow".into(),
            description: String::new(),
            workflow_template_id: "report_brief".into(),
            prompt: "Run safely".into(),
            trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
            source_scope: Vec::new(),
            approval_policy: WorkflowAutomationApprovalPolicy::default(),
            enabled: true,
        }
    }

    #[test]
    fn non_occurrence_approval_gate_is_owned_by_core() {
        assert!(requires_preclaim_approval_skip("folder", true));
        assert!(!requires_preclaim_approval_skip("folder", false));
        assert!(!requires_preclaim_approval_skip("schedule", true));
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
    fn one_launch_interface_preserves_scheduled_and_interactive_modes() {
        let db = Database::open_memory().expect("database");
        let config = save_agent(&db);
        let execution = WorkflowAutomationExecutionPolicy {
            project_id: Some("project-scheduled".into()),
            workspace_policy: WorkflowScheduleWorkspacePolicy::IsolatedPatch,
            source_root_fingerprint: Some("blake3:test".into()),
            agent_config_id: Some(config.id.clone()),
            provider: Some(config.provider.clone()),
            provider_endpoint_id: config.provider_endpoint_id.clone(),
            model: Some("qwen3.5-max".into()),
            power_mode: "nexus".into(),
            orchestration_profile: "researchUltra".into(),
            collaboration_mode: "delegated".into(),
            execution_mode: Some("plan".into()),
            ..WorkflowAutomationExecutionPolicy::default()
        };
        let approval = WorkflowAutomationApprovalPolicy {
            require_before_run: false,
            allowed_tools: vec!["read_file".into(), "web_search".into()],
            risk_level: "medium".into(),
        };
        let scheduled = build_launch_policy(
            config.clone(),
            &execution,
            &approval,
            WorkflowLaunchMode::AuthoritativeScheduled,
        )
        .expect("scheduled policy");
        let interactive = build_launch_policy(
            config,
            &execution,
            &WorkflowAutomationApprovalPolicy::default(),
            WorkflowLaunchMode::Interactive,
        )
        .expect("interactive policy");

        assert_eq!(scheduled.selected_config.model, "qwen3.5-max");
        assert!(scheduled.force_workspace_isolation);
        assert_eq!(scheduled.tool_approval_mode, ToolApprovalMode::AllowAll);
        assert!(scheduled.agent_config_is_authoritative);
        assert_eq!(interactive.allowed_tools, None);
        assert_eq!(interactive.tool_approval_mode, ToolApprovalMode::Ask);
        assert!(!interactive.force_workspace_isolation);
        assert!(!interactive.agent_config_is_authoritative);
    }

    #[test]
    fn save_preparation_snapshots_route_and_workspace_fingerprint() {
        let db = Database::open_memory().expect("database");
        let agent = save_agent(&db);
        let directory = tempfile::tempdir().expect("source root");
        let source = db
            .add_source(CreateSourceInput {
                root_path: directory.path().to_string_lossy().into_owned(),
                include_globs: Vec::new(),
                exclude_globs: Vec::new(),
                watch_enabled: false,
            })
            .expect("source");
        let project = db
            .create_project(&CreateProjectInput {
                name: "Scheduled project".into(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: Some(vec![source.id.clone()]),
            })
            .expect("project");
        let mut input = schedule_input("0 9 * * *");
        input.source_scope = vec![source.id.clone()];
        input.approval_policy.allowed_tools = vec!["run_shell".into()];
        let mut schedule = WorkflowAutomationScheduleConfig::default();
        schedule.execution_policy.project_id = Some(project.id);
        schedule.execution_policy.agent_config_id = Some(agent.id.clone());
        schedule.execution_policy.workspace_policy = WorkflowScheduleWorkspacePolicy::IsolatedPatch;

        let prepared =
            prepare_workflow_automation_save(&db, input, Some(schedule)).expect("prepare schedule");

        assert_eq!(prepared.input.source_scope, vec![source.id]);
        assert_eq!(
            prepared.schedule_config.execution_policy.provider,
            Some(agent.provider)
        );
        assert_eq!(
            prepared.schedule_config.execution_policy.model,
            Some(agent.model)
        );
        assert!(prepared
            .schedule_config
            .execution_policy
            .source_root_fingerprint
            .as_deref()
            .is_some_and(|value| value.starts_with("blake3:")));
    }

    #[test]
    fn unsafe_workspace_and_legacy_schedules_fail_closed() {
        let db = Database::open_memory().expect("database");
        let mut write_input = schedule_input("0 9 * * *");
        write_input.approval_policy.allowed_tools = vec!["run_shell".into()];
        let error = prepare_workflow_automation_save(
            &db,
            write_input,
            Some(WorkflowAutomationScheduleConfig::default()),
        )
        .expect_err("workspace write must require isolation");
        assert!(error.to_string().contains("select isolated_patch"));

        let safe = prepare_workflow_automation_save(&db, schedule_input("0 9 * * *"), None)
            .expect("safe legacy daily UTC schedule");
        assert_eq!(safe.schedule_config.timezone, "UTC");
        let unsafe_error = prepare_workflow_automation_save(&db, schedule_input("* * * * *"), None)
            .expect_err("wildcard legacy schedule requires review");
        assert!(unsafe_error.to_string().contains("requires review"));
    }

    #[test]
    fn retry_skip_event_uses_the_shared_vocabulary() {
        let exhausted = WorkflowAutomationSchedulerRetryDecision {
            allowed: false,
            max_attempts: 4,
            attempts_exhausted: true,
            retryable_failure_count: 4,
            last_retryable_event_type: None,
            last_retryable_event_at: None,
            backoff_seconds: None,
            backoff_until: None,
            retry_after_seconds: None,
        };
        assert_eq!(
            scheduler_retry_skip_event(&exhausted).0,
            WorkflowSchedulerEventType::SkippedRetryLimit
        );
    }
}
