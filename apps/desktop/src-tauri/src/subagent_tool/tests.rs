use super::*;
use nexa_core::conversation::{ConversationMessage, CreateConversationInput};
use nexa_core::llm::ProviderType;

fn test_runtime() -> DelegationRuntime {
    DelegationRuntime::new(
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
    )
}

fn observed_batch_run(id: &str) -> SubagentRunArtifact {
    failed_subagent_run_artifact(
        id.to_string(),
        SpawnSubagentArgs {
            task: format!("task-{id}"),
            task_id: None,
            role_id: None,
            role: None,
            model_policy: None,
            context: None,
            expected_output: None,
            max_iterations: None,
            timeout_secs: None,
            acceptance_criteria: None,
            evidence_chunk_ids: None,
            source_ids: None,
            allowed_tools: None,
            parallel_group: None,
            deliverable_style: None,
            return_sections: None,
        },
        None,
        &CoreError::Agent(format!("settled-{id}")),
    )
}

#[tokio::test]
async fn observe_batch_returns_after_one_new_supplemental_result() {
    let runtime = test_runtime();
    runtime.register_batch("batch-1", 3);
    runtime.record_batch_result("batch-1", 0, observed_batch_run("first"));
    let tool = ObserveSubagentBatchTool::from_runtime(runtime.clone());
    let db = Database::open_memory().unwrap();
    let arguments = serde_json::json!({
        "batchId": "batch-1",
        "waitMs": 120_000,
    })
    .to_string();
    let source_scope = Vec::new();
    let observe = tool.execute(nexa_core::tools::ToolExecutionContext::new(
        "observe-1",
        &arguments,
        &db,
        &source_scope,
    ));
    let complete_next = async {
        tokio::task::yield_now().await;
        runtime.record_batch_result("batch-1", 1, observed_batch_run("second"));
    };

    let (result, ()) = tokio::time::timeout(Duration::from_millis(250), async {
        tokio::join!(observe, complete_next)
    })
    .await
    .expect("observation returns after one new result");
    let artifacts = result.unwrap().artifacts.unwrap();

    assert_eq!(artifacts["completedWorkers"], 2);
    assert_eq!(artifacts["pendingWorkers"], 1);
}

#[test]
fn test_normalize_spawn_args_clamps_timeout() {
    let args = normalize_spawn_args(SpawnSubagentArgs {
        task: "Investigate".into(),
        task_id: Some("  worker-1  ".into()),
        role_id: None,
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: Some(999),
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: None,
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    })
    .unwrap();

    assert_eq!(args.timeout_secs, Some(180));
    assert_eq!(args.task_id.as_deref(), Some("worker-1"));
}

#[test]
fn test_delegation_timeout_treats_unlimited_parent_as_default_budget() {
    let mut config = AgentConfig::default();
    config.tool_timeout_secs = Some(0);
    config.agent_timeout_secs = Some(0);

    assert_eq!(resolve_delegation_timeout_secs(&config, None), 120);
}

#[test]
fn test_model_policy_routes_only_to_same_provider_auxiliary_model() {
    let mut config = AgentConfig {
        model: Some("gpt-5".into()),
        summarization_model: Some("gpt-5-mini".into()),
        summarization_provider_type: Some(ProviderType::OpenAi),
        ..AgentConfig::default()
    };
    let openai = ProviderConfig {
        provider_type: ProviderType::OpenAi,
        base_url: None,
        api_key: None,
        org_id: None,
        timeout_secs: None,
        streaming: Default::default(),
    };
    assert!(!apply_delegated_model_policy(
        &mut config,
        &openai,
        Some(&ModelRoutingClass::Fast)
    ));
    assert_eq!(config.model.as_deref(), Some("gpt-5-mini"));

    config.model = Some("claude-opus".into());
    config.summarization_model = Some("gpt-5-mini".into());
    let anthropic = ProviderConfig {
        provider_type: ProviderType::Anthropic,
        base_url: None,
        api_key: None,
        org_id: None,
        timeout_secs: None,
        streaming: Default::default(),
    };
    assert!(apply_delegated_model_policy(
        &mut config,
        &anthropic,
        Some(&ModelRoutingClass::IndependentReviewer)
    ));
    assert_eq!(config.model.as_deref(), Some("claude-opus"));
}

#[tokio::test]
async fn test_budget_uses_realistic_default_token_budget() {
    let budget = SubagentBudgetController::new(&AgentConfig::default());
    let snapshot = budget.snapshot().await;

    assert_eq!(snapshot.token_budget, 32_000);
}

#[test]
fn test_default_subagent_tools_include_read_only_web_research() {
    let tools = default_subagent_tool_names();

    assert!(tools.contains(&"web_search".to_string()));
    assert!(tools.contains(&"web_research_context".to_string()));
    assert!(tools.contains(&"browser_evidence_capture".to_string()));
    assert!(!tools.contains(&"desktop_automation".to_string()));
    assert!(!tools.contains(&"edit_file".to_string()));
    assert!(!tools.contains(&"multi_edit".to_string()));
}

#[test]
fn test_normalize_spawn_args_accepts_structured_role_id() {
    let args = normalize_spawn_args(SpawnSubagentArgs {
        task: "Check the draft".into(),
        task_id: None,
        role_id: Some("Verifier".into()),
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: None,
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: None,
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    })
    .unwrap();

    assert_eq!(args.role_id.as_deref(), Some("verifier"));
    let profile = resolve_role_profile(args.role_id.as_deref(), args.role.as_deref())
        .unwrap()
        .unwrap();
    assert_eq!(profile.label, "Verifier");
    assert_eq!(
        build_return_sections(&args, Some(profile)),
        vec![
            "Verdict".to_string(),
            "Checks performed".to_string(),
            "Unverified or risky claims".to_string()
        ]
    );
}

