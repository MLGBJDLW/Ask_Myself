//! EditFileTool — edits or creates files within managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::file_checkpoint::{checkpoint_artifact, CreateFileCheckpointInput};

use super::create_file_tool::resolve_and_validate;
use super::document_utils::{
    edit_guidance_for_path, generated_document_mime, is_binary_file_error,
};
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/edit_file.json");

/// Maximum file size we will read (10 MB). Prevents OOM on huge files.
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default, alias = "old_string")]
    old_str: Option<String>,
    #[serde(default, alias = "new_string", alias = "content")]
    new_str: Option<String>,
    /// Optional 1-based inclusive line range limiting where replacement is searched.
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
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

fn normalized_action(args: &EditFileArgs) -> &str {
    match args.action.as_deref() {
        Some("replace") | Some("str_replace") => "str_replace",
        Some("create") => "create",
        Some(other) => other,
        None if args.old_str.as_ref().is_some_and(|s| !s.is_empty()) => "str_replace",
        None => "create",
    }
}

fn line_range_bounds(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<(usize, usize), String> {
    if start_line.is_none() && end_line.is_none() {
        return Ok((0, content.len()));
    }

    let mut line_starts = vec![0usize];
    for (idx, byte) in content.bytes().enumerate() {
        if byte == b'\n' && idx + 1 < content.len() {
            line_starts.push(idx + 1);
        }
    }

    let line_count = if content.is_empty() {
        1
    } else {
        line_starts.len()
    };
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(line_count);

    if start == 0 || end == 0 {
        return Err("start_line and end_line are 1-based; use values >= 1.".to_string());
    }
    if start > end {
        return Err("start_line must be less than or equal to end_line.".to_string());
    }
    if start > line_count {
        return Err(format!(
            "start_line {start} is beyond the file length ({line_count} lines)."
        ));
    }

    let start_byte = line_starts[start - 1];
    let end_byte = if end >= line_count {
        content.len()
    } else {
        line_starts[end]
    };
    Ok((start_byte, end_byte))
}

fn normalize_line_endings_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        if bytes[idx] == b'\r' && idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
            normalized.push('\n');
            byte_map.push(idx);
            idx += 2;
            continue;
        }

        let ch = input[idx..]
            .chars()
            .next()
            .expect("idx should always be on a char boundary");
        normalized.push(ch);
        byte_map.push(idx);
        idx += ch.len_utf8();
    }

    (normalized, byte_map)
}

fn find_line_ending_normalized_matches(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let (normalized_haystack, haystack_map) = normalize_line_endings_with_map(haystack);
    let (normalized_needle, _) = normalize_line_endings_with_map(needle);
    if normalized_needle.is_empty() || normalized_haystack == haystack {
        return Vec::new();
    }

    normalized_haystack
        .match_indices(&normalized_needle)
        .filter_map(|(start, matched)| {
            let end = start + matched.len();
            let original_start = *haystack_map.get(start)?;
            let original_end = haystack_map.get(end).copied().unwrap_or(haystack.len());
            Some((original_start, original_end.saturating_sub(original_start)))
        })
        .collect()
}

fn find_replacement_matches(
    content: &str,
    old_str: &str,
    start_byte: usize,
    end_byte: usize,
) -> Vec<(usize, usize)> {
    let search_area = &content[start_byte..end_byte];
    let exact: Vec<(usize, usize)> = search_area
        .match_indices(old_str)
        .map(|(offset, matched)| (start_byte + offset, matched.len()))
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    find_line_ending_normalized_matches(search_area, old_str)
        .into_iter()
        .map(|(offset, len)| (start_byte + offset, len))
        .collect()
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
        &[ToolCategory::FileSystem]
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
        self.execute_impl(call_id, arguments, db, source_scope, None)
            .await
    }

    async fn execute_with_context(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        self.execute_impl(call_id, arguments, db, source_scope, conversation_id)
            .await
    }
}

