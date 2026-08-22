//! MCP (Model Context Protocol) module — client, manager, and data models.

pub mod client;
pub mod config_file;

use crate::db::Database;
use crate::error::CoreError;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::TcpListener;
use std::process::{Child, Command as StdCommand};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use uuid::Uuid;

use self::client::McpClient;
use crate::tools::mcp_tool::{McpClientSlot, McpTool};
use crate::tools::ToolRegistry;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// Persisted MCP connector configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    pub name: String,
    /// Transport type: `"stdio"`, `"sse"`, or `"streamable_http"`.
    pub transport: String,
    pub command: Option<String>,
    /// JSON array string, e.g. `["--port", "8080"]`.
    pub args: Option<String>,
    pub url: Option<String>,
    /// JSON object string for environment variables.
    pub env_json: Option<String>,
    /// JSON object string for HTTP headers.
    pub headers_json: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Non-`None` for built-in servers managed by the app.
    /// Built-in connectors cannot be deleted and have their process lifecycle managed.
    pub builtin_id: Option<String>,
}

/// Input for creating or updating an MCP connector configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveMcpServerInput {
    /// `None` = create new, `Some` = update existing.
    pub id: Option<String>,
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Option<String>,
    pub url: Option<String>,
    pub env_json: Option<String>,
    pub headers_json: Option<String>,
    pub enabled: bool,
}

/// Tool information returned by an MCP connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

fn normalize_required_text(field: &str, value: &str) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_text(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn parse_mcp_args(args: &str) -> Result<Vec<String>, CoreError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|e| {
            CoreError::InvalidInput(format!(
                "Invalid args: expected a JSON array of strings, one arg per line, or comma-separated values ({e})"
            ))
        });
    }

    let values = if trimmed.contains('\n') {
        trimmed.lines().map(str::trim).collect::<Vec<_>>()
    } else if trimmed.contains(',') {
        trimmed.split(',').map(str::trim).collect::<Vec<_>>()
    } else {
        trimmed.split_whitespace().collect::<Vec<_>>()
    };

    Ok(values
        .into_iter()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn normalize_args_json(args: &Option<String>) -> Result<Option<String>, CoreError> {
    let Some(raw_args) = normalize_optional_text(args) else {
        return Ok(None);
    };
    let parsed = parse_mcp_args(&raw_args)?;
    if parsed.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&parsed)
        .map(Some)
        .map_err(CoreError::from)
}

fn normalize_json_string_map(
    field: &str,
    value: &Option<String>,
) -> Result<Option<String>, CoreError> {
    let Some(raw) = normalize_optional_text(value) else {
        return Ok(None);
    };

    // Keep persisted connector maps canonical. Besides making diffs readable,
    // this avoids treating a key-order-only rewrite as a runtime change.
    let parsed: BTreeMap<String, String> = serde_json::from_str(&raw).map_err(|e| {
        CoreError::InvalidInput(format!(
            "Invalid {field}: expected a JSON object of string values ({e})"
        ))
    })?;

    if parsed.is_empty() {
        return Ok(None);
    }

    if let Some(empty_key) = parsed.keys().find(|key| key.trim().is_empty()) {
        return Err(CoreError::InvalidInput(format!(
            "Invalid {field}: key '{empty_key}' cannot be empty"
        )));
    }

    serde_json::to_string(&parsed)
        .map(Some)
        .map_err(CoreError::from)
}

fn resolve_env_placeholders(value: &str) -> Result<String, CoreError> {
    let mut resolved = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("${env:") {
        resolved.push_str(&remaining[..start]);
        let placeholder = &remaining[start + 6..];
        let Some(end) = placeholder.find('}') else {
            return Err(CoreError::InvalidInput(
                "Invalid MCP environment reference: missing closing '}'".into(),
            ));
        };
        let variable = &placeholder[..end];
        if variable.is_empty()
            || !variable
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(CoreError::InvalidInput(format!(
                "Invalid MCP environment reference '${{env:{variable}}}'"
            )));
        }
        let secret = std::env::var(variable).map_err(|_| {
            CoreError::InvalidInput(format!(
                "MCP connector requires environment variable '{variable}'"
            ))
        })?;
        resolved.push_str(&secret);
        remaining = &placeholder[end + 1..];
    }
    resolved.push_str(remaining);
    Ok(resolved)
}

fn resolve_mcp_config_map(field: &str, raw: &str) -> Result<HashMap<String, String>, CoreError> {
    let values: HashMap<String, String> = serde_json::from_str(raw)
        .map_err(|error| CoreError::InvalidInput(format!("Invalid {field}: {error}")))?;
    values
        .into_iter()
        .map(|(key, value)| resolve_env_placeholders(&value).map(|value| (key, value)))
        .collect()
}