#[test]
fn test_unknown_role_id_is_rejected() {
    let err = normalize_spawn_args(SpawnSubagentArgs {
        task: "Check the draft".into(),
        task_id: None,
        role_id: Some("wizard".into()),
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: None,
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: None,
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    })
    .unwrap_err();

    assert!(err.to_string().contains("Unknown subagent role_id"));
}

#[test]
fn test_role_profile_narrows_default_tools() {
    let base_tools = vec![
        "search_knowledge_base".to_string(),
        "web_search".to_string(),
        "web_research_context".to_string(),
        "desktop_automation".to_string(),
        "run_shell".to_string(),
        "record_verification".to_string(),
    ];
    let verifier = role_profile_by_id("verifier").unwrap();
    let tools = resolve_allowed_tools_for_role(&base_tools, None, Some(verifier));

    assert!(tools.contains(&"search_knowledge_base".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
    assert!(tools.contains(&"web_research_context".to_string()));
    assert!(tools.contains(&"record_verification".to_string()));
    assert!(!tools.contains(&"desktop_automation".to_string()));
    assert!(!tools.contains(&"run_shell".to_string()));
}

#[test]
fn test_explicit_request_cannot_widen_role_tool_policy() {
    let base_tools = vec!["web_search".to_string(), "desktop_automation".to_string()];
    let verifier = role_profile_by_id("verifier").unwrap();

    let tools = resolve_allowed_tools_for_role(
        &base_tools,
        Some(&["desktop_automation".to_string()]),
        Some(verifier),
    );

    assert!(tools.is_empty());
}

#[test]
fn test_explicit_tool_scope_never_falls_back_to_parent_permissions() {
    let base_tools = vec!["read_file".to_string(), "web_search".to_string()];

    assert!(resolve_allowed_tools(&base_tools, Some(&[])).is_empty());
    assert!(
        resolve_allowed_tools(&base_tools, Some(&["desktop_automation".to_string()])).is_empty()
    );
}

#[test]
fn test_explicit_source_scope_never_falls_back_to_parent_scope() {
    let parent_scope = vec!["source-a".to_string()];

    assert!(resolve_source_scope(&parent_scope, Some(&[])).is_empty());
    assert!(resolve_source_scope(&parent_scope, Some(&["source-b".to_string()])).is_empty());
}

#[test]
fn test_preflight_rejects_tools_outside_parent_capabilities() {
    let args = SpawnSubagentArgs {
        task: "Inspect the repository".into(),
        task_id: None,
        role_id: None,
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: None,
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: Some(vec!["edit_file".into()]),
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    };
    let snapshot = DelegationContextSnapshot {
        id: "snapshot".into(),
        selected_message_ids: Arc::from(Vec::<String>::new()),
        messages: Arc::from(Vec::<Message>::new()),
        token_estimate: 0,
        context_limit: Some(128_000),
        handoff_token_budget: 64_000,
        dropped_invalid_messages: 0,
    };
    let error = validate_subagent_preflight(
        &args,
        "test-model",
        "openai",
        &["read_file".into()],
        &[],
        &[],
        &[],
        &snapshot,
    )
    .unwrap_err();

    let failure = subagent_preflight_failure_from_error(&error).unwrap();
    assert_eq!(failure.schema_version, 1);
    assert_eq!(failure.stage, SubagentPreflightStage::Policy);
    assert_eq!(failure.code, "tool_scope_widening");
    assert!(!failure.retryable);
    assert!(error.to_string().contains("edit_file"));
}

#[test]
fn test_preflight_rejects_interactive_surface_tools_even_when_parent_has_them() {
    let args = SpawnSubagentArgs {
        task: "Click the visible button".into(),
        task_id: None,
        role_id: Some("desktop_operator".into()),
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: None,
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: Some(vec!["computer_control".into(), "browser_session".into()]),
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    };
    let snapshot = DelegationContextSnapshot {
        id: "snapshot".into(),
        selected_message_ids: Arc::from(Vec::<String>::new()),
        messages: Arc::from(Vec::<Message>::new()),
        token_estimate: 0,
        context_limit: Some(128_000),
        handoff_token_budget: 64_000,
        dropped_invalid_messages: 0,
    };
    let error = validate_subagent_preflight(
        &args,
        "test-model",
        "openai",
        &["computer_control".into(), "browser_session".into()],
        &[],
        &[],
        &[],
        &snapshot,
    )
    .unwrap_err();

    let failure = subagent_preflight_failure_from_error(&error).unwrap();
    assert_eq!(failure.stage, SubagentPreflightStage::Policy);
    assert_eq!(failure.code, "interactive_tool_requires_parent_proxy");
    assert!(error.to_string().contains("parent agent"));
}

#[test]
fn test_preflight_classifies_invalid_inherited_history() {
    let args = SpawnSubagentArgs {
        task: "Inspect the repository".into(),
        task_id: None,
        role_id: None,
        role: None,
        model_policy: None,
        context: None,
        expected_output: None,
        max_iterations: None,
        timeout_secs: None,
        acceptance_criteria: None,
        evidence_chunk_ids: None,
        source_ids: None,
        allowed_tools: None,
        parallel_group: None,
        deliverable_style: None,
        return_sections: None,
    };
    let invalid_assistant = Message {
        role: Role::Assistant,
        parts: Vec::new(),
        name: None,
        tool_calls: None,
        reasoning_content: Some("private reasoning".into()),
        prompt_cache_hint: None,
    };
    let snapshot = DelegationContextSnapshot {
        id: "snapshot".into(),
        selected_message_ids: Arc::from(vec!["message-1".to_string()]),
        messages: Arc::from(vec![invalid_assistant]),
        token_estimate: 1,
        context_limit: Some(128_000),
        handoff_token_budget: 64_000,
        dropped_invalid_messages: 0,
    };

    let error =
        validate_subagent_preflight(&args, "test-model", "openai", &[], &[], &[], &[], &snapshot)
            .unwrap_err();
    let failure = subagent_preflight_failure_from_error(&error).unwrap();

    assert_eq!(failure.stage, SubagentPreflightStage::History);
    assert_eq!(failure.code, "inherited_history_invalid");
}

#[test]
fn test_runtime_saves_subagent_session_snapshot() {
    let runtime = test_runtime();
    runtime.save_session_snapshot(SubagentSessionSnapshot {
        task_id: "worker-1".to_string(),
        last_run_id: "run-1".to_string(),
        task: "Investigate".to_string(),
        role_id: Some("researcher".to_string()),
        role_name: Some("Researcher".to_string()),
        result: "Prior result".to_string(),
        finish_reason: Some("stop".to_string()),
        usage_total: Usage::default(),
        tool_event_count: 2,
    });

    let snapshot = runtime
        .get_session_snapshot("worker-1")
        .expect("snapshot should be saved");
    assert_eq!(snapshot.last_run_id, "run-1");
    assert_eq!(snapshot.result, "Prior result");
    assert_eq!(snapshot.tool_event_count, 2);
}

#[test]
fn test_workflow_template_expands_role_based_tasks() {
    let template = workflow_template_by_id("research_verify").unwrap();
    let tasks =
        expand_workflow_template_tasks(template, "Decide whether the proposal is supported", None);

    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].role_id.as_deref(), Some("researcher"));
    assert_eq!(tasks[1].role_id.as_deref(), Some("verifier"));
    assert_eq!(tasks[2].role_id.as_deref(), Some("critic"));
    assert_eq!(tasks[0].parallel_group.as_deref(), Some("research_verify"));
    assert!(tasks[0]
        .task
        .contains("Decide whether the proposal is supported"));
    assert!(tasks[0]
        .return_sections
        .as_ref()
        .is_some_and(|sections| sections.iter().any(|section| section == "Conclusion")));
}

