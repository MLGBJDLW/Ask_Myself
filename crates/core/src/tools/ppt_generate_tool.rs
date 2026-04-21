//! `ppt_generate` — generate a PowerPoint deck via frontend pptxgenjs renderer.
//!
//! Architecture: This tool validates the deck-JSON spec and returns it as a
//! `ppt_deck` artifact. The frontend detects the artifact, renders via pptxgenjs
//! in the WebView, and saves via the `save_pptx_bytes` Tauri command.
//!
//! The tool itself does NOT write bytes to disk; rendering is delegated.
//! It does, however, validate the target path against registered source roots
//! (matching the security model of `generate_docx`).

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/ppt_generate.json");

pub struct PptGenerateTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckArtifact {
    pub path: String,
    pub spec: serde_json::Value,
}

#[derive(Deserialize)]
struct PptGenerateArgs {
    path: String,
    spec: serde_json::Value,
}

#[async_trait]
impl Tool for PptGenerateTool {
    fn name(&self) -> &str {
        "ppt_generate"
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
            .unwrap_or("(unknown)");
        let slide_count = args
            .get("spec")
            .and_then(|s| s.get("slides"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Some(format!(
            "Generate PowerPoint deck ({slide_count} slides) to: {path}"
        ))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: PptGenerateArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid ppt_generate arguments: {e}")))?;

        if args.path.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: missing or empty `path`".into(),
                is_error: true,
                artifacts: None,
            });
        }

        if !args.path.to_ascii_lowercase().ends_with(".pptx") {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "ppt_generate: `path` must end with `.pptx` (got '{}').",
                    args.path
                ),
                is_error: true,
                artifacts: None,
            });
        }

        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: path must not contain '..' traversal sequences.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        if !args.spec.is_object() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: missing `spec` object".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let slides = args
            .spec
            .get("slides")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if slides == 0 {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: `spec.slides` must be a non-empty array".into(),
                is_error: true,
                artifacts: None,
            });
        }

        // Validate the path falls inside a registered source root. We do NOT
        // write the file here (the frontend does), but we resolve to the
        // canonical absolute path so the artifact carries a trusted location.
        let sources = scoped_sources(db, source_scope)?;
        if sources.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: no sources registered. Add a source directory first."
                    .into(),
                is_error: true,
                artifacts: None,
            });
        }
        let requested = PathBuf::from(&args.path);
        let canonical = match resolve_and_validate(&requested, &sources) {
            Ok(p) => p,
            Err(msg) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("ppt_generate: {msg}"),
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        let artifact = DeckArtifact {
            path: canonical.to_string_lossy().into_owned(),
            spec: args.spec,
        };
        let artifact_value = match serde_json::to_value(&artifact) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("ppt_generate: serialize artifact: {e}"),
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "Deck spec validated ({} slides). Rendering to {}…",
                slides,
                canonical.display()
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({ "ppt_deck": artifact_value })),
        })
    }
}
