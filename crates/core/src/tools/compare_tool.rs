//! CompareTool — compares content between two documents or chunks.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::document_utils::read_supported_file_content;
use super::path_utils::resolve_existing_file_in_sources;
use super::{ensure_source_in_scope, scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/compare_documents.json");

pub struct CompareTool;

#[derive(Deserialize)]
struct CompareArgs {
    path_a: Option<String>,
    path_b: Option<String>,
    chunk_id_a: Option<String>,
    chunk_id_b: Option<String>,
}

/// Read a file after validating it lives inside a registered source root.
fn read_validated_file(
    path_str: &str,
    sources: &[crate::models::Source],
) -> Result<String, String> {
    let canonical = resolve_existing_file_in_sources(Path::new(path_str), sources)
        .map_err(|e| e.to_string())?;
    read_supported_file_content(&canonical).map_err(|e| format!("Cannot read '{}': {e}", path_str))
}

/// Retrieve chunk content from the database.
fn read_chunk(db: &Database, chunk_id: &str) -> Result<(String, String, String), String> {
    let conn = db.conn();
    conn.query_row(
        "SELECT c.content, d.path, d.source_id FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id = ?1",
        params![chunk_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("Chunk '{}' not found.", chunk_id),
        other => format!("Database error looking up chunk '{}': {other}", chunk_id),
    })
}

/// Simple line-based comparison. Returns (common, only_a, only_b) line sets.
fn compare_lines(text_a: &str, text_b: &str) -> (usize, Vec<String>, Vec<String>) {
    let lines_a: Vec<&str> = text_a.lines().collect();
    let lines_b: Vec<&str> = text_b.lines().collect();

    let set_a: HashSet<&str> = lines_a.iter().copied().collect();
    let set_b: HashSet<&str> = lines_b.iter().copied().collect();

    let common = set_a.intersection(&set_b).count();
    let only_a: Vec<String> = lines_a
        .iter()
        .filter(|l| !set_b.contains(**l) && !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    let only_b: Vec<String> = lines_b
        .iter()
        .filter(|l| !set_a.contains(**l) && !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    (common, only_a, only_b)
}

/// Format the comparison output.
fn format_comparison(label_a: &str, label_b: &str, text_a: &str, text_b: &str) -> String {
    let (common, only_a, only_b) = compare_lines(text_a, text_b);
    let total_a = text_a.lines().count();
    let total_b = text_b.lines().count();

    let mut out = String::new();
    out.push_str(&format!("## Comparison: {label_a} vs {label_b}\n\n"));
    out.push_str(&format!(
        "| Metric | A ({label_a}) | B ({label_b}) |\n|---|---|---|\n"
    ));
    out.push_str(&format!("| Total lines | {total_a} | {total_b} |\n"));
    out.push_str(&format!("| Common lines | {common} | {common} |\n"));
    out.push_str(&format!(
        "| Unique lines | {} | {} |\n\n",
        only_a.len(),
        only_b.len()
    ));

    const MAX_DISPLAY: usize = 20;

    if !only_a.is_empty() {
        out.push_str(&format!("### Lines only in A ({label_a})\n```\n"));
        for line in only_a.iter().take(MAX_DISPLAY) {
            out.push_str(line);
            out.push('\n');
        }
        if only_a.len() > MAX_DISPLAY {
            out.push_str(&format!(
                "... and {} more lines\n",
                only_a.len() - MAX_DISPLAY
            ));
        }
        out.push_str("```\n\n");
    }

    if !only_b.is_empty() {
        out.push_str(&format!("### Lines only in B ({label_b})\n```\n"));
        for line in only_b.iter().take(MAX_DISPLAY) {
            out.push_str(line);
            out.push('\n');
        }
        if only_b.len() > MAX_DISPLAY {
            out.push_str(&format!(
                "... and {} more lines\n",
                only_b.len() - MAX_DISPLAY
            ));
        }
        out.push_str("```\n\n");
    }

    if only_a.is_empty() && only_b.is_empty() {
        out.push_str("The two documents have identical line content.\n");
    }

    out
}

#[async_trait]
impl Tool for CompareTool {
    fn name(&self) -> &str {
        "compare_documents"
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
        let args: CompareArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid compare_documents arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            // Determine mode: file paths or chunk IDs.
            let (text_a, label_a, text_b, label_b) =
                match (&args.path_a, &args.path_b, &args.chunk_id_a, &args.chunk_id_b) {
                    (Some(pa), Some(pb), _, _) => {
                        let sources = scoped_sources(&db, &source_scope)?;
                        if sources.is_empty() {
                            return Ok(ToolResult {
                                call_id,
                                content: "The current source scope does not include any readable files."
                                    .to_string(),
                                is_error: true,
                                artifacts: None,
                            });
                        }
                        let a = read_validated_file(pa, &sources).map_err(|e| {
                            CoreError::InvalidInput(e)
                        })?;
                        let b = read_validated_file(pb, &sources).map_err(|e| {
                            CoreError::InvalidInput(e)
                        })?;
                        (a, pa.clone(), b, pb.clone())
                    }
                    (_, _, Some(ca), Some(cb)) => {
                        let (text_a, path_a, source_a) = read_chunk(&db, ca).map_err(|e| {
                            CoreError::InvalidInput(e)
                        })?;
                        let (text_b, path_b, source_b) = read_chunk(&db, cb).map_err(|e| {
                            CoreError::InvalidInput(e)
                        })?;
                        ensure_source_in_scope(&source_a, &source_scope)
                            .map_err(CoreError::InvalidInput)?;
                        ensure_source_in_scope(&source_b, &source_scope)
                            .map_err(CoreError::InvalidInput)?;
                        let la = format!("chunk {} ({})", &ca[..ca.len().min(8)], path_a);
                        let lb = format!("chunk {} ({})", &cb[..cb.len().min(8)], path_b);
                        (text_a, la, text_b, lb)
                    }
                    _ => {
                        return Ok(ToolResult {
                            call_id,
                            content: "Provide either both path_a and path_b, or both chunk_id_a and chunk_id_b.".to_string(),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                };

            let output = format_comparison(&label_a, &label_b, &text_a, &text_b);

            Ok(ToolResult {
                call_id,
                content: output,
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
    use std::path::Path;

    fn setup_db_with_source(root: &Path) -> Database {
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
    async fn compare_two_files_shows_diff() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file_a = dir.path().join("a.txt");
        let file_b = dir.path().join("b.txt");
        std::fs::write(&file_a, "hello\nworld\nfoo\n").unwrap();
        std::fs::write(&file_b, "hello\nworld\nbar\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CompareTool;
        let args = serde_json::json!({
            "path_a": file_a.to_string_lossy(),
            "path_b": file_b.to_string_lossy(),
        });
        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .expect("execute");

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("Common lines"),
            "missing common-lines section"
        );
        assert!(result.content.contains("foo"), "missing line only in A");
        assert!(result.content.contains("bar"), "missing line only in B");
    }

    #[tokio::test]
    async fn compare_identical_files() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file_a = dir.path().join("same1.txt");
        let file_b = dir.path().join("same2.txt");
        let content = "line one\nline two\nline three\n";
        std::fs::write(&file_a, content).unwrap();
        std::fs::write(&file_b, content).unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CompareTool;
        let args = serde_json::json!({
            "path_a": file_a.to_string_lossy(),
            "path_b": file_b.to_string_lossy(),
        });
        let result = tool
            .execute("c2", &args.to_string(), &db, &[])
            .await
            .expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("identical line content"));
    }

    #[tokio::test]
    async fn compare_rejects_missing_params() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = setup_db_with_source(dir.path());
        let tool = CompareTool;
        // Provide only path_a, missing path_b
        let args = serde_json::json!({ "path_a": "/tmp/x.txt" });
        let result = tool
            .execute("c3", &args.to_string(), &db, &[])
            .await
            .expect("execute");

        assert!(result.is_error);
        assert!(result.content.contains("Provide either both"));
    }

    #[test]
    fn compare_lines_basic() {
        let a = "hello\nworld\nfoo";
        let b = "hello\nworld\nbar";
        let (common, only_a, only_b) = compare_lines(a, b);
        assert_eq!(common, 2);
        assert_eq!(only_a, vec!["foo"]);
        assert_eq!(only_b, vec!["bar"]);
    }
}
