//! Agent executor — ReAct-style reasoning loop with streaming and tool dispatch.

use std::time::Duration;

use futures::{StreamExt, future::join_all};
use tracing::{debug, warn, info_span, Instrument};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::conversation::ConversationMessage;
use crate::conversation::memory::{estimate_message_tokens, estimate_tokens, model_context_window, trim_to_context_window};
use crate::conversation::summarizer;
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{
    CompletionRequest, LlmProvider, Message, ContentPart, ProviderType, ReasoningEffort, Role, ToolCallDelta,
    ToolCallRequest, Usage,
};
use crate::privacy;
use crate::tools::ToolRegistry;

pub mod context;

// Re-export so consumers don't need to depend on tokio-util directly.
pub use tokio_util::sync::CancellationToken;

/// Maximum characters to keep in a tool result for LLM context.
/// ~4K tokens ≈ 16K chars for English text.
const MAX_TOOL_RESULT_CHARS: usize = 16_000;

/// Truncate tool result content to fit within a character budget.
///
/// Uses intelligent compression strategies before falling back to hard
/// truncation:
///   1. JSON arrays  — truncate long string values, then drop tail items.
///   2. Section-based text (--- / ===) — keep first & last sections fully,
///      truncate middle sections.
///   3. Fallback — keep beginning + end with a gap note.
fn truncate_tool_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    // Try intelligent compression first.
    if let Some(compressed) = try_smart_compress(content, max_chars) {
        if compressed.len() <= max_chars {
            return compressed;
        }
    }

    // Fallback: keep beginning + end (char-boundary–safe).
    let keep_each = max_chars / 2 - 100; // 100 chars reserved for separator
    let mut start_cut = keep_each;
    while !content.is_char_boundary(start_cut) {
        start_cut -= 1;
    }
    // Try to land on a line break for readability.
    if let Some(nl) = content[..start_cut].rfind('\n') {
        start_cut = nl;
    }

    let mut end_start = content.len() - keep_each;
    while !content.is_char_boundary(end_start) {
        end_start += 1;
    }
    if let Some(nl) = content[end_start..].find('\n') {
        end_start += nl + 1;
    }

    format!(
        "{}\n\n[... {} chars omitted ...]\n\n{}",
        &content[..start_cut],
        content.len() - start_cut - (content.len() - end_start),
        &content[end_start..]
    )
}

// ---------------------------------------------------------------------------
// Smart compression helpers
// ---------------------------------------------------------------------------

/// Attempt to compress the result using structure-aware strategies.
fn try_smart_compress(result: &str, max_chars: usize) -> Option<String> {
    let trimmed = result.trim();

    // JSON array → compress entries.
    if trimmed.starts_with('[') {
        return compress_json_array(trimmed, max_chars);
    }

    // Section-delimited text → compress middle sections.
    if trimmed.contains("---") || trimmed.contains("===") {
        return compress_sections(trimmed);
    }

    None
}

/// Compress a JSON array by truncating long string values inside each item,
/// then dropping trailing items if the total still exceeds the budget.
fn compress_json_array(json_str: &str, max_chars: usize) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let arr = parsed.as_array()?;
    let total = arr.len();
    let mut compressed: Vec<serde_json::Value> = Vec::with_capacity(total);

    for (i, item) in arr.iter().enumerate() {
        let item_json = serde_json::to_string(item).ok()?;
        if item_json.len() > 500 {
            compressed.push(truncate_json_values(item, 500));
        } else {
            compressed.push(item.clone());
        }

        // Check cumulative size periodically (every item for small arrays,
        // every 5th item for larger ones).
        if total < 20 || (i + 1) % 5 == 0 || i == total - 1 {
            let current_len: usize = compressed
                .iter()
                .filter_map(|v| serde_json::to_string(v).ok())
                .map(|s| s.len())
                .sum();
            if current_len > max_chars.saturating_sub(200) {
                let remaining = total - i - 1;
                let out =
                    serde_json::to_string_pretty(&serde_json::Value::Array(compressed)).ok()?;
                return Some(format!("{}\n[... {} more items omitted]", out, remaining));
            }
        }
    }

    serde_json::to_string_pretty(&serde_json::Value::Array(compressed)).ok()
}

