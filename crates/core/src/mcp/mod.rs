//! MCP (Model Context Protocol) module — client, manager, and data models.

pub mod client;

use crate::db::Database;
use crate::error::CoreError;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpListener;
use std::process::{Child, Command as StdCommand};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use self::client::McpClient;
use crate::tools::mcp_tool::McpTool;
use crate::tools::ToolRegistry;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// Persisted MCP server configuration.
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
    /// Non-`None` for built-in servers managed by the app (e.g. "open-websearch").
    /// Built-in servers cannot be deleted and have their process lifecycle managed.
    pub builtin_id: Option<String>,
}

/// Input for creating or updating an MCP server configuration.
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

/// Tool information returned by an MCP server.
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

    let parsed: HashMap<String, String> = serde_json::from_str(&raw).map_err(|e| {
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
    let name = normalize_required_text("MCP server name", &input.name)?;
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

// ---------------------------------------------------------------------------
// Database CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// List all MCP servers, newest first.
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

    /// Create or update an MCP server configuration.
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

    /// Delete an MCP server by ID.
    pub fn delete_mcp_server(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        // Prevent deletion of built-in servers.
        let builtin: Option<String> = conn
            .query_row(
                "SELECT builtin_id FROM mcp_servers WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .map_err(|_| CoreError::NotFound(format!("MCP server {id}")))?;
        if builtin.is_some() {
            return Err(CoreError::InvalidInput(
                "Cannot delete built-in MCP server".into(),
            ));
        }
        let affected = conn.execute(
            "DELETE FROM mcp_servers WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("MCP server {id}")));
        }
        Ok(())
    }

    /// Toggle an MCP server's enabled state.
    pub fn toggle_mcp_server(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE mcp_servers SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("MCP server {id}")));
        }
        Ok(())
    }

    /// Get only enabled MCP servers.
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
        .map_err(|_| CoreError::NotFound(format!("MCP server {id}")))
    }
}

// ---------------------------------------------------------------------------
// MCP Manager
// ---------------------------------------------------------------------------

/// Manages MCP server connections and their lifecycle.
pub struct McpManager {
    clients: HashMap<String, Arc<Mutex<McpClient>>>,
    connected_servers: HashMap<String, McpServer>,
    managed_processes: HashMap<String, Child>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            connected_servers: HashMap::new(),
            managed_processes: HashMap::new(),
        }
    }

    /// Start a managed process for a built-in MCP server.
    /// Returns the port the process is listening on.
    async fn start_managed_process(&mut self, server: &McpServer) -> Result<u16, CoreError> {
        let command = server
            .command
            .as_deref()
            .ok_or_else(|| CoreError::Mcp("Built-in server missing command".into()))?;

        let port = find_free_port()?;

        let args: Vec<String> = match &server.args {
            Some(a) => parse_mcp_args(a)?,
            None => Vec::new(),
        };

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

        // On Windows, create process in a new process group and hide the console window
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }

        let child = cmd.spawn().map_err(|e| {
            CoreError::Mcp(format!(
                "Failed to start managed server '{}': {e}. Is Node.js/npx installed?",
                server.name
            ))
        })?;

        tracing::info!(
            "Started managed MCP server '{}' (PID {}) on port {}",
            server.name,
            child.id(),
            port
        );

        self.managed_processes.insert(server.id.clone(), child);

        // Wait for the server to accept connections
        let addr = format!("127.0.0.1:{}", port);
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

    /// Connect to an MCP server and return the tools it offers.
    pub async fn connect_server(
        &mut self,
        server: &McpServer,
        call_timeout_secs: Option<u64>,
    ) -> Result<Vec<McpToolInfo>, CoreError> {
        // Disconnect existing connection if any.
        self.disconnect_server(&server.id).await.ok();

        // For built-in servers with a command, start managed process first
        let effective_url = if server.builtin_id.is_some() && server.command.is_some() {
            let port = self.start_managed_process(server).await?;
            Some(format!("http://localhost:{}/mcp", port))
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

                let env: Option<HashMap<String, String>> =
                    match &server.env_json {
                        Some(env_json) => Some(serde_json::from_str(env_json).map_err(|e| {
                            CoreError::InvalidInput(format!("Invalid envJson: {e}"))
                        })?),
                        None => None,
                    };

                let mut client =
                    McpClient::connect_stdio(&command, &args, env.as_ref(), &server.name).await?;
                if let Some(secs) = call_timeout_secs {
                    client.set_call_timeout(std::time::Duration::from_secs(secs));
                }
                let tools = client.list_tools().await?;
                self.clients
                    .insert(server.id.clone(), Arc::new(Mutex::new(client)));
                self.connected_servers
                    .insert(server.id.clone(), server.clone());
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
                        Some(serde_json::from_str(headers_json).map_err(|e| {
                            CoreError::InvalidInput(format!("Invalid headersJson: {e}"))
                        })?)
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
                self.connected_servers
                    .insert(server.id.clone(), server.clone());
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
            let needs_reconnect = self
                .connected_servers
                .get(&server.id)
                .map(|current| runtime_config_changed(current, server))
                .unwrap_or(true);

            if !needs_reconnect {
                continue;
            }

            if let Err(err) = self.connect_server(server, call_timeout_secs).await {
                errors.insert(server.id.clone(), err.to_string());
                self.disconnect_server(&server.id).await.ok();
            }
        }

        errors
    }

    /// Disconnect and shut down a specific MCP server.
    pub async fn disconnect_server(&mut self, server_id: &str) -> Result<(), CoreError> {
        self.connected_servers.remove(server_id);
        if let Some(client) = self.clients.remove(server_id) {
            let mut guard = client.lock().await;
            guard.shutdown().await.ok();
        }
        // Kill managed process if present
        if let Some(mut child) = self.managed_processes.remove(server_id) {
            tracing::info!("Killing managed process for server {}", server_id);
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    /// Disconnect all MCP servers.
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
        for (server_id, client) in &self.clients {
            let tools = {
                let mut guard = client.lock().await;
                guard.list_tools().await?
            };
            let server_name = self
                .connected_servers
                .get(server_id)
                .map(|server| server.name.as_str())
                .unwrap_or("mcp");
            for tool_info in tools {
                let mut registry_name = tool_info.name.clone();
                if registry.contains(&registry_name) {
                    let server_slug = server_name
                        .chars()
                        .map(|ch| match ch {
                            'a'..='z' | '0'..='9' => ch,
                            'A'..='Z' => ch.to_ascii_lowercase(),
                            _ => '_',
                        })
                        .collect::<String>()
                        .trim_matches('_')
                        .to_string();
                    let tool_slug = tool_info
                        .name
                        .chars()
                        .map(|ch| match ch {
                            'a'..='z' | '0'..='9' => ch,
                            'A'..='Z' => ch.to_ascii_lowercase(),
                            _ => '_',
                        })
                        .collect::<String>()
                        .trim_matches('_')
                        .to_string();
                    registry_name = format!(
                        "mcp__{}__{}",
                        if server_slug.is_empty() {
                            "server"
                        } else {
                            &server_slug
                        },
                        if tool_slug.is_empty() {
                            "tool"
                        } else {
                            &tool_slug
                        }
                    );
                    if registry.contains(&registry_name) {
                        registry_name =
                            format!("{registry_name}__{}", &server_id[..8.min(server_id.len())]);
                    }
                }
                let mcp_tool = McpTool::new(
                    tool_info,
                    client.clone(),
                    server_id.clone(),
                    registry_name,
                    server_name.to_string(),
                );
                registry.register(Box::new(mcp_tool));
            }
        }
        Ok(())
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
}
