//! Google Gemini LLM provider.
//!
//! Uses the Gemini REST API with API key authentication via query parameter.
//! System prompts use top-level `systemInstruction`, roles map "assistant" → "model",
//! and tool calls use `functionCall`/`functionResponse` parts.

use std::collections::HashMap;

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

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// Gemini API wire types
// ---------------------------------------------------------------------------

/// A part in a Gemini content message. Uses untagged enum for correct JSON layout.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
enum GeminiPartV2 {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContentV2 {
    role: String,
    parts: Vec<GeminiPartV2>,
}

#[derive(Serialize)]
struct GeminiSystemInstructionV2 {
    parts: Vec<GeminiPartV2>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequestV2 {
    contents: Vec<GeminiContentV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstructionV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolSet>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolSet {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Gemini API wire types — response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Option<Vec<GeminiPartV2>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

#[derive(Deserialize)]
struct GeminiErrorResponse {
    error: GeminiErrorBody,
}

#[derive(Deserialize)]
struct GeminiErrorBody {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiListModelsResponse {
    models: Option<Vec<GeminiModel>>,
}

#[derive(Deserialize)]
struct GeminiModel {
    name: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "RECITATION" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

/// Convert unified messages to Gemini format.
/// System messages are extracted as top-level systemInstruction.
fn convert_messages(
    messages: &[Message],
) -> (Option<GeminiSystemInstructionV2>, Vec<GeminiContentV2>) {
    let mut system_parts: Vec<GeminiPartV2> = Vec::new();
    let mut contents: Vec<GeminiContentV2> = Vec::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                system_parts.push(GeminiPartV2::Text {
                    text: msg.content.clone(),
                });
            }
            Role::User => {
                contents.push(GeminiContentV2 {
                    role: "user".to_string(),
                    parts: vec![GeminiPartV2::Text {
                        text: msg.content.clone(),
                    }],
                });
            }
            Role::Assistant => {
                let mut parts: Vec<GeminiPartV2> = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(GeminiPartV2::Text {
                        text: msg.content.clone(),
                    });
                }
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                        parts.push(GeminiPartV2::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: tc.name.clone(),
                                args,
                            },
                        });
                    }
                }
                contents.push(GeminiContentV2 {
                    role: "model".to_string(),
                    parts,
                });
            }
            Role::Tool => {
                // Gemini expects function responses as user-role parts.
                let tool_ref = msg.name.clone().unwrap_or_default();
                let tool_name = tool_id_to_name.get(&tool_ref).cloned().unwrap_or(tool_ref);

                // Gemini requires an object-like payload for functionResponse.response.
                let mut response_val: serde_json::Value = serde_json::from_str(&msg.content)
                    .unwrap_or_else(|_| serde_json::json!({ "content": msg.content }));
                if !response_val.is_object() {
                    response_val = serde_json::json!({ "content": response_val });
                }

                let make_part = || GeminiPartV2::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: tool_name.clone(),
                        response: response_val.clone(),
                    },
                };

                // Append to the last user content if possible, otherwise new user message.
                let appended = if let Some(last) = contents.last_mut() {
                    if last.role == "user" {
                        last.parts.push(make_part());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !appended {
                    contents.push(GeminiContentV2 {
                        role: "user".to_string(),
                        parts: vec![make_part()],
                    });
                }
            }
        }
    }

    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(GeminiSystemInstructionV2 {
            parts: system_parts,
        })
    };

    (system_instruction, contents)
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<GeminiToolSet> {
    vec![GeminiToolSet {
        function_declarations: tools
            .iter()
            .map(|t| GeminiFunctionDeclaration {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            })
            .collect(),
    }]
}

fn build_request_body(
    request: &CompletionRequest,
    system_instruction: Option<GeminiSystemInstructionV2>,
    contents: Vec<GeminiContentV2>,
) -> GeminiRequestV2 {
    let generation_config = if request.temperature.is_some()
        || request.max_tokens.is_some()
        || request.stop.is_some()
    {
        Some(GeminiGenerationConfig {
            temperature: request.temperature,
            max_output_tokens: request.max_tokens,
            stop_sequences: request.stop.clone(),
        })
    } else {
        None
    };

    GeminiRequestV2 {
        contents,
        system_instruction,
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
        generation_config,
    }
}

/// Extract text, tool calls, finish reason, and usage from a Gemini response.
fn extract_response(resp: &GeminiResponse) -> (String, Vec<ToolCallRequest>, FinishReason, Usage) {
    let candidate = resp.candidates.as_ref().and_then(|c| c.first());

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    if let Some(candidate) = candidate {
        if let Some(ref content) = candidate.content {
            if let Some(ref parts) = content.parts {
                for (idx, part) in parts.iter().enumerate() {
                    match part {
                        GeminiPartV2::Text { text } => text_parts.push(text.clone()),
                        GeminiPartV2::FunctionCall { function_call } => {
                            tool_calls.push(ToolCallRequest {
                                id: format!("call_{idx}"),
                                name: function_call.name.clone(),
                                arguments: serde_json::to_string(&function_call.args)
                                    .unwrap_or_default(),
                            });
                        }
                        GeminiPartV2::FunctionResponse { .. } => {}
                    }
                }
            }
        }
    }

    let finish_reason = candidate
        .and_then(|c| c.finish_reason.as_deref())
        .map(parse_finish_reason)
        .unwrap_or(if tool_calls.is_empty() {
            FinishReason::Other
        } else {
            FinishReason::ToolCalls
        });

    let usage = resp
        .usage_metadata
        .as_ref()
        .map(|u| Usage {
            prompt_tokens: u.prompt_token_count.unwrap_or(0),
            completion_tokens: u.candidates_token_count.unwrap_or(0),
            total_tokens: u.total_token_count.unwrap_or(0),
        })
        .unwrap_or_default();

    (text_parts.join(""), tool_calls, finish_reason, usage)
}

