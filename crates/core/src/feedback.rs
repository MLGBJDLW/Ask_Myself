//! User feedback module — 👍/👎/pin on evidence cards.

use std::collections::HashMap;
use std::fmt;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    pub id: String,
    pub chunk_id: String,
    pub query_text: String,
    pub action: FeedbackAction,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackAction {
    Upvote,
    Downvote,
    Pin,
}

impl fmt::Display for FeedbackAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upvote => write!(f, "upvote"),
            Self::Downvote => write!(f, "downvote"),
            Self::Pin => write!(f, "pin"),
        }
    }
}

impl FeedbackAction {
    fn from_str(s: &str) -> Result<Self, CoreError> {
        match s {
            "upvote" => Ok(Self::Upvote),
            "downvote" => Ok(Self::Downvote),
            "pin" => Ok(Self::Pin),
            other => Err(CoreError::InvalidInput(format!(
                "Invalid feedback action: {other}"
            ))),
        }
    }

    /// Per-event search boost adjustment.
    ///
    /// Upvote → +0.15, Downvote → −0.15, Pin → +0.25.
    pub(crate) fn boost(&self) -> f64 {
        match self {
            Self::Upvote => 0.15,
            Self::Downvote => -0.15,
            Self::Pin => 0.25,
        }
    }
}

// ── Database methods ─────────────────────────────────────────────────

impl Database {
    /// Record a feedback action on a chunk for a given query.
    pub fn add_feedback(
        &self,
        chunk_id: &str,
        query_text: &str,
        action: FeedbackAction,
    ) -> Result<Feedback, CoreError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let action_str = action.to_string();

        let conn = self.conn();
        conn.execute(
            "INSERT INTO feedback (id, chunk_id, query_text, action, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&id, chunk_id, query_text, &action_str, &now],
        )?;

