//! Durable task-run runtime for long-running agent work.
//!
//! The database already owns the rows. This Module gives callers a small
//! lifecycle Interface for starting, updating, cancelling, and attaching
//! delegated work without knowing the table layout.

use serde::{Deserialize, Serialize};

use crate::agent_run::{AgentRunEvent, AgentRunEventKind};
use crate::conversation::{AgentSubtaskRun, AgentTaskRun, AgentTaskRunEvent};
use crate::db::Database;
use crate::error::CoreError;
use crate::task_timeline::TaskTimelineEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Queued,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    Paused,
}

impl TaskRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl SubtaskRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

pub struct CreateTaskRunInput<'a> {
    pub conversation_id: &'a str,
    pub turn_id: &'a str,
    pub user_message_id: &'a str,
    pub title: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
}

pub struct CreateSubtaskRunInput<'a> {
    pub parent_run_id: &'a str,
    pub label: &'a str,
    pub role: &'a str,
    pub input: Option<&'a serde_json::Value>,
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskRunUpdate<'a> {
    pub status: Option<TaskRunStatus>,
    pub phase: Option<&'a str>,
    pub route_kind: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub plan: Option<&'a serde_json::Value>,
    pub artifacts: Option<&'a serde_json::Value>,
}

pub struct AgentTaskRuntime<'a> {
    db: &'a Database,
}

