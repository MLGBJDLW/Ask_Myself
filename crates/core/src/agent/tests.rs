use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tokio::sync::Notify;

use super::*;
use crate::approval::{ApprovalDecision, ToolApprovalMode};
use crate::conversation::{conversation_message_llm_context_content, CreateConversationInput};
use crate::llm::{CompletionResponse, FinishReason, StreamChunk};
use crate::tools::{Tool, ToolResult};

#[test]
fn test_tool_timeout_zero_disables_outer_timeout() {
    let timeout = tool_timeout_for_call(Some(0), "read_file", &serde_json::json!({}));
    assert_eq!(timeout, None);
}

#[test]
fn test_tool_timeout_honors_finite_run_shell_no_timeout() {
    let timeout = tool_timeout_for_call(
        Some(30),
        "run_shell",
        &serde_json::json!({ "program": "python", "args": ["-"], "stdin": "print('ok')", "timeout_secs": 0 }),
    );
    assert_eq!(timeout, None);
}

#[test]
fn test_tool_timeout_keeps_auto_detached_process_bounded() {
    let timeout = tool_timeout_for_call(
        Some(30),
        "run_shell",
        &serde_json::json!({
            "command": "python -m http.server 8080",
            "timeout_secs": 0,
            "background": true,
            "ready_timeout_secs": 75,
        }),
    );
    assert_eq!(timeout, Some(Duration::from_secs(30)));
}

#[test]
fn test_tool_timeout_extends_for_long_run_shell_timeout() {
    let timeout = tool_timeout_for_call(
        Some(30),
        "run_shell",
        &serde_json::json!({ "program": "python", "args": ["-"], "stdin": "print('ok')", "timeout_secs": 600 }),
    );
    assert_eq!(timeout, Some(Duration::from_secs(605)));
}

#[test]
fn test_tool_timeout_leaves_room_for_run_shell_default_timeout() {
    let timeout = tool_timeout_for_call(Some(30), "run_shell", &serde_json::json!({}));
    assert_eq!(timeout, Some(Duration::from_secs(35)));
}

#[test]
fn test_tool_timeout_preserves_existing_multipliers_and_subagent_minimums() {
    assert_eq!(
        tool_timeout_for_call(Some(30), "retrieve_evidence", &serde_json::json!({})),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        tool_timeout_for_call(Some(30), "spawn_subagent", &serde_json::json!({})),
        Some(Duration::from_secs(180))
    );
    assert_eq!(
        tool_timeout_for_call(Some(30), "spawn_subagent_batch", &serde_json::json!({})),
        Some(Duration::from_secs(240))
    );
    assert_eq!(
        tool_timeout_for_call(Some(300), "spawn_subagent", &serde_json::json!({})),
        Some(Duration::from_secs(300))
    );
}

#[test]
fn test_accumulate_new_tool_call() {
    let mut calls = Vec::new();
    let delta = ToolCallDelta {
        id: "call_1".into(),
        name: Some("search".into()),
        arguments_delta: r#"{"qu"#.into(),
        index: None,
        thought_signature: None,
    };
    accumulate_tool_call(&mut calls, &delta);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].name, "search");
    assert_eq!(calls[0].arguments, r#"{"qu"#);
}

#[test]
fn test_accumulate_appends_arguments() {
    let mut calls = vec![ToolCallRequest {
        id: "call_1".into(),
        name: "search".into(),
        arguments: r#"{"qu"#.into(),
        thought_signature: None,
    }];
    let delta = ToolCallDelta {
        id: "call_1".into(),
        name: None,
        arguments_delta: r#"ery":"test"}"#.into(),
        index: None,
        thought_signature: None,
    };
    accumulate_tool_call(&mut calls, &delta);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, r#"{"query":"test"}"#);
}

#[test]
fn test_accumulate_treats_a_complete_nested_object_as_an_opaque_delta() {
    let mut calls = vec![ToolCallRequest {
        id: "call_nested".into(),
        name: "create_file".into(),
        arguments: r#"{"metadata":"#.into(),
        thought_signature: None,
    }];

    for fragment in [r#"{"language":"rust"}"#, "}"] {
        assert!(accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: "call_nested".into(),
                name: None,
                arguments_delta: fragment.into(),
                index: None,
                thought_signature: None,
            },
        ));
    }

    assert_eq!(calls[0].arguments, r#"{"metadata":{"language":"rust"}}"#);
}

#[test]
fn test_accumulate_replaces_cumulative_provider_argument_snapshots() {
    let mut calls = Vec::new();
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call_snapshot".into(),
            name: Some("create_file".into()),
            arguments_delta: r#"{"path":"notes/a"#.into(),
            index: Some(0),
            thought_signature: None,
        },
    ));
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call_snapshot".into(),
            name: Some("create_file".into()),
            arguments_delta: r#"{"path":"notes/a.md","content":"ok"}"#.into(),
            index: Some(0),
            thought_signature: None,
        },
    ));

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].arguments,
        r#"{"path":"notes/a.md","content":"ok"}"#,
    );
    assert!(crate::llm::message_validation::is_complete_tool_call(
        &calls[0]
    ));
}

#[test]
fn test_accumulate_replaces_repeated_json_object_argument_snapshots() {
    let mut calls = Vec::new();
    for arguments in [r#"{"path":"a"}"#, r#"{"path":"b"}"#] {
        assert!(accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: "call-object-snapshot".into(),
                name: Some("create_file".into()),
                arguments_delta: crate::llm::ToolCallArgumentsDelta::snapshot(
                    arguments.to_string(),
                ),
                index: Some(0),
                thought_signature: None,
            },
        ));
    }

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, r#"{"path":"b"}"#);
    assert!(crate::llm::message_validation::is_complete_tool_call(
        &calls[0]
    ));
}

#[test]
fn test_cumulative_snapshot_limit_applies_to_replacement_not_snapshot_sum() {
    let final_arguments = format!(r#"{{"content":"{}"}}"#, "x".repeat(700_000));
    let partial_arguments = final_arguments[..600_000].to_string();
    let mut calls = Vec::new();

    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call-large-snapshot".into(),
            name: Some("create_file".into()),
            arguments_delta: partial_arguments.into(),
            index: Some(0),
            thought_signature: None,
        },
    ));
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call-large-snapshot".into(),
            name: Some("create_file".into()),
            arguments_delta: final_arguments.clone().into(),
            index: Some(0),
            thought_signature: None,
        },
    ));

    assert_eq!(calls[0].arguments, final_arguments);
    assert!(crate::llm::message_validation::is_complete_tool_call(
        &calls[0]
    ));
}

#[test]
fn test_accumulate_empty_id_appends_to_last() {
    let mut calls = vec![ToolCallRequest {
        id: "call_1".into(),
        name: "search".into(),
        arguments: r#"{"q"#.into(),
        thought_signature: None,
    }];
    let delta = ToolCallDelta {
        id: String::new(),
        name: None,
        arguments_delta: r#"":"v"}"#.into(),
        index: None,
        thought_signature: None,
    };
    accumulate_tool_call(&mut calls, &delta);
    assert_eq!(calls[0].arguments, r#"{"q":"v"}"#);
}

#[test]
fn test_accumulate_multiple_tool_calls() {
    let mut calls = Vec::new();
    accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call_1".into(),
            name: Some("search".into()),
            arguments_delta: "{}".into(),
            index: None,
            thought_signature: None,
        },
    );
    accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call_2".into(),
            name: Some("file".into()),
            arguments_delta: "{}".into(),
            index: None,
            thought_signature: None,
        },
    );
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "search");
    assert_eq!(calls[1].name, "file");
}

#[test]
fn test_accumulate_by_index_when_id_missing() {
    let mut calls = vec![
        ToolCallRequest {
            id: "call_0".into(),
            name: "search".into(),
            arguments: r#"{"q":"hel"#.into(),
            thought_signature: None,
        },
        ToolCallRequest {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"C"#.into(),
            thought_signature: None,
        },
    ];

    accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: String::new(),
            name: None,
            arguments_delta: r#"lo"}"#.into(),
            index: Some(0),
            thought_signature: None,
        },
    );
    accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: String::new(),
            name: None,
            arguments_delta: r#":\a.md"}"#.into(),
            index: Some(1),
            thought_signature: None,
        },
    );

    assert_eq!(calls[0].arguments, r#"{"q":"hello"}"#);
    assert_eq!(calls[1].arguments, r#"{"path":"C:\a.md"}"#);
}

#[test]
fn test_accumulate_late_real_id_updates_the_existing_index_slot() {
    let mut calls = Vec::new();
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: String::new(),
            name: Some("search".into()),
            arguments_delta: r#"{"query":"rus"#.into(),
            index: Some(0),
            thought_signature: None,
        },
    ));
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "provider-call-1".into(),
            name: None,
            arguments_delta: r#"t"}"#.into(),
            index: Some(0),
            thought_signature: None,
        },
    ));

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "provider-call-1");
    assert_eq!(calls[0].name, "search");
    assert_eq!(calls[0].arguments, r#"{"query":"rust"}"#);
    assert!(crate::llm::message_validation::is_complete_tool_call(
        &calls[0]
    ));
}

#[test]
fn test_accumulate_valid_id_accepts_sparse_provider_output_index() {
    let mut calls = Vec::new();
    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "provider-call-after-reasoning".into(),
            name: Some("read_file".into()),
            arguments_delta: r#"{"path":"README.md"}"#.into(),
            // Responses indexes the full output array. Slot zero may be a
            // reasoning item rather than a client function call.
            index: Some(1),
            thought_signature: None,
        },
    ));

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "provider-call-after-reasoning");
    assert!(crate::llm::message_validation::is_complete_tool_call(
        &calls[0]
    ));
}

#[test]
fn test_accumulate_valid_id_does_not_overwrite_dense_slot_on_sparse_index_collision() {
    let mut calls = vec![
        ToolCallRequest {
            id: "call-a".into(),
            name: "search".into(),
            arguments: "{}".into(),
            thought_signature: None,
        },
        ToolCallRequest {
            id: "call-b".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"b.md"}"#.into(),
            thought_signature: None,
        },
    ];

    assert!(accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: "call-c".into(),
            name: Some("write_file".into()),
            arguments_delta: r#"{"path":"c.md"}"#.into(),
            index: Some(1),
            thought_signature: None,
        },
    ));

    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].id, "call-b");
    assert_eq!(calls[2].id, "call-c");
}

#[test]
fn test_accumulate_rejects_unaddressed_parallel_fragment() {
    let mut calls = vec![
        ToolCallRequest {
            id: "call-1".into(),
            name: "search".into(),
            arguments: r#"{"query":"a"}"#.into(),
            thought_signature: None,
        },
        ToolCallRequest {
            id: "call-2".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"b"}"#.into(),
            thought_signature: None,
        },
    ];

    assert!(!accumulate_tool_call(
        &mut calls,
        &ToolCallDelta {
            id: String::new(),
            name: None,
            arguments_delta: "corrupt".into(),
            index: None,
            thought_signature: None,
        },
    ));
    assert_eq!(calls[0].arguments, r#"{"query":"a"}"#);
    assert_eq!(calls[1].arguments, r#"{"path":"b"}"#);
}

#[test]
fn test_default_config() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.max_iterations, u32::MAX);
    assert!(cfg.system_prompt.contains("local-first workspace agent"));
    assert_eq!(cfg.temperature, Some(0.3));
    assert_eq!(cfg.max_tokens, None);
}

#[test]
fn model_step_output_reserve_is_provider_aware_and_respects_explicit_choice() {
    let standard = AgentConfig::default();
    let standard_plan = standard.resolved_output_budget("unknown-model");
    assert_eq!(
        standard_plan.effective_tokens,
        FALLBACK_AGENT_RESPONSE_TOKENS
    );
    assert_eq!(
        standard_plan.authority,
        OutputBudgetAuthority::AutomaticFallbackReserve
    );

    let unknown_deepseek = AgentConfig {
        provider_type: Some(ProviderType::DeepSeek),
        ..AgentConfig::default()
    };
    assert_eq!(
        unknown_deepseek.resolved_max_response_tokens("deepseek-unknown"),
        FALLBACK_DEEPSEEK_RESPONSE_TOKENS
    );

    let deepseek = AgentConfig {
        provider_type: Some(ProviderType::DeepSeek),
        ..AgentConfig::default()
    };
    let catalog_driven = deepseek.resolved_output_budget("deepseek-v4-pro");
    assert_eq!(
        catalog_driven.authority,
        OutputBudgetAuthority::VerifiedCatalogCapability
    );
    assert_eq!(
        catalog_driven.effective_tokens,
        catalog_driven.catalog_cap.unwrap()
    );
    assert!(catalog_driven.effective_tokens > FALLBACK_DEEPSEEK_RESPONSE_TOKENS);

    for (provider_type, model) in [
        (ProviderType::OpenAi, "gpt-5.6"),
        (ProviderType::Anthropic, "claude-fable-5"),
        (ProviderType::Google, "gemini-3.7-flash"),
        (ProviderType::DeepSeek, "deepseek-v4-pro"),
    ] {
        let plan = AgentConfig {
            provider_type: Some(provider_type),
            ..AgentConfig::default()
        }
        .resolved_output_budget(model);
        assert_eq!(
            plan.authority,
            OutputBudgetAuthority::VerifiedCatalogCapability,
            "{provider_type:?}/{model} should use catalog output authority"
        );
        assert_eq!(plan.effective_tokens, plan.catalog_cap.unwrap());
    }

    let private_openai_compatible = AgentConfig {
        provider_type: Some(ProviderType::OpenAi),
        context_window_resolution: Some(crate::conversation::memory::ResolvedContextWindow {
            capacity_tokens: None,
            authority: crate::conversation::memory::ContextWindowAuthority::ProviderManaged,
        }),
        ..AgentConfig::default()
    };
    let private_plan = private_openai_compatible.resolved_output_budget("gpt-5.6");
    assert_eq!(
        private_plan.authority,
        OutputBudgetAuthority::AutomaticFallbackReserve,
        "a private endpoint must not inherit output authority from a public model alias"
    );
    assert_eq!(private_plan.catalog_cap, None);
    assert_eq!(private_plan.context_cap, None);
    assert_eq!(
        private_plan.effective_tokens,
        FALLBACK_AGENT_RESPONSE_TOKENS
    );

    let explicit = AgentConfig {
        max_tokens: Some(12_000),
        context_window: Some(16_000),
        ..deepseek.clone()
    };
    let explicit_plan = explicit.resolved_output_budget("deepseek-v4-pro");
    assert_eq!(
        explicit_plan.effective_tokens, 12_000,
        "explicit output caps are not reduced to half the context window"
    );
    assert_eq!(explicit_plan.requested_tokens, 12_000);
    assert_eq!(
        explicit_plan.authority,
        OutputBudgetAuthority::SavedExplicitOverride
    );

    for (provider_type, model) in [
        (
            Some(ProviderType::OpenRouter),
            "deepseek/deepseek-v4-pro:free",
        ),
        (Some(ProviderType::OpenAi), "deepseek-ai/deepseek-r1"),
    ] {
        let routed = AgentConfig {
            provider_type,
            ..AgentConfig::default()
        };
        let plan = routed.resolved_output_budget(model);
        assert!(plan.effective_tokens >= FALLBACK_DEEPSEEK_RESPONSE_TOKENS);
        assert!(matches!(
            plan.authority,
            OutputBudgetAuthority::VerifiedCatalogCapability
                | OutputBudgetAuthority::AutomaticFallbackReserve
        ));
    }

    let constrained = AgentConfig {
        context_window: Some(8_192),
        ..deepseek
    };
    assert_eq!(
        constrained.resolved_max_response_tokens("deepseek-chat"),
        4_096
    );
    assert_eq!(
        constrained
            .resolved_output_budget("deepseek-chat")
            .context_cap,
        Some(4_096)
    );
    assert_eq!(
        explicit_plan.recommended_text_tool_chunk_chars(4_097),
        8_194
    );
}

#[test]
fn test_build_system_prompt_preserves_core_rules() {
    let prompt = build_system_prompt(
        Some("Prefer terse answers."),
        &["## User Preferences\n\n- Prefer PDFs first"],
    );

    let core_idx = prompt
        .find("You are **Nexa**")
        .expect("core prompt should be present");
    let custom_idx = prompt
        .find("## Conversation-Specific Instructions")
        .expect("custom section should be present");
    let dynamic_idx = prompt
        .find("## User Preferences")
        .expect("dynamic section should be present");

    assert_eq!(core_idx, 0, "core prompt should stay first");
    assert!(
        custom_idx > core_idx,
        "custom instructions should be appended"
    );
    assert!(
        dynamic_idx > custom_idx,
        "dynamic sections should follow custom text"
    );
    assert!(prompt.contains("Prefer terse answers."));
    assert!(prompt.contains("Own the requested outcome"));
    assert!(prompt.contains("Protect user work"));
    assert!(prompt.contains("A tool call is not evidence of success"));
    assert!(prompt.contains("closed observe-fix-verify loop"));
    assert!(prompt.contains("browser_evidence_capture"));
    assert!(prompt.contains("keep recent complete turns verbatim"));
    assert!(!prompt.contains("## Tool Contract: run_shell"));
    assert!(prompt.len() < 8_000);
}

#[test]
fn test_build_system_prompt_skips_blank_sections() {
    let prompt = build_system_prompt(Some("   "), &["", "  ", "\n\n"]);
    assert_eq!(prompt, default_system_prompt());
}

#[test]
fn test_route_pack_injects_run_shell_contract_only_for_codebase_work() {
    let code_route = route_user_turn(
        "为什么主agent没有办法调用run_shell？请仔细排查并全面修复。",
        "",
        false,
    );

    assert_eq!(code_route.kind, AgentRouteKind::CodebaseOperation);
    assert!(code_route
        .prompt_section
        .contains("## Route Pack: Codebase"));
    assert!(code_route
        .prompt_section
        .contains("## Tool Contract: run_shell"));

    let direct_route = route_user_turn("Say hello in one sentence.", "", false);
    assert_eq!(direct_route.kind, AgentRouteKind::DirectResponse);
    assert!(!direct_route
        .prompt_section
        .contains("## Tool Contract: run_shell"));
}

#[test]
fn test_route_user_turn_prefers_collection_context() {
    let route = route_user_turn(
            "Explain what this saved citation means",
            "## Collection Context\nTitle: Retry Collection\n\nUse this collection and its saved evidence as your primary working set.",
            true,
        );

    assert_eq!(route.kind, AgentRouteKind::CollectionFocused);
    assert!(route
        .visibility_decision
        .route_categories
        .contains(&ToolCategory::Knowledge));
}

#[test]
fn test_route_user_turn_ignores_persona_saved_evidence_phrase() {
    let route = route_user_turn(
        "Say hello in one sentence.",
        "## Active Persona\nInstructions: Prefer saved evidence when it exists.",
        false,
    );

    assert_eq!(route.kind, AgentRouteKind::DirectResponse);
}

#[test]
fn test_route_user_turn_prefers_knowledge_retrieval_for_question_with_sources() {
    let route = route_user_turn("Why did the retry guard fail?", "", true);

    assert_eq!(route.kind, AgentRouteKind::KnowledgeRetrieval);
    assert!(route
        .visibility_decision
        .route_categories
        .contains(&ToolCategory::DocumentAnalysis));
}

#[test]
fn test_route_user_turn_treats_office_generation_as_file_operation() {
    let route = route_user_turn("请创建一份 Word 商业计划书", "", false);

    assert_eq!(route.kind, AgentRouteKind::FileOperation);
    assert!(route
        .visibility_decision
        .route_categories
        .contains(&ToolCategory::FileSystem));
}

#[test]
fn test_route_user_turn_treats_tool_repair_as_file_operation() {
    let route = route_user_turn(
        "为什么主agent没有办法调用run_shell？请仔细排查并全面修复。",
        "",
        false,
    );

    assert_eq!(route.kind, AgentRouteKind::CodebaseOperation);
    assert!(route
        .visibility_decision
        .route_categories
        .contains(&ToolCategory::FileSystem));
    assert!(route.prompt_section.contains("code_intelligence"));
    assert!(route.prompt_section.contains("grep_files/search_files"));
    assert!(route.prompt_section.contains("before reading"));
    assert!(route
        .prompt_section
        .contains("read_file/read_files as follow-up"));
    assert!(route.prompt_section.contains("project_tool"));
}

#[test]
fn test_agent_config_defaults_to_cache_stable_tool_visibility() {
    assert!(!AgentConfig::default().dynamic_tool_visibility);
}