/// Recursively truncate string values inside a JSON value.
fn truncate_json_values(value: &serde_json::Value, max_str_len: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max_str_len {
                // Find a char-boundary–safe cut point.
                let mut cut = max_str_len;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                serde_json::Value::String(format!("{}...[truncated]", &s[..cut]))
            } else {
                value.clone()
            }
        }
        serde_json::Value::Object(map) => {
            let new_map: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), truncate_json_values(v, max_str_len)))
                .collect();
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| truncate_json_values(v, max_str_len))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Compress section-delimited text by keeping first & last sections fully and
/// truncating middle sections to 300 chars each.
fn compress_sections(text: &str) -> Option<String> {
    let separator = if text.contains("---") { "---" } else { "===" };
    let sections: Vec<&str> = text.split(separator).collect();

    if sections.len() < 3 {
        return None;
    }

    let mut result: Vec<String> = Vec::with_capacity(sections.len());
    for (i, section) in sections.iter().enumerate() {
        if i == 0 || i == sections.len() - 1 {
            result.push(section.to_string());
        } else {
            let trimmed = section.trim();
            if trimmed.len() > 300 {
                // Char-boundary–safe cut.
                let mut cut = 300;
                while !trimmed.is_char_boundary(cut) {
                    cut -= 1;
                }
                result.push(format!("{}...", &trimmed[..cut]));
            } else {
                result.push(trimmed.to_string());
            }
        }
    }

    let compressed = result.join(&format!("\n{}\n", separator));
    if compressed.len() < text.len() {
        Some(compressed)
    } else {
        None
    }
}

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
        /// Whether this response came from the answer cache.
        #[serde(default)]
        cached: bool,
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
    /// Whether to enable reasoning/thinking for models that support it.
    pub reasoning_enabled: Option<bool>,
    /// Thinking budget in tokens (Anthropic, Gemini).
    pub thinking_budget: Option<u32>,
    /// Reasoning effort level (OpenAI o-series).
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Provider type hint — passed through to CompletionRequest.
    pub provider_type: Option<ProviderType>,
    /// Optional cheaper model name for summarization (e.g. "gpt-4o-mini").
    /// Falls back to main model when `None`.
    pub summarization_model: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 6,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            model: None,
            temperature: Some(0.3),
            max_tokens: Some(4096),
            context_window: None,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: None,
            summarization_model: None,
        }
    }
}

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/system.md");

const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// Internal result of a direct-dispatch pattern match.
struct DirectDispatch {
    tool_name: String,
    arguments: String,
}

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
    /// Optional separate provider for summarization (cheaper model).
    summarization_provider: Option<Box<dyn LlmProvider>>,
    tools: ToolRegistry,
    config: AgentConfig,
    cancel_token: CancellationToken,
}

