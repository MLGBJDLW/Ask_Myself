//! Per-call tool approval system.
//!
//! Provides types and policy storage for the "per-call GUI confirmation"
//! flow. High-risk tools (shell exec, destructive source operations, writes
//! to user-specified disk paths, etc.) can be gated on an approval callback
//! that surfaces a dialog to the user and blocks execution until a decision
//! is returned.
//!
//! Policies have three scopes:
//!   * `AllowOnce`     — not persisted, applies only to the current call.
//!   * `AllowSession`  — stored in-memory by [`SessionApprovalStore`] and
//!     applies to the remainder of the process lifetime for the same
//!     tool/target permission key.
//!   * `Never`         — persisted to `tool_approval_policies` and denies
//!     the tool until the user clears the rule.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

/// Max characters of the arguments preview embedded in an [`ApprovalRequest`].
const ARGUMENTS_PREVIEW_LIMIT: usize = 2_000;

/// Risk classification surfaced in the approval dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
}

/// A pending approval prompt emitted to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    pub permission_key: String,
    pub target_kind: String,
    pub target_value: String,
    /// Pretty-printed JSON arguments, truncated to [`ARGUMENTS_PREVIEW_LIMIT`].
    pub arguments_preview: String,
    pub risk_level: ApprovalRisk,
    /// Human-readable one-line reason (falls back to tool name).
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_preview: Option<ApprovalCheckpointPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCheckpointPreview {
    pub planned: bool,
    pub target_paths: Vec<String>,
    pub note: String,
}

impl ApprovalRequest {
    pub fn new(
        id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: &serde_json::Value,
        risk_level: ApprovalRisk,
        reason: impl Into<String>,
    ) -> Self {
        let tool_name = tool_name.into();
        let permission = permission_key_for_tool(&tool_name, arguments);
        let preview =
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string());
        let preview = if preview.len() > ARGUMENTS_PREVIEW_LIMIT {
            let mut cut = ARGUMENTS_PREVIEW_LIMIT;
            while !preview.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}\n…[truncated]", &preview[..cut])
        } else {
            preview
        };
        Self {
            id: id.into(),
            tool_name: tool_name.clone(),
            permission_key: permission.permission_key(),
            target_kind: permission.target_kind,
            target_value: permission.target_value,
            arguments_preview: preview,
            risk_level,
            reason: reason.into(),
            checkpoint_preview: checkpoint_preview(&tool_name, arguments),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ToolPermissionKey {
    pub tool_name: String,
    pub target_kind: String,
    pub target_value: String,
}

impl ToolPermissionKey {
    pub fn new(
        tool_name: impl Into<String>,
        target_kind: impl Into<String>,
        target_value: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            target_kind: target_kind.into(),
            target_value: target_value.into(),
        }
    }

    pub fn permission_key(&self) -> String {
        format!(
            "{}|{}|{}",
            encode_permission_segment(&self.tool_name),
            encode_permission_segment(&self.target_kind),
            encode_permission_segment(&self.target_value)
        )
    }

    pub fn parse(permission_key: &str) -> Option<Self> {
        let mut parts = permission_key.splitn(3, '|');
        let tool_name = decode_permission_segment(parts.next()?)?;
        let target_kind = decode_permission_segment(parts.next()?)?;
        let target_value = decode_permission_segment(parts.next()?)?;
        Some(Self {
            tool_name,
            target_kind,
            target_value,
        })
    }

    pub fn from_request(req: &ApprovalRequest) -> Self {
        Self::new(
            req.tool_name.clone(),
            req.target_kind.clone(),
            req.target_value.clone(),
        )
    }

    pub fn from_invocation(invocation: &crate::tools::ToolInvocation) -> Self {
        let args = &invocation.arguments;
        if invocation.tool_name == "run_shell" {
            if let Some(command) = args
                .get("command")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|command| !command.is_empty())
            {
                let mut parts = command.split_whitespace();
                let target = match (parts.next(), parts.next()) {
                    (Some(program), Some(first_arg)) => format!("{program} {first_arg}"),
                    (Some(program), None) => program.to_string(),
                    _ => "<unknown>".to_string(),
                };
                return Self::new(&invocation.tool_name, "command", target);
            }
            let program = args
                .get("program")
                .and_then(|value| value.as_str())
                .unwrap_or("<unknown>");
            let first_arg = args
                .get("args")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
                .and_then(|value| value.as_str());
            let target = match first_arg {
                Some(first) if !first.trim().is_empty() => format!("{program} {}", first.trim()),
                _ => program.to_string(),
            };
            return Self::new(&invocation.tool_name, "command", target);
        }

        if invocation.tool_name == "project_tool" {
            let action = args
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or("<unknown>");
            if action == "run" {
                let name = args
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("<unknown>");
                let target = args
                    .get("manifestHash")
                    .or_else(|| args.get("manifest_hash"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|hash| !hash.is_empty())
                    .map(|hash| format!("{name}@{}", short_permission_hash(hash)))
                    .unwrap_or_else(|| name.to_string());
                return Self::new(&invocation.tool_name, "project_tool", target);
            }
            return Self::new(&invocation.tool_name, "project_tool_catalog", action);
        }

        if invocation.tool_name == "mcp_tool" || invocation.tool_name.starts_with("mcp__") {
            return Self::new(&invocation.tool_name, "mcp_tool", &invocation.tool_name);
        }

        if invocation.access_profile.can_access_network {
            if let Some(host) = args
                .get("url")
                .and_then(|value| value.as_str())
                .and_then(extract_url_host)
            {
                return Self::new(&invocation.tool_name, "network", host);
            }
        }

        if invocation.access_profile.can_write || invocation.access_profile.can_read {
            if let Some(file_key) = invocation
                .capabilities
                .resource_keys
                .iter()
                .find_map(|key| key.strip_prefix("file:"))
            {
                return Self::new(&invocation.tool_name, "file", file_key);
            }
            if let Some(source_key) = invocation
                .capabilities
                .resource_keys
                .iter()
                .find_map(|key| key.strip_prefix("source:"))
            {
                return Self::new(&invocation.tool_name, "source", source_key);
            }
        }

        Self::new(&invocation.tool_name, "tool", "*")
    }
}