struct MockProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "call_1".to_string(),
                    name: Some("mock_tool".to_string()),
                    arguments_delta: r#"{"value":"ok"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                // Some providers return `stop` even when tool calls are present.
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        } else {
            vec![Ok(StreamChunk {
                delta: "final answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct ThinkingMockProvider {
    stream_calls: Arc<AtomicUsize>,
}

struct MissingRequiredReasoningProvider {
    complete_calls: Arc<AtomicUsize>,
}

struct RouteAwareReplayPolicyProvider {
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
}

struct RecoverablePrimaryRouteProvider {
    stream_calls: Arc<AtomicUsize>,
}

struct ToolCallingFallbackRouteProvider {
    stream_calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

fn recoverable_primary_route(model: &str) -> crate::llm::provider_turn::RouteSnapshot {
    crate::llm::provider_turn::RouteSnapshot {
        provider_endpoint_id: "primary-openai-endpoint".to_string(),
        provider_family: "openai-compatible".to_string(),
        api_style: crate::llm::reasoning_profile::ReasoningApiStyle::OpenAiChatCompletions,
        model_id: model.to_string(),
        reasoning_profile_id: "primary-not-required-v1".to_string(),
        reasoning_profile_version: 1,
        replay_policy: ReasoningReplayPolicy::NotRequired,
    }
}

fn tool_calling_fallback_route(model: &str) -> crate::llm::provider_turn::RouteSnapshot {
    crate::llm::provider_turn::RouteSnapshot {
        provider_endpoint_id: "fallback-deepseek-endpoint".to_string(),
        provider_family: "deepseek".to_string(),
        api_style: crate::llm::reasoning_profile::ReasoningApiStyle::OpenAiChatCompletions,
        model_id: model.to_string(),
        reasoning_profile_id: "fallback-required-v1".to_string(),
        reasoning_profile_version: 1,
        replay_policy: ReasoningReplayPolicy::RequiredOnToolCall,
    }
}

struct UnknownReplayThinkingProvider {
    attempt_tool_call: bool,
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
    request_reasoning: Arc<Mutex<Vec<Option<bool>>>>,
    rejected_reasoning_seen_in_history: Arc<Mutex<Vec<bool>>>,
}

impl UnknownReplayThinkingProvider {
    fn observe_request(&self, request: &CompletionRequest) {
        self.request_reasoning
            .lock()
            .unwrap()
            .push(request.reasoning_enabled);
        self.rejected_reasoning_seen_in_history
            .lock()
            .unwrap()
            .push(request.messages.iter().any(|message| {
                message.reasoning_content.as_deref()
                    == Some("visible reasoning from an unverified route")
                    || message
                        .text_content()
                        .contains("visible reasoning from an unverified route")
            }));
    }
}

#[async_trait]
impl LlmProvider for UnknownReplayThinkingProvider {
    fn name(&self) -> &str {
        "unknown-replay-thinking-mock"
    }

    fn route_snapshot(
        &self,
        request: &CompletionRequest,
    ) -> crate::llm::provider_turn::RouteSnapshot {
        crate::llm::provider_turn::RouteSnapshot {
            provider_endpoint_id: "custom-compatible-endpoint".to_string(),
            provider_family: "openai-compatible".to_string(),
            api_style: crate::llm::reasoning_profile::ReasoningApiStyle::OpenAiChatCompletions,
            model_id: request.model.clone(),
            reasoning_profile_id: "custom-compatible-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy: if request.reasoning_enabled == Some(false) {
                ReasoningReplayPolicy::NotRequired
            } else {
                ReasoningReplayPolicy::Unknown
            },
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["custom-reasoner".to_string()])
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        self.observe_request(request);
        Ok(CompletionResponse {
            content: String::new(),
            tool_calls: Some(vec![ToolCallRequest {
                id: "call-safe-restart".to_string(),
                name: "recording_tool".to_string(),
                arguments: r#"{"value":"safe-restart"}"#.to_string(),
                thought_signature: None,
            }]),
            finish_reason: FinishReason::ToolCalls,
            usage: Usage::default(),
            thinking: None,
            provider_replay: None,
        })
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.observe_request(request);
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if !self.attempt_tool_call {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some("visible reasoning from an unverified route".to_string()),
                }),
                Ok(StreamChunk {
                    delta: "final answer with visible reasoning".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                }),
            ]
        } else if call_no == 0 {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some("visible reasoning from an unverified route".to_string()),
                }),
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: "call-unverified".to_string(),
                        name: Some("recording_tool".to_string()),
                        arguments_delta: r#"{"value":"must-not-run"}"#.to_string().into(),
                        index: Some(0),
                        thought_signature: None,
                    }),
                    finish_reason: Some(FinishReason::ToolCalls),
                    usage: None,
                    thinking_delta: None,
                }),
            ]
        } else {
            vec![Ok(StreamChunk {
                delta: "final answer after verified restart".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for RouteAwareReplayPolicyProvider {
    fn name(&self) -> &str {
        "route-aware-replay-policy-mock"
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::NotRequired
    }

    fn reasoning_replay_history_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::RequiredOnToolCall
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["primary-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Err(CoreError::Llm(
            "permissive output must not enter reasoning recovery".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "call-primary".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"safe"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            })]
        } else {
            vec![Ok(StreamChunk {
                delta: "final answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for RecoverablePrimaryRouteProvider {
    fn name(&self) -> &str {
        "recoverable-primary-route"
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::NotRequired
    }

    fn route_snapshot(
        &self,
        request: &CompletionRequest,
    ) -> crate::llm::provider_turn::RouteSnapshot {
        recoverable_primary_route(&request.model)
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["primary-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "the recoverable primary must not enter completion mode".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter(vec![
            ProviderStreamEvent::RecoverableError {
                message: "primary disconnected before visible output".to_string(),
            },
        ])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ToolCallingFallbackRouteProvider {
    fn name(&self) -> &str {
        "tool-calling-fallback-route"
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::RequiredOnToolCall
    }

    fn route_snapshot(
        &self,
        request: &CompletionRequest,
    ) -> crate::llm::provider_turn::RouteSnapshot {
        tool_calling_fallback_route(&request.model)
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["fallback-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "the fallback route must remain streaming".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let events = match call_no {
            0 => vec![
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: None,
                        thinking_delta: Some("fallback reasoning state".to_string()),
                    }),
                },
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: Some(ToolCallDelta {
                            id: "fallback-call".to_string(),
                            name: Some("recording_tool".to_string()),
                            arguments_delta: r#"{"value":"fallback"}"#.to_string().into(),
                            index: Some(0),
                            thought_signature: None,
                        }),
                        finish_reason: Some(FinishReason::ToolCalls),
                        usage: Some(Usage::default()),
                        thinking_delta: None,
                    }),
                },
            ],
            1 => vec![ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: "fallback final answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: Some(Usage::default()),
                    thinking_delta: None,
                }),
            }],
            _ => {
                return Err(CoreError::Llm(
                    "the fallback route received an unexpected extra request".to_string(),
                ));
            }
        };
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for MissingRequiredReasoningProvider {
    fn name(&self) -> &str {
        "missing-required-reasoning-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-v4".to_string()])
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            content: String::new(),
            tool_calls: Some(vec![ToolCallRequest {
                id: "call-recovery".to_string(),
                name: "recording_tool".to_string(),
                arguments: if request.reasoning_enabled == Some(false) {
                    r#"{"value":"safe-restart"}"#.to_string()
                } else {
                    r#"{"value":"missing-replay"}"#.to_string()
                },
                thought_signature: None,
            }]),
            finish_reason: FinishReason::ToolCalls,
            usage: Usage::default(),
            thinking: None,
            provider_replay: None,
        })
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        if request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool)
        {
            return crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![
                Ok(StreamChunk {
                    delta: "final answer after safe restart".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                }),
            ])));
        }
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "call-stream".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"unsafe"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            },
        )])))
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::RequiredOnToolCall
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ThinkingMockProvider {
    fn name(&self) -> &str {
        "thinking-mock"
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::RequiredOnToolCall
    }

    fn route_snapshot(
        &self,
        request: &CompletionRequest,
    ) -> crate::llm::provider_turn::RouteSnapshot {
        crate::llm::provider_turn::RouteSnapshot {
            provider_endpoint_id: "deepseek-public".to_string(),
            provider_family: "deepseek".to_string(),
            api_style: crate::llm::reasoning_profile::ReasoningApiStyle::OpenAiChatCompletions,
            model_id: request.model.clone(),
            reasoning_profile_id: "deepseek-chat-v1".to_string(),
            reasoning_profile_version: 1,
            replay_policy: if request.reasoning_enabled == Some(false) {
                ReasoningReplayPolicy::NotRequired
            } else {
                ReasoningReplayPolicy::RequiredOnToolCall
            },
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some("first round reasoning".to_string()),
                }),
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: "call_1".to_string(),
                        name: Some("mock_tool".to_string()),
                        arguments_delta: r#"{"value":"ok"}"#.to_string().into(),
                        index: Some(0),
                        thought_signature: None,
                    }),
                    finish_reason: Some(crate::llm::FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                }),
            ]
        } else {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some("second round reasoning".to_string()),
                }),
                Ok(StreamChunk {
                    delta: "final answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(crate::llm::FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                }),
            ]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct RecoveringStreamProvider {
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
}

struct EmptyMetadataContextOverflowProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for EmptyMetadataContextOverflowProvider {
    fn name(&self) -> &str {
        "empty-metadata-context-overflow-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Ok(CompletionResponse {
            content: "Older turns were compacted.".to_string(),
            tool_calls: None,
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
            thinking: None,
            provider_replay: None,
        })
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        let call = self.stream_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call > 3 {
            return Err(CoreError::Internal(
                "context compaction circuit breaker allowed a fourth model request".to_string(),
            ));
        }

        Ok(Box::pin(stream::iter(vec![
            ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: Some(Usage::default()),
                    thinking_delta: None,
                }),
            },
            ProviderStreamEvent::TerminalError {
                failure: crate::llm::ProviderStreamFailure::ContextOverflow {
                    prompt_tokens: 200,
                    max_tokens: 100,
                },
            },
        ])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for RecoveringStreamProvider {
    fn name(&self) -> &str {
        "recovering-stream-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            content: "complete answer".to_string(),
            tool_calls: None,
            finish_reason: FinishReason::Stop,
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 2,
                total_tokens: 12,
                thinking_tokens: None,
                tool_prompt_tokens: None,
                cache_read_tokens: None,
                cache_miss_tokens: None,
                cache_creation_tokens: None,
                provider_raw: None,
            },
            thinking: None,
            provider_replay: None,
        })
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Err(
            CoreError::StreamIncomplete(
                "stream interrupted before output: error decoding response body".to_string(),
            ),
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct FlakyThenSuccessfulStreamProvider {
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for FlakyThenSuccessfulStreamProvider {
    fn name(&self) -> &str {
        "flaky-then-successful-stream-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Err(CoreError::Llm(
            "non-streaming fallback should not be needed".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if call_no == 0 {
            return crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![
                Err(CoreError::StreamIncomplete(
                    "stream interrupted before output: error decoding response body".to_string(),
                )),
            ])));
        }

        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: "stream answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct VisibleThenInterruptedProvider {
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
}

struct ToolCallThenInterruptedProvider {
    stream_calls: Arc<AtomicUsize>,
}

struct CancelledStreamProvider {
    stream_calls: Arc<AtomicUsize>,
    visible_output: bool,
}

#[derive(Clone, Copy)]
enum PendingCancellationPoint {
    StreamOpen,
    StreamReadAfterVisible,
}

struct PendingCancellationProvider {
    stream_calls: Arc<AtomicUsize>,
    invocation_started: Arc<Notify>,
    cancellation_point: PendingCancellationPoint,
}

struct ProviderHostedToolProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for ProviderHostedToolProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-v4-flash".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "provider-hosted tool lifecycle must stay on the original stream".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let hosted = |status| ProviderStreamEvent::HostedTool {
            tool: Box::new(crate::llm::ProviderHostedToolEvent {
                call_id: "ws-1".to_string(),
                tool_name: "web_search".to_string(),
                kind: crate::llm::ProviderHostedToolKind::WebSearch,
                provider_id: "deepseek".to_string(),
                status,
                arguments: Some("{\"query\":\"Nexa\"}".to_string()),
                content: None,
                artifacts: Some(serde_json::json!({
                    "kind": "providerHostedTool",
                    "providerId": "deepseek",
                    "itemType": "web_search_call",
                })),
            }),
        };
        Ok(Box::pin(stream::iter(vec![
            hosted(ProviderHostedToolStatus::Running),
            hosted(ProviderHostedToolStatus::Completed),
            ProviderStreamEvent::ReplayState {
                replay: Box::new(
                    crate::llm::provider_turn::ProviderReplayPayload::DeepSeekResponseItems(
                        crate::llm::provider_turn::ResponsesReplayPayload {
                            response_status: "completed".to_string(),
                            items: vec![
                                serde_json::json!({
                                    "type": "reasoning",
                                    "id": "rs-1",
                                    "status": "completed",
                                    "content": [{"type": "reasoning_text", "text": "search"}]
                                }),
                                serde_json::json!({
                                    "type": "web_search_call",
                                    "id": "ws-1",
                                    "status": "completed",
                                    "action": {"type": "search", "query": "Nexa"}
                                }),
                                serde_json::json!({
                                    "type": "message",
                                    "id": "msg-1",
                                    "status": "completed",
                                    "content": [{"type": "output_text", "text": "provider answer"}]
                                }),
                            ],
                        },
                    ),
                ),
            },
            ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: "provider answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }),
            },
            ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: Some(Usage::default()),
                    thinking_delta: None,
                }),
            },
        ])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for VisibleThenInterruptedProvider {
    fn name(&self) -> &str {
        "visible-then-interrupted-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Err(CoreError::Llm(
            "visible output must never enter non-streaming fallback".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![
                Ok(StreamChunk {
                    delta: "partial answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }),
                Err(CoreError::StreamIncomplete(
                    "stream interrupted after output".to_string(),
                )),
            ]
        } else {
            vec![Ok(StreamChunk {
                delta: "complete replayed answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: Some(Usage::default()),
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ToolCallThenInterruptedProvider {
    fn name(&self) -> &str {
        "tool-call-then-interrupted-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "stream replay should recover before non-streaming fallback".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let tool_chunk = || StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "write-once".to_string(),
                name: Some("recording_tool".to_string()),
                arguments_delta: r#"{"value":"write once"}"#.to_string().into(),
                index: Some(0),
                thought_signature: None,
            }),
            finish_reason: Some(FinishReason::ToolCalls),
            usage: Some(Usage::default()),
            thinking_delta: None,
        };
        let chunks = match call_no {
            0 => vec![
                Ok(StreamChunk {
                    finish_reason: None,
                    ..tool_chunk()
                }),
                Err(CoreError::StreamIncomplete(
                    "connection closed while tool arguments were visible".to_string(),
                )),
            ],
            1 => vec![Ok(tool_chunk())],
            _ => vec![Ok(StreamChunk {
                delta: "write recovered and verified".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: Some(Usage::default()),
                thinking_delta: None,
            })],
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for CancelledStreamProvider {
    fn name(&self) -> &str {
        "cancelled-stream-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "cancelled stream must not enter completion fallback".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let mut events = if self.visible_output {
            vec![
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: None,
                        thinking_delta: Some("visible reasoning before cancellation".to_string()),
                    }),
                },
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: "visible answer before cancellation".to_string(),
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: None,
                        thinking_delta: None,
                    }),
                },
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: Some(ToolCallDelta {
                            id: "cancelled-call".to_string(),
                            name: Some("recording_tool".to_string()),
                            arguments_delta: r#"{"value":"must-not-run"}"#.to_string().into(),
                            index: Some(0),
                            thought_signature: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        thinking_delta: None,
                    }),
                },
            ]
        } else {
            vec![ProviderStreamEvent::Chunk {
                chunk: Box::new(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: Some(Usage::default()),
                    thinking_delta: None,
                }),
            }]
        };
        events.push(ProviderStreamEvent::Cancelled {
            message: "cancelled by user".to_string(),
        });
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for PendingCancellationProvider {
    fn name(&self) -> &str {
        "pending-cancellation-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "pending stream cancellation must not enter completion fallback".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        match self.cancellation_point {
            PendingCancellationPoint::StreamOpen => {
                self.invocation_started.notify_one();
                std::future::pending::<Result<BoxStream<'_, ProviderStreamEvent>, CoreError>>()
                    .await
            }
            PendingCancellationPoint::StreamReadAfterVisible => {
                let invocation_started = Arc::clone(&self.invocation_started);
                Ok(Box::pin(stream::unfold(0_u8, move |state| {
                    let invocation_started = Arc::clone(&invocation_started);
                    async move {
                        match state {
                            0 => Some((
                                ProviderStreamEvent::Chunk {
                                    chunk: Box::new(StreamChunk {
                                        delta: "partial answer before user cancellation"
                                            .to_string(),
                                        tool_call_delta: Some(ToolCallDelta {
                                            id: "pending-cancelled-call".to_string(),
                                            name: Some("recording_tool".to_string()),
                                            arguments_delta:
                                                r#"{"value":"must-not-run"}"#.to_string().into(),
                                            index: Some(0),
                                            thought_signature: None,
                                        }),
                                        finish_reason: None,
                                        usage: None,
                                        thinking_delta: Some(
                                            "partial reasoning before user cancellation"
                                                .to_string(),
                                        ),
                                    }),
                                },
                                1,
                            )),
                            _ => {
                                invocation_started.notify_one();
                                std::future::pending::<Option<(ProviderStreamEvent, u8)>>().await
                            }
                        }
                    }
                })))
            }
        }
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct SteeringInterruptProvider {
    stream_calls: Arc<AtomicUsize>,
    request_texts: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl LlmProvider for SteeringInterruptProvider {
    fn name(&self) -> &str {
        "steering-interrupt-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.request_texts.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|message| format!("{:?}:{}", message.role, message.text_content()))
                .collect(),
        );

        if call_no == 0 {
            return crate::llm::provider_events_from_chunk_stream(Box::pin(stream::unfold(
                0,
                |state| async move {
                    match state {
                        0 => Some((
                            Ok(StreamChunk {
                                delta: "obsolete draft ".to_string(),
                                tool_call_delta: None,
                                finish_reason: None,
                                usage: None,
                                thinking_delta: None,
                            }),
                            1,
                        )),
                        1 => {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            Some((
                                Ok(StreamChunk {
                                    delta: "should not be used".to_string(),
                                    tool_call_delta: None,
                                    finish_reason: Some(FinishReason::Stop),
                                    usage: None,
                                    thinking_delta: None,
                                }),
                                2,
                            ))
                        }
                        _ => None,
                    }
                },
            )));
        }

        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: "steered answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: Some(Usage {
                    prompt_tokens: 12,
                    completion_tokens: 2,
                    total_tokens: 14,
                    thinking_tokens: None,
                    tool_prompt_tokens: None,
                    cache_read_tokens: None,
                    cache_miss_tokens: None,
                    cache_creation_tokens: None,
                    provider_raw: None,
                }),
                thinking_delta: None,
            },
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct MockTool;

struct RecordingTool {
    executions: Arc<AtomicUsize>,
}

