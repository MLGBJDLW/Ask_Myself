//! Search module — query execution and result ranking.

use std::time::Instant;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::{EvidenceCard, FileType, Highlight, SearchQuery};

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
        });
    }

    let fts_query = build_fts_query(trimmed);
    if fts_query.is_empty() {
        return Ok(SearchResult {
            query: query.text.clone(),
            total_matches: 0,
            evidence_cards: Vec::new(),
            search_time_ms: start.elapsed().as_millis() as u64,
        });
    }
    let limit = if query.limit == 0 { 20 } else { query.limit };
    let terms = extract_terms(trimmed);

    // -- build dynamic SQL ------------------------------------------------

    let mut sql = String::from(
        "SELECT c.id, c.document_id, c.content, c.chunk_index, c.metadata_json,
                d.path, d.title, d.source_id, s.root_path,
                fts.rank
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
        let placeholders: Vec<String> = filters
            .file_types
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", param_idx + i))
            .collect();
        sql.push_str(&format!(" AND d.mime_type IN ({})", placeholders.join(",")));
        for ft in &filters.file_types {
            param_values.push(Box::new(file_type_to_mime(ft)));
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
    param_values.push(Box::new(limit as i64));
    param_idx += 1;

    sql.push_str(&format!(" OFFSET ?{}", param_idx));
    param_values.push(Box::new(query.offset as i64));

    // -- execute ----------------------------------------------------------

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

            let heading_path = parse_heading_path(&metadata_json);
            let source_name = extract_source_name(&source_root);
            let highlights = compute_highlights(&content, &terms);

            Ok(EvidenceCard {
                chunk_id: Uuid::parse_str(&chunk_id).unwrap_or_default(),
                document_id: Uuid::parse_str(&document_id).unwrap_or_default(),
                source_name,
                document_path: doc_path,
                document_title: doc_title.unwrap_or_default(),
                content,
                heading_path,
                score: -rank, // negate: FTS5 BM25 is negative
                highlights,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Count total matches (without LIMIT/OFFSET) for accurate pagination info.
    let total_matches = if cards.len() < limit as usize && query.offset == 0 {
        // If we got fewer results than the limit on the first page, total = len.
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
            let placeholders: Vec<String> = filters
                .file_types
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", cp_idx + i))
                .collect();
            count_sql.push_str(&format!(" AND d.mime_type IN ({})", placeholders.join(",")));
            for ft in &filters.file_types {
                count_params.push(Box::new(file_type_to_mime(ft)));
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
        conn.query_row(&count_sql, count_refs.as_slice(), |row| row.get::<_, usize>(0))
            .unwrap_or(cards.len())
    };

    Ok(SearchResult {
        query: query.text.clone(),
        total_matches,
        evidence_cards: cards,
        search_time_ms: start.elapsed().as_millis() as u64,
    })
}

/// Retrieve a single evidence card by chunk ID (for playbook citation lookups).
pub fn get_evidence_card(db: &Database, chunk_id: &str) -> Result<EvidenceCard, CoreError> {
    let conn = db.conn();
    conn.query_row(
        "SELECT c.id, c.document_id, c.content, c.chunk_index, c.metadata_json,
                d.path, d.title, d.source_id, s.root_path
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

            Ok(EvidenceCard {
                chunk_id: Uuid::parse_str(&cid).unwrap_or_default(),
                document_id: Uuid::parse_str(&did).unwrap_or_default(),
                source_name: extract_source_name(&source_root),
                document_path: doc_path,
                document_title: doc_title.unwrap_or_default(),
                content,
                heading_path: parse_heading_path(&metadata_json),
                score: 0.0,
                highlights: Vec::new(),
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

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

/// Convert a [`FileType`] enum variant to the MIME string stored in the DB.
fn file_type_to_mime(ft: &FileType) -> String {
    match ft {
        FileType::Markdown => "text/markdown".to_string(),
        FileType::PlainText => "text/plain".to_string(),
        FileType::Log => "text/x-log".to_string(),
    }
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
        Database::open_memory().expect("open in-memory db")
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
            params![&id, document_id, idx, content, content.len() as i64, format!("hash-{}", &id[..8])],
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
            params![&id, document_id, idx, content, content.len() as i64, format!("hash-{}", &id[..8]), &metadata],
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
            insert_chunk(&conn, &did, "rust guarantees memory safety without garbage collection");
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
}
