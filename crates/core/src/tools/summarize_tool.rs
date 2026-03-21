//! RetrieveEvidenceTool — retrieves and formats evidence cards by chunk IDs.
//! SummarizeDocumentTool — gathers all chunks of a document for summarization.

use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::{current_scope_miss_message, ensure_source_in_scope, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/retrieve_evidence.json");

static SUMMARIZE_DEF: OnceLock<ToolDef> = OnceLock::new();
const SUMMARIZE_DEF_JSON: &str = include_str!("../../prompts/tools/summarize_document.json");

/// Tool that looks up specific chunks by their IDs and returns formatted
/// content suitable for citation in LLM responses.
pub struct RetrieveEvidenceTool;

#[derive(Deserialize)]
struct RetrieveEvidenceArgs {
    chunk_ids: Vec<String>,
}

#[async_trait]
impl Tool for RetrieveEvidenceTool {
    fn name(&self) -> &str {
        "retrieve_evidence"
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
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: RetrieveEvidenceArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid retrieve_evidence arguments: {e}"))
        })?;

        if args.chunk_ids.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "No chunk IDs provided.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn();

            let mut text = String::new();
            let mut found = 0usize;
            let mut artifacts: Vec<serde_json::Value> = Vec::new();

            for chunk_id in &args.chunk_ids {
                let row = conn.query_row(
                    "SELECT c.id, c.content, d.path, d.title, d.source_id
                     FROM chunks c
                     JOIN documents d ON d.id = c.document_id
                     WHERE c.id = ?1",
                    params![chunk_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                );

                match row {
                    Ok((id, content, path, title, source_id)) => {
                        if ensure_source_in_scope(&source_id, &source_scope).is_err() {
                            text.push_str(&format!(
                                "--- Chunk {} ---\n{}\n\n",
                                chunk_id,
                                current_scope_miss_message()
                            ));
                            continue;
                        }
                        found += 1;
                        text.push_str(&format!(
                            "--- Chunk ---\n\
                             [chunk_id: {}]\n\
                             Path: {}\n\
                             Title: {}\n\
                             Content:\n{}\n\n",
                            id, path, title, content
                        ));
                        artifacts.push(json!({
                            "chunkId": id,
                            "path": path,
                            "title": title,
                            "sourceId": source_id,
                            "content": content,
                        }));
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {
                        text.push_str(&format!("--- Chunk {} ---\nNot found.\n\n", chunk_id));
                    }
                    Err(e) => return Err(CoreError::Database(e)),
                }
            }

            let header = format!(
                "Retrieved {found} of {} requested chunk(s).\n\n",
                args.chunk_ids.len()
            );

            Ok(ToolResult {
                call_id,
                content: format!("{header}{text}"),
                is_error: false,
                artifacts: Some(serde_json::to_value(&artifacts).unwrap_or_default()),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// SummarizeDocumentTool
// ---------------------------------------------------------------------------

/// Tool that retrieves all chunks of a document in order, returning their
/// content for the LLM agent to summarize. Does NOT call any LLM itself.
pub struct SummarizeDocumentTool;

#[derive(Deserialize)]
struct SummarizeDocumentArgs {
    path: Option<String>,
    document_id: Option<String>,
    #[serde(default = "default_max_chunks")]
    max_chunks: usize,
}

fn default_max_chunks() -> usize {
    100
}

#[async_trait]
impl Tool for SummarizeDocumentTool {
    fn name(&self) -> &str {
        "summarize_document"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&SUMMARIZE_DEF, SUMMARIZE_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&SUMMARIZE_DEF, SUMMARIZE_DEF_JSON)
            .parameters
            .clone()
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
        let args: SummarizeDocumentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid summarize_document arguments: {e}"))
        })?;

        if args.path.is_none() && args.document_id.is_none() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "At least one of 'path' or 'document_id' must be provided.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let max_chunks = args.max_chunks.min(500).max(1);
        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();

        tokio::task::spawn_blocking(move || {
            let conn = db.conn();

            // 1. Resolve the document.
            let doc_row: Result<(String, String, Option<String>, String), rusqlite::Error> =
                if let Some(ref id) = args.document_id {
                    conn.query_row(
                        "SELECT d.id, d.path, d.title, d.source_id FROM documents d WHERE d.id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                } else {
                    let path = args.path.as_ref().unwrap();
                    conn.query_row(
                        "SELECT d.id, d.path, d.title, d.source_id FROM documents d WHERE d.path = ?1",
                        params![path],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                };

            let (doc_id, doc_path, doc_title, source_id) = match doc_row {
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

            // 2. Count total chunks.
            let total_chunks: usize = conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                params![&doc_id],
                |row| row.get(0),
            )?;

            // 3. Fetch chunks ordered by chunk_index, up to max_chunks.
            let mut stmt = conn.prepare(
                "SELECT c.id, c.chunk_index, c.content
                 FROM chunks c
                 WHERE c.document_id = ?1
                 ORDER BY c.chunk_index
                 LIMIT ?2",
            )?;

            let chunks: Vec<(String, i64, String)> = stmt
                .query_map(params![&doc_id, max_chunks as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let shown = chunks.len();
            let title_display = doc_title.as_deref().unwrap_or("(untitled)");

            // 4. Build formatted output.
            let mut text = format!(
                "Document: {doc_path}\n\
                 Title: {title_display}\n\
                 Document ID: {doc_id}\n\
                 Source ID: {source_id}\n\
                 Suggested citation: [doc:{doc_id}|{title_display}]\n\
                 Total chunks: {total_chunks} (showing {shown})\n\n"
            );

            for (i, (chunk_id, chunk_index, content)) in chunks.iter().enumerate() {
                text.push_str(&format!(
                    "--- Chunk {}/{total_chunks} (index {chunk_index}, id: {chunk_id}) ---\n\
                     {content}\n\n",
                    i + 1,
                ));
            }

            if shown < total_chunks {
                text.push_str(&format!(
                    "... {} more chunk(s) not shown. Increase max_chunks to retrieve more.\n",
                    total_chunks - shown,
                ));
            }

            let artifacts: Vec<serde_json::Value> = chunks
                .iter()
                .map(|(id, idx, content)| {
                    json!({
                        "chunkId": id,
                        "chunkIndex": idx,
                        "content": content,
                    })
                })
                .collect();

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(json!({
                    "documentId": doc_id,
                    "documentPath": doc_path,
                    "documentTitle": doc_title,
                    "sourceId": source_id,
                    "suggestedCitation": format!("[doc:{doc_id}|{title_display}]"),
                    "chunks": artifacts,
                })),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db_with_chunks(chunk_count: usize) -> (Database, String, String) {
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
             VALUES (?1, ?2, '/tmp/test/notes.md', 'My Notes', 'text/markdown', 1024, '2025-01-01 00:00:00', 'hash1')",
            params![&doc_id, &source_id],
        )
        .unwrap();
        for i in 0..chunk_count {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', ?4, 0, 100, 1, 10, 'chash')",
                params![&cid, &doc_id, i as i64, format!("Content of chunk {i}")],
            )
            .unwrap();
        }
        let doc_path = "/tmp/test/notes.md".to_string();
        drop(conn);
        (db, doc_id, doc_path)
    }

    #[tokio::test]
    async fn test_summarize_document_by_path() {
        let (db, _doc_id, _doc_path) = setup_db_with_chunks(3);
        let tool = SummarizeDocumentTool;
        let result = tool
            .execute("call-1", r#"{"path": "/tmp/test/notes.md"}"#, &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Document: /tmp/test/notes.md"));
        assert!(result.content.contains("Title: My Notes"));
        assert!(result.content.contains("Suggested citation: [doc:"));
        assert!(result.content.contains("Total chunks: 3 (showing 3)"));
        assert!(result.content.contains("Content of chunk 0"));
        assert!(result.content.contains("Content of chunk 1"));
        assert!(result.content.contains("Content of chunk 2"));
        assert!(result.content.contains("Chunk 1/3"));
        assert!(result.content.contains("Chunk 3/3"));
    }

    #[tokio::test]
    async fn test_summarize_document_by_id() {
        let (db, doc_id, _doc_path) = setup_db_with_chunks(2);
        let tool = SummarizeDocumentTool;
        let args = format!(r#"{{"document_id": "{doc_id}"}}"#);
        let result = tool.execute("call-2", &args, &db, &[]).await.unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Total chunks: 2 (showing 2)"));
        assert!(result.content.contains("Content of chunk 0"));
        assert!(result.content.contains("Content of chunk 1"));
        // Verify artifacts
        let artifacts = result.artifacts.unwrap();
        let chunks = artifacts["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["chunkIndex"], 0);
        assert_eq!(chunks[1]["chunkIndex"], 1);
        assert_eq!(artifacts["documentId"], doc_id);
    }

    #[tokio::test]
    async fn test_summarize_document_max_chunks_truncation() {
        let (db, _doc_id, _doc_path) = setup_db_with_chunks(5);
        let tool = SummarizeDocumentTool;
        let result = tool
            .execute(
                "call-3",
                r#"{"path": "/tmp/test/notes.md", "max_chunks": 2}"#,
                &db,
                &[],
            )
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Total chunks: 5 (showing 2)"));
        assert!(result.content.contains("Content of chunk 0"));
        assert!(result.content.contains("Content of chunk 1"));
        assert!(!result.content.contains("Content of chunk 2"));
        assert!(result.content.contains("3 more chunk(s) not shown"));
    }

    #[tokio::test]
    async fn test_summarize_document_not_found() {
        let (db, _, _) = setup_db_with_chunks(1);
        let tool = SummarizeDocumentTool;
        let result = tool
            .execute("call-4", r#"{"path": "/nonexistent.md"}"#, &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_summarize_document_no_args() {
        let (db, _, _) = setup_db_with_chunks(1);
        let tool = SummarizeDocumentTool;
        let result = tool.execute("call-5", r#"{}"#, &db, &[]).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("At least one"));
    }

    #[tokio::test]
    async fn test_summarize_document_rejects_out_of_scope_source() {
        let (db, _doc_id, doc_path) = setup_db_with_chunks(1);
        let tool = SummarizeDocumentTool;
        let result = tool
            .execute(
                "call-6",
                &format!(r#"{{"path":"{doc_path}"}}"#),
                &db,
                &["different-source".to_string()],
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("outside the current source scope"));
    }
}