struct NamedMockTool(&'static str);

struct CountingNamedMockTool {
    name: &'static str,
    executions: Arc<AtomicUsize>,
}

struct ErrorNamedMockTool {
    name: &'static str,
    executions: Arc<AtomicUsize>,
}

struct DefinitionCountingTool {
    definition_calls: Arc<AtomicUsize>,
    schema_repetitions: usize,
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        "mock_tool"
    }

    fn description(&self) -> &str {
        "Mock tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "tool-ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for RecordingTool {
    fn name(&self) -> &str {
        "recording_tool"
    }

    fn description(&self) -> &str {
        "Records whether it was executed"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: "recording-tool-executed".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for NamedMockTool {
    fn name(&self) -> &str {
        self.0
    }

    fn description(&self) -> &str {
        "Named mock tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for CountingNamedMockTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Counting named mock tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": { "type": "array" },
                "batch_goal": { "type": "string" },
                "parallel_group": { "type": "string" },
                "max_parallel": { "type": "integer" }
            },
            "required": ["tasks"],
            "additionalProperties": true
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: "ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for ErrorNamedMockTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Failing named mock tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: "direct-dispatch-error".to_string(),
            is_error: true,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for DefinitionCountingTool {
    fn name(&self) -> &str {
        "definition_counting_tool"
    }

    fn description(&self) -> &str {
        "Tracks whether an answer-only turn constructs its schema"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.definition_calls.fetch_add(1, Ordering::SeqCst);
        serde_json::json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "string",
                    "description": "schema content that answer-only mode must never account "
                        .repeat(self.schema_repetitions)
                }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        Ok(ToolResult {
            call_id: context.call_id.to_string(),
            content: "unexpected".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

struct ParallelProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for ParallelProvider {
    fn name(&self) -> &str {
        "parallel-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: "fast_call".to_string(),
                        name: Some("fast_tool".to_string()),
                        arguments_delta: r#"{"value":"fast"}"#.to_string().into(),
                        index: Some(0),
                        thought_signature: None,
                    }),
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }),
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: "slow_call".to_string(),
                        name: Some("slow_tool".to_string()),
                        arguments_delta: r#"{"value":"slow"}"#.to_string().into(),
                        index: Some(1),
                        thought_signature: None,
                    }),
                    finish_reason: Some(crate::llm::FinishReason::Stop),
                    usage: None,
                    thinking_delta: None,
                }),
            ]
        } else {
            vec![Ok(StreamChunk {
                delta: "parallel final answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct ScriptedProvider {
    stream_calls: Arc<AtomicUsize>,
    first_chunks: Vec<StreamChunk>,
    final_answer: &'static str,
}

struct ThoughtOnlyProvider {
    stream_calls: Arc<AtomicUsize>,
    finish_reason: FinishReason,
}

#[async_trait]
impl LlmProvider for ThoughtOnlyProvider {
    fn name(&self) -> &str {
        "thought-only-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: Some(self.finish_reason.clone()),
                usage: None,
                thinking_delta: Some("raw internal reasoning".to_string()),
            },
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct AnswerOnlyRecoveryProvider {
    stream_calls: Arc<AtomicUsize>,
    request_reasoning: Arc<Mutex<Vec<(Option<bool>, Option<u32>, Option<String>)>>>,
}

struct ToolingAnswerRecoveryProvider {
    stream_calls: Arc<AtomicUsize>,
    request_reasoning: Arc<Mutex<Vec<Option<bool>>>>,
}

struct LengthContinuationProvider {
    stream_calls: Arc<AtomicUsize>,
    request_reasoning: Arc<Mutex<Vec<Option<bool>>>>,
}

struct MultiLengthContinuationProvider {
    stream_calls: Arc<AtomicUsize>,
}

struct ContextLimitTerminalProvider {
    stream_calls: Arc<AtomicUsize>,
    saw_compacted_retry: Arc<Mutex<bool>>,
    draft_tool_on_first_sample: bool,
}

struct TruncatedToolCallProvider {
    stream_calls: Arc<AtomicUsize>,
    saw_safe_replan_context: Arc<Mutex<bool>>,
}

struct TruncatedThenCommittedToolProvider {
    stream_calls: Arc<AtomicUsize>,
}

struct AnswerOnlyToolViolationProvider {
    stream_calls: Arc<AtomicUsize>,
    saw_tools_suppressed: Arc<Mutex<Vec<bool>>>,
}

struct MalformedToolCallProvider {
    stream_calls: Arc<AtomicUsize>,
    saw_safe_replan_context: Arc<Mutex<bool>>,
}

#[async_trait]
impl LlmProvider for AnswerOnlyRecoveryProvider {
    fn name(&self) -> &str {
        "answer-only-recovery-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-reasoner".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.request_reasoning.lock().unwrap().push((
            request.reasoning_enabled,
            request.thinking_budget,
            request.reasoning_effort.as_ref().map(ToString::to_string),
        ));
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: Some("raw internal reasoning that must stay private".to_string()),
            }
        } else {
            StreamChunk {
                delta: "recovered final answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

fn provider_context_limit_history() -> Vec<Message> {
    (0..8)
        .flat_map(|turn| {
            [
                Message::text(
                    Role::User,
                    format!("older user turn {turn} with context to preserve"),
                ),
                Message::text(
                    Role::Assistant,
                    format!("older assistant turn {turn} with verified facts"),
                ),
            ]
        })
        .collect()
}

#[async_trait]
impl LlmProvider for ToolingAnswerRecoveryProvider {
    fn name(&self) -> &str {
        "tooling-answer-recovery-mock"
    }

    fn reasoning_replay_policy(&self, _model: &str) -> ReasoningReplayPolicy {
        ReasoningReplayPolicy::NotRequired
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["reasoning-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.request_reasoning
            .lock()
            .unwrap()
            .push(request.reasoning_enabled);
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = match call_no {
            0 => StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: Some("initial reasoning filled the response budget".to_string()),
            },
            1 => StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "recovery-tool-call".to_string(),
                    name: Some("mock_tool".to_string()),
                    arguments_delta: r#"{"value":"ok"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            },
            _ if request.reasoning_enabled == Some(false) => StreamChunk {
                delta: "recovered final answer after tool use".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
            _ => StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: Some("reasoning was incorrectly re-enabled".to_string()),
            },
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for LengthContinuationProvider {
    fn name(&self) -> &str {
        "length-continuation-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["reasoning-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.request_reasoning
            .lock()
            .unwrap()
            .push(request.reasoning_enabled);
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: "first half, ".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: Some("private reasoning".to_string()),
            }
        } else {
            StreamChunk {
                delta: "second half".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for MultiLengthContinuationProvider {
    fn name(&self) -> &str {
        "multi-length-continuation-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["reasoning-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let (delta, finish_reason) = match call_no {
            0 => ("one ", FinishReason::Length),
            1 => ("two ", FinishReason::Length),
            2 => ("three ", FinishReason::Length),
            _ => ("four", FinishReason::Stop),
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: delta.to_string(),
                tool_call_delta: None,
                finish_reason: Some(finish_reason),
                usage: None,
                thinking_delta: None,
            },
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ContextLimitTerminalProvider {
    fn name(&self) -> &str {
        "context-limit-terminal-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["private-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Ok(CompletionResponse {
            content: "Compacted facts from older turns.".to_string(),
            tool_calls: None,
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
            thinking: None,
            provider_replay: None,
        })
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: String::new(),
                tool_call_delta: self.draft_tool_on_first_sample.then(|| ToolCallDelta {
                    id: "context-limited-draft".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"must-not-run"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ContextLimit),
                usage: None,
                thinking_delta: None,
            }
        } else {
            *self.saw_compacted_retry.lock().unwrap() = request.messages.iter().any(|message| {
                message.role == Role::System
                    && message
                        .text_content()
                        .starts_with("## Earlier conversation context (compacted)")
            });
            StreamChunk {
                delta: "final answer after context rollover".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for TruncatedToolCallProvider {
    fn name(&self) -> &str {
        "truncated-tool-call-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "truncated-call".to_string(),
                    name: Some("recording_tool".to_string()),
                    // This is valid JSON, but the provider may have cut a longer
                    // string at the output boundary. It is unsafe to execute.
                    arguments_delta: r#"{"value":"apparently-valid"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: None,
            }
        } else {
            let has_tool_protocol_unit = request.messages.iter().any(|message| {
                message.role == Role::Tool
                    || message
                        .tool_calls
                        .as_ref()
                        .is_some_and(|calls| !calls.is_empty())
            });
            let has_replan_instruction = request.messages.iter().any(|message| {
                message.role == Role::System
                    && message.text_content().contains("use append operations")
            });
            *self.saw_safe_replan_context.lock().unwrap() =
                !has_tool_protocol_unit && has_replan_instruction;
            StreamChunk {
                delta: "final answer after re-planning".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for TruncatedThenCommittedToolProvider {
    fn name(&self) -> &str {
        "truncated-then-committed-tool-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = match call_no {
            0 => StreamChunk {
                delta: "discarded draft".to_string(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "draft-call".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"draft"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::Length),
                usage: None,
                thinking_delta: None,
            },
            1 => StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "committed-call".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"committed"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            },
            _ => StreamChunk {
                delta: "final after one verified tool round".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for AnswerOnlyToolViolationProvider {
    fn name(&self) -> &str {
        "answer-only-tool-violation-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.saw_tools_suppressed
            .lock()
            .unwrap()
            .push(request.tools.as_ref().is_none_or(Vec::is_empty));
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "forbidden-call".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"must-not-run"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            }
        } else {
            StreamChunk {
                delta: "answer after respecting the synthesis boundary".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for MalformedToolCallProvider {
    fn name(&self) -> &str {
        "malformed-tool-call-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-v4-pro".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no == 0 {
            StreamChunk {
                delta: "I will inspect the repository.".to_string(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "malformed-call".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"unterminated""#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            }
        } else {
            let has_incomplete_replay = request.messages.iter().any(|message| {
                message.tool_calls.as_deref().is_some_and(|calls| {
                    calls
                        .iter()
                        .any(|call| !crate::llm::message_validation::is_complete_tool_call(call))
                })
            });
            let has_tool_result = request
                .messages
                .iter()
                .any(|message| message.role == Role::Tool);
            let has_replan_instruction = request.messages.iter().any(|message| {
                message.role == Role::System
                    && message
                        .text_content()
                        .contains("incomplete tool-call envelope")
            });
            *self.saw_safe_replan_context.lock().unwrap() =
                !has_incomplete_replay && !has_tool_result && has_replan_instruction;
            StreamChunk {
                delta: "final answer after safe re-planning".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(chunk)])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct GoalLifecycleProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for GoalLifecycleProvider {
    fn name(&self) -> &str {
        "goal-lifecycle-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = match call_no {
            0 => vec![StreamChunk {
                delta: "I made partial progress.".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }],
            1 => vec![StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "complete-goal".to_string(),
                    name: Some("update_goal".to_string()),
                    arguments_delta: r#"{"status":"complete"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }],
            _ => vec![StreamChunk {
                delta: "The goal is complete and verified.".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }],
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(
            chunks.into_iter().map(Ok),
        )))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            self.first_chunks.clone()
        } else {
            vec![StreamChunk {
                delta: self.final_answer.to_string(),
                tool_call_delta: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(
            chunks.into_iter().map(Ok),
        )))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct CapturingScriptedProvider {
    stream_calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Vec<(Role, String)>>>>,
    first_chunks: Vec<StreamChunk>,
    final_answer: &'static str,
}

struct ToolSurfaceCapturingProvider {
    tool_names: Arc<Mutex<Vec<Vec<String>>>>,
    latest_user_texts: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl LlmProvider for ToolSurfaceCapturingProvider {
    fn name(&self) -> &str {
        "tool-surface-capturing-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-chat".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        self.latest_user_texts.lock().unwrap().push(
            request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == Role::User)
                .map(Message::text_content),
        );
        self.tool_names.lock().unwrap().push(
            request
                .tools
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
        );
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
            StreamChunk {
                delta: "done".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        )])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct DeferredCacheTool;

struct OversizedDeferredCacheTool;

#[async_trait]
impl Tool for DeferredCacheTool {
    fn name(&self) -> &str {
        "mcp__cache_test__deferred"
    }

    fn description(&self) -> &str {
        "A route-deferred tool used to verify cache-stable tool surfaces."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{}})
    }

    fn categories(&self) -> &'static [crate::tools::ToolCategory] {
        &[crate::tools::ToolCategory::Mcp]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl Tool for OversizedDeferredCacheTool {
    fn name(&self) -> &str {
        "mcp__cache_test__oversized"
    }

    fn description(&self) -> &str {
        "An oversized deferred schema used to verify bounded cache-stable surfaces."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "string",
                    "description": "cache schema field description ".repeat(300)
                }
            }
        })
    }

    fn categories(&self) -> &'static [crate::tools::ToolCategory] {
        &[crate::tools::ToolCategory::Mcp]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

#[async_trait]
impl LlmProvider for CapturingScriptedProvider {
    fn name(&self) -> &str {
        "capturing-scripted-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|message| (message.role.clone(), message.text_content()))
                .collect(),
        );
        let chunks = if call_no == 0 {
            self.first_chunks.clone()
        } else {
            vec![StreamChunk {
                delta: self.final_answer.to_string(),
                tool_call_delta: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(
            chunks.into_iter().map(Ok),
        )))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn prefix_cached_provider_pins_full_tool_surface_when_dynamic_visibility_is_enabled() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    registry.register(Box::new(DeferredCacheTool));
    registry.register(Box::new(NamedMockTool("update_plan")));
    let expected = registry
        .definitions()
        .into_iter()
        .filter(|tool| tool.name != "update_plan")
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let latest_user_texts = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured),
            latest_user_texts,
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            dynamic_tool_visibility: true,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(16);

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Explain why a stable prompt prefix matters.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("agent turn");

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0], expected);
    assert!(requests[0]
        .iter()
        .any(|name| name == "mcp__cache_test__deferred"));
    assert!(!requests[0].iter().any(|name| name == "update_plan"));
}

#[tokio::test]
async fn explicit_nexus_keeps_task_plan_tool_visible() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    registry.register(Box::new(NamedMockTool("update_plan")));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            dynamic_tool_visibility: false,
            power_mode: power_mode::AgentPowerMode::Nexus,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(16);

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Plan and verify a multi-stage change.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("agent turn");

    assert!(captured.lock().unwrap()[0]
        .iter()
        .any(|name| name == "update_plan"));
}

#[tokio::test]
async fn failed_direct_dispatch_consumes_the_finite_tool_round() {
    let direct_executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ErrorNamedMockTool {
        name: "list_sources",
        executions: Arc::clone(&direct_executions),
    }));
    registry.register(Box::new(MockTool));
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured_tools),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("mock-model".to_string()),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "list sources".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the failed direct dispatch should fall back to synthesis");

    assert_eq!(final_message.text_content(), "done");
    assert_eq!(direct_executions.load(Ordering::SeqCst), 1);
    let captured_tools = captured_tools.lock().unwrap();
    assert_eq!(captured_tools.len(), 1);
    assert!(
        captured_tools[0].is_empty(),
        "the failed direct execution must consume the sole tool round before model fallback"
    );
}

#[tokio::test]
async fn answer_only_prompt_never_constructs_or_accounts_tool_schemas() {
    let definition_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DefinitionCountingTool {
        definition_calls: Arc::clone(&definition_calls),
        schema_repetitions: 300,
    }));
    let calls_after_registration = definition_calls.load(Ordering::SeqCst);
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured_tools),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("mock-model".to_string()),
            max_iterations: 0,
            max_tokens: Some(256),
            context_window: Some(8_192),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Answer from the supplied conversation without tools.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("answer-only prompt should fit without tool schema reservation");

    assert_eq!(final_message.text_content(), "done");
    assert_eq!(
        definition_calls.load(Ordering::SeqCst),
        calls_after_registration,
        "answer-only prompt preparation must not even construct the unavailable schema"
    );
    let captured_tools = captured_tools.lock().unwrap();
    assert_eq!(captured_tools.len(), 1);
    assert!(captured_tools[0].is_empty());
}

#[tokio::test]
async fn reserved_final_sample_excludes_suppressed_schemas_from_cumulative_accounting() {
    let definition_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DefinitionCountingTool {
        definition_calls: Arc::clone(&definition_calls),
        schema_repetitions: 4_000,
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(ScriptedProvider {
            stream_calls: Arc::clone(&stream_calls),
            first_chunks: vec![StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "large-schema-call".to_string(),
                    name: Some("definition_counting_tool".to_string()),
                    arguments_delta: r#"{"payload":"evidence"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: Some(Usage {
                    prompt_tokens: 109_000,
                    completion_tokens: 1_000,
                    total_tokens: 110_000,
                    thinking_tokens: None,
                    tool_prompt_tokens: None,
                    cache_read_tokens: None,
                    cache_miss_tokens: None,
                    cache_creation_tokens: None,
                    provider_raw: None,
                }),
                thinking_delta: None,
            }],
            final_answer: "final answer from the committed tool result",
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("mock-model".to_string()),
            max_iterations: 1,
            max_tokens: Some(2_048),
            context_window: Some(400_000),
            max_actual_tokens_per_run: Some(150_000),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Use the tool once, then synthesize the answer.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the tool-free final sample should fit its remaining cumulative budget");

    assert_eq!(
        final_message.text_content(),
        "final answer from the committed tool result"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert!(
        definition_calls.load(Ordering::SeqCst) > 0,
        "the first tool-enabled sample must still build the large schema"
    );
}

#[tokio::test]
async fn nexus_keeps_delegation_tools_visible_on_the_first_model_step() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(crate::tools::tool_search_tool::ToolSearchTool));
    registry.register(Box::new(MockTool));
    for name in [
        "spawn_subagent_batch",
        "spawn_subagent",
        "judge_subagent_results",
    ] {
        registry.register(Box::new(NamedMockTool(name)));
    }
    let captured = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("gpt-5.6".to_string()),
            provider_type: Some(ProviderType::OpenAi),
            dynamic_tool_visibility: true,
            power_mode: power_mode::AgentPowerMode::Nexus,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(16);

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Investigate a complex cross-module regression.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("agent turn");

    let requests = captured.lock().unwrap();
    // Nexus may issue a controller follow-up when the mock result does not
    // satisfy its verification gates. This contract concerns the first model
    // surface, not the total number of verification rounds.
    assert!(
        !requests.is_empty(),
        "Nexus should make at least one model request"
    );
    for name in [
        "spawn_subagent_batch",
        "spawn_subagent",
        "judge_subagent_results",
    ] {
        assert!(requests[0].iter().any(|offered| offered == name));
    }
}

#[tokio::test]
async fn nexus_answer_only_budget_blocks_controller_reconnaissance() {
    let reconnaissance_executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingNamedMockTool {
        name: "spawn_subagent_batch",
        executions: Arc::clone(&reconnaissance_executions),
    }));
    registry.register(Box::new(NamedMockTool("spawn_subagent")));
    registry.register(Box::new(NamedMockTool("judge_subagent_results")));
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured_tools),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("gpt-5.6".to_string()),
            provider_type: Some(ProviderType::OpenAi),
            power_mode: power_mode::AgentPowerMode::Nexus,
            orchestration_profile: OrchestrationProfile::ResearchUltra,
            max_iterations: 0,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(256);

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Investigate and verify a complex cross-module Rust refactor, run cargo tests, and use independent agents."
                    .to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("answer-only Nexus turn");

    assert_eq!(
        reconnaissance_executions.load(Ordering::SeqCst),
        0,
        "zero verified tool rounds must block controller-owned reconnaissance"
    );
    let captured_tools = captured_tools.lock().unwrap();
    assert_eq!(captured_tools.len(), 1);
    assert!(captured_tools[0].is_empty());
}

#[tokio::test]
async fn nexus_reconnaissance_consumes_the_finite_tool_round() {
    let reconnaissance_executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingNamedMockTool {
        name: "spawn_subagent_batch",
        executions: Arc::clone(&reconnaissance_executions),
    }));
    registry.register(Box::new(NamedMockTool("spawn_subagent")));
    registry.register(Box::new(NamedMockTool("judge_subagent_results")));
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured_tools),
            latest_user_texts: Arc::new(Mutex::new(Vec::new())),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("gpt-5.6".to_string()),
            provider_type: Some(ProviderType::OpenAi),
            power_mode: power_mode::AgentPowerMode::Nexus,
            orchestration_profile: OrchestrationProfile::ResearchUltra,
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(256);

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Investigate and verify a complex cross-module Rust refactor, run cargo tests, and use independent agents."
                    .to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("bounded Nexus turn");

    let mut controller_codes = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ControllerStatus { code, .. } = event {
            controller_codes.push(code);
        }
    }
    let captured_tools = captured_tools.lock().unwrap();
    assert_eq!(
        reconnaissance_executions.load(Ordering::SeqCst),
        1,
        "the automatic reconnaissance batch must consume the sole tool round; controller_codes={controller_codes:?}, captured_tools={captured_tools:?}"
    );
    assert_eq!(captured_tools.len(), 1);
    assert!(
        captured_tools[0].is_empty(),
        "the reserved synthesis sample must suppress tools after reconnaissance uses the budget"
    );
}

#[tokio::test]
async fn prefix_cached_provider_uses_bounded_resident_surface_without_dropping_user() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(crate::tools::tool_search_tool::ToolSearchTool));
    registry.register(Box::new(MockTool));
    registry.register(Box::new(OversizedDeferredCacheTool));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let latest_user_texts = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(ToolSurfaceCapturingProvider {
            tool_names: Arc::clone(&captured),
            latest_user_texts: Arc::clone(&latest_user_texts),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            context_window: Some(8_192),
            dynamic_tool_visibility: true,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(16);
    let query = "Keep this exact user request in the bounded prompt.";

    executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: query.to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("agent turn");

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].iter().any(|name| name == "tool_search"));
    assert!(!requests[0].iter().any(|name| name == "mock_tool"));
    assert!(!requests[0]
        .iter()
        .any(|name| name == "mcp__cache_test__oversized"));
    assert_eq!(
        latest_user_texts.lock().unwrap().as_slice(),
        &[Some(query.to_string())]
    );
}

struct LoopGuardSteeringProvider {
    stream_calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Vec<(Role, String)>>>>,
    steering_tx: mpsc::UnboundedSender<AgentSteeringMessage>,
}

struct PostSynthesisSteeringProvider {
    stream_calls: Arc<AtomicUsize>,
    steering_tx: mpsc::UnboundedSender<AgentSteeringMessage>,
    saw_steering_on_replacement: Arc<Mutex<bool>>,
}

struct ProviderPauseReplayProvider {
    stream_calls: Arc<AtomicUsize>,
    saw_native_pause_blocks: Arc<Mutex<bool>>,
}

struct LoopGuardBudgetProvider {
    stream_calls: Arc<AtomicUsize>,
    tools_visible_on_strategy_change: Arc<Mutex<bool>>,
}

const LOOP_GUARD_REPEATED_DRAFT: &str = "This repeated stale draft keeps restating the same incomplete conclusion without taking a new action or adding useful evidence.";

#[async_trait]
impl LlmProvider for LoopGuardSteeringProvider {
    fn name(&self) -> &str {
        "loop-guard-steering-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["deepseek-chat".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|message| (message.role.clone(), message.text_content()))
                .collect(),
        );

        let delta = if call_no < 3 {
            LOOP_GUARD_REPEATED_DRAFT
        } else {
            "fresh final answer"
        };
        let steering_tx = self.steering_tx.clone();
        let steering_text =
            (call_no < 2).then(|| format!("steering note {}", call_no.saturating_add(1)));
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::unfold(
            (0u8, Some(delta.to_string()), steering_text, steering_tx),
            |(state, delta, steering_text, steering_tx)| async move {
                if state == 0 {
                    return Some((
                        Ok(StreamChunk {
                            delta: delta.expect("first stream state should carry delta"),
                            tool_call_delta: None,
                            finish_reason: Some(crate::llm::FinishReason::Stop),
                            usage: None,
                            thinking_delta: None,
                        }),
                        (1, None, steering_text, steering_tx),
                    ));
                }

                if let Some(text) = steering_text {
                    let _ = steering_tx.send(AgentSteeringMessage::text(text));
                }
                None
            },
        )))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for PostSynthesisSteeringProvider {
    fn name(&self) -> &str {
        "post-synthesis-steering-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if call_no > 0 {
            *self.saw_steering_on_replacement.lock().unwrap() =
                request.messages.iter().any(|message| {
                    message.role == Role::User
                        && message
                            .text_content()
                            .contains("Replace the draft with the steered answer")
                });
        }
        let delta = if call_no == 0 {
            "obsolete synthesis draft"
        } else {
            "replacement answer after steering"
        };
        let steering =
            (call_no == 0).then(|| "Replace the draft with the steered answer.".to_string());
        let steering_tx = self.steering_tx.clone();
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::unfold(
            (0u8, Some(delta.to_string()), steering, steering_tx),
            |(state, delta, steering, steering_tx)| async move {
                if state == 0 {
                    return Some((
                        Ok(StreamChunk {
                            delta: delta.expect("first stream state should carry text"),
                            tool_call_delta: None,
                            finish_reason: Some(FinishReason::Stop),
                            usage: None,
                            thinking_delta: None,
                        }),
                        (1, None, steering, steering_tx),
                    ));
                }
                if let Some(steering) = steering {
                    let _ = steering_tx.send(AgentSteeringMessage::text(steering));
                }
                None
            },
        )))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for ProviderPauseReplayProvider {
    fn name(&self) -> &str {
        "anthropic-pause-replay-mock"
    }

    fn reasoning_replay_policy(
        &self,
        _model: &str,
    ) -> crate::llm::reasoning_profile::ReasoningReplayPolicy {
        crate::llm::reasoning_profile::ReasoningReplayPolicy::RequiredOnToolCall
    }

    fn route_snapshot(
        &self,
        request: &CompletionRequest,
    ) -> crate::llm::provider_turn::RouteSnapshot {
        crate::llm::provider_turn::RouteSnapshot {
            provider_endpoint_id: "anthropic-test".to_string(),
            provider_family: "anthropic".to_string(),
            api_style: crate::llm::reasoning_profile::ReasoningApiStyle::AnthropicMessages,
            model_id: request.model.clone(),
            reasoning_profile_id: "anthropic-pause-test".to_string(),
            reasoning_profile_version: 1,
            replay_policy: crate::llm::reasoning_profile::ReasoningReplayPolicy::RequiredOnToolCall,
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["claude-sonnet-5".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if call_no == 0 {
            let blocks = vec![
                serde_json::json!({"type":"text","text":"Searching"}),
                serde_json::json!({
                    "type":"server_tool_use",
                    "id":"srvtoolu_1",
                    "name":"web_search",
                    "input":{"query":"Nexa"}
                }),
                serde_json::json!({
                    "type":"web_search_tool_result",
                    "tool_use_id":"srvtoolu_1",
                    "content":[{"type":"web_search_result","url":"https://example.com"}]
                }),
            ];
            return Ok(Box::pin(stream::iter(vec![
                ProviderStreamEvent::ReplayState {
                    replay: Box::new(
                        crate::llm::provider_turn::ProviderReplayPayload::AnthropicPausedTurnBlocks(
                            blocks,
                        ),
                    ),
                },
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: Some(ToolCallDelta {
                            id: "toolu_draft".to_string(),
                            name: Some("write_file".to_string()),
                            arguments_delta: r#"{"path":"draft.txt"}"#.into(),
                            index: Some(0),
                            thought_signature: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        thinking_delta: None,
                    }),
                },
                ProviderStreamEvent::Chunk {
                    chunk: Box::new(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: Some(FinishReason::ProviderPause),
                        usage: None,
                        thinking_delta: None,
                    }),
                },
            ])));
        }

        *self.saw_native_pause_blocks.lock().unwrap() = request.messages.iter().any(|message| {
            message.provider_turn().is_some_and(|envelope| {
                matches!(
                    &envelope.replay_payload,
                    crate::llm::provider_turn::ProviderReplayPayload::AnthropicPausedTurnBlocks(
                        blocks
                    ) if blocks.iter().any(|block| {
                        block.get("type").and_then(serde_json::Value::as_str)
                            == Some("server_tool_use")
                    }) && blocks.iter().all(|block| {
                        block.get("type").and_then(serde_json::Value::as_str)
                            != Some("tool_use")
                    })
                )
            })
        });
        Ok(Box::pin(stream::iter(vec![ProviderStreamEvent::Chunk {
            chunk: Box::new(StreamChunk {
                delta: "completed after provider pause".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }),
        }])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for LoopGuardBudgetProvider {
    fn name(&self) -> &str {
        "loop-guard-budget-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunk = if call_no < 3 {
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: format!("repeat-{call_no}"),
                    name: Some("read_evidence".to_string()),
                    arguments_delta: r#"{"tasks":[],"scope":"same"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            }
        } else if call_no == 3 {
            *self.tools_visible_on_strategy_change.lock().unwrap() = request
                .tools
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|tool| tool.name == "alternate_action");
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "alternate-3".to_string(),
                    name: Some("alternate_action".to_string()),
                    arguments_delta: r#"{"tasks":[],"scope":"different"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                thinking_delta: None,
            }
        } else {
            StreamChunk {
                delta: "final after alternate strategy".to_string(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            }
        };
        Ok(Box::pin(stream::iter(vec![ProviderStreamEvent::Chunk {
            chunk: Box::new(chunk),
        }])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct DelayTool {
    name: &'static str,
    delay_ms: u64,
}

#[async_trait]
impl Tool for DelayTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Delay tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("{}-ok", self.name),
            is_error: false,
            artifacts: None,
        })
    }
}

struct BlockingTool {
    name: &'static str,
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for BlockingTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Blocking tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        self.release.notified().await;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("{}-ok", self.name),
            is_error: false,
            artifacts: None,
        })
    }
}

fn large_read_file_content() -> String {
    (0..260)
        .map(|index| {
            format!(
                "line-{index:03} {}",
                "abcdefghijklmnopqrstuvwxyz0123456789".repeat(8)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct LargeReadFileTool;

#[async_trait]
impl Tool for LargeReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a large file"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        })
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: large_read_file_content(),
            is_error: false,
            artifacts: None,
        })
    }
}

struct SerialDelayTool {
    name: &'static str,
    delay_ms: u64,
}

#[async_trait]
impl Tool for SerialDelayTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Serial delay tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
        false
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("{}-ok", self.name),
            is_error: false,
            artifacts: None,
        })
    }
}