impl EditFileTool {
    async fn execute_impl(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        let args: EditFileArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid edit_file arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        let conversation_id = conversation_id.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let file_policy = file_access_policy(&db, &source_scope)?;
            let requested = PathBuf::from(&args.path);
            if file_policy.sources.is_empty()
                && !(file_policy.allow_unregistered_absolute_paths && requested.is_absolute())
            {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            match normalized_action(&args) {
                "str_replace" => {
                    let old_str = match args.old_str.as_deref() {
                        Some(s) if !s.is_empty() => s,
                        _ => {
                            return Ok(ToolResult {
                                call_id: call_id.clone(),
                                content: "str_replace requires a non-empty 'old_str' parameter. The alias 'old_string' is also accepted."
                                    .to_string(),
                                is_error: true,
                                artifacts: None,
                            });
                        }
                    };
                    let new_str = args.new_str.as_deref().unwrap_or("");

                    let canonical = match resolve_and_validate(
                        &requested,
                        &file_policy.sources,
                        file_policy.allow_unregistered_absolute_paths,
                    ) {
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
                                .unwrap_or_else(|| "Use run_shell + doc-script-editor for Office/PDF creation, validation, conversion, rendering, template work, and rich edits. For Office files, pair it with docx-document-design, pptx-presentation-design, or xlsx-workbook-design as appropriate.".to_string()),
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

                    let (search_start, search_end) = match line_range_bounds(
                        &content,
                        args.start_line,
                        args.end_line,
                    ) {
                        Ok(range) => range,
                        Err(msg) => {
                            return Ok(ToolResult {
                                call_id: call_id.clone(),
                                content: msg,
                                is_error: true,
                                artifacts: None,
                            });
                        }
                    };

                    // Count occurrences of old_str within the requested line range.
                    let matches = find_replacement_matches(
                        &content,
                        old_str,
                        search_start,
                        search_end,
                    );

                    if matches.is_empty() {
                        let range_hint = match (args.start_line, args.end_line) {
                            (None, None) => String::new(),
                            (start, end) => format!(
                                " within lines {}..{}",
                                start
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| "1".to_string()),
                                end.map(|n| n.to_string())
                                    .unwrap_or_else(|| "end".to_string())
                            ),
                        };
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "old_str not found in '{}'{}. Make sure the string matches exactly, including whitespace. Accepted aliases: old_string/new_string; action 'replace' is treated as 'str_replace'.",
                                args.path,
                                range_hint
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    if matches.len() > 1 {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "old_str found {} times in '{}'. It must match exactly once. Include more surrounding context or pass start_line/end_line to narrow the replacement.",
                                matches.len(),
                                args.path
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let (byte_offset, matched_len) = matches[0];
                    let new_content = format!(
                        "{}{}{}",
                        &content[..byte_offset],
                        new_str,
                        &content[byte_offset + matched_len..]
                    );

                    let checkpoint = db.create_file_checkpoint(CreateFileCheckpointInput {
                        conversation_id: conversation_id.as_deref(),
                        tool_call_id: &call_id,
                        tool_name: "edit_file",
                        operation: "str_replace",
                        path: &args.path,
                        absolute_path: &canonical,
                    })?;

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
                            "Successfully replaced text in '{}'.\nCheckpoint: {}\n\nContext around edit:\n{}",
                            args.path, checkpoint.id, snippet
                        ),
                        is_error: false,
                        artifacts: Some(checkpoint_artifact(
                            &checkpoint,
                            Some(new_content.len() as u64),
                        )),
                    })
                }

                "create" => {
                    let file_content = args.new_str.as_deref().unwrap_or("");

                    let canonical = match resolve_and_validate(
                        &requested,
                        &file_policy.sources,
                        file_policy.allow_unregistered_absolute_paths,
                    ) {
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
                                .unwrap_or_else(|| "Use run_shell + doc-script-editor for Office/PDF creation, validation, conversion, rendering, template work, and rich edits. For Office files, pair it with docx-document-design, pptx-presentation-design, or xlsx-workbook-design as appropriate.".to_string()),
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

                    let checkpoint = db.create_file_checkpoint(CreateFileCheckpointInput {
                        conversation_id: conversation_id.as_deref(),
                        tool_call_id: &call_id,
                        tool_name: "edit_file",
                        operation: "create",
                        path: &args.path,
                        absolute_path: &canonical,
                    })?;

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
                            "Created file '{}' ({} bytes).\nCheckpoint: {}",
                            args.path, size, checkpoint.id
                        ),
                        is_error: false,
                        artifacts: Some(checkpoint_artifact(&checkpoint, Some(size as u64))),
                    })
                }

