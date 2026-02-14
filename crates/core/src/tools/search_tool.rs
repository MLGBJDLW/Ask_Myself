//! SearchTool — wraps the existing hybrid/FTS search for agent use.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::{FileType, SearchQuery};
use crate::search;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/search_knowledge_base.json");

/// Tool that searches the local knowledge base using full-text and vector
/// search, returning evidence cards with content, source paths, and scores.
pub struct SearchTool;

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    source_ids: Vec<String>,
    #[serde(default)]
    file_types: Vec<String>,
    #[serde(default)]
    date_from: Option<String>,
    #[serde(default)]
    date_to: Option<String>,
}

fn default_limit() -> u32 {
    5
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search_knowledge_base"
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
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SearchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid search_knowledge_base arguments: {e}"))
        })?;

        let limit = args.limit.min(20).max(1);

        let mut filters = crate::models::SearchFilters::default();

        // Merge agent-supplied source_ids with conversation-level source_scope.
        let mut all_source_ids: Vec<uuid::Uuid> = args
            .source_ids
            .iter()
            .filter_map(|s| uuid::Uuid::parse_str(s).ok())
            .collect();
        for s in source_scope {
            if let Ok(u) = uuid::Uuid::parse_str(s) {
                if !all_source_ids.contains(&u) {
                    all_source_ids.push(u);
                }
            }
        }
        filters.source_ids = all_source_ids;

        // Map string file type names to the FileType enum.
        filters.file_types = args
            .file_types
            .iter()
            .filter_map(|ft| match ft.to_lowercase().as_str() {
                "markdown" => Some(FileType::Markdown),
                "plaintext" | "plain_text" | "text" => Some(FileType::PlainText),
                "log" => Some(FileType::Log),
                "pdf" => Some(FileType::Pdf),
                "docx" => Some(FileType::Docx),
                "excel" => Some(FileType::Excel),
                "pptx" => Some(FileType::Pptx),
                _ => None,
            })
            .collect();

        // Parse optional date range filters.
        if let Some(ref df) = args.date_from {
            filters.date_from = chrono::DateTime::parse_from_rfc3339(df)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc));
        }
        if let Some(ref dt) = args.date_to {
            filters.date_to = chrono::DateTime::parse_from_rfc3339(dt)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc));
        }

        let sq = SearchQuery {
            text: args.query,
            filters,
            limit,
            offset: 0,
        };

        // Run blocking search on a dedicated thread to avoid deadlocking the async runtime.
        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            // Try hybrid search first; fall back to FTS-only on failure.
            let result = match search::hybrid_search(&db, &sq) {
                Ok(r) => r,
                Err(_) => search::search(&db, &sq)?,
            };

            // Format human-readable text for the LLM.
            let mut text = format!(
                "Found {} results ({} ms, mode: {}):\n\n",
                result.total_matches, result.search_time_ms, result.search_mode
            );

            for (i, card) in result.evidence_cards.iter().enumerate() {
                text.push_str(&format!(
                    "--- Result {} (score: {:.3}) ---\n\
                     Source: {}\n\
                     Path: {}\n\
                     Title: {}\n\
                     Content:\n{}\n\n",
                    i + 1,
                    card.score,
                    card.source_name,
                    card.document_path,
                    card.document_title,
                    card.content,
                ));
            }

            // Structured artifacts for frontend consumption.
            let artifacts = serde_json::to_value(&result.evidence_cards).ok();

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
