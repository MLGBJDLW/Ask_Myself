//! WriteNoteTool — creates or updates note files in source directories.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/write_note.json");

/// Tool that creates or appends to note files within the `notes/`
/// subdirectory of a registered source root.
pub struct WriteNoteTool;

#[derive(Deserialize)]
struct WriteNoteArgs {
    filename: String,
    content: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    source_id: Option<String>,
}

fn default_mode() -> String {
    "create".to_string()
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

const ALLOWED_EXTENSIONS: &[&str] = &[".md", ".txt", ".org", ".rst"];

fn validate_filename(filename: &str) -> Result<(), String> {
    // No path separators or traversal.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Filename must not contain path separators or '..'".to_string());
    }

    // Must not be empty.
    if filename.trim().is_empty() {
        return Err("Filename must not be empty".to_string());
    }

    // Must end with an allowed extension.
    let lower = filename.to_lowercase();
    if !ALLOWED_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return Err(format!(
            "Filename must end with one of: {}",
            ALLOWED_EXTENSIONS.join(", ")
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for WriteNoteTool {
    fn name(&self) -> &str {
        "write_note"
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
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: WriteNoteArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid write_note arguments: {e}")))?;

        // Validate filename.
        if let Err(msg) = validate_filename(&args.filename) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: msg,
                is_error: true,
                artifacts: None,
            });
        }

        // Validate mode.
        let mode = args.mode.to_lowercase();
        if !matches!(mode.as_str(), "create" | "append" | "overwrite") {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "Invalid mode '{}'. Must be 'create', 'append', or 'overwrite'.",
                    args.mode
                ),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            // Resolve source directory.
            let sources = db.list_sources()?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let source = if let Some(ref sid) = args.source_id {
                sources.iter().find(|s| s.id == *sid).ok_or_else(|| {
                    CoreError::InvalidInput(format!("Source with id '{sid}' not found"))
                })?
            } else {
                &sources[0]
            };

            // Build the file path: <source_root>/notes/<filename>
            let notes_dir = PathBuf::from(&source.root_path).join("notes");
            if !notes_dir.exists() {
                std::fs::create_dir_all(&notes_dir).map_err(CoreError::Io)?;
            }

            let file_path = notes_dir.join(&args.filename);

            // Safety: verify the resolved path is still within the notes dir.
            let canonical_notes = std::fs::canonicalize(&notes_dir).map_err(CoreError::Io)?;
            // For create/overwrite on non-existent file, parent must match.
            if file_path.exists() {
                let canonical_file = std::fs::canonicalize(&file_path).map_err(CoreError::Io)?;
                if !canonical_file.starts_with(&canonical_notes) {
                    return Ok(ToolResult {
                        call_id: call_id.clone(),
                        content: "Path traversal detected — access denied.".to_string(),
                        is_error: true,
                        artifacts: None,
                    });
                }
            }

            // Execute the write based on mode.
            match mode.as_str() {
                "create" => {
                    if file_path.exists() {
                        return Ok(ToolResult {
                            call_id: call_id.clone(),
                            content: format!(
                                "File '{}' already exists. Use mode 'append' or 'overwrite'.",
                                args.filename
                            ),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    std::fs::write(&file_path, &args.content).map_err(CoreError::Io)?;
                }
                "append" => {
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&file_path)
                        .map_err(CoreError::Io)?;
                    f.write_all(args.content.as_bytes())
                        .map_err(CoreError::Io)?;
                }
                "overwrite" => {
                    std::fs::write(&file_path, &args.content).map_err(CoreError::Io)?;
                }
                _ => unreachable!(),
            }

            let size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);

            let text = format!(
                "Note '{}' written successfully.\nPath: {}\nSize: {} bytes\nMode: {}",
                args.filename,
                file_path.display(),
                size,
                mode,
            );

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
