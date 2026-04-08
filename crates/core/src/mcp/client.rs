//! MCP client for stdio, legacy SSE, and Streamable HTTP transports.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::{Client as HttpClient, StatusCode, Url};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::CoreError;
use crate::mcp::McpToolInfo;

const JSONRPC_VERSION: &str = "2.0";
const METHOD_NOT_FOUND_CODE: i64 = -32601;
const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_SSE: &str = "text/event-stream";
const HEADER_MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const HEADER_MCP_SESSION_ID: &str = "mcp-session-id";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

struct StdioTransport {
    child: Child,
    stdin: tokio::io::BufWriter<tokio::process::ChildStdin>,
    stdout_rx: mpsc::Receiver<Value>,
    reader_handle: tokio::task::JoinHandle<()>,
    stderr_buf: Arc<Mutex<String>>,
    stderr_handle: tokio::task::JoinHandle<()>,
}

struct LegacySseTransport {
    client: HttpClient,
    message_url: Url,
    custom_headers: HeaderMap,
    events_rx: mpsc::Receiver<Value>,
    diagnostics: Arc<Mutex<String>>,
    stream_handle: tokio::task::JoinHandle<()>,
}

struct StreamableHttpTransport {
    client: HttpClient,
    endpoint_url: Url,
    custom_headers: HeaderMap,
    session_id: Option<String>,
}

enum Transport {
    Stdio(StdioTransport),
    LegacySse(LegacySseTransport),
    StreamableHttp(StreamableHttpTransport),
}

/// MCP client communicating with a server over one of the supported transports.
pub struct McpClient {
    transport: Transport,
    request_id: AtomicI64,
    server_name: String,
    protocol_version: String,
    /// Timeout for individual JSON-RPC requests. Defaults to [`DEFAULT_TIMEOUT`].
    call_timeout: Duration,
}

enum StreamablePostOutcome {
    Accepted,
    Json(Value),
    Sse(reqwest::Response),
}

enum StreamablePostError {
    Core(CoreError),
    SessionExpired,
}

impl From<CoreError> for StreamablePostError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl McpClient {
    /// Connect to an MCP server via stdio transport.
    pub async fn connect_stdio(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
        server_name: &str,
    ) -> Result<Self, CoreError> {
        let transport = Self::build_stdio_transport(command, args, env).await?;
        let mut client = Self {
            transport: Transport::Stdio(transport),
            request_id: AtomicI64::new(1),
            server_name: server_name.to_string(),
            protocol_version: SUPPORTED_PROTOCOL_VERSIONS[0].to_string(),
            call_timeout: DEFAULT_TIMEOUT,
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    /// Connect to an MCP server via legacy SSE transport.
    pub async fn connect_sse(
        url: &str,
        headers: Option<&HashMap<String, String>>,
        server_name: &str,
    ) -> Result<Self, CoreError> {
        let transport = Self::build_legacy_sse_transport(url, headers).await?;
        let mut client = Self {
            transport: Transport::LegacySse(transport),
            request_id: AtomicI64::new(1),
            server_name: server_name.to_string(),
            protocol_version: SUPPORTED_PROTOCOL_VERSIONS[0].to_string(),
            call_timeout: DEFAULT_TIMEOUT,
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    /// Connect to an MCP server via Streamable HTTP transport.
    pub async fn connect_streamable_http(
        url: &str,
        headers: Option<&HashMap<String, String>>,
        server_name: &str,
    ) -> Result<Self, CoreError> {
        let transport = Self::build_streamable_http_transport(url, headers)?;
        let mut client = Self {
            transport: Transport::StreamableHttp(transport),
            request_id: AtomicI64::new(1),
            server_name: server_name.to_string(),
            protocol_version: SUPPORTED_PROTOCOL_VERSIONS[0].to_string(),
            call_timeout: DEFAULT_TIMEOUT,
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    /// Override the call timeout for this client.
    pub fn set_call_timeout(&mut self, timeout: Duration) {
        self.call_timeout = timeout;
    }

    /// List tools available on the connected MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, CoreError> {
        let response = self
            .send_request("tools/list", Some(serde_json::json!({})))
            .await?;

        let tools_val = response
            .get("tools")
            .ok_or_else(|| CoreError::Mcp("tools/list response missing 'tools' field".into()))?;

        serde_json::from_value(tools_val.clone())
            .map_err(|e| CoreError::Mcp(format!("Failed to parse tools list: {e}")))
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String, CoreError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });
        let response = self.send_request("tools/call", Some(params)).await?;

        if let Some(content) = response.get("content").and_then(|value| value.as_array()) {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .collect();
            if !texts.is_empty() {
                return Ok(texts.join("\n"));
            }
        }

        Ok(serde_json::to_string(&response).unwrap_or_default())
    }

    /// Gracefully shut down the MCP server connection.
    pub async fn shutdown(&mut self) -> Result<(), CoreError> {
        if let Transport::Stdio(transport) = &mut self.transport {
            let _ = transport.child.kill().await;
            transport.reader_handle.abort();
            transport.stderr_handle.abort();
            return Ok(());
        }

        if let Transport::LegacySse(transport) = &mut self.transport {
            transport.stream_handle.abort();
            return Ok(());
        }

        let _ = self.reset_streamable_http_session().await;
        Ok(())
    }

    async fn initialize_handshake(&mut self) -> Result<(), CoreError> {
        let mut last_error = None;

        for version in SUPPORTED_PROTOCOL_VERSIONS {
            self.protocol_version = version.to_string();
            let init_params = serde_json::json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {
                    "name": "ask-myself",
                    "version": "0.1.0"
                }
            });

            match self
                .send_request_inner("initialize", Some(init_params), false)
                .await
            {
                Ok(result) => {
                    if let Some(negotiated) = result
                        .get("protocolVersion")
                        .and_then(|value| value.as_str())
                    {
                        self.protocol_version = negotiated.to_string();
                    }
                    self.send_notification("notifications/initialized", None)
                        .await?;
                    return Ok(());
                }
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            CoreError::Mcp("Failed to negotiate an MCP protocol version".into())
        }))
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, CoreError> {
        self.send_request_inner(method, params, true).await
    }

    async fn send_request_inner(
        &mut self,
        method: &str,
        params: Option<Value>,
        allow_reinitialize: bool,
    ) -> Result<Value, CoreError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let mut request = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            request
                .as_object_mut()
                .expect("request object")
                .insert("params".to_string(), p);
        }

