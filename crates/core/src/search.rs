//! Search module — query execution and result ranking.

use std::collections::HashMap;
use std::time::Instant;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::embed::{cosine_similarity, create_embedder, Embedder, TfIdfEmbedder};
use crate::error::CoreError;
use crate::models::{EvidenceCard, FileType, Highlight, SearchQuery};
use crate::personalization;

// ---------------------------------------------------------------------------
// Defaults (also exposed as AppConfig fields for configurability)
// ---------------------------------------------------------------------------

/// Default search result limit when the caller doesn't specify one.
const DEFAULT_SEARCH_LIMIT: u32 = 20;

/// Maximum length for the snippet preview field.
const SNIPPET_MAX_LEN: usize = 150;

fn truncate_to_char_boundary(content: &str, max_len: usize) -> &str {
    if content.len() <= max_len {
        return content;
    }

    let mut end = max_len;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

/// Generate a short snippet from full content for preview display.
fn make_snippet(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= SNIPPET_MAX_LEN {
        Some(trimmed.to_string())
    } else {
        // Break at a word boundary if possible.
        let slice = truncate_to_char_boundary(trimmed, SNIPPET_MAX_LEN);
        let end = slice.rfind(' ').unwrap_or(slice.len());
        let snippet = if end == 0 { slice } else { &slice[..end] };
        Some(format!("{}…", snippet))
    }
}

/// Deduplicate evidence cards by document: keep only the highest-scored card
/// per `document_id`.
fn deduplicate_by_document(cards: Vec<EvidenceCard>) -> Vec<EvidenceCard> {
    let mut best: HashMap<Uuid, EvidenceCard> = HashMap::new();
    for card in cards {
        best.entry(card.document_id)
            .and_modify(|existing| {
                if card.score > existing.score {
                    *existing = card.clone();
                }
            })
            .or_insert(card);
    }
    let mut result: Vec<EvidenceCard> = best.into_values().collect();
    result.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

/// Minimum cosine similarity to include a vector search result.
const DEFAULT_MIN_SEARCH_SIMILARITY: f32 = 0.2;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of a search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub query: String,
    pub total_matches: usize,
    pub evidence_cards: Vec<EvidenceCard>,
    pub search_time_ms: u64,
    pub search_mode: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Execute a full-text search and return ranked evidence cards.
///
/// Builds an FTS5 MATCH query from the user text, joins through
/// `fts_chunks → chunks → documents → sources`, applies any filters,
/// ranks by BM25, and assembles [`EvidenceCard`]s with highlights.
pub fn search(db: &Database, query: &SearchQuery) -> Result<SearchResult, CoreError> {
    let start = Instant::now();

    let trimmed = query.text.trim();
    if trimmed.is_empty() {
        return Ok(SearchResult {
            query: query.text.clone(),
            total_matches: 0,
            evidence_cards: Vec::new(),
            search_time_ms: start.elapsed().as_millis() as u64,
            search_mode: "fts".to_string(),
        });
    }

    let base_fts = build_fts_query(trimmed);
    if base_fts.is_empty() {
        return Ok(SearchResult {
            query: query.text.clone(),
            total_matches: 0,
            evidence_cards: Vec::new(),
            search_time_ms: start.elapsed().as_millis() as u64,
            search_mode: "fts".to_string(),
        });
    }

    // Query expansion from feedback history.
    let extra_terms = db
        .get_related_feedback_terms(trimmed, 5)
        .unwrap_or_default();
    let fts_query = if extra_terms.is_empty() {
        base_fts
    } else {
        let extras = extra_terms
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(" OR ");
        format!("({}) OR ({})", base_fts, extras)
    };

    let limit = if query.limit == 0 {
        DEFAULT_SEARCH_LIMIT
    } else {
        query.limit
    };
    // Over-fetch so feedback reranking can surface high-value results
    // that BM25 alone might rank outside the requested limit.
    let internal_limit = std::cmp::min(limit * 3, limit + 30);
    let terms = extract_terms(trimmed);

    // -- build dynamic SQL ------------------------------------------------

    let mut sql = String::from(
        "SELECT c.id, c.document_id, c.content, c.chunk_index, c.metadata_json,
                d.path, d.title, d.source_id, s.root_path,
                fts.rank, COALESCE(d.metadata, '{}')
         FROM fts_chunks fts
         JOIN chunks c ON c.rowid = fts.rowid
         JOIN documents d ON d.id = c.document_id
         JOIN sources s ON s.id = d.source_id
         WHERE fts_chunks MATCH ?1",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(fts_query.clone()));
    let mut param_idx: usize = 2;

    let filters = &query.filters;

    if !filters.source_ids.is_empty() {
        let placeholders: Vec<String> = filters
            .source_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", param_idx + i))
            .collect();
        sql.push_str(&format!(" AND d.source_id IN ({})", placeholders.join(",")));
        for sid in &filters.source_ids {
            param_values.push(Box::new(sid.to_string()));
            param_idx += 1;
        }
    }

    if !filters.file_types.is_empty() {
        let all_mimes: Vec<String> = filters
            .file_types
            .iter()
            .flat_map(file_type_to_mimes)
            .collect();
        let placeholders: Vec<String> = all_mimes
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", param_idx + i))
            .collect();
        sql.push_str(&format!(" AND d.mime_type IN ({})", placeholders.join(",")));
        for mime in all_mimes {
            param_values.push(Box::new(mime));
            param_idx += 1;
        }
    }

    if let Some(ref from) = filters.date_from {
        sql.push_str(&format!(" AND d.indexed_at >= ?{}", param_idx));
        param_values.push(Box::new(from.to_rfc3339()));
        param_idx += 1;
    }

    if let Some(ref to) = filters.date_to {
        sql.push_str(&format!(" AND d.indexed_at <= ?{}", param_idx));
        param_values.push(Box::new(to.to_rfc3339()));
        param_idx += 1;
    }

    // FTS5 `rank` is negative BM25 — lower (more negative) = better match.
    sql.push_str(" ORDER BY fts.rank");
    sql.push_str(&format!(" LIMIT ?{}", param_idx));
    param_values.push(Box::new(internal_limit as i64));
    param_idx += 1;

    sql.push_str(&format!(" OFFSET ?{}", param_idx));
    param_values.push(Box::new(query.offset as i64));

    // -- execute ----------------------------------------------------------

    let (mut cards, total_matches) = {
        let conn = db.conn();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;

        let cards: Vec<EvidenceCard> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let chunk_id: String = row.get(0)?;
                let document_id: String = row.get(1)?;
                let content: String = row.get(2)?;
                let _chunk_index: i64 = row.get(3)?;
                let metadata_json: String = row.get(4)?;
                let doc_path: String = row.get(5)?;
                let doc_title: Option<String> = row.get(6)?;
                let _source_id: String = row.get(7)?;
                let source_root: String = row.get(8)?;
                let rank: f64 = row.get(9)?;
                let doc_metadata: String = row.get(10)?;

                let heading_path = parse_heading_path(&metadata_json);
                let source_name = extract_source_name(&source_root);
                let highlights = compute_highlights(&content, &terms);

                let snippet = make_snippet(&content);
                Ok(EvidenceCard {
                    chunk_id: Uuid::parse_str(&chunk_id).unwrap_or_default(),
                    document_id: Uuid::parse_str(&document_id).unwrap_or_default(),
                    source_id: Uuid::parse_str(&_source_id).unwrap_or_default(),
                    source_name,
                    document_path: doc_path,
                    document_title: doc_title.unwrap_or_default(),
                    content,
                    heading_path,
                    score: -rank, // negate: FTS5 BM25 is negative
                    highlights,
                    snippet,
                    document_date: extract_document_date(&doc_metadata),
                    credibility: None,
                    freshness_days: None,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Count total matches (without LIMIT/OFFSET) for accurate pagination info.
        let total_matches = if cards.len() < internal_limit as usize && query.offset == 0 {
            // If we got fewer results than the internal limit on the first page, total = len.
            cards.len()
        } else {
            // Run a separate count query for the true total.
            let mut count_sql = String::from(
                "SELECT COUNT(*)
             FROM fts_chunks fts
             JOIN chunks c ON c.rowid = fts.rowid
             JOIN documents d ON d.id = c.document_id
             JOIN sources s ON s.id = d.source_id
             WHERE fts_chunks MATCH ?1",
            );
            // Re-apply the same filters (reuse the filter params before limit/offset).
            // The param_values list has filters then limit then offset at the end.
            // Rebuild a minimal count param list with just the fts_query + filters.
            let mut count_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            count_params.push(Box::new(fts_query.clone()));
            let mut cp_idx: usize = 2;

            if !filters.source_ids.is_empty() {
                let placeholders: Vec<String> = filters
                    .source_ids
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", cp_idx + i))
                    .collect();
                count_sql.push_str(&format!(" AND d.source_id IN ({})", placeholders.join(",")));
                for sid in &filters.source_ids {
                    count_params.push(Box::new(sid.to_string()));
                    cp_idx += 1;
                }
            }
            if !filters.file_types.is_empty() {
                let all_mimes: Vec<String> = filters
                    .file_types
                    .iter()
                    .flat_map(file_type_to_mimes)
                    .collect();
                let placeholders: Vec<String> = all_mimes
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", cp_idx + i))
                    .collect();
                count_sql.push_str(&format!(" AND d.mime_type IN ({})", placeholders.join(",")));
                for mime in all_mimes {
                    count_params.push(Box::new(mime));
                    cp_idx += 1;
                }
            }
            if let Some(ref from) = filters.date_from {
                count_sql.push_str(&format!(" AND d.indexed_at >= ?{}", cp_idx));
                count_params.push(Box::new(from.to_rfc3339()));
                cp_idx += 1;
            }
            if let Some(ref to) = filters.date_to {
                count_sql.push_str(&format!(" AND d.indexed_at <= ?{}", cp_idx));
                count_params.push(Box::new(to.to_rfc3339()));
                let _ = cp_idx;
            }

            let count_refs: Vec<&dyn rusqlite::types::ToSql> =
                count_params.iter().map(|p| p.as_ref()).collect();
            conn.query_row(&count_sql, count_refs.as_slice(), |row| {
                row.get::<_, usize>(0)
            })
            .unwrap_or(cards.len())
        };

        (cards, total_matches)
    }; // conn dropped here

    // Apply feedback-based re-ranking (must happen after conn is released).
    apply_feedback_reranking(&mut cards, db, trimmed)?;

    // Enrich with credibility and freshness, blend into ranking.
    apply_credibility_scoring(&mut cards);

    // Deduplicate: keep only the highest-scored card per document.
    let cards = deduplicate_by_document(cards);

    // Truncate to the user-requested limit after reranking.
    let cards: Vec<EvidenceCard> = cards.into_iter().take(limit as usize).collect();

    Ok(SearchResult {
        query: query.text.clone(),
        total_matches,
        evidence_cards: cards,
        search_time_ms: start.elapsed().as_millis() as u64,
        search_mode: "fts".to_string(),
    })
}

/// Retrieve a single evidence card by chunk ID (for playbook citation lookups).
pub fn get_evidence_card(db: &Database, chunk_id: &str) -> Result<EvidenceCard, CoreError> {
    let conn = db.conn();
    conn.query_row(
        "SELECT c.id, c.document_id, c.content, c.chunk_index, c.metadata_json,
                d.path, d.title, d.source_id, s.root_path,
                COALESCE(d.metadata, '{}')
         FROM chunks c
         JOIN documents d ON d.id = c.document_id
         JOIN sources s ON s.id = d.source_id
         WHERE c.id = ?1",
        params![chunk_id],
        |row| {
            let cid: String = row.get(0)?;
            let did: String = row.get(1)?;
            let content: String = row.get(2)?;
            let _chunk_index: i64 = row.get(3)?;
            let metadata_json: String = row.get(4)?;
            let doc_path: String = row.get(5)?;
            let doc_title: Option<String> = row.get(6)?;
            let _source_id: String = row.get(7)?;
            let source_root: String = row.get(8)?;
            let doc_metadata: String = row.get(9)?;

            let snippet = make_snippet(&content);
            Ok(EvidenceCard {
                chunk_id: Uuid::parse_str(&cid).unwrap_or_default(),
                document_id: Uuid::parse_str(&did).unwrap_or_default(),
                source_id: Uuid::parse_str(&_source_id).unwrap_or_default(),
                source_name: extract_source_name(&source_root),
                document_path: doc_path,
                document_title: doc_title.unwrap_or_default(),
                content,
                heading_path: parse_heading_path(&metadata_json),
                score: 0.0,
                highlights: Vec::new(),
                snippet,
                document_date: extract_document_date(&doc_metadata),
                credibility: None,
                freshness_days: None,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            CoreError::NotFound(format!("Chunk not found: {chunk_id}"))
        }
        other => CoreError::Database(other),
    })
}

/// Retrieve multiple evidence cards by chunk ID, preserving input order.
pub fn get_evidence_cards(
    db: &Database,
    chunk_ids: &[String],
) -> Result<Vec<EvidenceCard>, CoreError> {
    let mut cards = Vec::with_capacity(chunk_ids.len());
    for chunk_id in chunk_ids {
        cards.push(get_evidence_card(db, chunk_id)?);
    }
    Ok(cards)
}

// ---------------------------------------------------------------------------
// Hybrid search
// ---------------------------------------------------------------------------

/// Execute a hybrid search combining FTS5 BM25 and TF-IDF vector cosine
/// similarity via Reciprocal Rank Fusion (RRF).
///
/// Falls back to pure FTS5 when no embeddings or embedder state exist.
pub fn hybrid_search(db: &Database, query: &SearchQuery) -> Result<SearchResult, CoreError> {
    let start = Instant::now();
    let trimmed = query.text.trim();

    if trimmed.is_empty() {
        return Ok(SearchResult {
            query: query.text.clone(),
            total_matches: 0,
            evidence_cards: Vec::new(),
            search_time_ms: start.elapsed().as_millis() as u64,
            search_mode: "hybrid".to_string(),
        });
    }

    let user_limit = if query.limit == 0 {
        DEFAULT_SEARCH_LIMIT
    } else {
        query.limit
    } as usize;
    // Over-fetch so reranking has more candidates to work with.
    let internal_limit: usize = std::cmp::min(user_limit * 3, user_limit + 30);
    let terms = extract_terms(trimmed);

    // Step 1: FTS5 search with larger internal limit.
    let fts_query = SearchQuery {
        text: query.text.clone(),
        filters: query.filters.clone(),
        limit: internal_limit as u32,
        offset: 0,
    };
    let fts_result = search(db, &fts_query)?;

    // Step 2: Vector search — use the configured embedder model.
    let vec_results =
        {
            let config = db.get_embedder_config()?;
            match config.provider.as_str() {
                "local" | "api" => {
                    // Determine model name for DB embedding lookup.
                    let model_name = if config.provider == "local" {
                        config.local_embedding_model().model_name().to_string()
                    } else if config.api_model.is_empty() {
                        "text-embedding-3-small".to_string()
                    } else {
                        config.api_model.clone()
                    };

                    match create_embedder(&config) {
                        Ok(embedder) => {
                            // If create_embedder fell back to an empty TF-IDF
                            // (e.g. ONNX not downloaded), its dimensions will be 0.
                            if embedder.dimensions() == 0 {
                                tracing::warn!(
                                    "Configured embedder ({}) returned empty dimensions, \
                                 falling back to TF-IDF state from DB",
                                    config.provider
                                );
                                tfidf_vector_search(db, trimmed, internal_limit)
                            } else {
                                match embedder.embed(trimmed) {
                                    Ok(query_vec) => {
                                        if query_vec.iter().all(|&v| v == 0.0) {
                                            Vec::new()
                                        } else {
                                            vector_search_top_k(
                                                db,
                                                &query_vec,
                                                &model_name,
                                                internal_limit,
                                                None,
                                            )
                                            .unwrap_or_else(|e| {
                                                tracing::warn!(
                                                    "Vector search with {} failed: {e}, \
                                                 trying TF-IDF fallback",
                                                    model_name
                                                );
                                                tfidf_vector_search(db, trimmed, internal_limit)
                                            })
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to embed query with {}: {e}, \
                                         trying TF-IDF fallback",
                                            config.provider
                                        );
                                        tfidf_vector_search(db, trimmed, internal_limit)
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create embedder ({}): {e}, \
                             trying TF-IDF fallback",
                                config.provider
                            );
                            tfidf_vector_search(db, trimmed, internal_limit)
                        }
                    }
                }
                _ => {
                    // TF-IDF or unknown provider — use TF-IDF from DB state.
                    tfidf_vector_search(db, trimmed, internal_limit)
                }
            }
        };

    // Fallback: no embeddings available → return pure FTS.
    if vec_results.is_empty() {
        let final_cards: Vec<EvidenceCard> = fts_result
            .evidence_cards
            .into_iter()
            .take(user_limit)
            .collect();
        let total = final_cards.len();
        return Ok(SearchResult {
            query: query.text.clone(),
            total_matches: total,
            evidence_cards: final_cards,
            search_time_ms: start.elapsed().as_millis() as u64,
            search_mode: "fts".to_string(),
        });
    }

    // Step 3: Build ranked ID lists for RRF.
    let fts_ranked: Vec<(String, f32)> = fts_result
        .evidence_cards
        .iter()
        .map(|card| (card.chunk_id.to_string(), card.score as f32))
        .collect();

    // Step 4: RRF merge (with optional feedback-based query expansion as third signal).
    let mut merged = rrf_merge(&fts_ranked, &vec_results, 60.0);

    // Query expansion: add feedback-derived terms as a third RRF signal.
    let extra_terms = db
        .get_related_feedback_terms(trimmed, 5)
        .unwrap_or_default();
    if !extra_terms.is_empty() {
        let expansion_text = extra_terms.join(" ");
        let expansion_query = SearchQuery {
            text: expansion_text,
            filters: query.filters.clone(),
            limit: internal_limit as u32,
            offset: 0,
        };
        if let Ok(exp_result) = search(db, &expansion_query) {
            if !exp_result.evidence_cards.is_empty() {
                let k = 60.0_f32;
                let mut score_map: HashMap<String, f32> = merged.into_iter().collect();
                for (rank, card) in exp_result.evidence_cards.iter().enumerate() {
                    let r = (rank + 1) as f32;
                    *score_map.entry(card.chunk_id.to_string()).or_insert(0.0) += 1.0 / (k + r);
                }
                merged = score_map.into_iter().collect();
                merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            }
        }
    }

    // Step 5: Assemble EvidenceCards for the top results.
    let fts_card_map: HashMap<String, EvidenceCard> = fts_result
        .evidence_cards
        .into_iter()
        .map(|card| (card.chunk_id.to_string(), card))
        .collect();

    let mut cards = Vec::new();
    for (chunk_id, rrf_score) in merged.iter().take(user_limit) {
        let mut card = if let Some(fts_card) = fts_card_map.get(chunk_id) {
            fts_card.clone()
        } else {
            // Chunk only surfaced by vector search — fetch from DB.
            match get_evidence_card(db, chunk_id) {
                Ok(c) => c,
                Err(_) => continue,
            }
        };
        card.score = *rrf_score as f64;
        card.highlights = compute_highlights(&card.content, &terms);
        cards.push(card);
    }

    // Apply feedback-based re-ranking.
    apply_feedback_reranking(&mut cards, db, trimmed)?;

    // Enrich with credibility and freshness, blend into ranking.
    apply_credibility_scoring(&mut cards);

    // Deduplicate: keep only the highest-scored card per document.
    let cards = deduplicate_by_document(cards);
    let total = cards.len();

    Ok(SearchResult {
        query: query.text.clone(),
        total_matches: total,
        evidence_cards: cards,
        search_time_ms: start.elapsed().as_millis() as u64,
        search_mode: "hybrid".to_string(),
    })
}

/// Try TF-IDF vector search using saved embedder state from the DB.
///
/// Returns an empty vec if no TF-IDF state exists (graceful degradation).
fn tfidf_vector_search(db: &Database, query_text: &str, limit: usize) -> Vec<(String, f32)> {
    match db.load_embedder_state("tfidf-v1") {
        Ok(Some((vocab, idf))) => {
            let embedder = TfIdfEmbedder::from_vocabulary(vocab, idf);
            match embedder.embed(query_text) {
                Ok(query_vec) => {
                    if query_vec.iter().all(|&v| v == 0.0) {
                        return Vec::new();
                    }
                    vector_search_top_k(db, &query_vec, "tfidf-v1", limit, None).unwrap_or_else(
                        |e| {
                            tracing::warn!("TF-IDF vector search failed: {e}");
                            Vec::new()
                        },
                    )
                }
                Err(e) => {
                    tracing::warn!("TF-IDF query embedding failed: {e}");
                    Vec::new()
                }
            }
        }
        Ok(None) => {
            tracing::debug!("No TF-IDF embedder state in DB, skipping vector search");
            Vec::new()
        }
        Err(e) => {
            tracing::warn!("Failed to load TF-IDF embedder state: {e}");
            Vec::new()
        }
    }
}

/// Vector search optimized for large datasets using batched loading and a
/// min-heap to maintain top-k results without loading all embeddings at once.
///
/// Returns `(chunk_id, cosine_similarity)` pairs sorted by similarity DESC.
pub fn vector_search_top_k(
    db: &Database,
    query_vec: &[f32],
    model: &str,
    k: usize,
    min_sim: Option<f32>,
) -> Result<Vec<(String, f32)>, CoreError> {
    if k == 0 || query_vec.iter().all(|&v| v == 0.0) {
        return Ok(Vec::new());
    }

    const BATCH_SIZE: usize = 10_000;
    let min_similarity = min_sim.unwrap_or(DEFAULT_MIN_SEARCH_SIMILARITY);
    let mut top_k: Vec<(String, f32)> = Vec::with_capacity(k + 1);
    let mut threshold: f32 = min_similarity;

    let mut offset = 0usize;
    loop {
        let batch = db.get_embeddings_batched(model, BATCH_SIZE, offset)?;
        if batch.is_empty() {
            break;
        }
        let batch_len = batch.len();

        for (chunk_id, vec) in batch {
            let sim = cosine_similarity(query_vec, &vec);
            if sim <= min_similarity {
                continue;
            }

            if top_k.len() < k {
                top_k.push((chunk_id, sim));
                if top_k.len() == k {
                    threshold = top_k.iter().map(|(_, s)| *s).fold(f32::INFINITY, f32::min);
                }
            } else if sim > threshold {
                // Replace the element with the lowest score.
                let min_idx = top_k
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap();
                top_k[min_idx] = (chunk_id, sim);
                threshold = top_k.iter().map(|(_, s)| *s).fold(f32::INFINITY, f32::min);
            }
        }

        if batch_len < BATCH_SIZE {
            break;
        }
        offset += BATCH_SIZE;
    }

    top_k.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(top_k)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Returns a small score boost based on document recency.
/// < 3 days: +0.08, < 7 days: +0.05, < 30 days: +0.02, older: 0.0
fn recency_boost(modified_at: &str) -> f64 {
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(modified_at) {
        let age = chrono::Utc::now().signed_duration_since(ts);
        if age.num_days() < 3 {
            0.08
        } else if age.num_days() < 7 {
            0.05
        } else if age.num_days() < 30 {
            0.02
        } else {
            0.0
        }
    } else {
        // Try parsing as SQLite datetime format (YYYY-MM-DD HH:MM:SS)
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(modified_at, "%Y-%m-%d %H:%M:%S") {
            let ts = naive.and_utc();
            let age = chrono::Utc::now().signed_duration_since(ts);
            if age.num_days() < 3 {
                0.08
            } else if age.num_days() < 7 {
                0.05
            } else if age.num_days() < 30 {
                0.02
            } else {
                0.0
            }
        } else {
            0.0
        }
    }
}

/// Query document modified_at timestamps and compute per-document recency boosts.
fn get_document_recency_boosts(
    db: &Database,
    doc_ids: &[String],
) -> Result<HashMap<String, f64>, CoreError> {
    if doc_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = db.conn();
    let placeholders: Vec<String> = (1..=doc_ids.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT id, modified_at FROM documents WHERE id IN ({})",
        placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = doc_ids
        .iter()
        .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut boosts = HashMap::new();
    let rows = stmt.query_map(&*param_refs, |row| {
        let id: String = row.get(0)?;
        let modified_at: String = row.get(1)?;
        Ok((id, modified_at))
    })?;
    for row in rows {
        let (id, modified_at) = row?;
        let boost = recency_boost(&modified_at);
        if boost > 0.0 {
            boosts.insert(id, boost);
        }
    }
    Ok(boosts)
}

/// Query the `playbook_citations` table and return a map of chunk_id → number of
/// distinct playbooks citing that chunk. Used by Layer 5 of feedback reranking.
fn get_playbook_cited_chunks(db: &Database) -> Result<HashMap<String, usize>, CoreError> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT chunk_id, COUNT(DISTINCT playbook_id) FROM playbook_citations GROUP BY chunk_id",
    )?;
    let mut map = HashMap::new();
    let rows = stmt.query_map([], |row| {
        let chunk_id: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((chunk_id, count as usize))
    })?;
    for row in rows {
        let (chunk_id, count) = row?;
        map.insert(chunk_id, count);
    }
    Ok(map)
}

/// Compute a credibility score for a document based on its path/source.
///
/// Returns a value in \[0.0, 1.0\]:
/// - Official/authoritative domains (gov, edu, org, major news) → 0.9–1.0
/// - Known tech sources (stackoverflow, github, MDN, docs sites) → 0.8–0.9
/// - General web pages → 0.5–0.7
/// - Local files → 0.7 (user-curated content)
/// - Unknown/low-quality → 0.3–0.5
fn compute_credibility(document_path: &str) -> f64 {
    let path_lower = document_path.to_lowercase();

    // Check for web URLs first.
    if path_lower.starts_with("http://") || path_lower.starts_with("https://") {
        // Official / authoritative domains
        if path_lower.contains(".gov")
            || path_lower.contains(".edu")
            || path_lower.contains("reuters.com")
            || path_lower.contains("apnews.com")
            || path_lower.contains("bbc.com")
            || path_lower.contains("nytimes.com")
        {
            return 0.95;
        }
        // Known tech sources
        if path_lower.contains("stackoverflow.com")
            || path_lower.contains("github.com")
            || path_lower.contains("developer.mozilla.org")
            || path_lower.contains("docs.rs")
            || path_lower.contains("docs.python.org")
            || path_lower.contains("docs.microsoft.com")
            || path_lower.contains("learn.microsoft.com")
            || path_lower.contains("dev.to")
        {
            return 0.85;
        }
        // Known general sources
        if path_lower.contains("wikipedia.org") || path_lower.contains(".org") {
            return 0.7;
        }
        // General web
        return 0.5;
    }

    // Local files — user-curated content.
    0.7
}

/// Compute freshness in days from a document date string, and return
/// `(freshness_days, freshness_bonus)`.
///
/// Bonus: within 30 days → +0.1, within 1 year → +0.05, older → 0.0.
fn compute_freshness(document_date: &Option<String>) -> (Option<i64>, f64) {
    let date_str = match document_date {
        Some(d) if !d.is_empty() => d,
        _ => return (None, 0.0),
    };
    // Try RFC 3339
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(date_str) {
        let days = chrono::Utc::now().signed_duration_since(ts).num_days();
        let bonus = if days <= 30 {
            0.1
        } else if days <= 365 {
            0.05
        } else {
            0.0
        };
        return (Some(days), bonus);
    }
    // Try YYYY-MM-DD
    if let Ok(naive) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let days = (chrono::Utc::now().date_naive() - naive).num_days();
        let bonus = if days <= 30 {
            0.1
        } else if days <= 365 {
            0.05
        } else {
            0.0
        };
        return (Some(days), bonus);
    }
    // Try SQLite datetime format
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        let days = chrono::Utc::now()
            .signed_duration_since(naive.and_utc())
            .num_days();
        let bonus = if days <= 30 {
            0.1
        } else if days <= 365 {
            0.05
        } else {
            0.0
        };
        return (Some(days), bonus);
    }
    (None, 0.0)
}

/// Enrich evidence cards with credibility and freshness scores, and blend
/// them into the final ranking score.
///
/// `final_score = bm25_score * 0.7 + credibility * 0.2 + freshness * 0.1`
fn apply_credibility_scoring(cards: &mut [EvidenceCard]) {
    for card in cards.iter_mut() {
        let credibility = compute_credibility(&card.document_path);
        let (freshness_days, freshness_bonus) = compute_freshness(&card.document_date);

        card.credibility = Some(credibility);
        card.freshness_days = freshness_days;

        card.score = card.score * 0.7 + credibility * 0.2 + freshness_bonus * 0.1;
    }
    cards.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Apply feedback-based score adjustments, document-level feedback propagation,
/// recency boost, source preference injection, playbook citation boost, and re-sort.
///
/// Layers applied:
/// 1. Direct chunk/query feedback (upvote +0.15, downvote −0.15, pin +0.25, clamped ±0.5)
/// 2. Document-level feedback propagation (+0.03*N/-0.02*N, only if no direct feedback)
/// 3. Recency boost based on document modified_at
/// 4. Source preference boost (+0.05 for preferred sources)
/// 5. Playbook citation boost (+0.10 single playbook, +0.15 multiple playbooks)
fn apply_feedback_reranking(
    cards: &mut [EvidenceCard],
    db: &Database,
    query_text: &str,
) -> Result<(), CoreError> {
    if cards.is_empty() {
        return Ok(());
    }
    let chunk_ids: Vec<String> = cards.iter().map(|c| c.chunk_id.to_string()).collect();

    // Layer 1: Direct chunk/query feedback
    let adjustments = db.get_feedback_adjustments(query_text, &chunk_ids)?;

    // Layer 2: Document-level feedback propagation
    let doc_adjustments = db.get_document_feedback_adjustments(&chunk_ids)?;

    // Layer 3: Recency boost
    let doc_ids: Vec<String> = cards.iter().map(|c| c.document_id.to_string()).collect();
    let recency_boosts = get_document_recency_boosts(db, &doc_ids)?;

    // Layer 4: Source preference
    let preferred_sources = personalization::get_preferred_source_paths(db, 5).unwrap_or_default();
    let preferred_names: Vec<String> = preferred_sources
        .iter()
        .map(|p| extract_source_name(p))
        .collect();

    // Layer 5: Playbook citation boost
    let playbook_cited = get_playbook_cited_chunks(db)?;

    for card in cards.iter_mut() {
        let id = card.chunk_id.to_string();
        let mut adj = adjustments.get(&id).copied().unwrap_or(0.0);

        // Only add doc-level boost if this chunk has no direct feedback
        if !adjustments.contains_key(&id) {
            adj += doc_adjustments.get(&id).copied().unwrap_or(0.0);
        }

        // Add recency boost
        let doc_id = card.document_id.to_string();
        adj += recency_boosts.get(&doc_id).copied().unwrap_or(0.0);

        // Add source preference boost
        if preferred_names.contains(&card.source_name) {
            adj += 0.05;
        }

        // Add playbook citation boost
        if let Some(&count) = playbook_cited.get(&id) {
            adj += if count > 1 { 0.15 } else { 0.10 };
        }

        if adj.abs() > 1e-9 {
            card.score = (card.score + adj).clamp(0.0, 1.0);
        }
    }
    cards.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(())
}

/// Build an FTS5 MATCH expression from raw user input.
///
/// Each whitespace-separated token is double-quoted to escape FTS5 special
/// characters. A trailing `*` on the last token is preserved for prefix
/// search (e.g. `"depl"*`).
fn build_fts_query(input: &str) -> String {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == tokens.len() - 1;
        if is_last && token.ends_with('*') {
            let base = &token[..token.len() - 1];
            if base.is_empty() {
                continue; // lone `*` — skip
            }
            // Prefix search: "term"*
            parts.push(format!("\"{}\"*", escape_fts_quotes(base)));
        } else {
            parts.push(format!("\"{}\"", escape_fts_quotes(token)));
        }
    }

    parts.join(" ")
}

/// Escape double-quotes inside a token so it can be safely wrapped in `"…"`.
fn escape_fts_quotes(s: &str) -> String {
    s.replace('"', "\"\"")
}

/// Extract search terms from the user query for highlight computation.
///
/// Strips trailing `*` from tokens and lowercases them.
fn extract_terms(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|t| t.trim_end_matches('*').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Find all case-insensitive occurrences of each term in `content`.
///
/// Returns highlights sorted by start position.
fn compute_highlights(content: &str, terms: &[String]) -> Vec<Highlight> {
    let mut highlights = Vec::new();
    let content_lower = content.to_lowercase();

    for term in terms {
        if term.is_empty() {
            continue;
        }
        let mut start = 0;
        while let Some(pos) = content_lower[start..].find(term.as_str()) {
            let abs_start = start + pos;
            let abs_end = abs_start + term.len();
            highlights.push(Highlight {
                start: abs_start,
                end: abs_end,
                term: term.clone(),
            });
            start = abs_end;
        }
    }

    highlights.sort_by_key(|h| h.start);
    highlights
}

/// Extract `heading_context` from the chunk's `metadata_json`.
///
/// The ingest module stores it as `{"heading_context":"Some Heading"}`.
/// We return it as a single-element `Vec<String>` (or empty if absent).
fn parse_heading_path(metadata_json: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Meta {
        heading_context: Option<String>,
    }

    serde_json::from_str::<Meta>(metadata_json)
        .ok()
        .and_then(|m| m.heading_context)
        .map(|h| vec![h])
        .unwrap_or_default()
}

/// Derive a human-readable source name from a root path.
///
/// Uses the last path component (directory name).
fn extract_source_name(root_path: &str) -> String {
    std::path::Path::new(root_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root_path.to_string())
}

/// Extract a document date from serialized metadata JSON.
///
/// Checks for `date`, `created`, `modified`, `fs_created_at`, `fs_modified_at`
/// keys (in that priority order) and returns the first found.
fn extract_document_date(metadata_json: &str) -> Option<String> {
    let map: HashMap<String, String> = serde_json::from_str(metadata_json).unwrap_or_default();
    map.get("date")
        .or_else(|| map.get("created"))
        .or_else(|| map.get("modified"))
        .or_else(|| map.get("fs_created_at"))
        .or_else(|| map.get("fs_modified_at"))
        .cloned()
}

/// Convert a [`FileType`] enum variant to the MIME string(s) stored in the DB.
fn file_type_to_mimes(ft: &FileType) -> Vec<String> {
    match ft {
        FileType::Markdown => vec!["text/markdown".to_string()],
        FileType::PlainText => vec!["text/plain".to_string()],
        FileType::Log => vec!["text/x-log".to_string()],
        FileType::Pdf => vec!["application/pdf".to_string()],
        FileType::Docx => {
            vec![
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_string(),
            ]
        }
        FileType::Excel => {
            vec!["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()]
        }
        FileType::Pptx => {
            vec![
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                    .to_string(),
            ]
        }
        FileType::Image => vec![
            "image/jpeg".to_string(),
            "image/png".to_string(),
            "image/gif".to_string(),
            "image/webp".to_string(),
        ],
        FileType::Video => {
            #[cfg(feature = "video")]
            {
                crate::video::VIDEO_MIME_TYPES
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }
            #[cfg(not(feature = "video"))]
            {
                vec![
                    "video/mp4".to_string(),
                    "video/webm".to_string(),
                    "video/quicktime".to_string(),
                    "video/x-matroska".to_string(),
                ]
            }
        }
        FileType::Audio => {
            #[cfg(feature = "video")]
            {
                crate::video::AUDIO_MIME_TYPES
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }
            #[cfg(not(feature = "video"))]
            {
                vec![
                    "audio/mpeg".to_string(),
                    "audio/wav".to_string(),
                    "audio/flac".to_string(),
                    "audio/ogg".to_string(),
                    "audio/aac".to_string(),
                    "audio/mp4".to_string(),
                    "audio/x-ms-wma".to_string(),
                    "audio/opus".to_string(),
                ]
            }
        }
    }
}

/// Perform a vector-only search using the saved TF-IDF embedder.
///
/// Returns `(chunk_id, cosine_similarity)` pairs sorted by similarity DESC.
/// Returns an empty vec when no embedder state or embeddings exist.
#[allow(dead_code)] // kept for tests; hybrid_search now uses vector_search_top_k
fn vector_search(
    db: &Database,
    query_text: &str,
    limit: usize,
) -> Result<Vec<(String, f32)>, CoreError> {
    let state = db.load_embedder_state("tfidf-v1")?;
    let (vocab, idf) = match state {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };

    let embedder = TfIdfEmbedder::from_vocabulary(vocab, idf);
    let query_vec = embedder.embed(query_text)?;

    // All-zero query vector means no recognizable terms.
    if query_vec.iter().all(|&v| v == 0.0) {
        return Ok(Vec::new());
    }

    let all_embeddings = db.get_all_embeddings("tfidf-v1")?;
    if all_embeddings.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: Vec<(String, f32)> = all_embeddings
        .into_iter()
        .map(|(chunk_id, vec)| {
            let sim = cosine_similarity(&query_vec, &vec);
            (chunk_id, sim)
        })
        .filter(|(_, sim)| *sim > 0.0)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

/// Reciprocal Rank Fusion merge of two ranked result lists.
///
/// `k` is the RRF constant (typically 60). Ranks are 1-indexed.
/// Returns `(chunk_id, rrf_score)` sorted by score DESC.
fn rrf_merge(
    fts_results: &[(String, f32)],
    vec_results: &[(String, f32)],
    k: f32,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (rank, (chunk_id, _)) in fts_results.iter().enumerate() {
        let r = (rank + 1) as f32;
        *scores.entry(chunk_id.clone()).or_insert(0.0) += 1.0 / (k + r);
    }

    for (rank, (chunk_id, _)) in vec_results.iter().enumerate() {
        let r = (rank + 1) as f32;
        *scores.entry(chunk_id.clone()).or_insert(0.0) += 1.0 / (k + r);
    }

    let mut merged: Vec<(String, f32)> = scores.into_iter().collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SearchFilters;
    use rusqlite::params;

    // ── helpers ──────────────────────────────────────────────────────────

    fn test_db() -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.save_embedder_config(&crate::embed::EmbedderConfig {
            provider: "tfidf".into(),
            ..crate::embed::EmbedderConfig::default()
        })
        .expect("set tfidf config for test");
        db
    }

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn insert_source(conn: &std::sync::MutexGuard<'_, rusqlite::Connection>) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', ?2)",
            params![&id, format!("/tmp/src-{}", &id[..8])],
        )
        .expect("insert source");
        id
    }

    fn insert_document(
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        source_id: &str,
        mime_type: &str,
    ) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'Test Doc', ?4, 1234, datetime('now'), ?5)",
            params![
                &id,
                source_id,
                format!("/tmp/doc-{}.md", &id[..8]),
                mime_type,
                format!("hash-{}", &id[..8]),
            ],
        )
        .expect("insert document");
        id
    }

    /// Auto-incrementing chunk index for tests with multiple chunks per document.
    static CHUNK_INDEX: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    fn insert_chunk(
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        document_id: &str,
        content: &str,
    ) -> String {
        let id = new_id();
        let idx = CHUNK_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content,
                                 start_offset, end_offset, line_start, line_end,
                                 content_hash, metadata_json)
             VALUES (?1, ?2, ?3, 'text', ?4, 0, ?5, 1, 10, ?6, '{}')",
            params![
                &id,
                document_id,
                idx,
                content,
                content.len() as i64,
                format!("hash-{}", &id[..8])
            ],
        )
        .expect("insert chunk");
        id
    }

    fn insert_chunk_with_heading(
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        document_id: &str,
        content: &str,
        heading: &str,
    ) -> String {
        let id = new_id();
        let idx = CHUNK_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let metadata = format!(
            r#"{{"heading_context":{}}}"#,
            serde_json::to_string(heading).unwrap()
        );
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content,
                                 start_offset, end_offset, line_start, line_end,
                                 content_hash, metadata_json)
             VALUES (?1, ?2, ?3, 'text', ?4, 0, ?5, 1, 10, ?6, ?7)",
            params![
                &id,
                document_id,
                idx,
                content,
                content.len() as i64,
                format!("hash-{}", &id[..8]),
                &metadata
            ],
        )
        .expect("insert chunk");
        id
    }

    fn default_query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.to_string(),
            filters: SearchFilters::default(),
            limit: 20,
            offset: 0,
        }
    }

    // ── tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_basic_fts_search() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            insert_chunk(&conn, &did, "the quick brown fox jumps over the lazy dog");
        }

        let result = search(&db, &default_query("quick")).unwrap();
        assert_eq!(result.total_matches, 1);
        assert!(result.evidence_cards[0].content.contains("quick"));
        assert!(result.evidence_cards[0].score > 0.0);
    }

    #[test]
    fn test_search_multiple_results() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/plain");
            insert_chunk(&conn, &did, "rust is a systems programming language");
            insert_chunk(
                &conn,
                &did,
                "rust guarantees memory safety without garbage collection",
            );
            insert_chunk(&conn, &did, "python is an interpreted language");
        }

        let result = search(&db, &default_query("rust")).unwrap();
        assert_eq!(result.total_matches, 2);
    }

    #[test]
    fn test_search_no_results() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/plain");
            insert_chunk(&conn, &did, "hello world from test document");
        }

        let result = search(&db, &default_query("nonexistentxyzterm")).unwrap();
        assert_eq!(result.total_matches, 0);
        assert!(result.evidence_cards.is_empty());
    }

    #[test]
    fn test_search_empty_query() {
        let db = test_db();

        let result = search(&db, &default_query("")).unwrap();
        assert_eq!(result.total_matches, 0);
        assert!(result.evidence_cards.is_empty());

        let result = search(&db, &default_query("   ")).unwrap();
        assert_eq!(result.total_matches, 0);
    }

    #[test]
    fn test_make_snippet_preserves_utf8_boundaries() {
        let content = "测试条目甲乙丙丁戊己庚辛壬癸".repeat(20);

        let snippet = make_snippet(&content).expect("snippet should be generated");

        assert!(snippet.ends_with('…'));
        assert!(snippet.is_char_boundary(snippet.len() - '…'.len_utf8()));
        assert!(!snippet.contains('\u{fffd}'));
    }

    #[test]
    fn test_search_filter_by_source_id() {
        let db = test_db();
        let sid1;
        let sid2;
        {
            let conn = db.conn();
            sid1 = insert_source(&conn);
            sid2 = insert_source(&conn);
            let did1 = insert_document(&conn, &sid1, "text/plain");
            let did2 = insert_document(&conn, &sid2, "text/plain");
            insert_chunk(&conn, &did1, "deploy the application to production server");
            insert_chunk(&conn, &did2, "deploy the service to staging server");
        }

        let mut query = default_query("deploy");
        query.filters.source_ids = vec![Uuid::parse_str(&sid1).unwrap()];

        let result = search(&db, &query).unwrap();
        assert_eq!(result.total_matches, 1);
        assert!(result.evidence_cards[0].content.contains("production"));
    }

    #[test]
    fn test_search_filter_by_file_type() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did_md = insert_document(&conn, &sid, "text/markdown");
            let did_txt = insert_document(&conn, &sid, "text/plain");
            insert_chunk(&conn, &did_md, "kubernetes deployment configuration guide");
            insert_chunk(&conn, &did_txt, "kubernetes cluster monitoring setup");
        }

        let mut query = default_query("kubernetes");
        query.filters.file_types = vec![FileType::Markdown];

        let result = search(&db, &query).unwrap();
        assert_eq!(result.total_matches, 1);
        assert!(result.evidence_cards[0].content.contains("deployment"));
    }

    #[test]
    fn test_highlight_generation() {
        let content = "The Rust programming language is blazingly fast. Rust is safe.";
        let terms = vec!["rust".to_string()];
        let highlights = compute_highlights(content, &terms);

        assert_eq!(highlights.len(), 2);
        assert_eq!(highlights[0].start, 4);
        assert_eq!(highlights[0].end, 8);
        assert_eq!(highlights[0].term, "rust");
        assert_eq!(highlights[1].start, 49);
        assert_eq!(highlights[1].end, 53);
    }

    #[test]
    fn test_highlight_case_insensitive() {
        let content = "RUST and rust and Rust";
        let terms = vec!["rust".to_string()];
        let highlights = compute_highlights(content, &terms);

        assert_eq!(highlights.len(), 3);
    }

    #[test]
    fn test_highlight_multiple_terms() {
        let content = "the quick brown fox jumps over the lazy dog";
        let terms = vec!["quick".to_string(), "fox".to_string()];
        let highlights = compute_highlights(content, &terms);

        assert_eq!(highlights.len(), 2);
        // sorted by start
        assert_eq!(highlights[0].term, "quick");
        assert_eq!(highlights[1].term, "fox");
    }

    #[test]
    fn test_get_evidence_card() {
        let db = test_db();
        let chunk_id;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            chunk_id =
                insert_chunk_with_heading(&conn, &did, "evidence card test content", "Setup");
        }

        let card = get_evidence_card(&db, &chunk_id).unwrap();
        assert_eq!(card.chunk_id.to_string(), chunk_id);
        assert_eq!(card.content, "evidence card test content");
        assert_eq!(card.heading_path, vec!["Setup".to_string()]);
        assert_eq!(card.document_title, "Test Doc");
    }

    #[test]
    fn test_get_evidence_card_not_found() {
        let db = test_db();
        let result = get_evidence_card(&db, "nonexistent-id");
        assert!(result.is_err());
        match result.unwrap_err() {
            CoreError::NotFound(msg) => assert!(msg.contains("nonexistent-id")),
            other => panic!("expected NotFound, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_fts_query_basic() {
        assert_eq!(build_fts_query("hello world"), r#""hello" "world""#);
    }

    #[test]
    fn test_build_fts_query_prefix() {
        assert_eq!(build_fts_query("depl*"), r#""depl"*"#);
        assert_eq!(build_fts_query("hello depl*"), r#""hello" "depl"*"#);
    }

    #[test]
    fn test_build_fts_query_special_chars() {
        assert_eq!(build_fts_query("c++"), r#""c++""#);
        assert_eq!(build_fts_query("NOT this"), r#""NOT" "this""#);
    }

    #[test]
    fn test_extract_source_name() {
        assert_eq!(extract_source_name("/home/user/notes"), "notes");
        assert_eq!(extract_source_name("/tmp/src-abc"), "src-abc");
    }

    #[test]
    fn test_search_with_limit() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/plain");
            for i in 0..5 {
                let id = new_id();
                conn.execute(
                    "INSERT INTO chunks (id, document_id, chunk_index, kind, content,
                                         start_offset, end_offset, line_start, line_end,
                                         content_hash, metadata_json)
                     VALUES (?1, ?2, ?3, 'text', ?4, 0, 100, 1, 10, ?5, '{}')",
                    params![
                        &id,
                        &did,
                        i,
                        format!("searchable term number {i}"),
                        format!("hash{i}"),
                    ],
                )
                .expect("insert chunks");
            }
        }

        let mut query = default_query("searchable");
        query.limit = 2;

        let result = search(&db, &query).unwrap();
        assert_eq!(result.evidence_cards.len(), 2);
    }

    #[test]
    fn test_search_time_recorded() {
        let db = test_db();
        let result = search(&db, &default_query("anything")).unwrap();
        // search_time_ms should be set (may be 0 if very fast, but field exists)
        assert!(result.search_time_ms < 5000);
    }

    #[test]
    fn test_search_result_has_highlights() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            insert_chunk(&conn, &did, "Rust is fast and Rust is safe");
        }

        let result = search(&db, &default_query("rust")).unwrap();
        assert_eq!(result.total_matches, 1);

        let card = &result.evidence_cards[0];
        assert_eq!(card.highlights.len(), 2);
        assert_eq!(card.highlights[0].term, "rust");
    }

    #[test]
    fn test_search_lone_star_does_not_crash() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/plain");
            insert_chunk(&conn, &did, "test content for star search");
        }
        // A lone "*" now produces an empty FTS query → returns empty result
        let result = search(&db, &default_query("*")).unwrap();
        assert_eq!(result.total_matches, 0);
        assert!(result.evidence_cards.is_empty());
    }

    // ── RRF merge tests ─────────────────────────────────────────────

    #[test]
    fn test_rrf_merge_basic() {
        let fts = vec![
            ("a".to_string(), 10.0),
            ("b".to_string(), 8.0),
            ("c".to_string(), 6.0),
        ];
        let vec = vec![
            ("b".to_string(), 0.9),
            ("d".to_string(), 0.8),
            ("a".to_string(), 0.7),
        ];
        let merged = rrf_merge(&fts, &vec, 60.0);

        // All 4 unique IDs present.
        assert_eq!(merged.len(), 4);

        let score_map: HashMap<String, f32> = merged.into_iter().collect();
        let score_a = score_map["a"]; // FTS rank 1 + vec rank 3
        let score_b = score_map["b"]; // FTS rank 2 + vec rank 1
        let score_c = score_map["c"]; // FTS rank 3 only
        let score_d = score_map["d"]; // vec rank 2 only

        // Items in both lists beat items in only one.
        assert!(score_b > score_c);
        assert!(score_a > score_d);
        assert!(score_b > score_d);
        assert!(score_a > score_c);
    }

    #[test]
    fn test_rrf_merge_disjoint() {
        let fts = vec![("a".to_string(), 10.0), ("b".to_string(), 8.0)];
        let vec = vec![("c".to_string(), 0.9), ("d".to_string(), 0.8)];
        let merged = rrf_merge(&fts, &vec, 60.0);

        assert_eq!(merged.len(), 4);
        let score_map: HashMap<String, f32> = merged.into_iter().collect();
        // Same-rank items from different lists get the same score.
        assert!((score_map["a"] - score_map["c"]).abs() < 1e-6);
        assert!((score_map["b"] - score_map["d"]).abs() < 1e-6);
    }

    #[test]
    fn test_rrf_merge_empty_inputs() {
        let fts: Vec<(String, f32)> = vec![];
        let vec: Vec<(String, f32)> = vec![];
        let merged = rrf_merge(&fts, &vec, 60.0);
        assert!(merged.is_empty());
    }

    // ── Hybrid search tests ─────────────────────────────────────────

    #[test]
    fn test_hybrid_search_empty_query() {
        let db = test_db();
        let result = hybrid_search(&db, &default_query("")).unwrap();
        assert_eq!(result.total_matches, 0);
        assert!(result.evidence_cards.is_empty());
        assert_eq!(result.search_mode, "hybrid");
    }

    #[test]
    fn test_hybrid_search_fallback_no_embeddings() {
        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            insert_chunk(&conn, &did, "rust is a great programming language");
        }

        // No embeddings stored → should fall back to FTS.
        let result = hybrid_search(&db, &default_query("rust")).unwrap();
        assert_eq!(result.search_mode, "fts");
        assert!(!result.evidence_cards.is_empty());
        assert!(result.evidence_cards[0].content.contains("rust"));
    }

    #[test]
    fn test_hybrid_search_with_embeddings() {
        let db = test_db();
        let chunk_id_1;
        let chunk_id_2;
        let chunk_id_3;
        // Extra chunks so "rust" (in 2/7 docs) gets non-zero IDF.
        let chunk_id_4;
        let chunk_id_5;
        let chunk_id_6;
        let chunk_id_7;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            chunk_id_1 = insert_chunk(&conn, &did, "rust programming language systems");
            chunk_id_2 = insert_chunk(&conn, &did, "python scripting dynamic typing");
            chunk_id_3 = insert_chunk(&conn, &did, "rust compiler memory safety performance");
            chunk_id_4 = insert_chunk(
                &conn,
                &did,
                "javascript web frontend development frameworks",
            );
            chunk_id_5 = insert_chunk(&conn, &did, "database sql query optimization indexes");
            chunk_id_6 = insert_chunk(&conn, &did, "networking protocols http server requests");
            chunk_id_7 = insert_chunk(&conn, &did, "testing integration unit coverage reports");
        }

        // Build embedder from corpus and persist state + embeddings.
        let corpus: Vec<&str> = vec![
            "rust programming language systems",
            "python scripting dynamic typing",
            "rust compiler memory safety performance",
            "javascript web frontend development frameworks",
            "database sql query optimization indexes",
            "networking protocols http server requests",
            "testing integration unit coverage reports",
        ];
        let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
        db.save_embedder_state("tfidf-v1", &embedder.vocabulary, &embedder.idf)
            .unwrap();

        for (cid, text) in [
            (&chunk_id_1, corpus[0]),
            (&chunk_id_2, corpus[1]),
            (&chunk_id_3, corpus[2]),
            (&chunk_id_4, corpus[3]),
            (&chunk_id_5, corpus[4]),
            (&chunk_id_6, corpus[5]),
            (&chunk_id_7, corpus[6]),
        ] {
            let v = embedder.embed(text).unwrap();
            db.store_embedding(cid, "tfidf-v1", &v).unwrap();
        }

        let result = hybrid_search(&db, &default_query("rust")).unwrap();
        assert_eq!(result.search_mode, "hybrid");
        assert!(!result.evidence_cards.is_empty());

        // Rust-related chunks should appear in the results.
        let ids: Vec<String> = result
            .evidence_cards
            .iter()
            .map(|c| c.chunk_id.to_string())
            .collect();
        assert!(
            ids.contains(&chunk_id_1) || ids.contains(&chunk_id_3),
            "expected at least one rust chunk in results"
        );
    }

    #[test]
    fn test_hybrid_search_includes_both_sources() {
        let db = test_db();
        let chunk_fts;
        let chunk_vec;
        let chunk_3;
        let chunk_4;
        let chunk_5;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            chunk_fts = insert_chunk(&conn, &did, "deploy application to production server");
            chunk_vec = insert_chunk(&conn, &did, "release software to live environment");
            // Extra chunks so "deploy" (in 1/5 docs) gets non-zero IDF.
            chunk_3 = insert_chunk(&conn, &did, "database schema migration strategy planning");
            chunk_4 = insert_chunk(&conn, &did, "monitoring alerts dashboards observability");
            chunk_5 = insert_chunk(&conn, &did, "authentication security oauth tokens sessions");
        }

        let corpus: Vec<&str> = vec![
            "deploy application to production server",
            "release software to live environment",
            "database schema migration strategy planning",
            "monitoring alerts dashboards observability",
            "authentication security oauth tokens sessions",
        ];
        let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
        db.save_embedder_state("tfidf-v1", &embedder.vocabulary, &embedder.idf)
            .unwrap();

        for (cid, text) in [
            (&chunk_fts, corpus[0]),
            (&chunk_vec, corpus[1]),
            (&chunk_3, corpus[2]),
            (&chunk_4, corpus[3]),
            (&chunk_5, corpus[4]),
        ] {
            let v = embedder.embed(text).unwrap();
            db.store_embedding(cid, "tfidf-v1", &v).unwrap();
        }

        let result = hybrid_search(&db, &default_query("deploy")).unwrap();
        assert_eq!(result.search_mode, "hybrid");

        // The FTS-matched chunk must always be present.
        let ids: Vec<String> = result
            .evidence_cards
            .iter()
            .map(|c| c.chunk_id.to_string())
            .collect();
        assert!(
            ids.contains(&chunk_fts),
            "FTS-matched chunk must be in hybrid results"
        );
    }

    #[test]
    fn test_vector_search_no_embedder_state() {
        let db = test_db();
        let result = vector_search(&db, "test query", 50).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_vector_search_top_k_basic() {
        let db = test_db();
        let chunk_ids;
        let embedder;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            let texts = vec![
                "rust programming language systems",
                "python scripting dynamic typing",
                "javascript web frontend development",
                "database sql query optimization",
                "networking protocols http server",
            ];
            let mut ids = Vec::new();
            for text in &texts {
                ids.push(insert_chunk(&conn, &did, text));
            }
            chunk_ids = ids;
            let refs: Vec<&str> = texts.iter().copied().collect();
            embedder = TfIdfEmbedder::build_from_corpus(&refs);
        }

        // Store embeddings.
        db.save_embedder_state("tfidf-v1", &embedder.vocabulary, &embedder.idf)
            .unwrap();
        let corpus = [
            "rust programming language systems",
            "python scripting dynamic typing",
            "javascript web frontend development",
            "database sql query optimization",
            "networking protocols http server",
        ];
        for (cid, text) in chunk_ids.iter().zip(corpus.iter()) {
            let v = embedder.embed(text).unwrap();
            db.store_embedding(cid, "tfidf-v1", &v).unwrap();
        }

        let query_vec = embedder.embed("rust systems programming").unwrap();
        let results = vector_search_top_k(&db, &query_vec, "tfidf-v1", 3, None).unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 3);

        // Results should be sorted by similarity DESC.
        for i in 1..results.len() {
            assert!(results[i - 1].1 >= results[i].1);
        }

        // First result should be the rust-related chunk.
        assert_eq!(results[0].0, chunk_ids[0]);
    }

    #[test]
    fn test_vector_search_top_k_empty() {
        let db = test_db();
        let zero_vec = vec![0.0; 10];
        let results = vector_search_top_k(&db, &zero_vec, "tfidf-v1", 5, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_vector_search_top_k_zero_k() {
        let db = test_db();
        let query = vec![1.0, 0.0];
        let results = vector_search_top_k(&db, &query, "tfidf-v1", 0, None).unwrap();
        assert!(results.is_empty());
    }

    // ── Feedback re-ranking tests ───────────────────────────────────

    #[test]
    fn test_feedback_boosts_search_ranking() {
        use crate::feedback::FeedbackAction;

        let db = test_db();
        let chunk_b;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            let _chunk_a = insert_chunk(&conn, &did, "deploy application to production server");
            chunk_b = insert_chunk(&conn, &did, "deploy service to staging environment");
        }

        // Search without feedback — record baseline score for chunk_b.
        let result_before = search(&db, &default_query("deploy")).unwrap();
        assert_eq!(result_before.evidence_cards.len(), 2);
        let score_b_before = result_before
            .evidence_cards
            .iter()
            .find(|c| c.chunk_id.to_string() == chunk_b)
            .unwrap()
            .score;

        // Upvote + pin chunk_b (+0.15 + 0.25 = +0.40 adjustment).
        db.add_feedback(&chunk_b, "deploy", FeedbackAction::Upvote)
            .unwrap();
        db.add_feedback(&chunk_b, "deploy", FeedbackAction::Pin)
            .unwrap();

        // Search again — chunk_b should have a higher score.
        let result_after = search(&db, &default_query("deploy")).unwrap();
        assert_eq!(result_after.evidence_cards.len(), 2);
        let score_b_after = result_after
            .evidence_cards
            .iter()
            .find(|c| c.chunk_id.to_string() == chunk_b)
            .unwrap()
            .score;

        assert!(
            score_b_after > score_b_before,
            "upvoted chunk score should increase: before={score_b_before}, after={score_b_after}"
        );

        // chunk_b should now rank first (it received a boost, chunk_a did not).
        assert_eq!(
            result_after.evidence_cards[0].chunk_id.to_string(),
            chunk_b,
            "upvoted chunk should rank first"
        );
    }

    #[test]
    fn test_feedback_penalizes_downvoted() {
        use crate::feedback::FeedbackAction;

        let db = test_db();
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            insert_chunk(&conn, &did, "configure server networking rules");
            insert_chunk(&conn, &did, "configure network firewall settings");
        }

        // Search without feedback — record baseline score for chunk_a.
        let result_before = search(&db, &default_query("configure")).unwrap();
        assert_eq!(result_before.evidence_cards.len(), 2);
        let first_id_before = result_before.evidence_cards[0].chunk_id.to_string();
        let score_first_before = result_before.evidence_cards[0].score;

        // Downvote whichever chunk ranked first.
        db.add_feedback(&first_id_before, "configure", FeedbackAction::Downvote)
            .unwrap();
        db.add_feedback(&first_id_before, "configure", FeedbackAction::Downvote)
            .unwrap();

        // Search again — the downvoted chunk's score should decrease.
        let result_after = search(&db, &default_query("configure")).unwrap();
        let score_first_after = result_after
            .evidence_cards
            .iter()
            .find(|c| c.chunk_id.to_string() == first_id_before)
            .unwrap()
            .score;

        assert!(
            score_first_after < score_first_before,
            "downvoted chunk score should decrease: before={score_first_before}, after={score_first_after}"
        );
    }

    #[test]
    fn test_apply_feedback_reranking_direct() {
        use crate::feedback::FeedbackAction;

        let db = test_db();
        let chunk_a;
        let chunk_b;
        let chunk_c;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            chunk_a = insert_chunk(&conn, &did, "alpha content");
            chunk_b = insert_chunk(&conn, &did, "beta content");
            chunk_c = insert_chunk(&conn, &did, "gamma content");
        }

        // Seed feedback for query "test".
        db.add_feedback(&chunk_c, "test", FeedbackAction::Pin)
            .unwrap(); // +0.25
        db.add_feedback(&chunk_a, "test", FeedbackAction::Downvote)
            .unwrap(); // -0.15

        // Build cards with known scores.
        let mut cards = vec![
            EvidenceCard {
                chunk_id: Uuid::parse_str(&chunk_a).unwrap(),
                document_id: Uuid::nil(),
                source_id: Uuid::nil(),
                source_name: String::new(),
                document_path: String::new(),
                document_title: String::new(),
                content: String::new(),
                heading_path: Vec::new(),
                score: 0.80,
                highlights: Vec::new(),
                snippet: None,
                document_date: None,
                credibility: None,
                freshness_days: None,
            },
            EvidenceCard {
                chunk_id: Uuid::parse_str(&chunk_b).unwrap(),
                document_id: Uuid::nil(),
                source_id: Uuid::nil(),
                source_name: String::new(),
                document_path: String::new(),
                document_title: String::new(),
                content: String::new(),
                heading_path: Vec::new(),
                score: 0.50,
                highlights: Vec::new(),
                snippet: None,
                document_date: None,
                credibility: None,
                freshness_days: None,
            },
            EvidenceCard {
                chunk_id: Uuid::parse_str(&chunk_c).unwrap(),
                document_id: Uuid::nil(),
                source_id: Uuid::nil(),
                source_name: String::new(),
                document_path: String::new(),
                document_title: String::new(),
                content: String::new(),
                heading_path: Vec::new(),
                score: 0.40,
                highlights: Vec::new(),
                snippet: None,
                document_date: None,
                credibility: None,
                freshness_days: None,
            },
        ];

        apply_feedback_reranking(&mut cards, &db, "test").unwrap();

        // chunk_a: 0.80 + (-0.15) = 0.65
        // chunk_b: 0.50 + doc-level(0.03*1 - 0.02*1 = 0.01) = 0.51
        //   (doc has 1 pin on chunk_c + 1 downvote on chunk_a; chunk_b has no direct feedback)
        // chunk_c: 0.40 + 0.25 = 0.65
        // Sorted DESC: chunk_a(0.65), chunk_c(0.65), then chunk_b(0.51)
        assert!(
            (cards[0].score - 0.65).abs() < 1e-6,
            "first score: {}",
            cards[0].score
        );
        assert!(
            (cards[1].score - 0.65).abs() < 1e-6,
            "second score: {}",
            cards[1].score
        );
        assert!(
            (cards[2].score - 0.51).abs() < 1e-6,
            "third score: {}",
            cards[2].score
        );
        // chunk_b should be last
        assert_eq!(cards[2].chunk_id.to_string(), chunk_b);
    }

    #[test]
    fn test_feedback_adjustment_clamped_to_half() {
        use crate::feedback::FeedbackAction;

        let db = test_db();
        let chunk_id;
        {
            let conn = db.conn();
            let sid = insert_source(&conn);
            let did = insert_document(&conn, &sid, "text/markdown");
            chunk_id = insert_chunk(&conn, &did, "clamp test content");
        }

        // 4 upvotes = 4 * 0.15 = 0.60 → clamped to 0.50
        for _ in 0..4 {
            db.add_feedback(&chunk_id, "clamp", FeedbackAction::Upvote)
                .unwrap();
        }

        let adjustments = db
            .get_feedback_adjustments("clamp", &[chunk_id.clone()])
            .unwrap();
        let adj = adjustments.get(&chunk_id).copied().unwrap_or(0.0);
        assert!(
            (adj - 0.5).abs() < 1e-6,
            "adjustment should be clamped to 0.5, got {adj}"
        );
    }

    #[test]
    fn test_recency_boost_values() {
        use chrono::{Duration, Utc};

        // Less than 3 days old → +0.08
        let recent = (Utc::now() - Duration::hours(24)).to_rfc3339();
        assert!(
            (recency_boost(&recent) - 0.08).abs() < 1e-6,
            "1-day-old doc"
        );

        // 5 days old → +0.05
        let five_days = (Utc::now() - Duration::days(5)).to_rfc3339();
        assert!(
            (recency_boost(&five_days) - 0.05).abs() < 1e-6,
            "5-day-old doc"
        );

        // 15 days old → +0.02
        let fifteen_days = (Utc::now() - Duration::days(15)).to_rfc3339();
        assert!(
            (recency_boost(&fifteen_days) - 0.02).abs() < 1e-6,
            "15-day-old doc"
        );

        // 60 days old → 0.0
        let old = (Utc::now() - Duration::days(60)).to_rfc3339();
        assert!((recency_boost(&old)).abs() < 1e-6, "60-day-old doc");

        // Invalid date → 0.0
        assert!((recency_boost("not-a-date")).abs() < 1e-6, "invalid date");
    }
}
