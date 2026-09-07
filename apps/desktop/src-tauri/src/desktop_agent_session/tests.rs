use super::*;
use async_trait::async_trait;
use nexa_core::agent_run::{AgentRunEventKind, AgentRunPhase};
use nexa_core::app_settings::ShellAccessMode;
use nexa_core::approval::{ApprovalRisk, ToolApprovalMode};
use nexa_core::conversation::{
    AgentTaskRun, CollectionContext, CreateConversationInput, ImageAttachment,
};
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::llm::ProviderType;
use nexa_core::run_event_outbox::{AgentRunEventDelivery, AgentRunEventOutboxes};
use nexa_core::runtime::{ActiveAgentTurn, AgentTurnHandle};
use nexa_core::sources::CreateSourceInput;
use nexa_core::tools::{Tool, ToolExecutionContext, ToolResult};
use nexa_core::workflow_automation::{
    SaveWorkflowAutomationInput, WorkflowAutomationApprovalPolicy, WorkflowAutomationTrigger,
};
use std::sync::atomic::{AtomicBool, Ordering};

const DESKTOP_DELEGATION_TOOL_NAMES: &[&str] = &[
    "spawn_subagent",
    "spawn_subagent_batch",
    "judge_subagent_results",
    "observe_subagent_batch",
    "observe_subagent",
    "wait_subagent",
    "send_subagent_input",
    "cancel_subagent",
    "close_subagent",
];

fn desktop_delegation_tool_registry() -> ToolRegistry {
    let runtime = DelegationRuntime::new(
        ProviderConfig {
            provider_type: ProviderType::OpenAi,
            base_url: None,
            api_key: None,
            org_id: None,
            timeout_secs: None,
            streaming: Default::default(),
        },
        AgentConfig::default(),
        None,
        None,
        SubagentLifecycleRuntime::default(),
        CancellationToken::new(),
        None,
        None,
    );
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SubagentTool::from_runtime(runtime.clone())));
    registry.register(Box::new(SubagentBatchTool::from_runtime(runtime.clone())));
    registry.register(Box::new(JudgeSubagentResultsTool::from_runtime(
        runtime.clone(),
    )));
    registry.register(Box::new(ObserveSubagentBatchTool::from_runtime(
        runtime.clone(),
    )));
    for lifecycle_tool in SubagentLifecycleTool::all(runtime) {
        registry.register(Box::new(lifecycle_tool));
    }
    registry
}

struct BlockingStopDelivery {
    entered: Arc<AtomicBool>,
}

struct UnownedDesktopTestTool;

#[async_trait]
impl Tool for UnownedDesktopTestTool {
    fn name(&self) -> &str {
        "unowned_desktop_test_tool"
    }

