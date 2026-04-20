//! ListSourcesTool — lists all registered knowledge-base sources.

use std::sync::OnceLock;

use async_trait::async_trait;

use crate::db::Database;
use crate::error::CoreError;

use super::{scope_is_active, Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/list_sources.json");

/// Tool that lists all registered sources with document counts and last scan times.
pub struct ListSourcesTool;

#[async_trait]
impl Tool for ListSourcesTool {
    fn name(&self) -> &str {
        "list_sources"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        _arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn();
            let mut stmt = conn.prepare(
                "SELECT s.id, s.kind, s.root_path, s.created_at, s.updated_at,
                        COUNT(d.id) AS doc_count,
                        MAX(d.indexed_at) AS last_scan
                 FROM sources s
                 LEFT JOIN documents d ON d.source_id = s.id
                 GROUP BY s.id
                 ORDER BY s.created_at",
            )?;

            let mut rows: Vec<(String, String, String, String, String, i64, Option<String>)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            if scope_is_active(&source_scope) {
                rows.retain(|(id, _, _, _, _, _, _)| source_scope.iter().any(|sid| sid == id));
            }

            if rows.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: if scope_is_active(&source_scope) {
                        "No sources available in the current source scope.".to_string()
                    } else {
                        "No sources registered.".to_string()
                    },
                    is_error: false,
                    artifacts: None,
                });
            }

            let mut text = format!("Found {} source(s):\n\n", rows.len());
            let mut artifacts = Vec::new();

            for (id, kind, root_path, created_at, _updated_at, doc_count, last_scan) in &rows {
                let scan_display = last_scan.as_deref().unwrap_or("never");
                text.push_str(&format!(
                    "- ID: {id}\n  Path: {root_path}\n  Kind: {kind}\n  Documents: {doc_count}\n  Last scan: {scan_display}\n  Created: {created_at}\n\n",
                ));
                artifacts.push(serde_json::json!({
                    "id": id,
                    "kind": kind,
                    "root_path": root_path,
                    "doc_count": doc_count,
                    "last_scan": last_scan,
                    "created_at": created_at,
                }));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(serde_json::Value::Array(artifacts)),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