impl AgentExecutor {
    /// Create a new executor from a provider, tool registry, and config.
    pub fn new(provider: Box<dyn LlmProvider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            provider,
            summarization_provider: None,
            tools,
            config,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Attach a cancellation token for cooperative cancellation.
    ///
    /// When the token is cancelled, the agent will stop at the next
    /// checkpoint, save any partial conversation, and return gracefully.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    /// Attach a separate LLM provider for summarization (cheaper model).
    ///
    /// When set, context-window summarization will use this provider
    /// instead of the main one, saving cost on a task that doesn't
    /// need the full model's reasoning ability.
    pub fn with_summarization_provider(mut self, provider: Box<dyn LlmProvider>) -> Self {
        self.summarization_provider = Some(provider);
        self
    }

    /// Run the agent loop for a single user turn.
    ///
    /// * `history` — prior conversation messages (already stored in DB).
    /// * `user_parts` — content parts for the new user input (text + optional images).
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
        user_parts: Vec<ContentPart>,
        db: &Database,
        conversation_id: Option<&str>,
        tx: mpsc::Sender<AgentEvent>,
        next_sort_order: i64,
    ) -> Result<Message, CoreError> {
        let model = self.config.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let max_response_tokens = self.config.max_tokens.unwrap_or(4096);

        // --- 0. Early cancellation check before any work ----------------------
        if self.cancel_token.is_cancelled() {
            let msg = Message::text(Role::Assistant, "Request cancelled by user.".to_string());
            let _ = tx.send(AgentEvent::Done {
                message: msg.clone(),
                usage_total: Usage::default(),
                cached: false,
            }).await;
            return Ok(msg);
        }

        // --- 0b. Pre-summarize evicted history if context is getting full -----
        let history = self
            .summarize_if_needed(history, model, max_response_tokens)
            .await;

        // --- 1. Build initial messages with context-window trimming -----------
        let mut messages = context::prepare_messages(
            &self.config.system_prompt,
            &history,
            &user_parts,
            model,
            max_response_tokens,
            self.config.context_window,
        );

        // --- 2. Privacy redaction on outgoing user content --------------------
        let privacy_cfg = db.load_privacy_config().unwrap_or_default();
        if privacy_cfg.enabled {
            for msg in &mut messages {
                if msg.role == Role::User {
                    for part in &mut msg.parts {
                        if let ContentPart::Text { text } = part {
                            *text = privacy::redact_content(text, &privacy_cfg.redact_patterns);
                        }
                    }
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
        let mut thinking_buffer = String::new();

        // --- 3c. Extract user query text and build cache key -----------------
        let user_query_text: String = user_parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        let cache_source_filter: Option<String> = if source_scope.is_empty() {
            None
        } else {
            let mut sorted = source_scope.clone();
            sorted.sort();
            Some(sorted.join(","))
        };

        // --- 3c'. Try direct dispatch (skip LLM for simple commands) ---------
        if let Some(msg) = self
            .try_direct_dispatch(
                &user_query_text,
                db,
                &source_scope,
                &tx,
                conversation_id,
                next_sort_order,
            )
            .await
        {
            return Ok(msg);
        }

        // --- 3d. Check answer cache before ReAct loop ------------------------
        if !user_query_text.is_empty() {
            if let Ok(Some(cached)) = db.find_cached_answer(
                &user_query_text,
                cache_source_filter.as_deref(),
                None,
            ) {
                let _ = db.increment_cache_hit(&cached.id);
                debug!("Cache hit for query: {}", user_query_text);
                let _ = tx
                    .send(AgentEvent::TextDelta {
                        delta: cached.answer_text.clone(),
                    })
                    .await;
                let msg = Message::text(Role::Assistant, cached.answer_text);

                // Save cached response to conversation history.
                if let Some(cid) = conversation_id {
                    let conv_msg = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: cid.to_string(),
                        role: Role::Assistant,
                        content: msg.text_content(),
                        tool_call_id: None,
                        tool_calls: vec![],
                        token_count: estimate_message_tokens(&msg),
                        created_at: String::new(),
                        sort_order,
                        thinking: None,
                    };
                    let _ = db.add_message(&conv_msg);
                }

                let _ = tx
                    .send(AgentEvent::Done {
                        message: msg.clone(),
                        usage_total: Usage::default(),
                        cached: true,
                    })
                    .await;
                return Ok(msg);
            }
        }

        // Macro for cancellation checkpoints — saves partial conversation and
        // returns gracefully when the token is cancelled.
        macro_rules! check_cancelled {
            () => {
                if self.cancel_token.is_cancelled() {
                    warn!("Agent execution cancelled by user");
                    if !accumulated_content.is_empty() {
                        let note = "\n\n*[Request cancelled by user]*";
                        let _ = tx.send(AgentEvent::TextDelta { delta: note.to_string() }).await;
                        accumulated_content.push_str(note);
                    }
                    let cancel_text = if accumulated_content.is_empty() {
                        "Request cancelled by user.".to_string()
                    } else {
                        accumulated_content.clone()
                    };
                    let final_msg = Message::text(Role::Assistant, cancel_text);
                    if let Some(cid) = conversation_id {
                        let conv_msg = ConversationMessage {
                            id: Uuid::new_v4().to_string(),
                            conversation_id: cid.to_string(),
                            role: Role::Assistant,
                            content: final_msg.text_content(),
                            tool_call_id: None,
                            tool_calls: vec![],
                            token_count: estimate_message_tokens(&final_msg),
                            created_at: String::new(),
                            sort_order,
                            thinking: if thinking_buffer.is_empty() { None } else { Some(thinking_buffer.clone()) },
                        };
                        let _ = db.add_message(&conv_msg);
                    }
                    let _ = tx.send(AgentEvent::Done {
                        message: final_msg.clone(),
                        usage_total: total_usage.clone(),
                        cached: false,
                    }).await;
                    return Ok(final_msg);
                }
            };
        }

        // --- 4. ReAct loop ----------------------------------------------------
        for iteration in 0..self.config.max_iterations {
            // ── Cancellation checkpoint: before LLM call ─────────────────
            check_cancelled!();
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
                thinking_budget: if self.config.reasoning_enabled.unwrap_or(false) {
                    Some(self.config.thinking_budget.unwrap_or(10_000))
                } else {
                    None
                },
                reasoning_effort: if self.config.reasoning_enabled.unwrap_or(false) {
                    self.config.reasoning_effort.clone()
                } else {
                    None
                },
                provider_type: self.config.provider_type.clone(),
            };

            // -- 4a. Stream LLM response (with rate-limit retry) ----------------
            const MAX_LLM_RETRIES: u32 = 3;
            let mut retry_count = 0u32;
            let mut stream = loop {
                match self.provider.stream(&request).await {
                    Ok(s) => break s,
                    Err(CoreError::RateLimited { retry_after_secs }) => {
                        retry_count += 1;
                        if retry_count > MAX_LLM_RETRIES {
                            let _ = tx
                                .send(AgentEvent::Error {
                                    message: format!(
                                        "Rate limited after {} retries",
                                        MAX_LLM_RETRIES
                                    ),
                                })
                                .await;
                            return Err(CoreError::RateLimited { retry_after_secs });
                        }
                        // Use server's Retry-After, falling back to exponential backoff.
                        let wait = if retry_after_secs > 0 {
                            retry_after_secs
                        } else {
                            2u64.pow(retry_count)
                        };
                        warn!(
                            "Rate limited. Retry {}/{} after {}s",
                            retry_count, MAX_LLM_RETRIES, wait
                        );
                        let _ = tx
                            .send(AgentEvent::Thinking {
                                content: format!("Rate limited. Retrying in {}s…", wait),
                            })
                            .await;
                        tokio::time::sleep(Duration::from_secs(wait)).await;
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
            };

            let mut full_content = String::new();
            let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
            let mut chunk_usage: Option<Usage> = None;
            let mut iteration_thinking = String::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        // Forward thinking deltas.
                        if let Some(ref thinking) = chunk.thinking_delta {
                            if !thinking.is_empty() {
                                iteration_thinking.push_str(thinking);
                                let _ = tx
                                    .send(AgentEvent::Thinking {
                                        content: thinking.clone(),
                                    })
                                    .await;
                            }
                        }
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
                if let Some(t) = u.thinking_tokens {
                    *total_usage.thinking_tokens.get_or_insert(0) += t;
                }
            }

            // Merge iteration thinking into buffer
            if !iteration_thinking.is_empty() {
                if !thinking_buffer.is_empty() {
                    thinking_buffer.push('\n');
                }
                thinking_buffer.push_str(&iteration_thinking);
            }

            // -- 4c. Build assistant message -----------------------------------
            let assistant_msg = Message {
                role: Role::Assistant,
                parts: vec![ContentPart::Text { text: full_content }],
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
                        content: assistant_msg.text_content(),
                        tool_call_id: None,
                        tool_calls: assistant_msg.tool_calls.clone().unwrap_or_default(),
                        token_count: estimate_message_tokens(&assistant_msg),
                        created_at: String::new(),
                        sort_order,
                        thinking: if thinking_buffer.is_empty() { None } else { Some(thinking_buffer.clone()) },
                    };
                    if let Err(e) = db.add_message(&conv_msg) {
                        warn!("Failed to save final assistant message: {e}");
                    }
                }

                // Cache the answer if it contains citations (used the knowledge base).
                let final_text = assistant_msg.text_content();
                if !final_text.is_empty() && !user_query_text.is_empty() {
                    let citations = crate::cache::extract_citations(&final_text);
                    if !citations.is_empty() {
                        let _ = db.cache_answer(
                            &user_query_text,
                            &final_text,
                            &citations,
                            cache_source_filter.as_deref(),
                        );
                    }
                }

                let _ = tx
                    .send(AgentEvent::Done {
                        message: assistant_msg.clone(),
                        usage_total: total_usage,
                        cached: false,
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
                    content: assistant_msg.text_content(),
                    tool_call_id: None,
                    tool_calls: tool_calls.clone(),
                    token_count: estimate_message_tokens(&assistant_msg),
                    created_at: String::new(),
                    sort_order,
                    thinking: if iteration_thinking.is_empty() { None } else { Some(iteration_thinking.clone()) },
                };
                if let Err(e) = db.add_message(&conv_msg) {
                    warn!("Failed to save intermediate assistant message: {e}");
                }
                sort_order += 1;
            }

            // ── Cancellation checkpoint: before tool execution ────────
            check_cancelled!();

            // -- 4e. Execute tool calls in parallel ------------------------------
            // Emit ToolCallStart events for all tools before launching.
            for tc in &tool_calls {
                let _ = tx
                    .send(AgentEvent::ToolCallStart {
                        call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .await;
            }

            // Build futures for all tool calls and execute concurrently.
            let tool_futures: Vec<_> = tool_calls.iter().map(|tc| {
                let tool_timeout = match tc.name.as_str() {
                    "retrieve_evidence" => Duration::from_secs(60),
                    _ => Duration::from_secs(30),
                };
                let source_scope = &source_scope;
                let tool_span = info_span!("tool_execution", tool = %tc.name);
                async move {
                    let result = tokio::time::timeout(
                        tool_timeout,
                        self.tools.execute(&tc.name, &tc.id, &tc.arguments, db, source_scope),
                    )
                    .await;
                    (tc, tool_timeout, result)
                }.instrument(tool_span)
            }).collect();

            let tool_results = join_all(tool_futures).await;

            // Process results in original order (join_all preserves order).
            for (tc, tool_timeout, tool_result) in tool_results {
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
                        token_count: estimate_tokens(&content),
                        created_at: String::new(),
                        sort_order,
                        thinking: None,
                    };
                    if let Err(e) = db.add_message(&tool_conv_msg) {
                        warn!("Failed to save tool result message: {e}");
                    }
                    sort_order += 1;
                }

                // Truncate large tool results for LLM context to prevent
                // crowding out conversation history.
                let context_content = truncate_tool_result(&content, MAX_TOOL_RESULT_CHARS);

                messages.push(Message::text_with_name(Role::Tool, context_content, tc.id.clone()));
            }

            // ── Cancellation checkpoint: after tool execution ─────────
            check_cancelled!();

            // Re-trim messages to fit context window after appending tool results.
            // This prevents unbounded growth across iterations.
            let max_ctx = self.config.context_window.unwrap_or_else(|| model_context_window(model));
            messages = trim_to_context_window(&messages, max_ctx, max_response_tokens);

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

        let final_msg = Message::text(Role::Assistant, accumulated_content);

        if let Some(cid) = conversation_id {
            let conv_msg = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                token_count: estimate_message_tokens(&final_msg),
                created_at: String::new(),
                sort_order,
                thinking: if thinking_buffer.is_empty() { None } else { Some(thinking_buffer.clone()) },
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save final assistant message: {e}");
            }
        }

        let _ = tx
            .send(AgentEvent::Done {
                message: final_msg.clone(),
                usage_total: total_usage,
                cached: false,
            })
            .await;
        Ok(final_msg)
    }

    // -----------------------------------------------------------------------
    // Pre-summarization helper
    // -----------------------------------------------------------------------

    /// If the conversation history is large enough to trigger eviction,
    /// use the LLM to produce an abstractive summary of the messages that
    /// *would* be evicted, then replace those messages with a single
    /// `System` summary message.  This keeps more nuance than the
    /// extractive (truncation-based) recap in `context.rs`.
    ///
    /// The method is intentionally conservative: it only fires when the
    /// total estimated token count exceeds 50% of the context window so
    /// that short conversations are unaffected.
    async fn summarize_if_needed(
        &self,
        history: Vec<Message>,
        model: &str,
        max_response_tokens: u32,
    ) -> Vec<Message> {
        if history.is_empty() {
            return history;
        }

        let ctx_window = self
            .config
            .context_window
            .unwrap_or_else(|| model_context_window(model));

        // Budget available for history (context window minus response reservation).
        let budget = ctx_window.saturating_sub(max_response_tokens);
        if budget == 0 {
            return history;
        }

        // Estimate total tokens across the history.
        let total_tokens: u32 = history.iter().map(|m| estimate_message_tokens(m)).sum();

        // Only trigger when history consumes >50% of available budget.
        if total_tokens <= budget / 2 {
            return history;
        }

        // Figure out which messages would be evicted by trim_to_context_window.
        // That function keeps the system message + newest messages. We simulate
        // it to identify the split point.
        let trimmed = trim_to_context_window(&history, ctx_window, max_response_tokens);
        let kept_count = trimmed.len();
        let evict_count = history.len().saturating_sub(kept_count);

        if evict_count == 0 {
            return history;
        }

        let evicted = &history[..evict_count];

        // Build the extractive fallback first (cheap, in-process).
        let extractive_fallback = context::build_evicted_recap_from_messages(evicted);

        // Attempt LLM summarization.
        // Use dedicated summarization provider/model if configured,
        // otherwise fall back to the main provider and model.
        let summ_provider: &dyn LlmProvider = self
            .summarization_provider
            .as_deref()
            .unwrap_or(self.provider.as_ref());
        let summ_model = self
            .config
            .summarization_model
            .as_deref()
            .unwrap_or(model);
        let summary = summarizer::summarize_evicted_messages(
            summ_provider,
            summ_model,
            evicted,
            &extractive_fallback,
        )
        .await;

        // Build a replacement history: summary message + surviving messages.
        let mut new_history =
            Vec::with_capacity(1 + history.len() - evict_count);
        new_history.push(Message::text(
            Role::System,
            format!(
                "## Earlier conversation context (summarized)\n\
                 The following is a summary of earlier conversation turns that \
                 were condensed to save context space:\n{}",
                summary
            ),
        ));
        new_history.extend_from_slice(&history[evict_count..]);
        new_history
    }

    // -----------------------------------------------------------------------
    // Direct dispatch — skip LLM for simple commands
    // -----------------------------------------------------------------------

    /// Attempt to handle the query without an LLM call by detecting simple,
    /// unambiguous command patterns. Returns `Some(Message)` if handled
    /// directly, `None` to fall through to the normal ReAct loop.
    async fn try_direct_dispatch(
        &self,
        user_text: &str,
        db: &Database,
        source_scope: &[String],
        tx: &mpsc::Sender<AgentEvent>,
        conversation_id: Option<&str>,
        sort_order: i64,
    ) -> Option<Message> {
        if user_text.is_empty() {
            return None;
        }

        let dispatch = self.match_direct_pattern(user_text, db)?;

        debug!(
            "Direct dispatch: tool={}, args={}",
            dispatch.tool_name, dispatch.arguments
        );

        let call_id = format!("direct_{}", Uuid::new_v4());

        // Emit ToolCallStart so the frontend shows tool-call UI.
        let _ = tx
            .send(AgentEvent::ToolCallStart {
                call_id: call_id.clone(),
                tool_name: dispatch.tool_name.clone(),
                arguments: dispatch.arguments.clone(),
            })
            .await;

        // Execute the tool directly.
        let result = self
            .tools
            .execute(
                &dispatch.tool_name,
                &call_id,
                &dispatch.arguments,
                db,
                source_scope,
            )
            .await;

        match result {
            Ok(tool_result) => {
                let _ = tx
                    .send(AgentEvent::ToolCallResult {
                        call_id: tool_result.call_id.clone(),
                        tool_name: dispatch.tool_name.clone(),
                        content: tool_result.content.clone(),
                        is_error: tool_result.is_error,
                        artifacts: tool_result.artifacts.clone(),
                    })
                    .await;

                if tool_result.is_error {
                    // Tool returned an error — fall through to LLM for
                    // a better user-facing response.
                    return None;
                }

                // Emit the content as text so streaming listeners see it.
                let _ = tx
                    .send(AgentEvent::TextDelta {
                        delta: tool_result.content.clone(),
                    })
                    .await;

                let msg = Message::text(Role::Assistant, tool_result.content);

                // Persist the assistant message.
                if let Some(cid) = conversation_id {
                    let conv_msg = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: cid.to_string(),
                        role: Role::Assistant,
                        content: msg.text_content(),
                        tool_call_id: None,
                        tool_calls: vec![],
                        token_count: estimate_message_tokens(&msg),
                        created_at: String::new(),
                        sort_order,
                        thinking: None,
                    };
                    let _ = db.add_message(&conv_msg);
                }

                let _ = tx
                    .send(AgentEvent::Done {
                        message: msg.clone(),
                        usage_total: Usage::default(),
                        cached: false,
                    })
                    .await;

                Some(msg)
            }
            Err(e) => {
                warn!("Direct dispatch failed ({}): {}", dispatch.tool_name, e);
                None // Fall through to LLM
            }
        }
    }

