//! Nexa tools for an upstream-owned agent loop. No model is sampled here.

use super::tool_dispatch::{ToolDispatchContext, ToolDispatchRuntime};
use super::*;
use crate::tools::ToolResult;
use std::collections::HashMap;

pub struct PersistedAssistantMessage {
    pub id: String,
    pub message: Message,
}

pub struct ExternalToolSessionInput {
    pub tools: ToolRegistry,
    pub config: AgentConfig,
    pub db: Arc<Database>,
    pub conversation_id: String,
    pub turn_id: String,
    pub next_sort_order: i64,
    pub user_prompt: String,
    pub events: mpsc::Sender<AgentEvent>,
    pub cancellation: CancellationToken,
    pub approval: ApprovalCallback,
    pub visual_interpreter: Option<ToolVisualInterpreter>,
    pub native_vision: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolExecutionContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountTool(Arc<AtomicUsize>, Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl Tool for CountTool {
        fn name(&self) -> &str {
            "count_effect"
        }
        fn description(&self) -> &str {
            "Record an effect for runtime contract tests"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            self.1.fetch_add(1, Ordering::SeqCst);
            serde_json::json!({"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false})
        }
        fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
            true
        }
        async fn execute(
            &self,
            context: ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult {
                call_id: context.call_id.to_string(),
                content: "effect recorded".into(),
                is_error: false,
                artifacts: None,
            })
        }
    }
    fn session(
        limit: u32,
    ) -> (
        ExternalToolSession,
        Arc<AtomicUsize>,
        mpsc::Receiver<AgentEvent>,
    ) {
        let db = Arc::new(Database::open_memory().unwrap());
        let conversation = db
            .create_conversation(&crate::conversation::CreateConversationInput {
                provider: "subscription".into(),
                model: "native".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Record the requested count".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 5,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, None)
            .unwrap();
        db.create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            &user.content,
            Some("subscription"),
            Some("native"),
        )
        .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountTool(
            count.clone(),
            Arc::new(AtomicUsize::new(0)),
        )));
        let (events, rx) = mpsc::channel(256);
        let input = ExternalToolSessionInput {
            tools,
            config: AgentConfig {
                model: Some("native".into()),
                max_iterations: limit,
                tool_approval_mode: crate::approval::ToolApprovalMode::AllowAll,
                ..AgentConfig::default()
            },
            db,
            conversation_id: conversation.id,
            turn_id: turn.id,
            next_sort_order: 1,
            user_prompt: user.content,
            events,
            cancellation: CancellationToken::new(),
            approval: Arc::new(|_| {
                Box::pin(async { crate::approval::ApprovalDecision::AllowOnce })
            }),
            visual_interpreter: None,
            native_vision: false,
        };
        (ExternalToolSession::new(input).unwrap(), count, rx)
    }
    fn call(id: &str, value: i32) -> ToolCallRequest {
        ToolCallRequest {
            id: id.into(),
            name: "count_effect".into(),
            arguments: serde_json::json!({"value":value}).to_string(),
            thought_signature: None,
        }
    }