struct ResourceLockedTool;

#[async_trait]
impl Tool for ResourceLockedTool {
    fn name(&self) -> &str {
        "locked_write"
    }

    fn description(&self) -> &str {
        "Resource locked write tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        })
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "locked-write-ok".to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}

struct ApprovalRequiredProvider {
    stream_calls: Arc<AtomicUsize>,
    tool_name: &'static str,
    arguments: &'static str,
}

#[async_trait]
impl LlmProvider for ApprovalRequiredProvider {
    fn name(&self) -> &str {
        "approval-required-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["mock-model".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm("not implemented".to_string()))
    }

    async fn stream_events(
        &self,
        _request: &CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'_, crate::llm::ProviderStreamEvent>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "approval_call_1".to_string(),
                    name: Some(self.tool_name.to_string()),
                    arguments_delta: self.arguments.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        } else {
            vec![Ok(StreamChunk {
                delta: "final answer".to_string(),
                tool_call_delta: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            })]
        };
        crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

fn test_tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        thought_signature: None,
    }
}

#[tokio::test]
async fn test_allow_all_tool_approval_does_not_emit_approval_request() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ResourceLockedTool));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ApprovalRequiredProvider {
        stream_calls: Arc::clone(&stream_calls),
        tool_name: "locked_write",
        arguments: r#"{"path":"notes/a.md"}"#,
    };
    let approval_calls = Arc::new(AtomicUsize::new(0));
    let approval_calls_for_cb = Arc::clone(&approval_calls);
    let approval_cb: ApprovalCallback = Arc::new(move |_req| {
        approval_calls_for_cb.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { ApprovalDecision::AllowOnce })
    });

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            require_tool_confirmation: true,
            tool_approval_mode: ToolApprovalMode::AllowAll,
            ..AgentConfig::default()
        },
    )
    .with_approval_callback(approval_cb);

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("run should succeed");

    let mut approval_requested = 0;
    let mut approval_resolved = 0;
    let mut approval_pending_updates = 0;
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::ApprovalRequested { .. }) => approval_requested += 1,
            Some(AgentEvent::ApprovalResolved { .. }) => approval_resolved += 1,
            Some(AgentEvent::ToolRunUpdated { run })
                if run.status == ToolRunStatus::ApprovalPending =>
            {
                approval_pending_updates += 1;
            }
            Some(AgentEvent::Done { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }

    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(approval_calls.load(Ordering::SeqCst), 0);
    assert_eq!(approval_requested, 0);
    assert_eq!(approval_resolved, 0);
    assert_eq!(approval_pending_updates, 0);
    assert_eq!(final_msg.text_content(), "final answer");
}

#[tokio::test]
async fn test_allow_all_cannot_bypass_computer_control_approval() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        crate::tools::computer_use_tool::ComputerControlTool,
    ));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ApprovalRequiredProvider {
        stream_calls: Arc::clone(&stream_calls),
        tool_name: "computer_control",
        arguments: r#"{"action":"focus_window","observation_id":"missing","window_id":42}"#,
    };
    let approval_calls = Arc::new(AtomicUsize::new(0));
    let approval_calls_for_cb = Arc::clone(&approval_calls);
    let approval_cb: ApprovalCallback = Arc::new(move |_request| {
        approval_calls_for_cb.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { ApprovalDecision::AllowOnce })
    });
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            tool_approval_mode: ToolApprovalMode::AllowAll,
            ..AgentConfig::default()
        },
    )
    .with_approval_callback(approval_cb);
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "focus an app".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("agent should recover from the expected missing observation");

    let mut approval_requested = 0;
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::ApprovalRequested { .. }) => approval_requested += 1,
            Some(AgentEvent::Done { .. }) | None => break,
            Some(_) => {}
        }
    }
    assert_eq!(approval_calls.load(Ordering::SeqCst), 1);
    assert_eq!(approval_requested, 1);
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(final_msg.text_content(), "final answer");
}

#[tokio::test]
async fn test_one_tool_round_budget_reserves_a_final_answer_sample() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = MockProvider {
        stream_calls: Arc::clone(&stream_calls),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("run should succeed");

    // Should perform two LLM calls: one for tool request, one after tool result.
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(final_msg.text_content(), "final answer");

    #[derive(Debug, PartialEq, Eq)]
    enum ToolLifecycleEvent {
        RunStarted,
        RunUpdated,
        RunCompleted,
        Preparing,
        Start,
        ArgsDelta,
        Result,
    }

    // Drain events and assert the stream exposes a stable lifecycle:
    // preparing while arguments are incomplete, start only after the final
    // arguments are available, and no generic partial-arguments deltas.
    let mut lifecycle = Vec::new();
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::ToolRunStarted { run }) => {
                assert_eq!(run.call_id, "call_1");
                assert_eq!(run.tool_name, "mock_tool");
                assert_eq!(run.status, ToolRunStatus::Preparing);
                lifecycle.push(ToolLifecycleEvent::RunStarted);
            }
            Some(AgentEvent::ToolRunUpdated { run }) => {
                assert_eq!(run.call_id, "call_1");
                assert_eq!(run.tool_name, "mock_tool");
                assert_eq!(run.status, ToolRunStatus::Running);
                assert_eq!(run.arguments.as_deref(), Some(r#"{"value":"ok"}"#));
                lifecycle.push(ToolLifecycleEvent::RunUpdated);
            }
            Some(AgentEvent::ToolRunCompleted { run }) => {
                assert_eq!(run.call_id, "call_1");
                assert_eq!(run.tool_name, "mock_tool");
                assert_eq!(run.status, ToolRunStatus::Completed);
                assert_eq!(run.content.as_deref(), Some("tool-ok"));
                lifecycle.push(ToolLifecycleEvent::RunCompleted);
            }
            Some(AgentEvent::ToolCallPreparing {
                call_id,
                tool_name,
                index,
                ..
            }) => {
                assert_eq!(call_id, "call_1");
                assert_eq!(tool_name, "mock_tool");
                assert_eq!(index, 0);
                lifecycle.push(ToolLifecycleEvent::Preparing);
            }
            Some(AgentEvent::ToolCallStart {
                call_id,
                tool_name,
                arguments,
            }) => {
                assert_eq!(call_id, "call_1");
                assert_eq!(tool_name, "mock_tool");
                assert_eq!(arguments, r#"{"value":"ok"}"#);
                lifecycle.push(ToolLifecycleEvent::Start);
            }
            Some(AgentEvent::ToolCallArgsDelta { .. }) => {
                lifecycle.push(ToolLifecycleEvent::ArgsDelta);
            }
            Some(AgentEvent::ToolCallResult { .. }) => {
                lifecycle.push(ToolLifecycleEvent::Result);
            }
            Some(AgentEvent::Done { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }

    assert_eq!(
        lifecycle,
        vec![
            ToolLifecycleEvent::RunStarted,
            ToolLifecycleEvent::Preparing,
            ToolLifecycleEvent::RunUpdated,
            ToolLifecycleEvent::Start,
            ToolLifecycleEvent::Result,
            ToolLifecycleEvent::RunCompleted,
        ],
    );
}

#[tokio::test]
async fn test_executes_complete_tool_with_sparse_responses_output_index() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        final_answer: "final answer after sparse tool",
        first_chunks: vec![StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "sparse-call".to_string(),
                name: Some("mock_tool".to_string()),
                arguments_delta: r#"{"value":"ok"}"#.to_string().into(),
                // A Responses reasoning item can occupy provider output slot 0.
                index: Some(1),
                thought_signature: None,
            }),
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            thinking_delta: None,
        }],
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("deepseek-v4-pro".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "use the sparse-index tool".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("a canonical call id should authorize the sparse-index tool");

    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(final_msg.text_content(), "final answer after sparse tool");
    let mut saw_result = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(
            event,
            AgentEvent::ToolCallResult { ref call_id, .. } if call_id == "sparse-call"
        ) {
            saw_result = true;
        }
    }
    assert!(saw_result, "the sparse-index call must reach dispatch");
}

#[tokio::test]
async fn test_parallel_tool_result_streams_when_each_tool_finishes() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DelayTool {
        name: "fast_tool",
        delay_ms: 0,
    }));
    let slow_release = Arc::new(Notify::new());
    registry.register(Box::new(BlockingTool {
        name: "slow_tool",
        release: Arc::clone(&slow_release),
    }));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ParallelProvider {
        stream_calls: Arc::clone(&stream_calls),
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);
    let run = executor.run(
        vec![],
        vec![ContentPart::Text {
            text: "run parallel tools".to_string(),
        }],
        &db,
        None,
        None,
        tx,
        0,
    );
    tokio::pin!(run);

    let first_result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                biased;
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::ToolCallResult { call_id, content, .. }) => {
                            break (call_id, content);
                        }
                        Some(_) => {}
                        None => panic!("event stream closed before a tool result"),
                    }
                }
                result = &mut run => {
                    panic!("agent run completed before streaming a tool result: {result:?}");
                }
            }
        }
    })
    .await
    .expect("fast tool result should stream before the slow tool finishes");

    assert_eq!(first_result.0, "fast_call");
    assert_eq!(first_result.1, "fast_tool-ok");

    slow_release.notify_one();
    let final_msg = tokio::time::timeout(Duration::from_secs(5), &mut run)
        .await
        .expect("run should finish after releasing the slow tool")
        .expect("run should succeed");
    assert_eq!(final_msg.text_content(), "parallel final answer");
}

#[tokio::test]
async fn complete_tool_envelope_survives_a_clean_stream_close_without_finish_reason() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        final_answer: "final answer after recovered tool boundary",
        first_chunks: vec![StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "clean-close-call".to_string(),
                name: Some("recording_tool".to_string()),
                arguments_delta: r#"{"value":"complete"}"#.to_string().into(),
                index: Some(0),
                thought_signature: None,
            }),
            finish_reason: None,
            usage: None,
            thinking_delta: None,
        }],
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(64);

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "use the recording tool".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("a sealable clean-close tool round should continue");

    assert_eq!(
        final_message.text_content(),
        "final answer after recovered tool boundary"
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_non_concurrency_safe_tool_creates_execution_barrier() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SerialDelayTool {
        name: "serial_tool",
        delay_ms: 300,
    }));
    registry.register(Box::new(DelayTool {
        name: "fast_tool",
        delay_ms: 0,
    }));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        final_answer: "serial final answer",
        first_chunks: vec![
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "serial_call".to_string(),
                    name: Some("serial_tool".to_string()),
                    arguments_delta: r#"{"value":"slow"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: None,
                usage: None,
                thinking_delta: None,
            },
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "fast_call".to_string(),
                    name: Some("fast_tool".to_string()),
                    arguments_delta: r#"{"value":"fast"}"#.to_string().into(),
                    index: Some(1),
                    thought_signature: None,
                }),
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        ],
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);
    let run = executor.run(
        vec![],
        vec![ContentPart::Text {
            text: "run serial then fast".to_string(),
        }],
        &db,
        None,
        None,
        tx,
        0,
    );
    tokio::pin!(run);

    // The full core suite runs thousands of tests in parallel; keep this a
    // deadlock guard without turning host scheduling pressure into a failure.
    let first_result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                biased;
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::ToolCallResult { call_id, content, .. }) => {
                            break (call_id, content);
                        }
                        Some(_) => {}
                        None => panic!("event stream closed before a tool result"),
                    }
                }
                result = &mut run => {
                    panic!("agent run completed before streaming a tool result: {result:?}");
                }
            }
        }
    })
    .await
    .expect("serial tool should finish within the test timeout");

    assert_eq!(first_result.0, "serial_call");
    assert_eq!(first_result.1, "serial_tool-ok");

    let final_msg = run.await.expect("run should succeed");
    assert_eq!(final_msg.text_content(), "serial final answer");
}

#[tokio::test]
async fn request_user_input_defers_every_later_tool_call() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        crate::tools::request_user_input_tool::RequestUserInputTool,
    ));
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let provider = ScriptedProvider {
        stream_calls: Arc::new(AtomicUsize::new(0)),
        final_answer: "must not continue",
        first_chunks: vec![
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "question-call".to_string(),
                    name: Some("request_user_input".to_string()),
                    arguments_delta: serde_json::json!({
                        "questions": [{
                            "id": "scope",
                            "header": "Scope",
                            "question": "Which scope should be changed?",
                            "type": "short"
                        }]
                    })
                    .to_string()
                    .into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: None,
                usage: None,
                thinking_delta: None,
            },
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "later-write".to_string(),
                    name: Some("recording_tool".to_string()),
                    arguments_delta: r#"{"value":"unsafe without answer"}"#.to_string().into(),
                    index: Some(1),
                    thought_signature: None,
                }),
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        ],
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().unwrap();
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "mock".into(),
            model: "mock-model".into(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Change the requested scope.".into(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 5,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .unwrap();
    db.create_agent_task_run(
        &conversation.id,
        &turn.id,
        &user_message.id,
        "Change the requested scope",
        Some("mock"),
        Some("mock-model"),
    )
    .unwrap();
    let (tx, mut rx) = mpsc::channel(64);

    let result = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: user_message.content,
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await;

    assert!(matches!(result, Err(CoreError::AwaitingUserInput { .. })));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let mut deferred = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        if let AgentEvent::ToolCallResult {
            call_id, artifacts, ..
        } = event
        {
            if call_id == "later-write" {
                deferred = artifacts.as_ref().is_some_and(|artifact| {
                    artifact.get("kind").and_then(serde_json::Value::as_str) == Some("toolDeferred")
                });
            }
        }
    }
    assert!(deferred, "later tool should be closed as deferred");
}

#[tokio::test]
async fn active_goal_continues_until_the_model_explicitly_completes_it() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        crate::tools::conversation_goal_tool::UpdateGoalTool,
    ));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(GoalLifecycleProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 5,
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "openai".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    db.set_conversation_goal(&conversation.id, "Finish and verify the task")
        .unwrap();
    let (tx, _rx) = mpsc::channel(64);

    let result = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Begin the goal".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("goal run should finish");

    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert!(result.text_content().contains("complete and verified"));
    assert_eq!(
        db.get_conversation_goal(&conversation.id)
            .unwrap()
            .unwrap()
            .status,
        crate::conversation::ConversationGoalStatus::Complete
    );
}

