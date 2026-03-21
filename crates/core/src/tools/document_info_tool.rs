//! GetDocumentInfoTool — retrieves metadata about a specific document.

use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{ensure_source_in_scope, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/get_document_info.json");

/// Tool that retrieves detailed metadata about a single document.
pub struct GetDocumentInfoTool;

#[derive(Deserialize)]
struct GetDocumentInfoArgs {
    path: Option<String>,
    document_id: Option<String>,
}

#[async_trait]
impl Tool for GetDocumentInfoTool {
    fn name(&self) -> &str {
        "get_document_info"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::DocumentAnalysis]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GetDocumentInfoArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid get_document_info arguments: {e}"))
        })?;

        if args.path.is_none() && args.document_id.is_none() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "At least one of 'path' or 'document_id' must be provided.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn();

            // Query document by id or path.
            let row: Result<
                (
                    String,
                    String,
                    String,
                    Option<String>,
                    String,
                    i64,
                    String,
                    String,
                    String,
                    String,
                ),
                rusqlite::Error,
            > = if let Some(ref id) = args.document_id {
                conn.query_row(
                    "SELECT d.id, d.source_id, d.path, d.title, d.mime_type, d.file_size,
                            d.modified_at, d.content_hash, d.indexed_at, d.metadata
                     FROM documents d
                     WHERE d.id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                        ))
                    },
                )
            } else {
                let path = args.path.as_ref().unwrap();
                conn.query_row(
                    "SELECT d.id, d.source_id, d.path, d.title, d.mime_type, d.file_size,
                            d.modified_at, d.content_hash, d.indexed_at, d.metadata
                     FROM documents d
                     WHERE d.path = ?1",
                    params![path],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                        ))
                    },
                )
            };

            let (
                id,
                source_id,
                path,
                title,
                mime_type,
                file_size,
                modified_at,
                content_hash,
                indexed_at,
                metadata,
            ) = match row {
                Ok(r) => r,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    let lookup = if let Some(ref did) = args.document_id {
                        format!("id '{did}'")
                    } else {
                        format!("path '{}'", args.path.as_ref().unwrap())
                    };
                    return Ok(ToolResult {
                        call_id,
                        content: format!("Document not found with {lookup}."),
                        is_error: true,
                        artifacts: None,
                    });
                }
                Err(e) => return Err(CoreError::Database(e)),
            };

            if let Err(message) = ensure_source_in_scope(&source_id, &source_scope) {
                return Ok(ToolResult {
                    call_id,
                    content: message,
                    is_error: true,
                    artifacts: None,
                });
            }

            // Count chunks for this document.
            let chunk_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                params![&id],
                |row| row.get(0),
            )?;

            // Look up root_path from source for context.
            let source_root: Option<String> = conn
                .query_row(
                    "SELECT root_path FROM sources WHERE id = ?1",
                    params![&source_id],
                    |row| row.get(0),
                )
                .ok();

            let title_display = title.as_deref().unwrap_or("(untitled)");
            let text = format!(
                "Document: {path}\n\
                 Title: {title_display}\n\
                 ID: {id}\n\
                 Source ID: {source_id}\n\
                 Suggested citation: [doc:{id}|{title_display}]\n\
                 Source root: {}\n\
                 MIME type: {mime_type}\n\
                 File size: {file_size} bytes\n\
                 Modified: {modified_at}\n\
                 Indexed: {indexed_at}\n\
                 Content hash: {content_hash}\n\
                 Chunks: {chunk_count}\n\
                 Metadata: {metadata}",
                source_root.as_deref().unwrap_or("(unknown)"),
            );

            let artifact = serde_json::json!({
                "id": id,
                "source_id": source_id,
                "source_root": source_root,
                "path": path,
                "title": title,
                "mime_type": mime_type,
                "file_size": file_size,
                "modified_at": modified_at,
                "indexed_at": indexed_at,
                "content_hash": content_hash,
                "chunk_count": chunk_count,
                "metadata": metadata,
                "suggestedCitation": format!("[doc:{id}|{title_display}]"),
            });

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(artifact),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        let db = Database::open_memory().expect("open_memory");
        let conn = db.conn();
        let source_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', '/tmp/test')",
            params![&source_id],
        )
        .unwrap();
        let doc_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/test/hello.md', 'Hello', 'text/markdown', 512, '2025-01-01 00:00:00', 'abc123')",
            params![&doc_id, &source_id],
        )
        .unwrap();
        // Insert two chunks.
        for i in 0..2 {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', 'chunk content', 0, 13, 1, 5, 'chash')",
                params![&cid, &doc_id, i],
            )
            .unwrap();
        }
        drop(conn);
        db
    }

    #[tokio::test]
    async fn test_get_document_info_by_path() {
        let db = setup_db();
        let tool = GetDocumentInfoTool;
        let result = tool
            .execute("call-1", r#"{"path": "/tmp/test/hello.md"}"#, &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("/tmp/test/hello.md"));
        assert!(result.content.contains("Chunks: 2"));
        assert!(result.content.contains("text/markdown"));
    }

    #[tokio::test]
    async fn test_get_document_info_by_id() {
        let db = setup_db();
        // Retrieve the doc id from the DB.
        let doc_id: String = db
            .conn()
            .query_row("SELECT id FROM documents LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let tool = GetDocumentInfoTool;
        let args = format!(r#"{{"document_id": "{doc_id}"}}"#);
        let result = tool.execute("call-2", &args, &db, &[]).await.unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Hello"));
        assert!(result.content.contains("512 bytes"));
    }

    #[tokio::test]
    async fn test_get_document_info_not_found() {
        let db = setup_db();
        let tool = GetDocumentInfoTool;
        let result = tool
            .execute("call-3", r#"{"path": "/nonexistent/file.md"}"#, &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_get_document_info_no_args() {
        let db = setup_db();
        let tool = GetDocumentInfoTool;
        let result = tool.execute("call-4", r#"{}"#, &db, &[]).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("At least one"));
    }
}