impl<'a> AgentTaskRuntime<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    pub fn create_run(&self, input: CreateTaskRunInput<'_>) -> Result<AgentTaskRun, CoreError> {
        self.db.create_agent_task_run(
            input.conversation_id,
            input.turn_id,
            input.user_message_id,
            input.title,
            input.provider,
            input.model,
        )
    }

    pub fn start_run(&self, run_id: &str, phase: &str) -> Result<AgentTaskRun, CoreError> {
        self.db.mark_agent_task_run_started(run_id, phase)?;
        self.db.get_agent_task_run(run_id)
    }

    pub fn update_run(
        &self,
        run_id: &str,
        update: TaskRunUpdate<'_>,
    ) -> Result<AgentTaskRun, CoreError> {
        self.db.update_agent_task_run_progress(
            run_id,
            update.status.map(TaskRunStatus::as_str),
            update.phase,
            update.route_kind,
            update.summary,
            update.plan,
            update.artifacts,
        )?;
        self.db.get_agent_task_run(run_id)
    }

    pub fn finish_run(
        &self,
        run_id: &str,
        status: TaskRunStatus,
        summary: Option<&str>,
        error_message: Option<&str>,
        artifacts: Option<&serde_json::Value>,
    ) -> Result<AgentTaskRun, CoreError> {
        self.db.finish_agent_task_run(
            run_id,
            status.as_str(),
            summary,
            error_message,
            artifacts,
        )?;
        self.db.get_agent_task_run(run_id)
    }

    pub fn cancel_run(&self, run_id: &str, reason: &str) -> Result<AgentTaskRun, CoreError> {
        self.finish_run(
            run_id,
            TaskRunStatus::Cancelled,
            Some(reason),
            Some(reason),
            None,
        )
    }

    pub fn apply_run_event(
        &self,
        run_id: &str,
        event: &AgentRunEvent,
    ) -> Result<AgentTaskRun, CoreError> {
        self.update_progress_from_event(run_id, event)?;
        self.db.get_agent_task_run(run_id)
    }

    pub fn record_timeline_event(
        &self,
        run_id: &str,
        event: &TaskTimelineEvent,
    ) -> Result<AgentTaskRunEvent, CoreError> {
        let payload = event.task_event_payload();
        self.db.record_agent_task_run_event(
            run_id,
            event.event_type(),
            &event.label,
            event.status.as_deref(),
            Some(&payload),
        )
    }

    pub fn enqueue_subtask(
        &self,
        input: CreateSubtaskRunInput<'_>,
    ) -> Result<AgentSubtaskRun, CoreError> {
        self.db.create_agent_subtask_run(
            input.parent_run_id,
            input.label,
            input.role,
            input.input,
            input.token_budget,
        )
    }

    pub fn start_subtask(
        &self,
        subtask_run_id: &str,
        phase: &str,
    ) -> Result<AgentSubtaskRun, CoreError> {
        self.db
            .mark_agent_subtask_run_started(subtask_run_id, phase)?;
        self.db.get_agent_subtask_run(subtask_run_id)
    }

    pub fn finish_subtask(
        &self,
        subtask_run_id: &str,
        status: SubtaskRunStatus,
        output: Option<&serde_json::Value>,
        error_message: Option<&str>,
    ) -> Result<AgentSubtaskRun, CoreError> {
        self.db
            .finish_agent_subtask_run(subtask_run_id, status.as_str(), output, error_message)?;
        self.db.get_agent_subtask_run(subtask_run_id)
    }

    fn update_progress_from_event(
        &self,
        run_id: &str,
        event: &AgentRunEvent,
    ) -> Result<(), CoreError> {
        match event.kind {
            AgentRunEventKind::ApprovalRequested => self.db.update_agent_task_run_progress(
                run_id,
                Some(TaskRunStatus::WaitingApproval.as_str()),
                Some("approval"),
                None,
                Some(&event.label),
                None,
                None,
            ),
            AgentRunEventKind::ApprovalResolved => self.db.update_agent_task_run_progress(
                run_id,
                Some(TaskRunStatus::Running.as_str()),
                Some("tooling"),
                None,
                Some(&event.label),
                None,
                None,
            ),
            AgentRunEventKind::ToolPreparing
            | AgentRunEventKind::ToolStarted
            | AgentRunEventKind::ToolProgress
            | AgentRunEventKind::ToolCompleted => self.db.update_agent_task_run_progress(
                run_id,
                Some(TaskRunStatus::Running.as_str()),
                Some("tooling"),
                None,
                Some(&event.label),
                None,
                None,
            ),
            AgentRunEventKind::PlanUpdated => self.db.update_agent_task_run_progress(
                run_id,
                Some(TaskRunStatus::Running.as_str()),
                Some(event.phase.as_str()),
                None,
                Some(&event.label),
                event.payload.get("plan"),
                None,
            ),
            AgentRunEventKind::Status => {
                let route_kind = event.label.strip_prefix("Route selected: ").map(str::trim);
                self.db.update_agent_task_run_progress(
                    run_id,
                    Some(TaskRunStatus::Running.as_str()),
                    Some(event.phase.as_str()),
                    route_kind,
                    Some(&event.label),
                    None,
                    None,
                )
            }
            AgentRunEventKind::RecoveryAttempt => self.db.update_agent_task_run_progress(
                run_id,
                Some(TaskRunStatus::Running.as_str()),
                Some("recovering"),
                None,
                Some(&event.label),
                None,
                None,
            ),
            AgentRunEventKind::Done => {
                let (status, summary) = match event.status.as_deref() {
                    Some("cancelled") => (TaskRunStatus::Cancelled, "Agent execution cancelled"),
                    Some("timed_out") => (TaskRunStatus::TimedOut, "Agent execution timed out"),
                    _ => (TaskRunStatus::Completed, event.label.as_str()),
                };
                self.db
                    .finish_agent_task_run(run_id, status.as_str(), Some(summary), None, None)
            }
            AgentRunEventKind::Error => {
                let (status, summary) = match event.status.as_deref() {
                    Some("cancelled") => (TaskRunStatus::Cancelled, "Agent execution cancelled"),
                    Some("timed_out") => (TaskRunStatus::TimedOut, "Agent execution timed out"),
                    _ => (TaskRunStatus::Failed, "Agent execution failed"),
                };
                self.db.finish_agent_task_run(
                    run_id,
                    status.as_str(),
                    Some(summary),
                    Some(&event.label),
                    None,
                )
            }
            AgentRunEventKind::OutputDelta
            | AgentRunEventKind::StreamReset
            | AgentRunEventKind::Thinking
            | AgentRunEventKind::UsageUpdated
            | AgentRunEventKind::AutoCompacted => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::agent_run::AgentRunEvent;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::{Message, Role, Usage};

    fn create_started_run(db: &Database, suffix: &str) -> (String, String) {
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let message = ConversationMessage {
            id: format!("msg-{suffix}"),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Investigate my notes.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .unwrap();
        let runtime = AgentTaskRuntime::new(db);
        let run = runtime
            .create_run(CreateTaskRunInput {
                conversation_id: &conversation.id,
                turn_id: &turn.id,
                user_message_id: &message.id,
                title: "Investigate my notes",
                provider: Some("openai"),
                model: Some("gpt-4o"),
            })
            .unwrap();
        runtime.start_run(&run.id, "routing").unwrap();
        (run.id, turn.id)
    }

    #[test]
    fn runtime_applies_run_events_and_subtasks() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let message = ConversationMessage {
            id: "msg-1".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Investigate my notes.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .unwrap();

        let runtime = AgentTaskRuntime::new(&db);
        let run = runtime
            .create_run(CreateTaskRunInput {
                conversation_id: &conversation.id,
                turn_id: &turn.id,
                user_message_id: &message.id,
                title: "Investigate my notes",
                provider: Some("openai"),
                model: Some("gpt-4o"),
            })
            .unwrap();
        runtime.start_run(&run.id, "routing").unwrap();

        let route_event = AgentRunEvent::from_agent_event(&AgentEvent::Status {
            content: "Route selected: KnowledgeRetrieval".to_string(),
            tone: None,
        })
        .with_context(Some(&run.id), Some(&turn.id), Some(1));
        let updated_run = runtime.apply_run_event(&run.id, &route_event).unwrap();
        assert_eq!(updated_run.phase, "routing");
        assert!(db.get_agent_task_run_events(&run.id).unwrap().is_empty());

        let subtask = runtime
            .enqueue_subtask(CreateSubtaskRunInput {
                parent_run_id: &run.id,
                label: "Collect evidence",
                role: "researcher",
                input: Some(&serde_json::json!({ "query": "notes" })),
                token_budget: Some(1000),
            })
            .unwrap();
        let subtask = runtime.start_subtask(&subtask.id, "running").unwrap();
        assert_eq!(subtask.status, "running");
        let subtask = runtime
            .finish_subtask(
                &subtask.id,
                SubtaskRunStatus::Completed,
                Some(&serde_json::json!({ "summary": "done" })),
                None,
            )
            .unwrap();
        assert_eq!(subtask.status, "completed");

        let timeline_event = runtime
            .record_timeline_event(
                &run.id,
                &TaskTimelineEvent::subtask(
                    "Collect evidence",
                    "completed",
                    Some(&serde_json::json!({ "subtaskRunId": subtask.id })),
                ),
            )
            .unwrap();
        assert_eq!(timeline_event.event_type, "subtask");
        assert_eq!(
            timeline_event.payload.unwrap()["taskTimeline"]["kind"],
            "subtask"
        );

        let run = db.get_agent_task_run(&run.id).unwrap();
        assert_eq!(run.phase, "routing");
        assert_eq!(run.route_kind.as_deref(), Some("KnowledgeRetrieval"));
    }

    #[test]
    fn runtime_preserves_terminal_error_statuses() {
        let db = Database::open_memory().unwrap();
        let runtime = AgentTaskRuntime::new(&db);

        for (seq, status) in [(1, "failed"), (2, "cancelled"), (3, "timed_out")] {
            let (run_id, turn_id) = create_started_run(&db, status);
            let mut run_event = AgentRunEvent::from_agent_event(&AgentEvent::Error {
                message: format!("terminal {status}"),
            })
            .with_context(Some(&run_id), Some(&turn_id), Some(seq));
            run_event.status = Some(status.to_string());

            let run = runtime.apply_run_event(&run_id, &run_event).unwrap();
            assert_eq!(run.status, status);
            assert!(db.get_agent_task_run_events(&run_id).unwrap().is_empty());
        }

        let (run_id, turn_id) = create_started_run(&db, "done-cancelled");
        let done_event = AgentRunEvent::from_agent_event(&AgentEvent::Done {
            message: Message::text(Role::Assistant, "Request cancelled by user."),
            usage_total: Usage::default(),
            last_prompt_tokens: 0,
            context_breakdown: None,
            cached: false,
            finish_reason: Some("cancelled".to_string()),
        })
        .with_context(Some(&run_id), Some(&turn_id), Some(4));

        let run = runtime.apply_run_event(&run_id, &done_event).unwrap();
        assert_eq!(run.status, "cancelled");
        assert!(db.get_agent_task_run_events(&run_id).unwrap().is_empty());
    }
}
