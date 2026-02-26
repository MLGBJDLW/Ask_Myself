//! SearchTool — wraps the existing hybrid/FTS search for agent use.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::{EvidenceCard, FileType, SearchQuery};
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
    #[serde(default)]
    queries: Option<Vec<String>>,
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

/// RRF merge across multiple ranked result lists.
fn multi_query_rrf_merge(
    ranked_lists: &[Vec<(String, f32)>],
    k: f32,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for ranked in ranked_lists {
        for (rank, (chunk_id, _)) in ranked.iter().enumerate() {
            let r = (rank + 1) as f32;
            *scores.entry(chunk_id.clone()).or_insert(0.0) += 1.0 / (k + r);
        }
    }
    let mut merged: Vec<(String, f32)> = scores.into_iter().collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

/// Format a SearchResult into a ToolResult for the LLM.
fn format_search_result(call_id: &str, result: &search::SearchResult) -> ToolResult {
    let mut text = format!(
        "Found {} results ({} ms, mode: {}):\n\n",
        result.total_matches, result.search_time_ms, result.search_mode
    );

    for (i, card) in result.evidence_cards.iter().enumerate() {
        text.push_str(&format!(
            "--- Result {} (score: {:.3}) ---\n\
             [chunk_id: {}]\n\
             Source: {}\n\
             Path: {}\n\
             Title: {}\n\
             Content:\n{}\n\n",
            i + 1,
            card.score,
            card.chunk_id,
            card.source_name,
            card.document_path,
            card.document_title,
            card.content,
        ));
    }

    let artifacts = serde_json::to_value(&result.evidence_cards).ok();

    ToolResult {
        call_id: call_id.to_string(),
        content: text,
        is_error: false,
        artifacts,
    }
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
                "image" => Some(FileType::Image),
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

        // Determine which queries to run.
        let queries: Vec<String> = match args.queries {
            Some(ref qs) if !qs.is_empty() => qs.clone(),
            _ => vec![args.query.clone()],
        };

        // Run blocking search on a dedicated thread to avoid deadlocking the async runtime.
        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            if queries.len() == 1 {
                // Single query — original path.
                let sq = SearchQuery {
                    text: queries[0].clone(),
                    filters,
                    limit,
                    offset: 0,
                };

                let result = match search::hybrid_search(&db, &sq) {
                    Ok(r) => r,
                    Err(_) => search::search(&db, &sq)?,
                };

                Ok(format_search_result(&call_id, &result))
            } else {
                // Multi-query — run each and merge via Reciprocal Rank Fusion.
                let mut all_ranked: Vec<Vec<(String, f32)>> = Vec::new();
                let mut card_map: HashMap<String, EvidenceCard> = HashMap::new();
                let mut total_time_ms: u64 = 0;
                let query_count = queries.len();

                // Over-fetch per query so RRF has more candidates.
                let per_query_limit = std::cmp::min(limit * 2, 20);

                for q in &queries {
                    let sq = SearchQuery {
                        text: q.clone(),
                        filters: filters.clone(),
                        limit: per_query_limit,
                        offset: 0,
                    };
                    let result = match search::hybrid_search(&db, &sq) {
                        Ok(r) => r,
                        Err(_) => search::search(&db, &sq)?,
                    };
                    total_time_ms += result.search_time_ms;

                    let ranked: Vec<(String, f32)> = result
                        .evidence_cards
                        .iter()
                        .map(|c| (c.chunk_id.to_string(), c.score as f32))
                        .collect();
                    all_ranked.push(ranked);

                    for card in result.evidence_cards {
                        let id = card.chunk_id.to_string();
                        card_map.entry(id).or_insert(card);
                    }
                }

                // RRF merge across all query result lists.
                let merged = multi_query_rrf_merge(&all_ranked, 60.0);

                // Assemble final evidence cards up to the requested limit.
                let mut cards: Vec<EvidenceCard> = Vec::new();
                for (chunk_id, rrf_score) in merged.iter().take(limit as usize) {
                    if let Some(mut card) = card_map.remove(chunk_id) {
                        card.score = *rrf_score as f64;
                        cards.push(card);
                    }
                }

                let merged_result = search::SearchResult {
                    query: queries.join(" | "),
                    total_matches: cards.len(),
                    evidence_cards: cards,
                    search_time_ms: total_time_ms,
                    search_mode: format!("multi-query ({} queries, hybrid)", query_count),
                };

                Ok(format_search_result(&call_id, &merged_result))
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
