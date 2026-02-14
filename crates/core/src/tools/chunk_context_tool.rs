//! ChunkContextTool — retrieves a chunk and its surrounding context from the same document.

use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/get_chunk_context.json");

/// Tool that retrieves a chunk and its surrounding chunks from the same document,
/// ordered by `chunk_index`.
pub struct ChunkContextTool;

#[derive(Deserialize)]
struct ChunkContextArgs {
    chunk_id: String,
    #[serde(default = "default_context_chunks")]
    context_chunks: usize,
}

fn default_context_chunks() -> usize {
    2
}

#[async_trait]
impl Tool for ChunkContextTool {
    fn name(&self) -> &str {
        "get_chunk_context"
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
        let args: ChunkContextArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid get_chunk_context arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            let context_count = args.context_chunks.min(5);
            let conn = db.conn();

            // 1. Look up the target chunk to get its document_id and chunk_index.
            let target = conn.query_row(
                "SELECT c.id, c.document_id, c.chunk_index, c.content, d.path, d.title
                 FROM chunks c
                 JOIN documents d ON d.id = c.document_id
                 WHERE c.id = ?1",
                params![&args.chunk_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            );

            let (chunk_id, document_id, target_index, _target_content, doc_path, doc_title) =
                match target {
                    Ok(row) => row,
                    Err(rusqlite::Error::QueryReturnedNoRows) => {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!("Chunk '{}' not found.", args.chunk_id),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    Err(e) => return Err(CoreError::Database(e)),
                };

            // 2. Get all chunks for the same document, ordered by chunk_index.
            let mut stmt = conn.prepare(
                "SELECT c.id, c.chunk_index, c.content
                 FROM chunks c
                 WHERE c.document_id = ?1
                 ORDER BY c.chunk_index",
            )?;

            let all_chunks: Vec<(String, i64, String)> = stmt
                .query_map(params![&document_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            // 3. Find the target chunk's position in the ordered list.
            let target_pos = all_chunks
                .iter()
                .position(|(id, _, _)| id == &chunk_id)
                .unwrap_or(0);

            let start = target_pos.saturating_sub(context_count);
            let end = (target_pos + context_count + 1).min(all_chunks.len());
            let window = &all_chunks[start..end];

            // 4. Format output with clear markers.
            let mut text = format!(
                "Document: {doc_title}\nPath: {doc_path}\nShowing chunks {} to {} of {} total\n\n",
                start + 1,
                end,
                all_chunks.len()
            );

            let mut artifacts = Vec::new();

            for (id, idx, content) in window {
                let marker = if *id == chunk_id {
                    format!("--- [TARGET CHUNK (index {idx})] ---")
                } else {
                    format!("--- [chunk index {idx}] ---")
                };
                text.push_str(&marker);
                text.push('\n');
                text.push_str(content);
                text.push_str("\n\n");

                artifacts.push(json!({
                    "chunkId": id,
                    "chunkIndex": idx,
                    "isTarget": *id == chunk_id,
                    "content": content,
                }));
            }

            // Include navigation hint if there's more beyond the window.
            if start > 0 {
                text.push_str(&format!(
                    "(\u{2026} {} earlier chunk(s) not shown)\n",
                    start
                ));
            }
            if end < all_chunks.len() {
                text.push_str(&format!(
                    "(\u{2026} {} later chunk(s) not shown)\n",
                    all_chunks.len() - end
                ));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(json!({
                    "documentId": document_id,
                    "documentPath": doc_path,
                    "documentTitle": doc_title,
                    "targetChunkIndex": target_index,
                    "totalChunks": all_chunks.len(),
                    "chunks": artifacts,
                })),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
