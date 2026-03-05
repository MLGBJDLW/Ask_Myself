//! SubmitFeedbackTool — records user feedback on search result chunks.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::feedback::FeedbackAction;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/submit_feedback.json");

#[derive(Deserialize)]
struct SubmitFeedbackArgs {
    chunk_id: String,
    kind: FeedbackAction,
    #[serde(default)]
    query: Option<String>,
}

pub struct SubmitFeedbackTool;

#[async_trait]
impl Tool for SubmitFeedbackTool {
    fn name(&self) -> &str {
        "submit_feedback"
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
        let args: SubmitFeedbackArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid submit_feedback arguments: {e}"))
        })?;

        let query_text = args.query.as_deref().unwrap_or("");
        let action = args.kind;

        let db = db.clone();
        let call_id = call_id.to_string();
        let chunk_id = args.chunk_id;
        let query_text = query_text.to_string();
        let action_display = action.to_string();

        tokio::task::spawn_blocking(move || {
            let feedback = db.add_feedback(&chunk_id, &query_text, action)?;
            Ok(ToolResult {
                call_id,
                content: format!(
                    "Feedback recorded: {} on chunk {} (id: {}).",
                    action_display, chunk_id, feedback.id
                ),
                is_error: false,
                artifacts: None,
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// Insert the minimum row chain needed for a valid chunk_id FK.
    fn insert_test_chunk(db: &Database, chunk_id: &str) {
        let source_id = "src-test";
        let doc_id = "doc-test";
        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', '/tmp/test', '[]', '[]', 0)",
            params![source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/test/a.md', 'text/plain', 100, datetime('now'), 'hash')",
            params![doc_id, source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, 0, 'text', 'test content', 0, 12, 1, 1, 'chunkhash')",
            params![chunk_id, doc_id],
        )
        .unwrap();
    }

    #[test]
    fn test_tool_definition_loads() {
        let tool = SubmitFeedbackTool;
        assert_eq!(tool.name(), "submit_feedback");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["chunk_id"].is_object());
        assert!(schema["properties"]["kind"].is_object());
    }

    #[tokio::test]
    async fn test_execute_records_feedback() {
        let db = Database::open_memory().expect("in-memory db");
        insert_test_chunk(&db, "chunk-abc-123");
        let tool = SubmitFeedbackTool;

        let args = serde_json::json!({
            "chunk_id": "chunk-abc-123",
            "kind": "upvote",
            "query": "how does search work"
        });

        let result = tool
            .execute("call-1", &args.to_string(), &db, &[])
            .await
            .expect("execute should succeed");

        assert!(!result.is_error, "result should not be error: {}", result.content);
        assert!(result.content.contains("upvote"));
        assert!(result.content.contains("chunk-abc-123"));

        // Verify it was persisted.
        let entries = db
            .get_feedback_for_chunk("chunk-abc-123")
            .expect("query feedback");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, FeedbackAction::Upvote);
        assert_eq!(entries[0].query_text, "how does search work");
    }

    #[tokio::test]
    async fn test_execute_invalid_kind_returns_error() {
        let db = Database::open_memory().expect("in-memory db");
        let tool = SubmitFeedbackTool;

        let args = serde_json::json!({
            "chunk_id": "chunk-xyz",
            "kind": "invalid_action"
        });

        let result = tool.execute("call-2", &args.to_string(), &db, &[]).await;
        // Serde deserialization should fail for invalid enum variant.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_without_query() {
        let db = Database::open_memory().expect("in-memory db");
        insert_test_chunk(&db, "chunk-no-query");
        let tool = SubmitFeedbackTool;

        let args = serde_json::json!({
            "chunk_id": "chunk-no-query",
            "kind": "downvote"
        });

        let result = tool
            .execute("call-3", &args.to_string(), &db, &[])
            .await
            .expect("execute should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("downvote"));

        let entries = db
            .get_feedback_for_chunk("chunk-no-query")
            .expect("query feedback");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].query_text, "");
    }
}