        match &self.transport {
            Transport::StreamableHttp(_) => {
                self.send_streamable_http_request(request, id, method, allow_reinitialize)
                    .await
            }
            _ => {
                self.send_transport_message(&request).await?;
                self.wait_for_response(id, method).await
            }
        }
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), CoreError> {
        let mut notification = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
        });
        if let Some(p) = params {
            notification
                .as_object_mut()
                .expect("notification object")
                .insert("params".to_string(), p);
        }
        self.send_transport_message(&notification).await
    }

    async fn send_transport_message(&mut self, payload: &Value) -> Result<(), CoreError> {
        if let Transport::Stdio(transport) = &mut self.transport {
            let mut msg = serde_json::to_string(payload)
                .map_err(|e| CoreError::Mcp(format!("Failed to serialize request: {e}")))?;
            msg.push('\n');
            transport
                .stdin
                .write_all(msg.as_bytes())
                .await
                .map_err(|e| CoreError::Mcp(format!("Failed to write to MCP server stdin: {e}")))?;
            transport
                .stdin
                .flush()
                .await
                .map_err(|e| CoreError::Mcp(format!("Failed to flush MCP server stdin: {e}")))?;
            return Ok(());
        }

        if matches!(&self.transport, Transport::LegacySse(_)) {
            self.post_legacy_sse_message(payload).await?;
            return Ok(());
        }

        match self.post_streamable_http(payload).await.map_err(|err| {
            streamable_post_error_into_core(err, &self.server_name, "notification")
        })? {
            StreamablePostOutcome::Accepted => Ok(()),
            StreamablePostOutcome::Json(value) => {
                self.process_http_json_payload(value, None, "notification")
                    .await?;
                Ok(())
            }
            StreamablePostOutcome::Sse(response) => {
                self.process_streamable_http_sse(response, None, "notification")
                    .await?;
                Ok(())
            }
        }
    }

    async fn wait_for_response(&mut self, id: i64, method: &str) -> Result<Value, CoreError> {
        let result = tokio::time::timeout(self.call_timeout, async {
            loop {
                let next = match &mut self.transport {
                    Transport::Stdio(transport) => transport.stdout_rx.recv().await,
                    Transport::LegacySse(transport) => transport.events_rx.recv().await,
                    Transport::StreamableHttp(_) => {
                        unreachable!("queue wait only used for stdio/SSE")
                    }
                };

                match next {
                    Some(message) => {
                        if let Some(result) = self
                            .process_incoming_message(message, Some(id), method)
                            .await?
                        {
                            return Ok(result);
                        }
                    }
                    None => return Err(self.transport_closed_error().await),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(self.transport_timeout_error(method).await),
        }
    }

    async fn send_streamable_http_request(
        &mut self,
        request: Value,
        request_id: i64,
        method: &str,
        allow_reinitialize: bool,
    ) -> Result<Value, CoreError> {
        let mut can_reinitialize = allow_reinitialize;

        loop {
            match self.post_streamable_http(&request).await {
                Ok(StreamablePostOutcome::Accepted) => {
                    let response = self.open_streamable_http_get().await.map_err(|err| {
                        streamable_post_error_into_core(err, &self.server_name, method)
                    })?;
                    if let Some(result) = self
                        .process_streamable_http_sse(response, Some(request_id), method)
                        .await?
                    {
                        return Ok(result);
                    }
                    return Err(CoreError::Mcp(format!(
                        "MCP server '{}' accepted {method} but no matching response arrived.",
                        self.server_name
                    )));
                }
                Ok(StreamablePostOutcome::Json(value)) => {
                    if let Some(result) = self
                        .process_http_json_payload(value, Some(request_id), method)
                        .await?
                    {
                        return Ok(result);
                    }
                    return Err(CoreError::Mcp(format!(
                        "MCP server '{}' returned JSON for {method} without a matching response id.",
                        self.server_name
                    )));
                }
                Ok(StreamablePostOutcome::Sse(response)) => {
                    if let Some(result) = self
                        .process_streamable_http_sse(response, Some(request_id), method)
                        .await?
                    {
                        return Ok(result);
                    }
                    return Err(CoreError::Mcp(format!(
                        "MCP server '{}' closed the SSE response for {method} before replying.",
                        self.server_name
                    )));
                }
                Err(StreamablePostError::SessionExpired) if can_reinitialize => {
                    let _ = self.reset_streamable_http_session().await;
                    Box::pin(self.initialize_handshake()).await?;
                    can_reinitialize = false;
                }
                Err(err) => {
                    return Err(streamable_post_error_into_core(
                        err,
                        &self.server_name,
                        method,
                    ));
                }
            }
        }
    }

    async fn process_http_json_payload(
        &mut self,
        payload: Value,
        expected_id: Option<i64>,
        context: &str,
    ) -> Result<Option<Value>, CoreError> {
        match payload {
            Value::Array(items) => {
                for item in items {
                    if let Some(result) = self
                        .process_incoming_message(item, expected_id, context)
                        .await?
                    {
                        return Ok(Some(result));
                    }
                }
                Ok(None)
            }
            Value::Object(_) => {
                self.process_incoming_message(payload, expected_id, context)
                    .await
            }
            other => Err(CoreError::Mcp(format!(
                "MCP server '{}' returned an invalid JSON-RPC payload for {context}: {other}",
                self.server_name
            ))),
        }
    }

    async fn process_streamable_http_sse(
        &mut self,
        response: reqwest::Response,
        expected_id: Option<i64>,
        context: &str,
    ) -> Result<Option<Value>, CoreError> {
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let idle_timeout = if expected_id.is_some() {
            self.call_timeout
        } else {
            Duration::from_secs(2)
        };

        loop {
            let next_chunk = tokio::time::timeout(idle_timeout, stream.next()).await;
            let chunk = match next_chunk {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(err))) => {
                    return Err(CoreError::Mcp(format!(
                        "Failed to read SSE response from MCP server '{}': {err}",
                        self.server_name
                    )))
                }
                Ok(None) => {
                    return Ok(None);
                }
                Err(_) if expected_id.is_none() => return Ok(None),
                Err(_) => return Err(self.transport_timeout_error(context).await),
            };

            buffer.extend_from_slice(&chunk);
            while let Some(raw_event) = drain_sse_event(&mut buffer) {
                if let Some(result) = self
                    .handle_sse_event(&raw_event, expected_id, context)
                    .await?
                {
                    return Ok(Some(result));
                }
            }
        }
    }

    async fn handle_sse_event(
        &mut self,
        raw_event: &str,
        expected_id: Option<i64>,
        context: &str,
    ) -> Result<Option<Value>, CoreError> {
        let (event_name, data) = parse_sse_event(raw_event);
        let trimmed = data.trim();
        if trimmed.is_empty() || matches!(event_name.as_deref(), Some("ping")) {
            return Ok(None);
        }

        let payload: Value = serde_json::from_str(trimmed).map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to parse SSE event from MCP server '{}': {e}",
                self.server_name
            ))
        })?;
        self.process_http_json_payload(payload, expected_id, context)
            .await
    }

    async fn process_incoming_message(
        &mut self,
        message: Value,
        expected_id: Option<i64>,
        context: &str,
    ) -> Result<Option<Value>, CoreError> {
        if !message.is_object() {
            return Err(CoreError::Mcp(format!(
                "MCP server '{}' returned a non-object JSON-RPC message for {context}.",
                self.server_name
            )));
        }

        if is_server_request(&message) {
            self.respond_method_not_found(&message).await?;
            return Ok(None);
        }

        if message.get("method").is_some() {
            return Ok(None);
        }

        let Some(id_value) = message.get("id") else {
            return Ok(None);
        };

        if !matches_request_id(id_value, expected_id) {
            return Ok(None);
        }

        if let Some(error) = message.get("error") {
            return Err(CoreError::Mcp(format!(
                "MCP {context} failed on server '{}': {}",
                self.server_name,
                format_json_rpc_error(error)
            )));
        }

        if let Some(result) = message.get("result") {
            return Ok(Some(result.clone()));
        }

        Err(CoreError::Mcp(format!(
            "MCP server '{}' returned a response for {context} without result or error.",
            self.server_name
        )))
    }

    async fn respond_method_not_found(&mut self, message: &Value) -> Result<(), CoreError> {
        let Some(id) = message.get("id") else {
            return Ok(());
        };

        let response = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id.clone(),
            "error": {
                "code": METHOD_NOT_FOUND_CODE,
                "message": "Method not found"
            }
        });
        Box::pin(self.send_transport_message(&response)).await
    }

    async fn transport_closed_error(&self) -> CoreError {
        match &self.transport {
            Transport::Stdio(transport) => {
                let stderr = transport.stderr_buf.lock().await;
                if stderr.trim().is_empty() {
                    CoreError::Mcp(format!(
                        "MCP stdio server '{}' closed unexpectedly.",
                        self.server_name
                    ))
                } else {
                    CoreError::Mcp(format!(
                        "MCP stdio server '{}' closed unexpectedly. stderr:\n{}",
                        self.server_name,
                        stderr.trim()
                    ))
                }
            }
            Transport::LegacySse(transport) => {
                let diagnostics = transport.diagnostics.lock().await;
                if diagnostics.trim().is_empty() {
                    CoreError::Mcp(format!(
                        "MCP SSE server '{}' closed its event stream unexpectedly.",
                        self.server_name
                    ))
                } else {
                    CoreError::Mcp(format!(
                        "MCP SSE server '{}' closed its event stream unexpectedly. Details:\n{}",
                        self.server_name,
                        diagnostics.trim()
                    ))
                }
            }
            Transport::StreamableHttp(_) => CoreError::Mcp(format!(
                "MCP Streamable HTTP server '{}' closed unexpectedly.",
                self.server_name
            )),
        }
    }

    async fn transport_timeout_error(&self, method: &str) -> CoreError {
        CoreError::Mcp(format!(
            "Timed out waiting for MCP server '{}' to finish {method}.",
            self.server_name
        ))
    }

    async fn build_stdio_transport(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
    ) -> Result<StdioTransport, CoreError> {
        let normalized_command = normalize_stdio_command_name(command);
        let resolved_command = resolve_stdio_command_path(&normalized_command)
            .unwrap_or_else(|| PathBuf::from(&normalized_command));
        let mut process = Command::new(&resolved_command);
        process
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut has_port = false;
        if let Some(env_map) = env {
            for (key, value) in env_map {
                if key.eq_ignore_ascii_case("PORT") {
                    has_port = true;
                }
                process.env(key, value);
            }
        }
        if !has_port {
            process.env("PORT", "0");
        }

        let mut child = process.spawn().map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to spawn MCP server command '{command}' (resolved to '{}'): {e}",
                resolved_command.display()
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Mcp("MCP stdio child process has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Mcp("MCP stdio child process has no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::Mcp("MCP stdio child process has no stderr".into()))?;

        let diagnostics = Arc::new(Mutex::new(String::new()));
        let (stdout_tx, stdout_rx) = mpsc::channel(64);
        let stdout_diagnostics = diagnostics.clone();
        let reader_handle = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        match serde_json::from_str::<Value>(trimmed) {
                            Ok(value) => {
                                if stdout_tx.send(value).await.is_err() {
                                    break;
                                }
                            }
                            Err(err) => {
                                append_diagnostics(
                                    &stdout_diagnostics,
                                    &format!("Ignored non-JSON stdout line from MCP server: {line} ({err})"),
                                )
                                .await;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        append_diagnostics(
                            &stdout_diagnostics,
                            &format!("Failed to read MCP stdout: {err}"),
                        )
                        .await;
                        break;
                    }
                }
            }
        });

        let stderr_diagnostics = diagnostics.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => append_diagnostics(&stderr_diagnostics, &line).await,
                    Ok(None) => break,
                    Err(err) => {
                        append_diagnostics(
                            &stderr_diagnostics,
                            &format!("Failed to read MCP stderr: {err}"),
                        )
                        .await;
                        break;
                    }
                }
            }
        });

        Ok(StdioTransport {
            child,
            stdin: tokio::io::BufWriter::new(stdin),
            stdout_rx,
            reader_handle,
            stderr_buf: diagnostics,
            stderr_handle,
        })
    }

    async fn build_legacy_sse_transport(
        url: &str,
        headers: Option<&HashMap<String, String>>,
    ) -> Result<LegacySseTransport, CoreError> {
        let client = build_http_client()?;
        let base_url = parse_url(url, "legacy SSE")?;
        let custom_headers = build_header_map(headers)?;

        let mut request_headers = HeaderMap::new();
        apply_custom_headers(&mut request_headers, &custom_headers);
        request_headers.insert(ACCEPT, HeaderValue::from_static(CONTENT_TYPE_SSE));
        request_headers.insert(
            HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION),
            HeaderValue::from_static(SUPPORTED_PROTOCOL_VERSIONS[0]),
        );

        let response = tokio::time::timeout(
            SSE_CONNECT_TIMEOUT,
            client.get(base_url.clone()).headers(request_headers).send(),
        )
        .await
        .map_err(|_| {
            CoreError::Mcp(format!(
                "Timed out connecting to legacy SSE MCP server at {}",
                base_url
            ))
        })?
        .map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to connect to legacy SSE MCP server at {}: {e}",
                base_url
            ))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::Mcp(format!(
                "Legacy SSE MCP server at {} returned {status}: {}",
                base_url,
                body.trim()
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !content_type.contains(CONTENT_TYPE_SSE) {
            return Err(CoreError::Mcp(format!(
                "Legacy SSE MCP server at {} did not return an SSE stream.",
                base_url
            )));
        }

        let (events_tx, events_rx) = mpsc::channel(64);
        let diagnostics = Arc::new(Mutex::new(String::new()));
        let (endpoint_tx, endpoint_rx) = oneshot::channel::<Result<Url, CoreError>>();
        let stream_diagnostics = diagnostics.clone();
        let stream_handle = tokio::spawn(async move {
            read_legacy_sse_stream(
                response,
                base_url,
                events_tx,
                Some(endpoint_tx),
                stream_diagnostics,
            )
            .await;
        });

        let message_url = match tokio::time::timeout(SSE_CONNECT_TIMEOUT, endpoint_rx).await {
            Ok(Ok(Ok(url))) => url,
            Ok(Ok(Err(err))) => {
                stream_handle.abort();
                return Err(err);
            }
            Ok(Err(_)) => {
                stream_handle.abort();
                return Err(CoreError::Mcp(
                    "Legacy SSE MCP connection closed before publishing a message endpoint.".into(),
                ));
            }
            Err(_) => {
                stream_handle.abort();
                return Err(CoreError::Mcp(
                    "Timed out waiting for a legacy SSE MCP endpoint event.".into(),
                ));
            }
        };

        Ok(LegacySseTransport {
            client,
            message_url,
            custom_headers,
            events_rx,
            diagnostics,
            stream_handle,
        })
    }

    fn build_streamable_http_transport(
        url: &str,
        headers: Option<&HashMap<String, String>>,
    ) -> Result<StreamableHttpTransport, CoreError> {
        Ok(StreamableHttpTransport {
            client: build_http_client()?,
            endpoint_url: parse_url(url, "Streamable HTTP")?,
            custom_headers: build_header_map(headers)?,
            session_id: None,
        })
    }

    async fn open_streamable_http_get(&mut self) -> Result<reqwest::Response, StreamablePostError> {
        let (client, endpoint_url, custom_headers, session_id) = match &self.transport {
            Transport::StreamableHttp(transport) => (
                transport.client.clone(),
                transport.endpoint_url.clone(),
                transport.custom_headers.clone(),
                transport.session_id.clone(),
            ),
            _ => {
                return Err(StreamablePostError::Core(CoreError::Internal(
                    "open_streamable_http_get called for non-HTTP transport".into(),
                )))
            }
        };

        let mut headers = HeaderMap::new();
        apply_custom_headers(&mut headers, &custom_headers);
        headers.insert(ACCEPT, HeaderValue::from_static(CONTENT_TYPE_SSE));
        headers.insert(
            HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION),
            HeaderValue::from_str(&self.protocol_version).map_err(|e| {
                StreamablePostError::Core(CoreError::Mcp(format!(
                    "Invalid MCP protocol version header '{}': {e}",
                    self.protocol_version
                )))
            })?,
        );
        if let Some(session_id) = session_id.as_deref() {
            headers.insert(
                HeaderName::from_static(HEADER_MCP_SESSION_ID),
                HeaderValue::from_str(session_id).map_err(|e| {
                    StreamablePostError::Core(CoreError::Mcp(format!(
                        "Invalid MCP session id '{session_id}': {e}",
                    )))
                })?,
            );
        }

        let response = tokio::time::timeout(
            self.call_timeout,
            client.get(endpoint_url.clone()).headers(headers).send(),
        )
        .await
        .map_err(|_| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Timed out opening the Streamable HTTP event stream at {endpoint_url}"
            )))
        })?
        .map_err(|e| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Failed to open the Streamable HTTP event stream at {endpoint_url}: {e}"
            )))
        })?;

        let status = response.status();
        if status == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamablePostError::SessionExpired);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamablePostError::Core(CoreError::Mcp(format!(
                "Streamable HTTP GET {} returned {status}: {}",
                endpoint_url,
                body.trim()
            ))));
        }

        self.update_session_id(response.headers())?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !content_type.contains(CONTENT_TYPE_SSE) {
            return Err(StreamablePostError::Core(CoreError::Mcp(format!(
                "Streamable HTTP GET {} did not return an SSE stream.",
                endpoint_url
            ))));
        }

        Ok(response)
    }

    async fn post_legacy_sse_message(&mut self, payload: &Value) -> Result<(), CoreError> {
        let (client, message_url, custom_headers) = match &self.transport {
            Transport::LegacySse(transport) => (
                transport.client.clone(),
                transport.message_url.clone(),
                transport.custom_headers.clone(),
            ),
            _ => {
                return Err(CoreError::Internal(
                    "post_legacy_sse_message called for non-SSE transport".into(),
                ))
            }
        };

        let mut headers = HeaderMap::new();
        apply_custom_headers(&mut headers, &custom_headers);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_JSON));
        headers.insert(ACCEPT, HeaderValue::from_static(CONTENT_TYPE_JSON));
        headers.insert(
            HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION),
            HeaderValue::from_str(&self.protocol_version).map_err(|e| {
                CoreError::Mcp(format!(
                    "Invalid MCP protocol version header '{}': {e}",
                    self.protocol_version
                ))
            })?,
        );

        let response = tokio::time::timeout(
            self.call_timeout,
            client
                .post(message_url.clone())
                .headers(headers)
                .json(payload)
                .send(),
        )
        .await
        .map_err(|_| {
            CoreError::Mcp(format!(
                "Timed out sending a request to legacy SSE MCP server at {message_url}"
            ))
        })?
        .map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to send a request to legacy SSE MCP server at {message_url}: {e}"
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::Mcp(format!(
                "Legacy SSE MCP server at {message_url} returned {status}: {}",
                body.trim()
            )));
        }

        Ok(())
    }

    async fn post_streamable_http(
        &mut self,
        payload: &Value,
    ) -> Result<StreamablePostOutcome, StreamablePostError> {
        let (client, endpoint_url, custom_headers, session_id) = match &self.transport {
            Transport::StreamableHttp(transport) => (
                transport.client.clone(),
                transport.endpoint_url.clone(),
                transport.custom_headers.clone(),
                transport.session_id.clone(),
            ),
            _ => {
                return Err(StreamablePostError::Core(CoreError::Internal(
                    "post_streamable_http called for non-HTTP transport".into(),
                )))
            }
        };

        let mut headers = HeaderMap::new();
        apply_custom_headers(&mut headers, &custom_headers);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_JSON));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION),
            HeaderValue::from_str(&self.protocol_version).map_err(|e| {
                StreamablePostError::Core(CoreError::Mcp(format!(
                    "Invalid MCP protocol version header '{}': {e}",
                    self.protocol_version
                )))
            })?,
        );
        if let Some(session_id) = session_id.as_deref() {
            headers.insert(
                HeaderName::from_static(HEADER_MCP_SESSION_ID),
                HeaderValue::from_str(session_id).map_err(|e| {
                    StreamablePostError::Core(CoreError::Mcp(format!(
                        "Invalid MCP session id '{session_id}': {e}",
                    )))
                })?,
            );
        }

        let response = tokio::time::timeout(
            self.call_timeout,
            client
                .post(endpoint_url.clone())
                .headers(headers)
                .json(payload)
                .send(),
        )
        .await
        .map_err(|_| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Timed out sending a Streamable HTTP request to {endpoint_url}"
            )))
        })?
        .map_err(|e| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Failed to send a Streamable HTTP request to {endpoint_url}: {e}"
            )))
        })?;

        let status = response.status();
        if status == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamablePostError::SessionExpired);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamablePostError::Core(CoreError::Mcp(format!(
                "Streamable HTTP endpoint {} returned {status}: {}",
                endpoint_url,
                body.trim()
            ))));
        }

        self.update_session_id(response.headers())?;
        if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
            return Ok(StreamablePostOutcome::Accepted);
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.contains(CONTENT_TYPE_SSE) {
            return Ok(StreamablePostOutcome::Sse(response));
        }

        let body = response.text().await.map_err(|e| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Failed to read Streamable HTTP response from {}: {e}",
                endpoint_url
            )))
        })?;
        if body.trim().is_empty() {
            return Ok(StreamablePostOutcome::Accepted);
        }

        let json = serde_json::from_str(&body).map_err(|e| {
            StreamablePostError::Core(CoreError::Mcp(format!(
                "Failed to parse Streamable HTTP response from {} as JSON: {e}",
                endpoint_url
            )))
        })?;
        Ok(StreamablePostOutcome::Json(json))
    }

    async fn reset_streamable_http_session(&mut self) -> Result<(), CoreError> {
        let (client, endpoint_url, custom_headers, session_id) = match &self.transport {
            Transport::StreamableHttp(transport) => (
                transport.client.clone(),
                transport.endpoint_url.clone(),
                transport.custom_headers.clone(),
                transport.session_id.clone(),
            ),
            _ => return Ok(()),
        };

        if let Some(session_id) = session_id.as_deref() {
            let mut headers = HeaderMap::new();
            apply_custom_headers(&mut headers, &custom_headers);
            headers.insert(
                HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION),
                HeaderValue::from_str(&self.protocol_version).map_err(|e| {
                    CoreError::Mcp(format!(
                        "Invalid MCP protocol version header '{}': {e}",
                        self.protocol_version
                    ))
                })?,
            );
            headers.insert(
                HeaderName::from_static(HEADER_MCP_SESSION_ID),
                HeaderValue::from_str(session_id).map_err(|e| {
                    CoreError::Mcp(format!("Invalid MCP session id '{session_id}': {e}"))
                })?,
            );

            if let Ok(Ok(response)) = tokio::time::timeout(
                self.call_timeout,
                client.delete(endpoint_url).headers(headers).send(),
            )
            .await
            {
                let _ = response.bytes().await;
            }
        }

        if let Transport::StreamableHttp(transport) = &mut self.transport {
            transport.session_id = None;
        }
        Ok(())
    }

    fn update_session_id(&mut self, headers: &HeaderMap) -> Result<(), CoreError> {
        let Some(session_id) = headers.get(HEADER_MCP_SESSION_ID) else {
            return Ok(());
        };
        let session_id = session_id.to_str().map_err(|e| {
            CoreError::Mcp(format!(
                "MCP server returned an invalid session id header: {e}"
            ))
        })?;
        if session_id.trim().is_empty() {
            return Ok(());
        }

        if let Transport::StreamableHttp(transport) = &mut self.transport {
            transport.session_id = Some(session_id.to_string());
        }
        Ok(())
    }
}