fn normalize_http_url(field: &str, value: &Option<String>) -> Result<Option<String>, CoreError> {
    let Some(raw) = normalize_optional_text(value) else {
        return Ok(None);
    };

    let parsed = Url::parse(&raw).map_err(|e| {
        CoreError::InvalidInput(format!("Invalid {field}: expected an http/https URL ({e})"))
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(Some(parsed.to_string())),
        other => Err(CoreError::InvalidInput(format!(
            "Invalid {field}: expected an http/https URL, got '{other}'"
        ))),
    }
}

fn normalize_save_input(input: &SaveMcpServerInput) -> Result<SaveMcpServerInput, CoreError> {
    let name = normalize_required_text("MCP connector name", &input.name)?;
    let transport = match input.transport.trim() {
        "" => {
            return Err(CoreError::InvalidInput(
                "MCP transport cannot be empty".into(),
            ))
        }
        "stdio" => "stdio".to_string(),
        "sse" => "sse".to_string(),
        "streamable_http" => "streamable_http".to_string(),
        other => {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported MCP transport: {other}. Expected 'stdio', 'sse', or 'streamable_http'."
            )))
        }
    };

    match transport.as_str() {
        "stdio" => {
            let command = normalize_optional_text(&input.command);
            if command.is_none() {
                return Err(CoreError::InvalidInput(
                    "stdio transport requires a command".into(),
                ));
            }
            if normalize_optional_text(&input.url).is_some() {
                return Err(CoreError::InvalidInput(
                    "stdio transport does not use a URL".into(),
                ));
            }
            if normalize_optional_text(&input.headers_json).is_some() {
                return Err(CoreError::InvalidInput(
                    "stdio transport does not use headersJson".into(),
                ));
            }

            Ok(SaveMcpServerInput {
                id: input.id.clone(),
                name,
                transport,
                command,
                args: normalize_args_json(&input.args)?,
                url: None,
                env_json: normalize_json_string_map("envJson", &input.env_json)?,
                headers_json: None,
                enabled: input.enabled,
            })
        }
        "sse" | "streamable_http" => {
            if normalize_optional_text(&input.command).is_some() {
                return Err(CoreError::InvalidInput(format!(
                    "{transport} transport does not use a command"
                )));
            }
            if normalize_optional_text(&input.args).is_some() {
                return Err(CoreError::InvalidInput(format!(
                    "{transport} transport does not use args"
                )));
            }
            if normalize_optional_text(&input.env_json).is_some() {
                return Err(CoreError::InvalidInput(format!(
                    "{transport} transport does not use envJson"
                )));
            }

            let url = normalize_http_url("url", &input.url)?;
            if url.is_none() {
                return Err(CoreError::InvalidInput(format!(
                    "{transport} transport requires a URL"
                )));
            }

            Ok(SaveMcpServerInput {
                id: input.id.clone(),
                name,
                transport,
                command: None,
                args: None,
                url,
                env_json: None,
                headers_json: normalize_json_string_map("headersJson", &input.headers_json)?,
                enabled: input.enabled,
            })
        }
        _ => unreachable!("transport already normalized"),
    }
}

