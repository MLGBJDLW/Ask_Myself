//! ManageSourceTool — add or remove knowledge source directories.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::sources::CreateSourceInput;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_source.json");

#[derive(Deserialize)]
struct ManageSourceArgs {
    action: String,
    path: Option<String>,
    source_id: Option<String>,
}

pub struct ManageSourceTool;

#[async_trait]
impl Tool for ManageSourceTool {
    fn name(&self) -> &str {
        "manage_source"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SourceManagement]
    }

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        args.get("action").and_then(|v| v.as_str()) == Some("remove")
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        if args.get("action").and_then(|v| v.as_str()) == Some("remove") {
            let id = args.get("source_id").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            Some(format!("Remove source: {id}"))
        } else {
            None
        }
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ManageSourceArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_source arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || match args.action.as_str() {
            "add" => {
                let path = args.path.ok_or_else(|| {
                    CoreError::InvalidInput(
                        "'path' is required when action is 'add'".to_string(),
                    )
                })?;

                let input = CreateSourceInput {
                    root_path: path,
                    include_globs: vec![],
                    exclude_globs: vec![],
                    watch_enabled: true,
                };
                let source = db.add_source(input)?;

                Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Source added successfully.\n  ID: {}\n  Path: {}\n  Indexing will begin automatically.",
                        source.id, source.root_path
                    ),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "id": source.id,
                        "root_path": source.root_path,
                        "kind": source.kind,
                    })),
                })
            }
            "remove" => {
                let source_id = args.source_id.ok_or_else(|| {
                    CoreError::InvalidInput(
                        "'source_id' is required when action is 'remove'".to_string(),
                    )
                })?;

                // get_source validates existence; delete_source cascades.
                let source = db.get_source(&source_id)?;
                db.delete_source(&source_id)?;

                Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Source removed successfully.\n  ID: {}\n  Path: {}",
                        source.id, source.root_path
                    ),
                    is_error: false,
                    artifacts: None,
                })
            }
            other => Ok(ToolResult {
                call_id,
                content: format!("Unknown action '{other}'. Use 'add' or 'remove'."),
                is_error: true,
                artifacts: None,
            }),
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_memory().expect("failed to open in-memory db")
    }

    #[tokio::test]
    async fn test_manage_source_add() {
        let db = test_db();
        let tool = ManageSourceTool;
        let dir = tempfile::tempdir().expect("tempdir");

        let args = serde_json::json!({
            "action": "add",
            "path": dir.path().to_string_lossy()
        });
        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .expect("execute should succeed");

        assert!(
            !result.is_error,
            "result should not be error: {}",
            result.content
        );
        assert!(result.content.contains("Source added successfully"));
        assert!(result.artifacts.is_some());

        // Verify source is now in the database.
        let sources = db.list_sources().expect("list_sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].root_path,
            dir.path().to_string_lossy().to_string()
        );
    }

    #[tokio::test]
    async fn test_manage_source_add_duplicate() {
        let db = test_db();
        let tool = ManageSourceTool;
        let dir = tempfile::tempdir().expect("tempdir");

        let args = serde_json::json!({
            "action": "add",
            "path": dir.path().to_string_lossy()
        });
        tool.execute("c1", &args.to_string(), &db, &[])
            .await
            .expect("first add should succeed");

        // Second add should fail (duplicate path).
        let result = tool.execute("c2", &args.to_string(), &db, &[]).await;
        assert!(result.is_err(), "duplicate add should error");
    }

    #[tokio::test]
    async fn test_manage_source_remove() {
        let db = test_db();
        let tool = ManageSourceTool;
        let dir = tempfile::tempdir().expect("tempdir");

        // Add first.
        let add_args = serde_json::json!({
            "action": "add",
            "path": dir.path().to_string_lossy()
        });
        let add_result = tool
            .execute("c1", &add_args.to_string(), &db, &[])
            .await
            .expect("add should succeed");

        let source_id = add_result.artifacts.as_ref().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Remove.
        let rm_args = serde_json::json!({
            "action": "remove",
            "source_id": source_id
        });
        let rm_result = tool
            .execute("c2", &rm_args.to_string(), &db, &[])
            .await
            .expect("remove should succeed");

        assert!(!rm_result.is_error);
        assert!(rm_result.content.contains("Source removed successfully"));

        // Verify empty.
        let sources = db.list_sources().expect("list_sources");
        assert!(sources.is_empty());
    }

    #[tokio::test]
    async fn test_manage_source_remove_nonexistent() {
        let db = test_db();
        let tool = ManageSourceTool;

        let args = serde_json::json!({
            "action": "remove",
            "source_id": "nonexistent-id"
        });
        let result = tool.execute("c1", &args.to_string(), &db, &[]).await;
        assert!(result.is_err(), "removing nonexistent source should error");
    }

    #[tokio::test]
    async fn test_manage_source_add_missing_path() {
        let db = test_db();
        let tool = ManageSourceTool;

        let args = serde_json::json!({ "action": "add" });
        let result = tool.execute("c1", &args.to_string(), &db, &[]).await;
        assert!(result.is_err(), "add without path should error");
    }
}
