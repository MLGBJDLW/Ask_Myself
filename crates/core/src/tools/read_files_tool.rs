//! `read_files` — batch-reads multiple files in a single call.
//!
//! Mirrors [`FileTool`](super::file_tool::FileTool)'s sandboxing and content
//! extraction rules, but operates on up to `MAX_FILES_PER_CALL` paths in
//! parallel. Designed to save LLM round-trips when the model already knows
//! which files it wants.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use futures::future::join_all;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;
use crate::privacy::{self, PrivacyConfig};

use super::document_utils::read_supported_file_content;
use super::path_utils::resolve_existing_file_in_sources;
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/read_files.json");

const MAX_FILES_PER_CALL: usize = 20;
const DEFAULT_MAX_LINES_PER_FILE: usize = 500;

pub struct ReadFilesTool;

#[derive(Deserialize)]
struct ReadFilesArgs {
    paths: Vec<String>,
    #[serde(default)]
    max_lines_per_file: Option<usize>,
}

/// Per-file read outcome. Kept private — serialised directly into the
/// tool result JSON.
struct FileReadOutcome {
    path: String,
    result: Result<FileReadOk, String>,
}

struct FileReadOk {
    canonical_path: String,
    content: String,
    total_lines: usize,
    shown_lines: usize,
    truncated: bool,
}

#[async_trait]
impl Tool for ReadFilesTool {
    fn name(&self) -> &str {
        "read_files"
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

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ReadFilesArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid read_files arguments: {e}")))?;

        if args.paths.is_empty() {
            return Err(CoreError::InvalidInput(
                "read_files requires at least one path".to_string(),
            ));
        }
        if args.paths.len() > MAX_FILES_PER_CALL {
            return Err(CoreError::InvalidInput(format!(
                "read_files accepts at most {MAX_FILES_PER_CALL} files per call (got {})",
                args.paths.len()
            )));
        }

        let max_lines = args
            .max_lines_per_file
            .unwrap_or(DEFAULT_MAX_LINES_PER_FILE)
            .max(1);

        // Resolve sources + privacy config once, outside the per-file tasks.
        let sources = scoped_sources(db, source_scope)?;
        let privacy_config = db.load_privacy_config().unwrap_or_default();

        // If there are no sources in scope every path is inaccessible —
        // return a uniform error per path rather than failing the whole call.
        let tasks = args.paths.into_iter().map(|raw_path| {
            let sources = sources.clone();
            let privacy_config = privacy_config.clone();
            tokio::task::spawn_blocking(move || {
                let outcome = read_single_file(&raw_path, &sources, &privacy_config, max_lines);
                FileReadOutcome {
                    path: raw_path,
                    result: outcome,
                }
            })
        });

        let results = join_all(tasks).await;

        let mut files_json = Vec::with_capacity(results.len());
        let mut text_blocks: Vec<String> = Vec::with_capacity(results.len());
        let mut all_errored = true;

        for joined in results {
            let outcome = joined
                .map_err(|e| CoreError::Internal(format!("read_files task join failed: {e}")))?;
            match outcome.result {
                Ok(ok) => {
                    all_errored = false;
                    let mut header = format!("File: {}", ok.canonical_path);
                    if ok.truncated {
                        header.push_str(&format!(
                            " (showing lines 1–{} of {})",
                            ok.shown_lines, ok.total_lines
                        ));
                    }
                    text_blocks.push(format!("{}\n---\n{}", header, ok.content));
                    files_json.push(json!({
                        "path": ok.canonical_path,
                        "content": ok.content,
                        "totalLines": ok.total_lines,
                        "shownLines": ok.shown_lines,
                        "truncated": ok.truncated,
                    }));
                }
                Err(msg) => {
                    text_blocks.push(format!("File: {}\nError: {}", outcome.path, msg));
                    files_json.push(json!({
                        "path": outcome.path,
                        "error": msg,
                    }));
                }
            }
        }

        let content = text_blocks.join("\n\n");
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: all_errored,
            artifacts: Some(json!({ "files": files_json })),
        })
    }
}

