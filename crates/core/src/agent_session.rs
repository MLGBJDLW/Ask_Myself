//! Agent Session adapter over the existing [`AgentExecutor`].
//!
//! This Module is the first concrete Runtime Protocol Adapter: it translates
//! host-agnostic turn input into an executor run and exposes ordered
//! `AgentRunEvent`s through the Agent Session Interface.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::agent::{AgentConfig, AgentEvent, AgentExecutor};
use crate::agent_run::{AgentRunEvent, AgentRunPhase};
use crate::approval::ApprovalDecision;
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{ContentPart, LlmProvider, Message};
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
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let turn_id = Uuid::new_v4().to_string();
        let session_id = self.config.session_id.clone();
        let (tx, rx) = mpsc::channel::<AgentEvent>(64);
        let events = Arc::new(Mutex::new(Vec::<AgentRunEvent>::new()));
        let collector_events = Arc::clone(&events);
        let collector_run_id = run_id.clone();
        let collector_turn_id = turn_id.clone();
        let collector = tokio::spawn(async move {
            collect_run_events(rx, collector_events, collector_run_id, collector_turn_id).await;
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
                self.config.conversation_id.as_deref(),
                None,
                self.source_scope_override.clone(),
                tx,
                self.next_sort_order,
            )
            .await;

        let _ = collector.await;
        let mut run_events = {
            let guard = events.lock().await;
            guard.clone()
        };

        let final_message = match result {
            Ok(message) => Some(message.text_content()),
            Err(err) => {
                if !run_events.iter().any(AgentRunEvent::is_terminal) {
                    let next_seq = run_events
                        .last()
                        .map(|event| event.event_seq + 1)
                        .unwrap_or(1);
                    run_events.push(AgentRunEvent::terminal_error(
                        &run_id,
                        Some(&turn_id),
                        next_seq,
                        "Agent execution failed.",
                        "failed",
                        Some(&serde_json::json!({ "error": err.to_string() })),
                    ));
                }
                None
            }
        };

        let report = validate_runtime_turn_events(&run_events)
            .map_err(|err| CoreError::Agent(format!("Runtime turn contract failed: {err}")))?;
        self.db.save_agent_run_events(&run_events)?;

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
    events: Arc<Mutex<Vec<AgentRunEvent>>>,
    run_id: String,
    turn_id: String,
) {
    let mut event_seq = 1u64;
    while let Some(event) = rx.recv().await {
        let run_event = AgentRunEvent::from_agent_event(&event).with_context(
            Some(&run_id),
            Some(&turn_id),
            Some(event_seq),
        );
        event_seq += 1;
        events.lock().await.push(run_event);
    }
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
    use async_trait::async_trait;
    use futures::{stream, StreamExt};

    use crate::llm::{
        CompletionRequest, CompletionResponse, FinishReason, StreamChunk, ToolCallDelta,
    };

    struct StaticProvider;

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

        async fn stream(
            &self,
            _request: &CompletionRequest,
        ) -> Result<futures::stream::BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError>
        {
            let chunks = vec![Ok(StreamChunk {
                delta: "session answer".to_string(),
                tool_call_delta: None::<ToolCallDelta>,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })];
            Ok(stream::iter(chunks).boxed())
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn executor_agent_session_runs_turn_and_exposes_events() {
        let db = Database::open_memory().unwrap();
        let mut runtime_config = AgentSessionConfig::default();
        runtime_config.task_run_id = Some("run-1".to_string());
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
        assert_eq!(handle.run_id, "run-1");
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
        let mut runtime_config = AgentSessionConfig::default();
        runtime_config.task_run_id = Some("run-persisted".to_string());
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
        assert_eq!(handle.run_id, "run-persisted");
        assert!(events.iter().any(|event| event.kind.as_str() == "done"));
    }
}
