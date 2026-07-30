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
fn test_default_config() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.max_iterations, u32::MAX);
    assert!(cfg.system_prompt.contains("local-first workspace agent"));
    assert_eq!(cfg.temperature, Some(0.3));
    assert_eq!(cfg.max_tokens, Some(4096));
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "call_1".to_string(),
                    name: Some("mock_tool".to_string()),
                    arguments_delta: r#"{"value":"ok"}"#.to_string(),
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
        Ok(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct ThinkingMockProvider {
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for ThinkingMockProvider {
    fn name(&self) -> &str {
        "thinking-mock"
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
                        arguments_delta: r#"{"value":"ok"}"#.to_string(),
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
        Ok(Box::pin(stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct RecoveringStreamProvider {
    stream_calls: Arc<AtomicUsize>,
    complete_calls: Arc<AtomicUsize>,
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
            },
            thinking: None,
        })
    }

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamChunk {
                delta: "partial ".to_string(),
                tool_call_delta: None,
                finish_reason: None,
                usage: None,
                thinking_delta: None,
            }),
            Err(CoreError::StreamIncomplete(
                "stream interrupted: error decoding response body".to_string(),
            )),
        ])))
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if call_no == 0 {
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamChunk {
                    delta: "partial ".to_string(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }),
                Err(CoreError::StreamIncomplete(
                    "stream interrupted: error decoding response body".to_string(),
                )),
            ])));
        }

        Ok(Box::pin(stream::iter(vec![Ok(StreamChunk {
            delta: "stream answer".to_string(),
            tool_call_delta: None,
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        })])))
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

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.request_texts.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|message| format!("{:?}:{}", message.role, message.text_content()))
                .collect(),
        );

        if call_no == 0 {
            return Ok(Box::pin(stream::unfold(0, |state| async move {
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
            })));
        }

        Ok(Box::pin(stream::iter(vec![Ok(StreamChunk {
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
            }),
            thinking_delta: None,
        })])))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct MockTool;

struct NamedMockTool(&'static str);

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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![
                Ok(StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: "fast_call".to_string(),
                        name: Some("fast_tool".to_string()),
                        arguments_delta: r#"{"value":"fast"}"#.to_string(),
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
                        arguments_delta: r#"{"value":"slow"}"#.to_string(),
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
        Ok(Box::pin(stream::iter(chunks)))
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
                    arguments_delta: r#"{"status":"complete"}"#.to_string(),
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
        Ok(Box::pin(stream::iter(chunks.into_iter().map(Ok))))
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
        Ok(Box::pin(stream::iter(chunks.into_iter().map(Ok))))
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

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
        Ok(Box::pin(stream::iter(vec![Ok(StreamChunk {
            delta: "done".to_string(),
            tool_call_delta: None,
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            thinking_delta: None,
        })])))
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

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
        Ok(Box::pin(stream::iter(chunks.into_iter().map(Ok))))
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
    let expected = registry
        .definitions()
        .into_iter()
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
    assert_eq!(requests.len(), 1);
    for name in [
        "spawn_subagent_batch",
        "spawn_subagent",
        "judge_subagent_results",
    ] {
        assert!(requests[0].iter().any(|offered| offered == name));
    }
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
    assert!(requests[0].iter().any(|name| name == "mock_tool"));
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

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
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
        Ok(Box::pin(stream::unfold(
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

    async fn stream(
        &self,
        _request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let call_no = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if call_no == 0 {
            vec![Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: Some(ToolCallDelta {
                    id: "approval_call_1".to_string(),
                    name: Some("locked_write".to_string()),
                    arguments_delta: r#"{"path":"notes/a.md"}"#.to_string(),
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
        Ok(Box::pin(stream::iter(chunks)))
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
async fn test_executes_tool_even_when_finish_reason_is_stop() {
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
                    arguments_delta: r#"{"value":"slow"}"#.to_string(),
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
                    arguments_delta: r#"{"value":"fast"}"#.to_string(),
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

    let first_result = tokio::time::timeout(Duration::from_secs(3), async {
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
    let policy = ToolSchedulerPolicy::new(None, false, offered, registered);
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
    let policy = ToolSchedulerPolicy::new(None, false, offered, registered);
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
    let policy = ToolSchedulerPolicy::new(None, false, offered, registered);
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
                arguments_delta: r#"{"value":"slow"}"#.to_string(),
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
                    arguments_delta: r#"{"prompt":"hel"#.to_string(),
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
                    arguments_delta: r#"lo"}"#.to_string(),
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
    assert!(persisted.iter().any(
        |message| message.role == Role::System && message.content.contains("Active Task Plan")
    ));
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
                arguments_delta: r#"{"path":"large.txt"}"#.to_string(),
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
                arguments_delta: r#"{"value":"ok"}"#.to_string(),
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
    assert_eq!(non_loop_items.len(), 5);
    assert_eq!(
        non_loop_items[0].get("kind").and_then(|v| v.as_str()),
        Some("toolVisibility")
    );
    assert_eq!(
        non_loop_items[0]["decision"]["route"].as_str(),
        Some("DirectResponse")
    );
    assert!(non_loop_items[0]["decision"]["log"]
        .as_array()
        .is_some_and(|log| !log.is_empty()));
    assert_eq!(
        non_loop_items[1].get("kind").and_then(|v| v.as_str()),
        Some("thinking")
    );
    assert_eq!(
        non_loop_items[2].get("kind").and_then(|v| v.as_str()),
        Some("tool")
    );
    assert_eq!(
        non_loop_items[3].get("kind").and_then(|v| v.as_str()),
        Some("thinking")
    );
    assert_eq!(
        non_loop_items[4].get("kind").and_then(|v| v.as_str()),
        Some("status")
    );
}

#[tokio::test]
async fn test_stream_incomplete_replays_stream_before_non_streaming_fallback() {
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

    assert!(saw_reset, "expected partial stream reset before replay");
    assert!(!saw_error, "stream replay should not surface an error");
    assert_eq!(visible_text, "stream answer");
}

#[tokio::test]
async fn test_stream_incomplete_recovers_with_non_streaming_retry() {
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
        .expect("run should recover");

    assert_eq!(final_msg.text_content(), "complete answer");
    assert_eq!(stream_calls.load(Ordering::SeqCst), 3);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 1);

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

    assert!(
        saw_reset,
        "expected partial stream reset before retry replay"
    );
    assert!(!saw_error, "stream recovery should not surface an error");
    assert_eq!(visible_text, "complete answer");
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