#[test]
fn test_child_runtime_blocks_recursive_delegation() {
    let runtime = test_runtime();
    assert!(runtime.can_delegate_further());

    let child = runtime.spawn_child_runtime(CancellationToken::new());
    assert!(!child.can_delegate_further());
}

#[tokio::test]
async fn test_budget_reservations_are_soft_for_parallel_fanout() {
    let config = AgentConfig {
        subagent_token_budget: Some(256),
        ..Default::default()
    };

    let budget = SubagentBudgetController::new(&config);
    let cancel_token = CancellationToken::new();
    let permit = budget
        .begin_call("worker-a", 220, false, &cancel_token)
        .await
        .unwrap();
    let snapshot = budget.snapshot().await;
    assert_eq!(snapshot.tokens_reserved, 220);
    assert_eq!(snapshot.remaining_tokens, 36);

    let second = budget
        .begin_call("worker-b", 50, false, &cancel_token)
        .await;
    assert!(second.is_ok(), "estimated reservations are a soft budget");
    drop(second);

    drop(permit);
    budget.release_reservation(220).await;
    budget.release_reservation(50).await;
    assert_eq!(budget.snapshot().await.tokens_reserved, 0);
}

#[tokio::test]
async fn test_cancelled_worker_queue_releases_budget_reservation() {
    let config = AgentConfig {
        subagent_max_parallel: Some(1),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let active_cancel = CancellationToken::new();
    let active_permit = budget
        .begin_call("worker-a", 200, false, &active_cancel)
        .await
        .unwrap();

    let queued_budget = budget.clone();
    let queued_cancel = CancellationToken::new();
    let queued_cancel_for_task = queued_cancel.clone();
    let queued = tokio::spawn(async move {
        queued_budget
            .begin_call("worker-b", 300, false, &queued_cancel_for_task)
            .await
    });
    tokio::task::yield_now().await;

    let queued_snapshot = budget.snapshot().await;
    assert_eq!(
        queued_snapshot.calls_started, 1,
        "queued admission must not consume call count before a worker slot exists"
    );
    assert_eq!(
        queued_snapshot.tokens_reserved, 200,
        "queued admission must not reserve output credit before a worker slot exists"
    );
    queued_cancel.cancel();

    assert!(queued.await.unwrap().is_err());
    let snapshot = budget.snapshot().await;
    assert_eq!(snapshot.calls_started, 1);
    assert_eq!(snapshot.tokens_reserved, 200);

    drop(active_permit);
    budget.release_reservation(200).await;
}

#[tokio::test]
async fn test_nexus_preserves_tokens_and_a_call_for_verification() {
    let config = AgentConfig {
        subagent_max_calls_per_turn: Some(3),
        subagent_token_budget: Some(1_000),
        subagent_verification_reserve_percent: Some(25),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel_token = CancellationToken::new();

    let worker = budget
        .begin_call("worker-a", 700, false, &cancel_token)
        .await
        .unwrap();
    assert!(budget
        .begin_call("worker-b", 100, false, &cancel_token)
        .await
        .is_err());
    let verifier = budget
        .begin_call("verifier", 300, true, &cancel_token)
        .await;
    assert!(verifier.is_ok());

    drop(verifier);
    drop(worker);
    budget.release_reservation(700).await;
    budget.release_reservation(300).await;
    let snapshot = budget.snapshot().await;
    assert_eq!(snapshot.verification_reserve_tokens, 0);
    assert_eq!(snapshot.exploration_lane_slots, 1);
    assert_eq!(snapshot.verification_lane_slots, 1);
    assert_eq!(snapshot.judge_lane_slots, 1);
    assert_eq!(snapshot.calls_started, 2);
}

#[tokio::test]
async fn test_nexus_verifier_cannot_consume_the_reserved_judge_call() {
    let config = AgentConfig {
        subagent_max_calls_per_turn: Some(3),
        subagent_verification_reserve_percent: Some(25),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();

    let worker = budget
        .begin_call("worker", 100, false, &cancel)
        .await
        .unwrap();
    let verifier = budget
        .begin_call("verifier", 100, true, &cancel)
        .await
        .unwrap();
    assert!(budget
        .begin_call("second-verifier", 100, true, &cancel)
        .await
        .is_err());
    let judge = budget
        .begin_judge_call("judge", 100, &cancel)
        .await
        .expect("judge keeps its reserved call admission");

    drop((worker, verifier, judge));
    for _ in 0..3 {
        budget.release_reservation(100).await;
    }
    assert_eq!(budget.snapshot().await.calls_started, 3);
}

#[tokio::test]
async fn test_small_custom_call_budget_keeps_exploration_admissible() {
    let config = AgentConfig {
        subagent_max_parallel: Some(3),
        subagent_max_calls_per_turn: Some(2),
        subagent_verification_reserve_percent: Some(25),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();

    let first = budget
        .begin_call("worker-a", 100, false, &cancel)
        .await
        .expect("a small custom call budget must still admit exploration");
    let second = budget
        .begin_call("worker-b", 100, false, &cancel)
        .await
        .expect("all explicitly configured calls remain usable without control lanes");

    drop((first, second));
    assert_eq!(budget.snapshot().await.calls_started, 2);
}

#[tokio::test]
async fn test_worker_queue_has_an_independent_deadline() {
    let config = AgentConfig {
        subagent_max_parallel: Some(1),
        ..Default::default()
    };
    let budget =
        SubagentBudgetController::new_with_queue_deadline(&config, Duration::from_millis(10));
    let cancel = CancellationToken::new();
    let active = budget
        .begin_call("worker-a", 100, false, &cancel)
        .await
        .unwrap();

    let error = budget
        .begin_call("worker-b", 100, false, &cancel)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("queue deadline"));
    assert_eq!(budget.snapshot().await.calls_started, 1);
    drop(active);
    budget.release_reservation(100).await;
}

#[test]
fn delegated_output_helper_honors_explicit_value_and_catalog_ceiling() {
    let config = AgentConfig {
        max_tokens: Some(50_000),
        ..Default::default()
    };

    assert_eq!(resolve_delegated_max_output(&config, None), 50_000);
    assert_eq!(resolve_delegated_max_output(&config, Some(40_000)), 40_000);
}

#[tokio::test]
async fn parent_worker_watchdog_keeps_only_the_hard_run_deadline() {
    let (_fatal_tx, mut fatal_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let error = await_subagent_worker_completion(
        "slow-reasoner",
        &cancel,
        &mut fatal_rx,
        std::future::pending::<Result<(), CoreError>>(),
        10,
    )
    .await
    .expect_err("the parent must retain a hard total deadline");

    assert!(error.to_string().contains("timed out after 10ms"));
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn parent_worker_watchdog_preserves_fatal_error_priority() {
    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel();
    fatal_tx.send("provider failed".to_string()).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = await_subagent_worker_completion(
        "failed-worker",
        &cancel,
        &mut fatal_rx,
        async { Ok::<_, CoreError>(()) },
        1_000,
    )
    .await
    .expect_err("biased fatal errors must win over cancellation and completion");

    assert!(error.to_string().contains("provider failed"));
}

#[tokio::test]
async fn batch_slot_wait_shares_the_global_queue_deadline() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let _occupied = Arc::clone(&slots).acquire_owned().await.unwrap();
    let cancel = CancellationToken::new();

    let error = acquire_batch_slot(slots, &cancel, "queued-worker", Instant::now(), 20)
        .await
        .expect_err("batch-local admission must remain bounded");

    assert!(error.to_string().contains("20ms queue deadline"));
}

#[tokio::test]
async fn batch_queue_failure_rolls_back_unstarted_call_and_token_credit() {
    let config = AgentConfig {
        subagent_max_parallel: Some(1),
        subagent_max_calls_per_turn: Some(2),
        subagent_token_budget: Some(1_000),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();
    let permit = budget
        .begin_call("queued", 100, false, &cancel)
        .await
        .unwrap();

    budget.rollback_unstarted_worker(100, false).await;
    let snapshot = budget.snapshot().await;

    assert_eq!(snapshot.calls_started, 0);
    assert_eq!(snapshot.tokens_reserved, 0);
    drop(permit);
}

#[tokio::test]
async fn judge_startup_failure_rolls_back_global_and_judge_admission() {
    let config = AgentConfig {
        subagent_max_parallel: Some(3),
        subagent_max_calls_per_turn: Some(3),
        subagent_token_budget: Some(10_000),
        subagent_verification_reserve_percent: Some(25),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();
    let failed_judge = budget
        .begin_judge_call("failed-judge", 100, &cancel)
        .await
        .unwrap();

    budget.rollback_unstarted_judge(100).await;
    drop(failed_judge);
    let snapshot = budget.snapshot().await;
    assert_eq!(snapshot.calls_started, 0);
    assert_eq!(snapshot.tokens_reserved, 0);

    drop(
        budget
            .begin_call("explorer", 100, false, &cancel)
            .await
            .unwrap(),
    );
    drop(
        budget
            .begin_call("verifier", 100, true, &cancel)
            .await
            .unwrap(),
    );
    let error = budget
        .begin_call("extra-verifier", 100, true, &cancel)
        .await
        .expect_err("judge call credit must be reserved again after rollback");
    assert!(error.to_string().contains("remain reserved"));
}

#[test]
fn v2_run_deadline_replaces_legacy_role_default_unless_call_is_explicitly_shorter() {
    let config = AgentConfig {
        delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
            run_deadline_ms: Some(240_000),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        resolve_delegation_run_deadline_ms(&config, None, 60, 240_000),
        240_000
    );
    assert_eq!(
        resolve_delegation_run_deadline_ms(&config, Some(30), 60, 240_000),
        30_000
    );
    assert_eq!(
        resolve_delegation_run_deadline_ms(&AgentConfig::default(), None, 60, 180_000),
        60_000
    );
}

#[tokio::test]
async fn unknown_remote_pricing_keeps_cost_limit_advisory_instead_of_blocking_workers() {
    let config = AgentConfig {
        provider_type: Some(ProviderType::OpenAi),
        delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
            total_cost_soft_limit_micros: Some(1_000),
            max_parallel: Some(1),
            max_calls_per_turn: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();

    let permit = budget
        .begin_call("remote-worker", 100, false, &cancel)
        .await
        .expect("unknown pricing must not disable remote delegation");
    let snapshot = budget.snapshot().await;

    assert!(!snapshot.cost_accounting_available);
    assert_eq!(snapshot.cost_soft_limit_micros, Some(1_000));
    drop(permit);
}

#[tokio::test]
async fn token_soft_limit_blocks_new_calls_while_residual_workers_are_running() {
    let config = AgentConfig {
        delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
            total_actual_tokens_soft_limit: Some(256),
            max_parallel: Some(3),
            max_calls_per_turn: Some(4),
            ..Default::default()
        }),
        subagent_verification_reserve_percent: Some(0),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();
    let first = budget
        .begin_call("first", 100, false, &cancel)
        .await
        .unwrap();
    let residual = budget
        .begin_call("residual", 100, false, &cancel)
        .await
        .unwrap();
    budget
        .finish_call(
            100,
            &Usage {
                total_tokens: 300,
                ..Default::default()
            },
            None,
        )
        .await;
    drop(first);

    let error = budget
        .begin_call("new-worker", 100, false, &cancel)
        .await
        .expect_err("actual usage over the soft limit must stop new admission");

    assert!(error.to_string().contains("token soft limit exhausted"));
    drop(residual);
}

#[tokio::test]
async fn nexus_control_lanes_remain_admissible_after_exploration_soft_limit() {
    let config = AgentConfig {
        delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
            total_actual_tokens_soft_limit: Some(256),
            max_parallel: Some(3),
            max_calls_per_turn: Some(4),
            ..Default::default()
        }),
        subagent_verification_reserve_percent: Some(25),
        ..Default::default()
    };
    let budget = SubagentBudgetController::new(&config);
    let cancel = CancellationToken::new();
    let explorer = budget
        .begin_call("explorer", 100, false, &cancel)
        .await
        .unwrap();
    budget
        .finish_call(
            100,
            &Usage {
                total_tokens: 300,
                ..Default::default()
            },
            None,
        )
        .await;
    drop(explorer);

    let verifier = budget
        .begin_call("verifier", 32, true, &cancel)
        .await
        .expect("verification lane survives exploration token exhaustion");
    let judge = budget
        .begin_judge_call("judge", 32, &cancel)
        .await
        .expect("judge lane survives exploration token exhaustion");
    drop(verifier);
    drop(judge);
}

#[test]
fn independent_auto_limits_prefer_model_catalog_over_parent_limits() {
    let mut config = AgentConfig {
        context_window: Some(128_000),
        max_tokens: Some(8_192),
        ..Default::default()
    };

    apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Auto,
        DelegationLimitPolicy::Auto,
        ResolvedContextWindow {
            capacity_tokens: Some(1_000_000),
            authority: ContextWindowAuthority::Catalog,
        },
        Some(65_536),
        true,
    );

    assert_eq!(config.context_window, Some(1_000_000));
    assert_eq!(config.max_tokens, Some(65_536));
}

#[test]
fn nexus_long_reasoning_workers_use_interactive_effort_instead_of_parent_max() {
    let mut qwen = AgentConfig {
        power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
        provider_type: Some(ProviderType::Qwen),
        model: Some("qwen3.8-max".to_string()),
        reasoning_enabled: Some(true),
        reasoning_effort: Some(ReasoningEffort::Max),
        ..Default::default()
    };
    apply_nexus_worker_reasoning_policy(&mut qwen, role_profile_by_id("researcher"));
    assert_eq!(qwen.reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(qwen.thinking_budget, None);

    apply_nexus_worker_reasoning_policy(&mut qwen, role_profile_by_id("verifier"));
    assert_eq!(qwen.reasoning_effort, Some(ReasoningEffort::Medium));

    let mut direct_kimi = AgentConfig {
        power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
        provider_type: Some(ProviderType::Moonshot),
        model: Some("kimi-k3".to_string()),
        reasoning_effort: Some(ReasoningEffort::Max),
        ..Default::default()
    };
    apply_nexus_worker_reasoning_policy(&mut direct_kimi, role_profile_by_id("verifier"));
    assert_eq!(direct_kimi.reasoning_effort, Some(ReasoningEffort::High));

    let mut routed_kimi = AgentConfig {
        power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
        provider_type: Some(ProviderType::AlibabaModelStudio),
        model: Some("kimi/kimi-k3".to_string()),
        reasoning_effort: Some(ReasoningEffort::Max),
        ..Default::default()
    };
    apply_nexus_worker_reasoning_policy(&mut routed_kimi, role_profile_by_id("researcher"));
    assert_eq!(routed_kimi.reasoning_effort, Some(ReasoningEffort::Max));

    let mut qwen_request = CompletionRequest {
        model: "qwen3.8-max".to_string(),
        messages: Vec::new(),
        temperature: None,
        max_tokens: Some(4_000),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_enabled: Some(true),
        reasoning_effort: Some(ReasoningEffort::Medium),
        provider_type: Some(ProviderType::Qwen),
        routing_session_id: None,
        parallel_tool_calls: true,
    };
    apply_judge_recovery_controls(&mut qwen_request);
    assert_eq!(qwen_request.reasoning_enabled, Some(false));

    let mut kimi_request = CompletionRequest {
        model: "kimi-k3".to_string(),
        provider_type: Some(ProviderType::Moonshot),
        ..qwen_request
    };
    apply_judge_recovery_controls(&mut kimi_request);
    assert_eq!(kimi_request.reasoning_enabled, Some(true));
    assert_eq!(kimi_request.reasoning_effort, Some(ReasoningEffort::Low));
}

#[test]
fn nexus_reasoning_policy_is_catalog_driven_across_provider_families() {
    for (provider, model) in [
        (ProviderType::OpenAi, "gpt-5.6"),
        (ProviderType::Anthropic, "claude-fable-5"),
        (ProviderType::Google, "gemini-3.8-flash"),
        (ProviderType::DeepSeek, "deepseek-v4-pro"),
        (ProviderType::Moonshot, "kimi-k3"),
        (ProviderType::Qwen, "qwen3.8-max"),
        (ProviderType::AlibabaModelStudio, "qwen3.8-max"),
        (ProviderType::Zhipu, "glm-5.3"),
        (ProviderType::OpenRouter, "moonshotai/kimi-k3"),
    ] {
        let reasoning = model_capabilities_from_catalog(provider, model)
            .and_then(|capabilities| capabilities.reasoning)
            .unwrap_or_else(|| panic!("missing reasoning profile for {provider:?}:{model}"));
        let mut config = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(provider),
            model: Some(model.to_string()),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Max),
            thinking_budget: Some(262_144),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("researcher"));
        if reasoning.effort_levels.is_empty() {
            assert!(config.thinking_budget.is_some_and(|budget| budget <= 4_096));
        } else if reasoning.effort_levels.iter().any(|level| level == "low") {
            assert_eq!(config.reasoning_effort, Some(ReasoningEffort::Low));
            assert_eq!(config.thinking_budget, None);
        } else {
            assert!(config.reasoning_effort.is_some());
        }
    }
}

#[test]
fn nexus_unknown_model_reasoning_is_bounded_for_every_provider_type() {
    for provider in [
        ProviderType::OpenAi,
        ProviderType::OpenRouter,
        ProviderType::Anthropic,
        ProviderType::Google,
        ProviderType::DeepSeek,
        ProviderType::Ollama,
        ProviderType::LmStudio,
        ProviderType::AzureOpenAi,
        ProviderType::Zhipu,
        ProviderType::Moonshot,
        ProviderType::Qwen,
        ProviderType::AlibabaModelStudio,
        ProviderType::SiliconFlow,
        ProviderType::Doubao,
        ProviderType::Yi,
        ProviderType::Baichuan,
        ProviderType::Custom,
    ] {
        let mut config = AgentConfig {
            power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
            provider_type: Some(provider),
            model: Some("private-unknown-reasoner".to_string()),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..Default::default()
        };
        apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("researcher"));
        assert_eq!(
            config.reasoning_effort,
            Some(ReasoningEffort::Low),
            "unknown model inherited parent max for {provider:?}"
        );
        apply_nexus_worker_reasoning_policy(&mut config, role_profile_by_id("verifier"));
        assert_eq!(config.reasoning_effort, Some(ReasoningEffort::Medium));
    }

    let mut budget_controlled = AgentConfig {
        power_mode: nexa_core::agent::power_mode::AgentPowerMode::Nexus,
        provider_type: Some(ProviderType::Custom),
        model: Some("private-budget-reasoner".to_string()),
        reasoning_enabled: Some(true),
        thinking_budget: Some(262_144),
        ..Default::default()
    };
    apply_nexus_worker_reasoning_policy(&mut budget_controlled, role_profile_by_id("researcher"));
    assert_eq!(budget_controlled.thinking_budget, Some(4_096));
    assert_eq!(budget_controlled.reasoning_effort, None);
}

#[test]
fn independent_auto_output_uses_safe_8k_fallback_without_catalog_data() {
    let mut config = AgentConfig {
        max_tokens: Some(8_192),
        ..Default::default()
    };

    apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Auto,
        DelegationLimitPolicy::Auto,
        ResolvedContextWindow {
            capacity_tokens: None,
            authority: ContextWindowAuthority::ProviderManaged,
        },
        None,
        true,
    );

    assert_eq!(config.max_tokens, Some(DEFAULT_SUBAGENT_MAX_TOKENS));
}

