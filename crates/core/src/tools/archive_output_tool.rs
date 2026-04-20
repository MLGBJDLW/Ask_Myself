//! ArchiveOutputTool — archive agent responses as knowledge base documents.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/archive_output.json");

#[derive(Deserialize)]
struct ArchiveOutputArgs {
    title: String,
    content: String,
    source_directory: String,
}

pub struct ArchiveOutputTool;

#[async_trait]
impl Tool for ArchiveOutputTool {
    fn name(&self) -> &str {
        "archive_output"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Knowledge]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("untitled");
        let dir = args
            .get("source_directory")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Some(format!("Archive document '{title}' to {dir}/_kb_archive/?"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ArchiveOutputArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid archive_output arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            // Use a placeholder conversation_id since we don't have the actual one in tool context
            let result = db.archive_agent_output(
                "tool-archive",
                &args.content,
                &args.title,
                &args.source_directory,
            )?;

            Ok(ToolResult {
                call_id,
                content: format!(
                    "Archived as document ID {} at: {}\nTitle: {}",
                    result.document_id, result.source, result.title,
                ),
                is_error: false,
                artifacts: None,
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}