        Ok(Feedback {
            id,
            chunk_id: chunk_id.to_string(),
            query_text: query_text.to_string(),
            action,
            created_at: now,
        })
    }

    /// Get all feedback entries for a specific query text.
    pub fn get_feedback_for_query(&self, query_text: &str) -> Result<Vec<Feedback>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, chunk_id, query_text, action, created_at
             FROM feedback
             WHERE query_text = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![query_text], row_to_feedback)?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all feedback entries for a specific chunk.
    // TODO: integrate — per-chunk feedback query, not yet exposed in UI
    pub fn get_feedback_for_chunk(&self, chunk_id: &str) -> Result<Vec<Feedback>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, chunk_id, query_text, action, created_at
             FROM feedback
             WHERE chunk_id = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![chunk_id], row_to_feedback)?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Delete a feedback entry by id.
    pub fn delete_feedback(&self, feedback_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM feedback WHERE id = ?1", params![feedback_id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Feedback not found: {feedback_id}"
            )));
        }
        Ok(())
    }

    /// Compute per-chunk feedback adjustments based on ALL feedback for each chunk,
    /// regardless of query text. Applies time-based decay: recent feedback has more weight.
    ///
    /// - feedback < 7 days old: full weight (1.0×)
    /// - feedback 7–30 days old: 0.7× weight
    /// - feedback > 30 days old: 0.4× weight
    ///
    /// If `exclude_query` is `Some(q)`, feedback entries whose `query_text` matches `q`
    /// are excluded from the aggregation. This prevents double-counting when the caller
    /// will layer exact-query feedback on top separately.
    ///
    /// Per-chunk total is still clamped to \[−0.5, +0.5\].
    pub fn get_chunk_feedback_adjustments(
        &self,
        chunk_ids: &[String],
        exclude_query: Option<&str>,
    ) -> Result<HashMap<String, f64>, CoreError> {
        if chunk_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self.conn();

        let placeholders: Vec<String> = (1..=chunk_ids.len()).map(|i| format!("?{i}")).collect();
        let base_filter = format!("chunk_id IN ({})", placeholders.join(", "));
        let sql = if exclude_query.is_some() {
            format!(
                "SELECT chunk_id, action, created_at FROM feedback WHERE {} AND query_text != ?{}",
                base_filter,
                chunk_ids.len() + 1
            )
        } else {
            format!(
                "SELECT chunk_id, action, created_at FROM feedback WHERE {}",
                base_filter
            )
        };

        let mut stmt = conn.prepare(&sql)?;

        let now = chrono::Utc::now();
        let mut adjustments: HashMap<String, f64> = HashMap::new();

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = chunk_ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        if let Some(q) = exclude_query {
            params.push(Box::new(q.to_string()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(&*param_refs, |row| {
            let chunk_id: String = row.get(0)?;
            let action_str: String = row.get(1)?;
            let created_at: String = row.get(2)?;
            Ok((chunk_id, action_str, created_at))
        })?;

        for row in rows {
            let (chunk_id, action_str, created_at) = row?;
            let action = FeedbackAction::from_str(&action_str)
                .map_err(|e| CoreError::Internal(format!("bad feedback action: {e}")))?;

            let decay = if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&created_at) {
                let age = now.signed_duration_since(ts);
                if age.num_days() < 7 {
                    1.0
                } else if age.num_days() < 30 {
                    0.7
                } else {
                    0.4
                }
            } else {
                0.5
            };

            *adjustments.entry(chunk_id).or_insert(0.0) += action.boost() * decay;
        }

        for val in adjustments.values_mut() {
            *val = val.clamp(-0.5, 0.5);
        }

        Ok(adjustments)
    }

    /// Find query texts from feedback that share terms with the given query.
    /// Returns additional terms to OR into the FTS search.
    /// Only considers queries with ≥2 upvote/pin feedback entries.
    pub fn get_related_feedback_terms(
        &self,
        query_text: &str,
        max_terms: usize,
    ) -> Result<Vec<String>, CoreError> {
        if max_terms == 0 {
            return Ok(Vec::new());
        }

        // Tokenize input query into lowercase words (skip len < 2)
        let input_tokens: std::collections::HashSet<String> = query_text
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| t.len() >= 2)
            .collect();

        if input_tokens.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT query_text FROM feedback \
             WHERE action IN ('upvote', 'pin') \
             GROUP BY query_text HAVING COUNT(*) >= 2",
        )?;

        let rows = stmt.query_map([], |row| {
            let qt: String = row.get(0)?;
            Ok(qt)
        })?;

        let mut extra_terms: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for row in rows {
            let candidate = row?;
            let candidate_tokens: std::collections::HashSet<String> = candidate
                .split_whitespace()
                .map(|t| t.to_lowercase())
                .filter(|t| t.len() >= 2)
                .collect();

            // Check if candidate shares ≥2 tokens with the input query
            let shared: usize = candidate_tokens.intersection(&input_tokens).count();
            if shared >= 2 {
                // Collect unique tokens NOT in the input query
                for token in &candidate_tokens {
                    if !input_tokens.contains(token) && seen.insert(token.clone()) {
                        extra_terms.push(token.clone());
                        if extra_terms.len() >= max_terms {
                            return Ok(extra_terms);
                        }
                    }
                }
            }
        }

        Ok(extra_terms)
    }

    /// Get soft boost for chunks based on their document's overall feedback.
    /// If a document has N upvoted chunks, other chunks from that doc get +0.03*N (max +0.15).
    /// Downvoted chunks contribute -0.02*N (max -0.10).
    pub fn get_document_feedback_adjustments(
        &self,
        chunk_ids: &[String],
    ) -> Result<HashMap<String, f64>, CoreError> {
        if chunk_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self.conn();

        // Step 1: Get document_id for each chunk_id
        let placeholders: Vec<String> = (1..=chunk_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, document_id FROM chunks WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = chunk_ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut chunk_to_doc: HashMap<String, String> = HashMap::new();
        let rows = stmt.query_map(&*param_refs, |row| {
            let cid: String = row.get(0)?;
            let did: String = row.get(1)?;
            Ok((cid, did))
        })?;
        for row in rows {
            let (cid, did) = row?;
            chunk_to_doc.insert(cid, did);
        }

        if chunk_to_doc.is_empty() {
            return Ok(HashMap::new());
        }

        // Step 2: Collect unique doc IDs and count feedback per document
        let unique_docs: Vec<String> = chunk_to_doc
            .values()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let doc_placeholders: Vec<String> =
            (1..=unique_docs.len()).map(|i| format!("?{i}")).collect();
        let feedback_sql = format!(
            "SELECT c.document_id, f.action, COUNT(*) FROM feedback f \
             JOIN chunks c ON c.id = f.chunk_id \
             WHERE c.document_id IN ({}) \
             GROUP BY c.document_id, f.action",
            doc_placeholders.join(", ")
        );
        let mut stmt2 = conn.prepare(&feedback_sql)?;
        let doc_params: Vec<Box<dyn rusqlite::types::ToSql>> = unique_docs
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let doc_param_refs: Vec<&dyn rusqlite::types::ToSql> =
            doc_params.iter().map(|p| p.as_ref()).collect();

        // doc_id -> (positive_count, negative_count)
        let mut doc_feedback: HashMap<String, (i64, i64)> = HashMap::new();
        let rows2 = stmt2.query_map(&*doc_param_refs, |row| {
            let doc_id: String = row.get(0)?;
            let action: String = row.get(1)?;
            let count: i64 = row.get(2)?;
            Ok((doc_id, action, count))
        })?;
        for row in rows2 {
            let (doc_id, action, count) = row?;
            let entry = doc_feedback.entry(doc_id).or_insert((0, 0));
            match action.as_str() {
                "upvote" | "pin" => entry.0 += count,
                "downvote" => entry.1 += count,
                _ => {}
            }
        }

        // Step 3: Calculate adjustment for each chunk
        let mut adjustments: HashMap<String, f64> = HashMap::new();
        for chunk_id in chunk_ids {
            if let Some(doc_id) = chunk_to_doc.get(chunk_id) {
                if let Some(&(pos, neg)) = doc_feedback.get(doc_id) {
                    let adj = (0.03 * pos as f64 - 0.02 * neg as f64).clamp(-0.10, 0.15);
                    if adj.abs() > 1e-9 {
                        adjustments.insert(chunk_id.clone(), adj);
                    }
                }
            }
        }

        Ok(adjustments)
    }

    /// Compute per-chunk feedback adjustments for search re-ranking.
    ///
    /// Combines chunk-based feedback (any query, with time decay) as a base,
    /// then layers exact-query feedback on top at full weight.
    /// The total per-chunk adjustment is clamped to \[−0.5, +0.5\].
    /// Only chunks present in `chunk_ids` are included.
    pub fn get_feedback_adjustments(
        &self,
        query_text: &str,
        chunk_ids: &[String],
    ) -> Result<HashMap<String, f64>, CoreError> {
        // Start with chunk-based feedback (any query, with decay),
        // excluding exact-query entries to avoid double-counting.
        let mut adjustments = self.get_chunk_feedback_adjustments(chunk_ids, Some(query_text))?;

        // Layer exact-query feedback on top (full weight, no decay)
        let exact_feedbacks = self.get_feedback_for_query(query_text)?;
        let id_set: std::collections::HashSet<&str> =
            chunk_ids.iter().map(|s| s.as_str()).collect();
        for fb in exact_feedbacks {
            if id_set.contains(fb.chunk_id.as_str()) {
                *adjustments.entry(fb.chunk_id).or_insert(0.0) += fb.action.boost();
            }
        }

        // Re-clamp after combining
        for val in adjustments.values_mut() {
            *val = val.clamp(-0.5, 0.5);
        }

        Ok(adjustments)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn row_to_feedback(row: &rusqlite::Row<'_>) -> rusqlite::Result<Feedback> {
    let action_str: String = row.get(3)?;
    let action = FeedbackAction::from_str(&action_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Feedback {
        id: row.get(0)?,
        chunk_id: row.get(1)?,
        query_text: row.get(2)?,
        action,
        created_at: row.get(4)?,
    })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Set up an in-memory DB with a minimal chunk for FK satisfaction.
    fn setup_db_with_chunk() -> (Database, String) {
        let db = Database::open_memory().expect("open_memory");

        let source_id = uuid::Uuid::new_v4().to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();
        let chunk_id = uuid::Uuid::new_v4().to_string();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', '/tmp/test', '[]', '[]', 0)",
            params![&source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/test/a.md', 'text/plain', 100, datetime('now'), 'hash')",
            params![&doc_id, &source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, 0, 'text', 'test content', 0, 12, 1, 1, 'chunkhash')",
            params![&chunk_id, &doc_id],
        )
        .unwrap();
        drop(conn);

        (db, chunk_id)
    }

    #[test]
    fn test_add_and_get_feedback() {
        let (db, chunk_id) = setup_db_with_chunk();

        let fb = db
            .add_feedback(&chunk_id, "test query", FeedbackAction::Upvote)
            .expect("add_feedback");
        assert_eq!(fb.chunk_id, chunk_id);
        assert_eq!(fb.action, FeedbackAction::Upvote);

        let by_query = db.get_feedback_for_query("test query").unwrap();
        assert_eq!(by_query.len(), 1);
        assert_eq!(by_query[0].id, fb.id);

        let by_chunk = db.get_feedback_for_chunk(&chunk_id).unwrap();
        assert_eq!(by_chunk.len(), 1);
        assert_eq!(by_chunk[0].id, fb.id);
    }

    #[test]
    fn test_delete_feedback() {
        let (db, chunk_id) = setup_db_with_chunk();

        let fb = db
            .add_feedback(&chunk_id, "q", FeedbackAction::Pin)
            .unwrap();
        db.delete_feedback(&fb.id).unwrap();

        let remaining = db.get_feedback_for_chunk(&chunk_id).unwrap();
        assert!(remaining.is_empty());

        let err = db.delete_feedback("nonexistent");
        assert!(err.is_err());
    }

    #[test]
    fn test_chunk_feedback_adjustments_across_queries() {
        let (db, chunk_id) = setup_db_with_chunk();

        // Upvote via query "deploy"
        db.add_feedback(&chunk_id, "deploy", FeedbackAction::Upvote)
            .unwrap();
        // Upvote via query "how to deploy"
        db.add_feedback(&chunk_id, "how to deploy", FeedbackAction::Upvote)
            .unwrap();

        // Chunk-based: should see both upvotes regardless of query
        let adjustments = db
            .get_chunk_feedback_adjustments(&[chunk_id.clone()], None)
            .unwrap();
        assert!(adjustments.contains_key(&chunk_id));
        let adj = adjustments[&chunk_id];
        assert!(adj > 0.2, "Expected ~0.3 (2 × 0.15), got {adj}");

        // get_feedback_adjustments with a third query should still pick up chunk feedback
        let adj2 = db
            .get_feedback_adjustments("deployment process", &[chunk_id.clone()])
            .unwrap();
        assert!(adj2.contains_key(&chunk_id));
        assert!(
            adj2[&chunk_id] > 0.2,
            "Chunk feedback should apply to new queries"
        );
    }

    #[test]
    fn test_chunk_feedback_empty_ids() {
        let (db, _chunk_id) = setup_db_with_chunk();
        let adjustments = db.get_chunk_feedback_adjustments(&[], None).unwrap();
        assert!(adjustments.is_empty());
    }

    #[test]
    fn test_related_feedback_terms() {
        let (db, chunk_id) = setup_db_with_chunk();

        // "deploy strategy" gets 2 upvotes → qualifies
        db.add_feedback(&chunk_id, "deploy strategy", FeedbackAction::Upvote)
            .unwrap();
        db.add_feedback(&chunk_id, "deploy strategy", FeedbackAction::Pin)
            .unwrap();

        // "config setup" gets only 1 upvote → excluded
        db.add_feedback(&chunk_id, "config setup", FeedbackAction::Upvote)
            .unwrap();

        // Query "deployment deploy plan" shares "deploy" with "deploy strategy"
        // but only 1 token overlap → not enough (need ≥2).
        let terms = db
            .get_related_feedback_terms("deployment deploy plan", 5)
            .unwrap();
        // "deploy" is shared, but only 1 token overlaps → 0 extra terms
        assert!(terms.is_empty(), "need ≥2 shared tokens; got {:?}", terms);

        // Query "deploy strategy review" shares "deploy" AND "strategy" → qualifies
        // But all candidate tokens ("deploy", "strategy") are in the input → no extras
        let terms2 = db
            .get_related_feedback_terms("deploy strategy review", 5)
            .unwrap();
        assert!(
            terms2.is_empty(),
            "all candidate tokens already in input; got {:?}",
            terms2
        );

        // Add another qualifying feedback query that will yield new terms
        db.add_feedback(
            &chunk_id,
            "deploy strategy rollback",
            FeedbackAction::Upvote,
        )
        .unwrap();
        db.add_feedback(
            &chunk_id,
            "deploy strategy rollback",
            FeedbackAction::Upvote,
        )
        .unwrap();

        // Query "deploy strategy" shares 2 tokens → should get "rollback" from candidate
        let terms3 = db.get_related_feedback_terms("deploy strategy", 5).unwrap();
        assert!(
            terms3.contains(&"rollback".to_string()),
            "expected 'rollback'; got {:?}",
            terms3
        );
    }

    #[test]
    fn test_related_feedback_terms_empty() {
        let (db, _chunk_id) = setup_db_with_chunk();
        let terms = db.get_related_feedback_terms("anything", 5).unwrap();
        assert!(terms.is_empty());
    }

    #[test]
    fn test_document_feedback_adjustments() {
        let db = Database::open_memory().expect("open_memory");

        let source_id = uuid::Uuid::new_v4().to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', '/tmp/test', '[]', '[]', 0)",
            params![&source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/test/a.md', 'text/plain', 100, datetime('now'), 'hash')",
            params![&doc_id, &source_id],
        ).unwrap();

        // Create 4 chunks in the same document
        let mut chunk_ids = Vec::new();
        for i in 0..4 {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', 'content', 0, 7, 1, 1, ?4)",
                params![&cid, &doc_id, i, format!("hash{i}")],
            ).unwrap();
            chunk_ids.push(cid);
        }
        drop(conn);

        // Add 3 upvotes on first 3 chunks
        for cid in &chunk_ids[..3] {
            db.add_feedback(cid, "some query", FeedbackAction::Upvote)
                .unwrap();
        }

        // Query the 4th chunk (no direct feedback) → should get +0.09 (3 * 0.03)
        let adjustments = db
            .get_document_feedback_adjustments(&[chunk_ids[3].clone()])
            .unwrap();
        let adj = adjustments.get(&chunk_ids[3]).copied().unwrap_or(0.0);
        assert!(
            (adj - 0.09).abs() < 1e-6,
            "expected ~0.09 (3 upvotes * 0.03), got {adj}"
        );
    }

    #[test]
    fn test_document_feedback_adjustments_clamped() {
        let db = Database::open_memory().expect("open_memory");

        let source_id = uuid::Uuid::new_v4().to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', '/tmp/test', '[]', '[]', 0)",
            params![&source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/test/a.md', 'text/plain', 100, datetime('now'), 'hash')",
            params![&doc_id, &source_id],
        ).unwrap();

        // Create 7 chunks, upvote all → 7 * 0.03 = 0.21, clamped to 0.15
        let mut chunk_ids = Vec::new();
        for i in 0..7 {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', 'content', 0, 7, 1, 1, ?4)",
                params![&cid, &doc_id, i, format!("hash{i}")],
            ).unwrap();
            chunk_ids.push(cid);
        }
        drop(conn);

        for cid in &chunk_ids[..6] {
            db.add_feedback(cid, "query", FeedbackAction::Upvote)
                .unwrap();
        }

        let adjustments = db
            .get_document_feedback_adjustments(&[chunk_ids[6].clone()])
            .unwrap();
        let adj = adjustments.get(&chunk_ids[6]).copied().unwrap_or(0.0);
        assert!(
            (adj - 0.15).abs() < 1e-6,
            "expected clamped to 0.15, got {adj}"
        );
    }

    #[test]
    fn test_document_feedback_adjustments_empty() {
        let (db, _chunk_id) = setup_db_with_chunk();
        let adjustments = db.get_document_feedback_adjustments(&[]).unwrap();
        assert!(adjustments.is_empty());
    }
}
