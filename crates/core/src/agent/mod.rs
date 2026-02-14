//! Agent executor — ReAct-style reasoning loop with streaming and tool dispatch.

use std::time::Duration;

use futures::StreamExt;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::conversation::ConversationMessage;
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{
    CompletionRequest, LlmProvider, Message, Role, ToolCallDelta, ToolCallRequest, Usage,
};
use crate::privacy;
use crate::tools::ToolRegistry;

pub mod context;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events emitted by the agent during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// Incremental text token from the LLM.
    TextDelta { delta: String },
    /// A tool call is about to be executed.
    ToolCallStart {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        arguments: String,
    },
    /// Result of a tool execution.
    ToolCallResult {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        content: String,
        #[serde(rename = "isError")]
        is_error: bool,
        artifacts: Option<serde_json::Value>,
    },
    /// Thinking / chain-of-thought text (if the model supports it).
    Thinking { content: String },
    /// The agent finished producing a final answer.
    Done {
        message: Message,
        #[serde(rename = "usageTotal")]
        usage_total: Usage,
    },
    /// An error occurred during execution.
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Maximum number of LLM round-trips (prevents runaway tool loops).
    pub max_iterations: u32,
    /// System prompt prepended to every request.
    pub system_prompt: String,
    /// Override model name (provider default used when `None`).
    pub model: Option<String>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Maximum tokens for the LLM response.
    pub max_tokens: Option<u32>,
    /// Override context window size (auto-detected from model when `None`).
    pub context_window: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            model: None,
            temperature: Some(0.3),
            max_tokens: Some(4096),
            context_window: None,
        }
    }
}

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/system.md");

const DEFAULT_MODEL: &str = "gpt-4o-mini";

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// The main agent executor implementing a ReAct-style loop.
///
/// Each call to [`run`](AgentExecutor::run) performs up to `max_iterations`
/// LLM round-trips, dispatching tool calls between each round until the model
/// produces a final text answer (or the iteration cap is hit).
pub struct AgentExecutor {
    provider: Box<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
}

impl AgentExecutor {
    /// Create a new executor from a provider, tool registry, and config.
    pub fn new(provider: Box<dyn LlmProvider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

    /// Run the agent loop for a single user turn.
    ///
    /// * `history` — prior conversation messages (already stored in DB).
    /// * `user_message` — the new user input.
    /// * `db` — database handle passed through to tools and privacy config.
    /// * `conversation_id` — optional conversation ID for source scoping.
    /// * `tx` — channel for streaming [`AgentEvent`]s to the caller (e.g. Tauri).
    /// * `next_sort_order` — the sort_order to use for the first message saved
    ///   by the executor (intermediate + final). The caller should set this to
    ///   one past the last message it already persisted (e.g. the user message).
    ///
    /// Returns the final assistant [`Message`] on success.
    pub async fn run(
        &self,
        history: Vec<Message>,
        user_message: String,
        db: &Database,
        conversation_id: Option<&str>,
        tx: mpsc::Sender<AgentEvent>,
        next_sort_order: i64,
    ) -> Result<Message, CoreError> {
        let model = self.config.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let max_response_tokens = self.config.max_tokens.unwrap_or(4096);

        // --- 1. Build initial messages with context-window trimming -----------
        let mut messages = context::prepare_messages(
            &self.config.system_prompt,
            &history,
            &user_message,
            model,
            max_response_tokens,
            self.config.context_window,
        );

        // --- 2. Privacy redaction on outgoing user content --------------------
        let privacy_cfg = db.load_privacy_config().unwrap_or_default();
        if privacy_cfg.enabled {
            for msg in &mut messages {
                if msg.role == Role::User {
                    msg.content =
                        privacy::redact_content(&msg.content, &privacy_cfg.redact_patterns);
                }
            }
        }

        // --- 3. Prepare tool definitions -------------------------------------
        let tool_defs = self.tools.definitions();
        let tools_param = if tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs)
        };

        // --- 3b. Resolve source scope for this conversation ------------------
        let source_scope: Vec<String> = match conversation_id {
            Some(cid) => db.get_linked_sources(cid).unwrap_or_default(),
            None => Vec::new(),
        };

