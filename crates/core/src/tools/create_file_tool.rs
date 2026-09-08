//! CreateFileTool — creates, overwrites, or incrementally appends plain-text files.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

#[cfg(test)]
use crate::db::Database;
use crate::error::CoreError;
use crate::file_checkpoint::CreateFileCheckpointInput;

use super::diff_stats::{
    checkpoint_artifact_with_diff, create_file_diff_artifact, text_diff_artifact,
};
use super::document_utils::{edit_guidance_for_path, generated_document_mime};
use super::path_utils::{
    has_path_traversal as has_path_traversal_impl, resolve_writable_file_for_file_access,
};
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/create_file.json");
const CREATE_FILE_PROTOCOL_VERSION: u16 = 2;

pub struct CreateFileTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileWriteMode {
    Create,
    Overwrite,
    Append,
}

impl FileWriteMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Overwrite => "overwrite",
            Self::Append => "append",
        }
    }
}

#[derive(Deserialize)]
struct CreateFileArgs {
    path: String,
    content: String,
    #[serde(default)]
    overwrite: bool,
    /// Explicit write mode.
    ///
    /// Compatibility lifecycle: introducedIn=0.1, deprecatedIn=0.10.23,
    /// removeIn=the first release after two consecutive minor versions report
    /// zero compatibility hits, migration=use mode="overwrite", owner=core-tools.
    #[serde(default)]
    mode: Option<String>,
    /// Required for append mode so retries and dependent chunks cannot silently
    /// duplicate or reorder content. This is the current UTF-8 byte length.
    #[serde(default, alias = "expectedBytes")]
    expected_bytes: Option<u64>,
}

fn normalized_mode(args: &CreateFileArgs) -> Result<FileWriteMode, String> {
    let requested = args
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(str::to_ascii_lowercase);

    if args.overwrite {
        if requested.as_deref().is_some_and(|mode| mode != "overwrite") {
            return Err(
                "Do not combine overwrite=true with mode='create' or mode='append'. Use mode='overwrite' instead."
                    .to_string(),
            );
        }
        return Ok(FileWriteMode::Overwrite);
    }

    match requested.as_deref().unwrap_or("create") {
        "create" => Ok(FileWriteMode::Create),
        "overwrite" => Ok(FileWriteMode::Overwrite),
        "append" => Ok(FileWriteMode::Append),
        other => Err(format!(
            "Invalid create_file mode '{other}'. Must be 'create', 'overwrite', or 'append'."
        )),
    }
}

/// Resolve the requested path against registered source roots and validate
/// that it falls within one of them. Returns the validated path.
/// For new files, walks up the ancestor chain to find the nearest existing
/// directory, canonicalizes it, then reconstructs the full path.
pub(crate) fn resolve_and_validate(
    requested: &Path,
    sources: &[crate::models::Source],
    allow_unregistered_absolute_paths: bool,
) -> Result<PathBuf, String> {
    resolve_writable_file_for_file_access(requested, sources, allow_unregistered_absolute_paths)
}

/// Reject paths containing traversal sequences.
pub(crate) fn has_path_traversal(path: &str) -> bool {
    has_path_traversal_impl(path)
}

fn error_result(call_id: &str, content: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: content.into(),
        is_error: true,
        artifacts: None,
    }
}