/// Find an available TCP port by binding to port 0.
fn find_free_port() -> Result<u16, CoreError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| CoreError::Mcp(format!("Failed to find free port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| CoreError::Mcp(format!("Failed to get port: {e}")))?
        .port();
    drop(listener);
    Ok(port)
}

fn runtime_config_changed(current: &McpServer, desired: &McpServer) -> bool {
    current.name != desired.name
        || current.transport != desired.transport
        || current.command != desired.command
        || current.args != desired.args
        || current.url != desired.url
        || current.env_json != desired.env_json
        || current.headers_json != desired.headers_json
}

fn expand_managed_arg(arg: &str, port: u16) -> String {
    arg.replace("${PORT}", &port.to_string())
}

// ---------------------------------------------------------------------------
// Database CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// List all MCP connectors, newest first.
    pub fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, transport, command, args, url, env_json, headers_json,
                    enabled, created_at, updated_at, builtin_id
             FROM mcp_servers
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(McpServer {
                id: row.get(0)?,
                name: row.get(1)?,
                transport: row.get(2)?,
                command: row.get(3)?,
                args: row.get(4)?,
                url: row.get(5)?,
                env_json: row.get(6)?,
                headers_json: row.get(7)?,
                enabled: row.get::<_, i32>(8)? != 0,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                builtin_id: row.get(11)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Create or update an MCP connector configuration.
    pub fn save_mcp_server(&self, input: &SaveMcpServerInput) -> Result<McpServer, CoreError> {
        let input = normalize_save_input(input)?;
        let conn = self.conn();
        let id = match &input.id {
            Some(existing_id) => {
                // Check if the existing server is built-in; if so, block transport/command/args changes.
                let existing_builtin_id: Option<String> = conn
                    .query_row(
                        "SELECT builtin_id FROM mcp_servers WHERE id = ?1",
                        rusqlite::params![existing_id],
                        |row| row.get(0),
                    )
                    .ok();

                if existing_builtin_id.is_some() {
                    // For built-in servers, only allow toggling name/enabled/url/headers — not transport/command/args.
                    conn.execute(
                        "UPDATE mcp_servers
                         SET name = ?2, url = ?3, headers_json = ?4,
                             enabled = ?5, updated_at = datetime('now')
                         WHERE id = ?1",
                        rusqlite::params![
                            existing_id,
                            &input.name,
                            &input.url,
                            &input.headers_json,
                            input.enabled as i32,
                        ],
                    )?;
                } else {
                    conn.execute(
                        "UPDATE mcp_servers
                         SET name = ?2, transport = ?3, command = ?4, args = ?5,
                             url = ?6, env_json = ?7, headers_json = ?8,
                             enabled = ?9, updated_at = datetime('now')
                         WHERE id = ?1",
                        rusqlite::params![
                            existing_id,
                            &input.name,
                            &input.transport,
                            &input.command,
                            &input.args,
                            &input.url,
                            &input.env_json,
                            &input.headers_json,
                            input.enabled as i32,
                        ],
                    )?;
                }
                existing_id.clone()
            }
            None => {
                let new_id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO mcp_servers (id, name, transport, command, args, url, env_json, headers_json, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        &new_id,
                        &input.name,
                        &input.transport,
                        &input.command,
                        &input.args,
                        &input.url,
                        &input.env_json,
                        &input.headers_json,
                        input.enabled as i32,
                    ],
                )?;
                new_id
            }
        };
        drop(conn);
        self.get_mcp_server(&id)
    }

    /// Delete an MCP connector by ID.
    pub fn delete_mcp_server(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        // Prevent deletion of built-in connectors.
        let builtin: Option<String> = conn
            .query_row(
                "SELECT builtin_id FROM mcp_servers WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .map_err(|_| CoreError::NotFound(format!("MCP connector {id}")))?;
        if builtin.is_some() {
            return Err(CoreError::InvalidInput(
                "Cannot delete built-in MCP connector".into(),
            ));
        }
        let affected = conn.execute(
            "DELETE FROM mcp_servers WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("MCP connector {id}")));
        }
        Ok(())
    }

    /// Toggle an MCP connector's enabled state.
    pub fn toggle_mcp_server(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE mcp_servers SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("MCP connector {id}")));
        }
        Ok(())
    }

    /// Get only enabled MCP connectors.
    pub fn get_enabled_mcp_servers(&self) -> Result<Vec<McpServer>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, transport, command, args, url, env_json, headers_json,
                    enabled, created_at, updated_at, builtin_id
             FROM mcp_servers
             WHERE enabled = 1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(McpServer {
                id: row.get(0)?,
                name: row.get(1)?,
                transport: row.get(2)?,
                command: row.get(3)?,
                args: row.get(4)?,
                url: row.get(5)?,
                env_json: row.get(6)?,
                headers_json: row.get(7)?,
                enabled: true,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                builtin_id: row.get(11)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn get_mcp_server(&self, id: &str) -> Result<McpServer, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, transport, command, args, url, env_json, headers_json,
                    enabled, created_at, updated_at, builtin_id
             FROM mcp_servers
             WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(McpServer {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    transport: row.get(2)?,
                    command: row.get(3)?,
                    args: row.get(4)?,
                    url: row.get(5)?,
                    env_json: row.get(6)?,
                    headers_json: row.get(7)?,
                    enabled: row.get::<_, i32>(8)? != 0,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                    builtin_id: row.get(11)?,
                })
            },
        )
        .map_err(|_| CoreError::NotFound(format!("MCP connector {id}")))
    }
}

// ---------------------------------------------------------------------------
// MCP Manager
// ---------------------------------------------------------------------------

/// Manages MCP connector connections and their lifecycle.
pub struct McpManager {
    clients: HashMap<String, Arc<Mutex<McpClient>>>,
    connection_health: HashMap<String, Arc<McpConnectionHealth>>,
    connection_call_timeout_secs: HashMap<String, Option<u64>>,
    connected_servers: HashMap<String, McpServer>,
    managed_processes: HashMap<String, Child>,
    connection_generation: Arc<AtomicU64>,
}

/// Shared liveness state held by both the manager and registered MCP tools.
/// A failed tool call invalidates the registry generation without requiring
/// the tool to own or lock the manager.
pub(crate) struct McpConnectionHealth {
    healthy: AtomicBool,
    connection_generation: Arc<AtomicU64>,
}