#[test]
fn independent_auto_output_uses_catalog_as_ceiling_not_kimi_allocation() {
    let mut config = AgentConfig {
        context_window: Some(128_000),
        max_tokens: Some(8_192),
        ..Default::default()
    };

    apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Auto,
        DelegationLimitPolicy::Auto,
        ResolvedContextWindow {
            capacity_tokens: Some(1_048_576),
            authority: ContextWindowAuthority::Catalog,
        },
        Some(1_048_576),
        true,
    );

    assert_eq!(config.context_window, Some(1_048_576));
    assert_eq!(config.max_tokens, Some(CONSERVATIVE_SUBAGENT_MAX_TOKENS));
    assert!(config.max_tokens.unwrap() < config.context_window.unwrap());
    assert_eq!(model_context_window("moonshotai/kimi-k3:free"), 1_048_576);
    assert_eq!(model_context_window("qwen3.8-max-latest"), 1_000_000);
}

#[test]
fn delegated_fallback_contract_covers_local_compatible_and_unknown_providers() {
    for (provider, model, expected_context) in [
        (ProviderType::Ollama, "qwen3.8-max", Some(1_000_000)),
        (ProviderType::LmStudio, "openai/gpt-5.6", Some(1_050_000)),
        (
            ProviderType::SiliconFlow,
            "deepseek/deepseek-v4-pro",
            Some(1_000_000),
        ),
        (
            ProviderType::Doubao,
            "doubao-seed-1-6-thinking",
            Some(256_000),
        ),
        (ProviderType::Yi, "yi-large", Some(128_000)),
        (ProviderType::Baichuan, "baichuan-m3", Some(32_000)),
        (ProviderType::Custom, "unknown-private-model", None),
    ] {
        let mut config = AgentConfig {
            provider_type: Some(provider),
            model: Some(model.to_string()),
            context_window: None,
            max_tokens: None,
            ..Default::default()
        };
        apply_delegated_model_limits(
            &mut config,
            DelegationLimitPolicy::Auto,
            DelegationLimitPolicy::Auto,
            resolve_model_context_window(model),
            None,
            true,
        );
        assert_eq!(
            config.context_window, expected_context,
            "fallback context mismatch for {provider:?}:{model}"
        );
        assert_eq!(config.max_tokens, Some(DEFAULT_SUBAGENT_MAX_TOKENS));
    }
}

