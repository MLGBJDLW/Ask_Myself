use super::*;
use nexa_core::{
    agent::ToolVisualObservation,
    approval::{ApprovalDecision, ToolApprovalMode},
    conversation::{ConversationMessage, CreateConversationInput},
    llm::Role,
    tools::{Tool, ToolExecutionContext, ToolRegistry, ToolResult},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct NonceTool {
    calls: Arc<AtomicUsize>,
    nonce: String,
}

/// Offer the real core schemas while making accidental live-probe calls inert.
struct CatalogOnlyTool(nexa_core::llm::ToolDefinition);
#[async_trait::async_trait]
impl Tool for CatalogOnlyTool {
    fn name(&self) -> &str {
        &self.0.name
    }
    fn description(&self) -> &str {
        &self.0.description
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.0.parameters.clone()
    }
    async fn execute(&self, _context: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError> {
        Err(CoreError::InvalidInput(
            "Only read_test_nonce may execute in this integration probe".into(),
        ))
    }
}

#[test]
fn saved_subscription_configs_keep_the_native_route_and_reasoning_level() {
    let db = Database::open_memory().unwrap();
    for provider in ["github_copilot", "openai_codex"] {
        let input = serde_json::from_value(serde_json::json!({"name":provider,"provider":provider,"apiKey":"","model":"gpt-native-test","isDefault":true,"reasoningEffort":"ultra"})).unwrap();
        let saved = db.save_agent_config(&input).unwrap();
        let loaded = db.get_agent_config(&saved.id).unwrap();
        assert_eq!(loaded.provider, provider);
        assert_eq!(loaded.model, "gpt-native-test");
        assert_eq!(loaded.reasoning_effort.as_deref(), Some("ultra"));
        assert!(loaded.api_key.is_empty());
        assert!(loaded.base_url.is_none());
        assert!(SubscriptionRuntimeKind::from_provider(&loaded.provider).is_some());
    }
}
#[async_trait::async_trait]
impl Tool for NonceTool {
    fn name(&self) -> &str {
        "read_test_nonce"
    }
    fn description(&self) -> &str {
        "Read the integration test nonce. This has no external effects."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
    }
    async fn execute(&self, context: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: self.nonce.clone(),
            is_error: false,
            artifacts: None,
        })
    }
}

pub(super) fn fixture(
    kind: SubscriptionRuntimeKind,
    model: &str,
) -> (
    SubscriptionTurnRequest,
    mpsc::Receiver<AgentEvent>,
    Arc<AtomicUsize>,
    String,
) {
    let db = Arc::new(Database::open_memory().unwrap());
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "subscription".into(),
            model: model.into(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let prompt = "Call read_test_nonce exactly once, then reply with the returned nonce and nothing else. Do not use any other tool.";
    let user = ConversationMessage {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: prompt.into(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 30,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, None)
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(NonceTool {
        calls: calls.clone(),
        nonce: nonce.clone(),
    }));
    let (events, rx) = mpsc::channel(512);
    let (_steer, steering) = mpsc::unbounded_channel();
    let request = SubscriptionTurnRequest {
        kind,
        config: AgentConfig {
            model: Some(model.into()),
            max_iterations: 3,
            tool_approval_mode: ToolApprovalMode::AllowAll,
            ..AgentConfig::default()
        },
        dependencies: DesktopAgentSessionDependencies {
            tools,
            selected_skills: vec![],
            auto_loaded_skills: vec![],
            metrics: Default::default(),
        },
        db,
        conversation_id: conversation.id,
        turn_id: turn.id,
        next_sort_order: 1,
        history: vec![],
        user_parts: vec![ContentPart::Text {
            text: prompt.into(),
        }],
        events,
        cancellation: CancellationToken::new(),
        steering,
        approval: Arc::new(|_| Box::pin(async { ApprovalDecision::AllowOnce })),
        visual_interpreter: Arc::new(|_| {
            Box::pin(async {
                ToolVisualObservation::unavailable("test", "no-images", "No test images")
            })
        }),
    };
    (request, rx, calls, nonce)
}

pub(super) async fn run_live(kind: SubscriptionRuntimeKind, model: &str) {
    let (mut request, mut rx, calls, nonce) = fixture(kind, model);
    for definition in nexa_core::tools::default_tool_registry().definitions() {
        request
            .dependencies
            .tools
            .register(Box::new(CatalogOnlyTool(definition)));
    }
    let db = request.db.clone();
    let conversation = request.conversation_id.clone();
    let drain = tokio::spawn(async move {
        let mut done = 0;
        let mut deltas = 0;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Done { .. } => done += 1,
                AgentEvent::StreamBlockDelta { .. } => deltas += 1,
                _ => {}
            }
        }
        (done, deltas)
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(150), run(request))
        .await
        .expect("live runtime deadline")
        .expect("live runtime completion");
    assert!(
        result.text_content().contains(&nonce),
        "answer must use actual Nexa tool evidence"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (done, deltas) = drain.await.unwrap();
    assert_eq!(done, 1);
    assert!(deltas > 0);
    let history = db.get_messages(&conversation).unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == Role::Tool)
            .count(),
        1
    );
    assert!(history.last().unwrap().content.contains(&nonce));
}