        let mut total_usage = Usage::default();
        let mut sort_order = next_sort_order;
        let mut accumulated_content = String::new();

        // --- 4. ReAct loop ----------------------------------------------------
        for iteration in 0..self.config.max_iterations {
            debug!(
                "Agent iteration {}/{}",
                iteration + 1,
                self.config.max_iterations
            );

            let request = CompletionRequest {
                model: model.to_string(),
                messages: messages.clone(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
                tools: tools_param.clone(),
                stop: None,
            };

            // -- 4a. Stream LLM response --------------------------------------
            let mut stream = match self.provider.stream(&request).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx
                        .send(AgentEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return Err(e);
                }
            };

            let mut full_content = String::new();
            let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
            let mut chunk_usage: Option<Usage> = None;

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        // Forward text deltas.
                        if !chunk.delta.is_empty() {
                            full_content.push_str(&chunk.delta);
                            accumulated_content.push_str(&chunk.delta);
                            let _ = tx.send(AgentEvent::TextDelta { delta: chunk.delta }).await;
                        }
                        // Accumulate tool-call deltas.
                        if let Some(ref tc_delta) = chunk.tool_call_delta {
                            accumulate_tool_call(&mut tool_calls, tc_delta);
                        }
                        if let Some(u) = chunk.usage {
                            chunk_usage = Some(u);
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(AgentEvent::Error {
                                message: e.to_string(),
                            })
                            .await;
                        return Err(e);
                    }
                }
            }

            // -- 4b. Accumulate usage ------------------------------------------
            if let Some(u) = chunk_usage {
                total_usage.prompt_tokens += u.prompt_tokens;
                total_usage.completion_tokens += u.completion_tokens;
                total_usage.total_tokens += u.total_tokens;
            }

            // -- 4c. Build assistant message -----------------------------------
            let assistant_msg = Message {
                role: Role::Assistant,
                content: full_content,
                name: None,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.clone())
                },
            };
            messages.push(assistant_msg.clone());

            // -- 4d. Check termination -----------------------------------------
            if tool_calls.is_empty() {
                // Save final assistant message to DB.
                if let Some(cid) = conversation_id {
                    let conv_msg = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: cid.to_string(),
                        role: Role::Assistant,
                        content: assistant_msg.content.clone(),
                        tool_call_id: None,
                        tool_calls: assistant_msg.tool_calls.clone().unwrap_or_default(),
                        token_count: (assistant_msg.content.len() / 4) as u32,
                        created_at: String::new(),
                        sort_order,
                    };
                    if let Err(e) = db.add_message(&conv_msg) {
                        warn!("Failed to save final assistant message: {e}");
                    }
                }

                let _ = tx
                    .send(AgentEvent::Done {
                        message: assistant_msg.clone(),
                        usage_total: total_usage,
                    })
                    .await;
                return Ok(assistant_msg);
            }

            // -- 4d'. Save intermediate assistant message (with tool_calls) ----
            if let Some(cid) = conversation_id {
                let conv_msg = ConversationMessage {
                    id: Uuid::new_v4().to_string(),
                    conversation_id: cid.to_string(),
                    role: Role::Assistant,
                    content: assistant_msg.content.clone(),
                    tool_call_id: None,
                    tool_calls: tool_calls.clone(),
                    token_count: (assistant_msg.content.len() / 4) as u32,
                    created_at: String::new(),
                    sort_order,
                };
                if let Err(e) = db.add_message(&conv_msg) {
                    warn!("Failed to save intermediate assistant message: {e}");
                }
                sort_order += 1;
            }

            // -- 4e. Execute tool calls ----------------------------------------
            for tc in &tool_calls {
                let _ = tx
                    .send(AgentEvent::ToolCallStart {
                        call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .await;

                // Per-tool timeout to prevent hangs.
                let tool_timeout = match tc.name.as_str() {
                    "retrieve_evidence" => Duration::from_secs(60),
                    _ => Duration::from_secs(30),
                };

                let tool_result = tokio::time::timeout(
                    tool_timeout,
                    self.tools
                        .execute(&tc.name, &tc.id, &tc.arguments, db, &source_scope),
                )
                .await;

                let tool_msg = match tool_result {
                    Ok(Ok(result)) => {
                        let _ = tx
                            .send(AgentEvent::ToolCallResult {
                                call_id: result.call_id.clone(),
                                tool_name: tc.name.clone(),
                                content: result.content.clone(),
                                is_error: result.is_error,
                                artifacts: result.artifacts.clone(),
                            })
                            .await;
                        result.content
                    }
                    Ok(Err(e)) => {
                        let err_content = format!("Error: {e}");
                        let _ = tx
                            .send(AgentEvent::ToolCallResult {
                                call_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                                content: err_content.clone(),
                                is_error: true,
                                artifacts: None,
                            })
                            .await;
                        err_content
                    }
                    Err(_elapsed) => {
                        warn!("Tool '{}' timed out after {:?}", tc.name, tool_timeout);
                        let err_content = format!(
                            "Error: tool '{}' timed out after {} seconds. Try a simpler query or different approach.",
                            tc.name, tool_timeout.as_secs()
                        );
                        let _ = tx
                            .send(AgentEvent::ToolCallResult {
                                call_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                                content: err_content.clone(),
                                is_error: true,
                                artifacts: None,
                            })
                            .await;
                        err_content
                    }
                };

                // Redact tool output before adding to context.
                let content = if privacy_cfg.enabled {
                    privacy::redact_content(&tool_msg, &privacy_cfg.redact_patterns)
                } else {
                    tool_msg
                };

                // Save tool result message to DB.
                if let Some(cid) = conversation_id {
                    let tool_conv_msg = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: cid.to_string(),
                        role: Role::Tool,
                        content: content.clone(),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                        token_count: (content.len() / 4) as u32,
                        created_at: String::new(),
                        sort_order,
                    };
                    if let Err(e) = db.add_message(&tool_conv_msg) {
                        warn!("Failed to save tool result message: {e}");
                    }
                    sort_order += 1;
                }

                messages.push(Message {
                    role: Role::Tool,
                    content,
                    name: Some(tc.id.clone()),
                    tool_calls: None,
                });
            }
            // Loop back → next LLM call with tool results.
        }

        // Graceful fallback: return partial answer instead of hard error.
        warn!(
            "Agent reached max iterations ({}); returning partial answer",
            self.config.max_iterations
        );

        if !accumulated_content.is_empty() {
            let note = "\n\n*[Note: I used all available tool calls. The answer above may be incomplete.]*";
            let _ = tx
                .send(AgentEvent::TextDelta {
                    delta: note.to_string(),
                })
                .await;
            accumulated_content.push_str(note);
        }

        let final_msg = Message {
            role: Role::Assistant,
            content: accumulated_content,
            name: None,
            tool_calls: None,
        };

        if let Some(cid) = conversation_id {
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_msg.content.clone(),
                tool_call_id: None,
                tool_calls: vec![],
                token_count: (final_msg.content.len() / 4) as u32,
                created_at: String::new(),
                sort_order,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save final assistant message: {e}");
            }
        }

        let _ = tx
            .send(AgentEvent::Done {
                message: final_msg.clone(),
                usage_total: total_usage,
            })
            .await;
        Ok(final_msg)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Accumulate streaming tool-call deltas into complete [`ToolCallRequest`]s.