#[test]
fn explicit_worker_context_is_not_clamped_by_an_inferred_capacity() {
    let mut config = AgentConfig {
        context_window: Some(32_000),
        ..Default::default()
    };
    let authority = apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Explicit(750_000),
        DelegationLimitPolicy::Auto,
        ResolvedContextWindow {
            capacity_tokens: Some(32_000),
            authority: ContextWindowAuthority::ModelProfile,
        },
        None,
        true,
    );

    assert_eq!(config.context_window, Some(750_000));
    assert_eq!(authority, ContextWindowAuthority::UserOverride);
}

#[test]
fn explicit_worker_output_cap_below_legacy_minimum_is_preserved() {
    let mut config = AgentConfig {
        max_tokens: Some(8_192),
        ..Default::default()
    };

    apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Auto,
        DelegationLimitPolicy::Explicit(512),
        ResolvedContextWindow {
            capacity_tokens: None,
            authority: ContextWindowAuthority::ProviderManaged,
        },
        Some(65_536),
        true,
    );

    assert_eq!(config.max_tokens, Some(512));

    apply_delegated_model_limits(
        &mut config,
        DelegationLimitPolicy::Auto,
        DelegationLimitPolicy::Explicit(512),
        ResolvedContextWindow {
            capacity_tokens: None,
            authority: ContextWindowAuthority::ProviderManaged,
        },
        Some(400),
        true,
    );

    assert_eq!(config.max_tokens, Some(400));
}

