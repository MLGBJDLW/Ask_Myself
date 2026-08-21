use super::{normalize_save_input, McpServer, SaveMcpServerInput};
use crate::db::Database;
use crate::error::CoreError;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const MCP_CONFIG_FILE_NAME: &str = "mcp-connectors.json";
pub const USER_JSON_ID_PREFIX: &str = "user-json:";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_CONNECTORS: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpConnectorDocument {
    pub version: u32,
    #[serde(default)]
    pub connectors: BTreeMap<String, McpConnectorDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpConnectorDeclaration {
    pub name: String,
    pub transport: McpConnectorTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpConnectorTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigReloadReport {
    pub path: String,
    pub imported: usize,
    pub removed: usize,
    pub disabled_after_change: usize,
}

pub fn user_mcp_config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(MCP_CONFIG_FILE_NAME)
}

pub fn ensure_user_mcp_config(path: &Path) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let document = McpConnectorDocument {
        version: 1,
        connectors: BTreeMap::new(),
    };
    let mut encoded = serde_json::to_string_pretty(&document)?;
    encoded.push('\n');
    std::fs::write(path, encoded)?;
    Ok(())
}

fn validate_connector_key(key: &str) -> Result<(), CoreError> {
    if key.is_empty() || key.len() > 64 {
        return Err(CoreError::InvalidInput(format!(
            "MCP config connector id '{key}' must contain 1-64 characters"
        )));
    }
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(CoreError::InvalidInput(format!(
            "MCP config connector id '{key}' may contain only letters, numbers, '.', '-' and '_'"
        )));
    }
    Ok(())
}

fn is_secret_shaped_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    let tokens: Vec<&str> = upper
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    tokens.iter().any(|token| {
        matches!(
            *token,
            "AUTHORIZATION" | "COOKIE" | "TOKEN" | "SECRET" | "PASSWORD" | "PASSWD"
        )
    }) || tokens
        .windows(2)
        .any(|pair| matches!(pair, ["API", "KEY"] | ["PRIVATE", "KEY"]))
        || matches!(upper.as_str(), "APIKEY" | "PRIVATEKEY")
}

fn validate_env_references(value: &str) -> Result<bool, CoreError> {
    let mut found = false;
    let mut remaining = value;
    while let Some(start) = remaining.find("${env:") {
        found = true;
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
        remaining = &placeholder[end + 1..];
    }
    Ok(found)
}

fn validate_config_map(
    connector_id: &str,
    field: &str,
    values: &BTreeMap<String, String>,
) -> Result<(), CoreError> {
    for (key, value) in values {
        if key.trim().is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "MCP config connector '{connector_id}' has an empty {field} key"
            )));
        }
        let references_environment = validate_env_references(value)?;
        if is_secret_shaped_key(key) && !references_environment {
            return Err(CoreError::InvalidInput(format!(
                "MCP config connector '{connector_id}' must reference secret-shaped {field} '{key}' with ${{env:VARIABLE}} instead of storing a literal value"
            )));
        }
    }
    Ok(())
}

fn declaration_to_input(
    key: &str,
    declaration: &McpConnectorDeclaration,
) -> Result<SaveMcpServerInput, CoreError> {
    validate_connector_key(key)?;
    let id = Some(format!("{USER_JSON_ID_PREFIX}{key}"));
    let input = match &declaration.transport {
        McpConnectorTransport::Stdio { command, args, env } => {
            validate_config_map(key, "environment variable", env)?;
            SaveMcpServerInput {
                id,
                name: declaration.name.clone(),
                transport: "stdio".into(),
                command: Some(command.clone()),
                args: if args.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(args)?)
                },
                url: None,
                env_json: if env.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(env)?)
                },
                headers_json: None,
                enabled: false,
            }
        }
        McpConnectorTransport::StreamableHttp { url, headers }
        | McpConnectorTransport::Sse { url, headers } => {
            validate_config_map(key, "header", headers)?;
            SaveMcpServerInput {
                id,
                name: declaration.name.clone(),
                transport: if matches!(
                    &declaration.transport,
                    McpConnectorTransport::StreamableHttp { .. }
                ) {
                    "streamable_http".into()
                } else {
                    "sse".into()
                },
                command: None,
                args: None,
                url: Some(url.clone()),
                env_json: None,
                headers_json: if headers.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(headers)?)
                },
                enabled: false,
            }
        }
    };
    normalize_save_input(&input)
}

fn same_trust_relevant_config(server: &McpServer, input: &SaveMcpServerInput) -> bool {
    server.name == input.name
        && server.transport == input.transport
        && server.command == input.command
        && server.args == input.args
        && server.url == input.url
        && same_json_string_map(&server.env_json, &input.env_json)
        && same_json_string_map(&server.headers_json, &input.headers_json)
}

