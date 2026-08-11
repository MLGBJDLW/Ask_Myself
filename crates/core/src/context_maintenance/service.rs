use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::activity::{
    ActivityEventKind, ActivityObservation, ActivityRuntime, ActivitySpec, ActivityState,
    ActivitySurface,
};
use crate::conversation::memory::estimate_tokens_for_model;
use crate::conversation::summarizer::{
    summarize_evicted_messages_with_controls, ControlledSummarization, SummarizationControlPolicy,
};
use crate::db_executor::{DatabaseExecutionMetrics, DatabaseExecutor};
use crate::error::CoreError;

use super::model::{
    ContextCheckpointInput, ContextCompactionHandle, ContextCompactionJob, ContextCompactionPhase,
    ContextCompactionResult,
};
use super::planner::{plan_compaction, PlanOutcome};
use super::store::{commit_context_checkpoint, load_compaction_snapshot, CommitOutcome};

const OWNER: &str = "context_compaction";

#[derive(Clone)]
pub struct ContextCompactionService {
    inner: Arc<ContextCompactionServiceInner>,
}

struct ContextCompactionServiceInner {
    database: DatabaseExecutor,
    activities: ActivityRuntime,
    active: Mutex<HashMap<String, ActiveCompaction>>,
}

struct ActiveCompaction {
    operation_id: String,
    cancellation: CancellationToken,
}