fn build_http_client() -> Result<HttpClient, CoreError> {
    HttpClient::builder()
        .connect_timeout(SSE_CONNECT_TIMEOUT)
        .build()
        .map_err(|e| CoreError::Mcp(format!("Failed to build HTTP client for MCP: {e}")))
}

fn build_header_map(headers: Option<&HashMap<String, String>>) -> Result<HeaderMap, CoreError> {
    let mut map = HeaderMap::new();
    if let Some(headers) = headers {
        for (name, value) in headers {
            let header_name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|e| {
                CoreError::InvalidInput(format!("Invalid HTTP header name '{name}': {e}"))
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                CoreError::InvalidInput(format!("Invalid HTTP header value for '{name}': {e}"))
            })?;
            map.insert(header_name, header_value);
        }
    }
    Ok(map)
}

fn apply_custom_headers(target: &mut HeaderMap, custom_headers: &HeaderMap) {
    for (name, value) in custom_headers {
        target.insert(name.clone(), value.clone());
    }
}

fn parse_url(url: &str, transport_name: &str) -> Result<Url, CoreError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "{transport_name} MCP transport requires a URL"
        )));
    }

    let parsed = Url::parse(trimmed).map_err(|e| {
        CoreError::InvalidInput(format!(
            "Invalid URL for {transport_name} MCP transport: {e}"
        ))
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(CoreError::InvalidInput(format!(
            "{transport_name} MCP transport only supports http/https URLs, got '{other}'"
        ))),
    }
}

