//! EditFileTool — edits or creates files within managed source directories.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

#[cfg(test)]
use crate::db::Database;
use crate::error::CoreError;
use crate::file_checkpoint::{checkpoint_artifact, CreateFileCheckpointInput};

use super::create_file_tool::resolve_and_validate;
use super::diff_stats::diff_stats_from_diff;
use super::document_utils::{
    edit_guidance_for_path, generated_document_mime, is_binary_file_error,
};
use super::text_match::{find_text_matches, TextMatch};
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/edit_file.json");

/// Maximum file size we will read (10 MB). Prevents OOM on huge files.
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
const DIFF_CONTEXT_LINES: usize = 3;
const MAX_CREATE_DIFF_LINES: usize = 400;

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

fn text_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.lines().collect()
    }
}

fn line_index_at_start(content: &str, byte_offset: usize) -> usize {
    let end = byte_offset.min(content.len());
    content.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

fn line_index_at_end_exclusive(content: &str, end_offset: usize) -> usize {
    if content.is_empty() || end_offset == 0 {
        return 0;
    }
    let end = end_offset.min(content.len());
    let before_last_byte = end.saturating_sub(1);
    content.as_bytes()[..before_last_byte]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

fn checkpoint_artifact_with_diff(
    checkpoint: &crate::file_checkpoint::FileCheckpoint,
    bytes_after: Option<u64>,
    diff: serde_json::Value,
    replacements: Option<usize>,
) -> serde_json::Value {
    let mut artifact = checkpoint_artifact(checkpoint, bytes_after);
    if let Some(object) = artifact.as_object_mut() {
        object.insert(
            "diffStats".to_string(),
            diff_stats_from_diff(&diff, replacements),
        );
        object.insert("diff".to_string(), diff);
    }
    artifact
}

fn replacement_diff_artifact(
    path: &str,
    old_content: &str,
    new_content: &str,
    byte_offset: usize,
    matched_len: usize,
    inserted_len: usize,
) -> serde_json::Value {
    let old_lines = text_lines(old_content);
    let new_lines = text_lines(new_content);

    let old_start_idx = line_index_at_start(old_content, byte_offset);
    let old_end_idx = if old_lines.is_empty() {
        0
    } else {
        line_index_at_end_exclusive(old_content, byte_offset + matched_len)
            .min(old_lines.len().saturating_sub(1))
    };
    let old_start_idx = old_start_idx.min(old_lines.len().saturating_sub(1));
    let old_changed_count = if old_lines.is_empty() {
        0
    } else {
        old_end_idx.saturating_sub(old_start_idx) + 1
    };

    let new_start_idx = line_index_at_start(new_content, byte_offset);
    let new_changed_count = if inserted_len == 0 && new_content.is_empty() {
        0
    } else if inserted_len == 0 {
        usize::from(!new_lines.is_empty())
    } else if new_lines.is_empty() {
        0
    } else {
        let new_end_idx = line_index_at_end_exclusive(new_content, byte_offset + inserted_len)
            .min(new_lines.len().saturating_sub(1));
        new_end_idx.saturating_sub(new_start_idx.min(new_lines.len().saturating_sub(1))) + 1
    };
    let new_start_idx = new_start_idx.min(new_lines.len().saturating_sub(1));

    let context_from = old_start_idx.saturating_sub(DIFF_CONTEXT_LINES);
    let before_count = old_start_idx.saturating_sub(context_from);
    let after_old_start = old_start_idx.saturating_add(old_changed_count);
    let after_new_start = new_start_idx.saturating_add(new_changed_count);
    let after_count = old_lines
        .len()
        .saturating_sub(after_old_start)
        .min(new_lines.len().saturating_sub(after_new_start))
        .min(DIFF_CONTEXT_LINES);

    let mut lines = Vec::new();
    for idx in context_from..old_start_idx {
        if let Some(content) = old_lines.get(idx) {
            lines.push(serde_json::json!({
                "type": "context",
                "oldLine": idx + 1,
                "newLine": idx + 1,
                "content": content,
            }));
        }
    }

    for idx in old_start_idx..old_start_idx.saturating_add(old_changed_count) {
        if let Some(content) = old_lines.get(idx) {
            lines.push(serde_json::json!({
                "type": "deletion",
                "oldLine": idx + 1,
                "newLine": null,
                "content": content,
            }));
        }
    }

    for idx in new_start_idx..new_start_idx.saturating_add(new_changed_count) {
        if let Some(content) = new_lines.get(idx) {
            lines.push(serde_json::json!({
                "type": "addition",
                "oldLine": null,
                "newLine": idx + 1,
                "content": content,
            }));
        }
    }

    for offset in 0..after_count {
        let old_idx = after_old_start + offset;
        let new_idx = after_new_start + offset;
        if let Some(content) = new_lines.get(new_idx) {
            lines.push(serde_json::json!({
                "type": "context",
                "oldLine": old_idx + 1,
                "newLine": new_idx + 1,
                "content": content,
            }));
        }
    }

    serde_json::json!({
        "path": path,
        "operation": "str_replace",
        "additions": new_changed_count,
        "deletions": old_changed_count,
        "hunks": [{
            "oldStart": context_from + 1,
            "newStart": context_from + 1,
            "oldLines": before_count + old_changed_count + after_count,
            "newLines": before_count + new_changed_count + after_count,
            "lines": lines,
        }]
    })
}

fn create_diff_artifact(path: &str, file_content: &str) -> serde_json::Value {
    let all_lines = text_lines(file_content);
    let displayed_count = all_lines.len().min(MAX_CREATE_DIFF_LINES);
    let lines: Vec<serde_json::Value> = all_lines
        .iter()
        .take(displayed_count)
        .enumerate()
        .map(|(idx, content)| {
            serde_json::json!({
                "type": "addition",
                "oldLine": null,
                "newLine": idx + 1,
                "content": content,
            })
        })
        .collect();

    serde_json::json!({
        "path": path,
        "operation": "create",
        "additions": all_lines.len(),
        "deletions": 0,
        "truncated": displayed_count < all_lines.len(),
        "omittedLineCount": all_lines.len().saturating_sub(displayed_count),
        "hunks": [{
            "oldStart": 0,
            "newStart": 1,
            "oldLines": 0,
            "newLines": all_lines.len(),
            "lines": lines,
        }]
    })
}

fn find_replacement_matches(
    content: &str,
    old_str: &str,
    start_byte: usize,
    end_byte: usize,
) -> Vec<TextMatch> {
    let search_area = &content[start_byte..end_byte];
    find_text_matches(search_area, old_str)
        .into_iter()
        .map(|mut matched| {
            matched.start += start_byte;
            matched
        })
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
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let file_changes = crate::turn_file_changes::FileChangeScope::from_context(&context);
        let mutation_cancel = context
            .cancel_token
            .map(tokio_util::sync::CancellationToken::child_token)
            .unwrap_or_default();
        let _cancel_mutation_on_drop = mutation_cancel.clone().drop_guard();
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id,
            ..
        } = context;
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
                                .unwrap_or_else(|| "Use office_artifact for DOCX/XLSX/PPTX creation and edits; use run_shell + doc-script-editor for PDF, conversion/rendering, or OOXML compatibility work. Pair it with the matching format skill.".to_string()),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let _mutation = crate::file_mutation::lock_file_mutation(&canonical, Some(&mutation_cancel))?;
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

                    let matched = &matches[0];
                    let byte_offset = matched.start;
                    let matched_len = matched.len;
                    let original = &content[byte_offset..byte_offset + matched_len];
                    let replacement = matched.replacement_text(original, new_str);
                    let new_content = format!(
                        "{}{}{}",
                        &content[..byte_offset],
                        replacement,
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

                    if mutation_cancel.is_cancelled() { return Err(CoreError::InvalidInput("File mutation cancelled before writing".into())); }
                    if let Err(e) = std::fs::write(&canonical, &new_content) {
                        return Ok(ToolResult {
                            call_id,
                            content: format!("Failed to write '{}': {e}", args.path),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    if let Some(scope) = &file_changes { scope.record_checkpoint(&checkpoint, new_content.as_bytes()); }
                    let snippet = snippet_around(&new_content, byte_offset, replacement.len());
                    let diff = replacement_diff_artifact(
                        &args.path,
                        &content,
                        &new_content,
                        byte_offset,
                        matched_len,
                        replacement.len(),
                    );
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "Successfully replaced text in '{}'.\nCheckpoint: {}\n\nContext around edit:\n{}",
                            args.path, checkpoint.id, snippet
                        ),
                        is_error: false,
                        artifacts: Some(checkpoint_artifact_with_diff(
                            &checkpoint,
                            Some(new_content.len() as u64),
                            diff,
                            Some(1),
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
                                .unwrap_or_else(|| "Use office_artifact for DOCX/XLSX/PPTX creation and edits; use run_shell + doc-script-editor for PDF, conversion/rendering, or OOXML compatibility work. Pair it with the matching format skill.".to_string()),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    let _mutation = crate::file_mutation::lock_file_mutation(&canonical, Some(&mutation_cancel))?;
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

                    if mutation_cancel.is_cancelled() { return Err(CoreError::InvalidInput("File mutation cancelled before writing".into())); }
                    if let Err(e) = std::fs::write(&canonical, file_content) {
                        return Ok(ToolResult {
                            call_id,
                            content: format!("Failed to write '{}': {e}", args.path),
                            is_error: true,
                            artifacts: None,
                        });
                    }

                    if let Some(scope) = &file_changes { scope.record_checkpoint(&checkpoint, file_content.as_bytes()); }
                    let size = file_content.len();
                    let diff = create_diff_artifact(&args.path, file_content);
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "Created file '{}' ({} bytes).\nCheckpoint: {}",
                            args.path, size, checkpoint.id
                        ),
                        is_error: false,
                        artifacts: Some(checkpoint_artifact_with_diff(
                            &checkpoint,
                            Some(size as u64),
                            diff,
                            Some(0),
                        )),
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c1",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Successfully replaced"));
        let artifact = result.artifacts.as_ref().unwrap();
        assert_eq!(artifact["diff"]["path"], file.to_string_lossy().as_ref());
        assert_eq!(artifact["diff"]["operation"], "str_replace");
        assert_eq!(artifact["diff"]["additions"], 1);
        assert_eq!(artifact["diff"]["deletions"], 1);
        assert_eq!(artifact["diff"]["hunks"][0]["lines"][0]["type"], "deletion");
        assert_eq!(artifact["diff"]["hunks"][0]["lines"][1]["type"], "addition");
        assert_eq!(artifact["diffStats"]["kind"], "diffStats");
        assert_eq!(artifact["diffStats"]["filesChanged"], 1);
        assert_eq!(artifact["diffStats"]["additions"], 1);
        assert_eq!(artifact["diffStats"]["deletions"], 1);
        assert_eq!(artifact["diffStats"]["hunks"], 1);
        assert_eq!(artifact["diffStats"]["replacements"], 1);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hi world\ngoodbye world\n");

        let checkpoint_id = artifact["checkpoint"]["id"].as_str().unwrap();
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-alias",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-replace",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-range",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-crlf",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "delta\r\ngamma\r\n"
        );
    }

    #[tokio::test]
    async fn test_str_replace_preserves_crlf_after_indentation_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(
            &file,
            "fn main() {\r\n    if ready {\r\n        run();\r\n    }\r\n}\r\n",
        )
        .unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "if ready {\n    run();\n}\n",
            "new_str": "if ready {\n    finish();\n}\n"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "c-crlf-indent",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            content,
            "fn main() {\r\n    if ready {\r\n        finish();\r\n    }\r\n}\r\n"
        );
        assert!(!content.replace("\r\n", "").contains('\n'));
    }

    #[tokio::test]
    async fn test_str_replace_tolerates_unicode_quote_variants_in_chinese_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("公主与恶龙")
            .join("正文")
            .join("第5章_我要出门了.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "她说：“我要出门了。”\n下一句。\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "她说:\"我要出门了。\"",
            "new_str": "她说：“我要去集市。”"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "c-unicode-quotes",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "她说：“我要去集市。”\n下一句。\n"
        );
    }

    #[tokio::test]
    async fn test_str_replace_tolerates_unicode_normalization_variants_across_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("لغات").join("日本語").join("한글.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "قال: «مرحبا»\nﾊﾟﾝを買う\nCafe\u{301}\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = EditFileTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "str_replace",
            "old_str": "قال: \"مرحبا\"\nパンを買う\nCafé",
            "new_str": "multi-script replacement"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "c-unicode-scripts",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "multi-script replacement\n"
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c2",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c3",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c4",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Created file"));
        let artifact = result.artifacts.as_ref().unwrap();
        assert_eq!(artifact["diff"]["operation"], "create");
        assert_eq!(artifact["diff"]["additions"], 2);
        assert_eq!(artifact["diff"]["deletions"], 0);
        assert_eq!(artifact["diffStats"]["operation"], "create");
        assert_eq!(artifact["diffStats"]["additions"], 2);
        assert_eq!(artifact["diffStats"]["deletions"], 0);
        assert_eq!(artifact["diffStats"]["replacements"], 0);

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
            .execute(crate::tools::ToolExecutionContext::new(
                "c5",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn1",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn-rel",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn2",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn3",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn-docx",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn4",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "cn6",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c6",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-open",
                &args.to_string(),
                &db,
                &[],
            ))
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c-allow-all",
                &args.to_string(),
                &db,
                &["unrelated-source-scope".to_string()],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    }
}
