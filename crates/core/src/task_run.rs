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
use crate::workflow_automation::TaskResumeCheckpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Queued,
    Running,
    Cancelling,
    WaitingApproval,
    AwaitingUserInput,
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
            Self::Cancelling => "cancelling",
            Self::WaitingApproval => "waiting_approval",
            Self::AwaitingUserInput => "awaiting_user_input",
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

#[derive(Debug, Clone)]
pub(crate) enum AgentRunFailClosedOutcome {
    Claimed {
        event_seq: u64,
        snapshot: Box<AgentTaskRun>,
    },
    AlreadyClosed {
        event_seq: u64,
    },
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
        let connection = self.db.conn();
        Self::update_progress_from_event_on_connection(&connection, run_id, event)?;
        Database::get_agent_task_run_on_connection(&connection, run_id)
    }

    /// Commit the durable Run Event batch and every task projection it causes
    /// in one SQLite transaction. Delivery can begin only after this returns.
    pub(crate) fn commit_run_event_batch(
        &self,
        run_id: &str,
        events: &[AgentRunEvent],
    ) -> Result<Vec<AgentTaskRun>, CoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        let mut connection = self.db.conn();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (_, already_closed) =
            Database::agent_run_event_head_on_connection(&transaction, run_id)?;
        if already_closed {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} is already closed"
            )));
        }
        crate::db::insert_agent_run_events(&transaction, events)?;

        let mut snapshots = Vec::new();
        for event in events {
            if Self::update_progress_from_event_on_connection(&transaction, run_id, event)? {
                snapshots.push(Database::get_agent_task_run_on_connection(
                    &transaction,
                    run_id,
                )?);
            }
        }
        transaction.commit()?;
        Ok(snapshots)
    }

    /// Commit a resumable pause as one durable boundary. The outbox supplies
    /// the sequence number; this transaction owns the checkpoint row and both
    /// materialized lifecycle projections so no restart can observe a partial
    /// pause.
    pub(crate) fn commit_pause_checkpoint(
        &self,
        run_id: &str,
        turn_id: &str,
        event_seq: u64,
        reason: &str,
    ) -> Result<(TaskResumeCheckpoint, AgentRunEvent, AgentTaskRun), CoreError> {
        let checkpoint = self
            .db
            .prepare_task_resume_checkpoint(run_id, reason, None)?;
        let artifacts = serde_json::json!({
            "kind": "resumeCheckpoint",
            "checkpointId": checkpoint.id,
            "resumePrompt": checkpoint.resume_prompt,
        });
        let event = AgentRunEvent::status_update(
            run_id,
            Some(turn_id),
            event_seq,
            crate::agent_run::AgentRunPhase::Paused,
            "Paused with a resumable checkpoint",
            Some(TaskRunStatus::Paused.as_str()),
            Some(&artifacts),
        );
        event.validate_durable_contract().map_err(|error| {
            CoreError::InvalidInput(format!("invalid pause Run Event: {error}"))
        })?;

        let mut connection = self.db.conn();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (durable_turn_id, status): (String, String) = transaction
            .query_row(
                "SELECT turn_id, status FROM agent_task_runs WHERE id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Agent task run {run_id}"))
                }
                other => CoreError::Database(other),
            })?;
        if durable_turn_id != turn_id {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} changed turns while its pause was being committed"
            )));
        }
        if !matches!(status.as_str(), "queued" | "running" | "waiting_approval") {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} cannot be paused from status {status}"
            )));
        }
        if Database::agent_run_has_unresolved_interactions_on_connection(&transaction, run_id)? {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} established a required user-input barrier before pause commit"
            )));
        }
        let (durable_head, already_closed) =
            Database::agent_run_event_head_on_connection(&transaction, run_id)?;
        if already_closed || event_seq != durable_head.saturating_add(1) {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} changed at sequence {durable_head} while its pause was being committed"
            )));
        }

        let checkpoint =
            Database::insert_task_resume_checkpoint_on_connection(&transaction, &checkpoint)?;
        crate::db::insert_agent_run_events(&transaction, std::slice::from_ref(&event))?;
        Self::update_progress_from_event_on_connection(&transaction, run_id, &event)?;
        let turn_updated = transaction.execute(
            "UPDATE conversation_turns
             SET status = 'paused', finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1
               AND status NOT IN ('success', 'error', 'cancelled')",
            [turn_id],
        )?;
        if turn_updated != 1 {
            return Err(CoreError::Conflict(format!(
                "Conversation turn {turn_id} changed while its pause was being committed"
            )));
        }
        let snapshot = Database::get_agent_task_run_on_connection(&transaction, run_id)?;
        transaction.commit()?;
        Ok((checkpoint, event, snapshot))
    }

    /// Atomically claim fail-closed ownership for an outbox actor.
    ///
    /// The closure check and failed task projection share one immediate
    /// transaction. A durable terminal publisher can therefore win before
    /// this transaction, or this transaction can close the task first, but a
    /// stale actor can never overwrite a committed terminal projection.
    pub(crate) fn fail_run_event_outbox_if_open(
        &self,
        run_id: &str,
        failure_reason: &str,
    ) -> Result<AgentRunFailClosedOutcome, CoreError> {
        let failure_payload = serde_json::json!({ "reason": failure_reason });
        let mut connection = self.db.conn();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (event_seq, already_closed) =
            Database::agent_run_event_head_on_connection(&transaction, run_id)?;
        if already_closed {
            transaction.commit()?;
            return Ok(AgentRunFailClosedOutcome::AlreadyClosed { event_seq });
        }

        Database::project_agent_task_run_finished_on_connection(
            &transaction,
            run_id,
            TaskRunStatus::Failed.as_str(),
            Some("Run Event outbox failed closed"),
            Some(failure_reason),
            Some(&failure_payload),
        )?;
        let snapshot = Database::get_agent_task_run_on_connection(&transaction, run_id)?;
        if snapshot.status != TaskRunStatus::Failed.as_str() {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} closed while the outbox failure claim was in progress"
            )));
        }
        transaction.commit()?;
        Ok(AgentRunFailClosedOutcome::Claimed {
            event_seq,
            snapshot: Box::new(snapshot),
        })
    }

    /// Fail closed when a durable launch was committed but its Run Event
    /// outbox could not be opened before executor registration.
    pub fn fail_pre_executor_launch_if_open(
        &self,
        run_id: &str,
        failure_reason: &str,
    ) -> Result<bool, CoreError> {
        self.fail_run_event_outbox_if_open(run_id, failure_reason)
            .map(|outcome| matches!(outcome, AgentRunFailClosedOutcome::Claimed { .. }))
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

    fn update_progress_from_event_on_connection(
        connection: &rusqlite::Connection,
        run_id: &str,
        event: &AgentRunEvent,
    ) -> Result<bool, CoreError> {
        let emits_snapshot = !matches!(
            event.kind,
            AgentRunEventKind::OutputDelta
                | AgentRunEventKind::OutputSnapshot
                | AgentRunEventKind::Thinking
                | AgentRunEventKind::UsageUpdated
        );
        if event.kind == AgentRunEventKind::Status
            && event.phase == crate::agent_run::AgentRunPhase::AwaitingUserInput
            && !Database::agent_run_has_unresolved_interactions_on_connection(connection, run_id)?
        {
            // A response can atomically re-queue the run while the suspended
            // executor is still draining its buffered status event. That old
            // event must not move the resumed run back behind the barrier.
            return Ok(emits_snapshot);
        }
        let preserves_awaiting_status = !matches!(
            event.kind,
            AgentRunEventKind::Done | AgentRunEventKind::Error
        ) && !(event.kind == AgentRunEventKind::Status
            && (event.phase == crate::agent_run::AgentRunPhase::AwaitingUserInput
                || event.status.as_deref() == Some("cancelling")));
        if preserves_awaiting_status
            && Database::get_agent_task_run_on_connection(connection, run_id)?.status
                == TaskRunStatus::AwaitingUserInput.as_str()
        {
            return Ok(emits_snapshot);
        }

        let projection = match event.kind {
            AgentRunEventKind::ApprovalRequested => {
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(TaskRunStatus::WaitingApproval.as_str()),
                    Some("approval"),
                    None,
                    Some(&event.label),
                    None,
                    None,
                )
            }
            AgentRunEventKind::ApprovalResolved => {
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(TaskRunStatus::Running.as_str()),
                    Some("tooling"),
                    None,
                    Some(&event.label),
                    None,
                    None,
                )
            }
            AgentRunEventKind::ToolPreparing
            | AgentRunEventKind::ToolStarted
            | AgentRunEventKind::ToolProgress
            | AgentRunEventKind::ToolCompleted => {
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(TaskRunStatus::Running.as_str()),
                    Some("tooling"),
                    None,
                    Some(&event.label),
                    None,
                    None,
                )
            }
            AgentRunEventKind::PlanUpdated => {
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(TaskRunStatus::Running.as_str()),
                    Some(event.phase.as_str()),
                    None,
                    Some(&event.label),
                    event.payload.get("plan"),
                    None,
                )
            }
            AgentRunEventKind::Status => {
                let route_kind = event.label.strip_prefix("Route selected: ").map(str::trim);
                let status = match event.status.as_deref() {
                    Some("queued") => TaskRunStatus::Queued,
                    Some("cancelling") => TaskRunStatus::Cancelling,
                    _ => match event.phase {
                        crate::agent_run::AgentRunPhase::AwaitingUserInput => {
                            TaskRunStatus::AwaitingUserInput
                        }
                        crate::agent_run::AgentRunPhase::Paused => TaskRunStatus::Paused,
                        _ => TaskRunStatus::Running,
                    },
                };
                let artifacts = (event.phase == crate::agent_run::AgentRunPhase::Paused)
                    .then_some(&event.payload);
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(status.as_str()),
                    Some(event.phase.as_str()),
                    route_kind,
                    Some(&event.label),
                    None,
                    artifacts,
                )
            }
            AgentRunEventKind::RecoveryAttempt => {
                Database::project_agent_task_run_progress_on_connection(
                    connection,
                    run_id,
                    Some(TaskRunStatus::Running.as_str()),
                    Some("recovering"),
                    None,
                    Some(&event.label),
                    None,
                    None,
                )
            }
            AgentRunEventKind::Done => {
                let (status, summary) = match event.status.as_deref() {
                    Some("cancelled") => (TaskRunStatus::Cancelled, "Agent execution cancelled"),
                    Some("timed_out") => (TaskRunStatus::TimedOut, "Agent execution timed out"),
                    Some("paused") => (TaskRunStatus::Paused, event.label.as_str()),
                    _ => (TaskRunStatus::Completed, event.label.as_str()),
                };
                Database::project_agent_task_run_finished_on_connection(
                    connection,
                    run_id,
                    status.as_str(),
                    Some(summary),
                    None,
                    None,
                )
            }
            AgentRunEventKind::Error => {
                let (status, summary) = match event.status.as_deref() {
                    Some("cancelled") => (TaskRunStatus::Cancelled, "Agent execution cancelled"),
                    Some("timed_out") => (TaskRunStatus::TimedOut, "Agent execution timed out"),
                    _ => (TaskRunStatus::Failed, "Agent execution failed"),
                };
                Database::project_agent_task_run_finished_on_connection(
                    connection,
                    run_id,
                    status.as_str(),
                    Some(summary),
                    Some(&event.label),
                    None,
                )
            }
            AgentRunEventKind::OutputDelta
            | AgentRunEventKind::OutputSnapshot
            | AgentRunEventKind::StreamReset
            | AgentRunEventKind::Thinking
            | AgentRunEventKind::UsageUpdated
            | AgentRunEventKind::AutoCompacted => Ok(()),
        };
        projection?;
        Ok(emits_snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::agent_run::AgentRunEvent;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::{Message, Role, Usage};

    fn create_run(db: &Database, suffix: &str, start: bool) -> (String, String) {
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
        if start {
            runtime.start_run(&run.id, "routing").unwrap();
        }
        (run.id, turn.id)
    }

    fn create_started_run(db: &Database, suffix: &str) -> (String, String) {
        create_run(db, suffix, true)
    }

    #[test]
    fn terminal_commit_and_fail_closed_claim_have_one_transaction_winner() {
        let db = Database::open_memory().unwrap();

        for attempt in 0..32 {
            let (run_id, turn_id) = create_started_run(&db, &format!("atomic-race-{attempt}"));
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let terminal = AgentRunEvent::terminal_status(
                &run_id,
                Some(&turn_id),
                1,
                "Completed",
                "completed",
                None,
            );

            let terminal_commit = {
                let db = db.clone();
                let run_id = run_id.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    AgentTaskRuntime::new(&db).commit_run_event_batch(&run_id, &[terminal])
                })
            };
            let failure_claim = {
                let db = db.clone();
                let run_id = run_id.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    AgentTaskRuntime::new(&db)
                        .fail_run_event_outbox_if_open(&run_id, "run_event_persistence_failed")
                })
            };
            barrier.wait();

            let terminal_result = terminal_commit.join().expect("terminal commit thread");
            let failure_result = failure_claim.join().expect("failure claim thread");
            let events = db.list_agent_run_events(&run_id).expect("run event ledger");
            let task = db.get_agent_task_run(&run_id).expect("task projection");

            match (terminal_result, failure_result) {
                (
                    Ok(_),
                    Ok(AgentRunFailClosedOutcome::AlreadyClosed { event_seq: 1 }),
                ) => {
                    assert_eq!(events.len(), 1);
                    assert_eq!(task.status, "completed");
                }
                (
                    Err(CoreError::Conflict(_)),
                    Ok(AgentRunFailClosedOutcome::Claimed {
                        event_seq: 0,
                        snapshot,
                    }),
                ) => {
                    assert!(events.is_empty());
                    assert_eq!(snapshot.status, "failed");
                    assert_eq!(task.status, "failed");
                    assert_eq!(
                        task.error_message.as_deref(),
                        Some("run_event_persistence_failed")
                    );
                }
                (terminal_result, failure_result) => panic!(
                    "unexpected transaction race result: terminal={terminal_result:?}, failure={failure_result:?}"
                ),
            }
        }
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

        let route_event = AgentRunEvent::from_agent_event(&AgentEvent::ControllerStatus {
            code: "route_selected".to_string(),
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

    #[test]
    fn runtime_preserves_authoritative_terminal_status_and_summary() {
        let db = Database::open_memory().unwrap();
        let runtime = AgentTaskRuntime::new(&db);

        let (direct_pause_run_id, direct_pause_turn_id) = create_started_run(&db, "direct-pause");
        let direct_pause = AgentRunEvent::terminal_status(
            &direct_pause_run_id,
            Some(&direct_pause_turn_id),
            1,
            "Paused with a resumable checkpoint",
            "paused",
            None,
        );
        let run = runtime
            .apply_run_event(&direct_pause_run_id, &direct_pause)
            .unwrap();
        assert_eq!(run.status, TaskRunStatus::Paused.as_str());

        let (paused_run_id, paused_turn_id) = create_started_run(&db, "paused-authority");
        let checkpoint = serde_json::json!({ "checkpointId": "checkpoint-1" });
        runtime
            .finish_run(
                &paused_run_id,
                TaskRunStatus::Paused,
                Some("Paused with a resumable checkpoint"),
                None,
                Some(&checkpoint),
            )
            .unwrap();
        let stale_status = AgentRunEvent::status_update(
            &paused_run_id,
            Some(&paused_turn_id),
            2,
            crate::agent_run::AgentRunPhase::Responding,
            "Pause checkpoint saved",
            Some("running"),
            None,
        );
        runtime
            .apply_run_event(&paused_run_id, &stale_status)
            .unwrap();
        let paused_terminal = AgentRunEvent::terminal_status(
            &paused_run_id,
            Some(&paused_turn_id),
            3,
            "Paused with a resumable checkpoint",
            "paused",
            Some(&checkpoint),
        );
        let run = runtime
            .apply_run_event(&paused_run_id, &paused_terminal)
            .unwrap();
        assert_eq!(run.status, TaskRunStatus::Paused.as_str());
        assert_eq!(
            run.summary.as_deref(),
            Some("Paused with a resumable checkpoint")
        );
        assert_eq!(run.artifacts.as_ref(), Some(&checkpoint));

        let (completed_run_id, completed_turn_id) = create_started_run(&db, "completed-authority");
        runtime
            .finish_run(
                &completed_run_id,
                TaskRunStatus::Completed,
                Some("Task completed with verification gap"),
                None,
                None,
            )
            .unwrap();
        let generic_done = AgentRunEvent::terminal_status(
            &completed_run_id,
            Some(&completed_turn_id),
            4,
            "Final answer produced",
            "completed",
            None,
        );
        let run = runtime
            .apply_run_event(&completed_run_id, &generic_done)
            .unwrap();
        assert_eq!(run.status, TaskRunStatus::Completed.as_str());
        assert_eq!(
            run.summary.as_deref(),
            Some("Task completed with verification gap")
        );
    }

    #[test]
    fn runtime_preserves_awaiting_status_until_resume_or_terminal_event() {
        let db = Database::open_memory().unwrap();
        let runtime = AgentTaskRuntime::new(&db);
        let (run_id, turn_id) = create_started_run(&db, "awaiting-tool-complete");
        db.update_agent_task_run_progress(
            &run_id,
            Some(TaskRunStatus::AwaitingUserInput.as_str()),
            Some("awaiting_user_input"),
            None,
            Some("Waiting for your answer"),
            None,
            None,
        )
        .unwrap();

        let tool_completed = AgentRunEvent::from_agent_event(&AgentEvent::ToolCallResult {
            call_id: "call-question".to_string(),
            tool_name: "request_user_input".to_string(),
            content: "Question requested".to_string(),
            is_error: false,
            artifacts: None,
        })
        .with_context(Some(&run_id), Some(&turn_id), Some(1));

        let run = runtime.apply_run_event(&run_id, &tool_completed).unwrap();
        assert_eq!(run.status, TaskRunStatus::AwaitingUserInput.as_str());
        assert_eq!(run.phase, "awaiting_user_input");
    }

    #[test]
    fn runtime_projects_queued_running_and_cancelling_statuses() {
        let db = Database::open_memory().unwrap();
        let runtime = AgentTaskRuntime::new(&db);
        let (run_id, turn_id) = create_run(&db, "status-projection", false);

        let queued = AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            1,
            crate::agent_run::AgentRunPhase::Routing,
            "Queued",
            Some("queued"),
            None,
        );
        let run = runtime.apply_run_event(&run_id, &queued).unwrap();
        assert_eq!(run.status, TaskRunStatus::Queued.as_str());
        assert!(run.started_at.is_none());

        let running = AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            2,
            crate::agent_run::AgentRunPhase::Responding,
            "Running",
            Some("running"),
            None,
        );
        let run = runtime.apply_run_event(&run_id, &running).unwrap();
        assert_eq!(run.status, TaskRunStatus::Running.as_str());
        assert!(run.started_at.is_some());

        let cancelling = AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            3,
            crate::agent_run::AgentRunPhase::Responding,
            "Cancelling",
            Some("cancelling"),
            None,
        );
        let run = runtime.apply_run_event(&run_id, &cancelling).unwrap();
        assert_eq!(run.status, TaskRunStatus::Cancelling.as_str());
    }
}