/// Blocking per-file read. Mirrors FileTool's logic but on a fixed window
/// (always starts at line 1) and returns a Result rather than a `ToolResult`.
fn read_single_file(
    raw_path: &str,
    sources: &[crate::models::Source],
    privacy_config: &PrivacyConfig,
    max_lines: usize,
) -> Result<FileReadOk, String> {
    if sources.is_empty() {
        return Err(format!(
            "Access denied: '{}' is not within any directory available in the current source scope.",
            raw_path
        ));
    }

    let requested = PathBuf::from(raw_path);
    let canonical =
        resolve_existing_file_in_sources(&requested, sources).map_err(|e| e.to_string())?;

    let raw = read_supported_file_content(&canonical).map_err(|e| e.to_string())?;

    let total_lines = raw.lines().count();
    let lines: Vec<&str> = raw.lines().take(max_lines).collect();
    let shown = lines.len();
    let truncated = shown < total_lines;
    let mut content = lines.join("\n");
    if truncated {
        let remaining = total_lines - shown;
        content.push_str(&format!("\n...[truncated, {remaining} more lines]"));
    }

    let redacted = if privacy_config.enabled {
        privacy::redact_content(&content, &privacy_config.redact_patterns)
    } else {
        content
    };

    Ok(FileReadOk {
        canonical_path: canonical.to_string_lossy().to_string(),
        content: redacted,
        total_lines,
        shown_lines: shown,
        truncated,
    })
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
    async fn read_files_returns_content_for_multiple_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "alpha line 1\nalpha line 2\n").unwrap();
        std::fs::write(&b, "beta line 1\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = ReadFilesTool;
        let args = json!({
            "paths": [a.to_string_lossy(), b.to_string_lossy()]
        })
        .to_string();

        let result = tool.execute("call-1", &args, &db, &[]).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("alpha line 1"));
        assert!(result.content.contains("beta line 1"));

        let artifacts = result.artifacts.expect("artifacts");
        let files = artifacts["files"].as_array().expect("files array");
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn read_files_rejects_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = ReadFilesTool;
        let paths: Vec<String> = (0..(MAX_FILES_PER_CALL + 1))
            .map(|i| format!("f{i}.txt"))
            .collect();
        let args = json!({ "paths": paths }).to_string();

        let err = tool
            .execute("call-over", &args, &db, &[])
            .await
            .expect_err("should reject over-limit");
        match err {
            CoreError::InvalidInput(msg) => {
                assert!(msg.contains("at most"), "msg was: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_files_reports_per_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.txt");
        std::fs::write(&ok, "hi\n").unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = ReadFilesTool;
        let args = json!({
            "paths": [ok.to_string_lossy(), "does-not-exist.txt"]
        })
        .to_string();

        let result = tool.execute("call-mix", &args, &db, &[]).await.unwrap();
        // One succeeded, so overall not an error.
        assert!(!result.is_error);
        let artifacts = result.artifacts.unwrap();
        let files = artifacts["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        let missing = files
            .iter()
            .find(|f| f["path"] == "does-not-exist.txt")
            .expect("missing entry");
        assert!(missing["error"].is_string());
    }

    #[tokio::test]
    async fn read_files_truncates_at_max_lines() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.txt");
        let body: String = (0..10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(&big, &body).unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = ReadFilesTool;
        let args = json!({
            "paths": [big.to_string_lossy()],
            "max_lines_per_file": 3
        })
        .to_string();

        let result = tool.execute("call-trunc", &args, &db, &[]).await.unwrap();
        assert!(result.content.contains("line0"));
        assert!(result.content.contains("line2"));
        assert!(!result.content.contains("line9"));
        assert!(result.content.contains("truncated"));
    }
}
