//! MultiEditTool - atomic multi-replacement edits for one text file.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;
use crate::file_checkpoint::{checkpoint_artifact, CreateFileCheckpointInput};

use super::diff_stats::{changed_line_count, diff_stats_artifact};
use super::document_utils::{edit_guidance_for_path, is_binary_file_error};
use super::path_utils::resolve_existing_file_for_file_access;
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/multi_edit.json");

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
const MAX_EDITS: usize = 20;
const MAX_PREVIEW_CHARS: usize = 120;

#[derive(Deserialize)]
struct MultiEditArgs {
    path: String,
    edits: Vec<MultiEditOperation>,
}

#[derive(Deserialize)]
struct MultiEditOperation {
    #[serde(default, alias = "old_string")]
    old_str: Option<String>,
    #[serde(default, alias = "new_string", alias = "content")]
    new_str: Option<String>,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedEditSummary {
    index: usize,
    replacements: usize,
    old_preview: String,
    new_preview: String,
}

pub struct MultiEditTool;

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        "multi_edit"
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
        let edit_count = args
            .get("edits")
            .and_then(|v| v.as_array())
            .map(|items| items.len())
            .unwrap_or(0);
        Some(format!("Apply {edit_count} text edit(s) to: {path}"))
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

impl MultiEditTool {
    async fn execute_impl(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        let args: MultiEditArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid multi_edit arguments: {e}")))?;

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

            if args.edits.is_empty() {
                return Ok(error_result(
                    &call_id,
                    "multi_edit requires at least one edit.",
                ));
            }
            if args.edits.len() > MAX_EDITS {
                return Ok(error_result(
                    &call_id,
                    &format!("multi_edit supports at most {MAX_EDITS} edits per call."),
                ));
            }

            let canonical = resolve_existing_file_for_file_access(
                &requested,
                &file_policy.sources,
                file_policy.allow_unregistered_absolute_paths,
            )
            .map_err(CoreError::InvalidInput)?;

            let original = match read_text_utf8(&canonical) {
                Ok(content) => content,
                Err(message) => {
                    return Ok(ToolResult {
                        call_id,
                        content: message,
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

            let mut content = original.clone();
            let mut summaries = Vec::new();
            let mut total_replacements = 0usize;
            let mut total_additions = 0usize;
            let mut total_deletions = 0usize;

            for (idx, edit) in args.edits.iter().enumerate() {
                let old_str = edit.old_str.as_deref().unwrap_or("");
                if old_str.is_empty() {
                    return Ok(error_result(
                        &call_id,
                        &format!("Edit {} old_str must be non-empty.", idx + 1),
                    ));
                }
                let new_str = edit.new_str.as_deref().unwrap_or("");
                let applied = match apply_one_edit(&content, edit) {
                    Ok(applied) => applied,
                    Err(message) => {
                        return Ok(error_result(
                            &call_id,
                            &format!("Edit {} failed: {message}", idx + 1),
                        ));
                    }
                };
                content = applied.content;
                total_replacements += applied.replacements;
                total_deletions += changed_line_count(old_str) * applied.replacements;
                total_additions += changed_line_count(new_str) * applied.replacements;
                summaries.push(AppliedEditSummary {
                    index: idx + 1,
                    replacements: applied.replacements,
                    old_preview: preview(old_str),
                    new_preview: preview(new_str),
                });
            }

            if content == original {
                return Ok(error_result(
                    &call_id,
                    "multi_edit would not change the file.",
                ));
            }

            let checkpoint = db.create_file_checkpoint(CreateFileCheckpointInput {
                conversation_id: conversation_id.as_deref(),
                tool_call_id: &call_id,
                tool_name: "multi_edit",
                operation: "multi_edit",
                path: &args.path,
                absolute_path: &canonical,
            })?;

            if let Err(e) = std::fs::write(&canonical, &content) {
                return Ok(ToolResult {
                    call_id,
                    content: format!("Failed to write '{}': {e}", args.path),
                    is_error: true,
                    artifacts: None,
                });
            }

            let mut artifact = checkpoint_artifact(&checkpoint, Some(content.len() as u64));
            if let Some(object) = artifact.as_object_mut() {
                object.insert("operation".to_string(), json!("multi_edit"));
                object.insert("editCount".to_string(), json!(summaries.len()));
                object.insert("replacementCount".to_string(), json!(total_replacements));
                object.insert(
                    "diffStats".to_string(),
                    diff_stats_artifact(
                        &args.path,
                        "multi_edit",
                        total_additions,
                        total_deletions,
                        summaries.len(),
                        Some(total_replacements),
                    ),
                );
                object.insert("edits".to_string(), json!(summaries));
            }

            Ok(ToolResult {
                call_id,
                content: format!(
                    "Applied {} edit(s), {} replacement(s), to '{}'.\nCheckpoint: {}",
                    args.edits.len(),
                    total_replacements,
                    args.path,
                    checkpoint.id
                ),
                is_error: false,
                artifacts: Some(artifact),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

struct AppliedEdit {
    content: String,
    replacements: usize,
}

fn apply_one_edit(content: &str, edit: &MultiEditOperation) -> Result<AppliedEdit, String> {
    let old_str = edit.old_str.as_deref().unwrap_or("");
    let new_str = edit.new_str.as_deref().unwrap_or("");
    let (range_start, range_end) = line_range_bounds(content, edit.start_line, edit.end_line)?;
    let before = &content[..range_start];
    let segment = &content[range_start..range_end];
    let after = &content[range_end..];

    let (search, replacement) = choose_line_ending_match(segment, old_str, new_str);
    let match_count = segment.match_indices(&search).count();
    if match_count == 0 {
        return Err("old_str not found.".to_string());
    }
    if match_count > 1 && !edit.replace_all {
        return Err(format!(
            "old_str found {match_count} times. Add more context, narrow with start_line/end_line, or set replace_all."
        ));
    }

    let changed_segment = if edit.replace_all {
        segment.replace(&search, &replacement)
    } else {
        segment.replacen(&search, &replacement, 1)
    };

    Ok(AppliedEdit {
        content: format!("{before}{changed_segment}{after}"),
        replacements: if edit.replace_all { match_count } else { 1 },
    })
}

fn choose_line_ending_match(segment: &str, old_str: &str, new_str: &str) -> (String, String) {
    if segment.contains(old_str) || !old_str.contains('\n') {
        return (old_str.to_string(), new_str.to_string());
    }

    let old_crlf = old_str.replace('\n', "\r\n");
    if segment.contains(&old_crlf) {
        (old_crlf, new_str.replace('\n', "\r\n"))
    } else {
        (old_str.to_string(), new_str.to_string())
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
        return Err("line ranges are 1-based; start_line/end_line must be >= 1.".to_string());
    }
    if start > end {
        return Err("start_line must be <= end_line.".to_string());
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

fn preview(text: &str) -> String {
    let mut out = text.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
    if text.chars().count() > MAX_PREVIEW_CHARS {
        out.push_str("...");
    }
    out
}

fn error_result(call_id: &str, content: &str) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: content.to_string(),
        is_error: true,
        artifacts: None,
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
    async fn multi_edit_applies_ordered_edits_and_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "title: old\nstatus: draft\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = MultiEditTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "edits": [
                { "old_str": "title: old", "new_str": "title: new" },
                { "old_str": "status: draft", "new_str": "status: reviewed" }
            ]
        });

        let result = tool
            .execute("multi-1", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "title: new\nstatus: reviewed\n"
        );
        assert_eq!(result.artifacts.as_ref().unwrap()["editCount"], 2);
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diffStats"]["kind"],
            "diffStats"
        );
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diffStats"]["operation"],
            "multi_edit"
        );
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diffStats"]["additions"],
            2
        );
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diffStats"]["deletions"],
            2
        );
        assert_eq!(result.artifacts.as_ref().unwrap()["diffStats"]["hunks"], 2);
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diffStats"]["replacements"],
            2
        );

        let checkpoint_id = result.artifacts.as_ref().unwrap()["checkpoint"]["id"]
            .as_str()
            .unwrap();
        db.restore_file_checkpoint(checkpoint_id).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "title: old\nstatus: draft\n"
        );
    }

    #[tokio::test]
    async fn multi_edit_is_atomic_when_later_edit_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "alpha\nbeta\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = MultiEditTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "edits": [
                { "old_str": "alpha", "new_str": "ALPHA" },
                { "old_str": "missing", "new_str": "MISSING" }
            ]
        });

        let result = tool
            .execute("multi-2", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Edit 2 failed"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
    }

    #[tokio::test]
    async fn multi_edit_requires_unique_match_unless_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "todo\ntodo\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = MultiEditTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "edits": [{ "old_str": "todo", "new_str": "done" }]
        });

        let result = tool
            .execute("multi-3", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("found 2 times"));

        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "edits": [{ "old_str": "todo", "new_str": "done", "replace_all": true }]
        });
        let result = tool
            .execute("multi-4", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "done\ndone\n");
    }

    #[tokio::test]
    async fn multi_edit_tolerates_crlf_line_endings_in_old_string() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "alpha\r\nbeta\r\ngamma\r\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = MultiEditTool;
        let args = serde_json::json!({
            "path": file.to_string_lossy(),
            "edits": [{ "old_str": "alpha\nbeta", "new_str": "delta" }]
        });

        let result = tool
            .execute("multi-crlf", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "delta\r\ngamma\r\n"
        );
    }
}
