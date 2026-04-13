//! EditFileTool — edits or creates files within managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::resolve_and_validate;
use super::document_utils::{
    edit_guidance_for_path, generated_document_mime, is_binary_file_error,
};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/edit_file.json");

/// Maximum file size we will read (10 MB). Prevents OOM on huge files.
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    action: String,
    old_str: Option<String>,
    new_str: Option<String>,
}

pub struct EditFileTool;

/// Try to read the file as UTF-8 text. Returns an error message if the file
/// appears to be binary (contains null bytes in the first 8 KB).
fn read_text_utf8(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read file: {e}"))?;
    if meta.len() > MAX_FILE_SIZE {
        return Err(format!(
            "File too large ({:.1} MB, limit is {} MB): {}",
            meta.len() as f64 / (1024.0 * 1024.0),
            MAX_FILE_SIZE / (1024 * 1024),
            path.display()
        ));
    }
    match crate::parse::read_text_file(path) {
        Ok(content) => Ok(content),
        Err(err) if is_binary_file_error(&err) => Err(edit_guidance_for_path(path)
            .unwrap_or_else(|| format!("File appears to be binary: {}", path.display()))),
        Err(err) => Err(err.to_string()),
    }
}

/// Return a few lines of context around the replacement site.
fn snippet_around(content: &str, byte_offset: usize, replacement_len: usize) -> String {
    let context_lines = 3;
    let lines: Vec<&str> = content.lines().collect();

    // Find the line containing the start of the replacement.
    let mut cumulative = 0usize;
    let mut start_line = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let line_end = cumulative + line.len() + 1; // +1 for newline
        if byte_offset < line_end {
            start_line = i;
            break;
        }
        cumulative = line_end;
    }

    // Find the line containing the end of the replacement.
    let end_byte = byte_offset + replacement_len;
    let mut end_line = start_line;
    cumulative = 0;
    for (i, line) in lines.iter().enumerate() {
        let line_end = cumulative + line.len() + 1;
        if end_byte <= line_end {
            end_line = i;
            break;
        }
        cumulative = line_end;
        end_line = i;
    }

    let from = start_line.saturating_sub(context_lines);
    let to = (end_line + context_lines + 1).min(lines.len());

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(to).skip(from) {
        out.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    out
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
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
        Some(format!("Edit file: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: EditFileArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid edit_file arguments: {e}")))?;

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

            match args.action.as_str() {
                "str_replace" => {
                    let old_str = match args.old_str.as_deref() {
                        Some(s) if !s.is_empty() => s,
                        _ => {
                            return Ok(ToolResult {
                                call_id: call_id.clone(),
                                content: "str_replace requires a non-empty 'old_str' parameter."
                                    .to_string(),
                                is_error: true,
                                artifacts: None,
                            });
                        }
                    };
                    let new_str = args.new_str.as_deref().unwrap_or("");

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

                    if !canonical.is_file() {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "File not found: '{}'",
                                args.path
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    if generated_document_mime(&canonical).is_some() {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: edit_guidance_for_path(&canonical)
                                .unwrap_or_else(|| "Use generate_document for Office files.".to_string()),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let content = match read_text_utf8(&canonical) {
                        Ok(c) => c,
                        Err(msg) => {
                            return Ok(ToolResult {
                                call_id: call_id.clone(),
                                content: msg,
                                is_error: true,
                                artifacts: None,
                            });
                        }
                    };

                    // Count occurrences of old_str.
                    let matches: Vec<_> = content.match_indices(old_str).collect();

                    if matches.is_empty() {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "old_str not found in '{}'. Make sure the string matches exactly, including whitespace and newlines.",
                                args.path
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    if matches.len() > 1 {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "old_str found {} times in '{}'. It must match exactly once. Include more surrounding context to make it unique.",
                                matches.len(),
                                args.path
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let byte_offset = matches[0].0;
                    let new_content = format!(
                        "{}{}{}",
                        &content[..byte_offset],
                        new_str,
                        &content[byte_offset + old_str.len()..]
                    );

                    if let Err(e) = std::fs::write(&canonical, &new_content) {
                        return Ok(ToolResult {
                            call_id,
                            content: format!("Failed to write '{}': {e}", args.path),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let snippet = snippet_around(&new_content, byte_offset, new_str.len());
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "Successfully replaced text in '{}'.\n\nContext around edit:\n{}",
                            args.path, snippet
                        ),
                        is_error: false,
                        artifacts: None,
                    })
                }

                "create" => {
                    let file_content = args.new_str.as_deref().unwrap_or("");

                    let canonical = match resolve_and_validate(&requested, &sources) {
                        Ok(p) => {
                            // File path resolved — check it doesn't already exist.
                            if p.exists() {
                                return Ok(ToolResult {
                                    call_id: call_id.clone(),
                                    content: format!(
                                        "File already exists: '{}'. Use str_replace to edit it instead.",
                                        args.path
                                    ),
                                    is_error: true,
                                    artifacts: None,
                                });
                            }
                            p
                        }
                        Err(msg) => {
                            // For new files the parent might exist but the file doesn't yet.
                            // resolve_and_validate already handles this, so propagate the error.
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
                                .unwrap_or_else(|| "Use generate_document for Office files.".to_string()),
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

                    if let Err(e) = std::fs::write(&canonical, file_content) {
                        return Ok(ToolResult {
                            call_id,
                            content: format!("Failed to write '{}': {e}", args.path),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let size = file_content.len();
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "Created file '{}' ({} bytes).",
                            args.path, size
                        ),
                        is_error: false,
                        artifacts: None,
                    })
                }

                other => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Unknown action '{}'. Must be 'str_replace' or 'create'.",
                        other
                    ),
                    is_error: true,
                    artifacts: None,
                }),
            }
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
    async fn test_str_replace_success() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\ngoodbye world\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "hello world",
            "new_str": "hi world"
        });

        let result = tool
            .execute("c1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Successfully replaced"));

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hi world\ngoodbye world\n");
    }

    #[tokio::test]
    async fn test_str_replace_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "does not exist",
            "new_str": "replacement"
        });

        let result = tool
            .execute("c2", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("old_str not found"));
    }

    #[tokio::test]
    async fn test_str_replace_multiple_matches() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "aaa\naaa\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "aaa",
            "new_str": "bbb"
        });

        let result = tool
            .execute("c3", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("found 2 times"));
    }

    #[tokio::test]
    async fn test_create_success() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new_file.md");

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "create",
            "new_str": "# New File\nContent here."
        });

        let result = tool
            .execute("c4", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Created file"));

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "# New File\nContent here.");
    }

    #[tokio::test]
    async fn test_create_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("existing.txt");
        std::fs::write(&file, "existing content").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "create",
            "new_str": "new content"
        });

        let result = tool
            .execute("c5", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_nested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("deep").join("file.txt");

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "create",
            "new_str": "nested content"
        });

        let result = tool
            .execute("cn1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Created file"));

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "nested content");
    }

    #[tokio::test]
    async fn test_create_resolves_source_relative_path() {
        let dir = tempfile::tempdir().unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": "notes/checklist.md",
            "action": "create",
            "new_str": "- one\n- two"
        });

        let result = tool
            .execute("cn-rel", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes").join("checklist.md")).unwrap(),
            "- one\n- two"
        );
    }

    #[tokio::test]
    async fn test_empty_old_str() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "",
            "new_str": "world"
        });

        let result = tool
            .execute("cn2", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("non-empty"));
    }

    #[tokio::test]
    async fn test_binary_file_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        // Write bytes containing nulls to simulate a binary file.
        std::fs::write(&file, b"hello\x00world").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "hello",
            "new_str": "bye"
        });

        let result = tool
            .execute("cn3", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("binary"));
    }

    #[tokio::test]
    async fn test_edit_file_guides_office_document_updates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("status.docx");
        std::fs::write(&file, b"PK\x03\x04placeholder").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "placeholder",
            "new_str": "updated"
        });

        let result = tool
            .execute("cn-docx", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("generate_document"));
    }

    #[tokio::test]
    async fn test_invalid_action() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "delete_file"
        });

        let result = tool
            .execute("cn4", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_file_too_large() {
        // Verify the MAX_FILE_SIZE constant and the size check in read_text_utf8.
        assert_eq!(MAX_FILE_SIZE, 10 * 1024 * 1024);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("small.txt");
        std::fs::write(&file, "small").unwrap();
        // A small file should pass the size check.
        assert!(read_text_utf8(&file).is_ok());
    }

    #[tokio::test]
    async fn test_str_replace_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello cruel world\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": " cruel",
            "new_str": ""
        });

        let result = tool
            .execute("cn6", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[tokio::test]
    async fn test_path_outside_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let file = other_dir.path().join("secret.txt");
        std::fs::write(&file, "secret").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "secret",
            "new_str": "hacked"
        });

        let result = tool
            .execute("c6", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Access denied"));

        // Verify file was not modified.
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "secret");
    }
}