impl McpConnectionHealth {
    fn new(connection_generation: Arc<AtomicU64>) -> Self {
        Self {
            healthy: AtomicBool::new(true),
            connection_generation,
        }
    }

    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(crate) fn mark_unhealthy(&self) {
        if self.healthy.swap(false, Ordering::AcqRel) {
            self.connection_generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            connection_health: HashMap::new(),
            connection_call_timeout_secs: HashMap::new(),
            connected_servers: HashMap::new(),
            managed_processes: HashMap::new(),
            connection_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn advance_connection_generation(&self) {
        self.connection_generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Start a managed process for a built-in MCP connector.
    /// Returns the port the process is listening on.
    async fn start_managed_process(&mut self, server: &McpServer) -> Result<u16, CoreError> {
        let command = server
            .command
            .as_deref()
            .ok_or_else(|| CoreError::Mcp("Built-in connector missing command".into()))?;

        let port = find_free_port()?;

        let args: Vec<String> = match &server.args {
            Some(a) => parse_mcp_args(a)?,
            None => Vec::new(),
        }
        .into_iter()
        .map(|arg| expand_managed_arg(&arg, port))
        .collect();

        // Merge environment variables, adding PORT
        let mut env_vars: HashMap<String, String> = server
            .env_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        env_vars.insert("PORT".to_string(), port.to_string());

        // On Windows, Node.js CLI tools are batch scripts (.cmd) — Command::new
        // won't find them without the extension since it doesn't use PATHEXT.
        #[cfg(windows)]
        let effective_command = {
            let lower = command.to_ascii_lowercase();
            if ["npx", "node", "npm", "yarn", "pnpm", "bunx"].contains(&lower.as_str()) {
                format!("{command}.cmd")
            } else {
                command.to_string()
            }
        };
        #[cfg(not(windows))]
        let effective_command = command.to_string();

        let mut cmd = StdCommand::new(&effective_command);
        cmd.args(&args);
        cmd.envs(&env_vars);
        // Prevent the child from inheriting stdin (important on Windows)
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        crate::background_process::configure_std_background_process_group(&mut cmd);

        let child = cmd.spawn().map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to start managed server '{}': {e}. Is Node.js/npx installed?",
                server.name
            ))
        })?;

        tracing::info!(
            "Started managed MCP connector '{}' (PID {}) on port {}",
            server.name,
            child.id(),
            port
        );

        self.managed_processes.insert(server.id.clone(), child);

