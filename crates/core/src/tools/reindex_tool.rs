//! ReindexTool — triggers re-indexing of a document by path or an entire source.

use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::ingest;

use super::path_utils::resolve_existing_file_in_sources;
use super::{ensure_source_in_scope, scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/reindex_document.json");

#[derive(Deserialize)]
struct ReindexArgs {
    path: Option<String>,
    source_id: Option<String>,
}

pub struct ReindexTool;

/// Find the source whose `root_path` contains the given file path.
fn find_source_for_path(
    db: &Database,
    file_path: &Path,
) -> Result<Option<crate::models::Source>, CoreError> {
    let sources = db.list_sources()?;
    let canonical = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());

    for source in sources {
        if let Ok(root) = std::fs::canonicalize(Path::new(&source.root_path)) {
            if canonical.starts_with(&root) {
                return Ok(Some(source));
            }
        }
    }
    Ok(None)
}

#[async_trait]
impl Tool for ReindexTool {
    fn name(&self) -> &str {
        "reindex_document"
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

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ReindexArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid reindex_document arguments: {e}"))
        })?;

        if args.path.is_none() && args.source_id.is_none() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "At least one of 'path' or 'source_id' must be provided.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();

        tokio::task::spawn_blocking(move || {
            if let Some(ref source_id) = args.source_id {
                // Validate source exists.
                let source = db.get_source(source_id).map_err(|_| {
                    CoreError::NotFound(format!("Source not found: {source_id}"))
                })?;
                ensure_source_in_scope(&source.id, &source_scope)
                    .map_err(CoreError::InvalidInput)?;

                let result = ingest::scan_source(&db, source_id)?;
                return Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Re-scanned source '{}': {} files scanned, {} added, {} updated, {} skipped.",
                        source_id,
                        result.files_scanned,
                        result.files_added,
                        result.files_updated,
                        result.files_skipped,
                    ),
                    is_error: false,
                    artifacts: None,
                });
            }

            // path mode
            let file_path_str = args.path.as_ref().unwrap();
            let sources = scoped_sources(&db, &source_scope)?;
            let file_path = resolve_existing_file_in_sources(Path::new(file_path_str), &sources)
                .map_err(CoreError::InvalidInput)?;

            let source = match find_source_for_path(&db, &file_path)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult {
                        call_id,
                        content: format!(
                            "File '{}' is not within any registered source directory.",
                            file_path_str
                        ),
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

            // Delete existing document to force re-index (even if hash unchanged).
            // Try multiple path formats since the DB may store OS-native or normalized paths.
            let canonical = std::fs::canonicalize(&file_path)
                .unwrap_or_else(|_| file_path.clone());
            let canonical_str = canonical.to_string_lossy();
            let _ = db.delete_document_by_path(&canonical_str);

            // Also try with forward-slash normalized path.
            let normalized = canonical_str.replace('\\', "/");
            if normalized != *canonical_str {
                let _ = db.delete_document_by_path(&normalized);
            }
            // And try the raw input path.
            let _ = db.delete_document_by_path(file_path_str);

            let outcome = ingest::ingest_single_file(&db, &source.id, &file_path)?;
            let status = match outcome {
                ingest::IngestFileResult::Added => "added (re-indexed)",
                ingest::IngestFileResult::Updated => "updated",
                ingest::IngestFileResult::Unchanged => "re-indexed (unchanged content)",
            };

            Ok(ToolResult {
                call_id,
                content: format!("Document '{}' {status}.", file_path_str),
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
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_db_with_source(dir: &Path) -> (Database, String) {
        let db = Database::open_memory().expect("open in-memory db");
        let source = db
            .add_source(crate::sources::CreateSourceInput {
                root_path: dir.to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: false,
            })
            .expect("add source");
        (db, source.id)
    }

    fn create_test_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create file");
        f.write_all(content.as_bytes()).expect("write file");
        path
    }

    #[tokio::test]
    async fn test_reindex_single_file() {
        let tmp = TempDir::new().expect("tempdir");
        let file = create_test_file(tmp.path(), "note.md", "# Hello\nWorld\n");
        let (db, source_id) = setup_db_with_source(tmp.path());

        // Initial ingest
        ingest::ingest_single_file(&db, &source_id, &file).expect("initial ingest");

        // Update the file content
        std::fs::write(&file, "# Hello\nUpdated world\n").expect("overwrite");

        // Reindex via tool
        let tool = ReindexTool;
        let args = serde_json::json!({ "path": file.to_string_lossy() }).to_string();
        let result = tool.execute("c1", &args, &db, &[]).await.expect("execute");

        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(
            result.content.contains("added (re-indexed)") || result.content.contains("updated"),
            "Unexpected message: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_reindex_source() {
        let tmp = TempDir::new().expect("tempdir");
        create_test_file(tmp.path(), "a.md", "# A\nContent A\n");
        create_test_file(tmp.path(), "b.md", "# B\nContent B\n");
        let (db, source_id) = setup_db_with_source(tmp.path());

        let tool = ReindexTool;
        let args = serde_json::json!({ "source_id": source_id }).to_string();
        let result = tool.execute("c2", &args, &db, &[]).await.expect("execute");

        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("Re-scanned source"));
        assert!(result.content.contains("2 files scanned"));
    }

    #[tokio::test]
    async fn test_reindex_requires_param() {
        let db = Database::open_memory().expect("open in-memory db");
        let tool = ReindexTool;
        let args = "{}";
        let result = tool.execute("c3", args, &db, &[]).await.expect("execute");
        assert!(result.is_error);
        assert!(result.content.contains("At least one"));
    }
}
