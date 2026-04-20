//! GenerateDocumentTool — backward-compatible router that dispatches to
//! `generate_docx` or `generate_xlsx` based on the `format` field.
//! PPTX requests are redirected to the dedicated `ppt_generate` tool.
//!
//! This tool is kept for backward compatibility. New code should call the
//! format-specific tools directly.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::generate_docx_tool::generate_docx;
use super::generate_xlsx_tool::generate_xlsx;
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_document.json");

pub struct GenerateDocumentTool;

#[derive(Deserialize)]
struct GenerateDocArgs {
    format: String,
    path: String,
    content: serde_json::Value,
}

#[async_trait]
impl Tool for GenerateDocumentTool {
    fn name(&self) -> &str {
        "generate_document"
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
        let fmt = args.get("format").and_then(|v| v.as_str()).unwrap_or("?");
        Some(format!("Generate {fmt} document: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GenerateDocArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_document arguments: {e}"))
        })?;

        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "Path must not contain '..' traversal sequences.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let format = args.format.to_lowercase();
        if format == "pptx" {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "For PPTX output, call the `ppt_generate` tool directly with a deck spec."
                    .into(),
                is_error: true,
                artifacts: None,
            });
        }
        if !matches!(format.as_str(), "docx" | "xlsx") {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("Unsupported format '{format}'. Use docx or xlsx (for pptx use `ppt_generate`)."),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let content = args.content;
        let path_str = args.path;
        let source_scope = source_scope.to_vec();

        tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db, &source_scope)?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".into(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let requested = PathBuf::from(&path_str);
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

            if let Some(parent) = canonical.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
                }
            }

            let result = match format.as_str() {
                "docx" => generate_docx(&canonical, &content),
                "xlsx" => generate_xlsx(&canonical, &content),
                _ => unreachable!(),
            };

            match result {
                Ok(size) => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Generated {format} document '{}' ({size} bytes).\nPath: {}",
                        path_str,
                        canonical.display()
                    ),
                    is_error: false,
                    artifacts: None,
                }),
                Err(msg) => Ok(ToolResult {
                    call_id,
                    content: msg,
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
