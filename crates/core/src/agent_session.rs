//! Agent Session adapter over the existing [`AgentExecutor`].
//!
//! This Module is the first concrete Runtime Protocol Adapter: it translates
//! host-agnostic turn input into an executor run and exposes ordered
//! `AgentRunEvent`s through the Agent Session Interface.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent::{AgentConfig, AgentEvent, AgentExecutor};
use crate::agent_run::{AgentRunEvent, AgentRunPhase};
use crate::approval::ApprovalDecision;
use crate::conversation::AgentTaskRun;
use crate::db::Database;
use crate::db_executor::DatabaseExecutor;
use crate::error::CoreError;
use crate::llm::{ContentPart, LlmProvider, Message};
use crate::run_event_outbox::{AgentRunEventDelivery, AgentRunEventOutbox, AgentRunEventOutboxes};
use crate::runtime::{
    validate_runtime_turn_events, AgentSession, AgentSessionConfig, AgentTurnHandle,
    AgentTurnInput, AgentTurnResult, AgentTurnState,
};
use crate::tools::ToolRegistry;

pub struct ExecutorAgentSession {
    config: AgentSessionConfig,
    executor: AgentExecutor,
    db: Database,
    history: Vec<Message>,
    source_scope_override: Option<Vec<String>>,
    next_sort_order: i64,
    events_by_run: HashMap<String, Vec<AgentRunEvent>>,
    results_by_run: HashMap<String, AgentTurnResult>,
    event_outboxes: Option<AgentRunEventOutboxes>,
}

struct AgentSessionRunEventDelivery;

impl AgentRunEventDelivery for AgentSessionRunEventDelivery {
    fn deliver_run_event(&self, _conversation_id: &str, _event: &AgentRunEvent) {}

    fn deliver_task_run_snapshot(&self, _conversation_id: &str, _snapshot: AgentTaskRun) {}
}

impl ExecutorAgentSession {
    pub fn new(
        config: AgentSessionConfig,
        provider: Box<dyn LlmProvider>,
        tools: ToolRegistry,
        executor_config: AgentConfig,
        db: Database,
    ) -> Self {
        Self {
            config,
            executor: AgentExecutor::new(provider, tools, executor_config).with_activity_runtime(
                crate::activity::ActivityRuntime::with_database(db.clone())
                    .unwrap_or_else(|_| crate::activity::ActivityRuntime::new()),
            ),
            db,
            history: Vec::new(),
            source_scope_override: None,
            next_sort_order: 0,
            events_by_run: HashMap::new(),
            results_by_run: HashMap::new(),
            event_outboxes: None,
        }
    }

    pub fn with_history(mut self, history: Vec<Message>, next_sort_order: i64) -> Self {
        self.history = history;
        self.next_sort_order = next_sort_order;
        self
    }

    pub fn with_source_scope_override(mut self, source_scope: Vec<String>) -> Self {
        self.source_scope_override = Some(source_scope);
        self
    }

    pub fn with_approval_callback(mut self, cb: crate::approval::ApprovalCallback) -> Self {
        self.executor = self.executor.with_approval_callback(cb);
        self
    }

    pub fn with_confirmation_callback(mut self, cb: crate::agent::ConfirmationCallback) -> Self {
        self.executor = self.executor.with_confirmation_callback(cb);
        self
    }

    pub fn with_auto_loaded_skills_override(mut self, skills: Vec<crate::skills::Skill>) -> Self {
        self.executor = self.executor.with_auto_loaded_skills_override(skills);
        self
    }

    pub fn with_skills_override(mut self, skills: Vec<crate::skills::Skill>) -> Self {
        self.executor = self.executor.with_skills_override(skills);
        self
    }

    pub fn result_for_run(&self, run_id: &str) -> Option<&AgentTurnResult> {
        self.results_by_run.get(run_id)
    }

    fn outboxes(&mut self) -> Result<AgentRunEventOutboxes, CoreError> {
        if self.event_outboxes.is_none() {
            let database = DatabaseExecutor::new(self.db.clone(), 64)?;
            self.event_outboxes = Some(AgentRunEventOutboxes::new(
                database,
                Arc::new(AgentSessionRunEventDelivery),
            ));
        }
        Ok(self
            .event_outboxes
            .as_ref()
            .expect("Agent Session outboxes were initialized")
            .clone())
    }
}

