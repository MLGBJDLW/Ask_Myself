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
//!     applies to the remainder of the process lifetime.
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
            arguments_preview: preview,
            risk_level,
            reason: reason.into(),
            checkpoint_preview: checkpoint_preview(&tool_name, arguments),
        }
    }
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
        let mut stmt = conn.prepare(
            "SELECT tool_name, decision, created_at FROM tool_approval_policies
             ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ToolApprovalPolicy {
                    tool_name: r.get(0)?,
                    decision: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn clear_tool_approval_policies(&self) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute("DELETE FROM tool_approval_policies", [])?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Risk classifier
// ---------------------------------------------------------------------------

/// Classify risk for a high-risk tool. Used purely for UX labeling.
pub fn classify_risk(tool_name: &str, args: &serde_json::Value) -> ApprovalRisk {
    match tool_name {
        "run_shell" => ApprovalRisk::High,
        "edit_file" => ApprovalRisk::High,
        "create_file" => {
            if args
                .get("overwrite")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            }
        }
        "write_note" => {
            let mode = args
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("create");
            if mode == "create" {
                ApprovalRisk::Medium
            } else {
                ApprovalRisk::High
            }
        }
        "manage_source" => {
            if args.get("action").and_then(|v| v.as_str()) == Some("remove") {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            }
        }
        "archive_output" => ApprovalRisk::Medium,
        _ => ApprovalRisk::Medium,
    }
}

/// Build a short human-readable description of what the tool is about to do.
pub fn describe_request(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run_shell" => {
            let cmd = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
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
        "edit_file" => {
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
        other => format!("Agent wants to invoke `{other}`"),
    }
}

fn checkpoint_preview(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<ApprovalCheckpointPreview> {
    let path_arg = match tool_name {
        "create_file" | "edit_file" => args
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
}