    /// Match user query text against known direct-dispatch patterns.
    ///
    /// Only matches CLEAR, unambiguous commands. Anything vague or
    /// conversational falls through to the LLM.
    fn match_direct_pattern(&self, user_text: &str, db: &Database) -> Option<DirectDispatch> {
        let q = user_text.trim().to_lowercase();
        let q = q
            .trim_end_matches(|c: char| ".?!\u{3002}\u{ff1f}\u{ff01}".contains(c))
            .trim();

        // Strip common polite prefixes/suffixes.
        let q = q.strip_prefix("please ").unwrap_or(q);
        let q = q.strip_prefix("can you ").unwrap_or(q);
        let q = q.strip_prefix("could you ").unwrap_or(q);
        let q = q.strip_suffix(" please").unwrap_or(q);
        let q = q.strip_prefix('\u{8BF7}').unwrap_or(q); // 请

        // --- List sources (no arguments) ------------------------------------
        const LIST_SOURCES: &[&str] = &[
            "list sources",
            "list my sources",
            "show sources",
            "show my sources",
            "show all sources",
            "what sources do i have",
            "what are my sources",
            "\u{663E}\u{793A}\u{6570}\u{636E}\u{6E90}",     // 显示数据源
            "\u{5217}\u{51FA}\u{6570}\u{636E}\u{6E90}",     // 列出数据源
            "\u{67E5}\u{770B}\u{6570}\u{636E}\u{6E90}",     // 查看数据源
            "\u{6570}\u{636E}\u{6E90}\u{5217}\u{8868}",     // 数据源列表
            "\u{30BD}\u{30FC}\u{30B9}\u{4E00}\u{89A7}",     // ソース一覧
            "\u{30BD}\u{30FC}\u{30B9}\u{3092}\u{8868}\u{793A}", // ソースを表示
        ];
        if LIST_SOURCES.iter().any(|p| q == *p) {
            return Some(DirectDispatch {
                tool_name: "list_sources".into(),
                arguments: "{}".into(),
            });
        }

        // --- List playbooks (action: list) ----------------------------------
        const LIST_PLAYBOOKS: &[&str] = &[
            "list playbooks",
            "list my playbooks",
            "show playbooks",
            "show my playbooks",
            "what playbooks do i have",
            "what are my playbooks",
            "\u{663E}\u{793A}\u{5267}\u{672C}",             // 显示剧本
            "\u{5217}\u{51FA}\u{5267}\u{672C}",             // 列出剧本
            "\u{67E5}\u{770B}\u{5267}\u{672C}",             // 查看剧本
            "\u{5267}\u{672C}\u{5217}\u{8868}",             // 剧本列表
            "\u{30D7}\u{30EC}\u{30A4}\u{30D6}\u{30C3}\u{30AF}\u{4E00}\u{89A7}", // プレイブック一覧
        ];
        if LIST_PLAYBOOKS.iter().any(|p| q == *p) {
            return Some(DirectDispatch {
                tool_name: "manage_playbook".into(),
                arguments: r#"{"action":"list"}"#.into(),
            });
        }

        // --- Browse directory (extract path) --------------------------------
        let path = None
            .or_else(|| q.strip_prefix("ls "))
            .or_else(|| q.strip_prefix("dir "))
            .or_else(|| q.strip_prefix("browse "))
            .or_else(|| q.strip_prefix("list directory "))
            .or_else(|| q.strip_prefix("list dir "));

        if let Some(raw_path) = path {
            let raw_path = raw_path.trim().trim_matches('"').trim_matches('\'');
            if !raw_path.is_empty() {
                let escaped = serde_json::to_string(raw_path)
                    .unwrap_or_else(|_| format!("\"{}\"", raw_path));
                return Some(DirectDispatch {
                    tool_name: "list_dir".into(),
                    arguments: format!(r#"{{"path":{}}}"#, escaped),
                });
            }
        }

        // --- List documents in source (resolve source name → ID) ------------
        let source_phrase = None
            .or_else(|| q.strip_prefix("list files in "))
            .or_else(|| q.strip_prefix("show files in "))
            .or_else(|| q.strip_prefix("list documents in "))
            .or_else(|| q.strip_prefix("show documents in "))
            .or_else(|| q.strip_suffix("\u{91CC}\u{7684}\u{6587}\u{4EF6}")) // 里的文件
            .or_else(|| q.strip_suffix("\u{306E}\u{30D5}\u{30A1}\u{30A4}\u{30EB}")); // のファイル

        if let Some(source_name) = source_phrase {
            let source_name = source_name.trim().trim_matches('"').trim_matches('\'');
            if !source_name.is_empty() {
                if let Ok(sources) = db.list_sources() {
                    let name_lower = source_name.to_lowercase();
                    let matches: Vec<_> = sources
                        .iter()
                        .filter(|s| {
                            let root_lower = s.root_path.to_lowercase();
                            s.id == source_name
                                || root_lower.ends_with(&name_lower)
                                || root_lower.contains(&name_lower)
                        })
                        .collect();

                    if matches.len() == 1 {
                        let source_id =
                            serde_json::to_string(&matches[0].id).unwrap_or_default();
                        return Some(DirectDispatch {
                            tool_name: "list_documents".into(),
                            arguments: format!(r#"{{"source_id":{}}}"#, source_id),
                        });
                    }
                    // 0 or >1 matches → ambiguous, fall through to LLM.
                }
            }
        }

        None
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
        assert_eq!(cfg.max_iterations, 6);
        assert!(cfg.system_prompt.contains("knowledge recall engine"));
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
            .run(vec![], vec![ContentPart::Text { text: "hello".to_string() }], &db, None, tx, 0)
            .await
            .expect("run should succeed");

        // Should perform two LLM calls: one for tool request, one after tool result.
        assert_eq!(stream_calls.load(Ordering::SeqCst), 2);
        assert_eq!(final_msg.text_content(), "final answer");

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
