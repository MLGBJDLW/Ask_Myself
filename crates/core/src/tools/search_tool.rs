//! SearchTool — wraps the existing hybrid/FTS search for agent use.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::SearchQuery;
use crate::search;

use super::{Tool, ToolResult};

/// Tool that searches the local knowledge base using full-text and vector
/// search, returning evidence cards with content, source paths, and scores.
pub struct SearchTool;

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: u32,
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
        "Search the local knowledge base using full-text and vector search. \
         Returns evidence cards with content, source paths, and relevance scores."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query text"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5, max 20)",
                    "default": 5,
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
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
        if !source_scope.is_empty() {
            filters.source_ids = source_scope
                .iter()
                .filter_map(|s| uuid::Uuid::parse_str(s).ok())
                .collect();
        }

        let sq = SearchQuery {
            text: args.query,
            filters,
            limit,
            offset: 0,
        };

        // Try hybrid search first; fall back to FTS-only on failure.
        let result = match search::hybrid_search(db, &sq) {
            Ok(r) => r,
            Err(_) => search::search(db, &sq)?,
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
            call_id: call_id.to_string(),
            content: text,
            is_error: false,
            artifacts,
        })
    }
}
