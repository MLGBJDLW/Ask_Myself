//! SummarizeTool — retrieves and formats evidence cards by chunk IDs.

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolResult};

/// Tool that looks up specific chunks by their IDs and returns formatted
/// content suitable for citation in LLM responses.
pub struct SummarizeTool;

#[derive(Deserialize)]
struct SummarizeArgs {
    chunk_ids: Vec<String>,
}

#[async_trait]
impl Tool for SummarizeTool {
    fn name(&self) -> &str {
        "summarize_evidence"
    }

    fn description(&self) -> &str {
        "Retrieve and format specific evidence cards by their chunk IDs for citation. \
         Returns the chunk content together with source path and document title."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "chunk_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of chunk IDs to retrieve and format"
                }
            },
            "required": ["chunk_ids"]
        })
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SummarizeArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid summarize_evidence arguments: {e}"))
        })?;

        if args.chunk_ids.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "No chunk IDs provided.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let conn = db.conn();

        let mut text = String::new();
        let mut found = 0usize;
        let mut artifacts: Vec<serde_json::Value> = Vec::new();

        for chunk_id in &args.chunk_ids {
            let row = conn.query_row(
                "SELECT c.id, c.content, d.path, d.title
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
                    ))
                },
            );

            match row {
                Ok((id, content, path, title)) => {
                    found += 1;
                    text.push_str(&format!(
                        "--- Chunk {} ---\n\
                         Path: {}\n\
                         Title: {}\n\
                         Content:\n{}\n\n",
                        id, path, title, content
                    ));
                    artifacts.push(json!({
                        "chunkId": id,
                        "path": path,
                        "title": title,
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
            call_id: call_id.to_string(),
            content: format!("{header}{text}"),
            is_error: false,
            artifacts: Some(serde_json::to_value(&artifacts).unwrap_or_default()),
        })
    }
}
