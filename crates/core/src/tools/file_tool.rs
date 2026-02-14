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

            // Read the file.
            let raw = std::fs::read_to_string(&canonical).map_err(|e| CoreError::Io(e))?;

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