fn same_json_string_map(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            let left = serde_json::from_str::<BTreeMap<String, String>>(left);
            let right = serde_json::from_str::<BTreeMap<String, String>>(right);
            matches!((left, right), (Ok(left), Ok(right)) if left == right)
        }
        _ => false,
    }
}

pub fn reload_user_mcp_config(
    db: &Database,
    path: &Path,
) -> Result<McpConfigReloadReport, CoreError> {
    ensure_user_mcp_config(path)?;
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "MCP config file exceeds the {} byte limit",
            MAX_CONFIG_BYTES
        )));
    }
    let raw = std::fs::read_to_string(path)?;
    let document: McpConnectorDocument = serde_json::from_str(&raw).map_err(|error| {
        CoreError::InvalidInput(format!(
            "Invalid MCP config at line {}, column {}: {error}",
            error.line(),
            error.column()
        ))
    })?;
    if document.version != 1 {
        return Err(CoreError::InvalidInput(format!(
            "Unsupported MCP config version {}; expected version 1",
            document.version
        )));
    }
    if document.connectors.len() > MAX_CONNECTORS {
        return Err(CoreError::InvalidInput(format!(
            "MCP config contains {} connectors; the maximum is {MAX_CONNECTORS}",
            document.connectors.len()
        )));
    }

    let mut desired = Vec::with_capacity(document.connectors.len());
    for (key, declaration) in &document.connectors {
        desired.push(declaration_to_input(key, declaration)?);
    }

    let desired_ids: HashSet<String> = desired
        .iter()
        .filter_map(|input| input.id.clone())
        .collect();
    let mut disabled_after_change = 0;

    let mut conn = db.conn();
    let tx = conn.transaction()?;
    // Activation is user-owned mutable state. Read it under the same database
    // lock and transaction as the projection write so a concurrent toggle is
    // ordered entirely before or after this reload, never overwritten from a
    // stale pre-transaction snapshot.
    let existing: HashMap<String, McpServer> = {
        let mut stmt = tx.prepare(
            "SELECT id, name, transport, command, args, url, env_json, headers_json,
                    enabled, created_at, updated_at, builtin_id
             FROM mcp_servers
             WHERE id LIKE ?1",
        )?;
        let rows = stmt.query_map(params![format!("{USER_JSON_ID_PREFIX}%")], |row| {
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
        let mut existing = HashMap::new();
        for server in rows {
            let server = server?;
            existing.insert(server.id.clone(), server);
        }
        existing
    };
    for input in &desired {
        let id = input.id.as_deref().expect("file connector id is assigned");
        let enabled = existing
            .get(id)
            .filter(|server| same_trust_relevant_config(server, input))
            .map(|server| server.enabled)
            .unwrap_or(false);
        if existing
            .get(id)
            .is_some_and(|server| server.enabled && !same_trust_relevant_config(server, input))
        {
            disabled_after_change += 1;
        }
        tx.execute(
            "INSERT INTO mcp_servers
                (id, name, transport, command, args, url, env_json, headers_json, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                transport = excluded.transport,
                command = excluded.command,
                args = excluded.args,
                url = excluded.url,
                env_json = excluded.env_json,
                headers_json = excluded.headers_json,
                enabled = excluded.enabled,
                updated_at = datetime('now')
             WHERE mcp_servers.builtin_id IS NULL",
            params![
                id,
                &input.name,
                &input.transport,
                &input.command,
                &input.args,
                &input.url,
                &input.env_json,
                &input.headers_json,
                enabled as i32,
            ],
        )?;
    }

    let mut removed = 0;
    for id in existing.keys().filter(|id| !desired_ids.contains(*id)) {
        removed += tx.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
    }
    tx.commit()?;

    Ok(McpConfigReloadReport {
        path: path.to_string_lossy().into_owned(),
        imported: desired.len(),
        removed,
        disabled_after_change,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_config(path: &Path, raw: &str) {
        fs::write(path, raw).unwrap();
    }

    fn file_connector(db: &Database, id: &str) -> McpServer {
        db.list_mcp_servers()
            .unwrap()
            .into_iter()
            .find(|server| server.id == format!("{USER_JSON_ID_PREFIX}{id}"))
            .unwrap_or_else(|| panic!("missing JSON connector {id}"))
    }

    #[test]
    fn reload_keeps_unchanged_activation_and_disables_execution_changes() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        write_config(
            &path,
            r#"{
              "version": 1,
              "connectors": {
                "docs": {
                  "name": "Docs",
                  "transport": { "type": "stdio", "command": "docs-mcp", "args": ["--safe"] }
                }
              }
            }"#,
        );

        let first = reload_user_mcp_config(&db, &path).unwrap();
        assert_eq!(first.imported, 1);
        let server = file_connector(&db, "docs");
        assert_eq!(server.id, "user-json:docs");
        assert!(!server.enabled);

        db.toggle_mcp_server(&server.id, true).unwrap();
        reload_user_mcp_config(&db, &path).unwrap();
        assert!(file_connector(&db, "docs").enabled);

        write_config(
            &path,
            r#"{
              "version": 1,
              "connectors": {
                "docs": {
                  "name": "Docs",
                  "transport": { "type": "stdio", "command": "docs-mcp-v2", "args": ["--safe"] }
                }
              }
            }"#,
        );
        let changed = reload_user_mcp_config(&db, &path).unwrap();
        assert_eq!(changed.disabled_after_change, 1);
        assert!(!file_connector(&db, "docs").enabled);
    }

    #[test]
    fn reload_compares_environment_maps_independently_of_key_order() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        write_config(
            &path,
            r#"{
              "version": 1,
              "connectors": {
                "docs": {
                  "name": "Docs",
                  "transport": {
                    "type": "stdio",
                    "command": "docs-mcp",
                    "env": { "ALPHA": "one", "BRAVO": "two" }
                  }
                }
              }
            }"#,
        );
        reload_user_mcp_config(&db, &path).unwrap();
        db.conn()
            .execute(
                "UPDATE mcp_servers SET env_json = ?1, enabled = 1 WHERE id = ?2",
                params![r#"{"BRAVO":"two","ALPHA":"one"}"#, "user-json:docs"],
            )
            .unwrap();

        let report = reload_user_mcp_config(&db, &path).unwrap();

        assert_eq!(report.disabled_after_change, 0);
        assert!(file_connector(&db, "docs").enabled);
    }

    #[test]
    fn invalid_json_retains_last_known_good_projection() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        write_config(
            &path,
            r#"{"version":1,"connectors":{"docs":{"name":"Docs","transport":{"type":"streamable_http","url":"https://example.com/mcp"}}}}"#,
        );
        reload_user_mcp_config(&db, &path).unwrap();
        write_config(&path, "{ invalid");

        let error = reload_user_mcp_config(&db, &path).unwrap_err().to_string();
        assert!(error.contains("line 1"));
        assert_eq!(
            db.list_mcp_servers()
                .unwrap()
                .into_iter()
                .filter(|server| server.id.starts_with(USER_JSON_ID_PREFIX))
                .count(),
            1
        );
    }

    #[test]
    fn reload_removes_only_connectors_owned_by_the_json_source() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        db.save_mcp_server(&SaveMcpServerInput {
            id: None,
            name: "Managed".into(),
            transport: "streamable_http".into(),
            command: None,
            args: None,
            url: Some("https://example.com/managed".into()),
            env_json: None,
            headers_json: None,
            enabled: false,
        })
        .unwrap();
        write_config(
            &path,
            r#"{"version":1,"connectors":{"docs":{"name":"Docs","transport":{"type":"streamable_http","url":"https://example.com/mcp"}}}}"#,
        );
        reload_user_mcp_config(&db, &path).unwrap();

        write_config(&path, r#"{"version":1,"connectors":{}}"#);
        let report = reload_user_mcp_config(&db, &path).unwrap();
        let remaining = db.list_mcp_servers().unwrap();
        assert_eq!(report.removed, 1);
        assert!(remaining
            .iter()
            .all(|server| !server.id.starts_with(USER_JSON_ID_PREFIX)));
        assert!(remaining.iter().any(|server| server.name == "Managed"));
        assert!(remaining.iter().any(|server| server.builtin_id.is_some()));
    }

    #[test]
    fn secret_shaped_values_require_environment_references() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        write_config(
            &path,
            r#"{"version":1,"connectors":{"github":{"name":"GitHub","transport":{"type":"stdio","command":"github-mcp","env":{"GITHUB_TOKEN":"literal-secret"}}}}}"#,
        );

        let error = reload_user_mcp_config(&db, &path).unwrap_err().to_string();
        assert!(error.contains("${env:VARIABLE}"));
        assert!(db
            .list_mcp_servers()
            .unwrap()
            .into_iter()
            .all(|server| !server.id.starts_with(USER_JSON_ID_PREFIX)));
    }

    #[test]
    fn ordinary_keys_containing_secret_substrings_accept_literal_values() {
        let dir = tempdir().unwrap();
        let path = user_mcp_config_path(dir.path());
        let db = Database::open_memory().unwrap();
        write_config(
            &path,
            r#"{"version":1,"connectors":{"tools":{"name":"Tools","transport":{"type":"stdio","command":"tools-mcp","env":{"KEYBOARD_LAYOUT":"us","HOTKEY":"ctrl-k","TOKENIZER_CACHE":"local"}}}}}"#,
        );

        let report = reload_user_mcp_config(&db, &path).unwrap();

        assert_eq!(report.imported, 1);
        assert_eq!(file_connector(&db, "tools").name, "Tools");
    }
}