///
/// OpenAI sends each tool call across multiple SSE chunks:
/// - First chunk provides `id` + `name`
/// - Subsequent chunks append to `arguments_delta` and may omit `id`, using `index`
///
/// When `id` is non-empty we either update an existing entry or create a new one.
/// When `id` is empty we fall back to `index`, then to the most recent tool call.
fn accumulate_tool_call(calls: &mut Vec<ToolCallRequest>, delta: &ToolCallDelta) {
    if !delta.id.is_empty() {
        // Lookup by id — update existing or insert new.
        if let Some(existing) = calls.iter_mut().find(|c| c.id == delta.id) {
            if let Some(ref name) = delta.name {
                existing.name.clone_from(name);
            }
            existing.arguments.push_str(&delta.arguments_delta);
        } else {
            calls.push(ToolCallRequest {
                id: delta.id.clone(),
                name: delta.name.clone().unwrap_or_default(),
                arguments: delta.arguments_delta.clone(),
            });
        }
    } else if let Some(index) = delta.index {
        // Some providers omit id on follow-up chunks and only send the call index.
        let index = index as usize;
        if let Some(existing) = calls.get_mut(index) {
            if let Some(ref name) = delta.name {
                existing.name.clone_from(name);
            }
            existing.arguments.push_str(&delta.arguments_delta);
        } else if index == calls.len() {
            calls.push(ToolCallRequest {
                id: format!("call_{index}"),
                name: delta.name.clone().unwrap_or_default(),
                arguments: delta.arguments_delta.clone(),
            });
        } else if let Some(last) = calls.last_mut() {
            if let Some(ref name) = delta.name {
                last.name.clone_from(name);
            }
            last.arguments.push_str(&delta.arguments_delta);
        }
    } else if let Some(last) = calls.last_mut() {
        // No id provided — append to the most recent tool call.
        if let Some(ref name) = delta.name {
            last.name.clone_from(name);
        }
        last.arguments.push_str(&delta.arguments_delta);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};

    use super::*;
    use crate::llm::{CompletionResponse, StreamChunk};
    use crate::tools::{Tool, ToolResult};

    #[test]
    fn test_accumulate_new_tool_call() {
        let mut calls = Vec::new();
        let delta = ToolCallDelta {
            id: "call_1".into(),
            name: Some("search".into()),
            arguments_delta: r#"{"qu"#.into(),
            index: None,
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
        }];
        let delta = ToolCallDelta {
            id: "call_1".into(),
            name: None,
            arguments_delta: r#"ery":"test"}"#.into(),
            index: None,
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
        }];
        let delta = ToolCallDelta {
            id: String::new(),
            name: None,
            arguments_delta: r#"":"v"}"#.into(),
            index: None,
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
            },
        );
        accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: "call_2".into(),
                name: Some("file".into()),
                arguments_delta: "{}".into(),
                index: None,
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
            },
            ToolCallRequest {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"C"#.into(),
            },
        ];

        accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: String::new(),
                name: None,
                arguments_delta: r#"lo"}"#.into(),
                index: Some(0),
            },
        );
        accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: String::new(),
                name: None,
                arguments_delta: r#":\a.md"}"#.into(),
                index: Some(1),
            },
        );

        assert_eq!(calls[0].arguments, r#"{"q":"hello"}"#);
        assert_eq!(calls[1].arguments, r#"{"path":"C:\a.md"}"#);
    }

    #[test]
    fn test_default_config() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.max_iterations, 10);
        assert!(cfg.system_prompt.contains("knowledge assistant"));
        assert_eq!(cfg.temperature, Some(0.3));
        assert_eq!(cfg.max_tokens, Some(4096));
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
                    }),
                    // Some providers return `stop` even when tool calls are present.
                    finish_reason: Some(crate::llm::FinishReason::Stop),
                    usage: None,
                })]
            } else {
                vec![Ok(StreamChunk {
                    delta: "final answer".to_string(),
                    tool_call_delta: None,
                    finish_reason: Some(crate::llm::FinishReason::Stop),
                    usage: None,
                })]
            };
            Ok(Box::pin(stream::iter(chunks)))
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
            .run(vec![], "hello".to_string(), &db, None, tx, 0)
            .await
            .expect("run should succeed");

        // Should perform two LLM calls: one for tool request, one after tool result.
        assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
        assert_eq!(final_msg.content, "final answer");

        // Drain events and assert tool call lifecycle happened.
        let mut saw_start = false;
        let mut saw_result = false;
        while let Ok(event) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
            match event {
                Some(AgentEvent::ToolCallStart { .. }) => saw_start = true,
                Some(AgentEvent::ToolCallResult { .. }) => saw_result = true,
                Some(AgentEvent::Done { .. }) => break,
                Some(_) => {}
                None => break,
            }
        }

        assert!(saw_start, "expected ToolCallStart event");
        assert!(saw_result, "expected ToolCallResult event");
    }
}