fn append_diff_artifact(path: &str, content: &str, base_bytes: u64) -> Value {
    let mut diff = create_file_diff_artifact(path, content);
    if let Some(map) = diff.as_object_mut() {
        map.insert("operation".to_string(), Value::String("append".to_string()));
        map.insert("baseBytes".to_string(), Value::Number(base_bytes.into()));
    }
    diff
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
        &[ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let path = args
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| {
                if args
                    .get("overwrite")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    "overwrite"
                } else {
                    "create"
                }
            });
        Some(format!("{} file: {path}", mode.to_ascii_uppercase()))
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let file_changes = crate::turn_file_changes::FileChangeScope::from_context(&context);
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id,
            ..
        } = context;
        let args: CreateFileArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid create_file arguments: {error}"))
        })?;
        tracing::info!(
            target: "nexa::tool_protocol",
            tool = "create_file",
            current_protocol_version = CREATE_FILE_PROTOCOL_VERSION,
            argument_protocol_version = if args.overwrite { 1 } else { 2 },
            compatibility_hit = args.overwrite,
            "tool protocol invocation"
        );
        let mode = normalized_mode(&args).map_err(CoreError::InvalidInput)?;

        if has_path_traversal(&args.path) {
            return Ok(error_result(
                call_id,
                "Path must not contain '..' traversal sequences.",
            ));
        }

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
                return Ok(error_result(
                    &call_id,
                    "No sources registered. Add a source directory first.",
                ));
            }

            let canonical = match resolve_and_validate(
                &requested,
                &file_policy.sources,
                file_policy.allow_unregistered_absolute_paths,
            ) {
                Ok(path) => path,
                Err(message) => return Ok(error_result(&call_id, message)),
            };

            if generated_document_mime(&canonical).is_some() {
                return Ok(error_result(
                    &call_id,
                    edit_guidance_for_path(&canonical).unwrap_or_else(|| {
                        "Use office_artifact for DOCX/XLSX/PPTX creation and edits; use run_shell + doc-script-editor for PDF, conversion/rendering, or OOXML compatibility work. Pair it with the matching format skill."
                            .to_string()
                    }),
                ));
            }

            let existed_before = canonical.exists();
            match mode {
                FileWriteMode::Create if existed_before => {
                    return Ok(error_result(
                        &call_id,
                        format!(
                            "File already exists: '{}'. Use mode='overwrite', mode='append', or edit_file to modify it.",
                            args.path
                        ),
                    ));
                }
                FileWriteMode::Append if !canonical.is_file() => {
                    return Ok(error_result(
                        &call_id,
                        format!(
                            "Cannot append because file does not exist: '{}'. Create the first chunk with mode='create'.",
                            args.path
                        ),
                    ));
                }
                _ => {}
            }

            if !matches!(mode, FileWriteMode::Append) {
                if let Some(parent) = canonical.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
                    }
                }
            }

            let bytes_before = if existed_before {
                std::fs::metadata(&canonical).map_err(CoreError::Io)?.len()
            } else {
                0
            };

            if matches!(mode, FileWriteMode::Append) {
                let Some(expected_bytes) = args.expected_bytes else {
                    return Ok(error_result(
                        &call_id,
                        format!(
                            "Append requires expected_bytes to prevent duplicate or out-of-order chunks. Current file size: {bytes_before} bytes."
                        ),
                    ));
                };
                if expected_bytes != bytes_before {
                    return Ok(error_result(
                        &call_id,
                        format!(
                            "Append precondition failed for '{}': expected {} bytes, but the file currently has {} bytes. Read or inspect the latest file state before retrying; do not blindly resend the chunk.",
                            args.path, expected_bytes, bytes_before
                        ),
                    ));
                }
            }

            let old_content = if existed_before && matches!(mode, FileWriteMode::Overwrite) {
                std::fs::read_to_string(&canonical).unwrap_or_default()
            } else {
                String::new()
            };

            let checkpoint = db.create_file_checkpoint(CreateFileCheckpointInput {
                conversation_id: conversation_id.as_deref(),
                tool_call_id: &call_id,
                tool_name: "create_file",
                operation: mode.as_str(),
                path: &args.path,
                absolute_path: &canonical,
            })?;

            let bytes_written = args.content.len() as u64;
            let write_result = match mode {
                FileWriteMode::Create | FileWriteMode::Overwrite => {
                    std::fs::write(&canonical, args.content.as_bytes())
                }
                FileWriteMode::Append => {
                    let mut file = match std::fs::OpenOptions::new()
                        .append(true)
                        .open(&canonical)
                    {
                        Ok(file) => file,
                        Err(error) => {
                            return Ok(error_result(
                                &call_id,
                                format!("Failed to open '{}' for append: {error}", args.path),
                            ));
                        }
                    };
                    let result = file
                        .write_all(args.content.as_bytes())
                        .and_then(|_| file.flush());
                    if result.is_err() {
                        // Best-effort rollback of a partial append. The checkpoint remains
                        // available as an additional recovery path.
                        let _ = file.set_len(bytes_before);
                    }
                    result
                }
            };

            if let Err(error) = write_result {
                return Ok(error_result(
                    &call_id,
                    format!("Failed to write '{}': {error}", args.path),
                ));
            }

            if let Some(scope) = &file_changes {
                if mode == FileWriteMode::Append { scope.record_append(&checkpoint, args.content.as_bytes()); }
                else { scope.record_checkpoint(&checkpoint, args.content.as_bytes()); }
            }
            let bytes_after = std::fs::metadata(&canonical).map_err(CoreError::Io)?.len();
            let diff = match mode {
                FileWriteMode::Create => create_file_diff_artifact(&args.path, &args.content),
                FileWriteMode::Overwrite => {
                    text_diff_artifact(&args.path, "overwrite", &old_content, &args.content)
                }
                FileWriteMode::Append => {
                    append_diff_artifact(&args.path, &args.content, bytes_before)
                }
            };
            let mut artifacts = checkpoint_artifact_with_diff(
                &checkpoint,
                Some(bytes_after),
                diff,
                Some(0),
            );
            if let Some(map) = artifacts.as_object_mut() {
                map.insert(
                    "writeProgress".to_string(),
                    json!({
                        "protocolVersion": CREATE_FILE_PROTOCOL_VERSION,
                        "legacyOverwriteArgument": args.overwrite,
                        "mode": mode.as_str(),
                        "bytesBefore": bytes_before,
                        "bytesWritten": bytes_written,
                        "bytesAfter": bytes_after,
                        "expectedBytes": args.expected_bytes,
                        "nextExpectedBytes": bytes_after,
                    }),
                );
            }

            let content = match mode {
                FileWriteMode::Create => format!(
                    "Created file '{}' ({} bytes).\nPath: {}\nCheckpoint: {}\nNext expected_bytes: {}",
                    args.path,
                    bytes_after,
                    canonical.display(),
                    checkpoint.id,
                    bytes_after,
                ),
                FileWriteMode::Overwrite => format!(
                    "Overwrote file '{}' ({} bytes).\nPath: {}\nCheckpoint: {}\nNext expected_bytes: {}",
                    args.path,
                    bytes_after,
                    canonical.display(),
                    checkpoint.id,
                    bytes_after,
                ),
                FileWriteMode::Append => format!(
                    "Appended {} bytes to '{}'.\nPath: {}\nFile size: {} bytes\nCheckpoint: {}\nNext expected_bytes: {}",
                    bytes_written,
                    args.path,
                    canonical.display(),
                    bytes_after,
                    checkpoint.id,
                    bytes_after,
                ),
            };

            Ok(ToolResult {
                call_id,
                content,
                is_error: false,
                artifacts: Some(artifacts),
            })
        })
        .await
        .map_err(|error| CoreError::Internal(format!("task join failed: {error}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_settings::{AppConfig, ShellAccessMode};
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

    fn enable_open_file_access(db: &Database) {
        let mut config = AppConfig::default();
        config.shell_access_mode = ShellAccessMode::Open;
        db.save_app_config(&config).expect("save app config");
    }

    #[tokio::test]
    async fn create_file_success_and_checkpoint_restore() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let file_path = dir.path().join("new_file.txt");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "hello world"
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
        assert!(result.content.contains("Created file"));
        let artifact = result.artifacts.as_ref().unwrap();
        assert_eq!(artifact["diff"]["operation"], "create");
        assert_eq!(artifact["diff"]["additions"], 1);
        assert_eq!(artifact["diff"]["deletions"], 0);
        assert_eq!(artifact["diff"]["hunks"][0]["lines"][0]["type"], "addition");
        assert_eq!(
            artifact["diff"]["hunks"][0]["lines"][0]["content"],
            "hello world"
        );
        assert_eq!(artifact["diffStats"]["kind"], "diffStats");
        assert_eq!(artifact["diffStats"]["operation"], "create");
        assert_eq!(artifact["writeProgress"]["protocolVersion"], 2);
        assert_eq!(artifact["writeProgress"]["legacyOverwriteArgument"], false);
        assert_eq!(artifact["writeProgress"]["nextExpectedBytes"], 11);
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello world");

        let checkpoint_id = artifact["checkpoint"]["id"].as_str().unwrap();
        db.restore_file_checkpoint(checkpoint_id).unwrap();
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn create_file_rejects_existing_path_without_explicit_mode() {
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c1",
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
    async fn overwrite_mode_remains_backward_compatible() {
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c1",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            result.artifacts.as_ref().unwrap()["diff"]["operation"],
            "overwrite"
        );
        assert_eq!(
            result.artifacts.as_ref().unwrap()["writeProgress"]["protocolVersion"],
            2
        );
        assert_eq!(
            result.artifacts.as_ref().unwrap()["writeProgress"]["legacyOverwriteArgument"],
            true
        );
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "new content");
    }

    #[tokio::test]
    async fn append_mode_is_ordered_and_checkpointed() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("long-plan.md");
        std::fs::write(&file_path, "first\n").unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "mode": "append",
            "expected_bytes": 6,
            "content": "second\n"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "append-1",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "first\nsecond\n"
        );
        let artifact = result.artifacts.as_ref().unwrap();
        assert_eq!(artifact["diff"]["operation"], "append");
        assert_eq!(artifact["diffStats"]["additions"], 1);
        assert_eq!(artifact["writeProgress"]["bytesBefore"], 6);
        assert_eq!(artifact["writeProgress"]["nextExpectedBytes"], 13);

        let checkpoint_id = artifact["checkpoint"]["id"].as_str().unwrap();
        db.restore_file_checkpoint(checkpoint_id).unwrap();
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "first\n");
    }

    #[tokio::test]
    async fn append_mode_requires_matching_expected_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("long-plan.md");
        std::fs::write(&file_path, "first\n").unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;

        for args in [
            serde_json::json!({
                "path": file_path.to_string_lossy(),
                "mode": "append",
                "content": "duplicate\n"
            }),
            serde_json::json!({
                "path": file_path.to_string_lossy(),
                "mode": "append",
                "expected_bytes": 5,
                "content": "out of order\n"
            }),
        ] {
            let result = tool
                .execute(crate::tools::ToolExecutionContext::new(
                    "append-invalid",
                    &args.to_string(),
                    &db,
                    &[],
                ))
                .await
                .unwrap();
            assert!(result.is_error);
        }
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "first\n");
    }

    #[tokio::test]
    async fn create_file_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let file_path = dir.path().join("sub").join("deep").join("file.md");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "# Hello"
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
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn create_file_resolves_source_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": "notes/today.md",
            "content": "hello"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "c-rel",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes").join("today.md")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn create_file_rejects_office_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": "reports/status.docx",
            "content": "this should be structured, not plain text"
        });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "c-docx",
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
    async fn create_file_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        let tool = CreateFileTool;
        let args = serde_json::json!({
            "path": format!("{}/../../../etc/passwd", dir.path().display()),
            "content": "evil"
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
        assert!(result.is_error);
        assert!(result.content.contains("traversal"));
    }

    #[tokio::test]
    async fn create_file_rejects_outside_source() {
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
            .execute(crate::tools::ToolExecutionContext::new(
                "c1",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Access denied"));
    }

    #[tokio::test]
    async fn create_file_open_mode_allows_absolute_path_outside_source() {
        let dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let db = setup_db_with_source(dir.path());
        enable_open_file_access(&db);
        let tool = CreateFileTool;
        let file_path = other_dir.path().join("outside.txt");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "allowed"
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
        assert_eq!(std::fs::read_to_string(file_path).unwrap(), "allowed");
    }

    #[test]
    fn mode_validation_rejects_conflicting_legacy_flag() {
        let args = CreateFileArgs {
            path: "x.txt".to_string(),
            content: String::new(),
            overwrite: true,
            mode: Some("append".to_string()),
            expected_bytes: Some(0),
        };
        assert!(normalized_mode(&args).is_err());
    }

    #[test]
    fn schema_advertises_the_current_write_protocol() {
        let schema = CreateFileTool.parameters_schema();
        assert_eq!(
            schema["x-nexa-protocol-version"],
            CREATE_FILE_PROTOCOL_VERSION
        );
    }
}