#[test]
fn test_resource_keys_allow_independent_writes_to_share_batch() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ResourceLockedTool));
    let offered = HashSet::from(["locked_write".to_string()]);
    let registered = registry.tool_names().into_iter().collect();
    let policy = ToolSchedulerPolicy::new(None, None, false, offered, registered);
    let calls = vec![
        test_tool_call("a", "locked_write", serde_json::json!({ "path": "a.txt" })),
        test_tool_call("b", "locked_write", serde_json::json!({ "path": "b.txt" })),
        test_tool_call("c", "locked_write", serde_json::json!({ "path": "a.txt" })),
    ];

    let batches = tool_call_execution_batches(&registry, &policy, &calls);

    assert_eq!(batches, vec![vec![0, 1], vec![2]]);
}

#[test]
fn test_unkeyed_exclusive_tool_remains_serial_barrier() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ResourceLockedTool));
    let offered = HashSet::from(["locked_write".to_string()]);
    let registered = registry.tool_names().into_iter().collect();
    let policy = ToolSchedulerPolicy::new(None, None, false, offered, registered);
    let calls = vec![
        test_tool_call("a", "locked_write", serde_json::json!({})),
        test_tool_call("b", "locked_write", serde_json::json!({ "path": "b.txt" })),
        test_tool_call("c", "locked_write", serde_json::json!({})),
    ];

    let batches = tool_call_execution_batches(&registry, &policy, &calls);

    assert_eq!(batches, vec![vec![0], vec![1], vec![2]]);
}

#[test]
fn test_wait_for_previous_forces_new_execution_batch() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DelayTool {
        name: "fast_tool",
        delay_ms: 0,
    }));
    let offered = HashSet::from(["fast_tool".to_string()]);
    let registered = registry.tool_names().into_iter().collect();
    let policy = ToolSchedulerPolicy::new(None, None, false, offered, registered);
    let calls = vec![
        test_tool_call("a", "fast_tool", serde_json::json!({ "value": "a" })),
        test_tool_call(
            "b",
            "fast_tool",
            serde_json::json!({ "value": "b", "wait_for_previous": true }),
        ),
        test_tool_call("c", "fast_tool", serde_json::json!({ "value": "c" })),
    ];

    let batches = tool_call_execution_batches(&registry, &policy, &calls);

    assert_eq!(batches, vec![vec![0], vec![1, 2]]);
}

#[tokio::test]
async fn test_cancellable_tool_run_completes_as_cancelled() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DelayTool {
        name: "slow_tool",
        delay_ms: 2_000,
    }));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        final_answer: "should not need final answer",
        first_chunks: vec![StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "slow_call".to_string(),
                name: Some("slow_tool".to_string()),
                arguments_delta: r#"{"value":"slow"}"#.to_string().into(),
                index: Some(0),
                thought_signature: None,
            }),
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
    };
    let cancel_token = CancellationToken::new();
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    )
    .with_cancel_token(cancel_token.clone());

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);
    let run = executor.run(
        vec![],
        vec![ContentPart::Text {
            text: "run cancellable tool".to_string(),
        }],
        &db,
        None,
        None,
        tx,
        0,
    );
    tokio::pin!(run);

    let mut run_completed = false;
    let mut saw_cancelled_run = false;
    tokio::time::timeout(Duration::from_millis(700), async {
            loop {
                tokio::select! {
                    maybe_event = rx.recv() => {
                        match maybe_event {
                            Some(AgentEvent::ToolCallStart { call_id, .. }) if call_id == "slow_call" => {
                                cancel_token.cancel();
                            }
                            Some(AgentEvent::ToolRunCompleted { run }) if run.call_id == "slow_call" => {
                                assert_eq!(run.status, ToolRunStatus::Cancelled);
                                saw_cancelled_run = true;
                                break;
                            }
                            Some(_) => {}
                            None => panic!("event stream closed before cancellation completion"),
                        }
                    }
                    result = &mut run => {
                        let final_msg = result.expect("cancelled run should finalize gracefully");
                        assert!(final_msg.text_content().contains("cancelled"));
                        run_completed = true;
                        break;
                    }
                }
            }
        })
        .await
        .expect("cancellable tool should report cancelled promptly");

    while !saw_cancelled_run {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Some(AgentEvent::ToolRunCompleted { run })) if run.call_id == "slow_call" => {
                assert_eq!(run.status, ToolRunStatus::Cancelled);
                saw_cancelled_run = true;
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    assert!(saw_cancelled_run);

    if !run_completed {
        let final_msg = run.await.expect("cancelled run should finalize gracefully");
        assert!(final_msg.text_content().contains("cancelled"));
    }
}

#[tokio::test]
async fn test_ui_preview_tool_streams_preparing_arguments_without_legacy_delta() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DelayTool {
        name: "generate_image",
        delay_ms: 0,
    }));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        final_answer: "preview final answer",
        first_chunks: vec![
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "preview_call".to_string(),
                    name: Some("generate_image".to_string()),
                    arguments_delta: r#"{"prompt":"hel"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: None,
                usage: None,
                thinking_delta: None,
            },
            StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "preview_call".to_string(),
                    name: Some("generate_image".to_string()),
                    arguments_delta: r#"lo"}"#.to_string().into(),
                    index: Some(0),
                    thought_signature: None,
                }),
                finish_reason: Some(crate::llm::FinishReason::Stop),
                usage: None,
                thinking_delta: None,
            },
        ],
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);
    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "preview image tool".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("run should succeed");
    assert_eq!(final_msg.text_content(), "preview final answer");

    let mut saw_preview_args = false;
    let mut saw_legacy_delta = false;
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::ToolRunStarted { run }) | Some(AgentEvent::ToolRunUpdated { run })
                if run.call_id == "preview_call" && run.status == ToolRunStatus::Preparing =>
            {
                if run
                    .arguments
                    .as_deref()
                    .is_some_and(|arguments| arguments.contains("hel"))
                {
                    saw_preview_args = true;
                }
            }
            Some(AgentEvent::ToolCallArgsDelta { .. }) => {
                saw_legacy_delta = true;
            }
            Some(AgentEvent::Done { .. }) | None => break,
            Some(_) => {}
        }
    }

    assert!(saw_preview_args);
    assert!(!saw_legacy_delta);
}

#[tokio::test]
async fn test_run_persists_typed_task_plan_on_task_run() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = MockProvider {
        stream_calls: Arc::clone(&stream_calls),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Say hello in one sentence.".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 6,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let task_run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user_msg.id,
            "Say hello in one sentence.",
            Some("mock"),
            Some("mock-model"),
        )
        .unwrap();

    let (tx, mut rx) = mpsc::channel(32);
    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_msg.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect("run should succeed");
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        if matches!(event, Some(AgentEvent::Done { .. }) | None) {
            break;
        }
    }

    assert_eq!(final_msg.text_content(), "final answer");
    let updated = db.get_agent_task_run(&task_run.id).unwrap();
    let plan = updated.plan.expect("task run should store typed plan");
    assert_eq!(plan["routeKind"], "DirectResponse");
    assert_eq!(plan["evidencePolicy"]["mode"], "notRequired");
    let artifacts = updated
        .artifacts
        .expect("task run should store verification artifacts");
    assert_eq!(artifacts["verification"]["kind"], "verification");

    let events = db.get_agent_task_run_events(&task_run.id).unwrap();
    let verification_event = events
        .iter()
        .find(|event| event.event_type == "verification")
        .expect("task run should record a verification timeline event");
    assert_eq!(
        verification_event.payload.as_ref().unwrap()["taskTimeline"]["kind"],
        "verification"
    );
}

#[tokio::test]
async fn test_exact_prefix_runtime_tail_is_persisted_for_next_turn_replay() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = CapturingScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        requests: Arc::clone(&requests),
        first_chunks: vec![StreamChunk {
            delta: "first answer".to_string(),
            tool_call_delta: None,
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
        final_answer: "unused",
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            volatile_system_sections: vec!["## Current Turn Time\nLocal time: 12:00:00".to_string()],
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "first question".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 2,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let (tx, _rx) = mpsc::channel(32);

    executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_msg.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect("run should succeed");

    let persisted = db.get_messages(&conversation.id).expect("messages");
    assert_eq!(persisted[0].role, Role::User);
    assert_eq!(persisted[1].role, Role::System);
    assert!(persisted[1].content.contains("Runtime Context"));
    assert!(persisted[1].content.contains("Current Turn Time"));
    assert!(persisted
        .iter()
        .any(|message| message.role == Role::System
            && message.content.contains("Active Routing Plan")));
    assert!(!persisted.iter().any(
        |message| message.role == Role::System && message.content.contains("Active Task Plan")
    ));
    assert!(!persisted.iter().any(|message| {
        message.role == Role::System && message.content.contains("Orchestration Quality Profile")
    }));
    assert_eq!(persisted.last().unwrap().role, Role::Assistant);

    let first_request = requests.lock().unwrap()[0].clone();
    let replay_history = persisted
        .iter()
        .map(|message| {
            let mut replay = Message::text(
                message.role.clone(),
                conversation_message_llm_context_content(message),
            );
            replay.name = message.tool_call_id.clone();
            replay.tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(message.tool_calls.clone())
            };
            replay
        })
        .collect::<Vec<_>>();
    let second_request = context::prepare_messages_with_options(
        "stable system",
        &replay_history,
        &[ContentPart::Text {
            text: "second question".to_string(),
        }],
        "deepseek-chat",
        4096,
        None,
        &[],
        &[],
        &[],
        context::PrepareMessagesOptions {
            include_skill_system_prompt: false,
            volatile_system_sections: &["## Current Turn Time\nLocal time: 12:01:00"],
            append_volatile_system_prompt_to_tail: true,
            ..context::PrepareMessagesOptions::default()
        },
    )
    .into_iter()
    .map(|message| {
        let text = message.text_content();
        (message.role, text)
    })
    .take(first_request.len())
    .collect::<Vec<_>>();

    assert_eq!(first_request, second_request);
}

#[tokio::test]
async fn test_prompt_cache_trace_compares_previous_turn_snapshot_with_new_executor() {
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();

    let first_user = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "first question".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 2,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&first_user).unwrap();
    let first_turn = db
        .create_conversation_turn(&conversation.id, &first_user.id, None)
        .unwrap();
    let first_provider = CapturingScriptedProvider {
        stream_calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
        first_chunks: vec![StreamChunk {
            delta: "first answer".to_string(),
            tool_call_delta: None,
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
        final_answer: "unused",
    };
    let first_executor = AgentExecutor::new(
        Box::new(first_provider),
        ToolRegistry::new(),
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            ..AgentConfig::default()
        },
    );
    let (tx, _rx) = mpsc::channel(32);
    first_executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: first_user.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&first_turn.id),
            tx,
            1,
        )
        .await
        .expect("first turn should succeed");

    let first_trace = db
        .get_conversation_turn(&first_turn.id)
        .unwrap()
        .trace
        .expect("first turn trace");
    assert!(first_trace["items"].as_array().unwrap().iter().any(|item| {
        item.get("kind").and_then(serde_json::Value::as_str) == Some("promptCache")
    }));

    let history = db
        .get_messages(&conversation.id)
        .unwrap()
        .into_iter()
        .map(|message| {
            let mut replay = Message::text(
                message.role.clone(),
                conversation_message_llm_context_content(&message),
            );
            replay.name = message.tool_call_id.clone();
            replay.tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(message.tool_calls.clone())
            };
            replay
        })
        .collect::<Vec<_>>();
    let second_user = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "second question".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 2,
        created_at: String::new(),
        sort_order: 100,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&second_user).unwrap();
    let second_turn = db
        .create_conversation_turn(&conversation.id, &second_user.id, None)
        .unwrap();
    let second_provider = CapturingScriptedProvider {
        stream_calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
        first_chunks: vec![StreamChunk {
            delta: "second answer".to_string(),
            tool_call_delta: None,
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
        final_answer: "unused",
    };
    let second_executor = AgentExecutor::new(
        Box::new(second_provider),
        ToolRegistry::new(),
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            ..AgentConfig::default()
        },
    );
    let (tx, _rx) = mpsc::channel(32);
    second_executor
        .run(
            history,
            vec![ContentPart::Text {
                text: second_user.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&second_turn.id),
            tx,
            101,
        )
        .await
        .expect("second turn should succeed");

    let second_trace = db
        .get_conversation_turn(&second_turn.id)
        .unwrap()
        .trace
        .expect("second turn trace");
    let first_prompt_cache = second_trace["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item.get("kind").and_then(serde_json::Value::as_str) == Some("promptCache"))
        .expect("second turn should persist prompt-cache observation");
    let observation = first_prompt_cache
        .get("observation")
        .expect("prompt-cache observation");
    assert_eq!(observation["requestKind"], "mainAgentStep");
    assert!(observation["snapshot"]["messageFingerprints"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item.get("reasoningHash").is_some())));
    assert_eq!(observation["fastCacheSettleRisk"], false);
    assert!(observation.get("modelStepIntervalMs").is_none());
    assert_eq!(
        observation["previousSnapshotSource"]["kind"],
        "previousConversationTurn"
    );
    assert_eq!(
        observation["previousSnapshotSource"]["turnId"],
        first_turn.id
    );
    assert_eq!(observation["sampleKind"], "warmAppend");
    assert_eq!(observation["prefixChanged"], false);
    assert_eq!(
        observation["changes"].as_array().map(Vec::len),
        Some(0),
        "append-only conversation growth must not be reported as a prefix rewrite"
    );
    assert!(observation["commonPrefixMessageCount"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(observation["estimatedReusablePrefixTokens"]
        .as_u64()
        .is_some_and(|tokens| tokens > 0));
}

#[tokio::test]
async fn test_tool_result_replay_matches_current_llm_context() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(LargeReadFileTool));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = CapturingScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        requests: Arc::clone(&requests),
        first_chunks: vec![StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "read_call".to_string(),
                name: Some("read_file".to_string()),
                arguments_delta: r#"{"path":"large.txt"}"#.to_string().into(),
                index: Some(0),
                thought_signature: None,
            }),
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
        final_answer: "done",
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "read a large file".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let (tx, _rx) = mpsc::channel(32);

    executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_msg.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect("run should succeed");

    let request_log = requests.lock().unwrap().clone();
    assert_eq!(request_log.len(), 2);
    let current_context_tool_result = request_log[1]
        .iter()
        .find(|(role, _)| *role == Role::Tool)
        .map(|(_, text)| text.clone())
        .expect("second request should include tool result");
    let persisted_tool = db
        .get_messages(&conversation.id)
        .unwrap()
        .into_iter()
        .find(|message| message.role == Role::Tool)
        .expect("tool message should be persisted");

    assert_eq!(persisted_tool.content, current_context_tool_result);
    assert!(persisted_tool.content.len() < large_read_file_content().len());
}

#[tokio::test]
async fn test_exact_prefix_tool_loop_system_state_is_persisted_for_replay() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = CapturingScriptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        requests: Arc::clone(&requests),
        first_chunks: vec![StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: "mock_call".to_string(),
                name: Some("mock_tool".to_string()),
                arguments_delta: r#"{"value":"ok"}"#.to_string().into(),
                index: Some(0),
                thought_signature: None,
            }),
            finish_reason: Some(crate::llm::FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        }],
        final_answer: "final answer",
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            volatile_system_sections: vec!["## Current Turn Time\nLocal time: 12:00:00".to_string()],
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "use a tool".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 3,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let (tx, _rx) = mpsc::channel(32);

    executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_msg.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect("run should succeed");

    let request_log = requests.lock().unwrap().clone();
    assert_eq!(request_log.len(), 2);
    assert!(request_log[1]
        .iter()
        .any(|(role, text)| *role == Role::System && text.contains("Long Task Control State")));

    let persisted = db.get_messages(&conversation.id).expect("messages");
    assert!(persisted.iter().any(|message| message.role == Role::System
        && message.content.contains("Long Task Control State")));
    let replay_history = persisted
        .iter()
        .map(|message| {
            let mut replay = Message::text(
                message.role.clone(),
                conversation_message_llm_context_content(message),
            );
            replay.name = message.tool_call_id.clone();
            replay.tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(message.tool_calls.clone())
            };
            replay
        })
        .collect::<Vec<_>>();
    let second_turn = context::prepare_messages_with_options(
        "stable system",
        &replay_history,
        &[ContentPart::Text {
            text: "next question".to_string(),
        }],
        "deepseek-chat",
        4096,
        None,
        &[],
        &[],
        &[],
        context::PrepareMessagesOptions {
            include_skill_system_prompt: false,
            volatile_system_sections: &["## Current Turn Time\nLocal time: 12:01:00"],
            append_volatile_system_prompt_to_tail: true,
            ..context::PrepareMessagesOptions::default()
        },
    )
    .into_iter()
    .map(|message| {
        let text = message.text_content();
        (message.role, text)
    })
    .take(request_log[1].len())
    .collect::<Vec<_>>();

    assert_eq!(request_log[1], second_turn);
}