    fn description(&self) -> &str {
        "Injects a tool without a Package Host owner for registry policy tests."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, context: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError> {
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: "unexpected execution".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

impl AgentRunEventDelivery for BlockingStopDelivery {
    fn deliver_run_event(&self, _conversation_id: &str, event: &AgentRunEvent) {
        if event.label == "block checkpoint queue" {
            self.entered.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn deliver_task_run_snapshot(&self, _conversation_id: &str, _snapshot: AgentTaskRun) {}
}

#[test]
fn registry_health_requires_activity_runtime_core_tools() {
    assert!(missing_core_runtime_tools(&canonical_builtin_tool_registry()).is_empty());
    assert_eq!(
        missing_core_runtime_tools(&ToolRegistry::new()),
        REQUIRED_ACTIVITY_RUNTIME_TOOLS
    );
}

#[test]
fn desktop_delegation_registry_has_one_package_owner_for_every_tool() {
    let registry = desktop_delegation_tool_registry();
    assert_eq!(
        registry.tool_names(),
        DESKTOP_DELEGATION_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>()
    );

    let capabilities = PackageRuntimeAssembler::from_host(&BuiltinPackageHost)
        .expect("built-in package snapshot")
        .assemble_tool_registry(registry)
        .expect("desktop delegation tools must all have package owners");

    for tool_name in DESKTOP_DELEGATION_TOOL_NAMES {
        assert_eq!(
            capabilities.tool_owners.get(*tool_name).map(String::as_str),
            Some("delegation"),
            "{tool_name} should be owned by the delegation package"
        );
    }
}

#[test]
fn disabling_delegation_removes_every_delegation_tool_from_the_runtime_registry() {
    let db = Database::open_memory().expect("in-memory database");
    db.set_package_host_package_enabled("delegation", false)
        .expect("disable delegation package");

    let capabilities = PackageRuntimeAssembler::database_builtin(&db)
        .and_then(|assembler| assembler.assemble_tool_registry(desktop_delegation_tool_registry()))
        .expect("disabled delegation should filter a fully owned desktop registry");

    for tool_name in DESKTOP_DELEGATION_TOOL_NAMES {
        assert!(
            !capabilities.tools.contains(tool_name),
            "{tool_name} must not reach model or execution registries"
        );
        assert!(
            capabilities
                .excluded_tools
                .iter()
                .any(|excluded| excluded == tool_name),
            "{tool_name} should be reported as excluded by Package Host"
        );
    }
}

#[test]
fn unowned_tool_uses_the_previous_successful_package_projection_not_the_prefilter_registry() {
    let assembler =
        PackageRuntimeAssembler::from_host(&BuiltinPackageHost).expect("built-in package snapshot");
    let known_good = canonical_builtin_tool_registry().filtered(&["run_shell".to_string()]);
    let first = resolve_desktop_package_registry(
        &known_good,
        assembler.assemble_tool_registry(known_good.clone()),
        Some("package-generation-a"),
        None,
    );
    let last_known_good = first
        .successful_snapshot
        .as_ref()
        .expect("successful package filtering should produce a reusable projection");

    let mut current_prefilter = known_good;
    current_prefilter.register(Box::new(UnownedDesktopTestTool));
    let failed_assembly = assembler.assemble_tool_registry(current_prefilter.clone());
    assert!(
        failed_assembly.is_err(),
        "the injected tool must be unowned"
    );

    let fallback = resolve_desktop_package_registry(
        &current_prefilter,
        failed_assembly,
        Some("package-generation-a"),
        Some(last_known_good),
    );

    assert!(fallback.used_last_known_good);
    assert_eq!(fallback.tools.tool_names(), vec!["run_shell".to_string()]);
    assert!(!fallback.tools.contains("unowned_desktop_test_tool"));
    assert_ne!(fallback.tools.tool_names(), current_prefilter.tool_names());
}

#[test]
fn root_tool_allowlist_only_narrows_the_assembled_registry() {
    let registry = canonical_builtin_tool_registry();
    let original_names = registry.tool_names();
    assert!(original_names.iter().any(|name| name == "run_shell"));
    assert!(original_names.iter().any(|name| name == "tool_search"));

    let filtered = filter_root_tool_registry(
        registry,
        &[
            " run_shell ".to_string(),
            "not_a_registered_tool".to_string(),
        ],
    );
    assert_eq!(filtered.tool_names(), vec!["run_shell".to_string()]);
}

#[test]
fn desktop_allow_all_never_bypasses_computer_or_screen_disclosure_approval() {
    let control = ApprovalRequest::new(
        "control",
        "computer_control",
        &serde_json::json!({
            "action": "click",
            "observation_id": "observation",
            "window_id": 42,
            "x": 1,
            "y": 1
        }),
        ApprovalRisk::High,
        "control",
    );
    let capture = ApprovalRequest::new(
        "capture",
        "computer_observe",
        &serde_json::json!({
            "action": " CAPTURE_WINDOW ",
            "observation_id": "observation",
            "window_id": 42
        }),
        ApprovalRisk::Medium,
        "capture",
    );
    let browser = ApprovalRequest::new(
        "browser",
        "browser_session",
        &serde_json::json!({
            "action": "click",
            "sessionId": "browser-a",
            "observationId": "obs-a",
            "targetRef": "e1"
        }),
        ApprovalRisk::High,
        "browser action",
    );
    assert!(requires_explicit_desktop_approval(&control));
    assert!(requires_explicit_desktop_approval(&capture));
    assert!(requires_explicit_desktop_approval(&browser));
    assert_eq!(
        desktop_approval_mode_decision(ToolApprovalMode::AllowAll, &control),
        None
    );
    assert_eq!(
        desktop_approval_mode_decision(ToolApprovalMode::AllowAll, &capture),
        None
    );
    assert_eq!(
        desktop_approval_mode_decision(ToolApprovalMode::AllowAll, &browser),
        None
    );
    assert_eq!(
        desktop_approval_mode_decision(ToolApprovalMode::DenyAll, &control),
        Some(ApprovalDecision::Deny)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_fences_execution_and_resolves_approval_before_checkpoint() {
    let db = Database::open_memory().expect("open memory database");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "test".to_string(),
            model: "test-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("create conversation");
    let message = ConversationMessage {
        id: "stop-user".to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Stop before the side effect".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&message).expect("add user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &message.id, None)
        .expect("create turn");
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &message.id,
            "Stop boundary",
            Some("test"),
            Some("test-model"),
        )
        .expect("create run");
    db.mark_agent_task_run_started(&run.id, "approval")
        .expect("start run");
    let activity_runtime = nexa_core::activity::ActivityRuntime::with_database(db.clone()).unwrap();
    activity_runtime
        .start(
            nexa_core::activity::ActivitySpec::new(
                nexa_core::activity::ActivitySurface::Desktop,
                "computer_control",
            )
            .with_activity_id("computer_control:turn:call_0")
            .with_conversation_id(&conversation.id)
            .with_turn_id(&turn.id),
        )
        .expect("start interactive action receipt");
    activity_runtime
        .transition(
            "computer_control:turn:call_0",
            nexa_core::activity::ActivityState::Completed,
            serde_json::json!({"stage": "observed"}),
        )
        .expect("complete interactive action receipt");

    let delivery_entered = Arc::new(AtomicBool::new(false));
    let executor = DatabaseExecutor::new(db.clone(), 8).expect("database executor");
    let outbox = AgentRunEventOutboxes::new(
        executor,
        Arc::new(BlockingStopDelivery {
            entered: Arc::clone(&delivery_entered),
        }),
    )
    .open(&conversation.id, &run.id)
    .await
    .expect("open outbox");

    let approval_request = ApprovalRequest::new(
        "approval-stop",
        "write_file",
        &serde_json::json!({ "path": "notes.md" }),
        ApprovalRisk::High,
        "write notes",
    );
    outbox
        .submit(
            AgentRunEvent::from_agent_event(&AgentEvent::ApprovalRequested {
                request: approval_request,
            })
            .with_context(Some(&run.id), Some(&turn.id), None),
        )
        .expect("submit approval request");
    outbox.flush().await.expect("persist approval request");

    let pending_approvals = Arc::new(TokioMutex::new(HashMap::new()));
    let (approval_sender, _approval_receiver) = tokio::sync::oneshot::channel();
    pending_approvals.lock().await.insert(
        "approval-stop".to_string(),
        crate::commands::PendingToolApproval {
            task_run_id: run.id.clone(),
            sender: approval_sender,
        },
    );

    outbox
        .submit(
            AgentRunEvent::from_agent_event(&AgentEvent::Status {
                content: "block checkpoint queue".to_string(),
                tone: Some("running".to_string()),
            })
            .with_context(Some(&run.id), Some(&turn.id), None),
        )
        .expect("submit blocking event");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !delivery_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delivery entered blocking section");

    let late_side_effect = Arc::new(AtomicBool::new(false));
    let late_side_effect_for_task = Arc::clone(&late_side_effect);
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        late_side_effect_for_task.store(true, Ordering::SeqCst);
    });
    let (steering_tx, _steering_rx) = mpsc::unbounded_channel();
    let active_turn = ActiveAgentTurn {
        handle: AgentTurnHandle::running(&conversation.id, &run.id, &turn.id),
        cancel_token: CancellationToken::new(),
        task,
        steering_tx,
        event_outbox: Arc::clone(&outbox),
        orchestrator_run_id: None,
        frontend_paint_recorded: AtomicBool::new(false),
    };

    fence_and_checkpoint_desktop_agent_turn(active_turn, &db, &pending_approvals)
        .await
        .expect("create resumable stop");

    assert!(
        !late_side_effect.load(Ordering::SeqCst),
        "executor must be fenced before checkpoint persistence can block"
    );
    assert!(pending_approvals.lock().await.is_empty());
    let events = db.list_agent_run_events(&run.id).expect("run event ledger");
    let resolved_index = events
        .iter()
        .position(|event| event.kind == AgentRunEventKind::ApprovalResolved)
        .expect("approval resolution event");
    let pause_index = events
        .iter()
        .position(|event| {
            event.phase == AgentRunPhase::Paused && event.status.as_deref() == Some("paused")
        })
        .expect("pause checkpoint event");
    assert!(resolved_index < pause_index);
    assert_eq!(
        events[resolved_index].payload["requestId"],
        serde_json::json!("approval-stop")
    );
    assert_eq!(
        events[resolved_index].payload["decision"],
        serde_json::json!("deny")
    );
    assert_eq!(
        db.get_agent_task_run(&run.id).expect("paused task").status,
        "paused"
    );
    let checkpoint = db
        .latest_task_resume_checkpoint(&run.id)
        .expect("load stop checkpoint")
        .expect("stop checkpoint");
    assert!(checkpoint
        .reason
        .starts_with("user_stop_requires_action_reconciliation:"));
    assert!(checkpoint
        .resume_prompt
        .contains("SAFETY FENCE: interactive action receipt"));
    assert!(checkpoint
        .resume_prompt
        .contains("Never redispatch the prior action"));
}

#[test]
fn outbox_failure_reconciler_preserves_task_reason_and_closes_host_projections() {
    let db = Database::open_memory().expect("open memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "gpt-test".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("create conversation");
    let message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Run the automated task".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&message).expect("add user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &message.id, Some("workflow"))
        .expect("create conversation turn");
    let task = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &message.id,
            "Automated task",
            Some("open_ai"),
            Some("gpt-test"),
        )
        .expect("create task run");
    db.mark_agent_task_run_started(&task.id, "responding")
        .expect("start task run");

    let automation = db
        .save_workflow_automation(&SaveWorkflowAutomationInput {
            id: None,
            name: "Outbox reconciliation".to_string(),
            description: "Exercise host lifecycle reconciliation".to_string(),
            workflow_template_id: "report_brief".to_string(),
            prompt: "Run the automated task".to_string(),
            trigger: WorkflowAutomationTrigger::Manual,
            source_scope: Vec::new(),
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: false,
                allowed_tools: Vec::new(),
                risk_level: "low".to_string(),
            },
            enabled: true,
        })
        .expect("save workflow automation");
    let orchestrator = db
        .record_workflow_automation_run(&automation.id, None, "queued", Some("queued"))
        .expect("queue workflow run");
    db.start_workflow_automation_run(&orchestrator.id, &task.id, Some("running"))
        .expect("start workflow run");

    let outbox_failure = AgentRunEventOutboxFailure::Persistence {
        message: "forced persistence failure".to_string(),
    };
    assert!(!reconcile_authoritative_run_event_outbox_failure(
        &db,
        &task.id,
        Some(&orchestrator.id),
        &turn.id,
        &outbox_failure,
    ));
    assert_eq!(
        db.get_agent_task_run(&task.id)
            .expect("active task remains authoritative")
            .status,
        "running"
    );
    assert_eq!(
        db.get_conversation_turn(&turn.id)
            .expect("active turn remains open")
            .status,
        "running"
    );
    assert_eq!(
        db.get_workflow_automation_run(&orchestrator.id)
            .expect("active workflow remains open")
            .status,
        nexa_core::workflow_automation::WorkflowAutomationRunStatus::Running
    );

    let authoritative_artifacts = serde_json::json!({
        "reason": "run_event_persistence_failed",
        "diagnostic": "owned by outbox",
    });
    db.finish_agent_task_run(
        &task.id,
        "failed",
        Some("Run Event outbox failed closed"),
        Some("run_event_persistence_failed"),
        Some(&authoritative_artifacts),
    )
    .expect("project authoritative outbox failure");

    assert!(reconcile_authoritative_run_event_outbox_failure(
        &db,
        &task.id,
        Some(&orchestrator.id),
        &turn.id,
        &outbox_failure,
    ));

    let failed_task = db.get_agent_task_run(&task.id).expect("failed task");
    assert_eq!(failed_task.status, "failed");
    assert_eq!(
        failed_task.error_message.as_deref(),
        Some("run_event_persistence_failed")
    );
    assert_eq!(failed_task.artifacts, Some(authoritative_artifacts));
    let failed_turn = db
        .get_conversation_turn(&turn.id)
        .expect("failed conversation turn");
    assert_eq!(failed_turn.status, "error");
    assert_eq!(
        failed_turn
            .trace
            .as_ref()
            .and_then(|trace| trace
                .get("runEventOutboxFailure")
                .and_then(|failure| failure.get("reason")))
            .and_then(serde_json::Value::as_str),
        Some("run_event_persistence_failed")
    );
    let failed_orchestrator = db
        .get_workflow_automation_run(&orchestrator.id)
        .expect("failed workflow run");
    assert_eq!(failed_orchestrator.status.as_str(), "failed");
    assert_eq!(
        failed_orchestrator.summary.as_deref(),
        Some("Run Event outbox failed closed")
    );
    assert!(failed_orchestrator.finished_at.is_some());
}

#[test]
fn registry_snapshot_generation_changes_with_mcp_configuration() {
    let assembler =
        PackageRuntimeAssembler::from_host(&BuiltinPackageHost).expect("built-in package snapshot");
    let mut server = nexa_core::mcp::McpServer {
        id: "mcp-1".to_string(),
        name: "Search".to_string(),
        transport: "streamable_http".to_string(),
        command: None,
        args: None,
        url: Some("https://mcp.example.test".to_string()),
        env_json: None,
        headers_json: Some(r#"{"Authorization":"Bearer first"}"#.to_string()),
        enabled: true,
        created_at: "2026-08-02T00:00:00Z".to_string(),
        updated_at: "2026-08-02T00:00:00Z".to_string(),
        builtin_id: None,
    };

    let first =
        desktop_tool_registry_generation(&assembler, &[server.clone()]).expect("first generation");
    server.headers_json = Some(r#"{"Authorization":"Bearer second"}"#.to_string());
    let second =
        desktop_tool_registry_generation(&assembler, &[server]).expect("second generation");

    assert_ne!(first, second);
}

fn test_agent_config() -> DbAgentConfig {
    DbAgentConfig {
        id: "agent-config-1".to_string(),
        name: "Primary".to_string(),
        provider: "open_ai".to_string(),
        api_key: "test-key".to_string(),
        base_url: None,
        model: "gpt-test".to_string(),
        temperature: Some(0.2),
        max_tokens: Some(1024),
        context_window: Some(128_000),
        is_default: true,
        reasoning_enabled: Some(true),
        thinking_budget: Some(4096),
        reasoning_effort: Some("medium".to_string()),
        max_iterations: Some(7),
        summarization_model: Some("gpt-summary".to_string()),
        summarization_provider: None,
        image_generation_model: None,
        subagent_allowed_tools: None,
        subagent_allowed_skill_ids: None,
        subagent_max_parallel: Some(2),
        subagent_max_calls_per_turn: Some(3),
        subagent_token_budget: Some(4096),
        delegation_limits_v2: None,
        tool_timeout_secs: None,
        agent_timeout_secs: None,
        provider_streaming: Default::default(),
        provider_endpoint_id: None,
        model_id: None,
        model_selection_resolution: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn test_provider_config(provider_type: ProviderType) -> ProviderConfig {
    ProviderConfig {
        provider_type,
        api_key: Some("test-key".to_string()),
        base_url: None,
        org_id: None,
        timeout_secs: None,
        streaming: Default::default(),
    }
}

fn successful_project_turn(db: &Database, conversation_id: &str) -> String {
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        role: Role::User,
        content: "Use project context".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 0,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message)
        .expect("add project user message");
    let turn = db
        .create_conversation_turn(conversation_id, &user_message.id, None)
        .expect("create project turn");
    db.finalize_conversation_turn(&turn.id, "success", None, None)
        .expect("finish project turn");
    turn.id
}

#[test]
fn local_provider_detection_requires_an_exact_loopback_host() {
    let mut config = test_provider_config(ProviderType::Ollama);
    config.base_url = Some("http://127.example.com:11434".to_string());
    assert!(!provider_config_is_local(&config));

    config.base_url = Some("http://127.0.0.1:11434".to_string());
    assert!(provider_config_is_local(&config));

    config.base_url = Some("http://[::1]:11434".to_string());
    assert!(provider_config_is_local(&config));

    config.base_url = Some("https://remote-ollama.example.com".to_string());
    assert!(!provider_config_is_local(&config));
}

#[test]
fn desktop_agent_user_content_parts_project_attachments() {
    let db = Database::open_memory().expect("open memory db");
    let mut db_config = test_agent_config();
    db_config.model = "gpt-4o".to_string();
    let attachments = vec![
        ImageAttachment {
            base64_data: "image-data".to_string(),
            media_type: "image/png".to_string(),
            original_name: "diagram.png".to_string(),
            attachment_id: None,
            attachment_hash: None,
            vision_analysis: None,
        },
        ImageAttachment {
            base64_data: base64::engine::general_purpose::STANDARD
                .encode("hello from attachment. ".repeat(8)),
            media_type: "text/plain".to_string(),
            original_name: "notes.txt".to_string(),
            attachment_id: None,
            attachment_hash: None,
            vision_analysis: None,
        },
    ];

    let parts = build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
        db: &db,
        app_handle: None,
        provider_config: &test_provider_config(ProviderType::OpenAi),
        db_config: &db_config,
        message: "Read these",
        attachments: Some(&attachments),
    })
    .expect("build user content parts");

    assert_eq!(parts.len(), 3);
    match &parts[0] {
        ContentPart::Text { text } => assert_eq!(text, "Read these"),
        other => panic!("unexpected first part: {other:?}"),
    }
    match &parts[1] {
        ContentPart::Image { media_type, data } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(data, "image-data");
        }
        other => panic!("unexpected image part: {other:?}"),
    }
    match &parts[2] {
        ContentPart::Text { text } => {
            assert!(text.contains("[Attached file: notes.txt]"));
            assert!(text.contains("hello from attachment"));
        }
        other => panic!("unexpected attachment part: {other:?}"),
    }
}

#[test]
fn desktop_agent_user_content_parts_return_decode_errors() {
    let db = Database::open_memory().expect("open memory db");
    let attachment = ImageAttachment {
        base64_data: "%not-base64".to_string(),
        media_type: "text/plain".to_string(),
        original_name: "broken.txt".to_string(),
        attachment_id: None,
        attachment_hash: None,
        vision_analysis: None,
    };

    let err = build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
        db: &db,
        app_handle: None,
        provider_config: &test_provider_config(ProviderType::OpenAi),
        db_config: &test_agent_config(),
        message: "Read this",
        attachments: Some(&[attachment]),
    })
    .expect_err("invalid base64 should fail");

