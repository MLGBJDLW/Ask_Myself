//! `ppt_generate` — generate a PowerPoint deck via frontend pptxgenjs renderer.
//!
//! Architecture: This tool validates the deck-JSON spec and returns it as a
//! `ppt_deck` artifact. The frontend detects the artifact, renders via pptxgenjs
//! in the WebView, and saves via the `save_pptx_bytes` Tauri command.
//!
//! The tool itself does NOT write bytes to disk; rendering is delegated.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

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
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: PptGenerateArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid ppt_generate arguments: {e}"))
        })?;

        if args.path.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ppt_generate: missing or empty `path`".into(),
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

        let artifact = DeckArtifact {
            path: args.path.clone(),
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
                slides, args.path
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({ "ppt_deck": artifact_value })),
        })
    }
}