#[tokio::test]
async fn answer_only_synthesis_steering_gets_a_replacement_sample() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_steering_on_replacement = Arc::new(Mutex::new(false));
    let (steering_tx, steering_rx) = mpsc::unbounded_channel();
    let executor = AgentExecutor::new(
        Box::new(PostSynthesisSteeringProvider {
            stream_calls: Arc::clone(&stream_calls),
            steering_tx,
            saw_steering_on_replacement: Arc::clone(&saw_steering_on_replacement),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 0,
            ..AgentConfig::default()
        },
    )
    .with_steering_receiver(steering_rx);
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Produce one answer-only synthesis.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("post-synthesis steering should replace the obsolete draft");

    assert_eq!(
        final_message.text_content(),
        "replacement answer after steering"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert!(
        *saw_steering_on_replacement.lock().unwrap(),
        "the replacement sample must include the late steering message"
    );
}

#[tokio::test]
async fn provider_pause_recovery_replays_native_state_after_rejecting_client_draft() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_native_pause_blocks = Arc::new(Mutex::new(false));
    let executor = AgentExecutor::new(
        Box::new(ProviderPauseReplayProvider {
            stream_calls: Arc::clone(&stream_calls),
            saw_native_pause_blocks: Arc::clone(&saw_native_pause_blocks),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("claude-sonnet-5".to_string()),
            provider_type: Some(ProviderType::Anthropic),
            max_iterations: 0,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Resume the provider-hosted search safely.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("pause_turn should resume from provider-native state");

    assert_eq!(
        final_message.text_content(),
        "completed after provider pause"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert!(
        *saw_native_pause_blocks.lock().unwrap(),
        "the recovery request must contain the original server_tool_use block"
    );
}

#[tokio::test]
async fn provider_pause_without_native_state_fails_closed() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(ScriptedProvider {
            stream_calls: Arc::clone(&stream_calls),
            first_chunks: vec![StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: Some(FinishReason::ProviderPause),
                usage: None,
                thinking_delta: None,
            }],
            final_answer: "must not restart the hosted tool",
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("claude-sonnet-5".to_string()),
            provider_type: Some(ProviderType::Anthropic),
            max_iterations: 0,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let error = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Do not duplicate the provider-hosted search.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect_err("missing pause replay state must stop the turn");

    assert!(error.to_string().contains("missing_replay_state"));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn loop_guard_blocked_batch_does_not_consume_the_remaining_tool_round() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingNamedMockTool {
        name: "read_evidence",
        executions: Arc::clone(&executions),
    }));
    registry.register(Box::new(CountingNamedMockTool {
        name: "alternate_action",
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let tools_visible_on_strategy_change = Arc::new(Mutex::new(false));
    let executor = AgentExecutor::new(
        Box::new(LoopGuardBudgetProvider {
            stream_calls: Arc::clone(&stream_calls),
            tools_visible_on_strategy_change: Arc::clone(&tools_visible_on_strategy_change),
        }),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("mock-model".to_string()),
            max_iterations: 3,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(256);

    let final_message = executor
        .run(
            Vec::new(),
            vec![ContentPart::Text {
                text: "Try evidence, then change strategy when instructed.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the alternate strategy should retain the unspent tool round");

    assert_eq!(
        final_message.text_content(),
        "final after alternate strategy"
    );
    assert_eq!(
        executions.load(Ordering::SeqCst),
        3,
        "two repeated batches and one alternate batch should execute; the blocked third repeat must not"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 5);
    assert!(
        *tools_visible_on_strategy_change.lock().unwrap(),
        "the strategy-change sample must still have tool authority"
    );
}

#[tokio::test]
async fn test_loop_guard_change_strategy_persists_assistant_draft_before_retry() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (steering_tx, steering_rx) = mpsc::unbounded_channel();
    let provider = LoopGuardSteeringProvider {
        stream_calls: Arc::clone(&stream_calls),
        requests: Arc::clone(&requests),
        steering_tx,
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            max_iterations: 5,
            ..AgentConfig::default()
        },
    )
    .with_steering_receiver(steering_rx);

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "start".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 1,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let (tx, _rx) = mpsc::channel(64);

    executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_msg.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect("run should succeed");

    let request_log = requests.lock().unwrap().clone();
    assert_eq!(request_log.len(), 4);
    assert!(request_log[3]
        .iter()
        .any(|(role, text)| *role == Role::System && text.contains("Loop Guard")));

    let persisted = db.get_messages(&conversation.id).expect("messages");
    let repeated_draft_count = persisted
        .iter()
        .filter(|message| {
            message.role == Role::Assistant && message.content == LOOP_GUARD_REPEATED_DRAFT
        })
        .count();
    assert_eq!(repeated_draft_count, 3);
    let third_draft_index = persisted
        .iter()
        .rposition(|message| {
            message.role == Role::Assistant && message.content == LOOP_GUARD_REPEATED_DRAFT
        })
        .expect("third repeated draft should be persisted");
    let loop_guard_index = persisted
        .iter()
        .position(|message| message.role == Role::System && message.content.contains("Loop Guard"))
        .expect("loop guard prompt should be persisted");
    assert!(third_draft_index < loop_guard_index);

    let replay_history = persisted
        .iter()
        .map(|message| {
            let mut replay = Message::text(
                message.role.clone(),
                conversation_message_llm_context_content(message),
            );
            replay.name = message.tool_call_id.clone();
            replay.tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(message.tool_calls.clone())
            };
            replay
        })
        .collect::<Vec<_>>();
    let replay_prefix = context::prepare_messages_with_options(
        "stable system",
        &replay_history,
        &[ContentPart::Text {
            text: "next turn".to_string(),
        }],
        "deepseek-chat",
        4096,
        None,
        &[],
        &[],
        &[],
        context::PrepareMessagesOptions {
            include_skill_system_prompt: false,
            volatile_system_sections: &[],
            append_volatile_system_prompt_to_tail: true,
            ..context::PrepareMessagesOptions::default()
        },
    )
    .into_iter()
    .map(|message| {
        let text = message.text_content();
        (message.role, text)
    })
    .take(request_log[3].len())
    .collect::<Vec<_>>();

    assert_eq!(request_log[3], replay_prefix);
}

#[tokio::test]
async fn test_persists_only_final_iteration_thinking_on_final_assistant() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));

    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ThinkingMockProvider {
        stream_calls: Arc::clone(&stream_calls),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&crate::conversation::CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, _rx) = mpsc::channel(32);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("run should succeed");

    assert_eq!(final_msg.text_content(), "final answer");

    let messages = db
        .get_messages(&conversation.id)
        .expect("messages should load");
    assert_eq!(messages.len(), 3, "assistant(tool), tool, assistant(final)");
    assert_eq!(
        messages[0].thinking.as_deref(),
        Some("first round reasoning")
    );
    assert_eq!(messages[0].tool_calls.len(), 1);
    let provider_turn = crate::conversation::conversation_message_provider_turn(&messages[0])
        .expect("streaming tool turn should persist its provider-native envelope");
    assert!(matches!(
        provider_turn.replay_payload,
        crate::llm::provider_turn::ProviderReplayPayload::DeepSeekReasoningContent(ref value)
            if value == "first round reasoning"
    ));
    let first_reasoning_envelope = messages[0]
        .artifacts
        .as_ref()
        .and_then(|value| value.get(crate::conversation::REASONING_ENVELOPE_ARTIFACT_KEY))
        .expect("tool-call assistant should persist a reasoning envelope");
    assert_eq!(
        first_reasoning_envelope["displayText"].as_str(),
        Some("first round reasoning")
    );
    assert_eq!(
        first_reasoning_envelope["replayPayload"].as_str(),
        Some("first round reasoning")
    );
    assert_eq!(
        first_reasoning_envelope["status"].as_str(),
        Some("captured")
    );
    assert_eq!(messages[1].role, Role::Tool);
    assert_eq!(messages[2].content, "final answer");
    assert_eq!(
        messages[2].thinking.as_deref(),
        Some("second round reasoning")
    );
    let artifacts = messages[2]
        .artifacts
        .as_ref()
        .and_then(|value| value.as_object())
        .expect("final assistant message should persist trace artifacts");
    assert_eq!(
        artifacts[crate::conversation::REASONING_ENVELOPE_ARTIFACT_KEY]["replayPayload"].as_str(),
        Some("second round reasoning")
    );
    assert_eq!(
        artifacts.get("kind").and_then(|v| v.as_str()),
        Some("traceTimeline")
    );
    let items = artifacts
        .get("items")
        .and_then(|v| v.as_array())
        .expect("trace timeline should include items");
    assert!(
        items
            .iter()
            .any(|item| item.get("kind").and_then(|v| v.as_str()) == Some("loop")),
        "trace timeline should include first-class loop events"
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item.get("kind").and_then(|v| v.as_str()) == Some("promptCache"))
            .count(),
        2,
        "trace timeline should include prompt-cache diagnostics for each model step"
    );
    let non_loop_items = items
        .iter()
        .filter(|item| {
            !matches!(
                item.get("kind").and_then(|v| v.as_str()),
                Some("loop") | Some("skillSelection") | Some("promptCache")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(non_loop_items.len(), 6);
    assert_eq!(
        non_loop_items[0].get("kind").and_then(|v| v.as_str()),
        Some("status")
    );
    assert!(non_loop_items[0]["text"]
        .as_str()
        .is_some_and(|content| content.contains("per-request output budget")));
    assert_eq!(
        non_loop_items[1].get("kind").and_then(|v| v.as_str()),
        Some("toolVisibility")
    );
    assert_eq!(
        non_loop_items[1]["decision"]["route"].as_str(),
        Some("DirectResponse")
    );
    assert!(non_loop_items[1]["decision"]["log"]
        .as_array()
        .is_some_and(|log| !log.is_empty()));
    assert_eq!(
        non_loop_items[2].get("kind").and_then(|v| v.as_str()),
        Some("thinking")
    );
    assert_eq!(
        non_loop_items[3].get("kind").and_then(|v| v.as_str()),
        Some("tool")
    );
    assert_eq!(
        non_loop_items[4].get("kind").and_then(|v| v.as_str()),
        Some("thinking")
    );
    assert_eq!(
        non_loop_items[5].get("kind").and_then(|v| v.as_str()),
        Some("status")
    );
}

#[tokio::test]
async fn test_thought_only_length_retries_without_promoting_reasoning_to_reply() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let request_reasoning = Arc::new(Mutex::new(Vec::new()));
    let provider = AnswerOnlyRecoveryProvider {
        stream_calls: Arc::clone(&stream_calls),
        request_reasoning: Arc::clone(&request_reasoning),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("deepseek-reasoner".to_string()),
            reasoning_enabled: Some(true),
            thinking_budget: Some(2_048),
            reasoning_effort: Some(crate::llm::ReasoningEffort::High),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-reasoner".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, mut rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("thought-only length response should recover");

    assert_eq!(final_msg.text_content(), "recovered final answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *request_reasoning.lock().unwrap(),
        vec![
            (Some(true), Some(2_048), Some("high".to_string())),
            (Some(false), None, None),
        ]
    );

    let messages = db
        .get_messages(&conversation.id)
        .expect("messages should load");
    let assistant_messages = messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_messages.len(),
        1,
        "only the final answer should be persisted"
    );
    assert_eq!(assistant_messages[0].content, "recovered final answer");
    assert!(
        messages
            .iter()
            .all(|message| message.content != "raw internal reasoning that must stay private"),
        "reasoning must never be persisted in an ordinary message content field"
    );

    let mut text_deltas = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::TextDelta { delta } = event {
            text_deltas.push(delta);
        }
    }
    assert_eq!(text_deltas, vec!["recovered final answer"]);
}

#[tokio::test]
async fn test_answer_only_recovery_survives_tool_rounds_until_final_answer() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let request_reasoning = Arc::new(Mutex::new(Vec::new()));
    let provider = ToolingAnswerRecoveryProvider {
        stream_calls: Arc::clone(&stream_calls),
        request_reasoning: Arc::clone(&request_reasoning),
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("reasoning-model".to_string()),
            reasoning_enabled: Some(true),
            thinking_budget: Some(2_048),
            reasoning_effort: Some(crate::llm::ReasoningEffort::High),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "finish a long tool-using task".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("answer-only recovery should remain active through tool use");

    assert_eq!(
        final_msg.text_content(),
        "recovered final answer after tool use"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        *request_reasoning.lock().unwrap(),
        vec![Some(true), Some(false), Some(false)],
        "the recovery phase must keep reasoning disabled until a visible answer terminates it"
    );

    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::TextDelta { delta } = event {
            assert_ne!(delta, "initial reasoning filled the response budget");
            assert_ne!(delta, "reasoning was incorrectly re-enabled");
        }
    }
}

#[tokio::test]
async fn test_length_truncated_visible_answer_continues_without_spending_tool_iteration() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let request_reasoning = Arc::new(Mutex::new(Vec::new()));
    let provider = LengthContinuationProvider {
        stream_calls: Arc::clone(&stream_calls),
        request_reasoning: Arc::clone(&request_reasoning),
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("reasoning-model".to_string()),
            reasoning_enabled: Some(true),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "reasoning-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, mut rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "write an answer longer than one provider response".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("length truncation should transparently continue");

    assert_eq!(final_msg.text_content(), "first half, second half");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *request_reasoning.lock().unwrap(),
        vec![Some(true), Some(false)]
    );
    let messages = db
        .get_messages(&conversation.id)
        .expect("messages should load");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "first half, second half");

    let mut streamed = String::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::TextDelta { delta } = event {
            streamed.push_str(&delta);
        }
    }
    assert_eq!(streamed, "first half, second half");
}

