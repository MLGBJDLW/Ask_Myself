//! Task Orchestrator state machine contract.
//!
//! Long-running manual, scheduled, folder-triggered, workflow, and delegated
//! work should share this state machine before delivery adapters render it.

use serde::{Deserialize, Serialize};

use crate::conversation::AgentTaskRun;
use crate::workflow_automation::{
    WorkflowAutomation, WorkflowAutomationDueRun, WorkflowAutomationRun,
};

pub const TASK_ORCHESTRATOR_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOrchestratorState {
    Draft,
    Queued,
    Running,
    WaitingApproval,
    Paused,
    Resuming,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Disabled,
}

impl TaskOrchestratorState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Disabled
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunOwnership {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub source_scope: Vec<String>,
    #[serde(default)]
    pub package_id: Option<String>,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOrchestratorRunKind {
    AgentTask,
    WorkflowAutomation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusProjection {
    pub raw_status: String,
    pub state: TaskOrchestratorState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorQueueItem {
    pub version: u16,
    pub queue_id: String,
    pub task_definition_id: String,
    pub state: TaskOrchestratorState,
    pub ownership: TaskRunOwnership,
    pub trigger_kind: String,
    pub due_reason: String,
    pub prompt: String,
    pub approval_required: bool,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub risk_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorDeliveryEnvelope {
    pub version: u16,
    pub queue_item: TaskOrchestratorQueueItem,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorExecutionTicket {
    pub version: u16,
    pub delivery: TaskOrchestratorDeliveryEnvelope,
    pub run: TaskOrchestratorRun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrchestratorRun {
    pub version: u16,
    pub run_id: String,
    #[serde(default)]
    pub task_run_id: Option<String>,
    #[serde(default)]
    pub task_definition_id: Option<String>,
    pub kind: TaskOrchestratorRunKind,
    pub status: TaskStatusProjection,
    pub ownership: TaskRunOwnership,
    #[serde(default)]
    pub trigger_kind: Option<String>,
    pub approval_required: bool,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStateTransition {
    pub run_id: String,
    pub from: TaskOrchestratorState,
    pub to: TaskOrchestratorState,
    pub reason: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaskOrchestratorError {
    #[error("illegal task state transition from {from:?} to {to:?}")]
    IllegalTransition {
        from: TaskOrchestratorState,
        to: TaskOrchestratorState,
    },
    #[error("terminal task state may not transition to {to:?}")]
    TerminalTransition { to: TaskOrchestratorState },
    #[error("unknown task status {status}")]
    UnknownStatus { status: String },
    #[error("execution ticket requires queued run state, got {state:?}")]
    ExecutionTicketState { state: TaskOrchestratorState },
    #[error(
        "execution ticket queue task definition {queue_task_definition_id} does not match run task definition {run_task_definition_id}"
    )]
    MismatchedExecutionTicket {
        queue_task_definition_id: String,
        run_task_definition_id: String,
    },
}

pub fn project_task_status(status: &str) -> Result<TaskStatusProjection, TaskOrchestratorError> {
    let raw_status = status.trim().to_ascii_lowercase();
    let state = match raw_status.as_str() {
        "draft" | "ready" => TaskOrchestratorState::Draft,
        "queued" | "pending" => TaskOrchestratorState::Queued,
        "running" | "initializing" | "in_progress" => TaskOrchestratorState::Running,
        "waiting_approval" => TaskOrchestratorState::WaitingApproval,
        "paused" => TaskOrchestratorState::Paused,
        "resuming" => TaskOrchestratorState::Resuming,
        "completed" | "cached" | "done" => TaskOrchestratorState::Completed,
        "failed" | "error" => TaskOrchestratorState::Failed,
        "cancelled" | "canceled" => TaskOrchestratorState::Cancelled,
        "timed_out" | "timeout" => TaskOrchestratorState::TimedOut,
        "disabled" => TaskOrchestratorState::Disabled,
        _ => {
            return Err(TaskOrchestratorError::UnknownStatus {
                status: status.to_string(),
            })
        }
    };
    Ok(TaskStatusProjection { raw_status, state })
}

pub fn can_transition_task_state(from: TaskOrchestratorState, to: TaskOrchestratorState) -> bool {
    use TaskOrchestratorState::*;
    matches!(
        (from, to),
        (Draft, Queued | Disabled)
            | (Queued, Running | Cancelled | Disabled)
            | (
                Running,
                WaitingApproval | Paused | Completed | Failed | Cancelled | TimedOut
            )
            | (
                WaitingApproval,
                Running | Paused | Cancelled | Failed | TimedOut
            )
            | (Paused, Resuming | Cancelled | Disabled)
            | (Resuming, Running | Failed | Cancelled | TimedOut)
    )
}

pub fn validate_task_transition(
    from: TaskOrchestratorState,
    to: TaskOrchestratorState,
) -> Result<(), TaskOrchestratorError> {
    if from.is_terminal() {
        return Err(TaskOrchestratorError::TerminalTransition { to });
    }
    if can_transition_task_state(from, to) {
        Ok(())
    } else {
        Err(TaskOrchestratorError::IllegalTransition { from, to })
    }
}

pub fn apply_task_transition(
    current: TaskOrchestratorState,
    transition: &TaskStateTransition,
) -> Result<TaskOrchestratorState, TaskOrchestratorError> {
    validate_task_transition(current, transition.to)?;
    Ok(transition.to)
}

pub fn workflow_due_run_queue_item(due: &WorkflowAutomationDueRun) -> TaskOrchestratorQueueItem {
    let automation = &due.automation;
    workflow_queue_item(
        &format!("workflow_due:{}", automation.id),
        automation,
        due.prompt.clone(),
        due.due_reason.clone(),
    )
}

pub fn workflow_due_run_delivery_envelope(
    due: &WorkflowAutomationDueRun,
) -> TaskOrchestratorDeliveryEnvelope {
    TaskOrchestratorDeliveryEnvelope {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        queue_item: workflow_due_run_queue_item(due),
        prompt: due.prompt.clone(),
    }
}

pub fn workflow_automation_delivery_envelope(
    automation: &WorkflowAutomation,
    prompt: impl Into<String>,
    due_reason: impl Into<String>,
) -> TaskOrchestratorDeliveryEnvelope {
    let prompt = prompt.into();
    TaskOrchestratorDeliveryEnvelope {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        queue_item: workflow_queue_item(
            &format!("workflow_delivery:{}", automation.id),
            automation,
            prompt.clone(),
            due_reason.into(),
        ),
        prompt,
    }
}

pub fn workflow_automation_execution_ticket(
    automation: &WorkflowAutomation,
    run: &WorkflowAutomationRun,
    delivery: TaskOrchestratorDeliveryEnvelope,
) -> Result<TaskOrchestratorExecutionTicket, TaskOrchestratorError> {
    let projected_run = workflow_automation_run_projection(automation, run)?;
    if projected_run.status.state != TaskOrchestratorState::Queued {
        return Err(TaskOrchestratorError::ExecutionTicketState {
            state: projected_run.status.state,
        });
    }

    let run_task_definition_id = projected_run.task_definition_id.clone().unwrap_or_default();
    if delivery.queue_item.task_definition_id != run_task_definition_id {
        return Err(TaskOrchestratorError::MismatchedExecutionTicket {
            queue_task_definition_id: delivery.queue_item.task_definition_id,
            run_task_definition_id,
        });
    }

    Ok(TaskOrchestratorExecutionTicket {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        delivery,
        run: projected_run,
    })
}

pub fn workflow_due_run_execution_ticket(
    due: &WorkflowAutomationDueRun,
    run: &WorkflowAutomationRun,
) -> Result<TaskOrchestratorExecutionTicket, TaskOrchestratorError> {
    workflow_automation_execution_ticket(
        &due.automation,
        run,
        workflow_due_run_delivery_envelope(due),
    )
}

fn workflow_queue_item(
    queue_id: &str,
    automation: &WorkflowAutomation,
    prompt: String,
    due_reason: String,
) -> TaskOrchestratorQueueItem {
    TaskOrchestratorQueueItem {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        queue_id: queue_id.to_string(),
        task_definition_id: automation.id.clone(),
        state: TaskOrchestratorState::Queued,
        ownership: workflow_automation_ownership(automation),
        trigger_kind: automation.trigger_kind.clone(),
        due_reason,
        prompt,
        approval_required: automation.approval_policy.require_before_run,
        allowed_tools: automation.approval_policy.allowed_tools.clone(),
        risk_level: Some(automation.approval_policy.risk_level.clone()),
    }
}

pub fn workflow_automation_run_projection(
    automation: &WorkflowAutomation,
    run: &WorkflowAutomationRun,
) -> Result<TaskOrchestratorRun, TaskOrchestratorError> {
    Ok(TaskOrchestratorRun {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        run_id: run.id.clone(),
        task_run_id: run.task_run_id.clone(),
        task_definition_id: Some(automation.id.clone()),
        kind: TaskOrchestratorRunKind::WorkflowAutomation,
        status: project_task_status(&run.status)?,
        ownership: workflow_automation_ownership(automation),
        trigger_kind: Some(automation.trigger_kind.clone()),
        approval_required: automation.approval_policy.require_before_run,
        allowed_tools: automation.approval_policy.allowed_tools.clone(),
        risk_level: Some(automation.approval_policy.risk_level.clone()),
        summary: run.summary.clone(),
        created_at: Some(run.created_at.clone()),
        finished_at: run.finished_at.clone(),
    })
}

pub fn agent_task_run_projection(
    run: &AgentTaskRun,
    source_scope: Vec<String>,
) -> Result<TaskOrchestratorRun, TaskOrchestratorError> {
    Ok(TaskOrchestratorRun {
        version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
        run_id: run.id.clone(),
        task_run_id: Some(run.id.clone()),
        task_definition_id: None,
        kind: TaskOrchestratorRunKind::AgentTask,
        status: project_task_status(&run.status)?,
        ownership: TaskRunOwnership {
            user_id: None,
            profile_id: None,
            source_scope,
            package_id: None,
            workflow_id: None,
            session_id: Some(run.conversation_id.clone()),
        },
        trigger_kind: None,
        approval_required: false,
        allowed_tools: Vec::new(),
        risk_level: None,
        summary: run.summary.clone(),
        created_at: Some(run.created_at.clone()),
        finished_at: run.finished_at.clone(),
    })
}

fn workflow_automation_ownership(automation: &WorkflowAutomation) -> TaskRunOwnership {
    TaskRunOwnership {
        user_id: None,
        profile_id: None,
        source_scope: automation.source_scope.clone(),
        package_id: Some(crate::skills::package::BUILTIN_WORKFLOWS_PACKAGE_ID.to_string()),
        workflow_id: Some(automation.workflow_template_id.clone()),
        session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_automation::{WorkflowAutomationApprovalPolicy, WorkflowAutomationTrigger};

    fn workflow_automation(enabled: bool) -> WorkflowAutomation {
        WorkflowAutomation {
            id: "automation-1".to_string(),
            name: "Daily report".to_string(),
            description: "Summarize daily evidence.".to_string(),
            workflow_template_id: "report_brief".to_string(),
            prompt: "Summarize reports.".to_string(),
            trigger_kind: "schedule".to_string(),
            trigger: WorkflowAutomationTrigger::Schedule {
                cron: "0 9 * * *".to_string(),
            },
            source_scope: vec!["source-1".to_string()],
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: true,
                allowed_tools: vec!["search_knowledge_base".to_string()],
                risk_level: "medium".to_string(),
            },
            enabled,
            status: if enabled {
                "ready".to_string()
            } else {
                "disabled".to_string()
            },
            last_run_at: None,
            next_run_at: Some("2099-01-01T09:00:00Z".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn workflow_run(status: &str) -> WorkflowAutomationRun {
        WorkflowAutomationRun {
            id: "workflow-run-1".to_string(),
            automation_id: "automation-1".to_string(),
            task_run_id: Some("task-run-1".to_string()),
            status: status.to_string(),
            summary: Some("done".to_string()),
            created_at: "2026-01-01T09:00:00Z".to_string(),
            finished_at: Some("2026-01-01T09:05:00Z".to_string()),
        }
    }

    #[test]
    fn state_machine_accepts_expected_workflow_path() {
        let path = [
            TaskOrchestratorState::Draft,
            TaskOrchestratorState::Queued,
            TaskOrchestratorState::Running,
            TaskOrchestratorState::WaitingApproval,
            TaskOrchestratorState::Running,
            TaskOrchestratorState::Completed,
        ];

        for window in path.windows(2) {
            validate_task_transition(window[0], window[1]).unwrap();
        }
    }

    #[test]
    fn state_machine_rejects_skipping_queue() {
        assert_eq!(
            validate_task_transition(TaskOrchestratorState::Draft, TaskOrchestratorState::Running)
                .unwrap_err(),
            TaskOrchestratorError::IllegalTransition {
                from: TaskOrchestratorState::Draft,
                to: TaskOrchestratorState::Running
            }
        );
    }

    #[test]
    fn terminal_states_do_not_resume() {
        assert_eq!(
            validate_task_transition(
                TaskOrchestratorState::Completed,
                TaskOrchestratorState::Running
            )
            .unwrap_err(),
            TaskOrchestratorError::TerminalTransition {
                to: TaskOrchestratorState::Running
            }
        );
    }

    #[test]
    fn status_projection_maps_known_task_statuses() {
        assert_eq!(
            project_task_status("ready").unwrap().state,
            TaskOrchestratorState::Draft
        );
        assert_eq!(
            project_task_status("queued").unwrap().state,
            TaskOrchestratorState::Queued
        );
        assert_eq!(
            project_task_status("paused").unwrap().state,
            TaskOrchestratorState::Paused
        );
        assert_eq!(
            project_task_status("timed_out").unwrap().state,
            TaskOrchestratorState::TimedOut
        );
        assert_eq!(
            project_task_status("surprise").unwrap_err(),
            TaskOrchestratorError::UnknownStatus {
                status: "surprise".to_string()
            }
        );
    }

    #[test]
    fn due_workflow_projects_to_queue_item_with_ownership_and_approval_policy() {
        let automation = workflow_automation(true);
        let due = WorkflowAutomationDueRun {
            automation,
            prompt: "Run the saved workflow.".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
        };

        let item = workflow_due_run_queue_item(&due);

        assert_eq!(item.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(item.queue_id, "workflow_due:automation-1");
        assert_eq!(item.state, TaskOrchestratorState::Queued);
        assert_eq!(item.task_definition_id, "automation-1");
        assert_eq!(item.ownership.source_scope, vec!["source-1".to_string()]);
        assert_eq!(
            item.ownership.package_id.as_deref(),
            Some(crate::skills::package::BUILTIN_WORKFLOWS_PACKAGE_ID)
        );
        assert_eq!(item.ownership.workflow_id.as_deref(), Some("report_brief"));
        assert!(item.approval_required);
        assert_eq!(
            item.allowed_tools,
            vec!["search_knowledge_base".to_string()]
        );
        assert_eq!(item.risk_level.as_deref(), Some("medium"));
    }

    #[test]
    fn due_workflow_delivery_envelope_preserves_due_prompt_and_reason() {
        let automation = workflow_automation(true);
        let due = WorkflowAutomationDueRun {
            automation,
            prompt: "Run the scheduled workflow.".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
        };

        let envelope = workflow_due_run_delivery_envelope(&due);

        assert_eq!(envelope.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(envelope.prompt, "Run the scheduled workflow.");
        assert_eq!(envelope.queue_item.queue_id, "workflow_due:automation-1");
        assert_eq!(envelope.queue_item.prompt, "Run the scheduled workflow.");
        assert_eq!(envelope.queue_item.due_reason, "schedule 0 9 * * *");
        assert_eq!(
            envelope.queue_item.ownership.workflow_id.as_deref(),
            Some("report_brief")
        );
    }

    #[test]
    fn workflow_delivery_envelope_carries_prompt_and_queue_item() {
        let automation = workflow_automation(true);

        let envelope = workflow_automation_delivery_envelope(
            &automation,
            "Run the saved workflow.",
            "manual run requested",
        );

        assert_eq!(envelope.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(envelope.prompt, "Run the saved workflow.");
        assert_eq!(
            envelope.queue_item.queue_id,
            "workflow_delivery:automation-1"
        );
        assert_eq!(envelope.queue_item.state, TaskOrchestratorState::Queued);
        assert_eq!(envelope.queue_item.due_reason, "manual run requested");
        assert_eq!(
            envelope.queue_item.ownership.workflow_id.as_deref(),
            Some("report_brief")
        );
        assert_eq!(
            envelope.queue_item.ownership.source_scope,
            vec!["source-1".to_string()]
        );
        assert!(envelope.queue_item.approval_required);
    }

    #[test]
    fn workflow_execution_ticket_binds_delivery_to_queued_run_projection() {
        let automation = workflow_automation(true);
        let run = workflow_run("queued");
        let envelope = workflow_automation_delivery_envelope(
            &automation,
            "Run the saved workflow.",
            "manual run requested",
        );

        let ticket = workflow_automation_execution_ticket(&automation, &run, envelope).unwrap();

        assert_eq!(ticket.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(
            ticket.delivery.queue_item.queue_id,
            "workflow_delivery:automation-1"
        );
        assert_eq!(ticket.run.run_id, "workflow-run-1");
        assert_eq!(ticket.run.status.state, TaskOrchestratorState::Queued);
        assert_eq!(
            ticket.run.task_definition_id.as_deref(),
            Some("automation-1")
        );
        assert_eq!(
            ticket.run.ownership.workflow_id.as_deref(),
            Some("report_brief")
        );
    }

    #[test]
    fn due_workflow_execution_ticket_binds_claimed_run_to_due_delivery() {
        let automation = workflow_automation(true);
        let due = WorkflowAutomationDueRun {
            automation,
            prompt: "Run the scheduled workflow.".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
        };
        let run = workflow_run("queued");

        let ticket = workflow_due_run_execution_ticket(&due, &run).unwrap();

        assert_eq!(ticket.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(
            ticket.delivery.queue_item.queue_id,
            "workflow_due:automation-1"
        );
        assert_eq!(ticket.delivery.prompt, "Run the scheduled workflow.");
        assert_eq!(ticket.run.run_id, "workflow-run-1");
        assert_eq!(ticket.run.status.state, TaskOrchestratorState::Queued);
        assert_eq!(
            ticket.run.task_definition_id.as_deref(),
            Some("automation-1")
        );
    }

    #[test]
    fn workflow_execution_ticket_requires_queued_run_projection() {
        let automation = workflow_automation(true);
        let run = workflow_run("running");
        let envelope = workflow_automation_delivery_envelope(
            &automation,
            "Run the saved workflow.",
            "manual run requested",
        );

        assert_eq!(
            workflow_automation_execution_ticket(&automation, &run, envelope).unwrap_err(),
            TaskOrchestratorError::ExecutionTicketState {
                state: TaskOrchestratorState::Running
            }
        );
    }

    #[test]
    fn workflow_run_projects_to_orchestrator_run() {
        let automation = workflow_automation(true);
        let run = workflow_run("completed");

        let projected = workflow_automation_run_projection(&automation, &run).unwrap();

        assert_eq!(projected.version, TASK_ORCHESTRATOR_CONTRACT_VERSION);
        assert_eq!(projected.run_id, "workflow-run-1");
        assert_eq!(projected.task_run_id.as_deref(), Some("task-run-1"));
        assert_eq!(projected.kind, TaskOrchestratorRunKind::WorkflowAutomation);
        assert_eq!(projected.status.raw_status, "completed");
        assert_eq!(projected.status.state, TaskOrchestratorState::Completed);
        assert_eq!(projected.trigger_kind.as_deref(), Some("schedule"));
        assert_eq!(projected.summary.as_deref(), Some("done"));
    }

    #[test]
    fn agent_task_run_projects_to_orchestrator_run() {
        let run = AgentTaskRun {
            id: "task-run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            turn_id: "turn-1".to_string(),
            user_message_id: "message-1".to_string(),
            status: "running".to_string(),
            phase: "executing".to_string(),
            title: "Investigate".to_string(),
            route_kind: None,
            summary: Some("working".to_string()),
            error_message: None,
            provider: Some("open_ai".to_string()),
            model: Some("gpt-test".to_string()),
            plan: None,
            artifacts: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:01Z".to_string(),
            started_at: Some("2026-01-01T00:00:01Z".to_string()),
            finished_at: None,
        };

        let projected = agent_task_run_projection(&run, vec!["source-1".to_string()]).unwrap();

        assert_eq!(projected.run_id, "task-run-1");
        assert_eq!(projected.task_run_id.as_deref(), Some("task-run-1"));
        assert_eq!(projected.kind, TaskOrchestratorRunKind::AgentTask);
        assert_eq!(projected.status.state, TaskOrchestratorState::Running);
        assert_eq!(
            projected.ownership.session_id.as_deref(),
            Some("conversation-1")
        );
        assert_eq!(
            projected.ownership.source_scope,
            vec!["source-1".to_string()]
        );
    }
}