fn resolve_sse_endpoint(base_url: &Url, endpoint: &str) -> Result<Url, CoreError> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Mcp(
            "Legacy SSE MCP endpoint event was empty".into(),
        ));
    }

    base_url.join(trimmed).or_else(|_| {
        Url::parse(trimmed).map_err(|e| {
            CoreError::Mcp(format!(
                "Legacy SSE MCP endpoint '{trimmed}' is invalid: {e}"
            ))
        })
    })
}

async fn read_legacy_sse_stream(
    response: reqwest::Response,
    base_url: Url,
    sender: mpsc::Sender<Value>,
    mut endpoint_tx: Option<oneshot::Sender<Result<Url, CoreError>>>,
    diagnostics: Arc<Mutex<String>>,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(err) => {
                let error = CoreError::Mcp(format!(
                    "Failed to read legacy SSE stream from {base_url}: {err}"
                ));
                append_diagnostics(&diagnostics, &error.to_string()).await;
                if let Some(tx) = endpoint_tx.take() {
                    let _ = tx.send(Err(error));
                }
                return;
            }
        };

        buffer.extend_from_slice(&chunk);
        while let Some(raw_event) = drain_sse_event(&mut buffer) {
            if let Err(err) =
                process_legacy_sse_event(&raw_event, &base_url, &sender, &mut endpoint_tx).await
            {
                append_diagnostics(&diagnostics, &err.to_string()).await;
                if let Some(tx) = endpoint_tx.take() {
                    let _ = tx.send(Err(err));
                }
                return;
            }
        }
    }

    if let Some(tx) = endpoint_tx.take() {
        let _ = tx.send(Err(CoreError::Mcp(
            "Legacy SSE MCP stream closed before publishing a message endpoint.".into(),
        )));
    }
}