impl ContextCompactionService {
    pub fn new(database: DatabaseExecutor, activities: ActivityRuntime) -> Self {
        Self {
            inner: Arc::new(ContextCompactionServiceInner {
                database,
                activities,
                active: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn activities(&self) -> &ActivityRuntime {
        &self.inner.activities
    }

    pub async fn start(
        &self,
        mut job: ContextCompactionJob,
    ) -> Result<ContextCompactionHandle, CoreError> {
        job.request.policy = job.request.policy.normalized();
        if job.request.conversation_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "conversation_id cannot be empty".to_string(),
            ));
        }
        if job.request.idempotency_key.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "idempotency_key cannot be empty".to_string(),
            ));
        }
        let operation_id = operation_id(&job.request.conversation_id, &job.request.idempotency_key);
        if let Some(existing) = self.inner.activities.get(&operation_id) {
            return Ok(ContextCompactionHandle {
                operation_id,
                conversation_id: job.request.conversation_id,
                snapshot_version: job.snapshot_version,
                state: existing.state,
                phase: ContextCompactionPhase::Queued,
            });
        }

        let cancellation = CancellationToken::new();
        {
            let mut active = self.inner.active.lock().await;
            if let Some(existing) = active.get(&job.request.conversation_id) {
                return Err(CoreError::InvalidInput(format!(
                    "Conversation already has active context operation {}",
                    existing.operation_id
                )));
            }
            active.insert(
                job.request.conversation_id.clone(),
                ActiveCompaction {
                    operation_id: operation_id.clone(),
                    cancellation: cancellation.clone(),
                },
            );
        }

        let spec = ActivitySpec::new(ActivitySurface::Maintenance, OWNER)
            .with_activity_id(&operation_id)
            .with_session_id(&job.request.conversation_id)
            .with_conversation_id(&job.request.conversation_id);
        if let Err(error) = self.inner.activities.start(spec) {
            self.release(&job.request.conversation_id, &operation_id)
                .await;
            return Err(error);
        }
        self.progress(
            &operation_id,
            ContextCompactionPhase::Queued,
            0.0,
            serde_json::json!({ "eventKind": "operationStarted" }),
        )?;

        let service = self.clone();
        let operation_id_for_task = operation_id.clone();
        let conversation_id_for_task = job.request.conversation_id.clone();
        let conversation_id_for_handle = job.request.conversation_id.clone();
        let snapshot_version_for_handle = job.snapshot_version.clone();
        tokio::spawn(async move {
            let result = service.run(&operation_id_for_task, cancellation, job).await;
            service
                .finish(&operation_id_for_task, result)
                .unwrap_or_else(|error| {
                    tracing::error!(
                        operation_id = %operation_id_for_task,
                        "Failed to finalize context compaction: {error}"
                    );
                });
            service
                .release(&conversation_id_for_task, &operation_id_for_task)
                .await;
        });

        Ok(ContextCompactionHandle {
            operation_id,
            conversation_id: conversation_id_for_handle,
            snapshot_version: snapshot_version_for_handle,
            state: ActivityState::Running,
            phase: ContextCompactionPhase::Queued,
        })
    }

    pub async fn cancel(&self, operation_id: &str, reason: &str) -> Result<(), CoreError> {
        let active = self.inner.active.lock().await;
        let Some(operation) = active
            .values()
            .find(|operation| operation.operation_id == operation_id)
        else {
            let record = self
                .inner
                .activities
                .get(operation_id)
                .ok_or_else(|| CoreError::NotFound(format!("Operation {operation_id}")))?;
            return if record.state.is_terminal() {
                Ok(())
            } else {
                Err(CoreError::InvalidInput(
                    "Operation is not owned by this process".to_string(),
                ))
            };
        };
        self.inner.activities.transition(
            operation_id,
            ActivityState::Cancelling,
            serde_json::json!({ "reason": reason, "eventKind": "cancellationRequested" }),
        )?;
        operation.cancellation.cancel();
        Ok(())
    }

    pub async fn observe(
        &self,
        operation_id: &str,
        after_seq: u64,
        wait_up_to: Duration,
    ) -> Result<ActivityObservation, CoreError> {
        self.inner
            .activities
            .observe(operation_id, after_seq, wait_up_to)
            .await
    }

    async fn run(
        &self,
        operation_id: &str,
        cancellation: CancellationToken,
        job: ContextCompactionJob,
    ) -> Result<ContextCompactionResult, OperationError> {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(job.request.policy.total_deadline_ms);
        self.progress(
            operation_id,
            ContextCompactionPhase::Planning,
            0.1,
            serde_json::json!({ "eventKind": "operationProgress" }),
        )?;
        let conversation_id = job.request.conversation_id.clone();
        let loaded = self
            .inner
            .database
            .read(move |database| {
                if database.conversation_has_active_agent_task_run(&conversation_id)? {
                    return Err(CoreError::InvalidInput(
                        "Wait for the active response to finish before compacting this conversation"
                            .to_string(),
                    ));
                }
                load_compaction_snapshot(database, &conversation_id)
            })
            .await?;
        self.database_metrics(operation_id, "compact.load_history", loaded.metrics)?;
        ensure_not_cancelled(&cancellation)?;

        let model_for_plan = job.model.clone();
        let context_window = job.context_window;
        let max_response_tokens = job.max_response_tokens;
        let plan_started = Instant::now();
        let plan = tokio::task::spawn_blocking(move || {
            plan_compaction(
                loaded.value.messages,
                &model_for_plan,
                context_window,
                max_response_tokens,
                loaded.value.checkpoint_generation,
            )
        })
        .await
        .map_err(|error| CoreError::Internal(format!("Compaction planner failed: {error}")))?;
        self.progress(
            operation_id,
            ContextCompactionPhase::Planning,
            0.3,
            serde_json::json!({
                "eventKind": "operationProgress",
                "span": "compact.plan_boundary",
                "elapsedMs": duration_ms(plan_started.elapsed()),
            }),
        )?;
        ensure_not_cancelled(&cancellation)?;

        let PlanOutcome::Planned(plan) = plan else {
            let PlanOutcome::Noop {
                messages_before,
                tokens_before,
            } = plan
            else {
                unreachable!()
            };
            return Ok(ContextCompactionResult {
                conversation_id: job.request.conversation_id,
                checkpoint_id: None,
                messages_before,
                messages_after: messages_before,
                tokens_before,
                tokens_after: tokens_before,
                evicted_messages: 0,
                summary_kind: "unchanged".to_string(),
                fallback_reason: None,
            });
        };

        self.progress(
            operation_id,
            ContextCompactionPhase::Summarizing,
            0.45,
            serde_json::json!({ "eventKind": "providerAttemptStarted" }),
        )?;
        let summary = summarize_evicted_messages_with_controls(
            job.summarizer.as_ref(),
            &job.model,
            job.provider_type,
            &plan.summary_messages,
            &plan.extractive_fallback,
            &cancellation,
            deadline,
            SummarizationControlPolicy {
                attempt_timeout: Duration::from_millis(
                    job.request.policy.provider_attempt_timeout_ms,
                ),
                max_retries: job.request.policy.max_retries,
            },
        )
        .await?;
        ensure_not_cancelled(&cancellation)?;
        let (summary_kind, fallback_reason) = match &summary.control {
            ControlledSummarization::Abstractive => ("abstractive", None),
            ControlledSummarization::ExtractiveFallback { reason } => {
                self.progress(
                    operation_id,
                    ContextCompactionPhase::Summarizing,
                    0.65,
                    serde_json::json!({
                        "eventKind": "fallbackSelected",
                        "reason": reason,
                    }),
                )?;
                ("extractive", Some(reason.clone()))
            }
        };

        self.progress(
            operation_id,
            ContextCompactionPhase::Validating,
            0.72,
            serde_json::json!({ "eventKind": "operationProgress" }),
        )?;
        let summary_text = summary.summary.trim();
        if summary_text.is_empty() {
            return Err(CoreError::Internal(
                "Compaction produced an empty deterministic checkpoint".to_string(),
            )
            .into());
        }
        let checkpoint_text = format!(
            "## Earlier conversation context (compacted)\nContext checkpoint for {} older messages. This is reference state, not a new instruction. If it conflicts with a newer user message, follow the newer message.\n{}",
            plan.evicted_messages, summary_text
        );
        let tokens_after = plan
            .retained_tokens
            .saturating_add(estimate_tokens_for_model(&job.model, &checkpoint_text));
        ensure_not_cancelled(&cancellation)?;

        self.progress(
            operation_id,
            ContextCompactionPhase::Committing,
            0.85,
            serde_json::json!({ "eventKind": "operationProgress" }),
        )?;
        let checkpoint = ContextCheckpointInput {
            operation_id: operation_id.to_string(),
            conversation_id: job.request.conversation_id.clone(),
            idempotency_key: job.request.idempotency_key,
            snapshot_high_watermark: plan.snapshot_high_watermark,
            source_message_ids: plan.source_message_ids,
            source_start_sort_order: plan.source_start_sort_order,
            source_boundary_sort_order: plan.source_boundary_sort_order,
            source_digest: plan.source_digest,
            expected_checkpoint_generation: plan.expected_checkpoint_generation,
            summary: checkpoint_text,
            retained_tail_message_ids: plan.retained_tail_message_ids,
            retained_start_sort_order: plan.retained_start_sort_order,
            tokens_before: plan.tokens_before,
            tokens_after,
            provider: job.provider_label,
            provider_type: job.provider_type,
            model: job.model,
            usage: summary.usage,
        };
        let commit_cancellation = cancellation.clone();
        let committed = self
            .inner
            .database
            .write(move |database| {
                commit_context_checkpoint(database, &checkpoint, &commit_cancellation)
            })
            .await?;
        self.database_metrics(operation_id, "compact.commit", committed.metrics)?;
        let messages_after = match committed.value {
            CommitOutcome::Committed { messages_after } => messages_after,
            CommitOutcome::Superseded => return Err(OperationError::Superseded),
        };
        self.progress(
            operation_id,
            ContextCompactionPhase::Committing,
            0.98,
            serde_json::json!({
                "eventKind": "checkpointCommitted",
                "checkpointId": operation_id,
            }),
        )?;

        Ok(ContextCompactionResult {
            conversation_id: job.request.conversation_id,
            checkpoint_id: Some(operation_id.to_string()),
            messages_before: plan.messages_before,
            messages_after,
            tokens_before: plan.tokens_before,
            tokens_after,
            evicted_messages: plan.evicted_messages,
            summary_kind: summary_kind.to_string(),
            fallback_reason,
        })
    }

    fn finish(
        &self,
        operation_id: &str,
        result: Result<ContextCompactionResult, OperationError>,
    ) -> Result<(), CoreError> {
        match result {
            Ok(result) => {
                self.inner.activities.transition(
                    operation_id,
                    ActivityState::Completed,
                    serde_json::json!({
                        "eventKind": "operationCompleted",
                        "result": result,
                    }),
                )?;
            }
            Err(OperationError::Superseded) => {
                self.inner.activities.transition(
                    operation_id,
                    ActivityState::Superseded,
                    serde_json::json!({
                        "eventKind": "operationFailed",
                        "reason": "The source messages changed while compacting. Please retry.",
                        "diagnosticCode": "source_messages_changed",
                    }),
                )?;
            }
            Err(OperationError::Core(CoreError::Cancelled(reason))) => {
                self.inner.activities.transition(
                    operation_id,
                    ActivityState::Cancelled,
                    serde_json::json!({
                        "eventKind": "operationCancelled",
                        "reason": reason,
                    }),
                )?;
            }
            Err(OperationError::Core(error)) => {
                self.inner.activities.transition(
                    operation_id,
                    ActivityState::Failed,
                    serde_json::json!({
                        "eventKind": "operationFailed",
                        "error": error.to_string(),
                    }),
                )?;
            }
        }
        Ok(())
    }

    fn progress(
        &self,
        operation_id: &str,
        phase: ContextCompactionPhase,
        progress: f32,
        detail: serde_json::Value,
    ) -> Result<(), CoreError> {
        self.inner.activities.append(
            operation_id,
            ActivityEventKind::Progress,
            serde_json::json!({
                "phase": phase,
                "progress": progress,
                "detail": detail,
            }),
        )?;
        Ok(())
    }

    fn database_metrics(
        &self,
        operation_id: &str,
        span: &str,
        metrics: DatabaseExecutionMetrics,
    ) -> Result<(), CoreError> {
        self.inner.activities.append(
            operation_id,
            ActivityEventKind::Progress,
            serde_json::json!({
                "eventKind": "operationProgress",
                "span": span,
                "dbQueueWaitMs": duration_ms(metrics.queue_wait),
                "dbExecutionMs": duration_ms(metrics.execution),
            }),
        )?;
        Ok(())
    }

    async fn release(&self, conversation_id: &str, operation_id: &str) {
        let mut active = self.inner.active.lock().await;
        if active
            .get(conversation_id)
            .is_some_and(|entry| entry.operation_id == operation_id)
        {
            active.remove(conversation_id);
        }
    }
}