        // Wait for the server to accept connections
        let addr = format!("localhost:{}", port);
        let timeout = std::time::Duration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                // Kill the process on timeout
                if let Some(mut child) = self.managed_processes.remove(&server.id) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(CoreError::Mcp(format!(
                    "Managed server '{}' failed to start within {}s on port {}",
                    server.name,
                    timeout.as_secs(),
                    port
                )));
            }
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => {
                    tracing::info!("Managed server '{}' is ready on port {}", server.name, port);
                    break;
                }
                Err(_) => {
                    // Check if process is still alive
                    if let Some(child) = self.managed_processes.get_mut(&server.id) {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                self.managed_processes.remove(&server.id);
                                return Err(CoreError::Mcp(format!(
                                    "Managed server '{}' exited with {status}",
                                    server.name
                                )));
                            }
                            Ok(None) => {} // Still running
                            Err(e) => {
                                tracing::warn!("Error checking process status: {e}");
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        }

        Ok(port)
    }

    /// Connect to an MCP connector and return the tools it offers.
    pub async fn connect_server(
        &mut self,
        server: &McpServer,
        call_timeout_secs: Option<u64>,
    ) -> Result<Vec<McpToolInfo>, CoreError> {
        // Disconnect existing connection if any.
        self.disconnect_server(&server.id).await.ok();

        // For built-in servers with a command, start managed process first
        let effective_url = if server.builtin_id.is_some()
            && server.command.is_some()
            && server.transport != "stdio"
        {
            let port = self.start_managed_process(server).await?;
            let path = if server.transport == "sse" {
                "sse"
            } else {
                "mcp"
            };
            Some(format!("http://localhost:{port}/{path}"))
        } else {
            None
        };

        match server.transport.as_str() {
            "stdio" => {
                let command = server.command.as_deref().ok_or_else(|| {
                    CoreError::InvalidInput("stdio transport requires a command".into())
                })?;

                let args: Vec<String> = match &server.args {
                    Some(args_str) => parse_mcp_args(args_str)?,
                    None => Vec::new(),
                };

                let env: Option<HashMap<String, String>> = match &server.env_json {
                    Some(env_json) => Some(resolve_mcp_config_map("envJson", env_json)?),
                    None => None,
                };

                let mut client =
                    McpClient::connect_stdio(command, &args, env.as_ref(), &server.name).await?;
                if let Some(secs) = call_timeout_secs {
                    client.set_call_timeout(std::time::Duration::from_secs(secs));
                }
                let tools = client.list_tools().await?;
                self.clients
                    .insert(server.id.clone(), Arc::new(Mutex::new(client)));
                self.connection_health.insert(
                    server.id.clone(),
                    Arc::new(McpConnectionHealth::new(Arc::clone(
                        &self.connection_generation,
                    ))),
                );
                self.connection_call_timeout_secs
                    .insert(server.id.clone(), call_timeout_secs);
                self.connected_servers
                    .insert(server.id.clone(), server.clone());
                self.advance_connection_generation();
                Ok(tools)
            }
            "sse" | "streamable_http" => {
                let url = effective_url
                    .as_deref()
                    .or(server.url.as_deref())
                    .ok_or_else(|| {
                        CoreError::InvalidInput(format!(
                            "{} transport requires a URL",
                            server.transport
                        ))
                    })?;

                let headers: Option<HashMap<String, String>> = match &server.headers_json {
                    Some(headers_json) => {
                        Some(resolve_mcp_config_map("headersJson", headers_json)?)
                    }
                    None => None,
                };

                let mut client = if server.transport == "sse" {
                    McpClient::connect_sse(url, headers.as_ref(), &server.name).await?
                } else {
                    McpClient::connect_streamable_http(url, headers.as_ref(), &server.name).await?
                };
                if let Some(secs) = call_timeout_secs {
                    client.set_call_timeout(std::time::Duration::from_secs(secs));
                }
                let tools = client.list_tools().await?;
                self.clients
                    .insert(server.id.clone(), Arc::new(Mutex::new(client)));
                self.connection_health.insert(
                    server.id.clone(),
                    Arc::new(McpConnectionHealth::new(Arc::clone(
                        &self.connection_generation,
                    ))),
                );
                self.connection_call_timeout_secs
                    .insert(server.id.clone(), call_timeout_secs);
                self.connected_servers
                    .insert(server.id.clone(), server.clone());
                self.advance_connection_generation();
                Ok(tools)
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unsupported MCP transport: {other}. Expected 'stdio', 'sse', or 'streamable_http'."
            ))),
        }
    }

    /// Ensure the active connections match the currently enabled server set.
    /// Returns per-server connection failures without aborting healthy servers.
    pub async fn sync_servers(
        &mut self,
        servers: &[McpServer],
        call_timeout_secs: Option<u64>,
    ) -> HashMap<String, String> {
        let desired: HashMap<&str, &McpServer> = servers
            .iter()
            .map(|server| (server.id.as_str(), server))
            .collect();
        let connected_ids: Vec<String> = self.connected_servers.keys().cloned().collect();

        for server_id in connected_ids {
            if !desired.contains_key(server_id.as_str()) {
                self.disconnect_server(&server_id).await.ok();
            }
        }

        let mut errors = HashMap::new();
        for server in servers {
            if !self.server_needs_reconnect(server) {
                continue;
            }

            if let Err(err) = self.connect_server(server, call_timeout_secs).await {
                errors.insert(server.id.clone(), err.to_string());
                self.disconnect_server(&server.id).await.ok();
            }
        }

        errors
    }

    fn server_needs_reconnect(&self, server: &McpServer) -> bool {
        let config_changed = self
            .connected_servers
            .get(&server.id)
            .map(|current| runtime_config_changed(current, server))
            .unwrap_or(true);
        let connection_unhealthy = self
            .connection_health
            .get(&server.id)
            .map(|health| !health.is_healthy())
            .unwrap_or(true);
        config_changed || connection_unhealthy
    }

    /// Recover a failed client immediately and return the active connection.
    /// Calls are serialized by the shared manager mutex. If another tool has
    /// already replaced the failed client, reuse that newer connection.
    pub async fn recover_server_after_failure(
        &mut self,
        server_id: &str,
        failed_client: &Arc<Mutex<McpClient>>,
    ) -> Result<Arc<Mutex<McpClient>>, CoreError> {
        let current_client = self.clients.get(server_id).cloned().ok_or_else(|| {
            CoreError::Internal(format!(
                "MCP connector {server_id} has no active client to recover"
            ))
        })?;
        if !Arc::ptr_eq(&current_client, failed_client) {
            return Ok(current_client);
        }
        if let Some(health) = self.connection_health.get(server_id) {
            health.mark_unhealthy();
        }

        let server = self
            .connected_servers
            .get(server_id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("MCP connector {server_id}")))?;
        let call_timeout_secs = self
            .connection_call_timeout_secs
            .get(server_id)
            .copied()
            .flatten();
        self.connect_server(&server, call_timeout_secs).await?;
        self.clients.get(server_id).cloned().ok_or_else(|| {
            CoreError::Internal(format!(
                "MCP connector {server_id} reconnected without an active client"
            ))
        })
    }

    /// Disconnect and shut down a specific MCP connector.
    pub async fn disconnect_server(&mut self, server_id: &str) -> Result<(), CoreError> {
        let removed_server = self.connected_servers.remove(server_id).is_some();
        let client = self.clients.remove(server_id);
        let removed_client = client.is_some();
        let removed_health = self.connection_health.remove(server_id).is_some();
        self.connection_call_timeout_secs.remove(server_id);
        if let Some(client) = client {
            let mut guard = client.lock().await;
            guard.shutdown().await.ok();
        }
        // Kill managed process if present
        let process = self.managed_processes.remove(server_id);
        let removed_process = process.is_some();
        if let Some(mut child) = process {
            tracing::info!("Killing managed process for server {}", server_id);
            let _ = child.kill();
            let _ = child.wait();
        }
        if removed_server || removed_client || removed_health || removed_process {
            self.advance_connection_generation();
        }
        Ok(())
    }

    /// Monotonic identity for the live client set. Registries that capture MCP
    /// client Arcs must include this value in their cache key.
    pub fn connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    /// Disconnect all MCP connectors.
    pub async fn disconnect_all(&mut self) {
        let ids: Vec<String> = self.clients.keys().cloned().collect();
        for id in ids {
            self.disconnect_server(&id).await.ok();
        }
        // Kill all remaining managed processes
        for (id, mut child) in self.managed_processes.drain() {
            tracing::info!("Killing managed process for server {}", id);
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Shutdown all connections and kill all managed processes.
    /// Call this when the app is closing.
    pub async fn shutdown(&mut self) {
        self.disconnect_all().await;
    }

    /// Get a client reference for tool execution.
    pub fn get_client(&self, server_id: &str) -> Option<Arc<Mutex<McpClient>>> {
        self.clients.get(server_id).cloned()
    }

    /// Register all MCP tools from connected servers into a ToolRegistry.
    pub async fn register_tools(&self, registry: &mut ToolRegistry) -> Result<(), CoreError> {
        self.register_tools_inner(registry, None).await
    }

    /// Register tools that can eagerly recover a failed connection for later
    /// calls in the same agent turn. The failed call itself is never retried,
    /// because an MCP mutation may already have reached the server.
    pub async fn register_tools_with_recovery(
        &self,
        registry: &mut ToolRegistry,
        manager: Weak<Mutex<McpManager>>,
    ) -> Result<(), CoreError> {
        self.register_tools_inner(registry, Some(manager)).await
    }

    async fn register_tools_inner(
        &self,
        registry: &mut ToolRegistry,
        recovery_manager: Option<Weak<Mutex<McpManager>>>,
    ) -> Result<(), CoreError> {
        for (server_id, client) in &self.clients {
            let health = self.connection_health.get(server_id).ok_or_else(|| {
                CoreError::Internal(format!(
                    "MCP connector {server_id} has no connection health state"
                ))
            })?;
            let tools = {
                let mut guard = client.lock().await;
                match guard.list_tools().await {
                    Ok(tools) => tools,
                    Err(error) => {
                        health.mark_unhealthy();
                        return Err(error);
                    }
                }
            };
            let server_name = self
                .connected_servers
                .get(server_id)
                .map(|server| server.name.as_str())
                .unwrap_or("mcp");
            let client_slot = Arc::new(McpClientSlot::new(Arc::clone(client)));
            for tool_info in tools {
                let server_slug = mcp_registry_slug(server_name, "server");
                let tool_slug = mcp_registry_slug(&tool_info.name, "tool");
                let mut registry_name = format!("mcp__{server_slug}__{tool_slug}");
                if registry.contains(&registry_name) {
                    registry_name =
                        format!("{registry_name}__{}", &server_id[..8.min(server_id.len())]);
                }
                let mcp_tool = McpTool::new(
                    tool_info,
                    Arc::clone(&client_slot),
                    server_id.clone(),
                    registry_name,
                    server_name.to_string(),
                    Arc::clone(health),
                    recovery_manager.clone(),
                );
                registry.register(Box::new(mcp_tool));
            }
        }
        Ok(())
    }
}

fn mcp_registry_slug(value: &str, fallback: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            _ => '_',
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener as TokioTcpListener;

    #[test]
    fn parse_mcp_args_accepts_json_array() {
        let parsed =
            parse_mcp_args(r#"["-y","@modelcontextprotocol/server-filesystem","D:/vault"]"#)
                .unwrap();
        assert_eq!(
            parsed,
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "D:/vault".to_string()
            ]
        );
    }

    #[test]
    fn environment_placeholders_resolve_without_persisting_secret_values() {
        let path = std::env::var("PATH").expect("PATH is available in the test environment");
        assert_eq!(
            resolve_env_placeholders("Bearer ${env:PATH}").unwrap(),
            format!("Bearer {path}")
        );

        let error = resolve_env_placeholders("${env:NEXA_MCP_MISSING_ENV_TEST_9F31}")
            .unwrap_err()
            .to_string();
        assert!(error.contains("NEXA_MCP_MISSING_ENV_TEST_9F31"));
        assert!(!error.contains(&path));
    }

    #[test]
    fn parse_mcp_args_accepts_legacy_text_formats() {
        assert_eq!(
            parse_mcp_args("-y, @modelcontextprotocol/server-filesystem, D:/vault").unwrap(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "D:/vault".to_string()
            ]
        );
        assert_eq!(
            parse_mcp_args("-y\n@modelcontextprotocol/server-filesystem\nD:/vault").unwrap(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "D:/vault".to_string()
            ]
        );
    }

    #[test]
    fn save_mcp_server_rejects_unknown_transport() {
        let db = Database::open_memory().unwrap();
        let result = db.save_mcp_server(&SaveMcpServerInput {
            id: None,
            name: "Remote".into(),
            transport: "websocket".into(),
            command: None,
            args: None,
            url: Some("http://localhost:8080/mcp".into()),
            env_json: None,
            headers_json: None,
            enabled: true,
        });
        assert!(result.is_err());
    }

    #[test]
    fn save_mcp_server_normalizes_remote_transport() {
        let db = Database::open_memory().unwrap();
        let server = db
            .save_mcp_server(&SaveMcpServerInput {
                id: None,
                name: "Remote".into(),
                transport: "streamable_http".into(),
                command: None,
                args: None,
                url: Some("https://example.com/mcp".into()),
                env_json: None,
                headers_json: Some(r#"{"Authorization":"Bearer token"}"#.into()),
                enabled: true,
            })
            .unwrap();

        assert_eq!(server.transport, "streamable_http");
        assert_eq!(server.url.as_deref(), Some("https://example.com/mcp"));
        assert_eq!(
            server.headers_json.as_deref(),
            Some(r#"{"Authorization":"Bearer token"}"#)
        );
        assert_eq!(server.command, None);
        assert_eq!(server.args, None);
        assert_eq!(server.env_json, None);
    }

    #[test]
    fn save_mcp_server_requires_url_for_remote_transport() {
        let db = Database::open_memory().unwrap();
        let result = db.save_mcp_server(&SaveMcpServerInput {
            id: None,
            name: "Remote".into(),
            transport: "sse".into(),
            command: None,
            args: None,
            url: None,
            env_json: None,
            headers_json: None,
            enabled: true,
        });

        assert!(result.is_err());
    }

    #[test]
    fn save_mcp_server_normalizes_args_and_env() {
        let db = Database::open_memory().unwrap();
        let server = db
            .save_mcp_server(&SaveMcpServerInput {
                id: None,
                name: "Filesystem".into(),
                transport: "stdio".into(),
                command: Some("npx".into()),
                args: Some("-y, @modelcontextprotocol/server-filesystem, D:/vault".into()),
                url: None,
                env_json: Some(r#"{"API_KEY":"secret"}"#.into()),
                headers_json: None,
                enabled: true,
            })
            .unwrap();

        assert_eq!(
            server.args.as_deref(),
            Some(r#"["-y","@modelcontextprotocol/server-filesystem","D:/vault"]"#)
        );
        assert_eq!(server.env_json.as_deref(), Some(r#"{"API_KEY":"secret"}"#));
    }

    #[test]
    fn mcp_registry_slug_normalizes_names() {
        assert_eq!(mcp_registry_slug("Web Search", "server"), "web_search");
        assert_eq!(mcp_registry_slug("search.query", "tool"), "search_query");
        assert_eq!(mcp_registry_slug("!!!", "tool"), "tool");
    }

    #[tokio::test]
    async fn disconnecting_a_live_server_advances_registry_generation() {
        let mut manager = McpManager::new();
        manager.connected_servers.insert(
            "server-1".into(),
            McpServer {
                id: "server-1".into(),
                name: "Test".into(),
                transport: "streamable_http".into(),
                command: None,
                args: None,
                url: Some("https://example.test/mcp".into()),
                env_json: None,
                headers_json: None,
                enabled: true,
                created_at: String::new(),
                updated_at: String::new(),
                builtin_id: None,
            },
        );

        let before = manager.connection_generation();
        manager.disconnect_server("server-1").await.unwrap();

        assert!(manager.connection_generation() > before);
    }

    #[test]
    fn failed_tool_connection_invalidates_snapshot_and_requires_reconnect() {
        let mut manager = McpManager::new();
        let server = McpServer {
            id: "server-1".into(),
            name: "Test".into(),
            transport: "streamable_http".into(),
            command: None,
            args: None,
            url: Some("https://example.test/mcp".into()),
            env_json: None,
            headers_json: None,
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin_id: None,
        };
        let health = Arc::new(McpConnectionHealth::new(Arc::clone(
            &manager.connection_generation,
        )));
        manager
            .connected_servers
            .insert(server.id.clone(), server.clone());
        manager
            .connection_health
            .insert(server.id.clone(), Arc::clone(&health));

        assert!(!manager.server_needs_reconnect(&server));
        let before = manager.connection_generation();
        health.mark_unhealthy();

        assert!(manager.connection_generation() > before);
        assert!(manager.server_needs_reconnect(&server));
    }

    async fn read_test_http_request(
        stream: &mut tokio::net::TcpStream,
    ) -> std::io::Result<(String, serde_json::Value)> {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "request closed before headers",
                ));
            }
            bytes.extend_from_slice(&chunk[..read]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let method = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or_default()
            .to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        let payload = if content_length == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap()
        };
        Ok((method, payload))
    }

    async fn write_test_http_response(
        stream: &mut tokio::net::TcpStream,
        status: &str,
        payload: Option<&serde_json::Value>,
    ) -> std::io::Result<()> {
        let body = payload
            .map(|value| serde_json::to_vec(value).map_err(std::io::Error::other))
            .transpose()?
            .unwrap_or_default();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(&body).await?;
        stream.flush().await
    }

    #[tokio::test]
    async fn only_transport_failure_recovers_connection_for_the_next_call() {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let initialize_calls = Arc::new(AtomicUsize::new(0));
        let recovery_started = Arc::new(tokio::sync::Semaphore::new(0));
        let recovery_release = Arc::new(tokio::sync::Semaphore::new(0));
        let server_calls = Arc::clone(&tool_calls);
        let server_initializes = Arc::clone(&initialize_calls);
        let server_recovery_started = Arc::clone(&recovery_started);
        let server_recovery_release = Arc::clone(&recovery_release);
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let tool_calls = Arc::clone(&server_calls);
                let initialize_calls = Arc::clone(&server_initializes);
                let recovery_started = Arc::clone(&server_recovery_started);
                let recovery_release = Arc::clone(&server_recovery_release);
                tokio::spawn(async move {
                    let (http_method, request) = read_test_http_request(&mut stream).await.unwrap();
                    if http_method == "DELETE" {
                        write_test_http_response(&mut stream, "204 No Content", None)
                            .await
                            .unwrap();
                        return;
                    }
                    let method = request
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let id = request
                        .get("id")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    match method {
                        "initialize" => {
                            if initialize_calls.fetch_add(1, Ordering::SeqCst) > 0 {
                                recovery_started.add_permits(1);
                                let _permit = recovery_release.acquire().await.unwrap();
                            }
                            let response = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "protocolVersion": "2025-11-25",
                                    "capabilities": {},
                                    "serverInfo": { "name": "remote", "version": "1.0.0" }
                                }
                            });
                            write_test_http_response(&mut stream, "200 OK", Some(&response))
                                .await
                                .unwrap();
                        }
                        "notifications/initialized" => {
                            write_test_http_response(&mut stream, "202 Accepted", None)
                                .await
                                .unwrap();
                        }
                        "tools/list" => {
                            let response = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "tools": [{
                                        "name": "demo",
                                        "description": "Demo tool",
                                        "inputSchema": { "type": "object", "properties": {} }
                                    }]
                                }
                            });
                            write_test_http_response(&mut stream, "200 OK", Some(&response))
                                .await
                                .unwrap();
                        }
                        "tools/call" => match tool_calls.fetch_add(1, Ordering::SeqCst) {
                            0 => {
                                let response = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": {
                                        "code": -32602,
                                        "message": "Invalid tool arguments"
                                    }
                                });
                                write_test_http_response(&mut stream, "200 OK", Some(&response))
                                    .await
                                    .unwrap();
                            }
                            1 => {
                                write_test_http_response(
                                    &mut stream,
                                    "500 Internal Server Error",
                                    Some(&serde_json::json!({ "error": "connection lost" })),
                                )
                                .await
                                .unwrap();
                            }
                            _ => {
                                let response = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "content": [{ "type": "text", "text": "recovered" }]
                                    }
                                });
                                write_test_http_response(&mut stream, "200 OK", Some(&response))
                                    .await
                                    .unwrap();
                            }
                        },
                        other => panic!("unexpected MCP method {other}"),
                    }
                });
            }
        });

        let manager = Arc::new(Mutex::new(McpManager::new()));
        let server = McpServer {
            id: "server-1".into(),
            name: "Remote".into(),
            transport: "streamable_http".into(),
            command: None,
            args: None,
            url: Some(format!("http://{address}/mcp")),
            env_json: None,
            headers_json: None,
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin_id: None,
        };
        let mut registry = ToolRegistry::new();
        {
            let mut guard = manager.lock().await;
            guard.connect_server(&server, Some(5)).await.unwrap();
            guard
                .register_tools_with_recovery(&mut registry, Arc::downgrade(&manager))
                .await
                .unwrap();
        }
        let db = Database::open_memory().unwrap();
        let source_scope = Vec::new();
        let tool = registry.get("mcp__remote__demo").unwrap();
        let application_error = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "call-1",
                "{}",
                &db,
                &source_scope,
            ))
            .await
            .unwrap();
        assert!(application_error.is_error);
        assert!(application_error.content.contains("Invalid tool arguments"));
        assert!(!application_error.content.contains("recovery"));
        assert_eq!(initialize_calls.load(Ordering::SeqCst), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), recovery_started.acquire())
                .await
                .is_err()
        );

        let transport_error = tokio::time::timeout(
            Duration::from_millis(250),
            tool.execute(crate::tools::ToolExecutionContext::new(
                "call-2",
                "{}",
                &db,
                &source_scope,
            )),
        )
        .await
        .expect("the original tool failure must not wait for reconnect")
        .unwrap();
        assert!(transport_error.is_error);
        assert!(transport_error
            .content
            .contains("recovery scheduled for subsequent calls"));
        tokio::time::timeout(Duration::from_secs(1), recovery_started.acquire())
            .await
            .expect("background recovery starts")
            .unwrap()
            .forget();
        recovery_release.add_permits(1);

        let second = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "call-3",
                "{}",
                &db,
                &source_scope,
            ))
            .await
            .unwrap();
        assert!(!second.is_error);
        assert_eq!(second.content, "recovered");
        assert_eq!(tool_calls.load(Ordering::SeqCst), 3);

        server_task.abort();
    }

    #[test]
    fn expand_managed_arg_replaces_port_placeholder() {
        assert_eq!(expand_managed_arg("--port=${PORT}", 8931), "--port=8931");
        assert_eq!(expand_managed_arg("static", 8931), "static");
    }
}