#[test]
fn test_delegated_failure_status_preserves_deadline_and_error_semantics() {
    for message in [
        "exceeded its 30000ms provider-connect deadline",
        "exceeded its 45000ms first-token deadline",
        "exceeded its 15000ms queue deadline",
        "timed out after 60s",
    ] {
        assert_eq!(delegated_failure_status(message), "timed_out");
    }
    assert_eq!(
        delegated_failure_status("was cancelled by the parent turn"),
        "cancelled"
    );
    assert_eq!(
        delegated_failure_status("authentication failed with status 401"),
        "failed"
    );
}

#[tokio::test]
async fn test_delegation_runtime_uses_distinct_connection_and_first_token_deadlines() {
    let limits = SubagentBudgetController::new(&AgentConfig::default())
        .limits()
        .await;

    assert!(limits.connect_deadline_ms > 0);
    assert!(limits.first_token_deadline_ms > limits.connect_deadline_ms);

    let ordinary = SubagentBudgetController::new(&AgentConfig {
        model: Some("ordinary-model".to_string()),
        provider_type: Some(ProviderType::OpenAi),
        ..Default::default()
    })
    .limits()
    .await;
    assert_eq!(ordinary.connect_deadline_ms, 15_000);
    assert_eq!(ordinary.first_token_deadline_ms, 45_000);
    assert_eq!(ordinary.run_deadline_ms, 180_000);

    let qwen = SubagentBudgetController::new(&AgentConfig {
        model: Some("qwen3.8-max".to_string()),
        provider_type: Some(ProviderType::Qwen),
        ..Default::default()
    })
    .limits()
    .await;
    assert_eq!(qwen.connect_deadline_ms, 90_000);
    assert_eq!(qwen.first_token_deadline_ms, 150_000);
    assert_eq!(qwen.run_deadline_ms, 360_000);

    for (provider, model) in [
        (ProviderType::OpenAi, "gpt-5.6"),
        (ProviderType::Anthropic, "claude-fable-5"),
        (ProviderType::Google, "gemini-3.8-flash"),
        (ProviderType::DeepSeek, "deepseek-v4-pro"),
        (ProviderType::Zhipu, "glm-5.3"),
    ] {
        let profiled = SubagentBudgetController::new(&AgentConfig {
            model: Some(model.to_string()),
            provider_type: Some(provider),
            ..Default::default()
        })
        .limits()
        .await;
        assert_eq!(
            profiled.connect_deadline_ms, 90_000,
            "catalog long-prefill profile missing for {provider:?}:{model}"
        );
        assert_eq!(profiled.first_token_deadline_ms, 150_000);
    }
}

