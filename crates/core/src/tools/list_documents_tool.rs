//! ListDocumentsTool — lists documents belonging to a specific source.

use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/list_documents.json");

/// Tool that lists documents in a given source.
pub struct ListDocumentsTool;

#[derive(Deserialize)]
struct ListDocumentsArgs {
    source_id: String,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    offset: u32,
}

fn default_limit() -> u32 {
    50
}

#[async_trait]
impl Tool for ListDocumentsTool {
    fn name(&self) -> &str {
        "list_documents"
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
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ListDocumentsArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid list_documents arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            let limit = args.limit.min(200).max(1);
            let offset = args.offset;

            // Verify the source exists.
            let _ = db.get_source(&args.source_id)?;

            let conn = db.conn();
            let mut stmt = conn.prepare(
                "SELECT id, path, title, mime_type, file_size, modified_at, indexed_at
                 FROM documents
                 WHERE source_id = ?1
                 ORDER BY path
                 LIMIT ?2 OFFSET ?3",
            )?;

            let rows: Vec<(String, String, Option<String>, String, i64, String, String)> = stmt
                .query_map(params![&args.source_id, limit, offset], |row| {
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

            // Get total count for pagination info.
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM documents WHERE source_id = ?1",
                params![&args.source_id],
                |row| row.get(0),
            )?;

            if rows.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!(
                        "No documents found in source '{}'.",
                        args.source_id
                    ),
                    is_error: false,
                    artifacts: None,
                });
            }

            let mut text = format!(
                "Showing {}-{} of {} document(s) in source '{}':\n\n",
                offset + 1,
                (offset + rows.len() as u32).min(total as u32),
                total,
                args.source_id,
            );
            let mut artifacts = Vec::new();

            for (id, path, title, mime_type, file_size, modified_at, _indexed_at) in &rows {
                let title_display = title.as_deref().unwrap_or("(untitled)");
                text.push_str(&format!(
                    "- {path}\n  Title: {title_display}\n  Type: {mime_type} | Size: {file_size} bytes\n  Modified: {modified_at}\n\n",
                ));
                artifacts.push(serde_json::json!({
                    "id": id,
                    "path": path,
                    "title": title,
                    "mime_type": mime_type,
                    "file_size": file_size,
                    "modified_at": modified_at,
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