async fn process_legacy_sse_event(
    raw_event: &str,
    base_url: &Url,
    sender: &mpsc::Sender<Value>,
    endpoint_tx: &mut Option<oneshot::Sender<Result<Url, CoreError>>>,
) -> Result<(), CoreError> {
    let (event_name, data) = parse_sse_event(raw_event);
    let trimmed = data.trim();
    if trimmed.is_empty() || matches!(event_name.as_deref(), Some("ping")) {
        return Ok(());
    }

    if matches!(event_name.as_deref(), Some("endpoint")) {
        let endpoint = resolve_sse_endpoint(base_url, trimmed)?;
        if let Some(tx) = endpoint_tx.take() {
            let _ = tx.send(Ok(endpoint));
        }
        return Ok(());
    }

    let message = serde_json::from_str::<Value>(trimmed).map_err(|e| {
        CoreError::Mcp(format!(
            "Failed to parse legacy SSE message '{trimmed}' as JSON: {e}"
        ))
    })?;
    sender
        .send(message)
        .await
        .map_err(|_| CoreError::Mcp("Legacy SSE MCP receiver was dropped".into()))
}

async fn append_diagnostics(buffer: &Arc<Mutex<String>>, line: &str) {
    let mut guard = buffer.lock().await;
    if !guard.is_empty() {
        guard.push('\n');
    }
    guard.push_str(line);
}

