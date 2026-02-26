//! SearchPlaybooksTool — search playbooks by keyword/topic.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/search_playbooks.json");

/// Tool that searches playbooks by topic, keyword, or cited content.
pub struct SearchPlaybooksTool;

#[derive(Deserialize)]
struct SearchPlaybooksArgs {
    query: String,
}

#[async_trait]
impl Tool for SearchPlaybooksTool {
    fn name(&self) -> &str {
        "search_playbooks"
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
        let args: SearchPlaybooksArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid search_playbooks arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let query = args.query;

        tokio::task::spawn_blocking(move || {
            let results = db.search_playbooks(&query)?;

            if results.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!("No playbooks found matching \"{query}\"."),
                    is_error: false,
                    artifacts: None,
                });
            }

            let mut text = format!("Found {} playbook(s) matching \"{query}\":\n\n", results.len());
            for r in &results {
                text.push_str(&format!(
                    "- **{}** (id: {}, {} citations, relevance: {:.0}%)\n",
                    r.title,
                    r.id,
                    r.citation_count,
                    r.relevance_score * 100.0,
                ));
                if !r.description.is_empty() {
                    text.push_str(&format!("  Description: {}\n", r.description));
                }
                if !r.cited_content_preview.is_empty() {
                    text.push_str("  Cited content preview:\n");
                    for preview in &r.cited_content_preview {
                        let short: String = preview.chars().take(120).collect();
                        text.push_str(&format!("    - {short}…\n"));
                    }
                }
                text.push('\n');
            }

            let artifacts = serde_json::to_value(&results).ok();

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts,
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
