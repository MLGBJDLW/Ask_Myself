//! FileTool — reads files from managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::privacy;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/read_file.json");

/// Tool that reads a file from the knowledge base, validating that it
/// belongs to a registered source root and optionally applying privacy
/// redaction.
pub struct FileTool;

#[derive(Deserialize)]
struct FileArgs {
    path: String,
    #[serde(default = "default_start_line")]
    start_line: usize,
    #[serde(default = "default_max_lines")]
    max_lines: usize,
}

fn default_start_line() -> usize {
    1
}

fn default_max_lines() -> usize {
    100
}

fn is_binary_file_error(err: &CoreError) -> bool {
    matches!(err, CoreError::Parse(msg) if msg.starts_with("File appears to be binary:"))
}

fn supports_document_fallback(path: &Path) -> bool {
    let mime = crate::parse::detect_mime_type(path);
    matches!(
        mime.as_str(),
        "application/pdf"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    ) || mime.starts_with("image/")
}

fn flatten_parsed_document_text(parsed: &crate::parse::ParsedDocument) -> String {
    let mut out = String::new();
    for chunk in &parsed.chunks {
        let visible = chunk
            .content
            .get(chunk.overlap_start..)
            .unwrap_or(chunk.content.as_str());
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(visible);
    }

    if out.trim().is_empty() {
        format!(
            "[No extractable text found in document: {}]",
            parsed.file_name
        )
    } else {
        out
    }
}

fn read_file_content(path: &Path) -> Result<String, CoreError> {
    match crate::parse::read_text_file(path) {
        Ok(raw) => Ok(raw),
        Err(err) if is_binary_file_error(&err) && supports_document_fallback(path) => {
            let parsed = crate::parse::parse_file(path, None, None)?;
            Ok(flatten_parsed_document_text(&parsed))
        }
        Err(err) => Err(err),
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &str {
        "read_file"
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
        let args: FileArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid read_file arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            let requested = PathBuf::from(&args.path);

            // Canonicalize the requested path so we can compare prefixes reliably.
            let canonical = std::fs::canonicalize(&requested).map_err(|e| {
                CoreError::InvalidInput(format!("Cannot resolve path '{}': {e}", args.path))
            })?;

            // Validate that the file is inside a registered source root.
            let sources = db.list_sources()?;
            let allowed = sources.iter().any(|s| {
                if let Ok(root) = std::fs::canonicalize(Path::new(&s.root_path)) {
                    canonical.starts_with(&root)
                } else {
                    false
                }
            });

            if !allowed {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!(
                        "Access denied: '{}' is not within any registered source directory.",
                        args.path
                    ),
                    is_error: true,
                    artifacts: None,
                });
            }

            // Read text files directly; for supported binary docs, parse and extract text.
            let raw = read_file_content(&canonical)?;

            // Skip to start_line (1-based) and truncate to max_lines.
            let start = args.start_line.max(1);
            let max = args.max_lines.max(1);
            let total_lines = raw.lines().count();
            let lines: Vec<&str> = raw.lines().skip(start - 1).take(max).collect();
            let showing_end = (start - 1 + lines.len()).min(total_lines);
            let truncated = showing_end < total_lines || start > 1;
            let content = lines.join("\n");

            // Apply privacy redaction.
            let privacy_config = db.load_privacy_config().unwrap_or_default();
            let redacted = if privacy_config.enabled {
                privacy::redact_content(&content, &privacy_config.redact_patterns)
            } else {
                content
            };

            let mut text = format!("File: {}\n", canonical.display());
            if truncated {
                text.push_str(&format!(
                    "(showing lines {start}–{showing_end} of {total_lines})\n"
                ));
            }
            text.push_str("---\n");
            text.push_str(&redacted);

            Ok(ToolResult {
                call_id,
                content: text,
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
    async fn read_file_falls_back_to_document_parser_for_binary_images() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let image_path = dir.path().join("diagram.png");
        std::fs::write(&image_path, [0_u8, 159, 1, 2, 3]).expect("write binary image bytes");

        let db = setup_db_with_source(dir.path());
        let tool = FileTool;
        let args = serde_json::json!({
            "path": image_path.to_string_lossy().to_string()
        })
        .to_string();

        let result = tool
            .execute("call-1", &args, &db, &[])
            .await
            .expect("read_file should fallback for image");

        assert!(!result.is_error);
        assert!(
            result.content.contains("[Image: diagram.png]"),
            "unexpected content: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn read_file_keeps_binary_error_for_unsupported_types() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let bin_path = dir.path().join("payload.bin");
        std::fs::write(&bin_path, [0_u8, 1, 2, 3]).expect("write binary payload");

        let db = setup_db_with_source(dir.path());
        let tool = FileTool;
        let args = serde_json::json!({
            "path": bin_path.to_string_lossy().to_string()
        })
        .to_string();

        let err = tool
            .execute("call-2", &args, &db, &[])
            .await
            .expect_err("unsupported binary should still error");

        match err {
            CoreError::Parse(msg) => {
                assert!(msg.contains("File appears to be binary"), "msg was: {msg}");
            }
            other => panic!("expected parse error, got: {other:?}"),
        }
    }
}