fn drain_sse_event(buffer: &mut Vec<u8>) -> Option<String> {
    let (index, delimiter_len) = find_sse_delimiter(buffer)?;
    let drained = buffer.drain(..index + delimiter_len).collect::<Vec<_>>();
    let payload = &drained[..index];
    Some(String::from_utf8_lossy(payload).into_owned())
}

fn find_sse_delimiter(buffer: &[u8]) -> Option<(usize, usize)> {
    if buffer.len() < 2 {
        return None;
    }

    for index in 0..buffer.len() - 1 {
        if buffer[index] == b'\n' && buffer[index + 1] == b'\n' {
            return Some((index, 2));
        }
        if index + 3 < buffer.len()
            && buffer[index] == b'\r'
            && buffer[index + 1] == b'\n'
            && buffer[index + 2] == b'\r'
            && buffer[index + 3] == b'\n'
        {
            return Some((index, 4));
        }
    }

    None
}

fn parse_sse_event(raw_event: &str) -> (Option<String>, String) {
    let mut event_name = None;
    let mut data_lines = Vec::new();

    for raw_line in raw_event.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }

    (event_name, data_lines.join("\n"))
}

fn is_server_request(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}

fn matches_request_id(id_value: &Value, expected_id: Option<i64>) -> bool {
    let Some(expected_id) = expected_id else {
        return false;
    };

    id_value
        .as_i64()
        .map(|value| value == expected_id)
        .or_else(|| {
            id_value
                .as_str()
                .and_then(|value| value.parse::<i64>().ok())
                .map(|value| value == expected_id)
        })
        .unwrap_or(false)
}

fn format_json_rpc_error(error: &Value) -> String {
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown MCP error");
    let data = error.get("data").and_then(|value| {
        if value.is_null() {
            None
        } else if let Some(text) = value.as_str() {
            Some(text.to_string())
        } else {
            Some(value.to_string())
        }
    });

    match data {
        Some(data) if !data.is_empty() => format!("code {code}: {message} ({data})"),
        _ => format!("code {code}: {message}"),
    }
}

fn streamable_post_error_into_core(
    error: StreamablePostError,
    server_name: &str,
    method: &str,
) -> CoreError {
    match error {
        StreamablePostError::Core(error) => error,
        StreamablePostError::SessionExpired => CoreError::Mcp(format!(
            "Streamable HTTP session for MCP server '{server_name}' expired while processing {method}.",
        )),
    }
}

