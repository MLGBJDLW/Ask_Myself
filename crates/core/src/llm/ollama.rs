//! Ollama LLM provider (local).
//!
//! Communicates with a local Ollama server using its native `/api/chat` endpoint.
//! Streaming uses NDJSON (newline-delimited JSON), not SSE.
//! Message format is similar to OpenAI but with Ollama-specific response fields.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{
    CompletionRequest, CompletionResponse, FinishReason, LlmProvider, Message, ProviderConfig,
    Role, StreamChunk, ToolCallDelta, ToolCallRequest, ToolDefinition, Usage,
};
use crate::error::CoreError;

const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_TIMEOUT_SECS: u64 = 300; // Local models can be slow on first load.

// ---------------------------------------------------------------------------
// Ollama API wire types — request
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCallOut>>,
}

#[derive(Serialize)]
struct OllamaToolCallOut {
    function: OllamaFunctionOut,
}

#[derive(Serialize)]
struct OllamaFunctionOut {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Serialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaToolFunction,
}

#[derive(Serialize)]
struct OllamaToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Ollama API wire types — response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OllamaResponse {
    message: Option<OllamaResponseMessage>,
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    // Duration fields (available when done=true).
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OllamaToolCallIn>>,
}

#[derive(Deserialize)]
struct OllamaToolCallIn {
    function: OllamaFunctionIn,
}

#[derive(Deserialize)]
struct OllamaFunctionIn {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaModelEntry>>,
}

#[derive(Deserialize)]
struct OllamaModelEntry {
    name: String,
}

#[derive(Deserialize)]
struct OllamaErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn role_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn convert_message(msg: &Message) -> OllamaMessage {
    let mut ollama_msg = OllamaMessage {
        role: role_str(&msg.role).to_string(),
        content: msg.content.clone(),
        tool_calls: None,
    };

    if let Some(ref calls) = msg.tool_calls {
        ollama_msg.tool_calls = Some(
            calls
                .iter()
                .map(|tc| {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                    OllamaToolCallOut {
                        function: OllamaFunctionOut {
                            name: tc.name.clone(),
                            arguments: args,
                        },
                    }
                })
                .collect(),
        );
    }

    ollama_msg
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<OllamaTool> {
    tools
        .iter()
        .map(|t| OllamaTool {
            tool_type: "function".to_string(),
            function: OllamaToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        })
        .collect()
}

fn build_request_body(request: &CompletionRequest, stream: bool) -> OllamaRequest {
    let options = if request.temperature.is_some()
        || request.max_tokens.is_some()
        || request.stop.is_some()
    {
        Some(OllamaOptions {
            temperature: request.temperature,
            num_predict: request.max_tokens,
            stop: request.stop.clone(),
        })
    } else {
        None
    };

    OllamaRequest {
        model: request.model.clone(),
        messages: request.messages.iter().map(convert_message).collect(),
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
        stream,
        options,
    }
}

fn parse_finish_reason(resp: &OllamaResponse) -> FinishReason {
    if let Some(ref reason) = resp.done_reason {
        match reason.as_str() {
            "stop" => return FinishReason::Stop,
            "length" => return FinishReason::Length,
            _ => {}
        }
    }
    // If there are tool calls, the stop reason is tool_calls.
    if let Some(ref msg) = resp.message {
        if msg.tool_calls.is_some() {
            return FinishReason::ToolCalls;
        }
    }
    if resp.done {
        FinishReason::Stop
    } else {
        FinishReason::Other
    }
}

// ---------------------------------------------------------------------------
// NDJSON stream parser
// ---------------------------------------------------------------------------

/// Parse Ollama's NDJSON streaming response.
///
/// Each line is a complete JSON object. The stream ends when `done: true` is received.
async fn parse_ollama_ndjson_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
) -> Result<(), CoreError> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.map_err(|e| CoreError::Llm(format!("Stream read error: {e}")))?;
        let text = std::str::from_utf8(&chunk)
            .map_err(|e| CoreError::Llm(format!("Invalid UTF-8 in stream: {e}")))?;
        buffer.push_str(text);

        // Process complete lines.
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            let resp: OllamaResponse = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(Err(CoreError::Llm(format!("NDJSON parse error: {e}"))))
                        .await;
                    continue;
                }
            };

            let delta = resp
                .message
                .as_ref()
                .and_then(|m| m.content.clone())
                .unwrap_or_default();

            let (finish_reason, usage) = if resp.done {
                let reason = parse_finish_reason(&resp);
                let prompt_tokens = resp.prompt_eval_count.unwrap_or(0);
                let completion_tokens = resp.eval_count.unwrap_or(0);
                (
                    Some(reason),
                    Some(Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    }),
                )
            } else {
                (None, None)
            };

            let chunk = StreamChunk {
                delta,
                tool_call_delta: None,
                finish_reason,
                usage,
            };

            if tx.send(Ok(chunk)).await.is_err() {
                return Ok(());
            }

            if let Some(tool_calls) = resp.message.as_ref().and_then(|m| m.tool_calls.as_ref()) {
                for (index, tc) in tool_calls.iter().enumerate() {
                    let tc_chunk = StreamChunk {
                        delta: String::new(),
                        tool_call_delta: Some(ToolCallDelta {
                            id: format!("call_{index}"),
                            name: Some(tc.function.name.clone()),
                            arguments_delta: serde_json::to_string(&tc.function.arguments)
                                .unwrap_or_default(),
                            index: Some(index as u32),
                        }),
                        finish_reason: None,
                        usage: None,
                    };
                    if tx.send(Ok(tc_chunk)).await.is_err() {
                        return Ok(());
                    }
                }
            }

            if resp.done {
                return Ok(());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Ollama local LLM provider.
pub struct OllamaProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl OllamaProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, CoreError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| CoreError::Llm(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self { client, config })
    }

    fn base_url(&self) -> &str {
        self.config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL)
    }

    async fn check_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, CoreError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(CoreError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<OllamaErrorResponse>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("HTTP {status}: {body}"));

        Err(CoreError::Llm(message))
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let url = format!("{}/api/tags", self.base_url());

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse tags response: {e}")))?;

        Ok(resp
            .models
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.name)
            .collect())
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let url = format!("{}/api/chat", self.base_url());
        let body = build_request_body(request, false);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: OllamaResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;

        let content = resp
            .message
            .as_ref()
            .and_then(|m| m.content.clone())
            .unwrap_or_default();

        let tool_calls = resp
            .message
            .as_ref()
            .and_then(|m| m.tool_calls.as_ref())
            .map(|tcs| {
                tcs.iter()
                    .enumerate()
                    .map(|(i, tc)| ToolCallRequest {
                        id: format!("call_{i}"),
                        name: tc.function.name.clone(),
                        arguments: serde_json::to_string(&tc.function.arguments)
                            .unwrap_or_default(),
                    })
                    .collect()
            });

        let finish_reason = parse_finish_reason(&resp);

        let prompt_tokens = resp.prompt_eval_count.unwrap_or(0);
        let completion_tokens = resp.eval_count.unwrap_or(0);
        let usage = Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        };

        Ok(CompletionResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let url = format!("{}/api/chat", self.base_url());
        let body = build_request_body(request, true);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            if let Err(e) = parse_ollama_ndjson_stream(response, tx.clone()).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        let stream = futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        // Just check if Ollama is running by listing models.
        self.list_models().await?;
        Ok(())
    }
}