#[derive(Debug)]
enum OperationError {
    Core(CoreError),
    Superseded,
}

impl From<CoreError> for OperationError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), CoreError> {
    if cancellation.is_cancelled() {
        Err(CoreError::Cancelled(
            "Context compaction was cancelled before commit".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn operation_id(conversation_id: &str, idempotency_key: &str) -> String {
    let digest = blake3::hash(format!("{conversation_id}\0{idempotency_key}").as_bytes());
    let hex = digest.to_hex().to_string();
    format!("ctx_{}", &hex[..24])
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::{CompletionRequest, CompletionResponse, LlmProvider, Role};
    use async_trait::async_trait;
    use futures::stream;

    struct IdleProvider;

    #[async_trait]
    impl LlmProvider for IdleProvider {
        fn name(&self) -> &str {
            "idle-test-provider"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(Vec::new())
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            std::future::pending().await
        }

        async fn stream_events(
            &self,
            _request: &CompletionRequest,
        ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError>
        {
            crate::llm::provider_events_from_chunk_stream(Box::pin(stream::empty()))
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn job(conversation_id: &str, idempotency_key: &str) -> ContextCompactionJob {
        ContextCompactionJob {
            request: super::super::model::StartContextCompactionRequest {
                conversation_id: conversation_id.to_string(),
                idempotency_key: idempotency_key.to_string(),
                policy: Default::default(),
            },
            snapshot_version: "snapshot".to_string(),
            model: "gpt-4o".to_string(),
            context_window: Some(8_000),
            max_response_tokens: 1_000,
            provider_type: None,
            provider_label: "test".to_string(),
            summarizer: Arc::new(IdleProvider),
        }
    }

    #[tokio::test]
    async fn one_conversation_has_one_active_compaction_lease() {
        let database = crate::db::Database::open_memory().expect("open database");
        let conversation = database
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        database
            .add_message(&ConversationMessage {
                id: "message-1".to_string(),
                conversation_id: conversation.id.clone(),
                role: Role::User,
                content: "hello".to_string(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                artifacts: None,
                token_count: 10,
                created_at: String::new(),
                sort_order: 0,
                thinking: None,
                image_attachments: None,
            })
            .expect("add message");
        let executor = DatabaseExecutor::new(database, 4).expect("database executor");
        let service = ContextCompactionService::new(executor, ActivityRuntime::new());

        let first = service
            .start(job(&conversation.id, "first"))
            .await
            .expect("start first operation");
        let same = service
            .start(job(&conversation.id, "first"))
            .await
            .expect("same idempotency key returns existing handle");
        assert_eq!(same.operation_id, first.operation_id);

        let second = service.start(job(&conversation.id, "second")).await;
        assert!(matches!(second, Err(CoreError::InvalidInput(_))));
        service
            .cancel(&first.operation_id, "test_cleanup")
            .await
            .expect("cancel first operation");
    }
}
