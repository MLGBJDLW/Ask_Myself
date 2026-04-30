//! CreateFileTool — creates new files at arbitrary paths within managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::document_utils::{edit_guidance_for_path, generated_document_mime};
use super::path_utils::{
    has_path_traversal as has_path_traversal_impl, resolve_writable_file_in_sources,
};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/create_file.json");

pub struct CreateFileTool;

#[derive(Deserialize)]
struct CreateFileArgs {
    path: String,
    content: String,
    #[serde(default)]
    overwrite: bool,
}

/// Resolve the requested path against registered source roots and validate
/// that it falls within one of them. Returns the validated path.
/// For new files, walks up the ancestor chain to find the nearest existing
/// directory, canonicalizes it, then reconstructs the full path.
pub(crate) fn resolve_and_validate(
    requested: &Path,
    sources: &[crate::models::Source],
) -> Result<PathBuf, String> {
    resolve_writable_file_in_sources(requested, sources)
}

/// Reject paths containing traversal sequences.
pub(crate) fn has_path_traversal(path: &str) -> bool {
    has_path_traversal_impl(path)
}

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core, ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        Some(format!("Create file: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: CreateFileArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid create_file arguments: {e}")))?;

        // Reject path traversal early.
        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "Path must not contain '..' traversal sequences.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db, &source_scope)?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let requested = PathBuf::from(&args.path);

            let canonical = match resolve_and_validate(&requested, &sources) {
                Ok(p) => p,
                Err(msg) => {
                    return Ok(ToolResult {
                        call_id: call_id.clone(),
                        content: msg,
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

            if generated_document_mime(&canonical).is_some() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: edit_guidance_for_path(&canonical)
                        .unwrap_or_else(|| "Use run_shell + doc-script-editor for Office/PDF creation, validation, conversion, rendering, template work, and rich edits. For Office files, pair it with docx-document-design, pptx-presentation-design, or xlsx-workbook-design as appropriate.".to_string()),
                    is_error: true,
                    artifacts: None,
                });
            }

            // Check if file exists.
            if canonical.exists() && !args.overwrite {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!(
                        "File already exists: '{}'. Set overwrite to true or use edit_file to modify it.",
                        args.path
                    ),
                    is_error: true,
                    artifacts: None,
                });
            }

            // Create parent directories if needed.
            if let Some(parent) = canonical.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
                }
            }

            // Write content.
            if let Err(e) = std::fs::write(&canonical, &args.content) {
                return Ok(ToolResult {
                    call_id,
                    content: format!("Failed to write '{}': {e}", args.path),
                    is_error: true,
                    artifacts: None,
                });
            }

            let size = args.content.len();
            Ok(ToolResult {
                call_id,
                content: format!(
                    "Created file '{}' ({} bytes).\nPath: {}",
                    args.path,
                    size,
                    canonical.display()
                ),
                is_error: false,
                artifacts: None,
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    fn setup_db_with_source(root: &std::path::Path) -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("register source root");
        db
    }

    #[tokio::test]
    async fn test_create_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let file_path = dir.path().join("new_file.txt");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "hello world"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Created file"));

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_create_file_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        std::fs::write(&file_path, "existing content").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "new content"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_file_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        std::fs::write(&file_path, "old").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "new content",
            "overwrite": true
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_create_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sub").join("deep").join("file.md");

        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "# Hello"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn test_create_file_resolves_source_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": "notes/today.md",
            "content": "hello"
        });

        let result = tool
            .execute("c-rel", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes").join("today.md")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn test_create_file_rejects_office_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": "reports/status.docx",
            "content": "this should be structured, not plain text"
        });

        let result = tool
            .execute("c-docx", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("doc-script-editor"));
    }

    #[tokio::test]
    async fn test_create_file_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": format!("{}/../../../etc/passwd", dir.path().display()),
            "content": "evil"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("traversal"));
    }

    #[tokio::test]
    async fn test_create_file_outside_source() {
        let dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let file_path = other_dir.path().join("outside.txt");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "not allowed"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Access denied"));
    }
}