fn normalize_stdio_command_name(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn resolve_stdio_command_path(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = Path::new(trimmed);
    if path.is_absolute() || has_directory_component(trimmed) {
        return resolve_stdio_candidate(path);
    }

    let path_dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    resolve_stdio_in_dirs(trimmed, &path_dirs, &windows_pathexts())
}

fn resolve_stdio_in_dirs(
    command: &str,
    path_dirs: &[PathBuf],
    pathexts: &[String],
) -> Option<PathBuf> {
    for dir in path_dirs {
        if let Some(candidate) = resolve_stdio_candidate_with_pathext(&dir.join(command), pathexts)
        {
            return Some(candidate);
        }
    }
    None
}

fn resolve_stdio_candidate(path: &Path) -> Option<PathBuf> {
    resolve_stdio_candidate_with_pathext(path, &windows_pathexts())
}

fn resolve_stdio_candidate_with_pathext(path: &Path, pathexts: &[String]) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if path.extension().is_none() {
            for extension in pathexts {
                let mut candidate = path.as_os_str().to_os_string();
                candidate.push(extension);
                let candidate = PathBuf::from(candidate);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    if path.is_file() {
        return Some(path.to_path_buf());
    }

    None
}

fn has_directory_component(command: &str) -> bool {
    command.contains(std::path::MAIN_SEPARATOR)
        || (cfg!(windows) && command.contains('/'))
        || (cfg!(windows) && command.contains('\\'))
}

fn windows_pathexts() -> Vec<String> {
    #[cfg(windows)]
    {
        let raw = env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
        raw.to_string_lossy()
            .split(';')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value.starts_with('.') {
                    value.to_string()
                } else {
                    format!(".{value}")
                }
            })
            .collect()
    }

    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use serde_json::json;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{mpsc, Mutex};

    #[derive(Debug)]
    struct TestHttpRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    #[tokio::test]
    async fn streamable_http_reinitializes_after_session_expiry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let initialize_calls = Arc::new(Mutex::new(0usize));
        let tool_sessions = Arc::new(Mutex::new(Vec::<String>::new()));

        let initialize_calls_server = initialize_calls.clone();
        let tool_sessions_server = tool_sessions.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let initialize_calls = initialize_calls_server.clone();
                let tool_sessions = tool_sessions_server.clone();

                tokio::spawn(async move {
                    let request = read_http_request(&mut stream).await.unwrap();
                    let payload: Value = if request.body.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&request.body).unwrap()
                    };

                    match (request.method.as_str(), request.path.as_str()) {
                        ("POST", "/mcp") => {
                            let method = payload
                                .get("method")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            match method {
                                "initialize" => {
                                    let mut count = initialize_calls.lock().await;
                                    *count += 1;
                                    let session_id = if *count == 1 {
                                        "session-old"
                                    } else {
                                        "session-new"
                                    };
                                    let response = json!({
                                        "jsonrpc": "2.0",
                                        "id": payload.get("id").cloned().unwrap(),
                                        "result": {
                                            "protocolVersion": "2025-11-25",
                                            "capabilities": {},
                                            "serverInfo": { "name": "remote", "version": "1.0.0" }
                                        }
                                    });
                                    write_json_response(
                                        &mut stream,
                                        "200 OK",
                                        Some(session_id),
                                        &response,
                                    )
                                    .await
                                    .unwrap();
                                }
                                "notifications/initialized" => {
                                    let session_id =
                                        request.headers.get(HEADER_MCP_SESSION_ID).cloned();
                                    write_empty_response(
                                        &mut stream,
                                        "202 Accepted",
                                        session_id.as_deref(),
                                    )
                                    .await
                                    .unwrap();
                                }
                                "tools/list" => {
                                    let session_id = request
                                        .headers
                                        .get(HEADER_MCP_SESSION_ID)
                                        .cloned()
                                        .unwrap_or_default();
                                    tool_sessions.lock().await.push(session_id.clone());

                                    if session_id == "session-old" {
                                        write_text_response(
                                            &mut stream,
                                            "404 Not Found",
                                            None,
                                            "expired",
                                        )
                                        .await
                                        .unwrap();
                                    } else {
                                        let response = json!({
                                            "jsonrpc": "2.0",
                                            "id": payload.get("id").cloned().unwrap(),
                                            "result": {
                                                "tools": [{
                                                    "name": "demo",
                                                    "description": "Demo tool",
                                                    "inputSchema": {
                                                        "type": "object",
                                                        "properties": {}
                                                    }
                                                }]
                                            }
                                        });
                                        write_json_response(
                                            &mut stream,
                                            "200 OK",
                                            Some("session-new"),
                                            &response,
                                        )
                                        .await
                                        .unwrap();
                                    }
                                }
                                _ => {
                                    write_text_response(
                                        &mut stream,
                                        "400 Bad Request",
                                        None,
                                        "unexpected method",
                                    )
                                    .await
                                    .unwrap();
                                }
                            }
                        }
                        ("DELETE", "/mcp") => {
                            write_empty_response(&mut stream, "204 No Content", None)
                                .await
                                .unwrap();
                        }
                        _ => {
                            write_text_response(&mut stream, "404 Not Found", None, "missing")
                                .await
                                .unwrap();
                        }
                    }
                });
            }
        });

        let url = format!("http://{addr}/mcp");
        let mut client = McpClient::connect_streamable_http(&url, None, "remote")
            .await
            .unwrap();
        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "demo");
        assert_eq!(*initialize_calls.lock().await, 2);
        assert_eq!(
            tool_sessions.lock().await.clone(),
            vec!["session-old".to_string(), "session-new".to_string()]
        );

        server_task.abort();
    }

    #[tokio::test]
    async fn legacy_sse_connects_and_lists_tools() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sse_sender = Arc::new(Mutex::new(None::<mpsc::UnboundedSender<String>>));

        let sse_sender_server = sse_sender.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let sse_sender = sse_sender_server.clone();

                tokio::spawn(async move {
                    let request = read_http_request(&mut stream).await.unwrap();
                    match (request.method.as_str(), request.path.as_str()) {
                        ("GET", "/sse") => {
                            let headers = concat!(
                                "HTTP/1.1 200 OK\r\n",
                                "Content-Type: text/event-stream\r\n",
                                "Cache-Control: no-cache\r\n",
                                "Connection: keep-alive\r\n",
                                "\r\n"
                            );
                            stream.write_all(headers.as_bytes()).await.unwrap();
                            stream
                                .write_all(b"event: endpoint\r\ndata: /messages\r\n\r\n")
                                .await
                                .unwrap();
                            stream.flush().await.unwrap();

                            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                            *sse_sender.lock().await = Some(tx);

                            while let Some(event) = rx.recv().await {
                                if stream.write_all(event.as_bytes()).await.is_err() {
                                    break;
                                }
                                if stream.flush().await.is_err() {
                                    break;
                                }
                            }
                        }
                        ("POST", "/messages") => {
                            let payload: Value = serde_json::from_slice(&request.body).unwrap();
                            let method = payload
                                .get("method")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let sender = sse_sender.lock().await.clone().unwrap();

                            match method {
                                "initialize" => {
                                    let response = json!({
                                        "jsonrpc": "2.0",
                                        "id": payload.get("id").cloned().unwrap(),
                                        "result": {
                                            "protocolVersion": "2025-11-25",
                                            "capabilities": {},
                                            "serverInfo": { "name": "legacy", "version": "1.0.0" }
                                        }
                                    });
                                    sender
                                        .send(format!(
                                            "data: {}\r\n\r\n",
                                            serde_json::to_string(&response).unwrap()
                                        ))
                                        .unwrap();
                                }
                                "tools/list" => {
                                    let response = json!({
                                        "jsonrpc": "2.0",
                                        "id": payload.get("id").cloned().unwrap(),
                                        "result": {
                                            "tools": [{
                                                "name": "legacy_tool",
                                                "description": "Legacy tool",
                                                "inputSchema": {
                                                    "type": "object",
                                                    "properties": {}
                                                }
                                            }]
                                        }
                                    });
                                    sender
                                        .send(format!(
                                            "data: {}\r\n\r\n",
                                            serde_json::to_string(&response).unwrap()
                                        ))
                                        .unwrap();
                                }
                                "notifications/initialized" => {}
                                _ => panic!("unexpected legacy SSE method: {method}"),
                            }

                            write_empty_response(&mut stream, "202 Accepted", None)
                                .await
                                .unwrap();
                        }
                        _ => {
                            write_text_response(&mut stream, "404 Not Found", None, "missing")
                                .await
                                .unwrap();
                        }
                    }
                });
            }
        });

        let url = format!("http://{addr}/sse");
        let mut client = McpClient::connect_sse(&url, None, "legacy").await.unwrap();
        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "legacy_tool");

        server_task.abort();
    }

    #[test]
    fn normalize_stdio_command_name_trims_wrapping_quotes() {
        assert_eq!(normalize_stdio_command_name("  \"npx\"  "), "npx");
        assert_eq!(
            normalize_stdio_command_name(" 'C:\\Program Files\\nodejs\\npx.cmd' "),
            "C:\\Program Files\\nodejs\\npx.cmd"
        );
        assert_eq!(normalize_stdio_command_name("npx"), "npx");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_stdio_in_dirs_prefers_windows_wrappers() {
        let temp = tempdir().unwrap();
        let cmd_path = temp.path().join("npx.cmd");
        fs::write(&cmd_path, "@echo off\r\n").unwrap();

        let resolved = resolve_stdio_in_dirs(
            "npx",
            &[temp.path().to_path_buf()],
            &[
                ".COM".to_string(),
                ".EXE".to_string(),
                ".BAT".to_string(),
                ".CMD".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(
            resolved.to_string_lossy().to_ascii_lowercase(),
            cmd_path.to_string_lossy().to_ascii_lowercase()
        );
    }

    async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<TestHttpRequest> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];

        loop {
            let bytes_read = stream.read(&mut chunk).await?;
            if bytes_read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before request completed",
                ));
            }

            buffer.extend_from_slice(&chunk[..bytes_read]);
            if let Some(header_end) = find_http_header_end(&buffer) {
                let header_text = String::from_utf8_lossy(&buffer[..header_end]);
                let mut lines = header_text.split("\r\n");
                let request_line = lines.next().unwrap_or_default();
                let mut request_parts = request_line.split_whitespace();
                let method = request_parts.next().unwrap_or_default().to_string();
                let path = request_parts.next().unwrap_or_default().to_string();

                let mut headers = HashMap::new();
                for line in lines {
                    if line.is_empty() {
                        continue;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }

                let body_start = header_end + 4;
                let content_length = headers
                    .get("content-length")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);

                while buffer.len() < body_start + content_length {
                    let bytes_read = stream.read(&mut chunk).await?;
                    if bytes_read == 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "connection closed before body completed",
                        ));
                    }
                    buffer.extend_from_slice(&chunk[..bytes_read]);
                }

                return Ok(TestHttpRequest {
                    method,
                    path,
                    headers,
                    body: buffer[body_start..body_start + content_length].to_vec(),
                });
            }
        }
    }

    fn find_http_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }

    async fn write_json_response(
        stream: &mut TcpStream,
        status: &str,
        session_id: Option<&str>,
        body: &Value,
    ) -> std::io::Result<()> {
        write_response(
            stream,
            status,
            session_id,
            CONTENT_TYPE_JSON,
            &serde_json::to_vec(body).unwrap(),
        )
        .await
    }

    async fn write_text_response(
        stream: &mut TcpStream,
        status: &str,
        session_id: Option<&str>,
        body: &str,
    ) -> std::io::Result<()> {
        write_response(stream, status, session_id, "text/plain", body.as_bytes()).await
    }

    async fn write_empty_response(
        stream: &mut TcpStream,
        status: &str,
        session_id: Option<&str>,
    ) -> std::io::Result<()> {
        write_response(stream, status, session_id, CONTENT_TYPE_JSON, &[]).await
    }

    async fn write_response(
        stream: &mut TcpStream,
        status: &str,
        session_id: Option<&str>,
        content_type: &str,
        body: &[u8],
    ) -> std::io::Result<()> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(session_id) = session_id {
            response.push_str(&format!("MCP-Session-Id: {session_id}\r\n"));
        }
        response.push_str("\r\n");

        stream.write_all(response.as_bytes()).await?;
        if !body.is_empty() {
            stream.write_all(body).await?;
        }
        stream.flush().await
    }
}
