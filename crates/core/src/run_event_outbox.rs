//! Durable, ordered publication for one Agent Run's Run Events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::{OptionalExtension, TransactionBehavior};

use crate::agent::CancellationToken;
use crate::agent_run::{AgentRunEvent, AgentRunEventKind};
use crate::conversation::AgentTaskRun;
use crate::db::Database;
use crate::db_executor::DatabaseExecutor;
use crate::error::CoreError;
use crate::task_run::{AgentRunFailClosedOutcome, AgentTaskRuntime};
use crate::workflow_automation::TaskResumeCheckpoint;

const RUN_EVENT_OUTBOX_CAPACITY: usize = 512;
const LIVE_JOURNAL_FLUSH_INTERVAL: Duration = Duration::from_millis(100);
const LIVE_JOURNAL_MAX_BATCH: usize = 32;
const AGENT_STARTED_LABEL: &str = "Agent started";

/// Host delivery seam invoked only after the durable ledger is committed.
pub trait AgentRunEventDelivery: Send + Sync + 'static {
    fn deliver_run_event(&self, conversation_id: &str, event: &AgentRunEvent);

    fn deliver_task_run_snapshot(&self, conversation_id: &str, snapshot: AgentTaskRun);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunEventOutboxCompletion {
    pub event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentRunEventOutboxFailure {
    #[error("Run Event persistence failed: {message}")]
    Persistence { message: String },
    #[error("Run Event outbox queue filled before another ordered event could be accepted")]
    QueueFull,
    #[error("Run Event outbox actor became unavailable before terminal commit")]
    ActorUnavailable,
}

impl AgentRunEventOutboxFailure {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::Persistence { .. } => "run_event_persistence_failed",
            Self::QueueFull => "run_event_queue_saturated",
            Self::ActorUnavailable => "run_event_outbox_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentRunEventOutboxFailureRequest {
    QueueFull,
}

enum AgentRunEventOutboxCommand {
    Event(AgentRunEvent),
    PauseCheckpoint {
        turn_id: String,
        reason: String,
        response:
            tokio::sync::oneshot::Sender<Result<TaskResumeCheckpoint, AgentRunEventOutboxFailure>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentRunEventSubmitError {
    #[error("Run Event outbox is already closed")]
    AlreadyClosed,
    #[error("Run Event producers must submit an unsequenced event, got {event_seq}")]
    SequencedEvent { event_seq: u64 },
    #[error("Run Event belongs to {actual_run_id}, expected {expected_run_id}")]
    RunMismatch {
        expected_run_id: String,
        actual_run_id: String,
    },
    #[error("Run Event producers cannot bypass the durable outbox")]
    EphemeralEvent,
    #[error("Run Event contract is invalid: {message}")]
    InvalidEvent { message: String },
    #[error("Run Event outbox queue is full; the run was failed closed")]
    QueueFull,
    #[error("Run Event outbox actor is unavailable")]
    ActorUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentRunEventOutboxOutcome {
    Open,
    TerminalCommitted(AgentRunEventOutboxCompletion),
    Failed(AgentRunEventOutboxFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentRunEventOutboxDurability {
    Committed(u64),
    Failed(AgentRunEventOutboxFailure),
}

/// Opens or reuses the single Run Event outbox for each Agent Run.
#[derive(Clone)]
pub struct AgentRunEventOutboxes {
    inner: Arc<AgentRunEventOutboxesInner>,
}

struct AgentRunEventOutboxesInner {
    database: DatabaseExecutor,
    delivery: Arc<dyn AgentRunEventDelivery>,
    active: tokio::sync::Mutex<HashMap<String, Weak<AgentRunEventOutbox>>>,
}

/// Result of reconciling incomplete Agent Runs before the host accepts new
/// launches. The Run Event ledger is authoritative; task and turn rows are
/// materialized projections of the selected durable boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentRunStartupRecovery {
    pub restored_suspensions: usize,
    pub repaired_terminals: usize,
    pub cancelled_runs: usize,
}

#[derive(Debug)]
struct StartupRecoveryPlan {
    recovery: AgentRunStartupRecovery,
    interrupted: Vec<InterruptedRun>,
}

#[derive(Debug, Clone)]
struct InterruptedRun {
    conversation_id: String,
    turn_id: String,
    run_id: String,
}

#[derive(Debug)]
struct DurableRunBoundary {
    event_seq: u64,
    kind: String,
    phase: String,
    label: String,
    status: Option<String>,
}

impl AgentRunEventOutboxes {
    pub fn new(database: DatabaseExecutor, delivery: Arc<dyn AgentRunEventDelivery>) -> Self {
        Self {
            inner: Arc::new(AgentRunEventOutboxesInner {
                database,
                delivery,
                active: tokio::sync::Mutex::new(HashMap::new()),
            }),
        }
    }

    pub async fn open(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Arc<AgentRunEventOutbox>, CoreError> {
        let mut active = self.inner.active.lock().await;
        active.retain(|_, outbox| outbox.strong_count() > 0);
        if let Some(outbox) = active.get(run_id).and_then(Weak::upgrade) {
            if outbox.conversation_id != conversation_id {
                return Err(CoreError::InvalidInput(format!(
                    "Agent Run {run_id} belongs to a different conversation"
                )));
            }
            return Ok(outbox);
        }

        let durable_run_id = run_id.to_string();
        let (initial_sequence, already_closed) = self
            .inner
            .database
            .write(move |database| database.agent_run_event_head(&durable_run_id))
            .await?
            .value;
        let (sender, receiver) = tokio::sync::mpsc::channel(RUN_EVENT_OUTBOX_CAPACITY);
        let terminal_submitted = Arc::new(Mutex::new(already_closed));
        let cancellation = CancellationToken::new();
        let initial_outcome = if already_closed {
            AgentRunEventOutboxOutcome::TerminalCommitted(AgentRunEventOutboxCompletion {
                event_seq: initial_sequence,
            })
        } else {
            AgentRunEventOutboxOutcome::Open
        };
        let (completion_sender, completion) = tokio::sync::watch::channel(initial_outcome);
        let (durability_sender, durability) =
            tokio::sync::watch::channel(AgentRunEventOutboxDurability::Committed(initial_sequence));
        let (failure_request, failure_requests) = tokio::sync::watch::channel(None);
        let outbox = Arc::new(AgentRunEventOutbox {
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sender,
            terminal_submitted: Arc::clone(&terminal_submitted),
            cancellation: cancellation.clone(),
            completion,
            accepted_high_water: Arc::new(AtomicU64::new(initial_sequence)),
            durability,
            failure_request,
        });
        if already_closed {
            drop(receiver);
            drop(completion_sender);
            drop(durability_sender);
            drop(failure_requests);
        } else {
            spawn_outbox_actor(
                self.inner.database.clone(),
                Arc::clone(&self.inner.delivery),
                conversation_id.to_string(),
                run_id.to_string(),
                Arc::clone(&outbox),
                initial_sequence,
                receiver,
                terminal_submitted,
                cancellation,
                completion_sender,
                durability_sender,
                failure_requests,
            );
        }
        active.insert(run_id.to_string(), Arc::downgrade(&outbox));
        Ok(outbox)
    }

    /// Reconcile Agent Runs left between durable lifecycle boundaries by a
    /// previous app process.
    ///
    /// Resumable pauses are restored from the ledger when no later committed
    /// `Agent started` marker exists. Existing true terminals repair stale
    /// projections in place. Every other active run is closed through its Run
    /// Event outbox, and its turn converges only after the terminal barrier.
    pub async fn recover_after_restart(&self) -> Result<AgentRunStartupRecovery, CoreError> {
        let plan = self
            .inner
            .database
            .write(build_startup_recovery_plan)
            .await?
            .value;
        let mut recovery = plan.recovery;

        for interrupted in plan.interrupted {
            let outbox = self
                .open(&interrupted.conversation_id, &interrupted.run_id)
                .await?;
            let submitted = match outbox.submit(AgentRunEvent::terminal_status(
                &interrupted.run_id,
                Some(&interrupted.turn_id),
                0,
                "Agent execution cancelled after app restart",
                "cancelled",
                Some(&serde_json::json!({
                    "reason": "app_restart_interrupted",
                })),
            )) {
                Ok(()) => true,
                Err(AgentRunEventSubmitError::AlreadyClosed) => false,
                Err(error) => {
                    return Err(CoreError::Internal(format!(
                        "Could not terminalize interrupted Agent Run {}: {error}",
                        interrupted.run_id
                    )))
                }
            };
            outbox.wait_for_terminal_commit().await.map_err(|error| {
                CoreError::Internal(format!(
                    "Interrupted Agent Run {} did not reach its durable terminal barrier: {error}",
                    interrupted.run_id
                ))
            })?;

            let run_id = interrupted.run_id.clone();
            self.inner
                .database
                .write(move |database| converge_terminal_projection(database, &run_id))
                .await?;
            if submitted {
                recovery.cancelled_runs += 1;
            }
        }

        Ok(recovery)
    }
}

fn build_startup_recovery_plan(database: &Database) -> Result<StartupRecoveryPlan, CoreError> {
    let mut connection = database.conn();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let candidates = {
        let mut statement = transaction.prepare(
            "SELECT task.conversation_id, task.turn_id, task.id, task.status, turn.status
             FROM agent_task_runs task
             JOIN conversation_turns turn ON turn.id = task.turn_id
             WHERE task.status IN ('queued', 'running', 'waiting_approval', 'cancelling')
                OR (task.status = 'paused' AND turn.status <> 'paused')
                OR (task.status = 'awaiting_user_input' AND turn.status <> 'awaiting_user_input')
                OR (
                     task.status IN ('paused', 'awaiting_user_input')
                     AND EXISTS (
                       SELECT 1 FROM agent_run_events terminal
                       WHERE terminal.run_id = task.id
                         AND terminal.kind IN ('done', 'error')
                         AND NOT (
                              terminal.kind = 'done'
                              AND COALESCE(terminal.status, '') = 'paused'
                         )
                     )
                )
                OR (task.status = 'completed' AND turn.status <> 'success')
                OR (task.status IN ('failed', 'timed_out') AND turn.status <> 'error')
                OR (task.status = 'cancelled' AND turn.status <> 'cancelled')
             ORDER BY task.created_at ASC, task.id ASC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    InterruptedRun {
                        conversation_id: row.get(0)?,
                        turn_id: row.get(1)?,
                        run_id: row.get(2)?,
                    },
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    let mut recovery = AgentRunStartupRecovery::default();
    let mut interrupted = Vec::new();
    for (candidate, task_status) in candidates {
        if let Some(terminal) = first_true_terminal(&transaction, &candidate.run_id)? {
            if materialize_terminal_boundary(&transaction, &candidate, &terminal)? {
                recovery.repaired_terminals += 1;
            }
            continue;
        }

        if is_terminal_task_status(&task_status) {
            materialize_turn_from_task_status(&transaction, &candidate.turn_id, &task_status)?;
            recovery.repaired_terminals += 1;
            continue;
        }

        if let Some(suspension) = latest_resumable_boundary(&transaction, &candidate.run_id)? {
            // Older stop flows durably marked the task as cancelling before
            // they had a canonical Run Event for that intent. Never let an
            // earlier suspension boundary undo that durable stop during an
            // upgrade restart.
            let resumption_invalidated = task_status == "cancelling"
                || transaction.query_row(
                    "SELECT EXISTS(
                    SELECT 1 FROM agent_run_events
                    WHERE run_id = ?1
                      AND event_seq > ?2
                      AND kind = 'status'
                      AND (
                           status = 'cancelling'
                           OR (
                                phase = 'routing'
                                AND status = 'running'
                                AND label = ?3
                           )
                      )
                 )",
                    rusqlite::params![&candidate.run_id, suspension.event_seq, AGENT_STARTED_LABEL],
                    |row| row.get(0),
                )?;
            if !resumption_invalidated {
                materialize_suspension_boundary(&transaction, &candidate, &suspension)?;
                recovery.restored_suspensions += 1;
                continue;
            }
        }

        interrupted.push(candidate);
    }

    transaction.commit()?;
    Ok(StartupRecoveryPlan {
        recovery,
        interrupted,
    })
}

fn first_true_terminal(
    connection: &rusqlite::Connection,
    run_id: &str,
) -> Result<Option<DurableRunBoundary>, CoreError> {
    connection
        .query_row(
            "SELECT event_seq, kind, phase, label, status
             FROM agent_run_events
             WHERE run_id = ?1
               AND kind IN ('done', 'error')
               AND NOT (kind = 'done' AND COALESCE(status, '') = 'paused')
             ORDER BY event_seq ASC
             LIMIT 1",
            [run_id],
            durable_boundary_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
}

fn latest_resumable_boundary(
    connection: &rusqlite::Connection,
    run_id: &str,
) -> Result<Option<DurableRunBoundary>, CoreError> {
    connection
        .query_row(
            "SELECT event_seq, kind, phase, label, status
             FROM agent_run_events
             WHERE run_id = ?1
               AND (
                    (kind = 'status' AND (
                         (phase = 'paused' AND status = 'paused')
                         OR (
                              phase = 'awaiting_user_input'
                              AND COALESCE(status, '') <> 'cancelling'
                         )
                    ))
                    OR (kind = 'done' AND status = 'paused')
               )
             ORDER BY event_seq DESC
             LIMIT 1",
            [run_id],
            durable_boundary_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
}

fn durable_boundary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableRunBoundary> {
    let event_seq = row.get::<_, i64>(0)?;
    let event_seq = u64::try_from(event_seq)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, event_seq))?;
    Ok(DurableRunBoundary {
        event_seq,
        kind: row.get(1)?,
        phase: row.get(2)?,
        label: row.get(3)?,
        status: row.get(4)?,
    })
}

fn materialize_suspension_boundary(
    connection: &rusqlite::Connection,
    run: &InterruptedRun,
    boundary: &DurableRunBoundary,
) -> Result<(), CoreError> {
    let suspension_status = if boundary.phase == "awaiting_user_input" {
        "awaiting_user_input"
    } else {
        "paused"
    };
    let task_updated = connection.execute(
        "UPDATE agent_task_runs
         SET status = ?2, phase = ?2, summary = ?3, error_message = NULL,
             finished_at = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![&run.run_id, suspension_status, &boundary.label],
    )?;
    let turn_updated = connection.execute(
        "UPDATE conversation_turns
         SET status = ?2, finished_at = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![&run.turn_id, suspension_status],
    )?;
    require_materialized_rows(run, task_updated, turn_updated)
}

fn materialize_terminal_boundary(
    connection: &rusqlite::Connection,
    run: &InterruptedRun,
    boundary: &DurableRunBoundary,
) -> Result<bool, CoreError> {
    let (task_status, turn_status, summary, error_message) = terminal_projection(
        boundary.kind.as_str(),
        boundary.status.as_deref(),
        &boundary.label,
    );
    let task_updated = connection.execute(
        "UPDATE agent_task_runs
         SET status = ?2, phase = 'done', summary = ?3, error_message = ?4,
             finished_at = COALESCE(finished_at, datetime('now')),
             updated_at = datetime('now')
         WHERE id = ?1
           AND (
                status IS NOT ?2
                OR phase <> 'done'
                OR summary IS NOT ?3
                OR error_message IS NOT ?4
                OR finished_at IS NULL
           )",
        rusqlite::params![&run.run_id, task_status, summary, error_message],
    )?;
    let turn_updated = connection.execute(
        "UPDATE conversation_turns
         SET status = ?2,
             finished_at = COALESCE(finished_at, datetime('now')),
             updated_at = datetime('now')
         WHERE id = ?1
           AND (status IS NOT ?2 OR finished_at IS NULL)",
        rusqlite::params![&run.turn_id, turn_status],
    )?;
    Ok(task_updated > 0 || turn_updated > 0)
}

fn materialize_turn_from_task_status(
    connection: &rusqlite::Connection,
    turn_id: &str,
    task_status: &str,
) -> Result<(), CoreError> {
    let turn_status = turn_status_for_task_status(task_status).ok_or_else(|| {
        CoreError::Internal(format!(
            "Agent task terminal status {task_status} has no turn projection"
        ))
    })?;
    let updated = connection.execute(
        "UPDATE conversation_turns
         SET status = ?2,
             finished_at = COALESCE(finished_at, datetime('now')),
             updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![turn_id, turn_status],
    )?;
    if updated != 1 {
        return Err(CoreError::NotFound(format!("Conversation turn {turn_id}")));
    }
    Ok(())
}

fn converge_terminal_projection(database: &Database, run_id: &str) -> Result<(), CoreError> {
    let mut connection = database.conn();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let run = transaction
        .query_row(
            "SELECT conversation_id, turn_id, id, status
             FROM agent_task_runs WHERE id = ?1",
            [run_id],
            |row| {
                Ok((
                    InterruptedRun {
                        conversation_id: row.get(0)?,
                        turn_id: row.get(1)?,
                        run_id: row.get(2)?,
                    },
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("Agent task run {run_id}")))?;
    if let Some(terminal) = first_true_terminal(&transaction, run_id)? {
        let _ = materialize_terminal_boundary(&transaction, &run.0, &terminal)?;
    } else if is_terminal_task_status(&run.1) {
        materialize_turn_from_task_status(&transaction, &run.0.turn_id, &run.1)?;
    } else {
        return Err(CoreError::Internal(format!(
            "Agent Run {run_id} crossed its terminal barrier without a terminal projection"
        )));
    }
    transaction.commit()?;
    Ok(())
}

fn terminal_projection<'a>(
    kind: &str,
    status: Option<&str>,
    label: &'a str,
) -> (&'static str, &'static str, &'a str, Option<&'a str>) {
    match (kind, status) {
        ("done", Some("cancelled")) => {
            ("cancelled", "cancelled", "Agent execution cancelled", None)
        }
        ("done", Some("timed_out")) => ("timed_out", "error", "Agent execution timed out", None),
        ("done", _) => ("completed", "success", label, None),
        ("error", Some("cancelled")) => (
            "cancelled",
            "cancelled",
            "Agent execution cancelled",
            Some(label),
        ),
        ("error", Some("timed_out")) => (
            "timed_out",
            "error",
            "Agent execution timed out",
            Some(label),
        ),
        ("error", _) => ("failed", "error", "Agent execution failed", Some(label)),
        _ => ("failed", "error", "Agent execution failed", Some(label)),
    }
}

fn turn_status_for_task_status(task_status: &str) -> Option<&'static str> {
    match task_status {
        "completed" => Some("success"),
        "failed" | "timed_out" => Some("error"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}

fn is_terminal_task_status(status: &str) -> bool {
    turn_status_for_task_status(status).is_some()
}

fn require_materialized_rows(
    run: &InterruptedRun,
    task_updated: usize,
    turn_updated: usize,
) -> Result<(), CoreError> {
    if task_updated != 1 {
        return Err(CoreError::NotFound(format!(
            "Agent task run {}",
            run.run_id
        )));
    }
    if turn_updated != 1 {
        return Err(CoreError::NotFound(format!(
            "Conversation turn {}",
            run.turn_id
        )));
    }
    Ok(())
}

/// Non-blocking producer interface for one Agent Run's ordered ledger.
#[derive(Clone)]
pub struct AgentRunEventOutbox {
    run_id: String,
    conversation_id: String,
    sender: tokio::sync::mpsc::Sender<AgentRunEventOutboxCommand>,
    terminal_submitted: Arc<Mutex<bool>>,
    cancellation: CancellationToken,
    completion: tokio::sync::watch::Receiver<AgentRunEventOutboxOutcome>,
    accepted_high_water: Arc<AtomicU64>,
    durability: tokio::sync::watch::Receiver<AgentRunEventOutboxDurability>,
    failure_request: tokio::sync::watch::Sender<Option<AgentRunEventOutboxFailureRequest>>,
}

impl AgentRunEventOutbox {
    pub fn submit(&self, event: AgentRunEvent) -> Result<(), AgentRunEventSubmitError> {
        if event.event_seq != 0 {
            return Err(AgentRunEventSubmitError::SequencedEvent {
                event_seq: event.event_seq,
            });
        }
        if event.run_id != self.run_id {
            return Err(AgentRunEventSubmitError::RunMismatch {
                expected_run_id: self.run_id.clone(),
                actual_run_id: event.run_id,
            });
        }
        if !event.is_durable() {
            return Err(AgentRunEventSubmitError::EphemeralEvent);
        }
        validate_producer_lifecycle(&event).map_err(|message| {
            AgentRunEventSubmitError::InvalidEvent {
                message: message.to_string(),
            }
        })?;
        let mut validated = event.clone();
        validated.event_seq = 1;
        validated.validate_durable_contract().map_err(|error| {
            AgentRunEventSubmitError::InvalidEvent {
                message: error.to_string(),
            }
        })?;
        let mut terminal_submitted = self
            .terminal_submitted
            .lock()
            .map_err(|_| AgentRunEventSubmitError::ActorUnavailable)?;
        if *terminal_submitted {
            return Err(AgentRunEventSubmitError::AlreadyClosed);
        }
        let terminal = event.closes_run();
        if let Err(error) = self
            .sender
            .try_send(AgentRunEventOutboxCommand::Event(event))
        {
            *terminal_submitted = true;
            self.cancellation.cancel();
            return Err(match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    self.failure_request
                        .send_replace(Some(AgentRunEventOutboxFailureRequest::QueueFull));
                    AgentRunEventSubmitError::QueueFull
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    AgentRunEventSubmitError::ActorUnavailable
                }
            });
        }
        let _ =
            self.accepted_high_water
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                    Some(current.saturating_add(1))
                });
        *terminal_submitted = terminal;
        Ok(())
    }

    /// Persist a resumable checkpoint through the same sequencer that owns
    /// this Run's event ledger. The returned checkpoint is visible only after
    /// its paused event and task/turn projections have committed together.
    pub async fn pause_with_checkpoint(
        &self,
        turn_id: &str,
        reason: &str,
    ) -> Result<TaskResumeCheckpoint, AgentRunEventOutboxFailure> {
        let result = self.enqueue_pause_checkpoint(turn_id, reason)?;
        result
            .await
            .map_err(|_| AgentRunEventOutboxFailure::ActorUnavailable)?
    }

    fn enqueue_pause_checkpoint(
        &self,
        turn_id: &str,
        reason: &str,
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<TaskResumeCheckpoint, AgentRunEventOutboxFailure>>,
        AgentRunEventOutboxFailure,
    > {
        let (response, result) = tokio::sync::oneshot::channel();
        let mut terminal_submitted = self
            .terminal_submitted
            .lock()
            .map_err(|_| AgentRunEventOutboxFailure::ActorUnavailable)?;
        if *terminal_submitted {
            return Err(AgentRunEventOutboxFailure::ActorUnavailable);
        }
        if let Err(error) = self
            .sender
            .try_send(AgentRunEventOutboxCommand::PauseCheckpoint {
                turn_id: turn_id.to_string(),
                reason: reason.to_string(),
                response,
            })
        {
            *terminal_submitted = true;
            self.cancellation.cancel();
            return Err(match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    self.failure_request
                        .send_replace(Some(AgentRunEventOutboxFailureRequest::QueueFull));
                    AgentRunEventOutboxFailure::QueueFull
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    AgentRunEventOutboxFailure::ActorUnavailable
                }
            });
        }
        let _ =
            self.accepted_high_water
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                    Some(current.saturating_add(1))
                });
        Ok(result)
    }

    pub fn is_closed_for_submission(&self) -> bool {
        self.terminal_submitted
            .lock()
            .map(|closed| *closed)
            .unwrap_or(true)
    }

    pub fn turn_cancellation_token(&self) -> CancellationToken {
        self.cancellation.child_token()
    }

    pub async fn flush(&self) -> Result<u64, AgentRunEventOutboxFailure> {
        let target = self.accepted_high_water.load(Ordering::SeqCst);
        let mut durability = self.durability.clone();
        loop {
            let progress = durability.borrow().clone();
            match progress {
                AgentRunEventOutboxDurability::Committed(sequence) if sequence >= target => {
                    return Ok(sequence);
                }
                AgentRunEventOutboxDurability::Committed(_) => durability
                    .changed()
                    .await
                    .map_err(|_| AgentRunEventOutboxFailure::ActorUnavailable)?,
                AgentRunEventOutboxDurability::Failed(failure) => return Err(failure),
            }
        }
    }

    pub async fn wait_for_terminal_commit(
        &self,
    ) -> Result<AgentRunEventOutboxCompletion, AgentRunEventOutboxFailure> {
        let mut completion = self.completion.clone();
        loop {
            let outcome = completion.borrow().clone();
            match outcome {
                AgentRunEventOutboxOutcome::Open => completion
                    .changed()
                    .await
                    .map_err(|_| AgentRunEventOutboxFailure::ActorUnavailable)?,
                AgentRunEventOutboxOutcome::TerminalCommitted(completion) => {
                    return Ok(completion);
                }
                AgentRunEventOutboxOutcome::Failed(failure) => return Err(failure),
            }
        }
    }
}

fn spawn_outbox_actor(
    database: DatabaseExecutor,
    delivery: Arc<dyn AgentRunEventDelivery>,
    conversation_id: String,
    run_id: String,
    outbox_lifetime: Arc<AgentRunEventOutbox>,
    initial_sequence: u64,
    mut receiver: tokio::sync::mpsc::Receiver<AgentRunEventOutboxCommand>,
    terminal_submitted: Arc<Mutex<bool>>,
    cancellation: CancellationToken,
    completion: tokio::sync::watch::Sender<AgentRunEventOutboxOutcome>,
    durability: tokio::sync::watch::Sender<AgentRunEventOutboxDurability>,
    mut failure_requests: tokio::sync::watch::Receiver<Option<AgentRunEventOutboxFailureRequest>>,
) {
    tokio::spawn(async move {
        // The registry holds only a Weak reference. The actor keeps the outbox
        // reachable for the whole open lifecycle, then releases it when a true
        // terminal or fail-closed outcome ends the actor.
        let _outbox_lifetime = outbox_lifetime;
        let mut sequence = initial_sequence;
        let mut last_turn_id = String::new();
        let mut pending = Vec::with_capacity(LIVE_JOURNAL_MAX_BATCH);
        let mut flush_tick = tokio::time::interval(LIVE_JOURNAL_FLUSH_INTERVAL);
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        flush_tick.tick().await;

        loop {
            tokio::select! {
                biased;
                failure_changed = failure_requests.changed() => {
                    if failure_changed.is_err() {
                        break;
                    }
                    let failure_request = *failure_requests.borrow_and_update();
                    let Some(AgentRunEventOutboxFailureRequest::QueueFull) = failure_request else {
                        continue;
                    };
                    receiver.close();
                    let mut drain_failure = None;
                    let mut rejected_pause_responses = Vec::new();
                    while let Some(command) = receiver.recv().await {
                        match command {
                            AgentRunEventOutboxCommand::Event(mut event) => {
                                sequence = sequence.saturating_add(1);
                                event.event_seq = sequence;
                                last_turn_id = event.turn_id.clone();
                                if let Err(error) = event.validate_durable_contract() {
                                    drain_failure = Some(CoreError::InvalidInput(format!(
                                        "invalid agent run event: {error}"
                                    )));
                                    break;
                                }
                                pending.push(event);
                                if pending.len() >= LIVE_JOURNAL_MAX_BATCH {
                                    if let Err(error) = commit_and_deliver(
                                        &database,
                                        delivery.as_ref(),
                                        &conversation_id,
                                        &run_id,
                                        &mut pending,
                                    ).await {
                                        drain_failure = Some(error);
                                        break;
                                    }
                                    durability.send_replace(
                                        AgentRunEventOutboxDurability::Committed(sequence),
                                    );
                                }
                            }
                            AgentRunEventOutboxCommand::PauseCheckpoint {
                                turn_id, response, ..
                            } => {
                                last_turn_id = turn_id;
                                // Once saturation is known, the run is doomed
                                // to fail closed. Do not create a checkpoint
                                // that could be mistaken for a resumable pause.
                                rejected_pause_responses.push(response);
                            }
                        }
                    }
                    if drain_failure.is_none() && !pending.is_empty() {
                        if let Err(error) = commit_and_deliver(
                            &database,
                            delivery.as_ref(),
                            &conversation_id,
                            &run_id,
                            &mut pending,
                        ).await {
                            drain_failure = Some(error);
                        } else {
                            durability.send_replace(
                                AgentRunEventOutboxDurability::Committed(sequence),
                            );
                        }
                    }
                    let failure = match drain_failure {
                        Some(error) => AgentRunEventOutboxFailure::Persistence {
                            message: error.to_string(),
                        },
                        None => AgentRunEventOutboxFailure::QueueFull,
                    };
                    fail_actor(
                        &database,
                        delivery.as_ref(),
                        &conversation_id,
                        &run_id,
                        &last_turn_id,
                        &terminal_submitted,
                        &cancellation,
                        &completion,
                        &durability,
                        failure.clone(),
                    ).await;
                    for response in rejected_pause_responses {
                        let _ = response.send(Err(failure.clone()));
                    }
                    break;
                }
                maybe_command = receiver.recv() => {
                    let Some(command) = maybe_command else {
                        let _ = commit_and_deliver(
                            &database,
                            delivery.as_ref(),
                            &conversation_id,
                            &run_id,
                            &mut pending,
                        ).await;
                        break;
                    };
                    match command {
                        AgentRunEventOutboxCommand::Event(mut event) => {
                            sequence = sequence.saturating_add(1);
                            event.event_seq = sequence;
                            last_turn_id = event.turn_id.clone();
                            if event.validate_durable_contract().is_err() {
                                break;
                            }
                            let deferred = is_deferred_event(&event);
                            let terminal = event.closes_run();
                            pending.push(event);
                            if !deferred || pending.len() >= LIVE_JOURNAL_MAX_BATCH {
                                if let Err(error) = commit_and_deliver(
                                    &database,
                                    delivery.as_ref(),
                                    &conversation_id,
                                    &run_id,
                                    &mut pending,
                                ).await {
                                    fail_actor(
                                        &database,
                                        delivery.as_ref(),
                                        &conversation_id,
                                        &run_id,
                                        &last_turn_id,
                                        &terminal_submitted,
                                        &cancellation,
                                        &completion,
                                        &durability,
                                        AgentRunEventOutboxFailure::Persistence {
                                            message: error.to_string(),
                                        },
                                    ).await;
                                    break;
                                }
                                durability.send_replace(
                                    AgentRunEventOutboxDurability::Committed(sequence),
                                );
                            }
                            if terminal {
                                completion.send_replace(
                                    AgentRunEventOutboxOutcome::TerminalCommitted(
                                        AgentRunEventOutboxCompletion { event_seq: sequence },
                                    ),
                                );
                                break;
                            }
                        }
                        AgentRunEventOutboxCommand::PauseCheckpoint {
                            turn_id,
                            reason,
                            response,
                        } => {
                            last_turn_id = turn_id.clone();
                            if !pending.is_empty() {
                                if let Err(error) = commit_and_deliver(
                                    &database,
                                    delivery.as_ref(),
                                    &conversation_id,
                                    &run_id,
                                    &mut pending,
                                ).await {
                                    let failure = AgentRunEventOutboxFailure::Persistence {
                                        message: error.to_string(),
                                    };
                                    fail_actor(
                                        &database,
                                        delivery.as_ref(),
                                        &conversation_id,
                                        &run_id,
                                        &last_turn_id,
                                        &terminal_submitted,
                                        &cancellation,
                                        &completion,
                                        &durability,
                                        failure.clone(),
                                    ).await;
                                    let _ = response.send(Err(failure));
                                    break;
                                }
                                durability.send_replace(
                                    AgentRunEventOutboxDurability::Committed(sequence),
                                );
                            }
                            sequence = sequence.saturating_add(1);
                            match commit_pause_checkpoint_and_deliver(
                                &database,
                                delivery.as_ref(),
                                &conversation_id,
                                &run_id,
                                &turn_id,
                                sequence,
                                &reason,
                            ).await {
                                Ok(checkpoint) => {
                                    durability.send_replace(
                                        AgentRunEventOutboxDurability::Committed(sequence),
                                    );
                                    let _ = response.send(Ok(checkpoint));
                                }
                                Err(error) => {
                                    let failure = AgentRunEventOutboxFailure::Persistence {
                                        message: error.to_string(),
                                    };
                                    fail_actor(
                                        &database,
                                        delivery.as_ref(),
                                        &conversation_id,
                                        &run_id,
                                        &last_turn_id,
                                        &terminal_submitted,
                                        &cancellation,
                                        &completion,
                                        &durability,
                                        failure.clone(),
                                    ).await;
                                    let _ = response.send(Err(failure));
                                    break;
                                }
                            }
                        }
                    }
                }
                _ = flush_tick.tick(), if !pending.is_empty() => {
                    if let Err(error) = commit_and_deliver(
                        &database,
                        delivery.as_ref(),
                        &conversation_id,
                        &run_id,
                        &mut pending,
                    ).await {
                        fail_actor(
                            &database,
                            delivery.as_ref(),
                            &conversation_id,
                            &run_id,
                            &last_turn_id,
                            &terminal_submitted,
                            &cancellation,
                            &completion,
                            &durability,
                            AgentRunEventOutboxFailure::Persistence {
                                message: error.to_string(),
                            },
                        ).await;
                        break;
                    }
                    durability
                        .send_replace(AgentRunEventOutboxDurability::Committed(sequence));
                }
            }
        }
    });
}

async fn fail_actor(
    database: &DatabaseExecutor,
    delivery: &dyn AgentRunEventDelivery,
    conversation_id: &str,
    run_id: &str,
    turn_id: &str,
    terminal_submitted: &Mutex<bool>,
    cancellation: &CancellationToken,
    completion: &tokio::sync::watch::Sender<AgentRunEventOutboxOutcome>,
    durability: &tokio::sync::watch::Sender<AgentRunEventOutboxDurability>,
    failure: AgentRunEventOutboxFailure,
) {
    if let Ok(mut closed) = terminal_submitted.lock() {
        *closed = true;
    }
    cancellation.cancel();

    let failure_reason = failure.reason_code();
    let task_run_id = run_id.to_string();
    let failure_claim = database
        .write(move |database| {
            AgentTaskRuntime::new(database)
                .fail_run_event_outbox_if_open(&task_run_id, failure_reason)
        })
        .await;
    let (durable_head, task_snapshot) = match failure_claim {
        Ok(execution) => match execution.value {
            AgentRunFailClosedOutcome::Claimed {
                event_seq,
                snapshot,
            } => (event_seq, snapshot),
            AgentRunFailClosedOutcome::AlreadyClosed { event_seq } => {
                // Another actor won the database transaction. Its durable
                // terminal (or fail-closed task projection) is authoritative;
                // this stale actor must not publish a second live terminal.
                durability.send_replace(AgentRunEventOutboxDurability::Committed(event_seq));
                completion.send_replace(AgentRunEventOutboxOutcome::TerminalCommitted(
                    AgentRunEventOutboxCompletion { event_seq },
                ));
                return;
            }
        },
        Err(_) => {
            // Without a successful transactional claim we cannot prove that a
            // competing terminal publisher did not win. Fail the waiters but
            // avoid an unowned second ephemeral terminal.
            durability.send_replace(AgentRunEventOutboxDurability::Failed(failure.clone()));
            completion.send_replace(AgentRunEventOutboxOutcome::Failed(failure));
            return;
        }
    };
    let terminal_sequence = durable_head.saturating_add(1);

    let mut terminal = AgentRunEvent::terminal_error(
        run_id,
        Some(turn_id),
        terminal_sequence,
        "The response stream could not be stored safely. Retry this message.",
        "failed",
        Some(&serde_json::json!({ "reason": failure_reason })),
    );
    terminal.persistence = crate::agent_run::AgentRunEventPersistence::Ephemeral;
    delivery.deliver_run_event(conversation_id, &terminal);
    delivery.deliver_task_run_snapshot(conversation_id, task_snapshot);
    durability.send_replace(AgentRunEventOutboxDurability::Failed(failure.clone()));
    completion.send_replace(AgentRunEventOutboxOutcome::Failed(failure));
}

async fn commit_and_deliver(
    database: &DatabaseExecutor,
    delivery: &dyn AgentRunEventDelivery,
    conversation_id: &str,
    run_id: &str,
    pending: &mut Vec<AgentRunEvent>,
) -> Result<(), CoreError> {
    if pending.is_empty() {
        return Ok(());
    }
    let events = std::mem::take(pending);
    let durable_events = events.clone();
    let durable_run_id = run_id.to_string();
    let snapshots = database
        .write(move |database| {
            let runtime = AgentTaskRuntime::new(database);
            runtime.commit_run_event_batch(&durable_run_id, &durable_events)
        })
        .await?
        .value;

    for event in &events {
        delivery.deliver_run_event(conversation_id, event);
    }
    for snapshot in snapshots {
        delivery.deliver_task_run_snapshot(conversation_id, snapshot);
    }
    Ok(())
}

async fn commit_pause_checkpoint_and_deliver(
    database: &DatabaseExecutor,
    delivery: &dyn AgentRunEventDelivery,
    conversation_id: &str,
    run_id: &str,
    turn_id: &str,
    event_seq: u64,
    reason: &str,
) -> Result<TaskResumeCheckpoint, CoreError> {
    let durable_run_id = run_id.to_string();
    let durable_turn_id = turn_id.to_string();
    let durable_reason = reason.to_string();
    let (checkpoint, event, snapshot) = database
        .write(move |database| {
            AgentTaskRuntime::new(database).commit_pause_checkpoint(
                &durable_run_id,
                &durable_turn_id,
                event_seq,
                &durable_reason,
            )
        })
        .await?
        .value;

    delivery.deliver_run_event(conversation_id, &event);
    delivery.deliver_task_run_snapshot(conversation_id, snapshot);
    Ok(checkpoint)
}

fn is_deferred_event(event: &AgentRunEvent) -> bool {
    matches!(
        event.kind,
        AgentRunEventKind::OutputDelta
            | AgentRunEventKind::Thinking
            | AgentRunEventKind::UsageUpdated
    )
}

fn validate_producer_lifecycle(event: &AgentRunEvent) -> Result<(), &'static str> {
    match event.kind {
        AgentRunEventKind::Done
            if !matches!(
                event.status.as_deref(),
                Some("completed" | "cancelled" | "timed_out")
            ) =>
        {
            Err("done events require completed, cancelled, or timed_out status")
        }
        AgentRunEventKind::Error
            if !matches!(
                event.status.as_deref(),
                Some("failed" | "cancelled" | "timed_out")
            ) =>
        {
            Err("error events require failed, cancelled, or timed_out status")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_run::{AgentRunEventPersistence, AgentRunPhase};
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::db::Database;
    use crate::llm::Role;

    #[derive(Default)]
    struct CaptureDelivery {
        events: Mutex<Vec<AgentRunEvent>>,
    }

    impl AgentRunEventDelivery for CaptureDelivery {
        fn deliver_run_event(&self, _conversation_id: &str, event: &AgentRunEvent) {
            self.events
                .lock()
                .expect("capture lock")
                .push(event.clone());
        }

        fn deliver_task_run_snapshot(&self, _conversation_id: &str, _snapshot: AgentTaskRun) {}
    }

    fn create_started_run(database: &Database) -> (String, String, String) {
        let conversation = database
            .create_conversation(&CreateConversationInput {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("conversation");
        let message = ConversationMessage {
            id: "message-1".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Run the task".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        database.add_message(&message).expect("user message");
        let turn = database
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("conversation turn");
        let run = database
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &message.id,
                "Run the task",
                Some("test"),
                Some("test-model"),
            )
            .expect("task run");
        database
            .mark_agent_task_run_started(&run.id, "responding")
            .expect("started task run");
        (conversation.id, turn.id, run.id)
    }

    #[tokio::test]
    async fn persistence_failure_fails_closed_with_one_ephemeral_terminal() {
        let database = Database::open_memory().expect("in-memory database");
        let (conversation_id, turn_id, run_id) = create_started_run(&database);
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_run_event_insert
                 BEFORE INSERT ON agent_run_events
                 BEGIN
                   SELECT RAISE(FAIL, 'forced run event persistence failure');
                 END;",
            )
            .expect("failure trigger");
        let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
        let delivery = Arc::new(CaptureDelivery::default());
        let outboxes = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
        let outbox = outboxes
            .open(&conversation_id, &run_id)
            .await
            .expect("run outbox");
        let cancellation = outbox.turn_cancellation_token();

        outbox
            .submit(AgentRunEvent::status_update(
                &run_id,
                Some(&turn_id),
                0,
                AgentRunPhase::Routing,
                "Routing",
                Some("running"),
                None,
            ))
            .expect("queued status event");
        assert!(matches!(
            outbox.wait_for_terminal_commit().await,
            Err(AgentRunEventOutboxFailure::Persistence { .. })
        ));

        assert!(cancellation.is_cancelled());
        assert_eq!(
            outbox.submit(AgentRunEvent::status_update(
                &run_id,
                Some(&turn_id),
                0,
                AgentRunPhase::Responding,
                "late",
                Some("running"),
                None,
            )),
            Err(AgentRunEventSubmitError::AlreadyClosed)
        );
        let events = delivery.events.lock().expect("capture lock").clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].persistence, AgentRunEventPersistence::Ephemeral);
        assert!(events[0].is_terminal());
        assert_eq!(events[0].event_seq, 1);
        assert_eq!(events[0].payload["reason"], "run_event_persistence_failed");
        let failed_run = database
            .get_agent_task_run(&run_id)
            .expect("failed task run");
        assert_eq!(failed_run.status, "failed");
        assert_eq!(
            failed_run.error_message.as_deref(),
            Some("run_event_persistence_failed")
        );

        drop(outbox);
        drop(outboxes);
        let restarted = AgentRunEventOutboxes::new(executor, delivery);
        let reopened = restarted
            .open(&conversation_id, &run_id)
            .await
            .expect("failed run outbox restores as closed");
        assert!(reopened.is_closed_for_submission());
        assert!(matches!(
            reopened.submit(AgentRunEvent::status_update(
                &run_id,
                Some(&turn_id),
                0,
                AgentRunPhase::Responding,
                "late after restart",
                Some("running"),
                None,
            )),
            Err(AgentRunEventSubmitError::AlreadyClosed)
        ));
    }

    #[tokio::test]
    async fn task_projection_failure_rolls_back_the_inserted_run_event_batch() {
        let database = Database::open_memory().expect("in-memory database");
        let (conversation_id, turn_id, run_id) = create_started_run(&database);
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_task_projection
                 BEFORE UPDATE ON agent_task_runs
                 WHEN NEW.summary = 'Projection must fail'
                 BEGIN
                   SELECT RAISE(FAIL, 'forced task projection failure');
                 END;",
            )
            .expect("projection failure trigger");
        let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
        let delivery = Arc::new(CaptureDelivery::default());
        let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
        let outbox = outboxes
            .open(&conversation_id, &run_id)
            .await
            .expect("run outbox");
        let cancellation = outbox.turn_cancellation_token();

        outbox
            .submit(AgentRunEvent::status_update(
                &run_id,
                Some(&turn_id),
                0,
                AgentRunPhase::Routing,
                "Projection must fail",
                Some("running"),
                None,
            ))
            .expect("queued semantic event");

        assert!(matches!(
            outbox.wait_for_terminal_commit().await,
            Err(AgentRunEventOutboxFailure::Persistence { .. })
        ));
        assert!(cancellation.is_cancelled());
        assert!(
            database
                .list_agent_run_events(&run_id)
                .expect("rolled-back run event ledger")
                .is_empty(),
            "the event insert must roll back with its failed task projection"
        );

        let events = delivery.events.lock().expect("capture lock").clone();
        assert_eq!(events.len(), 1);
        assert!(events[0].is_terminal());
        assert_eq!(events[0].event_seq, 1);
        assert_eq!(events[0].persistence, AgentRunEventPersistence::Ephemeral);
        assert_eq!(events[0].payload["reason"], "run_event_persistence_failed");

        let failed_run = database
            .get_agent_task_run(&run_id)
            .expect("best-effort failed task projection");
        assert_eq!(failed_run.status, "failed");
        assert_eq!(
            failed_run.error_message.as_deref(),
            Some("run_event_persistence_failed")
        );
    }

    #[tokio::test]
    async fn pause_projection_failure_rolls_back_checkpoint_and_run_event_together() {
        let database = Database::open_memory().expect("in-memory database");
        let (conversation_id, turn_id, run_id) = create_started_run(&database);
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_pause_turn_projection
                 BEFORE UPDATE ON conversation_turns
                 WHEN NEW.status = 'paused'
                 BEGIN
                   SELECT RAISE(FAIL, 'forced pause turn projection failure');
                 END;",
            )
            .expect("pause boundary failure trigger");
        let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
        let delivery = Arc::new(CaptureDelivery::default());
        let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
        let outbox = outboxes
            .open(&conversation_id, &run_id)
            .await
            .expect("run outbox");

        assert!(outbox
            .pause_with_checkpoint(&turn_id, "user_pause")
            .await
            .is_err());

        assert!(database
            .list_task_resume_checkpoints(&run_id)
            .expect("rolled-back checkpoints")
            .is_empty());
        assert!(database
            .list_agent_run_events(&run_id)
            .expect("rolled-back run ledger")
            .is_empty());
        assert_eq!(
            database
                .get_agent_task_run(&run_id)
                .expect("fail-closed task")
                .status,
            "failed"
        );
        assert_eq!(
            database
                .get_conversation_turn(&turn_id)
                .expect("uncommitted turn projection")
                .status,
            "running"
        );
        let delivered = delivery.events.lock().expect("capture lock").clone();
        assert_eq!(delivered.len(), 1);
        assert!(delivered[0].is_terminal());
        assert_eq!(
            delivered[0].persistence,
            AgentRunEventPersistence::Ephemeral
        );
        assert_ne!(delivered[0].phase, AgentRunPhase::Paused);
    }
}