#[async_trait]
impl AgentSession for ExecutorAgentSession {
    fn config(&self) -> &AgentSessionConfig {
        &self.config
    }

    async fn configure(&mut self, config: AgentSessionConfig) -> Result<(), CoreError> {
        self.config = config;
        self.config.apply_protocol_defaults();
        Ok(())
    }

    async fn start_turn(&mut self, input: AgentTurnInput) -> Result<AgentTurnHandle, CoreError> {
        let run_id = self
            .config
            .task_run_id
            .clone()
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "ExecutorAgentSession requires config.task_run_id backed by an existing Agent task run"
                        .to_string(),
                )
            })?;
        let task_run = self.db.get_agent_task_run(&run_id).map_err(|error| match error {
            CoreError::NotFound(_) => CoreError::InvalidInput(format!(
                "ExecutorAgentSession task_run_id {run_id} does not identify an existing Agent task run"
            )),
            other => other,
        })?;
        if let Some(configured_conversation_id) = self.config.conversation_id.as_deref() {
            if configured_conversation_id != task_run.conversation_id {
                return Err(CoreError::InvalidInput(format!(
                    "ExecutorAgentSession task_run_id {run_id} belongs to conversation {}, not {configured_conversation_id}",
                    task_run.conversation_id
                )));
            }
        }
        if !matches!(
            task_run.status.as_str(),
            "queued" | "running" | "waiting_approval"
        ) {
            return Err(CoreError::Conflict(format!(
                "ExecutorAgentSession cannot execute Agent Run {run_id} from status '{}'",
                task_run.status
            )));
        }

        let conversation_id = task_run.conversation_id;
        let turn_id = task_run.turn_id;
        let session_id = self.config.session_id.clone();
        let outbox = self.outboxes()?.open(&conversation_id, &run_id).await?;
        if outbox.is_closed_for_submission() {
            return Err(CoreError::Conflict(format!(
                "Agent Run {run_id} is already closed"
            )));
        }
        let (tx, rx) = mpsc::channel::<AgentEvent>(64);
        let collector_run_id = run_id.clone();
        let collector_turn_id = turn_id.clone();
        let collector_outbox = Arc::clone(&outbox);
        let collector = tokio::spawn(async move {
            collect_run_events(rx, collector_outbox, collector_run_id, collector_turn_id).await
        });

        let mut user_parts = vec![ContentPart::Text {
            text: input.user_text,
        }];
        for attachment in input.attachments {
            if let Some(path) = attachment.path {
                user_parts.push(ContentPart::Text {
                    text: format!("[Attachment: {}]", path),
                });
            }
        }

        let result = self
            .executor
            .run_with_source_scope(
                self.history.clone(),
                user_parts,
                &self.db,
                Some(&conversation_id),
                None,
                self.source_scope_override.clone(),
                tx,
                self.next_sort_order,
            )
            .await;

        let collected = collector
            .await
            .map_err(|error| CoreError::Agent(format!("Run Event collector failed: {error}")))?;
        let collected = match collected {
            Ok(collected) => collected,
            Err(submission_error) => {
                return match outbox.wait_for_terminal_commit().await {
                    Ok(_) => Err(submission_error),
                    Err(failure) => Err(CoreError::Agent(format!(
                        "Run Event submission failed and Agent Run {run_id} was failed closed: {failure}"
                    ))),
                };
            }
        };

        let final_message = match result {
            Ok(message) => Some(message.text_content()),
            Err(err) => {
                if !collected.terminal_submitted {
                    outbox
                        .submit(AgentRunEvent::terminal_error(
                            &run_id,
                            Some(&turn_id),
                            0,
                            "Agent execution failed.",
                            "failed",
                            Some(&serde_json::json!({ "error": err.to_string() })),
                        ))
                        .map_err(|error| {
                            CoreError::Agent(format!(
                                "Could not submit the terminal Run Event: {error}"
                            ))
                        })?;
                }
                None
            }
        };

        let missing_terminal = !collected.terminal_submitted && final_message.is_some();
        if missing_terminal {
            outbox
                .submit(AgentRunEvent::terminal_error(
                    &run_id,
                    Some(&turn_id),
                    0,
                    "Agent execution ended without a terminal Run Event.",
                    "failed",
                    Some(&serde_json::json!({ "reason": "missing_terminal_event" })),
                ))
                .map_err(|error| {
                    CoreError::Agent(format!(
                        "Could not fail the incomplete turn closed: {error}"
                    ))
                })?;
        }

        outbox.wait_for_terminal_commit().await.map_err(|error| {
            CoreError::Agent(format!(
                "Agent Run {run_id} did not reach its durable terminal barrier: {error}"
            ))
        })?;
        let run_events = self.db.list_agent_run_events(&run_id)?;

        if missing_terminal {
            return Err(CoreError::Agent(
                "Runtime turn contract failed: Agent execution ended without a terminal Run Event"
                    .to_string(),
            ));
        }

        let report = validate_runtime_turn_events(&run_events)
            .map_err(|err| CoreError::Agent(format!("Runtime turn contract failed: {err}")))?;

        let status = report.terminal_status;
        let handle = AgentTurnHandle {
            session_id,
            run_id: run_id.clone(),
            turn_id: turn_id.clone(),
            state: AgentTurnState::Terminal(status.clone()),
        };
        let turn_result = AgentTurnResult {
            handle: handle.clone(),
            final_message,
            artifacts: serde_json::json!({
                "kind": "agentSessionTurn",
                "version": 1,
                "eventCount": report.event_count,
                "approvalDenied": report.approval_denied,
            }),
            usage: serde_json::json!({}),
            trace_id: None,
            status,
        };
        self.events_by_run.insert(run_id.clone(), run_events);
        self.results_by_run.insert(run_id, turn_result);

        Ok(handle)
    }

    async fn steer_turn(&mut self, _turn_id: &str, _text: String) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "ExecutorAgentSession runs turns synchronously; steering requires a running turn"
                .to_string(),
        ))
    }

    async fn interrupt_turn(&mut self, _turn_id: &str, _reason: String) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "ExecutorAgentSession runs turns synchronously; interrupt requires a running turn"
                .to_string(),
        ))
    }

    async fn resolve_approval(
        &mut self,
        _request_id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "approval resolution is handled by the configured approval adapter".to_string(),
        ))
    }

    async fn read_events(&self, run_id: &str) -> Result<Vec<AgentRunEvent>, CoreError> {
        if let Some(events) = self.events_by_run.get(run_id) {
            return Ok(events.clone());
        }

        let events = self.db.list_agent_run_events(run_id)?;
        if events.is_empty() {
            return Err(CoreError::NotFound(format!("run events for {run_id}")));
        }
        Ok(events)
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        Ok(())
    }
}