pub fn permission_key_for_tool(tool_name: &str, args: &serde_json::Value) -> ToolPermissionKey {
    let invocation =
        crate::tools::default_tool_registry().build_invocation("approval", tool_name, args.clone());
    ToolPermissionKey::from_invocation(&invocation)
}

fn encode_permission_segment(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('|', "%7C")
        .replace(' ', "%20")
}

fn decode_permission_segment(value: &str) -> Option<String> {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let code = value.get(i + 1..i + 3)?;
            match code {
                "20" => out.push(' '),
                "25" => out.push('%'),
                "7C" | "7c" => out.push('|'),
                _ => return None,
            }
            i += 3;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Some(out)
}

fn short_permission_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn extract_url_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

/// User decision returned from the approval UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Allow just this one invocation.
    AllowOnce,
    /// Allow for the remainder of the current session.
    AllowSession,
    /// Deny this invocation.
    Deny,
    /// Deny this invocation and remember the rule across restarts.
    Never,
}

impl ApprovalDecision {
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowSession)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowSession => "allow_session",
            Self::Deny => "deny",
            Self::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow_once" => Some(Self::AllowOnce),
            "allow_session" => Some(Self::AllowSession),
            "deny" => Some(Self::Deny),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Async callback invoked to obtain a decision for an [`ApprovalRequest`].
///
/// Implementations are expected to consult persisted/session policies first
/// and only surface a UI prompt when no cached decision applies.
pub type ApprovalCallback = Arc<
    dyn Fn(ApprovalRequest) -> Pin<Box<dyn Future<Output = ApprovalDecision> + Send>> + Send + Sync,
>;

/// Cheap cloneable in-memory store for `AllowSession` grants.
#[derive(Default, Clone)]
pub struct SessionApprovalStore {
    inner: Arc<Mutex<HashMap<String, ApprovalDecision>>>,
}

impl SessionApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, tool_name: &str) -> Option<ApprovalDecision> {
        self.inner
            .lock()
            .ok()
            .and_then(|guard| guard.get(tool_name).copied())
    }

    pub fn set(&self, tool_name: &str, decision: ApprovalDecision) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(tool_name.to_string(), decision);
        }
    }

    pub fn remove(&self, tool_name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(tool_name);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }

    pub fn list(&self) -> Vec<(String, ApprovalDecision)> {
        self.inner
            .lock()
            .ok()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}

