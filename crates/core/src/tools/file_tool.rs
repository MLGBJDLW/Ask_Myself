//! FileTool — reads files from managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::privacy;

use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

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
            let parsed = crate::parse::parse_file(
                path,
                None,
                #[cfg(feature = "video")]
                None,
                None,
                None,
                None,
            )?;
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

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: FileArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid read_file arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let requested = PathBuf::from(&args.path);

            // Canonicalize the requested path so we can compare prefixes reliably.
            let canonical = std::fs::canonicalize(&requested).map_err(|e| {
                CoreError::InvalidInput(format!("Cannot resolve path '{}': {e}", args.path))
            })?;

            // Validate that the file is inside a registered source root.
            let sources = scoped_sources(&db, &source_scope)?;
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
                        "Access denied: '{}' is not within any directory available in the current source scope.",
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
            let canonical_str = canonical.to_string_lossy().to_string();
            let document_info: Option<(String, Option<String>)> = db
                .conn()
                .query_row(
                    "SELECT id, title FROM documents WHERE path = ?1",
                    rusqlite::params![&canonical_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            let file_label = canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            let suggested_citation = match &document_info {
                Some((document_id, _)) => format!("[doc:{document_id}|{file_label}]"),
                None => format!("[file:{canonical_str}:{start}-{showing_end}|{file_label}]"),
            };

            // Apply privacy redaction.
            let privacy_config = db.load_privacy_config().unwrap_or_default();
            let redacted = if privacy_config.enabled {
                privacy::redact_content(&content, &privacy_config.redact_patterns)
            } else {
                content
            };

            let mut text = format!("File: {}\n", canonical.display());
            if let Some((document_id, title)) = &document_info {
                let title_display = title.as_deref().unwrap_or("(untitled)");
                text.push_str(&format!("Document ID: {document_id}\n"));
                text.push_str(&format!("Title: {title_display}\n"));
            }
            text.push_str(&format!("Suggested citation: {suggested_citation}\n"));
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
                artifacts: Some(serde_json::json!({
                    "path": canonical_str,
                    "documentId": document_info.as_ref().map(|(id, _)| id.clone()),
                    "documentTitle": document_info.as_ref().and_then(|(_, title)| title.clone()),
                    "lineStart": start,
                    "lineEnd": showing_end,
                    "totalLines": total_lines,
                    "suggestedCitation": suggested_citation,
                })),
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

    #[tokio::test]
    async fn read_file_returns_suggested_document_citation_when_indexed() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file_path = dir.path().join("notes.md");
        std::fs::write(&file_path, "# Notes\nhello world\n").expect("write text file");
        let canonical_path = std::fs::canonicalize(&file_path).unwrap();

        let db = setup_db_with_source(dir.path());
        db.conn()
            .execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, (SELECT id FROM sources LIMIT 1), ?2, 'Notes', 'text/markdown', 20, '2025-01-01 00:00:00', 'hash-notes')",
                rusqlite::params!["doc-1", canonical_path.to_string_lossy().to_string()],
            )
            .unwrap();

        let tool = FileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy().to_string()
        })
        .to_string();

        let result = tool.execute("call-3", &args, &db, &[]).await.unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Document ID: doc-1"));
        assert!(result
            .content
            .contains("Suggested citation: [doc:doc-1|notes.md]"));
        assert_eq!(result.artifacts.unwrap()["documentId"], "doc-1");
    }

    #[tokio::test]
    async fn read_file_respects_source_scope() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file_path = dir.path().join("notes.md");
        std::fs::write(&file_path, "hello world\n").expect("write text file");

        let db = setup_db_with_source(dir.path());
        let tool = FileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy().to_string()
        })
        .to_string();

        let result = tool
            .execute("call-4", &args, &db, &["out-of-scope".to_string()])
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("current source scope"));
    }
}