async fn collect_run_events(
    mut rx: mpsc::Receiver<AgentEvent>,
    outbox: Arc<AgentRunEventOutbox>,
    run_id: String,
    turn_id: String,
) -> Result<CollectedRunEvents, CoreError> {
    let mut collected = CollectedRunEvents::default();
    while let Some(event) = rx.recv().await {
        let run_event = AgentRunEvent::from_agent_event(&event).with_context(
            Some(&run_id),
            Some(&turn_id),
            None,
        );
        let terminal = run_event.closes_run();
        outbox.submit(run_event).map_err(|error| {
            CoreError::Agent(format!("Could not submit an ordered Run Event: {error}"))
        })?;
        collected.terminal_submitted |= terminal;
    }
    Ok(collected)
}

#[derive(Debug, Default)]
struct CollectedRunEvents {
    terminal_submitted: bool,
}

pub fn status_event_for_session(
    run_id: &str,
    turn_id: &str,
    event_seq: u64,
    label: &str,
    status: &str,
) -> AgentRunEvent {
    AgentRunEvent::status_update(
        run_id,
        Some(turn_id),
        event_seq,
        AgentRunPhase::Responding,
        label,
        Some(status),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::{stream, StreamExt};

    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::Role;
    use crate::llm::{
        CompletionRequest, CompletionResponse, FinishReason, StreamChunk, ToolCallDelta,
    };
    use crate::task_run::{AgentTaskRuntime, CreateTaskRunInput};

    struct StaticProvider;

    struct CountingProvider {
        stream_calls: Arc<AtomicUsize>,
    }

    fn create_started_run(db: &Database, suffix: &str) -> (String, String, String) {
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "static".to_string(),
                model: "static-model".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("conversation");
        let message = ConversationMessage {
            id: format!("message-{suffix}"),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "hello".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 1,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).expect("message");
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("turn");
        let runtime = AgentTaskRuntime::new(db);
        let run = runtime
            .create_run(CreateTaskRunInput {
                conversation_id: &conversation.id,
                turn_id: &turn.id,
                user_message_id: &message.id,
                title: "Session turn",
                provider: Some("static"),
                model: Some("static-model"),
            })
            .expect("task run");
        runtime.start_run(&run.id, "routing").expect("start run");
        (conversation.id, turn.id, run.id)
    }

    #[async_trait]
    impl LlmProvider for StaticProvider {
        fn name(&self) -> &str {
            "static"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(vec!["static-model".to_string()])
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            Err(CoreError::Llm("not used".to_string()))
        }

        async fn stream_events(
            &self,
            _request: &CompletionRequest,
        ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError>
        {
            let chunks = vec![Ok(StreamChunk {
                delta: "session answer".to_string(),
                tool_call_delta: None::<ToolCallDelta>,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })];
            crate::llm::provider_events_from_chunk_stream(stream::iter(chunks).boxed())
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl LlmProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(vec!["static-model".to_string()])
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            Err(CoreError::Llm("not used".to_string()))
        }

        async fn stream_events(
            &self,
            _request: &CompletionRequest,
        ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError>
        {
            self.stream_calls.fetch_add(1, Ordering::SeqCst);
            crate::llm::provider_events_from_chunk_stream(
                stream::iter(vec![Ok(StreamChunk {
                    delta: "must not execute".to_string(),
                    tool_call_delta: None::<ToolCallDelta>,
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                })])
                .boxed(),
            )
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn executor_agent_session_runs_turn_and_exposes_events() {
        let db = Database::open_memory().unwrap();
        let (conversation_id, _turn_id, run_id) = create_started_run(&db, "runs-turn");
        let mut runtime_config = AgentSessionConfig::default();
        runtime_config.conversation_id = Some(conversation_id);
        runtime_config.task_run_id = Some(run_id.clone());
        let executor_config = AgentConfig {
            model: Some("static-model".to_string()),
            ..AgentConfig::default()
        };
        let mut session = ExecutorAgentSession::new(
            runtime_config,
            Box::new(StaticProvider),
            ToolRegistry::new(),
            executor_config,
            db,
        );

        let handle = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .expect("turn should run");
        let events = session.read_events(&handle.run_id).await.unwrap();
        let result = session.result_for_run(&handle.run_id).unwrap();
        let stored_events = session.db.list_agent_run_events(&handle.run_id).unwrap();

        validate_runtime_turn_events(&events).unwrap();
        validate_runtime_turn_events(&stored_events).unwrap();
        assert_eq!(handle.run_id, run_id);
        assert_eq!(
            result.status,
            crate::runtime::RuntimeTerminalStatus::Completed
        );
        assert_eq!(result.final_message.as_deref(), Some("session answer"));
        assert!(events.iter().any(|event| event.kind.as_str() == "done"));
        assert_eq!(stored_events.len(), events.len());
    }

    #[tokio::test]
    async fn executor_agent_session_reads_persisted_events_without_memory_cache() {
        let db = Database::open_memory().unwrap();
        let (conversation_id, _turn_id, run_id) = create_started_run(&db, "persisted");
        let mut runtime_config = AgentSessionConfig::default();
        runtime_config.conversation_id = Some(conversation_id);
        runtime_config.task_run_id = Some(run_id.clone());
        let executor_config = AgentConfig {
            model: Some("static-model".to_string()),
            ..AgentConfig::default()
        };
        let mut writer = ExecutorAgentSession::new(
            runtime_config.clone(),
            Box::new(StaticProvider),
            ToolRegistry::new(),
            executor_config.clone(),
            db.clone(),
        );

        let handle = writer
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .expect("turn should run");
        let reader = ExecutorAgentSession::new(
            runtime_config,
            Box::new(StaticProvider),
            ToolRegistry::new(),
            executor_config,
            db,
        );

        let events = reader.read_events(&handle.run_id).await.unwrap();

        validate_runtime_turn_events(&events).unwrap();
        assert_eq!(handle.run_id, run_id);
        assert!(events.iter().any(|event| event.kind.as_str() == "done"));
    }

    #[tokio::test]
    async fn executor_agent_session_continues_the_outbox_durable_sequence() {
        let db = Database::open_memory().unwrap();
        let (conversation_id, turn_id, run_id) = create_started_run(&db, "outbox-sequence");
        db.save_agent_run_event(&status_event_for_session(
            &run_id,
            &turn_id,
            1,
            "Agent started",
            "running",
        ))
        .expect("seed durable head");

        let runtime_config = AgentSessionConfig {
            conversation_id: Some(conversation_id),
            task_run_id: Some(run_id.clone()),
            ..AgentSessionConfig::default()
        };
        let executor_config = AgentConfig {
            model: Some("static-model".to_string()),
            ..AgentConfig::default()
        };
        let mut session = ExecutorAgentSession::new(
            runtime_config,
            Box::new(StaticProvider),
            ToolRegistry::new(),
            executor_config,
            db.clone(),
        );

        let handle = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .expect("turn should continue through the durable outbox");
        let events = session.read_events(&handle.run_id).await.unwrap();
        let task = db.get_agent_task_run(&run_id).expect("projected task run");

        assert_eq!(events.first().map(|event| event.event_seq), Some(1));
        assert_eq!(
            events.last().map(|event| event.event_seq),
            Some(events.len() as u64)
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[1].event_seq == pair[0].event_seq + 1));
        assert_eq!(task.status, "completed");
    }

    #[tokio::test]
    async fn executor_agent_session_rejects_missing_task_backing_before_provider_work() {
        let db = Database::open_memory().unwrap();
        let stream_calls = Arc::new(AtomicUsize::new(0));
        let runtime_config = AgentSessionConfig {
            task_run_id: Some("missing-run".to_string()),
            ..AgentSessionConfig::default()
        };
        let mut session = ExecutorAgentSession::new(
            runtime_config,
            Box::new(CountingProvider {
                stream_calls: Arc::clone(&stream_calls),
            }),
            ToolRegistry::new(),
            AgentConfig {
                model: Some("static-model".to_string()),
                ..AgentConfig::default()
            },
            db.clone(),
        );

        let error = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .expect_err("a durable task run is required");
        let stored_event_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert!(matches!(&error, CoreError::InvalidInput(_)));
        assert!(error.to_string().contains("existing Agent task run"));
        assert_eq!(stream_calls.load(Ordering::SeqCst), 0);
        assert_eq!(stored_event_count, 0);
    }

    #[tokio::test]
    async fn executor_agent_session_rejects_resumable_and_stopping_states_before_provider_work() {
        for status in ["paused", "awaiting_user_input", "cancelling"] {
            let db = Database::open_memory().unwrap();
            let (conversation_id, _turn_id, run_id) = create_started_run(&db, status);
            db.update_agent_task_run_progress(
                &run_id,
                Some(status),
                Some(status),
                None,
                None,
                None,
                None,
            )
            .expect("non-executable task state");
            let stream_calls = Arc::new(AtomicUsize::new(0));
            let mut session = ExecutorAgentSession::new(
                AgentSessionConfig {
                    conversation_id: Some(conversation_id),
                    task_run_id: Some(run_id.clone()),
                    ..AgentSessionConfig::default()
                },
                Box::new(CountingProvider {
                    stream_calls: Arc::clone(&stream_calls),
                }),
                ToolRegistry::new(),
                AgentConfig {
                    model: Some("static-model".to_string()),
                    ..AgentConfig::default()
                },
                db.clone(),
            );

            let error = session
                .start_turn(AgentTurnInput::text("hello"))
                .await
                .expect_err("a specialized resume or stop path is required");

            assert!(matches!(&error, CoreError::Conflict(_)));
            assert!(error.to_string().contains(status));
            assert_eq!(stream_calls.load(Ordering::SeqCst), 0);
            assert!(db
                .list_agent_run_events(&run_id)
                .expect("empty run ledger")
                .is_empty());
        }
    }

    #[tokio::test]
    async fn executor_agent_session_requires_a_task_run_id_before_provider_work() {
        let db = Database::open_memory().unwrap();
        let stream_calls = Arc::new(AtomicUsize::new(0));
        let mut session = ExecutorAgentSession::new(
            AgentSessionConfig::default(),
            Box::new(CountingProvider {
                stream_calls: Arc::clone(&stream_calls),
            }),
            ToolRegistry::new(),
            AgentConfig {
                model: Some("static-model".to_string()),
                ..AgentConfig::default()
            },
            db.clone(),
        );

        let error = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .expect_err("task_run_id is required");
        let stored_event_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert!(matches!(&error, CoreError::InvalidInput(_)));
        assert!(error.to_string().contains("config.task_run_id"));
        assert_eq!(stream_calls.load(Ordering::SeqCst), 0);
        assert_eq!(stored_event_count, 0);
    }
}