#[tokio::test]
async fn delegation_limits_v2_overrides_legacy_dimensions_and_deadlines() {
    let config = AgentConfig {
        provider_type: Some(ProviderType::Ollama),
        subagent_max_parallel: Some(2),
        subagent_token_budget: Some(12_000),
        delegation_limits_v2: Some(nexa_core::agent::DelegationLimitsConfig {
            input_context_limit: Some(1_000_000),
            handoff_context_tokens_per_worker: Some(40_000),
            max_output_tokens_per_step: None,
            max_output_tokens_per_worker: Some(65_536),
            max_actual_tokens_per_worker: Some(96_000),
            total_actual_tokens_soft_limit: Some(240_000),
            total_cost_soft_limit_micros: Some(1_000),
            max_parallel: Some(6),
            max_calls_per_turn: Some(12),
            queue_deadline_ms: Some(5_000),
            connect_deadline_ms: Some(20_000),
            first_token_deadline_ms: Some(60_000),
            run_deadline_ms: Some(240_000),
        }),
        ..Default::default()
    };

    let limits = SubagentBudgetController::new(&config).limits().await;

    assert_eq!(limits.max_parallel, 6);
    assert_eq!(limits.max_calls_per_turn, 12);
    assert_eq!(
        limits.input_context_policy,
        DelegationLimitPolicy::Explicit(1_000_000)
    );
    assert_eq!(
        limits.max_output_tokens_per_worker,
        DelegationLimitPolicy::Explicit(65_536)
    );
    assert_eq!(limits.total_actual_tokens_soft_limit, Some(240_000));
    assert_eq!(limits.total_cost_soft_limit_micros, Some(1_000));
    assert!(limits.cost_accounting_available);
    assert_eq!(limits.queue_deadline_ms, 5_000);
    assert_eq!(limits.connect_deadline_ms, 20_000);
    assert_eq!(limits.first_token_deadline_ms, 60_000);
    assert_eq!(limits.run_deadline_ms, 240_000);
}