    #[tokio::test]
    async fn dispatch_does_not_rebuild_schemas_for_each_scheduling_consumer() {
        let (mut session, count, _rx) = session(4);
        let schemas = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountTool(count.clone(), schemas.clone())));
        session.input.tools = tools;
        let result = session
            .execute(call("schema-preparation", 1))
            .await
            .unwrap();
        assert!(!result.result.is_error);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let constructed = schemas.load(Ordering::SeqCst);
        eprintln!("tool_dispatch_schema_constructions={constructed}");
        // One provider surface, one shared scheduling preparation, one final
        // execution validation. Successful output needs no error schema.
        assert_eq!(constructed, 3);
    }

    #[tokio::test]
    async fn duplicate_call_id_executes_once_and_changed_reuse_fails() {
        let (session, count, _rx) = session(4);
        assert!(
            !session
                .execute(call("one", 1))
                .await
                .unwrap()
                .result
                .is_error
        );
        assert!(
            !session
                .execute(call("one", 1))
                .await
                .unwrap()
                .result
                .is_error
        );
        assert!(session.execute(call("one", 2)).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let history = session
            .input
            .db
            .get_messages(&session.input.conversation_id)
            .unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|message| message.tool_call_id.as_deref() == Some("one"))
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn cancellation_and_budget_stop_before_another_effect() {
        let (session, count, _rx) = session(1);
        session.execute(call("one", 1)).await.unwrap();
        assert!(session
            .execute(call("two", 2))
            .await
            .unwrap_err()
            .to_string()
            .contains("budget"));
        session.input.cancellation.cancel();
        assert!(matches!(
            session.execute(call("three", 3)).await,
            Err(CoreError::Cancelled(_))
        ));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn invalid_schema_is_visible_but_does_not_execute() {
        let (session, count, _rx) = session(4);
        let mut invalid = call("one", 1);
        invalid.arguments = r#"{"value":"bad"}"#.into();
        let result = session.execute(invalid).await.unwrap().result;
        assert!(result.is_error);
        assert!(result.content.contains("value"));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn steering_is_durable_between_tool_results_and_the_next_answer() {
        let (session, _, _rx) = session(4);
        session.execute(call("first", 1)).await.unwrap();
        session
            .persist_steering(&AgentSteeringMessage::text("Use the corrected scope"))
            .await
            .unwrap();
        session
            .persist_answer("Applied the corrected scope")
            .await
            .unwrap();
        let history = session
            .input
            .db
            .get_messages(&session.input.conversation_id)
            .unwrap();
        let result = history
            .iter()
            .position(|message| message.tool_call_id.as_deref() == Some("first"))
            .unwrap();
        let steering = history
            .iter()
            .position(|message| message.content == "Use the corrected scope")
            .unwrap();
        assert!(result < steering && steering < history.len() - 1);
        assert_eq!(
            history[steering].artifacts.as_ref().unwrap()["kind"],
            "steering"
        );
        assert!(history
            .windows(2)
            .all(|pair| pair[0].sort_order < pair[1].sort_order));
    }

    #[tokio::test]
    async fn denial_and_cancellation_while_awaiting_approval_have_no_effect() {
        let (mut session, count, _rx) = session(4);
        session.input.config.tool_approval_mode = crate::approval::ToolApprovalMode::Ask;
        session.input.approval =
            Arc::new(|_| Box::pin(async { crate::approval::ApprovalDecision::Deny }));
        assert!(
            session
                .execute(call("deny", 1))
                .await
                .unwrap()
                .result
                .is_error
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let cancel = session.input.cancellation.clone();
        session.input.approval = Arc::new(move |_| {
            cancel.cancel();
            Box::pin(std::future::pending())
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            session.execute(call("cancel", 2)),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(CoreError::Cancelled(_)))
                || result.is_ok_and(|output| output.result.is_error)
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn structured_question_suspends_the_official_loop_and_preserves_resume_state() {
        let (mut session, _, _rx) = session(4);
        session.input.tools.register(Box::new(
            crate::tools::request_user_input_tool::RequestUserInputTool,
        ));
        let call = ToolCallRequest {id:"question".into(),name:"request_user_input".into(),arguments:r#"{"questions":[{"id":"scope","header":"Scope","question":"Which scope?","type":"single_choice","options":[{"label":"App","description":"Only this app"},{"label":"Repo","description":"Entire repository"}]}]}"#.into(),thought_signature:None};
        assert!(matches!(
            session.execute(call).await,
            Err(CoreError::AwaitingUserInput { .. })
        ));
        assert_eq!(
            session
                .input
                .db
                .get_agent_task_run_by_turn(&session.input.turn_id)
                .unwrap()
                .unwrap()
                .status,
            "awaiting_user_input"
        );
        assert_eq!(
            session
                .input
                .db
                .list_interaction_requests(Some(&session.input.conversation_id), false)
                .unwrap()
                .len(),
            1
        );
    }
}

/// Complete tool result plus current-call-only visual evidence. Pixels never
/// enter the durable conversation or the idempotency cache.
#[derive(Debug)]
pub struct ExternalToolOutput {
    pub result: ToolResult,
    pub visual_parts: Vec<ContentPart>,
}

pub struct ExternalToolSession {
    input: ExternalToolSessionInput,
    source_scope: Vec<String>,
    privacy: privacy::PrivacyConfig,
    activity: crate::activity::ActivityRuntime,
    state: TokioMutex<ExternalToolState>,
}

struct ExternalToolState {
    plan: AgentTaskPlan,
    recorder: TurnLoopRecorder,
    guard: AgentLoopGuard,
    trace_items: Vec<PersistedTraceItem>,
    next_sort_order: i64,
    rounds: u32,
    completed: HashMap<String, (String, ToolResult)>,
    action_reconciliation: super::turn_loop::ActionReconciliationFence,
}

impl ExternalToolSession {
    pub fn new(input: ExternalToolSessionInput) -> Result<Self, CoreError> {
        let source_scope = input
            .db
            .get_effective_conversation_source_scope(&input.conversation_id)?;
        let route = route_user_turn(
            &input.user_prompt,
            &input.config.system_prompt,
            !source_scope.is_empty(),
        );
        let plan = build_task_plan(TaskPlanningInput::for_requirements(
            &input.user_prompt,
            &route.requirements,
            !source_scope.is_empty(),
            source_scope.len(),
        ));
        let privacy = input.db.load_privacy_config()?;
        let activity = crate::activity::ActivityRuntime::with_database((*input.db).clone())?;
        let state = ExternalToolState {
            plan,
            recorder: TurnLoopRecorder::new(route.kind, input.config.max_iterations),
            guard: AgentLoopGuard::new(),
            trace_items: Vec::new(),
            next_sort_order: input.next_sort_order,
            rounds: 0,
            completed: HashMap::new(),
            action_reconciliation: super::turn_loop::ActionReconciliationFence::from_resume_prompt(
                &input.user_prompt,
            ),
        };
        Ok(Self {
            input,
            source_scope,
            privacy,
            activity,
            state: TokioMutex::new(state),
        })
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        if self.input.config.max_iterations == 0 {
            Vec::new()
        } else {
            self.input.tools.definitions()
        }
    }

    pub async fn execute(&self, call: ToolCallRequest) -> Result<ExternalToolOutput, CoreError> {
        let mut state = self.state.lock().await;
        if self.input.cancellation.is_cancelled() {
            return Err(CoreError::Cancelled("Stopped by user".into()));
        }
        let signature = format!("{}:{}", call.name, call.arguments);
        if let Some((previous, result)) = state.completed.get(&call.id) {
            if previous != &signature {
                return Err(CoreError::Agent(
                    "Upstream reused a tool call ID with different arguments".into(),
                ));
            }
            return Ok(ExternalToolOutput {
                result: result.clone(),
                visual_parts: Vec::new(),
            });
        }
        let limit = if self.input.config.max_iterations == u32::MAX {
            256
        } else {
            self.input.config.max_iterations
        };
        if state.rounds >= limit {
            return Err(CoreError::Agent(
                "External agent tool budget exhausted".into(),
            ));
        }
        let batch = tool_protocol::VerifiedToolCallBatch::seal(vec![call.clone()], false, true)
            .map_err(|_| {
                CoreError::InvalidInput("Upstream returned an incomplete tool call".into())
            })?;
        let block = state.guard.observe_model_step("", batch.as_slice());
        if block
            .as_ref()
            .is_some_and(|block| block.action == LoopGuardAction::StopLoop)
        {
            return Err(CoreError::Agent(
                "External agent repeated an unproductive tool call".into(),
            ));
        }
        let model = self.input.config.model.as_deref().unwrap_or("subscription");
        self.input.db.add_message(&ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: self.input.conversation_id.clone(),
            role: Role::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![call.clone()],
            artifacts: Some(
                serde_json::json!({"turnId":self.input.turn_id,"runtime":"subscription"}),
            ),
            token_count: 0,
            created_at: String::new(),
            sort_order: state.next_sort_order,
            thinking: None,
            image_attachments: None,
        })?;
        state.next_sort_order += 1;
        let block = block.map(|block| tool_dispatch::ToolDispatchBlock::LoopGuard(block.reason));
        let mut definitions = self.definitions();
        let mut messages = Vec::new();
        let mut trace = None;
        let mut started = HashSet::new();
        let approval = Some(self.input.approval.clone());
        let ExternalToolState {
            plan,
            recorder,
            guard,
            trace_items,
            next_sort_order,
            rounds,
            action_reconciliation,
            ..
        } = &mut *state;
        let outcome = ToolDispatchRuntime {
            native_vision: Some(self.input.native_vision),
            tools: &self.input.tools,
            config: &self.input.config,
            cancel_token: &self.input.cancellation,
            confirmation_callback: &None,
            approval_callback: &approval,
            tool_visual_interpreter: &self.input.visual_interpreter,
            activity_runtime: &self.activity,
        }
        .dispatch_tool_calls(
            ToolDispatchContext {
                db: &self.input.db,
                tx: &self.input.events,
                conversation_id: Some(&self.input.conversation_id),
                turn_id: Some(&self.input.turn_id),
                source_scope: &self.source_scope,
                model,
                privacy_cfg: &self.privacy,
                route_kind: AgentRouteKind::InteractionOperation,
                tool_round_index: *rounds,
                tool_defs: &mut definitions,
                messages: &mut messages,
                persisted_trace_items: trace_items,
                task_plan: plan,
                loop_recorder: recorder,
                loop_guard: guard,
                trace: &mut trace,
                sort_order: next_sort_order,
                pending_action_reconciliation: action_reconciliation.blocks_interactive_input(),
            },
            &batch,
            block,
            &mut started,
        )
        .await?;
        *rounds += 1;
        action_reconciliation.observe_tool_results(batch.as_slice(), &outcome.summaries);
        let awaiting_interaction =
            super::turn_loop::awaiting_user_input_interaction_id(&outcome.summaries);
        let summary = outcome
            .summaries
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Internal("Tool dispatcher returned no result".into()))?;
        let result = ToolResult {
            call_id: summary.call_id,
            content: summary.content,
            is_error: summary.is_error,
            artifacts: summary.artifacts,
        };
        state.completed.insert(call.id, (signature, result.clone()));
        if let Some(interaction_id) = awaiting_interaction {
            create_task_checkpoint_for_turn_with_state(
                &self.input.db,
                Some(&self.input.turn_id),
                &format!("awaiting_user_input:{interaction_id}"),
                None,
            )?;
            self.input
                .events
                .send(AgentEvent::ControllerStatus {
                    code: "awaiting_user_input".into(),
                    content: "Waiting for your response".into(),
                    tone: Some("attention".into()),
                })
                .await
                .map_err(|error| CoreError::Internal(error.to_string()))?;
            return Err(CoreError::AwaitingUserInput { interaction_id });
        }
        if let Some(reason) = outcome.terminal_loop_guard_reason {
            return Err(CoreError::Agent(reason));
        }
        let visual_parts = messages
            .into_iter()
            .filter(|message| message.role == Role::User)
            .flat_map(|message| message.parts)
            .collect();
        Ok(ExternalToolOutput {
            result,
            visual_parts,
        })
    }

    pub async fn persist_steering(&self, steering: &AgentSteeringMessage) -> Result<(), CoreError> {
        if steering.recovery_control.is_some() {
            return Err(CoreError::InvalidInput(
                "Recovery controls are not conversation messages".into(),
            ));
        }
        // The tool-state lock keeps accepted user input outside a pending
        // assistant-call/result pair and gives all messages one sort owner.
        let mut state = self.state.lock().await;
        self.input.db.add_message(&ConversationMessage {
            id: Uuid::new_v4().to_string(), conversation_id:self.input.conversation_id.clone(), role:Role::User,
            content:steering.content.clone(),tool_call_id:None,tool_calls:vec![],
            artifacts:Some(serde_json::json!({"kind":"steering","turnId":self.input.turn_id,"runtime":"subscription"})),
            token_count:estimate_tokens_for_model(self.input.config.model.as_deref().unwrap_or("subscription"),&steering.content),
            created_at:String::new(),sort_order:state.next_sort_order,thinking:None,image_attachments:steering.image_attachments.clone(),
        })?;
        state.next_sort_order += 1;
        Ok(())
    }

    pub async fn persist_answer(&self, text: &str) -> Result<PersistedAssistantMessage, CoreError> {
        let mut state = self.state.lock().await;
        let message = Message::text(Role::Assistant, text.to_string());
        let id = Uuid::new_v4().to_string();
        self.input.db.add_message(&ConversationMessage {
            id: id.clone(),
            conversation_id: self.input.conversation_id.clone(),
            role: Role::Assistant,
            content: text.to_string(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: Some(
                serde_json::json!({"turnId":self.input.turn_id,"runtime":"subscription"}),
            ),
            token_count: estimate_tokens_for_model(
                self.input.config.model.as_deref().unwrap_or("subscription"),
                text,
            ),
            created_at: String::new(),
            sort_order: state.next_sort_order,
            thinking: None,
            image_attachments: None,
        })?;
        state.next_sort_order += 1;
        Ok(PersistedAssistantMessage { id, message })
    }
}