    assert!(err.contains("Failed to decode attachment"));
}

#[test]
fn desktop_summarization_provider_config_requires_provider_override() {
    let db = Database::open_memory().expect("open memory db");
    let db_config = test_agent_config();

    assert!(
        resolve_desktop_summarization_provider_config(&db, &db_config)
            .expect("resolve no override")
            .is_none()
    );

    let mut with_provider = db_config;
    with_provider.provider = "openai".to_string();
    with_provider.summarization_provider = Some("open_ai".to_string());
    with_provider.base_url = Some("https://example.test/v1".to_string());

    let (config, name, model) = resolve_desktop_summarization_provider_config(&db, &with_provider)
        .expect("resolve provider override")
        .expect("provider override");

    assert_eq!(config.provider_type, ProviderType::OpenAi);
    assert_eq!(config.api_key.as_deref(), Some("test-key"));
    assert_eq!(config.base_url.as_deref(), Some("https://example.test/v1"));
    assert_eq!(config.timeout_secs, None);
    assert_eq!(name, "Primary");
    assert_eq!(model, "gpt-summary");
}

#[test]
fn desktop_summarization_provider_config_rejects_cross_provider_credential_reuse() {
    let db = Database::open_memory().expect("open memory db");
    let mut db_config = test_agent_config();
    db_config.summarization_provider = Some("deepseek".to_string());

    let error = resolve_desktop_summarization_provider_config(&db, &db_config)
        .expect_err("cross-provider summary needs independent saved credentials");

    assert!(error.contains("own saved provider configuration"));
    assert!(error.contains("never reused across providers"));
}