                other => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Unknown action '{}'. Must be 'str_replace', 'replace', or 'create'.",
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
    use crate::app_settings::{AppConfig, ShellAccessMode};
    use crate::approval::ToolApprovalMode;
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

    fn enable_open_file_access(db: &Database) {
        let mut config = AppConfig::default();
        config.shell_access_mode = ShellAccessMode::Open;
        db.save_app_config(&config).expect("save app config");
    }

    fn enable_allow_all_tool_approval(db: &Database) {
        let mut config = AppConfig::default();
        config.tool_approval_mode = ToolApprovalMode::AllowAll;
        db.save_app_config(&config).expect("save app config");
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

        let checkpoint_id = result.artifacts.as_ref().unwrap()["checkpoint"]["id"]
            .as_str()
            .unwrap();
        db.restore_file_checkpoint(checkpoint_id).unwrap();
        let restored = std::fs::read_to_string(&file).unwrap();
        assert_eq!(restored, "hello world\ngoodbye world\n");
    }

    #[tokio::test]
    async fn test_str_replace_accepts_common_aliases_and_infers_action() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("overview.md");
        std::fs::write(&file, "## Document Index\n\nold row\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "old_string": "old row",
            "new_string": "new row"
        });

        let result = tool
            .execute("c-alias", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "## Document Index\n\nnew row\n"
        );
    }

    #[tokio::test]
    async fn test_str_replace_replace_action_alias() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "alpha beta\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "replace",
            "old_str": "alpha",
            "new_str": "omega"
        });

        let result = tool
            .execute("c-replace", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "omega beta\n");
    }

    #[tokio::test]
    async fn test_str_replace_can_narrow_by_line_range() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "target\nkeep\nsection\nkeep\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "keep",
            "new_str": "changed",
            "start_line": 4,
            "end_line": 4
        });

        let result = tool
            .execute("c-range", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "target\nkeep\nsection\nchanged\n"
        );
    }

    #[tokio::test]
    async fn test_str_replace_tolerates_crlf_line_endings_in_old_string() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "alpha\r\nbeta\r\ngamma\r\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "alpha\nbeta",
            "new_str": "delta"
        });

        let result = tool
            .execute("c-crlf", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "delta\r\ngamma\r\n"
        );
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
        assert!(result.content.contains("doc-script-editor"));
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

    #[tokio::test]
    async fn test_open_mode_allows_absolute_path_outside_source() {
        let dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let file = other_dir.path().join("secret.txt");
        std::fs::write(&file, "secret").unwrap();

        let db = setup_db_with_source(dir.path());
        enable_open_file_access(&db);
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "secret",
            "new_str": "updated"
        });

        let result = tool
            .execute("c-open", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated");
    }

    #[tokio::test]
    async fn test_allow_all_tool_approval_reads_all_registered_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "before").unwrap();

        let db = setup_db_with_source(dir.path());
        enable_allow_all_tool_approval(&db);
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "before",
            "new_str": "after"
        });

        let result = tool
            .execute(
                "c-allow-all",
                &args.to_string(),
                &db,
                &["unrelated-source-scope".to_string()],
            )
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    }
}