// ---------------------------------------------------------------------------
// Gemini SSE stream parser
// ---------------------------------------------------------------------------

/// Parse Gemini's SSE streaming response.
///
/// Gemini streams the same JSON response shape as non-streaming, one chunk per SSE event.
async fn parse_gemini_stream(
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
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            // Extract data from SSE `data: ` lines.
            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };

            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }

            let resp: GeminiResponse = match serde_json::from_str(data) {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("Gemini SSE parse skip: {e}");
                    continue;
                }
            };

            let (text_content, tool_calls, finish_reason, usage) = extract_response(&resp);

            let has_finish = finish_reason != FinishReason::Other;

            let chunk = StreamChunk {
                delta: text_content,
                tool_call_delta: None,
                finish_reason: if has_finish {
                    Some(finish_reason)
                } else {
                    None
                },
                usage: if usage.total_tokens > 0 {
                    Some(usage)
                } else {
                    None
                },
            };

            if tx.send(Ok(chunk)).await.is_err() {
                return Ok(());
            }

            for tc in &tool_calls {
                let delta_chunk = StreamChunk {
                    delta: String::new(),
                    tool_call_delta: Some(ToolCallDelta {
                        id: tc.id.clone(),
                        name: Some(tc.name.clone()),
                        arguments_delta: tc.arguments.clone(),
                        index: tc
                            .id
                            .strip_prefix("call_")
                            .and_then(|s| s.parse::<u32>().ok()),
                    }),
                    finish_reason: None,
                    usage: None,
                };
                if tx.send(Ok(delta_chunk)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Google Gemini LLM provider.
pub struct GeminiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl GeminiProvider {
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

    fn api_key(&self) -> Result<&str, CoreError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| CoreError::Llm("Google API key not configured".to_string()))
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
        let message = serde_json::from_str::<GeminiErrorResponse>(&body)
            .map(|e| e.error.message)
            .unwrap_or_else(|_| format!("HTTP {status}: {body}"));

        Err(CoreError::Llm(message))
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let api_key = self.api_key()?;
        let url = format!("{}/models?key={}", self.base_url(), api_key);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: GeminiListModelsResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse models response: {e}")))?;

        Ok(resp
            .models
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                // Gemini returns "models/gemini-pro" — strip the prefix.
                m.name
                    .strip_prefix("models/")
                    .unwrap_or(&m.name)
                    .to_string()
            })
            .collect())
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let api_key = self.api_key()?;
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url(),
            request.model,
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: GeminiResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;

        let (content, tool_calls, finish_reason, usage) = extract_response(&resp);

        Ok(CompletionResponse {
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            finish_reason,
            usage,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let api_key = self.api_key()?;
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url(),
            request.model,
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);

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
            if let Err(e) = parse_gemini_stream(response, tx.clone()).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        let stream = futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        self.list_models().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_messages_maps_tool_call_id_to_function_name() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: String::new(),
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "call_0".to_string(),
                    name: "search_knowledge_base".to_string(),
                    arguments: r#"{"query":"rust"}"#.to_string(),
                }]),
            },
            Message {
                role: Role::Tool,
                content: r#"{"ok":true}"#.to_string(),
                name: Some("call_0".to_string()),
                tool_calls: None,
            },
        ];

        let (_system, contents) = convert_messages(&messages);
        let last = contents.last().expect("expected tool response message");
        assert_eq!(last.role, "user");
        let part = last.parts.last().expect("expected function response part");
        match part {
            GeminiPartV2::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "search_knowledge_base");
            }
            _ => panic!("expected FunctionResponse part"),
        }
    }

    #[test]
    fn test_convert_messages_wraps_non_object_tool_result() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: String::new(),
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "call_0".to_string(),
                    name: "write_note".to_string(),
                    arguments: r#"{"filename":"a.md"}"#.to_string(),
                }]),
            },
            Message {
                role: Role::Tool,
                content: "plain text result".to_string(),
                name: Some("call_0".to_string()),
                tool_calls: None,
            },
        ];

        let (_system, contents) = convert_messages(&messages);
        let last = contents.last().expect("expected tool response message");
        let part = last.parts.last().expect("expected function response part");
        match part {
            GeminiPartV2::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "write_note");
                assert!(function_response.response.is_object());
                assert_eq!(
                    function_response.response["content"],
                    serde_json::Value::String("plain text result".to_string())
                );
            }
            _ => panic!("expected FunctionResponse part"),
        }
    }
}