#[test]
fn test_context_snapshot_reuses_authorized_parent_history() {
    let db = Database::open_memory().unwrap();
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "google".to_string(),
            model: "gemini-2.5-pro".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    db.add_message(&ConversationMessage {
        id: "parent-message".to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Parent context that the delegated worker needs".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 10,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    })
    .unwrap();

    let first = load_delegation_context_snapshot(
        &db,
        Some(&conversation.id),
        "gemini-2.5-pro",
        Some(1_048_576),
        64_000,
    );
    let second = load_delegation_context_snapshot(
        &db,
        Some(&conversation.id),
        "gemini-2.5-pro",
        Some(1_048_576),
        64_000,
    );

    assert_eq!(first.id, second.id);
    assert_eq!(first.selected_message_ids.as_ref(), &["parent-message"]);
    assert_eq!(
        first.messages[0].text_content(),
        "Parent context that the delegated worker needs"
    );
    assert_eq!(first.context_limit, Some(1_048_576));
    assert_eq!(first.handoff_token_budget, 64_000);
}

#[test]
fn oversized_parent_message_cannot_overrun_worker_handoff_budget() {
    let db = Database::open_memory().unwrap();
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "qwen".to_string(),
            model: "qwen3.8-max".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    db.add_message(&ConversationMessage {
        id: "oversized-parent".to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "large parent context ".repeat(20_000),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 100_000,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    })
    .unwrap();

    let snapshot = load_delegation_context_snapshot(
        &db,
        Some(&conversation.id),
        "qwen3.8-max",
        Some(1_000_000),
        10_000,
    );
    assert!(snapshot.token_estimate <= 10_000);
    assert!(snapshot.messages.is_empty());
    assert_eq!(snapshot.dropped_invalid_messages, 1);
    assert_eq!(snapshot.context_limit, Some(1_000_000));
}

#[test]
fn test_batch_completion_policy_resolves_quorum_and_deadline() {
    let quorum_args = SpawnSubagentBatchArgs {
        tasks: Vec::new(),
        batch_goal: None,
        workflow_template: None,
        parallel_group: None,
        max_parallel: None,
        completion_policy: Some("quorum".to_string()),
        quorum: Some(3),
        deadline_ms: None,
        cancel_remaining: None,
    };
    assert_eq!(
        DelegationCompletionPolicy::resolve(&quorum_args, 4).unwrap(),
        DelegationCompletionPolicy::Quorum { required: 3 }
    );

    let deadline_args = SpawnSubagentBatchArgs {
        completion_policy: Some("deadline".to_string()),
        deadline_ms: Some(2_500),
        ..quorum_args
    };
    assert_eq!(
        DelegationCompletionPolicy::resolve(&deadline_args, 4).unwrap(),
        DelegationCompletionPolicy::Deadline { deadline_ms: 2_500 }
    );

    let parent_args = SpawnSubagentBatchArgs {
        completion_policy: Some("parent_decides".to_string()),
        ..deadline_args
    };
    let parent_policy = DelegationCompletionPolicy::resolve(&parent_args, 4).unwrap();
    assert_eq!(parent_policy, DelegationCompletionPolicy::ParentDecides);
    assert!(!parent_policy.is_satisfied(&[], 4));
    assert!(!parent_policy.is_satisfied(&[], 1));
    assert!(parent_policy.is_satisfied(&[observed_batch_run("decision")], 3));
    assert!(parent_policy.is_satisfied(&[], 0));

    let schema = spawn_subagent_batch_parameters_schema();
    assert_eq!(schema["properties"]["cancel_remaining"]["type"], "boolean");
}
