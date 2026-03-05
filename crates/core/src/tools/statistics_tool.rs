//! GetStatisticsTool — returns knowledge base health metrics.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/get_statistics.json");

#[derive(Deserialize)]
struct GetStatisticsArgs {
    source_id: Option<String>,
}

/// Tool that returns knowledge base statistics.
pub struct GetStatisticsTool;

#[async_trait]
impl Tool for GetStatisticsTool {
    fn name(&self) -> &str {
        "get_statistics"
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
        let args: GetStatisticsArgs = serde_json::from_str(arguments).unwrap_or(GetStatisticsArgs {
            source_id: None,
        });

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = db.conn();

            if let Some(ref sid) = args.source_id {
                // --- Per-source statistics ---
                let source_exists: bool = conn
                    .prepare("SELECT 1 FROM sources WHERE id = ?1")?
                    .exists(rusqlite::params![sid])?;
                if !source_exists {
                    return Ok(ToolResult {
                        call_id,
                        content: format!("Source '{sid}' not found."),
                        is_error: true,
                        artifacts: None,
                    });
                }

                let doc_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM documents WHERE source_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )?;
                let chunk_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chunks c JOIN documents d ON c.document_id = d.id WHERE d.source_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )?;
                let total_size: i64 = conn.query_row(
                    "SELECT COALESCE(SUM(file_size), 0) FROM documents WHERE source_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )?;
                let last_indexed: Option<String> = conn.query_row(
                    "SELECT MAX(indexed_at) FROM documents WHERE source_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )?;

                let text = format!(
                    "Statistics for source {sid}:\n\
                     - Documents: {doc_count}\n\
                     - Chunks: {chunk_count}\n\
                     - Total file size: {}\n\
                     - Last indexed: {}",
                    format_bytes(total_size),
                    last_indexed.as_deref().unwrap_or("never"),
                );

                Ok(ToolResult {
                    call_id,
                    content: text,
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "source_id": sid,
                        "documents": doc_count,
                        "chunks": chunk_count,
                        "total_file_size_bytes": total_size,
                        "last_indexed": last_indexed,
                    })),
                })
            } else {
                // --- Global statistics ---
                let source_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))?;
                let doc_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
                let chunk_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
                let total_size: i64 = conn.query_row(
                    "SELECT COALESCE(SUM(file_size), 0) FROM documents",
                    [],
                    |r| r.get(0),
                )?;
                let last_indexed: Option<String> = conn.query_row(
                    "SELECT MAX(indexed_at) FROM documents",
                    [],
                    |r| r.get(0),
                )?;
                let playbook_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM playbooks", [], |r| r.get(0))?;

                // Per-source breakdown
                let mut stmt = conn.prepare(
                    "SELECT s.id, s.root_path, COUNT(d.id) AS doc_count
                     FROM sources s
                     LEFT JOIN documents d ON d.source_id = s.id
                     GROUP BY s.id
                     ORDER BY doc_count DESC",
                )?;
                let breakdown: Vec<(String, String, i64)> = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;

                // DB file size
                let db_size_str = match db.db_path() {
                    Some(p) => std::fs::metadata(p)
                        .map(|m| format_bytes(m.len() as i64))
                        .unwrap_or_else(|_| "unknown".to_string()),
                    None => "in-memory".to_string(),
                };

                let mut text = format!(
                    "Knowledge base statistics:\n\
                     - Sources: {source_count}\n\
                     - Documents: {doc_count}\n\
                     - Chunks: {chunk_count}\n\
                     - Playbooks: {playbook_count}\n\
                     - Total file size: {}\n\
                     - Database size: {db_size_str}\n\
                     - Last indexed: {}\n",
                    format_bytes(total_size),
                    last_indexed.as_deref().unwrap_or("never"),
                );

                if !breakdown.is_empty() {
                    text.push_str("\nDocuments per source:\n");
                    for (id, root_path, count) in &breakdown {
                        text.push_str(&format!("  - {root_path} ({id}): {count} docs\n"));
                    }
                }

                Ok(ToolResult {
                    call_id,
                    content: text,
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "sources": source_count,
                        "documents": doc_count,
                        "chunks": chunk_count,
                        "playbooks": playbook_count,
                        "total_file_size_bytes": total_size,
                        "database_size": db_size_str,
                        "last_indexed": last_indexed,
                    })),
                })
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

fn format_bytes(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = 1024 * 1024;
    const GB: i64 = 1024 * 1024 * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();
        conn.execute_batch(
            "INSERT INTO sources (id, kind, root_path) VALUES ('s1', 'local_folder', '/tmp/src1');
             INSERT INTO sources (id, kind, root_path) VALUES ('s2', 'local_folder', '/tmp/src2');
             INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
                 VALUES ('d1', 's1', '/tmp/src1/a.md', 'text/markdown', 1024, '2025-01-01', 'h1');
             INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
                 VALUES ('d2', 's1', '/tmp/src1/b.md', 'text/markdown', 2048, '2025-01-02', 'h2');
             INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
                 VALUES ('d3', 's2', '/tmp/src2/c.md', 'text/markdown', 512, '2025-01-03', 'h3');
             INSERT INTO chunks (id, document_id, chunk_index, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES ('c1', 'd1', 0, 'chunk1', 0, 10, 1, 5, 'ch1');
             INSERT INTO chunks (id, document_id, chunk_index, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES ('c2', 'd1', 1, 'chunk2', 10, 20, 5, 10, 'ch2');
             INSERT INTO chunks (id, document_id, chunk_index, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES ('c3', 'd2', 0, 'chunk3', 0, 15, 1, 8, 'ch3');
             INSERT INTO chunks (id, document_id, chunk_index, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES ('c4', 'd3', 0, 'chunk4', 0, 12, 1, 6, 'ch4');",
        )
        .unwrap();
        drop(conn);
        db
    }

    #[tokio::test]
    async fn test_global_statistics() {
        let db = setup_db();
        let tool = GetStatisticsTool;

        let result = tool.execute("call1", "{}", &db, &[]).await.unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Sources: 2"));
        assert!(result.content.contains("Documents: 3"));
        assert!(result.content.contains("Chunks: 4"));
        assert!(result.content.contains("3.5 KB")); // 1024 + 2048 + 512 = 3584

        let artifacts = result.artifacts.unwrap();
        assert_eq!(artifacts["sources"], 2);
        assert_eq!(artifacts["documents"], 3);
        assert_eq!(artifacts["chunks"], 4);
    }

    #[tokio::test]
    async fn test_per_source_statistics() {
        let db = setup_db();
        let tool = GetStatisticsTool;

        let result = tool
            .execute("call2", r#"{"source_id": "s1"}"#, &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Documents: 2"));
        assert!(result.content.contains("Chunks: 3"));
        assert!(result.content.contains("3.0 KB")); // 1024 + 2048 = 3072

        let artifacts = result.artifacts.unwrap();
        assert_eq!(artifacts["documents"], 2);
        assert_eq!(artifacts["chunks"], 3);
    }

    #[tokio::test]
    async fn test_unknown_source_returns_error() {
        let db = setup_db();
        let tool = GetStatisticsTool;

        let result = tool
            .execute("call3", r#"{"source_id": "nonexistent"}"#, &db, &[])
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
