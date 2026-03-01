//! Answer caching — stores recent LLM answers to avoid redundant ReAct loops.
//!
//! When a user asks the same question twice, the cached answer is returned
//! directly, skipping the full ReAct loop and saving LLM costs.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

/// Default cache TTL in hours.
const DEFAULT_CACHE_TTL_HOURS: i64 = 24;

/// A cached answer retrieved from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedAnswer {
    pub id: String,
    pub query_hash: String,
    pub query_text: String,
    pub answer_text: String,
    pub citations: Vec<String>,
    pub source_filter: String,
    pub created_at: String,
    pub hit_count: i64,
}

/// Normalize a query for cache key purposes.
///
/// Lowercases, trims whitespace, collapses internal whitespace,
/// and removes trailing punctuation.
pub fn normalize_query(query: &str) -> String {
    let collapsed: String = query.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .to_lowercase()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_string()
}

/// Hash a normalized query using BLAKE3.
pub fn hash_query(normalized: &str) -> String {
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

/// Extract `[cite:CHUNK_ID]` references from an answer text.
pub fn extract_citations(text: &str) -> Vec<String> {
    let mut citations = Vec::new();
    let mut pos = 0;
    while let Some(start) = text[pos..].find("[cite:") {
        let abs_start = pos + start + 6; // skip "[cite:"
        if abs_start >= text.len() {
            break;
        }
        if let Some(end) = text[abs_start..].find(']') {
            let inner = &text[abs_start..abs_start + end];
            // inner might be "UUID" or "UUID|description"
            let uuid_part = inner.split('|').next().unwrap_or(inner).trim();
            if !uuid_part.is_empty() && !citations.contains(&uuid_part.to_string()) {
                citations.push(uuid_part.to_string());
            }
            pos = abs_start + end + 1;
        } else {
            break;
        }
    }
    citations
}

impl Database {
    /// Look up a cached answer for the given query.
    ///
    /// Returns `None` if no cache entry exists or the entry is older than
    /// `cache_ttl_hours` (defaults to 24).
    pub fn find_cached_answer(
        &self,
        query: &str,
        source_filter: Option<&str>,
        cache_ttl_hours: Option<i64>,
    ) -> Result<Option<CachedAnswer>, CoreError> {
        let normalized = normalize_query(query);
        if normalized.is_empty() {
            return Ok(None);
        }
        let qhash = hash_query(&normalized);
        let ttl = cache_ttl_hours.unwrap_or(DEFAULT_CACHE_TTL_HOURS);
        let filter_str = source_filter.unwrap_or("");

        let conn = self.conn();
        let result = conn.query_row(
            "SELECT id, query_hash, query_text, answer_text, citations,
                    source_filter, created_at, hit_count
             FROM answer_cache
             WHERE query_hash = ?1
               AND source_filter = ?2
               AND datetime(created_at, '+' || ?3 || ' hours') > datetime('now')",
            params![&qhash, filter_str, ttl],
            |row| {
                let citations_json: String = row.get(4)?;
                let citations: Vec<String> =
                    serde_json::from_str(&citations_json).unwrap_or_default();
                Ok(CachedAnswer {
                    id: row.get(0)?,
                    query_hash: row.get(1)?,
                    query_text: row.get(2)?,
                    answer_text: row.get(3)?,
                    citations,
                    source_filter: row.get(5)?,
                    created_at: row.get(6)?,
                    hit_count: row.get(7)?,
                })
            },
        );

        match result {
            Ok(cached) => Ok(Some(cached)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    /// Store an answer in the cache.
    ///
    /// If an entry with the same `(query_hash, source_filter)` already exists,
    /// it is replaced (upsert via `INSERT OR REPLACE`).
    pub fn cache_answer(
        &self,
        query: &str,
        answer: &str,
        citations: &[String],
        source_filter: Option<&str>,
    ) -> Result<(), CoreError> {
        let normalized = normalize_query(query);
        if normalized.is_empty() {
            return Ok(());
        }
        let qhash = hash_query(&normalized);
        let id = uuid::Uuid::new_v4().to_string();
        let citations_json = serde_json::to_string(citations).unwrap_or_else(|_| "[]".to_string());
        let filter_str = source_filter.unwrap_or("");

        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO answer_cache
                (id, query_hash, query_text, answer_text, citations,
                 source_filter, created_at, hit_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), 0)",
            params![&id, &qhash, query, answer, &citations_json, filter_str],
        )?;
        Ok(())
    }

    /// Increment the hit count for a cached answer.
    pub fn increment_cache_hit(&self, cache_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE answer_cache SET hit_count = hit_count + 1 WHERE id = ?1",
            params![cache_id],
        )?;
        Ok(())
    }

    /// Clear the entire answer cache.
    pub fn clear_answer_cache(&self) -> Result<usize, CoreError> {
        let conn = self.conn();
        let count = conn.execute("DELETE FROM answer_cache", [])?;
        Ok(count)
    }

    /// Invalidate cache entries that may reference chunks from the given source.
    ///
    /// Deletes entries where:
    /// - `source_filter` is empty (all-sources queries may include any source)
    /// - `source_filter` contains the given `source_id`
    pub fn invalidate_cache_for_source(&self, source_id: &str) -> Result<usize, CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM answer_cache
             WHERE source_filter = ''
                OR instr(source_filter, ?1) > 0",
            params![source_id],
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_query() {
        assert_eq!(normalize_query("  Hello  World  "), "hello world");
        assert_eq!(normalize_query("what is OAuth?"), "what is oauth");
        assert_eq!(normalize_query("SEARCH TERM"), "search term");
        assert_eq!(normalize_query(""), "");
        assert_eq!(normalize_query("  "), "");
    }

    #[test]
    fn test_hash_query_deterministic() {
        let h1 = hash_query("hello world");
        let h2 = hash_query("hello world");
        assert_eq!(h1, h2);
        assert_ne!(h1, hash_query("hello world!"));
    }

    #[test]
    fn test_extract_citations() {
        let text = "Based on [cite:abc-123] and [cite:def-456|some desc], the answer is...";
        let cites = extract_citations(text);
        assert_eq!(cites, vec!["abc-123", "def-456"]);
    }

    #[test]
    fn test_extract_citations_dedup() {
        let text = "See [cite:abc-123] and again [cite:abc-123].";
        let cites = extract_citations(text);
        assert_eq!(cites, vec!["abc-123"]);
    }

    #[test]
    fn test_extract_citations_empty() {
        let text = "No citations here.";
        let cites = extract_citations(text);
        assert!(cites.is_empty());
    }

    #[test]
    fn test_cache_roundtrip() {
        let db = Database::open_memory().expect("open_memory");

        // Store
        db.cache_answer(
            "What is OAuth?",
            "OAuth is an auth protocol.",
            &["chunk-1".to_string()],
            None,
        )
        .expect("cache_answer");

        // Lookup (normalized: lowercase, trailing ? removed)
        let cached = db
            .find_cached_answer("what is oauth?", None, None)
            .expect("find_cached_answer")
            .expect("should find cached answer");

        assert_eq!(cached.answer_text, "OAuth is an auth protocol.");
        assert_eq!(cached.citations, vec!["chunk-1"]);
        assert_eq!(cached.hit_count, 0);

        // Increment hit
        db.increment_cache_hit(&cached.id).expect("increment");
        let cached2 = db
            .find_cached_answer("what is oauth?", None, None)
            .expect("find_cached_answer")
            .expect("should find cached answer again");
        assert_eq!(cached2.hit_count, 1);
    }

    #[test]
    fn test_cache_source_filter() {
        let db = Database::open_memory().expect("open_memory");

        db.cache_answer("test query", "answer A", &[], Some("source-1"))
            .expect("cache_answer");
        db.cache_answer("test query", "answer B", &[], None)
            .expect("cache_answer");

        // Lookup with matching filter
        let cached = db
            .find_cached_answer("test query", Some("source-1"), None)
            .expect("find")
            .expect("should find");
        assert_eq!(cached.answer_text, "answer A");

        // Lookup with no filter
        let cached = db
            .find_cached_answer("test query", None, None)
            .expect("find")
            .expect("should find");
        assert_eq!(cached.answer_text, "answer B");

        // Lookup with non-matching filter
        let cached = db
            .find_cached_answer("test query", Some("source-2"), None)
            .expect("find");
        assert!(cached.is_none());
    }

    #[test]
    fn test_invalidate_cache_for_source() {
        let db = Database::open_memory().expect("open_memory");

        db.cache_answer("q1", "a1", &[], None).expect("cache");
        db.cache_answer("q2", "a2", &[], Some("source-1,source-2"))
            .expect("cache");
        db.cache_answer("q3", "a3", &[], Some("source-3"))
            .expect("cache");

        // Invalidate source-1 → deletes q1 (empty filter) and q2 (contains source-1)
        let deleted = db
            .invalidate_cache_for_source("source-1")
            .expect("invalidate");
        assert_eq!(deleted, 2);

        // q3 should survive
        let cached = db
            .find_cached_answer("q3", Some("source-3"), None)
            .expect("find");
        assert!(cached.is_some());
    }

    #[test]
    fn test_clear_answer_cache() {
        let db = Database::open_memory().expect("open_memory");

        db.cache_answer("q1", "a1", &[], None).expect("cache");
        db.cache_answer("q2", "a2", &[], None).expect("cache");

        let deleted = db.clear_answer_cache().expect("clear");
        assert_eq!(deleted, 2);

        let cached = db.find_cached_answer("q1", None, None).expect("find");
        assert!(cached.is_none());
    }
}