#[tokio::test]
async fn visible_progress_can_continue_past_the_former_global_two_sample_cap() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(MultiLengthContinuationProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("reasoning-model".to_string()),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "produce a response spanning several provider samples".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("visible progress should not hit a hidden continuation count");

    assert_eq!(final_msg.text_content(), "one two three four");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn context_limit_terminal_compacts_unknown_provider_history_before_retry() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_compacted_retry = Arc::new(Mutex::new(false));
    let executor = AgentExecutor::new(
        Box::new(ContextLimitTerminalProvider {
            stream_calls: Arc::clone(&stream_calls),
            saw_compacted_retry: Arc::clone(&saw_compacted_retry),
            draft_tool_on_first_sample: false,
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("private-model".to_string()),
            context_window_resolution: Some(crate::conversation::memory::ResolvedContextWindow {
                capacity_tokens: None,
                authority: crate::conversation::memory::ContextWindowAuthority::ProviderManaged,
            }),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(128);
    let final_msg = executor
        .run(
            provider_context_limit_history(),
            vec![ContentPart::Text {
                text: "continue from the full history".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("a typed context terminal should compact before retrying");

    assert_eq!(
        final_msg.text_content(),
        "final answer after context rollover"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert!(
        *saw_compacted_retry.lock().unwrap(),
        "the retry must contain a committed compaction checkpoint even when capacity is provider-managed"
    );
    let mut saw_auto_compaction = false;
    while let Ok(event) = rx.try_recv() {
        saw_auto_compaction |= matches!(event, AgentEvent::AutoCompacted { .. });
    }
    assert!(
        saw_auto_compaction,
        "the context-limit recovery should expose the actual compaction event"
    );
}

#[tokio::test]
async fn context_limited_tool_draft_is_rejected_then_compacted_before_replan() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_compacted_retry = Arc::new(Mutex::new(false));
    let executor = AgentExecutor::new(
        Box::new(ContextLimitTerminalProvider {
            stream_calls: Arc::clone(&stream_calls),
            saw_compacted_retry: Arc::clone(&saw_compacted_retry),
            draft_tool_on_first_sample: true,
        }),
        registry,
        AgentConfig {
            model: Some("private-model".to_string()),
            max_iterations: 1,
            context_window_resolution: Some(crate::conversation::memory::ResolvedContextWindow {
                capacity_tokens: None,
                authority: crate::conversation::memory::ContextWindowAuthority::ProviderManaged,
            }),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            provider_context_limit_history(),
            vec![ContentPart::Text {
                text: "use tools only after a safe context rollover".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the discarded draft should replan from compacted committed history");

    assert_eq!(
        final_msg.text_content(),
        "final answer after context rollover"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "a context-limited tool draft must never execute"
    );
    assert!(
        *saw_compacted_retry.lock().unwrap(),
        "the safe replan must follow a real context compaction"
    );
}

#[tokio::test]
async fn test_length_truncated_tool_call_replans_without_spending_tool_iteration() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_safe_replan_context = Arc::new(Mutex::new(false));
    let provider = TruncatedToolCallProvider {
        stream_calls: Arc::clone(&stream_calls),
        saw_safe_replan_context: Arc::clone(&saw_safe_replan_context),
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "use a tool safely".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("truncated tool call should be rejected and replanned");

    assert_eq!(final_msg.text_content(), "final answer after re-planning");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "a length-truncated tool call must never execute"
    );
    assert!(
        *saw_safe_replan_context.lock().unwrap(),
        "the next model step must receive a safe chunked-write replan without a fabricated tool unit"
    );
}

#[tokio::test]
async fn truncated_draft_then_one_committed_tool_round_still_gets_final_answer() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(TruncatedThenCommittedToolProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 1,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "write safely after a truncated draft".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("draft recovery, one tool round, and final synthesis should succeed");

    assert_eq!(
        final_msg.text_content(),
        "final after one verified tool round"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "only the committed call may execute"
    );
    let trace = db
        .get_agent_traces("")
        .expect("trace query")
        .pop()
        .expect("completed agent trace");
    assert_eq!(
        trace.total_iterations, 2,
        "a rejected provider sample must not inflate semantic agent iterations"
    );
    let tool_step = trace
        .steps
        .iter()
        .find(|step| step.tool_name.as_deref() == Some("recording_tool"))
        .expect("tool trace step");
    assert_eq!(
        tool_step.iteration, 0,
        "the first verified tool round remains logical round zero after recovery samples"
    );
}

#[tokio::test]
async fn answer_only_budget_rejects_provider_tool_calls_at_the_dispatch_boundary() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_tools_suppressed = Arc::new(Mutex::new(Vec::new()));
    let executor = AgentExecutor::new(
        Box::new(AnswerOnlyToolViolationProvider {
            stream_calls: Arc::clone(&stream_calls),
            saw_tools_suppressed: Arc::clone(&saw_tools_suppressed),
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            max_iterations: 0,
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "answer without tools".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("one protocol repair should recover an answer-only sample");

    assert_eq!(
        final_msg.text_content(),
        "answer after respecting the synthesis boundary"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(*saw_tools_suppressed.lock().unwrap(), vec![true, true]);
}

#[tokio::test]
async fn malformed_tool_call_is_quarantined_before_replan_and_persistence() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let saw_safe_replan_context = Arc::new(Mutex::new(false));
    let executor = AgentExecutor::new(
        Box::new(MalformedToolCallProvider {
            stream_calls: Arc::clone(&stream_calls),
            saw_safe_replan_context: Arc::clone(&saw_safe_replan_context),
        }),
        registry,
        AgentConfig {
            model: Some("deepseek-v4-pro".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "open_ai".to_string(),
            model: "deepseek-v4-pro".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, mut rx) = mpsc::channel(128);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "use a tool safely".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("malformed tool call should be quarantined and replanned");

    assert_eq!(
        final_msg.text_content(),
        "final answer after safe re-planning"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(*saw_safe_replan_context.lock().unwrap());
    let persisted = db
        .get_messages(&conversation.id)
        .expect("persisted messages");
    assert!(persisted.iter().all(|message| {
        message
            .tool_calls
            .iter()
            .all(|call| crate::llm::message_validation::is_complete_tool_call(call))
    }));
    assert!(persisted
        .iter()
        .all(|message| !message.content.contains("I will inspect the repository")));

    let mut visible_text = String::new();
    let mut reset_index = None;
    let mut rejection_status_index = None;
    let mut event_index = 0usize;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TextDelta { delta } => visible_text.push_str(&delta),
            AgentEvent::StreamReset { .. } => {
                reset_index = Some(event_index);
                visible_text.clear();
            }
            AgentEvent::ControllerStatus { code, .. }
                if code == "incomplete_tool_calls_rejected" =>
            {
                rejection_status_index = Some(event_index);
            }
            _ => {}
        }
        event_index += 1;
    }
    assert_eq!(visible_text, "final answer after safe re-planning");
    assert!(
        reset_index
            .is_some_and(|reset| { rejection_status_index.is_some_and(|status| reset < status) }),
        "the rejected sample must reset frontend text before re-plan status is emitted"
    );
}

#[tokio::test]
async fn missing_required_reasoning_safely_restarts_before_tool_execution() {
    let executions = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(MissingRequiredReasoningProvider {
            complete_calls: Arc::clone(&complete_calls),
        }),
        registry,
        AgentConfig {
            model: Some("deepseek-v4".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(128);
    let event_drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Use recording_tool once.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the same route should restart with reasoning disabled");

    event_drain.await.expect("agent event drain");

    assert_eq!(
        final_message.text_content(),
        "final answer after safe restart"
    );
    assert_eq!(complete_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "only the replay-safe restarted sample may execute"
    );
    assert_eq!(
        db.count_provider_turns().unwrap(),
        3,
        "rejected stream, reasoning-disabled restart, and final answer keep distinct sample ids"
    );
}

#[tokio::test]
async fn unknown_replay_route_keeps_current_reasoning_visible_but_never_replays_it() {
    let executions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let request_reasoning = Arc::new(Mutex::new(Vec::new()));
    let rejected_reasoning_seen_in_history = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(UnknownReplayThinkingProvider {
            attempt_tool_call: true,
            stream_calls: Arc::clone(&stream_calls),
            complete_calls: Arc::clone(&complete_calls),
            request_reasoning: Arc::clone(&request_reasoning),
            rejected_reasoning_seen_in_history: Arc::clone(&rejected_reasoning_seen_in_history),
        }),
        registry,
        AgentConfig {
            model: Some("custom-reasoner".to_string()),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(crate::llm::ReasoningEffort::High),
            thinking_budget: Some(2_048),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Use recording_tool safely.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("the unknown route should restart safely after displaying reasoning");

    assert_eq!(
        final_message.text_content(),
        "final answer after verified restart"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "only the reasoning-disabled replacement tool call may execute"
    );
    assert_eq!(
        *request_reasoning.lock().unwrap(),
        vec![Some(true), Some(false), Some(false)],
        "reasoning stays enabled for the visible sample, then remains disabled through the tool loop"
    );
    assert!(
        rejected_reasoning_seen_in_history
            .lock()
            .unwrap()
            .iter()
            .all(|seen| !seen),
        "reasoning from the rejected sample must never enter provider history"
    );

    let mut visible_reasoning_index = None;
    let mut reset_index = None;
    let mut event_index = 0usize;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::Thinking { content }
                if content == "visible reasoning from an unverified route" =>
            {
                visible_reasoning_index = Some(event_index);
            }
            AgentEvent::StreamReset { .. } => reset_index = Some(event_index),
            _ => {}
        }
        event_index += 1;
    }
    assert!(
        visible_reasoning_index
            .is_some_and(|thinking| { reset_index.is_some_and(|reset| thinking < reset) }),
        "the current sample's reasoning must be emitted before the unsafe tool turn is reset"
    );
}

#[tokio::test]
async fn unknown_replay_route_with_available_tools_keeps_reasoning_visible_for_final_answer() {
    let executions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let request_reasoning = Arc::new(Mutex::new(Vec::new()));
    let rejected_reasoning_seen_in_history = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(UnknownReplayThinkingProvider {
            attempt_tool_call: false,
            stream_calls: Arc::clone(&stream_calls),
            complete_calls: Arc::clone(&complete_calls),
            request_reasoning: Arc::clone(&request_reasoning),
            rejected_reasoning_seen_in_history,
        }),
        registry,
        AgentConfig {
            model: Some("custom-reasoner".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Answer normally; tools remain available.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("unknown replay must not disable current-turn reasoning");

    assert_eq!(
        final_message.text_content(),
        "final answer with visible reasoning"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(*request_reasoning.lock().unwrap(), vec![Some(true)]);
    let mut visible_reasoning = false;
    while let Ok(event) = rx.try_recv() {
        visible_reasoning |= matches!(
            event,
            AgentEvent::Thinking { content }
                if content == "visible reasoning from an unverified route"
        );
    }
    assert!(visible_reasoning);
}

#[tokio::test]
async fn provider_turn_commit_failure_blocks_all_tool_side_effects() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(MissingRequiredReasoningProvider {
            complete_calls: Arc::new(AtomicUsize::new(0)),
        }),
        registry,
        AgentConfig {
            model: Some("deepseek-v4".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    db.conn()
        .execute("DROP TABLE provider_turn_envelopes", [])
        .expect("fault injection should remove the pre-dispatch ledger");
    let (tx, _rx) = mpsc::channel(128);

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Use recording_tool once.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect_err("tool dispatch must fail when the provider turn cannot commit");

    assert!(error.to_string().contains("provider_turn_envelopes"));
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "database failure before commit must permit zero tool executions"
    );
}

#[tokio::test]
async fn tool_result_commit_failure_terminates_before_the_next_model_request() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(MockProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Use mock_tool once.".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).expect("user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .expect("conversation turn");
    db.conn()
        .execute_batch(
            "CREATE TRIGGER fail_tool_result_message
             BEFORE INSERT ON messages
             WHEN NEW.role = 'tool'
             BEGIN
               SELECT RAISE(FAIL, 'injected tool-result persistence failure');
             END;",
        )
        .expect("fault injection trigger");
    let (tx, _rx) = mpsc::channel(128);

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_message.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect_err("a tool result that cannot commit must terminate the turn");

    assert!(
        error
            .to_string()
            .contains("injected tool-result persistence failure"),
        "{error}"
    );
    assert_eq!(
        stream_calls.load(Ordering::SeqCst),
        1,
        "the unpersisted tool result must never reach a follow-up model request"
    );
    let persisted_tool_results = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE role = 'tool'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count tool results");
    assert_eq!(persisted_tool_results, 0);
    let finalized_turn = db
        .get_conversation_turn(&turn.id)
        .expect("finalized conversation turn");
    assert_eq!(finalized_turn.status, "error");
    assert!(
        finalized_turn.trace.is_some(),
        "error trace must be durable"
    );
}

#[tokio::test]
async fn output_validation_uses_the_route_policy_not_the_history_policy() {
    let executions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(RouteAwareReplayPolicyProvider {
            stream_calls: Arc::clone(&stream_calls),
            complete_calls: Arc::clone(&complete_calls),
        }),
        registry,
        AgentConfig {
            model: Some("primary-model".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Use recording_tool once.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("a permissive selected route may execute its tool without reasoning");

    assert_eq!(final_message.text_content(), "final answer");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn automatic_fallback_binds_tool_turn_and_history_to_the_accepted_route() {
    let primary_stream_calls = Arc::new(AtomicUsize::new(0));
    let fallback_stream_calls = Arc::new(AtomicUsize::new(0));
    let fallback_requests = Arc::new(Mutex::new(Vec::new()));
    let selections = Arc::new(Mutex::new(Vec::new()));
    let selections_for_callback = Arc::clone(&selections);
    let provider = crate::llm::fallback::AutomaticFallbackProvider::new(
        0,
        Box::new(RecoverablePrimaryRouteProvider {
            stream_calls: Arc::clone(&primary_stream_calls),
        }),
        "primary-model".to_string(),
        ProviderType::OpenAi,
        vec![crate::llm::fallback::AutomaticFallbackCandidate {
            fallback_index: 1,
            provider: Box::new(ToolCallingFallbackRouteProvider {
                stream_calls: Arc::clone(&fallback_stream_calls),
                requests: Arc::clone(&fallback_requests),
            }),
            model: "fallback-model".to_string(),
            provider_type: ProviderType::DeepSeek,
        }],
        Arc::new(move |from, to, reason| {
            selections_for_callback
                .lock()
                .unwrap()
                .push((from, to, reason.to_string()));
            Ok(())
        }),
    )
    .expect("automatic fallback provider");

    let fallback_route = tool_calling_fallback_route("fallback-model");
    let historical_tool_call = ToolCallRequest {
        id: "historical-fallback-call".to_string(),
        name: "recording_tool".to_string(),
        arguments: r#"{"value":"historical"}"#.to_string(),
        thought_signature: None,
    };
    let historical_envelope = crate::llm::provider_turn::ProviderTurnEnvelope::capture(
        "historical-turn-item",
        "historical-sample",
        fallback_route.clone(),
        "",
        Some("historical fallback reasoning"),
        Some("historical fallback reasoning"),
        vec![historical_tool_call.clone()],
        true,
    );
    assert!(
        historical_envelope.authorizes_tool_dispatch(),
        "the route-specific history fixture must carry valid fallback replay state"
    );
    let mut historical_assistant = Message::text(Role::Assistant, "");
    historical_assistant.tool_calls = Some(vec![historical_tool_call]);
    historical_assistant.set_provider_turn(historical_envelope);
    let history = vec![
        Message::text(Role::User, "historical fallback request"),
        historical_assistant,
        Message::text_with_name(
            Role::Tool,
            "historical fallback result",
            "historical-fallback-call",
        ),
        Message::text(Role::Assistant, "historical fallback final answer"),
    ];

    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("primary-model".to_string()),
            provider_type: Some(ProviderType::OpenAi),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "automatic".to_string(),
            model: "primary-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, _rx) = mpsc::channel(128);

    let final_message = executor
        .run(
            history,
            vec![ContentPart::Text {
                text: "Use recording_tool through the accepted route.".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect("the accepted fallback route must own the complete tool loop");

    assert_eq!(final_message.text_content(), "fallback final answer");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(primary_stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *selections.lock().unwrap(),
        vec![(
            0,
            1,
            "primary_invocation_failed_automatic_fallback".to_string()
        )]
    );

    let requests = fallback_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let historical_turn = request
            .iter()
            .find(|message| {
                message.tool_calls.as_ref().is_some_and(|calls| {
                    calls
                        .iter()
                        .any(|call| call.id == "historical-fallback-call")
                })
            })
            .expect("each fallback projection must start from the unprojected typed history");
        assert_eq!(
            historical_turn
                .provider_turn()
                .expect("historical fallback envelope")
                .route,
            fallback_route
        );
        assert!(request.iter().any(|message| {
            message.role == Role::Tool
                && message.name.as_deref() == Some("historical-fallback-call")
        }));
    }

    let accepted_tool_turn = requests[1]
        .iter()
        .rev()
        .find(|message| {
            message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "fallback-call"))
        })
        .expect("the follow-up request must replay the accepted fallback tool turn");
    let accepted_envelope = accepted_tool_turn
        .provider_turn()
        .expect("accepted tool turn must carry route provenance");
    assert_eq!(accepted_envelope.route, fallback_route);
    assert!(matches!(
        accepted_envelope.replay_payload,
        crate::llm::provider_turn::ProviderReplayPayload::DeepSeekReasoningContent(ref value)
            if value == "fallback reasoning state"
    ));
    assert!(accepted_envelope.authorizes_tool_dispatch());
    assert!(requests[1].iter().any(|message| {
        message.role == Role::Tool && message.name.as_deref() == Some("fallback-call")
    }));

    let persisted = db
        .get_messages(&conversation.id)
        .expect("persisted messages");
    let durable_tool_turn = persisted
        .iter()
        .find(|message| {
            message
                .tool_calls
                .iter()
                .any(|call| call.id == "fallback-call")
        })
        .expect("accepted fallback tool turn must be durable");
    let durable_envelope =
        crate::conversation::conversation_message_provider_turn(durable_tool_turn)
            .expect("durable accepted-route envelope");
    assert_eq!(durable_envelope.route, fallback_route);
    assert!(matches!(
        durable_envelope.replay_payload,
        crate::llm::provider_turn::ProviderReplayPayload::DeepSeekReasoningContent(ref value)
            if value == "fallback reasoning state"
    ));
    assert!(durable_envelope.authorizes_tool_dispatch());
}

#[test]
fn explicitly_disabled_reasoning_does_not_require_a_replay_payload() {
    let executor = AgentExecutor::new(
        Box::new(MissingRequiredReasoningProvider {
            complete_calls: Arc::new(AtomicUsize::new(0)),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("deepseek-v4".to_string()),
            reasoning_enabled: Some(false),
            ..AgentConfig::default()
        },
    );

    assert_eq!(
        executor.reasoning_replay_policy_for_request("deepseek-v4", false),
        ReasoningReplayPolicy::NotRequired
    );
    assert_eq!(
        executor.reasoning_replay_policy_for_request("deepseek-v4", true),
        ReasoningReplayPolicy::NotRequired
    );
}

#[tokio::test]
async fn test_repeated_thought_only_stop_fails_without_persisting_a_reply() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ThoughtOnlyProvider {
        stream_calls: Arc::clone(&stream_calls),
        finish_reason: FinishReason::Stop,
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "google".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, mut rx) = mpsc::channel(128);

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect_err("a second thought-only response must fail the turn");

    assert!(error
        .to_string()
        .contains("provider_finished_without_answer"));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert!(db
        .get_messages(&conversation.id)
        .expect("messages should load")
        .is_empty());

    let mut saw_error = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TextDelta { delta } => {
                panic!("reasoning leaked into answer delta: {delta}")
            }
            AgentEvent::Done { message, .. } => {
                panic!(
                    "thought-only response was finalized: {}",
                    message.text_content()
                )
            }
            AgentEvent::Error { message } => {
                saw_error = message.contains("without producing a final answer")
            }
            _ => {}
        }
    }
    assert!(
        saw_error,
        "the user should receive a recoverable terminal error"
    );
}

#[tokio::test]
async fn test_filtered_thought_only_response_fails_without_retrying_or_leaking() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let provider = ThoughtOnlyProvider {
        stream_calls: Arc::clone(&stream_calls),
        finish_reason: FinishReason::ContentFilter,
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "google".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let (tx, mut rx) = mpsc::channel(128);

    executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            Some(&conversation.id),
            None,
            tx,
            0,
        )
        .await
        .expect_err("a filtered response must fail the turn without retrying");

    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert!(db
        .get_messages(&conversation.id)
        .expect("messages should load")
        .is_empty());

    let mut saw_filtered_error = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TextDelta { delta } => {
                panic!("reasoning leaked into answer delta: {delta}")
            }
            AgentEvent::Done { message, .. } => {
                panic!(
                    "filtered response was finalized: {}",
                    message.text_content()
                )
            }
            AgentEvent::Error { message } => {
                saw_filtered_error = message.contains("blocked the response")
            }
            _ => {}
        }
    }
    assert!(
        saw_filtered_error,
        "the user should see a filter-specific error"
    );
}

#[tokio::test]
async fn test_stream_incomplete_before_visible_output_replays_stream() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let provider = FlakyThenSuccessfulStreamProvider {
        stream_calls: Arc::clone(&stream_calls),
        complete_calls: Arc::clone(&complete_calls),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let final_msg = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("run should recover by replaying the stream");

    assert_eq!(final_msg.text_content(), "stream answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);

    let mut saw_reset = false;
    let mut saw_error = false;
    let mut connection_states = Vec::new();
    let mut visible_text = String::new();
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::TextDelta { delta }) => visible_text.push_str(&delta),
            Some(AgentEvent::StreamReset { .. }) => {
                saw_reset = true;
                visible_text.clear();
            }
            Some(AgentEvent::ConnectionState { state }) => {
                connection_states.push(state.state);
            }
            Some(AgentEvent::Error { .. }) => saw_error = true,
            Some(AgentEvent::Done { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }

    assert!(saw_reset, "expected partial stream reset before replay");
    assert!(!saw_error, "stream replay should not surface an error");
    assert_eq!(
        connection_states,
        vec![
            ConnectionStateKind::Reconnecting,
            ConnectionStateKind::Recovered,
        ],
        "a successful replay must close the reconnecting state"
    );
    assert_eq!(visible_text, "stream answer");
}

#[tokio::test]
async fn test_repeated_stream_incomplete_fails_without_non_streaming_fallback() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let provider = RecoveringStreamProvider {
        stream_calls: Arc::clone(&stream_calls),
        complete_calls: Arc::clone(&complete_calls),
    };

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect_err("repeated stream disconnects should fail explicitly");

    assert!(matches!(
        error,
        CoreError::StreamIncomplete(ref message)
            if message.contains("disconnected after 2 replay attempt")
    ));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);

    let mut saw_reset = false;
    let mut saw_error = false;
    let mut visible_text = String::new();
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::TextDelta { delta }) => visible_text.push_str(&delta),
            Some(AgentEvent::StreamReset { .. }) => {
                saw_reset = true;
                visible_text.clear();
            }
            Some(AgentEvent::Error { .. }) => saw_error = true,
            Some(AgentEvent::Done { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }

    assert!(saw_reset, "expected stream resets before explicit failure");
    assert!(
        saw_error,
        "exhausted stream recovery should surface an error"
    );
    assert_eq!(visible_text, "");
}

#[tokio::test]
async fn empty_metadata_chunks_do_not_reset_context_compaction_circuit_breaker() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(EmptyMetadataContextOverflowProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("mock-model".to_string()),
            context_window: Some(1_000_000),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(128);
    let compaction_statuses = Arc::new(AtomicUsize::new(0));
    let drained_compaction_statuses = Arc::clone(&compaction_statuses);
    let event_drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                AgentEvent::Status { ref content, .. }
                    if content.starts_with("Context window overflow detected.")
            ) {
                drained_compaction_statuses.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let history = (0..6)
        .flat_map(|turn| {
            [
                Message::text(Role::User, format!("old user turn {turn}")),
                Message::text(Role::Assistant, format!("old assistant response {turn}")),
            ]
        })
        .collect();

    let error = executor
        .run(
            history,
            vec![ContentPart::Text {
                text: "answer after considering the history".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect_err("the third context overflow must open the circuit breaker");

    event_drain.await.expect("agent event drain");
    assert!(matches!(error, CoreError::ContextOverflow(200, 100)));
    assert_eq!(
        stream_calls.load(Ordering::SeqCst),
        3,
        "two compactions permit exactly three provider attempts"
    );
    assert_eq!(
        compaction_statuses.load(Ordering::SeqCst),
        2,
        "the run must expose exactly the two budgeted compaction attempts"
    );
}

#[tokio::test]
async fn test_stream_incomplete_after_resettable_text_replays_cleanly() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let complete_calls = Arc::new(AtomicUsize::new(0));
    let provider = VisibleThenInterruptedProvider {
        stream_calls: Arc::clone(&stream_calls),
        complete_calls: Arc::clone(&complete_calls),
    };
    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let result = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("resettable partial text should be cleared and replayed");

    assert_eq!(result.text_content(), "complete replayed answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);

    let mut visible_text = String::new();
    let mut saw_reset = false;
    let mut saw_error = false;
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::TextDelta { delta }) => visible_text.push_str(&delta),
            Some(AgentEvent::StreamReset { .. }) => {
                saw_reset = true;
                visible_text.clear();
            }
            Some(AgentEvent::Error { .. }) => saw_error = true,
            Some(_) => {}
            None => break,
        }
    }
    assert_eq!(visible_text, "complete replayed answer");
    assert!(
        saw_reset,
        "the partial draft must be visibly cleared before replay"
    );
    assert!(!saw_error, "a successful replay must not surface an error");
}

#[tokio::test]
async fn interrupted_streamed_tool_arguments_replay_before_single_execution() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(ToolCallThenInterruptedProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);

    let result = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "Write the requested change with recording_tool.".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("interrupted tool assembly should replay before dispatch");

    assert_eq!(result.text_content(), "write recovered and verified");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let mut saw_reset = false;
    let mut saw_error = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            AgentEvent::StreamReset { .. } => saw_reset = true,
            AgentEvent::Error { .. } => saw_error = true,
            _ => {}
        }
    }
    assert!(saw_reset);
    assert!(!saw_error);
}

#[tokio::test]
async fn visible_cancelled_stream_persists_accepted_interrupted_draft_once() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(CancelledStreamProvider {
            stream_calls: Arc::clone(&stream_calls),
            visible_output: true,
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "cancelled-stream-mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "start a cancellable response".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).expect("persist user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .expect("conversation turn");
    let (tx, mut rx) = mpsc::channel(128);
    let error_events = Arc::new(AtomicUsize::new(0));
    let done_events = Arc::new(AtomicUsize::new(0));
    let drained_errors = Arc::clone(&error_events);
    let drained_done = Arc::clone(&done_events);
    let event_drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Error { .. } => {
                    drained_errors.fetch_add(1, Ordering::SeqCst);
                }
                AgentEvent::Done { .. } => {
                    drained_done.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_message.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect_err("provider cancellation must terminate the turn");

    event_drain.await.expect("agent event drain");
    assert!(matches!(
        error,
        CoreError::Cancelled(ref message) if message == "cancelled by user"
    ));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(error_events.load(Ordering::SeqCst), 1);
    assert_eq!(done_events.load(Ordering::SeqCst), 0);
    assert_eq!(
        db.get_conversation_turn(&turn.id)
            .expect("finalized turn")
            .status,
        "error"
    );

    let persisted = db
        .get_messages(&conversation.id)
        .expect("persisted messages");
    let drafts = persisted
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(drafts.len(), 1, "the interrupted draft must persist once");
    let draft = drafts[0];
    assert_eq!(draft.content, "visible answer before cancellation");
    assert_eq!(
        draft.thinking.as_deref(),
        Some("visible reasoning before cancellation")
    );
    assert!(draft.tool_calls.is_empty());
    let envelope = crate::conversation::conversation_message_provider_turn(draft)
        .expect("cancelled visible draft must retain accepted provenance");
    assert!(!envelope.sample_id.is_empty());
    assert_eq!(envelope.route.provider_family, "cancelled-stream-mock");
    assert_eq!(envelope.route.model_id, "mock-model");
    assert_eq!(
        envelope.visible_content,
        "visible answer before cancellation"
    );
    assert_eq!(envelope.capture_status, ReasoningCaptureStatus::Interrupted);
    assert_eq!(db.count_provider_turns().expect("provider turns"), 1);
}

#[tokio::test]
async fn previsible_cancelled_stream_does_not_persist_assistant_draft() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(CancelledStreamProvider {
            stream_calls: Arc::clone(&stream_calls),
            visible_output: false,
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "cancelled-stream-mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "cancel before visible output".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).expect("persist user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .expect("conversation turn");
    let (tx, mut rx) = mpsc::channel(128);
    let event_drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let error = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: user_message.content.clone(),
            }],
            &db,
            Some(&conversation.id),
            Some(&turn.id),
            tx,
            1,
        )
        .await
        .expect_err("provider cancellation must terminate the turn");

    event_drain.await.expect("agent event drain");
    assert!(matches!(error, CoreError::Cancelled(_)));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert!(db
        .get_messages(&conversation.id)
        .expect("persisted messages")
        .iter()
        .all(|message| message.role != Role::Assistant));
    assert_eq!(db.count_provider_turns().expect("provider turns"), 0);
}

#[tokio::test]
async fn user_cancellation_interrupts_pending_stream_open_without_draft() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let invocation_started = Arc::new(Notify::new());
    let cancel_token = CancellationToken::new();
    let executor = AgentExecutor::new(
        Box::new(PendingCancellationProvider {
            stream_calls: Arc::clone(&stream_calls),
            invocation_started: Arc::clone(&invocation_started),
            cancellation_point: PendingCancellationPoint::StreamOpen,
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("mock-model".to_string()),
            ..AgentConfig::default()
        },
    )
    .with_cancel_token(cancel_token.clone());
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "pending-cancellation-mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "cancel while opening the model stream".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 6,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).expect("persist user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .expect("conversation turn");
    let (tx, mut rx) = mpsc::channel(128);
    let error_events = Arc::new(AtomicUsize::new(0));
    let done_events = Arc::new(AtomicUsize::new(0));
    let drained_errors = Arc::clone(&error_events);
    let drained_done = Arc::clone(&done_events);
    let event_drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Error { .. } => {
                    drained_errors.fetch_add(1, Ordering::SeqCst);
                }
                AgentEvent::Done { .. } => {
                    drained_done.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });
    let run_db = db.clone();
    let conversation_id = conversation.id.clone();
    let turn_id = turn.id.clone();
    let user_content = user_message.content.clone();
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text { text: user_content }],
                &run_db,
                Some(&conversation_id),
                Some(&turn_id),
                tx,
                1,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("provider stream-open invocation must begin");
    cancel_token.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("pending stream open must observe cancellation promptly")
        .expect("agent run task")
        .expect_err("user cancellation must terminate the run");

    event_drain.await.expect("agent event drain");
    assert!(matches!(error, CoreError::Cancelled(_)));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(error_events.load(Ordering::SeqCst), 1);
    assert_eq!(done_events.load(Ordering::SeqCst), 0);
    assert!(db
        .get_messages(&conversation.id)
        .expect("persisted messages")
        .iter()
        .all(|message| message.role != Role::Assistant));
    assert_eq!(db.count_provider_turns().expect("provider turns"), 0);
}

#[tokio::test]
async fn user_cancellation_interrupts_pending_stream_read_and_persists_draft() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(AtomicUsize::new(0));
    let invocation_started = Arc::new(Notify::new());
    let cancel_token = CancellationToken::new();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(PendingCancellationProvider {
            stream_calls: Arc::clone(&stream_calls),
            invocation_started: Arc::clone(&invocation_started),
            cancellation_point: PendingCancellationPoint::StreamReadAfterVisible,
        }),
        registry,
        AgentConfig {
            model: Some("mock-model".to_string()),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    )
    .with_cancel_token(cancel_token.clone());
    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "pending-cancellation-mock".to_string(),
            model: "mock-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let user_message = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "cancel while reading the model stream".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 6,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_message).expect("persist user message");
    let turn = db
        .create_conversation_turn(&conversation.id, &user_message.id, None)
        .expect("conversation turn");
    let (tx, mut rx) = mpsc::channel(128);
    let error_events = Arc::new(AtomicUsize::new(0));
    let done_events = Arc::new(AtomicUsize::new(0));
    let drained_errors = Arc::clone(&error_events);
    let drained_done = Arc::clone(&done_events);
    let event_drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Error { .. } => {
                    drained_errors.fetch_add(1, Ordering::SeqCst);
                }
                AgentEvent::Done { .. } => {
                    drained_done.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });
    let run_db = db.clone();
    let conversation_id = conversation.id.clone();
    let turn_id = turn.id.clone();
    let user_content = user_message.content.clone();
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text { text: user_content }],
                &run_db,
                Some(&conversation_id),
                Some(&turn_id),
                tx,
                1,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("provider stream read must become pending after visible output");
    cancel_token.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("pending stream read must observe cancellation promptly")
        .expect("agent run task")
        .expect_err("user cancellation must terminate the run");

    event_drain.await.expect("agent event drain");
    assert!(matches!(error, CoreError::Cancelled(_)));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(error_events.load(Ordering::SeqCst), 1);
    assert_eq!(done_events.load(Ordering::SeqCst), 0);
    let persisted = db
        .get_messages(&conversation.id)
        .expect("persisted messages");
    let drafts = persisted
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(drafts.len(), 1);
    let draft = drafts[0];
    assert_eq!(draft.content, "partial answer before user cancellation");
    assert_eq!(
        draft.thinking.as_deref(),
        Some("partial reasoning before user cancellation")
    );
    assert!(draft.tool_calls.is_empty());
    let envelope = crate::conversation::conversation_message_provider_turn(draft)
        .expect("visible partial cancellation must retain accepted provenance");
    assert!(!envelope.sample_id.is_empty());
    assert_eq!(envelope.route.provider_family, "pending-cancellation-mock");
    assert_eq!(envelope.capture_status, ReasoningCaptureStatus::Interrupted);
    assert_eq!(db.count_provider_turns().expect("provider turns"), 1);
}

#[tokio::test]
async fn test_provider_hosted_tool_is_rendered_without_local_dispatch_or_extra_round() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentExecutor::new(
        Box::new(ProviderHostedToolProvider {
            stream_calls: Arc::clone(&stream_calls),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("deepseek-v4-flash".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(32);

    let final_message = executor
        .run(
            vec![],
            vec![ContentPart::Text {
                text: "search".to_string(),
            }],
            &db,
            None,
            None,
            tx,
            0,
        )
        .await
        .expect("provider-hosted tool turn");
    assert_eq!(final_message.text_content(), "provider answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);

    let mut started = 0;
    let mut completed = 0;
    while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        match event {
            Some(AgentEvent::ToolRunStarted { run }) => {
                assert!(run.provider_executed);
                assert_eq!(run.call_id, "ws-1");
                started += 1;
            }
            Some(AgentEvent::ToolRunCompleted { run }) => {
                assert!(run.provider_executed);
                assert_eq!(run.call_id, "ws-1");
                completed += 1;
            }
            Some(AgentEvent::ToolCallStart { .. }) => {
                panic!("provider-hosted tool must not enter local dispatch")
            }
            Some(_) => {}
            None => break,
        }
    }
    assert_eq!(started, 1);
    assert_eq!(completed, 1);
    assert_eq!(
        db.count_provider_turns().expect("provider turns"),
        1,
        "a hosted-search-only answer must persist its turn-level replay sidecar"
    );
}

#[tokio::test]
async fn test_steering_interrupts_active_stream_and_restarts_with_message() {
    let registry = ToolRegistry::new();
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let request_texts = Arc::new(Mutex::new(Vec::new()));
    let provider = SteeringInterruptProvider {
        stream_calls: Arc::clone(&stream_calls),
        request_texts: Arc::clone(&request_texts),
    };
    let (steering_tx, steering_rx) = mpsc::unbounded_channel();

    let executor = AgentExecutor::new(
        Box::new(provider),
        registry,
        AgentConfig {
            system_prompt: "stable system".to_string(),
            model: Some("deepseek-chat".to_string()),
            provider_type: Some(ProviderType::DeepSeek),
            max_iterations: 3,
            ..AgentConfig::default()
        },
    )
    .with_steering_receiver(steering_rx);

    let db = Database::open_memory().expect("in-memory db");
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "deep_seek".to_string(),
            model: "deepseek-chat".to_string(),
            system_prompt: Some("stable system".to_string()),
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "start broad".to_string(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: 2,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    db.add_message(&user_msg).unwrap();
    let turn = db
        .create_conversation_turn(&conversation.id, &user_msg.id, None)
        .unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let run_db = db.clone();
    let conversation_id = conversation.id.clone();
    let turn_id = turn.id.clone();
    let user_content = user_msg.content.clone();

    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text { text: user_content }],
                &run_db,
                Some(&conversation_id),
                Some(&turn_id),
                tx,
                1,
            )
            .await
    });

    let mut visible_text = String::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(AgentEvent::TextDelta { delta })) => {
                visible_text.push_str(&delta);
                if visible_text.contains("obsolete draft") {
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("agent event channel closed before initial stream delta"),
            Err(_) => panic!("timed out waiting for initial stream delta"),
        }
    }

    steering_tx
        .send(AgentSteeringMessage::text("focus on edge cases instead"))
        .expect("steering send");

    let mut saw_reset = false;
    let mut saw_error = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Some(AgentEvent::StreamReset { .. })) => {
                saw_reset = true;
                visible_text.clear();
            }
            Ok(Some(AgentEvent::TextDelta { delta })) => {
                visible_text.push_str(&delta);
            }
            Ok(Some(AgentEvent::Error { .. })) => {
                saw_error = true;
            }
            Ok(Some(AgentEvent::Done { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => panic!("timed out waiting for steered completion"),
        }
    }

    let final_msg = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("run should finish")
        .expect("join should succeed")
        .expect("agent should succeed");

    assert!(saw_reset, "steering should reset the obsolete draft");
    assert!(!saw_error, "steering should not surface an error");
    assert_eq!(visible_text, "steered answer");
    assert_eq!(final_msg.text_content(), "steered answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);

    let persisted = db.get_messages(&conversation.id).expect("messages");
    let obsolete_draft_index = persisted
        .iter()
        .position(|message| {
            message.role == Role::Assistant && message.content.trim() == "obsolete draft"
        })
        .expect("interrupted assistant draft should be persisted");
    let steering_index = persisted
        .iter()
        .position(|message| {
            message.role == Role::User
                && message.artifacts.as_ref().is_some_and(|artifacts| {
                    artifacts.get("kind").and_then(serde_json::Value::as_str) == Some("steering")
                })
        })
        .expect("steering user message should be persisted");
    assert!(obsolete_draft_index < steering_index);

    let requests = request_texts.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .iter()
            .any(|message| message.contains("focus on edge cases instead")),
        "second LLM request should include steering text"
    );
    let replay_history = persisted
        .iter()
        .map(|message| {
            let mut replay = Message::text(
                message.role.clone(),
                conversation_message_llm_context_content(message),
            );
            replay.name = message.tool_call_id.clone();
            replay.tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(message.tool_calls.clone())
            };
            replay
        })
        .collect::<Vec<_>>();
    let replay_prefix = context::prepare_messages_with_options(
        "stable system",
        &replay_history,
        &[ContentPart::Text {
            text: "next turn".to_string(),
        }],
        "deepseek-chat",
        4096,
        None,
        &[],
        &[],
        &[],
        context::PrepareMessagesOptions {
            include_skill_system_prompt: false,
            volatile_system_sections: &[],
            append_volatile_system_prompt_to_tail: true,
            ..context::PrepareMessagesOptions::default()
        },
    )
    .into_iter()
    .map(|message| format!("{:?}:{}", message.role, message.text_content()))
    .take(requests[1].len())
    .collect::<Vec<_>>();
    assert_eq!(requests[1], replay_prefix);
}

#[derive(Clone, Copy)]
enum ModelProgressScript {
    ActiveThinkingThenAnswer,
    PendingStreamOpen,
}

struct ModelProgressScriptedProvider {
    script: ModelProgressScript,
    stream_calls: Arc<AtomicUsize>,
    thinking_chunks: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
    invocation_started: Arc<Notify>,
}

#[async_trait]
impl LlmProvider for ModelProgressScriptedProvider {
    fn name(&self) -> &str {
        "model-progress-scripted-mock"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        Ok(vec!["qwen3.8-max".to_string()])
    }

    async fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, CoreError> {
        Err(CoreError::Llm(
            "model-progress tests must remain on the streaming seam".to_string(),
        ))
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.requests.lock().unwrap().push(request.clone());
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.invocation_started.notify_one();

        match (self.script, call_no) {
            (ModelProgressScript::PendingStreamOpen, 0) => {
                std::future::pending::<Result<BoxStream<'_, ProviderStreamEvent>, CoreError>>()
                    .await
            }
            (ModelProgressScript::ActiveThinkingThenAnswer, 0) => {
                let thinking_chunks = Arc::clone(&self.thinking_chunks);
                crate::llm::provider_events_from_chunk_stream(Box::pin(stream::unfold(
                    0_u64,
                    move |tick| {
                        let thinking_chunks = Arc::clone(&thinking_chunks);
                        async move {
                            if tick == 180 {
                                return Some((
                                    Ok(StreamChunk {
                                        delta: "answer after active reasoning".to_string(),
                                        tool_call_delta: None,
                                        finish_reason: Some(FinishReason::Stop),
                                        usage: Some(Usage::default()),
                                        thinking_delta: None,
                                    }),
                                    tick + 1,
                                ));
                            }
                            if tick > 180 {
                                return None;
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            thinking_chunks.fetch_add(1, Ordering::SeqCst);
                            Some((
                                Ok(StreamChunk {
                                    delta: String::new(),
                                    tool_call_delta: None,
                                    finish_reason: None,
                                    usage: None,
                                    thinking_delta: Some(format!("thinking tick {tick}")),
                                }),
                                tick + 1,
                            ))
                        }
                    },
                )))
            }
            _ => crate::llm::provider_events_from_chunk_stream(Box::pin(stream::iter(vec![Ok(
                StreamChunk {
                    delta: "recovered answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: Some(Usage::default()),
                    thinking_delta: None,
                },
            )]))),
        }
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Default)]
struct ModelProgressEventCounts {
    thinking: usize,
    slow_warnings: usize,
    resets: usize,
    errors: usize,
    done: usize,
    done_usage: Option<Usage>,
}

async fn drain_model_progress_events(
    mut rx: mpsc::Receiver<AgentEvent>,
) -> ModelProgressEventCounts {
    let mut counts = ModelProgressEventCounts::default();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Thinking { .. } => counts.thinking += 1,
            AgentEvent::ControllerStatus { ref code, .. } if code == "model_planning_slow" => {
                counts.slow_warnings += 1;
            }
            AgentEvent::StreamReset { .. } => counts.resets += 1,
            AgentEvent::Error { .. } => counts.errors += 1,
            AgentEvent::Done { usage_total, .. } => {
                counts.done += 1;
                counts.done_usage = Some(usage_total);
            }
            _ => {}
        }
    }
    counts
}

async fn settle_paused_runtime() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn model_progress_qwen_thinking_stream_remains_alive_until_answer() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let thinking_chunks = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invocation_started = Arc::new(Notify::new());
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(ModelProgressScriptedProvider {
            script: ModelProgressScript::ActiveThinkingThenAnswer,
            stream_calls: Arc::clone(&stream_calls),
            thinking_chunks: Arc::clone(&thinking_chunks),
            requests: Arc::clone(&requests),
            invocation_started: Arc::clone(&invocation_started),
        }),
        registry,
        AgentConfig {
            model: Some("qwen3.8-max".to_string()),
            provider_type: Some(ProviderType::Qwen),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::XHigh),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let run_db = db.clone();
    let (tx, rx) = mpsc::channel(64);
    let event_drain = tokio::spawn(drain_model_progress_events(rx));
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "Inspect this codebase with recording_tool and report the result."
                        .to_string(),
                }],
                &run_db,
                None,
                None,
                tx,
                0,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("the first provider stream must open");
    for _ in 0..179 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle_paused_runtime().await;
    }

    assert!(
        !run.is_finished(),
        "an active reasoning stream must remain under user control"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert!(
        thinking_chunks.load(Ordering::SeqCst) >= 170,
        "the scripted stream must stay active with thinking-only chunks"
    );

    tokio::time::advance(Duration::from_secs(1)).await;
    settle_paused_runtime().await;
    let final_message = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("the active stream must complete immediately after its answer")
        .expect("agent run task")
        .expect("the original stream should succeed");
    let event_counts = event_drain.await.expect("agent event drain");

    assert_eq!(
        final_message.text_content(),
        "answer after active reasoning"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(event_counts.slow_warnings, 1);
    assert_eq!(event_counts.resets, 0);
    assert_eq!(event_counts.errors, 0);
    assert_eq!(event_counts.done, 1);
    assert!(event_counts.thinking >= 170);

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1, "active streams must not be replayed");
    assert_eq!(captured[0].reasoning_enabled, Some(true));
    assert_eq!(captured[0].reasoning_effort, Some(ReasoningEffort::XHigh));
}