/// Global tool-approval mode mirroring the Settings toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalMode {
    /// Default — route every high-risk call through the approval callback.
    #[default]
    Ask,
    /// Skip the gate entirely (opt-out).
    AllowAll,
    /// Deny every high-risk call without prompting.
    DenyAll,
}

impl ToolApprovalMode {
    pub fn short_circuit(self) -> Option<ApprovalDecision> {
        match self {
            Self::Ask => None,
            Self::AllowAll => Some(ApprovalDecision::AllowOnce),
            Self::DenyAll => Some(ApprovalDecision::Deny),
        }
    }
}

// ---------------------------------------------------------------------------
// Persistent "never" policies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalPolicy {
    pub tool_name: String,
    pub decision: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_value: Option<String>,
}

impl Database {
    pub fn get_tool_approval_policy(&self, tool_name: &str) -> Result<Option<String>, CoreError> {
        let conn = self.conn();
        let row = conn.query_row(
            "SELECT decision FROM tool_approval_policies WHERE tool_name = ?1",
            rusqlite::params![tool_name],
            |r| r.get::<_, String>(0),
        );
        match row {
            Ok(d) => Ok(Some(d)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_tool_approval_policy(
        &self,
        tool_name: &str,
        decision: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO tool_approval_policies (tool_name, decision) VALUES (?1, ?2)
             ON CONFLICT(tool_name) DO UPDATE SET decision = excluded.decision,
                 created_at = datetime('now')",
            rusqlite::params![tool_name, decision],
        )?;
        Ok(())
    }

    pub fn get_tool_permission_policy(
        &self,
        permission_key: &str,
    ) -> Result<Option<String>, CoreError> {
        let conn = self.conn();
        let row = conn.query_row(
            "SELECT decision FROM tool_permission_policies WHERE permission_key = ?1",
            rusqlite::params![permission_key],
            |r| r.get::<_, String>(0),
        );
        match row {
            Ok(d) => Ok(Some(d)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_tool_permission_policy(
        &self,
        key: &ToolPermissionKey,
        decision: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO tool_permission_policies
                (permission_key, tool_name, target_kind, target_value, decision)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(permission_key) DO UPDATE SET
                tool_name = excluded.tool_name,
                target_kind = excluded.target_kind,
                target_value = excluded.target_value,
                decision = excluded.decision,
                created_at = datetime('now')",
            rusqlite::params![
                key.permission_key(),
                key.tool_name,
                key.target_kind,
                key.target_value,
                decision
            ],
        )?;
        Ok(())
    }

    pub fn delete_tool_permission_policy(&self, permission_key: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM tool_permission_policies WHERE permission_key = ?1",
            rusqlite::params![permission_key],
        )?;
        Ok(())
    }

    pub fn delete_tool_approval_policy(&self, tool_name: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM tool_approval_policies WHERE tool_name = ?1",
            rusqlite::params![tool_name],
        )?;
        Ok(())
    }

    pub fn list_tool_approval_policies(&self) -> Result<Vec<ToolApprovalPolicy>, CoreError> {
        let conn = self.conn();
        let mut targeted_stmt = conn.prepare(
            "SELECT permission_key, tool_name, target_kind, target_value, decision, created_at
             FROM tool_permission_policies
             ORDER BY created_at DESC",
        )?;
        let mut policies = targeted_stmt
            .query_map([], |r| {
                Ok(ToolApprovalPolicy {
                    permission_key: Some(r.get(0)?),
                    tool_name: r.get(1)?,
                    target_kind: Some(r.get(2)?),
                    target_value: Some(r.get(3)?),
                    decision: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut stmt = conn.prepare(
            "SELECT tool_name, decision, created_at FROM tool_approval_policies
             ORDER BY created_at DESC",
        )?;
        let legacy = stmt
            .query_map([], |r| {
                Ok(ToolApprovalPolicy {
                    tool_name: r.get(0)?,
                    decision: r.get(1)?,
                    created_at: r.get(2)?,
                    permission_key: None,
                    target_kind: None,
                    target_value: None,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        policies.extend(legacy);
        Ok(policies)
    }

    pub fn clear_tool_approval_policies(&self) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute("DELETE FROM tool_permission_policies", [])?;
        conn.execute("DELETE FROM tool_approval_policies", [])?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Risk classifier
// ---------------------------------------------------------------------------

/// Classify risk for a high-risk tool. Used purely for UX labeling.
pub fn classify_risk(tool_name: &str, args: &serde_json::Value) -> ApprovalRisk {
    crate::tools::default_tool_registry()
        .access_profile(tool_name, args)
        .risk_level
}

/// Build a short human-readable description of what the tool is about to do.
pub fn describe_request(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run_shell" => {
            let cmd = args
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format_run_shell_argv(args));
            format!("Agent wants to run shell command: {cmd}")
        }
        "manage_source" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let id = args
                .get("source_id")
                .and_then(|v| v.as_str())
                .unwrap_or("<none>");
            format!("Agent wants to {action} source `{id}`")
        }
        "archive_output" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("untitled");
            let dir = args
                .get("source_directory")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            format!("Agent wants to archive `{title}` to `{dir}`")
        }
        "create_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            if args
                .get("overwrite")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                format!(
                    "Agent wants to overwrite `{path}`. A restorable file checkpoint will be saved first."
                )
            } else {
                format!(
                    "Agent wants to create `{path}`. A checkpoint will record that this file did not exist before the write."
                )
            }
        }
        "edit_file" | "multi_edit" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            format!(
                "Agent wants to edit `{path}`. A restorable file checkpoint will be saved first."
            )
        }
        "write_note" => {
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            format!(
                "Agent wants to write note `{filename}`. A restorable file checkpoint will be saved first."
            )
        }
        "project_tool" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            if action == "run" {
                let hash = args
                    .get("manifestHash")
                    .or_else(|| args.get("manifest_hash"))
                    .and_then(|v| v.as_str())
                    .map(short_permission_hash)
                    .unwrap_or("<missing>");
                format!("Agent wants to run project-local tool `{name}` from manifest {hash}")
            } else {
                format!("Agent wants to {action} project-local tool manifests")
            }
        }
        other => format!("Agent wants to invoke `{other}`"),
    }
}

fn format_run_shell_argv(args: &serde_json::Value) -> String {
    let Some(program) = args.get("program").and_then(|v| v.as_str()) else {
        return "<unknown>".to_string();
    };
    let mut parts = vec![program.to_string()];
    if let Some(argv) = args.get("args").and_then(|v| v.as_array()) {
        parts.extend(argv.iter().filter_map(|value| {
            value.as_str().map(|arg| {
                if arg.contains(char::is_whitespace) {
                    format!("{arg:?}")
                } else {
                    arg.to_string()
                }
            })
        }));
    }
    parts.join(" ")
}

fn checkpoint_preview(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<ApprovalCheckpointPreview> {
    let path_arg = match tool_name {
        "create_file" | "edit_file" | "multi_edit" => args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        "write_note" => args
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|filename| format!("notes/{filename}")),
        _ => None,
    }?;

    Some(ApprovalCheckpointPreview {
        planned: true,
        target_paths: vec![path_arg],
        note: "A file checkpoint will be created after approval and before the write.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_roundtrip() {
        for d in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::AllowSession,
            ApprovalDecision::Deny,
            ApprovalDecision::Never,
        ] {
            assert_eq!(ApprovalDecision::parse(d.as_str()), Some(d));
        }
        assert!(ApprovalDecision::parse("bogus").is_none());
    }

    #[test]
    fn decision_allowed() {
        assert!(ApprovalDecision::AllowOnce.is_allowed());
        assert!(ApprovalDecision::AllowSession.is_allowed());
        assert!(!ApprovalDecision::Deny.is_allowed());
        assert!(!ApprovalDecision::Never.is_allowed());
    }

    #[test]
    fn mode_short_circuit() {
        assert_eq!(ToolApprovalMode::Ask.short_circuit(), None);
        assert_eq!(
            ToolApprovalMode::AllowAll.short_circuit(),
            Some(ApprovalDecision::AllowOnce)
        );
        assert_eq!(
            ToolApprovalMode::DenyAll.short_circuit(),
            Some(ApprovalDecision::Deny)
        );
    }

    #[test]
    fn session_store_lifecycle() {
        let store = SessionApprovalStore::new();
        assert_eq!(store.get("run_shell"), None);
        store.set("run_shell", ApprovalDecision::AllowSession);
        assert_eq!(store.get("run_shell"), Some(ApprovalDecision::AllowSession));
        assert_eq!(store.list().len(), 1);
        store.remove("run_shell");
        assert_eq!(store.get("run_shell"), None);
        store.set("archive_output", ApprovalDecision::AllowSession);
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn risk_classification() {
        assert_eq!(
            classify_risk("run_shell", &serde_json::json!({})),
            ApprovalRisk::High
        );
        assert_eq!(
            classify_risk("edit_file", &serde_json::json!({ "path": "notes.md" })),
            ApprovalRisk::High
        );
        assert_eq!(
            classify_risk("create_file", &serde_json::json!({ "path": "notes.md" })),
            ApprovalRisk::Medium
        );
        assert_eq!(
            classify_risk(
                "create_file",
                &serde_json::json!({ "path": "notes.md", "overwrite": true })
            ),
            ApprovalRisk::High
        );
        assert_eq!(
            classify_risk("manage_source", &serde_json::json!({ "action": "remove" })),
            ApprovalRisk::High
        );
        assert_eq!(
            classify_risk("manage_source", &serde_json::json!({ "action": "add" })),
            ApprovalRisk::Medium
        );
    }

    #[test]
    fn request_truncates_preview() {
        let huge: String = "x".repeat(ARGUMENTS_PREVIEW_LIMIT + 500);
        let args = serde_json::json!({ "blob": huge });
        let req = ApprovalRequest::new("req-1", "run_shell", &args, ApprovalRisk::High, "test");
        assert!(req.arguments_preview.len() <= ARGUMENTS_PREVIEW_LIMIT + 20);
        assert!(req.arguments_preview.contains("truncated"));
    }

    #[test]
    fn approval_request_carries_targeted_permission_key() {
        let args = serde_json::json!({
            "program": "git",
            "args": ["status", "--short"],
            "cwd": "."
        });

        let req = ApprovalRequest::new("req-1", "run_shell", &args, ApprovalRisk::High, "test");

        assert_eq!(req.permission_key, "run_shell|command|git%20status");
        assert_eq!(req.target_kind, "command");
        assert_eq!(req.target_value, "git status");
    }

    #[test]
    fn approval_request_uses_command_string_permission_key() {
        let args = serde_json::json!({
            "command": "git status --short",
            "cwd": "."
        });

        let req = ApprovalRequest::new("req-1", "run_shell", &args, ApprovalRisk::High, "test");

        assert_eq!(req.permission_key, "run_shell|command|git%20status");
        assert_eq!(req.target_kind, "command");
        assert_eq!(req.target_value, "git status");
    }

    #[test]
    fn describe_run_shell_uses_argv_shape() {
        let args = serde_json::json!({
            "program": "git",
            "args": ["status", "--short"],
            "cwd": "."
        });

        let description = describe_request("run_shell", &args);

        assert!(description.contains("git status --short"));
    }

    #[test]
    fn describe_run_shell_uses_command_string() {
        let args = serde_json::json!({
            "command": "git status --short",
            "cwd": "."
        });

        let description = describe_request("run_shell", &args);

        assert!(description.contains("git status --short"));
    }

    #[test]
    fn permission_key_distinguishes_file_targets() {
        let edit_a = permission_key_for_tool(
            "edit_file",
            &serde_json::json!({ "path": "notes/a.md", "old_str": "a", "new_str": "b" }),
        );
        let edit_b = permission_key_for_tool(
            "edit_file",
            &serde_json::json!({ "path": "notes/b.md", "old_str": "a", "new_str": "b" }),
        );

        assert_ne!(edit_a.permission_key(), edit_b.permission_key());
        assert_eq!(edit_a.target_kind, "file");
        assert_eq!(edit_a.target_value, "notes/a.md");
    }

    #[test]
    fn permission_key_distinguishes_project_tool_manifests() {
        let lint = permission_key_for_tool(
            "project_tool",
            &serde_json::json!({
                "action": "run",
                "name": "lint",
                "manifestHash": "abcdef0123456789"
            }),
        );
        let test = permission_key_for_tool(
            "project_tool",
            &serde_json::json!({
                "action": "run",
                "name": "test",
                "manifestHash": "abcdef0123456789"
            }),
        );

        assert_ne!(lint.permission_key(), test.permission_key());
        assert_eq!(
            lint.permission_key(),
            "project_tool|project_tool|lint@abcdef012345"
        );
        assert_eq!(lint.target_kind, "project_tool");
        assert_eq!(lint.target_value, "lint@abcdef012345");
    }

    #[test]
    fn project_tool_request_carries_manifest_target() {
        let args = serde_json::json!({
            "action": "run",
            "name": "lint",
            "manifestHash": "abcdef0123456789",
            "arguments": { "path": "src/lib.rs" }
        });

        let req = ApprovalRequest::new("req-1", "project_tool", &args, ApprovalRisk::High, "test");

        assert_eq!(
            req.permission_key,
            "project_tool|project_tool|lint@abcdef012345"
        );
        assert_eq!(req.target_kind, "project_tool");
        assert_eq!(req.target_value, "lint@abcdef012345");
    }

    #[test]
    fn file_mutation_request_includes_checkpoint_preview() {
        let args = serde_json::json!({
            "path": "notes/today.md",
            "action": "str_replace",
            "old_str": "a",
            "new_str": "b"
        });
        let req = ApprovalRequest::new(
            "req-1",
            "edit_file",
            &args,
            ApprovalRisk::High,
            describe_request("edit_file", &args),
        );

        let preview = req.checkpoint_preview.expect("checkpoint preview");
        assert!(preview.planned);
        assert_eq!(preview.target_paths, vec!["notes/today.md".to_string()]);
    }

    #[test]
    fn db_policy_roundtrip() {
        let db = crate::db::Database::open_memory().expect("in-memory db");
        assert!(db.get_tool_approval_policy("run_shell").unwrap().is_none());
        db.save_tool_approval_policy("run_shell", "never").unwrap();
        assert_eq!(
            db.get_tool_approval_policy("run_shell").unwrap().as_deref(),
            Some("never")
        );
        let list = db.list_tool_approval_policies().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].tool_name, "run_shell");
        db.delete_tool_approval_policy("run_shell").unwrap();
        assert!(db.get_tool_approval_policy("run_shell").unwrap().is_none());
    }

    #[test]
    fn db_permission_policy_roundtrip_is_targeted() {
        let db = crate::db::Database::open_memory().expect("in-memory db");
        let key = ToolPermissionKey::new("run_shell", "command", "git status");

        assert!(db
            .get_tool_permission_policy(&key.permission_key())
            .unwrap()
            .is_none());
        db.save_tool_permission_policy(&key, "never").unwrap();

        assert_eq!(
            db.get_tool_permission_policy(&key.permission_key())
                .unwrap()
                .as_deref(),
            Some("never")
        );
        let list = db.list_tool_approval_policies().unwrap();
        assert!(list.iter().any(|policy| {
            policy.permission_key.as_deref() == Some("run_shell|command|git%20status")
                && policy.target_value.as_deref() == Some("git status")
        }));
        db.delete_tool_permission_policy(&key.permission_key())
            .unwrap();
        assert!(db
            .get_tool_permission_policy(&key.permission_key())
            .unwrap()
            .is_none());
    }

    #[test]
    fn db_permission_policies_allow_multiple_targets_for_same_tool() {
        let db = crate::db::Database::open_memory().expect("in-memory db");
        let lint = ToolPermissionKey::new("project_tool", "project_tool", "lint@111111111111");
        let test = ToolPermissionKey::new("project_tool", "project_tool", "test@222222222222");

        db.save_tool_permission_policy(&lint, "allow_session")
            .unwrap();
        db.save_tool_permission_policy(&test, "never").unwrap();

        assert_eq!(
            db.get_tool_permission_policy(&lint.permission_key())
                .unwrap()
                .as_deref(),
            Some("allow_session")
        );
        assert_eq!(
            db.get_tool_permission_policy(&test.permission_key())
                .unwrap()
                .as_deref(),
            Some("never")
        );
        let list = db.list_tool_approval_policies().unwrap();
        assert_eq!(
            list.iter()
                .filter(|policy| policy.tool_name == "project_tool")
                .count(),
            2
        );
    }
}