#[test]
fn desktop_memory_extraction_provider_config_uses_summary_overrides() {
    let mut db_config = test_agent_config();
    db_config.provider = "ollama".to_string();
    db_config.model = "llama3".to_string();
    db_config.summarization_model = Some("gpt-summary".to_string());

    let fallback = desktop_memory_extraction_provider_config(&db_config);
    assert_eq!(desktop_memory_extraction_model(&db_config), "gpt-summary");
    assert_eq!(fallback.provider_type, ProviderType::Ollama);

    db_config.summarization_provider = Some("open_ai".to_string());
    let override_config = desktop_memory_extraction_provider_config(&db_config);

    assert_eq!(override_config.provider_type, ProviderType::Ollama);
    assert_eq!(override_config.api_key.as_deref(), Some("test-key"));

    db_config.base_url = Some("https://api.deepseek.com/v1".to_string());
    let sniffed_config = desktop_memory_extraction_provider_config(&db_config);

    assert_eq!(sniffed_config.provider_type, ProviderType::DeepSeek);
}

#[test]
fn project_workspace_instructions_are_live_and_episodes_are_evidence() {
    let db = Database::open_memory().expect("open memory db");
    let project = db
        .create_project(&CreateProjectInput {
            name: "Workspace".to_string(),
            description: Some("Ship an auditable runtime".to_string()),
            icon: None,
            color: None,
            system_prompt: Some("Follow workspace instruction v1".to_string()),
            source_scope: None,
        })
        .expect("create project");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "gpt-test".to_string(),
            system_prompt: Some("Conversation-specific instruction".to_string()),
            collection_context: None,
            project_id: Some(project.id.clone()),
            persona_id: None,
        })
        .expect("create conversation");
    let legacy_snapshot = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "gpt-test".to_string(),
            system_prompt: Some("Follow workspace instruction v1".to_string()),
            collection_context: None,
            project_id: Some(project.id.clone()),
            persona_id: None,
        })
        .expect("create legacy project conversation");
    db.mark_legacy_project_system_prompt_ambiguous(&legacy_snapshot.id)
        .expect("mark copied legacy project prompt");
    let observed_turn_id = successful_project_turn(&db, &conversation.id);
    db.record_project_turn_completion(
        &conversation.id,
        &observed_turn_id,
        "run-observed",
        "Visible prior result with provenance",
    )
    .expect("record project episode");

    let db_config = test_agent_config();
    let app_cfg = AppConfig::default();
    fn request<'a>(
        db: &'a Database,
        conversation: &'a Conversation,
        db_config: &'a DbAgentConfig,
        app_cfg: &'a AppConfig,
    ) -> DesktopAgentTurnConfigRequest<'a> {
        DesktopAgentTurnConfigRequest {
            db,
            conversation,
            turn_id: "turn-current",
            message: "Use the prior result",
            persona_id: None,
            explicit_skill_ids: &[],
            db_config,
            app_cfg,
            execution_mode: AgentExecutionMode::Normal,
            power_mode: AgentPowerMode::Standard,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Balanced,
            custom_orchestration: None,
        }
    }
    let initial =
        build_desktop_agent_turn_config(request(&db, &conversation, &db_config, &app_cfg))
            .executor_config;
    assert!(initial
        .system_prompt
        .contains("Follow workspace instruction v1"));
    let volatile = initial.volatile_system_sections.join("\n");
    assert!(volatile.contains("Visible prior result with provenance"));
    assert!(volatile.contains(&format!("turn:{observed_turn_id}")));
    assert!(!initial
        .system_prompt
        .contains("Visible prior result with provenance"));

    db.update_project(
        &project.id,
        &UpdateProjectInput {
            name: None,
            description: None,
            icon: None,
            color: None,
            system_prompt: Some("Follow workspace instruction v2".to_string()),
            source_scope: None,
            archived: None,
        },
    )
    .expect("update live project instruction");
    let refreshed_conversation = db
        .get_conversation(&conversation.id)
        .expect("reload conversation");
    let refreshed = build_desktop_agent_turn_config(request(
        &db,
        &refreshed_conversation,
        &db_config,
        &app_cfg,
    ))
    .executor_config;
    assert!(refreshed
        .system_prompt
        .contains("Follow workspace instruction v2"));
    assert!(!refreshed
        .system_prompt
        .contains("Follow workspace instruction v1"));
    assert!(refreshed
        .system_prompt
        .contains("Conversation-specific instruction"));

    let legacy_after_project_edit = db
        .get_conversation(&legacy_snapshot.id)
        .expect("reload legacy project conversation");
    let migrated = build_desktop_agent_turn_config(request(
        &db,
        &legacy_after_project_edit,
        &db_config,
        &app_cfg,
    ))
    .executor_config;
    assert!(migrated
        .system_prompt
        .contains("Follow workspace instruction v2"));
    assert!(!migrated
        .system_prompt
        .contains("Follow workspace instruction v1"));

    db.update_conversation_system_prompt(&legacy_snapshot.id, "Explicit conversation override")
        .expect("save explicit conversation prompt");
    let explicit = db
        .get_conversation(&legacy_snapshot.id)
        .expect("reload explicit prompt");
    let explicit_config =
        build_desktop_agent_turn_config(request(&db, &explicit, &db_config, &app_cfg))
            .executor_config;
    assert!(explicit_config
        .system_prompt
        .contains("Explicit conversation override"));
    assert!(explicit_config
        .system_prompt
        .contains("Follow workspace instruction v2"));
}

