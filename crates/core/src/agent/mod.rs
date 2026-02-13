//! Agent executor — ReAct-style reasoning loop with streaming and tool dispatch.

use futures::StreamExt;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{
    CompletionRequest, FinishReason, LlmProvider, Message, Role, ToolCallDelta,
    ToolCallRequest, Usage,
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
        call_id: String,
        tool_name: String,
        arguments: String,
    },
    /// Result of a tool execution.
    ToolCallResult {
        call_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
        artifacts: Option<serde_json::Value>,
    },
    /// Thinking / chain-of-thought text (if the model supports it).
    Thinking { content: String },
    /// The agent finished producing a final answer.
    Done { message: Message, usage_total: Usage },
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

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are Ask Myself, a personal knowledge assistant grounded in the user's local knowledge base. You MUST search before answering any factual question.

## Tools Available
- **search_knowledge_base**: Search for relevant documents using full-text and vector search. Always start here.
- **summarize_evidence**: Retrieve specific chunks by ID for detailed citation.
- **read_file**: Read a full file from the knowledge base when you need more context.
- **manage_playbook**: Create or manage evidence collections (playbooks) when asked.

## Rules
1. ALWAYS call search_knowledge_base before answering factual questions. Do not rely on prior knowledge.
2. If search returns no relevant results, clearly state "I couldn't find information about this in your knowledge base." Do NOT fabricate or guess answers.
3. Cite sources inline using [source: path/to/document] format after each claim.
4. When multiple sources agree, synthesize them. When they conflict, note the discrepancy and present both.
5. For follow-up questions, use prior conversation context but search again if the topic shifts.
6. Be concise and direct. Use markdown formatting (headers, lists, bold) for readability.
7. When asked to create a playbook, use manage_playbook to create it and add relevant citations.
8. You can call multiple tools in sequence: search first, then summarize or read specific results for deeper context."#;

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
    pub fn new(
        provider: Box<dyn LlmProvider>,
        tools: ToolRegistry,
        config: AgentConfig,
    ) -> Self {
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
    ///
    /// Returns the final assistant [`Message`] on success.
    pub async fn run(
        &self,
        history: Vec<Message>,
        user_message: String,
        db: &Database,
        conversation_id: Option<&str>,
        tx: mpsc::Sender<AgentEvent>,
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

        // --- 4. ReAct loop ----------------------------------------------------
        for iteration in 0..self.config.max_iterations {
            debug!("Agent iteration {}/{}", iteration + 1, self.config.max_iterations);

            let request = CompletionRequest {
                model: model.to_string(),
                messages: messages.clone(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
                tools: tools_param.clone(),
                stop: None,
            };

            // -- 4a. Stream LLM response --------------------------------------
            let mut stream = self.provider.stream(&request).await?;

            let mut full_content = String::new();
            let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
            let mut finish_reason: Option<FinishReason> = None;
            let mut chunk_usage: Option<Usage> = None;

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        // Forward text deltas.
                        if !chunk.delta.is_empty() {
                            full_content.push_str(&chunk.delta);
                            let _ = tx
                                .send(AgentEvent::TextDelta {
                                    delta: chunk.delta,
                                })
                                .await;
                        }
                        // Accumulate tool-call deltas.
                        if let Some(ref tc_delta) = chunk.tool_call_delta {
                            accumulate_tool_call(&mut tool_calls, tc_delta);
                        }
                        if let Some(fr) = chunk.finish_reason {
                            finish_reason = Some(fr);
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
            if tool_calls.is_empty()
                || matches!(finish_reason, Some(FinishReason::Stop) | Some(FinishReason::Length))
            {
                let _ = tx
                    .send(AgentEvent::Done {
                        message: assistant_msg.clone(),
                        usage_total: total_usage,
                    })
                    .await;
                return Ok(assistant_msg);
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

                let tool_msg = match self
                    .tools
                    .execute(&tc.name, &tc.id, &tc.arguments, db, &source_scope)
                    .await
                {
                    Ok(result) => {
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
                    Err(e) => {
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
                };

                // Redact tool output before adding to context.
                let content = if privacy_cfg.enabled {
                    privacy::redact_content(&tool_msg, &privacy_cfg.redact_patterns)
                } else {
                    tool_msg
                };

                messages.push(Message {
                    role: Role::Tool,
                    content,
                    name: Some(tc.id.clone()),
                    tool_calls: None,
                });
            }
            // Loop back → next LLM call with tool results.
        }

        let err_msg = format!(
            "Agent exceeded maximum iterations ({})",
            self.config.max_iterations
        );
        warn!("{}", err_msg);
        let _ = tx
            .send(AgentEvent::Error {
                message: err_msg.clone(),
            })
            .await;
        Err(CoreError::Agent(err_msg))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Accumulate streaming tool-call deltas into complete [`ToolCallRequest`]s.
///
/// OpenAI sends each tool call across multiple SSE chunks:
/// - First chunk provides `id` + `name`
/// - Subsequent chunks append to `arguments_delta`
///
/// When `id` is non-empty we either update an existing entry or create a new one.
/// When `id` is empty the delta is appended to the most recent tool call.
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
    use super::*;

    #[test]
    fn test_accumulate_new_tool_call() {
        let mut calls = Vec::new();
        let delta = ToolCallDelta {
            id: "call_1".into(),
            name: Some("search".into()),
            arguments_delta: r#"{"qu"#.into(),
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
            },
        );
        accumulate_tool_call(
            &mut calls,
            &ToolCallDelta {
                id: "call_2".into(),
                name: Some("file".into()),
                arguments_delta: "{}".into(),
            },
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[1].name, "file");
    }

    #[test]
    fn test_default_config() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.max_iterations, 10);
        assert!(cfg.system_prompt.contains("knowledge assistant"));
        assert_eq!(cfg.temperature, Some(0.3));
        assert_eq!(cfg.max_tokens, Some(4096));
    }
}
