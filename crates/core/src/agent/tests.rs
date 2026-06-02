use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use super::*;
use crate::approval::{ApprovalDecision, ToolApprovalMode};
use crate::conversation::CreateConversationInput;
use crate::llm::{CompletionResponse, FinishReason, StreamChunk};
use crate::tools::{Tool, ToolResult};

#[test]
fn test_tool_timeout_zero_disables_outer_timeout() {
    let timeout = tool_timeout_for_call(Some(0), "read_file", &serde_json::json!({}));
    assert_eq!(timeout, None);
}

#[test]
fn test_tool_timeout_honors_run_shell_no_timeout() {
    let timeout = tool_timeout_for_call(
        Some(30),
        "run_shell",
        &serde_json::json!({ "timeout_secs": 0 }),
    );
    assert_eq!(timeout, None);
}

#[test]
fn test_tool_timeout_extends_for_long_run_shell_timeout() {
    let timeout = tool_timeout_for_call(
        Some(30),
        "run_shell",
        &serde_json::json!({ "timeout_secs": 600 }),
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
    assert_eq!(cfg.max_iterations, 25);
    assert!(cfg
        .system_prompt
        .contains("local-first personal workspace assistant"));
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
}

#[test]
fn test_build_system_prompt_skips_blank_sections() {
    let prompt = build_system_prompt(Some("   "), &["", "  ", "\n\n"]);
    assert_eq!(prompt, default_system_prompt());
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
        call_id: &str,
        _arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "tool-ok".to_string(),
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
        call_id: &str,
        _arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
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
        call_id: &str,
        _arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
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
        call_id: &str,
        _arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
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
    registry.register(Box::new(DelayTool {
        name: "slow_tool",
        delay_ms: 1000,
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

    let first_result = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            tokio::select! {
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

    let final_msg = run.await.expect("run should succeed");
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

    let first_result = tokio::time::timeout(Duration::from_millis(700), async {
        loop {
            tokio::select! {
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
    let non_loop_items = items
        .iter()
        .filter(|item| {
            !matches!(
                item.get("kind").and_then(|v| v.as_str()),
                Some("loop") | Some("skillSelection")
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
            model: Some("mock-model".to_string()),
            max_iterations: 3,
            ..AgentConfig::default()
        },
    )
    .with_steering_receiver(steering_rx);

    let db = Database::open_memory().expect("in-memory db");
    let (tx, mut rx) = mpsc::channel(64);

    let run = tokio::spawn(async move {
        executor
            .run(
                vec![],
                vec![ContentPart::Text {
                    text: "start broad".to_string(),
                }],
                &db,
                None,
                None,
                tx,
                0,
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

    let requests = request_texts.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .iter()
            .any(|message| message.contains("focus on edge cases instead")),
        "second LLM request should include steering text"
    );
}