#[test]
fn desktop_agent_turn_config_projects_prompt_and_executor_fields() {
    let db = Database::open_memory().expect("open memory db");
    let root = std::env::temp_dir().join(format!("nexa-turn-config-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create source root");
    let source = db
        .add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("add source");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "gpt-test".to_string(),
            system_prompt: Some("System base".to_string()),
            collection_context: Some(CollectionContext {
                title: "Research Set".to_string(),
                description: Some("Scoped notes".to_string()),
                query_text: Some("agent runtime".to_string()),
                source_ids: vec![source.id.clone()],
            }),
            project_id: None,
            persona_id: None,
        })
        .expect("create conversation");
    db.set_conversation_sources(&conversation.id, &[source.id.clone()])
        .expect("set source scope");

    let mut app_cfg = AppConfig::default();
    app_cfg.tool_approval_mode = ToolApprovalMode::DenyAll;
    app_cfg.shell_access_mode = ShellAccessMode::Open;
    app_cfg.dynamic_tool_visibility = false;
    app_cfg.trace_enabled = false;
    app_cfg.confirm_destructive = true;

    let explicit_skill_ids = vec![
        "builtin-evidence-first".to_string(),
        "explicit-skill".to_string(),
    ];
    let mut db_config = test_agent_config();
    db_config.tool_timeout_secs = Some(37);
    db_config.agent_timeout_secs = Some(91);
    let turn_config = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &db,
        conversation: &conversation,
        turn_id: "turn-1",
        message: "Summarize runtime evidence",
        persona_id: None,
        explicit_skill_ids: &explicit_skill_ids,
        db_config: &db_config,
        app_cfg: &app_cfg,
        execution_mode: AgentExecutionMode::Plan,
        power_mode: AgentPowerMode::Standard,
        collaboration_mode: AgentCollaborationMode::Direct,
        moa_preset: MoaPresetId::FastReview,
        orchestration_profile: OrchestrationProfile::Balanced,
        custom_orchestration: None,
    });

    assert_eq!(turn_config.source_scope_ids, vec![source.id.clone()]);
    assert!(turn_config
        .pinned_skill_ids
        .contains(&"builtin-evidence-first".to_string()));
    assert!(turn_config
        .pinned_skill_ids
        .contains(&"builtin-visual-explanations".to_string()));
    assert!(turn_config
        .pinned_skill_ids
        .contains(&"explicit-skill".to_string()));
    assert_eq!(
        turn_config
            .pinned_skill_ids
            .iter()
            .filter(|id| id.as_str() == "builtin-evidence-first")
            .count(),
        1
    );

    let executor = turn_config.executor_config;
    assert_eq!(executor.max_iterations, 7);
    assert_eq!(executor.model.as_deref(), Some("gpt-test"));
    assert_eq!(executor.temperature, Some(0.2));
    assert_eq!(
        executor.max_tokens, None,
        "plan turns must not inherit retired per-request caps"
    );
    assert_eq!(executor.context_window, Some(128_000));
    assert_eq!(executor.catalog_limits_authoritative, Some(false));
    assert_eq!(executor.reasoning_enabled, Some(true));
    assert_eq!(executor.thinking_budget, Some(4096));
    assert_eq!(executor.provider_type, Some(ProviderType::OpenAi));
    assert_eq!(executor.summarization_model.as_deref(), Some("gpt-summary"));
    assert_eq!(executor.subagent_max_parallel, Some(2));
    assert_eq!(executor.subagent_max_calls_per_turn, Some(3));
    assert_eq!(executor.subagent_token_budget, Some(4096));
    assert_eq!(executor.tool_timeout_secs, Some(37));
    assert_eq!(executor.agent_timeout_secs, Some(91));
    assert_eq!(executor.subagent_verification_reserve_percent, None);
    assert!(!executor.dynamic_tool_visibility);
    assert!(!executor.trace_enabled);
    assert!(executor.require_tool_confirmation);
    assert_eq!(executor.shell_access_mode, ShellAccessMode::Open);
    assert_eq!(executor.tool_approval_mode, ToolApprovalMode::DenyAll);
    assert_eq!(executor.execution_mode, AgentExecutionMode::Plan);
    assert_eq!(executor.power_mode, AgentPowerMode::Standard);
    let standard_limits = executor
        .delegation_limits_v2
        .as_ref()
        .expect("legacy/empty desktop configs are upgraded to independent V2 limits");
    assert_eq!(standard_limits.input_context_limit, None);
    assert_eq!(standard_limits.max_output_tokens_per_step, None);
    assert_eq!(standard_limits.max_actual_tokens_per_worker, None);

    let prompt_sections = executor.volatile_system_sections.join("\n");
    assert!(prompt_sections.contains("## Current Turn Time"));
    assert!(prompt_sections.contains("## Active Source Scope"));
    assert!(prompt_sections.contains(root.to_string_lossy().as_ref()));
    assert!(prompt_sections.contains("Research Set"));
    assert!(prompt_sections.contains("Plan Mode"));

    let mut nexus_db_config = test_agent_config();
    nexus_db_config.model = "gpt-5.6".to_string();
    nexus_db_config.delegation_limits_v2 = Some(nexa_core::agent::DelegationLimitsConfig {
        total_actual_tokens_soft_limit: Some(32_000),
        max_parallel: Some(3),
        max_calls_per_turn: Some(6),
        queue_deadline_ms: Some(9_000),
        ..Default::default()
    });
    let nexus = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &db,
        conversation: &conversation,
        turn_id: "turn-2",
        message: "Verify a difficult cross-module change",
        persona_id: None,
        explicit_skill_ids: &[],
        db_config: &nexus_db_config,
        app_cfg: &app_cfg,
        execution_mode: AgentExecutionMode::Normal,
        power_mode: AgentPowerMode::Nexus,
        collaboration_mode: AgentCollaborationMode::Direct,
        moa_preset: MoaPresetId::FastReview,
        orchestration_profile: OrchestrationProfile::Balanced,
        custom_orchestration: None,
    })
    .executor_config;
    assert_eq!(nexus.max_iterations, 7);
    assert_eq!(nexus.reasoning_effort, Some(ReasoningEffort::Max));
    assert_eq!(nexus.subagent_max_parallel, Some(3));
    assert_eq!(nexus.subagent_max_calls_per_turn, Some(6));
    assert_eq!(nexus.subagent_token_budget, Some(32_000));
    assert_eq!(nexus.subagent_verification_reserve_percent, Some(25));
    let nexus_limits = nexus
        .delegation_limits_v2
        .as_ref()
        .expect("saved V2 limits remain available");
    assert_eq!(nexus_limits.max_parallel, Some(3));
    assert_eq!(nexus_limits.max_calls_per_turn, Some(6));
    assert_eq!(nexus_limits.total_actual_tokens_soft_limit, Some(32_000));
    assert_eq!(nexus_limits.max_output_tokens_per_step, None);
    assert_eq!(nexus_limits.max_actual_tokens_per_worker, None);
    assert_eq!(nexus_limits.queue_deadline_ms, Some(9_000));
    assert_eq!(nexus.power_mode, AgentPowerMode::Nexus);
    assert!(nexus
        .volatile_system_sections
        .join("\n")
        .contains("## Nexus Execution Policy"));

    let mut nexus_auto_db_config = test_agent_config();
    nexus_auto_db_config.model = "qwen3.8-max".to_string();
    nexus_auto_db_config.delegation_limits_v2 = None;
    nexus_auto_db_config.subagent_max_parallel = None;
    nexus_auto_db_config.subagent_max_calls_per_turn = None;
    nexus_auto_db_config.subagent_token_budget = None;
    let nexus_auto = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &db,
        conversation: &conversation,
        turn_id: "turn-nexus-auto",
        message: "Run a deep parallel review",
        persona_id: None,
        explicit_skill_ids: &[],
        db_config: &nexus_auto_db_config,
        app_cfg: &app_cfg,
        execution_mode: AgentExecutionMode::Normal,
        power_mode: AgentPowerMode::Nexus,
        collaboration_mode: AgentCollaborationMode::Direct,
        moa_preset: MoaPresetId::FastReview,
        orchestration_profile: OrchestrationProfile::Balanced,
        custom_orchestration: None,
    })
    .executor_config;
    let nexus_auto_limits = nexus_auto
        .delegation_limits_v2
        .expect("Nexus creates independent auto limits even without saved V2 settings");
    assert_eq!(nexus_auto_limits.input_context_limit, None);
    assert_eq!(nexus_auto_limits.max_output_tokens_per_worker, None);
    assert_eq!(nexus_auto_limits.total_actual_tokens_soft_limit, None);
    assert_eq!(nexus_auto_limits.max_output_tokens_per_step, None);
    assert_eq!(nexus_auto_limits.max_actual_tokens_per_worker, None);
    assert_eq!(nexus_auto_limits.max_calls_per_turn, None);

    let mut unlimited_db_config = test_agent_config();
    unlimited_db_config.max_iterations = None;
    unlimited_db_config.delegation_limits_v2 = Some(nexa_core::agent::DelegationLimitsConfig {
        total_actual_tokens_soft_limit: Some(32_000),
        max_parallel: Some(3),
        max_calls_per_turn: Some(6),
        run_deadline_ms: Some(240_000),
        ..Default::default()
    });
    let custom = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &db,
        conversation: &conversation,
        turn_id: "turn-3",
        message: "Apply a bounded custom orchestration policy",
        persona_id: None,
        explicit_skill_ids: &[],
        db_config: &unlimited_db_config,
        app_cfg: &app_cfg,
        execution_mode: AgentExecutionMode::Normal,
        power_mode: AgentPowerMode::Standard,
        collaboration_mode: AgentCollaborationMode::Direct,
        moa_preset: MoaPresetId::FastReview,
        orchestration_profile: OrchestrationProfile::Custom,
        custom_orchestration: Some(CustomOrchestrationOptions {
            max_iterations: Some(48),
            max_parallel: Some(7),
            max_calls_per_turn: Some(15),
            delegated_token_budget: Some(77_000),
            ..Default::default()
        }),
    })
    .executor_config;
    assert_eq!(custom.max_iterations, 48);
    assert_eq!(custom.subagent_max_parallel, Some(7));
    assert_eq!(custom.subagent_max_calls_per_turn, Some(15));
    assert_eq!(custom.subagent_token_budget, Some(77_000));
    let custom_limits = custom
        .delegation_limits_v2
        .expect("custom turn merges into saved V2 limits");
    assert_eq!(custom_limits.max_parallel, Some(7));
    assert_eq!(custom_limits.max_calls_per_turn, Some(15));
    assert_eq!(custom_limits.total_actual_tokens_soft_limit, Some(77_000));
    assert_eq!(custom_limits.run_deadline_ms, Some(240_000));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_learned_memory_does_not_call_embedding_before_the_model_request() {
    use nexa_core::embed::{create_embedder, EmbedderConfig};
    use std::io::{BufRead, Read, Write};
    use std::sync::atomic::AtomicUsize;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let server_calls = calls.clone();
    let server_finished = finished.clone();
    let server = std::thread::spawn(move || {
        while !server_finished.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    // A TCP read is not an HTTP request. Closing with an unread
                    // POST body can reset the connection and make the embedder
                    // retry, corrupting the request-count regression signal.
                    let mut request = std::io::BufReader::new(&mut socket);
                    let mut line = String::new();
                    if request.read_line(&mut line).unwrap() == 0 {
                        continue;
                    }
                    assert!(line.starts_with("POST "));
                    let mut content_length = None;
                    loop {
                        line.clear();
                        assert!(request.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                content_length = Some(value.trim().parse::<usize>().unwrap());
                            }
                        }
                    }
                    let length = content_length.expect("JSON POST must declare its length");
                    assert!(length < 64 * 1024);
                    let mut body = vec![0; length];
                    request.read_exact(&mut body).unwrap();
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap();
                    drop(request);
                    server_calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let body = r#"{"data":[{"index":0,"embedding":[1.0,0.0,0.0]}]}"#;
                    write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(error) => panic!("embedding fixture: {error}"),
            }
        }
    });
    let config = EmbedderConfig {
        provider: "api".into(),
        api_key: "synthetic-key".into(),
        api_base_url: format!("http://{address}/v1"),
        api_model: "synthetic-embedding".into(),
        vector_dimensions: 3,
        ..Default::default()
    };
    // Establish that this exact HTTP fixture is reachable before using zero
    // requests as the regression signal. No environment/proxy mutation.
    create_embedder(&config)
        .unwrap()
        .embed("fixture preflight")
        .unwrap();
    assert_eq!(calls.swap(0, Ordering::SeqCst), 1);
    let db = Database::open_memory().unwrap();
    db.save_embedder_config(&config).unwrap();
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".into(),
            model: "gpt-test".into(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let db_config = test_agent_config();
    let app_cfg = AppConfig::default();
    let started = std::time::Instant::now();
    let _turn = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &db,
        conversation: &conversation,
        turn_id: "startup-test",
        message: "Synthetic query",
        persona_id: None,
        explicit_skill_ids: &[],
        db_config: &db_config,
        app_cfg: &app_cfg,
        execution_mode: AgentExecutionMode::Plan,
        power_mode: AgentPowerMode::Standard,
        collaboration_mode: AgentCollaborationMode::Direct,
        moa_preset: MoaPresetId::FastReview,
        orchestration_profile: OrchestrationProfile::Balanced,
        custom_orchestration: None,
    });
    let elapsed = started.elapsed();
    let empty_calls = calls.load(Ordering::SeqCst);
    db.add_message(&nexa_core::conversation::ConversationMessage {
        id: "learning-fixture-message".into(),
        conversation_id: conversation.id.clone(),
        role: nexa_core::llm::Role::Assistant,
        content: "Learned answer".into(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 1,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    })
    .unwrap();
    let learned_id = db
        .insert_learned_success(
            "Synthetic query",
            "Learned answer",
            "learning-fixture-message",
        )
        .unwrap();
    let unindexed =
        nexa_core::learning::retrieve_similar_successes(&db, "Synthetic query", 3).unwrap();
    let unindexed_calls = calls.load(Ordering::SeqCst);
    db.update_learned_success_embedding(&learned_id, &[1.0, 0.0, 0.0])
        .unwrap();
    let indexed =
        nexa_core::learning::retrieve_similar_successes(&db, "Synthetic query", 3).unwrap();
    finished.store(true, Ordering::SeqCst);
    server.join().unwrap();
    eprintln!(
        "empty_learning_context elapsed_ms={} embedding_calls={empty_calls}",
        elapsed.as_millis()
    );
    assert_eq!(
        empty_calls, 0,
        "empty learned memory must not delay the real model request with an embedding call"
    );
    assert!(unindexed.is_empty());
    assert_eq!(
        unindexed_calls, 0,
        "unindexed examples do not justify a network request"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "indexed examples still use semantic retrieval"
    );
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].0.response_summary, "Learned answer");
}