#[tokio::test(start_paused = true)]
async fn model_progress_user_recovery_restarts_with_request_side_controls() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let thinking_chunks = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invocation_started = Arc::new(Notify::new());
    let (steering_tx, steering_rx) = mpsc::unbounded_channel();
    let executor = AgentExecutor::new(
        Box::new(ModelProgressScriptedProvider {
            script: ModelProgressScript::ActiveThinkingThenAnswer,
            stream_calls: Arc::clone(&stream_calls),
            thinking_chunks,
            requests: Arc::clone(&requests),
            invocation_started: Arc::clone(&invocation_started),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("qwen3.8-max".to_string()),
            provider_type: Some(ProviderType::Qwen),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::XHigh),
            ..AgentConfig::default()
        },
    )
    .with_steering_receiver(steering_rx);
    let db = Database::open_memory().expect("in-memory db");
    let run_db = db.clone();
    let (tx, rx) = mpsc::channel(64);
    let event_drain = tokio::spawn(drain_model_progress_events(rx));
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "Inspect the active model stream.".to_string(),
                }],
                &run_db,
                None,
                None,
                tx,
                0,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("the first provider stream must open");
    tokio::time::advance(Duration::from_secs(1)).await;
    settle_paused_runtime().await;
    steering_tx
        .send(AgentSteeringMessage::recovery(
            AgentRecoveryControl::LowerReasoningAndRetry,
        ))
        .expect("recovery control send");
    settle_paused_runtime().await;

    let final_message = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("the controlled retry must complete")
        .expect("agent run task")
        .expect("controlled retry succeeds");
    let event_counts = event_drain.await.expect("agent event drain");

    assert_eq!(final_message.text_content(), "recovered answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
    assert_eq!(event_counts.resets, 1);
    assert_eq!(event_counts.errors, 0);
    let done_usage = event_counts.done_usage.as_ref().expect("final usage event");
    assert!(
        done_usage.prompt_tokens > 0 && done_usage.total_tokens >= done_usage.prompt_tokens,
        "the discarded physical sample must count toward prompt and total usage"
    );
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].reasoning_enabled, Some(true));
    assert_eq!(captured[0].reasoning_effort, Some(ReasoningEffort::XHigh));
    assert_eq!(captured[1].reasoning_enabled, Some(false));
    assert_eq!(captured[1].reasoning_effort, None);
}

#[tokio::test(start_paused = true)]
async fn model_progress_pending_stream_open_stops_without_executing_tools() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let thinking_chunks = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invocation_started = Arc::new(Notify::new());
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(ModelProgressScriptedProvider {
            script: ModelProgressScript::PendingStreamOpen,
            stream_calls: Arc::clone(&stream_calls),
            thinking_chunks,
            requests: Arc::clone(&requests),
            invocation_started: Arc::clone(&invocation_started),
        }),
        registry,
        AgentConfig {
            model: Some("qwen3.8-max".to_string()),
            provider_type: Some(ProviderType::Qwen),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let run_db = db.clone();
    let (tx, rx) = mpsc::channel(64);
    let event_drain = tokio::spawn(drain_model_progress_events(rx));
    let started_at = tokio::time::Instant::now();
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "Inspect this codebase with recording_tool.".to_string(),
                }],
                &run_db,
                None,
                None,
                tx,
                0,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("the provider stream-open future must start");
    tokio::time::advance(Duration::from_secs(89)).await;
    settle_paused_runtime().await;
    assert!(
        !run.is_finished(),
        "connect timeout fired before 90 seconds"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);

    tokio::time::advance(Duration::from_secs(1)).await;
    settle_paused_runtime().await;
    let error = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("connect deadline must terminalize the run")
        .expect("agent run task")
        .expect_err("a stream that never opens must fail");
    let event_counts = event_drain.await.expect("agent event drain");
    let elapsed = tokio::time::Instant::now().duration_since(started_at);

    assert!(matches!(
        error,
        CoreError::Agent(ref message) if message.contains("provider did not establish a model stream")
    ));
    assert!(elapsed >= Duration::from_secs(90));
    assert!(elapsed <= Duration::from_secs(91));
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(event_counts.slow_warnings, 0);
    assert_eq!(event_counts.resets, 0);
    assert_eq!(event_counts.errors, 1);
    assert_eq!(event_counts.done, 0);
}

#[tokio::test(start_paused = true)]
async fn model_progress_direct_kimi_k3_keeps_requested_effort_while_active() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let thinking_chunks = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invocation_started = Arc::new(Notify::new());
    let executor = AgentExecutor::new(
        Box::new(ModelProgressScriptedProvider {
            script: ModelProgressScript::ActiveThinkingThenAnswer,
            stream_calls: Arc::clone(&stream_calls),
            thinking_chunks,
            requests: Arc::clone(&requests),
            invocation_started: Arc::clone(&invocation_started),
        }),
        ToolRegistry::new(),
        AgentConfig {
            model: Some("kimi-k3".to_string()),
            provider_type: Some(ProviderType::Moonshot),
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..AgentConfig::default()
        },
    );
    let db = Database::open_memory().expect("in-memory db");
    let run_db = db.clone();
    let (tx, rx) = mpsc::channel(64);
    let event_drain = tokio::spawn(drain_model_progress_events(rx));
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "为什么主agent没有办法调用run_shell？请仔细排查并全面修复。".to_string(),
                }],
                &run_db,
                None,
                None,
                tx,
                0,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("the first Kimi stream must open");
    for _ in 0..180 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle_paused_runtime().await;
    }
    let final_message = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("the active Kimi stream must complete immediately after its answer")
        .expect("agent run task")
        .expect("Kimi stream should succeed without replay");
    let event_counts = event_drain.await.expect("agent event drain");

    assert_eq!(
        final_message.text_content(),
        "answer after active reasoning"
    );
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(event_counts.slow_warnings, 1);
    assert_eq!(event_counts.resets, 0);
    assert_eq!(event_counts.errors, 0);

    let captured = requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "active Kimi streams must not be replayed"
    );
    assert_eq!(captured[0].reasoning_enabled, Some(true));
    assert_eq!(captured[0].reasoning_effort, Some(ReasoningEffort::Max));
}

#[tokio::test(start_paused = true)]
async fn model_progress_cancellation_wins_the_deadline_race() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let thinking_chunks = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invocation_started = Arc::new(Notify::new());
    let executions = Arc::new(AtomicUsize::new(0));
    let cancel_token = CancellationToken::new();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(RecordingTool {
        executions: Arc::clone(&executions),
    }));
    let executor = AgentExecutor::new(
        Box::new(ModelProgressScriptedProvider {
            script: ModelProgressScript::ActiveThinkingThenAnswer,
            stream_calls: Arc::clone(&stream_calls),
            thinking_chunks,
            requests: Arc::clone(&requests),
            invocation_started: Arc::clone(&invocation_started),
        }),
        registry,
        AgentConfig {
            model: Some("qwen3.8-max".to_string()),
            provider_type: Some(ProviderType::Qwen),
            reasoning_enabled: Some(true),
            ..AgentConfig::default()
        },
    )
    .with_cancel_token(cancel_token.clone());
    let db = Database::open_memory().expect("in-memory db");
    let run_db = db.clone();
    let (tx, rx) = mpsc::channel(64);
    let event_drain = tokio::spawn(drain_model_progress_events(rx));
    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "Inspect this codebase with recording_tool.".to_string(),
                }],
                &run_db,
                None,
                None,
                tx,
                0,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), invocation_started.notified())
        .await
        .expect("the provider stream must open");
    for _ in 0..89 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle_paused_runtime().await;
    }
    assert!(!run.is_finished());

    // Cancellation remains authoritative even while reasoning bytes continue
    // to arrive and no semantic milestone deadline is armed.
    cancel_token.cancel();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle_paused_runtime().await;
    let error = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("cancellation must terminalize the deadline race")
        .expect("agent run task")
        .expect_err("the run should be cancelled");
    let event_counts = event_drain.await.expect("agent event drain");

    assert!(matches!(error, CoreError::Cancelled(_)));
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(event_counts.slow_warnings, 1);
    assert_eq!(event_counts.resets, 0);
    assert_eq!(event_counts.errors, 1);
    assert_eq!(event_counts.done, 0);
}
