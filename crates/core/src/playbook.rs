//! Playbook module — composable evidence collections.

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::{Playbook, PlaybookCitation};

/// Result of a playbook search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybookSearchResult {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub citation_count: usize,
    pub cited_content_preview: Vec<String>,
    pub relevance_score: f64,
}

/// A logged search query for analytics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryLog {
    pub id: String,
    pub query_text: String,
    pub result_count: i32,
    pub search_time_ms: i64,
    pub created_at: String,
}

/// Parse SQLite `datetime('now')` text into `DateTime<Utc>`.
fn parse_sqlite_datetime(s: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
}

/// Parse a UUID text value.
fn parse_uuid(s: &str) -> Result<Uuid, rusqlite::Error> {
    Uuid::parse_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })
}

/// Map a `playbooks` row to [`Playbook`] (citations left empty).
fn playbook_from_row(row: &rusqlite::Row) -> Result<Playbook, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let title: String = row.get(1)?;
    let description: String = row.get(2)?;
    let created_str: String = row.get(3)?;
    let updated_str: String = row.get(4)?;

    Ok(Playbook {
        id: parse_uuid(&id_str)?,
        title,
        description,
        citations: Vec::new(),
        created_at: parse_sqlite_datetime(&created_str)?,
        updated_at: parse_sqlite_datetime(&updated_str)?,
    })
}

/// Map a `playbook_citations` row to [`PlaybookCitation`].
fn citation_from_row(row: &rusqlite::Row) -> Result<PlaybookCitation, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let playbook_id_str: String = row.get(1)?;
    let chunk_id_str: String = row.get(2)?;
    let sort_order: u32 = row.get(3)?;
    let annotation: String = row.get(4)?;

    Ok(PlaybookCitation {
        id: parse_uuid(&id_str)?,
        playbook_id: parse_uuid(&playbook_id_str)?,
        chunk_id: parse_uuid(&chunk_id_str)?,
        annotation,
        order: sort_order,
    })
}

// ── Playbook CRUD ───────────────────────────────────────────────────────

impl Database {
    /// Create a new playbook.
    ///
    /// `query_text` is stored in the `goal` column for reference.
    pub fn create_playbook(
        &self,
        title: &str,
        description: &str,
        query_text: &str,
    ) -> Result<Playbook, CoreError> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO playbooks (id, title, body_md, goal) VALUES (?1, ?2, ?3, ?4)",
            params![&id, title, description, query_text],
        )?;
        drop(conn);
        self.get_playbook(&id)
    }

    /// Get a playbook by ID, including its citations ordered by `sort_order`.
    pub fn get_playbook(&self, playbook_id: &str) -> Result<Playbook, CoreError> {
        let conn = self.conn();
        let mut playbook = conn
            .query_row(
                "SELECT id, title, body_md, created_at, updated_at
                 FROM playbooks WHERE id = ?1",
                params![playbook_id],
                playbook_from_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Playbook not found: {playbook_id}"))
                }
                other => CoreError::Database(other),
            })?;

        let mut stmt = conn.prepare(
            "SELECT id, playbook_id, chunk_id, sort_order, annotation
             FROM playbook_citations
             WHERE playbook_id = ?1
             ORDER BY sort_order",
        )?;
        playbook.citations = stmt
            .query_map(params![playbook_id], citation_from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(playbook)
    }

    /// List all playbooks ordered by `updated_at` DESC (without citations).
    pub fn list_playbooks(&self) -> Result<Vec<Playbook>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, body_md, created_at, updated_at
             FROM playbooks
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], playbook_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Search playbooks by keyword across title, description, goal, and cited chunk content.
    ///
    /// Results include citation counts and a preview of cited content. Playbooks matching
    /// title/description/goal score higher than those matching only via cited chunks.
    pub fn search_playbooks(&self, query: &str) -> Result<Vec<PlaybookSearchResult>, CoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn();
        let like_pattern = format!(
            "%{}%",
            query.replace('%', "\\%").replace('_', "\\_")
        );

        let mut results: Vec<PlaybookSearchResult> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Pass 1: Match against playbook title, description (body_md), and goal.
        {
            let mut stmt = conn.prepare(
                "SELECT id, title, body_md, goal FROM playbooks
                 WHERE title LIKE ?1 ESCAPE '\\'
                    OR body_md LIKE ?1 ESCAPE '\\'
                    OR goal LIKE ?1 ESCAPE '\\'
                 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map(params![&like_pattern], |row| {
                let id: String = row.get(0)?;
                let title: String = row.get(1)?;
                let description: String = row.get(2)?;
                let goal: String = row.get(3)?;
                Ok((id, title, description, goal))
            })?;

            for row in rows {
                let (id, title, description, goal) = row?;
                if !seen_ids.insert(id.clone()) {
                    continue;
                }

                // Score: title match > description/goal match.
                let title_lower = title.to_lowercase();
                let query_lower = query.to_lowercase();
                let relevance = if title_lower.contains(&query_lower) {
                    1.0
                } else if description.to_lowercase().contains(&query_lower) {
                    0.8
                } else if goal.to_lowercase().contains(&query_lower) {
                    0.7
                } else {
                    0.6
                };

                let (citation_count, previews) =
                    Self::citation_summary_static(&conn, &id)?;

                results.push(PlaybookSearchResult {
                    id: parse_uuid(&id)?,
                    title,
                    description,
                    citation_count,
                    cited_content_preview: previews,
                    relevance_score: relevance,
                });
            }
        }

        // Pass 2: Match against cited chunk content.
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT pc.playbook_id
                 FROM playbook_citations pc
                 JOIN chunks c ON c.id = pc.chunk_id
                 WHERE c.content LIKE ?1 ESCAPE '\\'",
            )?;
            let rows = stmt.query_map(params![&like_pattern], |row| {
                let pb_id: String = row.get(0)?;
                Ok(pb_id)
            })?;

            for row in rows {
                let pb_id = row?;
                if !seen_ids.insert(pb_id.clone()) {
                    continue;
                }

                let pb_row = conn.query_row(
                    "SELECT id, title, body_md FROM playbooks WHERE id = ?1",
                    params![&pb_id],
                    |row| {
                        let id: String = row.get(0)?;
                        let title: String = row.get(1)?;
                        let desc: String = row.get(2)?;
                        Ok((id, title, desc))
                    },
                );

                match pb_row {
                    Ok((id, title, description)) => {
                        let (citation_count, previews) =
                            Self::citation_summary_static(&conn, &id)?;

                        results.push(PlaybookSearchResult {
                            id: parse_uuid(&id)?,
                            title,
                            description,
                            citation_count,
                            cited_content_preview: previews,
                            relevance_score: 0.5,
                        });
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                    Err(e) => return Err(CoreError::Database(e)),
                }
            }
        }

        // Sort by relevance descending, then by title.
        results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.title.cmp(&b.title))
        });

        Ok(results)
    }

    /// Helper: get citation count and content previews for a playbook.
    fn citation_summary_static(
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        playbook_id: &str,
    ) -> Result<(usize, Vec<String>), CoreError> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playbook_citations WHERE playbook_id = ?1",
            params![playbook_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT c.content FROM playbook_citations pc
             JOIN chunks c ON c.id = pc.chunk_id
             WHERE pc.playbook_id = ?1
             ORDER BY pc.sort_order
             LIMIT 3",
        )?;
        let previews: Vec<String> = stmt
            .query_map(params![playbook_id], |row| {
                let content: String = row.get(0)?;
                Ok(content.chars().take(200).collect::<String>())
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((count as usize, previews))
    }

    /// Update a playbook's title and description. Returns the updated playbook.
    pub fn update_playbook(
        &self,
        playbook_id: &str,
        title: &str,
        description: &str,
    ) -> Result<Playbook, CoreError> {
        let _ = self.get_playbook(playbook_id)?;
        let conn = self.conn();
        conn.execute(
            "UPDATE playbooks SET title = ?1, body_md = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            params![title, description, playbook_id],
        )?;
        drop(conn);
        self.get_playbook(playbook_id)
    }

    /// Delete a playbook. Citations are cascade-deleted by the FK constraint.
    pub fn delete_playbook(&self, playbook_id: &str) -> Result<(), CoreError> {
        let _ = self.get_playbook(playbook_id)?;
        let conn = self.conn();
        conn.execute("DELETE FROM playbooks WHERE id = ?1", params![playbook_id])?;
        Ok(())
    }
}

// ── Citation Management ─────────────────────────────────────────────────

impl Database {
    /// Add a citation linking a chunk to a playbook.
    pub fn add_citation(
        &self,
        playbook_id: &str,
        chunk_id: &str,
        note: &str,
        sort_order: u32,
    ) -> Result<PlaybookCitation, CoreError> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO playbook_citations (id, playbook_id, chunk_id, sort_order, annotation)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&id, playbook_id, chunk_id, sort_order, note],
        )?;

        conn.query_row(
            "SELECT id, playbook_id, chunk_id, sort_order, annotation
             FROM playbook_citations WHERE id = ?1",
            params![&id],
            citation_from_row,
        )
        .map_err(CoreError::Database)
    }

    /// List citations for a playbook, ordered by `sort_order`.
    pub fn list_citations(
        &self,
        playbook_id: &str,
    ) -> Result<Vec<PlaybookCitation>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, playbook_id, chunk_id, sort_order, annotation
             FROM playbook_citations
             WHERE playbook_id = ?1
             ORDER BY sort_order",
        )?;
        let rows = stmt
            .query_map(params![playbook_id], citation_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Remove a citation by its ID.
    pub fn remove_citation(&self, citation_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM playbook_citations WHERE id = ?1",
            params![citation_id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Citation not found: {citation_id}"
            )));
        }
        Ok(())
    }

    /// Update the annotation text on a citation.
    pub fn update_citation_note(
        &self,
        citation_id: &str,
        note: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE playbook_citations SET annotation = ?1 WHERE id = ?2",
            params![note, citation_id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Citation not found: {citation_id}"
            )));
        }
        Ok(())
    }

    /// Reorder citations — `sort_order` is set to each ID's position in the slice.
    pub fn reorder_citations(
        &self,
        playbook_id: &str,
        citation_ids: &[String],
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        for (i, cid) in citation_ids.iter().enumerate() {
            let affected = conn.execute(
                "UPDATE playbook_citations SET sort_order = ?1
                 WHERE id = ?2 AND playbook_id = ?3",
                params![i as i32, cid, playbook_id],
            )?;
            if affected == 0 {
                return Err(CoreError::NotFound(format!(
                    "Citation not found: {cid} in playbook {playbook_id}"
                )));
            }
        }
        Ok(())
    }
}

// ── Query Log ───────────────────────────────────────────────────────────

impl Database {
    /// Log a search query for analytics.
    pub fn log_query(
        &self,
        query_text: &str,
        result_count: i32,
        search_time_ms: i64,
    ) -> Result<(), CoreError> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO query_logs (id, query_text, result_count, duration_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![&id, query_text, result_count, search_time_ms],
        )?;
        Ok(())
    }

    /// Delete all query logs.
    pub fn clear_query_logs(&self) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute("DELETE FROM query_logs", [])?;
        Ok(())
    }

    /// Return the most recent query logs, ordered by `created_at` DESC.
    pub fn get_recent_queries(&self, limit: u32) -> Result<Vec<QueryLog>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, query_text, result_count, duration_ms, created_at
             FROM query_logs
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(QueryLog {
                    id: row.get(0)?,
                    query_text: row.get(1)?,
                    result_count: row.get(2)?,
                    search_time_ms: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_db() -> Database {
        Database::open_memory().expect("open in-memory db")
    }

    fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn insert_source(conn: &Connection) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', ?2)",
            params![&id, format!("/tmp/src-{}", &id[..8])],
        )
        .expect("insert source");
        id
    }

    fn insert_document(conn: &Connection, source_id: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'Test Doc', 'text/plain', 1234, datetime('now'), 'hash123')",
            params![&id, source_id, format!("/tmp/doc-{}.md", &id[..8])],
        )
        .expect("insert document");
        id
    }

    fn insert_chunk(conn: &Connection, document_id: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, 0, 'text', 'test content', 0, 12, 1, 1, 'chunkhash')",
            params![&id, document_id],
        )
        .expect("insert chunk");
        id
    }

    /// Create a full source → document → chunk chain, return the chunk ID.
    fn setup_chunk(db: &Database) -> String {
        let conn = db.conn();
        let src = insert_source(&conn);
        let doc = insert_document(&conn, &src);
        insert_chunk(&conn, &doc)
    }

    // ── Playbook CRUD ───────────────────────────────────────────────

    #[test]
    fn test_create_and_get_playbook() {
        let db = test_db();
        let pb = db
            .create_playbook("My SOP", "Description here", "search query")
            .expect("create_playbook");

        assert_eq!(pb.title, "My SOP");
        assert_eq!(pb.description, "Description here");
        assert!(pb.citations.is_empty());

        let fetched = db.get_playbook(&pb.id.to_string()).expect("get_playbook");
        assert_eq!(fetched.id, pb.id);
        assert_eq!(fetched.title, "My SOP");
    }

    #[test]
    fn test_get_playbook_not_found() {
        let db = test_db();
        let err = db.get_playbook("nonexistent-id").unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_list_playbooks_ordered() {
        let db = test_db();
        db.create_playbook("Older", "", "").unwrap();

        // Force the first playbook to an older timestamp so ordering is deterministic.
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE playbooks SET updated_at = datetime('now', '-1 hour') WHERE title = 'Older'",
                [],
            )
            .unwrap();
        }

        let p2 = db.create_playbook("Newer", "", "").unwrap();

        let list = db.list_playbooks().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "Newer");
        assert_eq!(list[1].title, "Older");
        // Verify descending order
        assert_eq!(list[0].id, p2.id);
    }

    #[test]
    fn test_update_playbook() {
        let db = test_db();
        let pb = db.create_playbook("Original", "Desc", "q").unwrap();

        let updated = db
            .update_playbook(&pb.id.to_string(), "Updated Title", "New Desc")
            .unwrap();
        assert_eq!(updated.title, "Updated Title");
        assert_eq!(updated.description, "New Desc");
        assert!(updated.updated_at >= pb.updated_at);
    }

    #[test]
    fn test_update_playbook_not_found() {
        let db = test_db();
        let err = db.update_playbook("nonexistent", "t", "d").unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_delete_playbook() {
        let db = test_db();
        let pb = db.create_playbook("To Delete", "", "").unwrap();
        db.delete_playbook(&pb.id.to_string()).unwrap();

        let err = db.get_playbook(&pb.id.to_string()).unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_delete_playbook_not_found() {
        let db = test_db();
        let err = db.delete_playbook("nonexistent").unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    // ── Citations ───────────────────────────────────────────────────

    #[test]
    fn test_add_and_list_citations() {
        let db = test_db();
        let pb = db.create_playbook("PB", "", "").unwrap();
        let chunk1 = setup_chunk(&db);
        let chunk2 = setup_chunk(&db);

        let c1 = db
            .add_citation(&pb.id.to_string(), &chunk1, "Note 1", 0)
            .unwrap();
        let c2 = db
            .add_citation(&pb.id.to_string(), &chunk2, "Note 2", 1)
            .unwrap();

        assert_eq!(c1.annotation, "Note 1");
        assert_eq!(c1.order, 0);
        assert_eq!(c2.order, 1);

        let list = db.list_citations(&pb.id.to_string()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, c1.id);
        assert_eq!(list[1].id, c2.id);
    }

    #[test]
    fn test_remove_citation() {
        let db = test_db();
        let pb = db.create_playbook("PB", "", "").unwrap();
        let chunk = setup_chunk(&db);
        let c = db
            .add_citation(&pb.id.to_string(), &chunk, "Note", 0)
            .unwrap();

        db.remove_citation(&c.id.to_string()).unwrap();
        let list = db.list_citations(&pb.id.to_string()).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_remove_citation_not_found() {
        let db = test_db();
        let err = db.remove_citation("nonexistent").unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_update_citation_note() {
        let db = test_db();
        let pb = db.create_playbook("PB", "", "").unwrap();
        let chunk = setup_chunk(&db);
        let c = db
            .add_citation(&pb.id.to_string(), &chunk, "Old note", 0)
            .unwrap();

        db.update_citation_note(&c.id.to_string(), "New note")
            .unwrap();

        let list = db.list_citations(&pb.id.to_string()).unwrap();
        assert_eq!(list[0].annotation, "New note");
    }

    #[test]
    fn test_update_citation_note_not_found() {
        let db = test_db();
        let err = db
            .update_citation_note("nonexistent", "note")
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_reorder_citations() {
        let db = test_db();
        let pb = db.create_playbook("PB", "", "").unwrap();
        let pb_id = pb.id.to_string();
        let chunk1 = setup_chunk(&db);
        let chunk2 = setup_chunk(&db);
        let chunk3 = setup_chunk(&db);

        let c1 = db.add_citation(&pb_id, &chunk1, "", 0).unwrap();
        let c2 = db.add_citation(&pb_id, &chunk2, "", 1).unwrap();
        let c3 = db.add_citation(&pb_id, &chunk3, "", 2).unwrap();

        // Reverse order: c3, c2, c1
        let new_order = vec![
            c3.id.to_string(),
            c2.id.to_string(),
            c1.id.to_string(),
        ];
        db.reorder_citations(&pb_id, &new_order).unwrap();

        let list = db.list_citations(&pb_id).unwrap();
        assert_eq!(list[0].id, c3.id);
        assert_eq!(list[1].id, c2.id);
        assert_eq!(list[2].id, c1.id);
    }

    // ── Cascade Delete ──────────────────────────────────────────────

    #[test]
    fn test_delete_playbook_cascades_citations() {
        let db = test_db();
        let pb = db.create_playbook("PB", "", "").unwrap();
        let pb_id = pb.id.to_string();
        let chunk = setup_chunk(&db);
        db.add_citation(&pb_id, &chunk, "Note", 0).unwrap();

        db.delete_playbook(&pb_id).unwrap();

        // Citations should be gone via ON DELETE CASCADE.
        let conn = db.conn();
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM playbook_citations WHERE playbook_id = ?1",
                params![&pb_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    // ── Query Logs ──────────────────────────────────────────────────

    #[test]
    fn test_log_and_get_recent_queries() {
        let db = test_db();
        db.log_query("search term 1", 10, 50).unwrap();
        db.log_query("search term 2", 5, 30).unwrap();

        let logs = db.get_recent_queries(10).unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].result_count + logs[1].result_count, 15);
    }

    #[test]
    fn test_get_recent_queries_respects_limit() {
        let db = test_db();
        for i in 0..5 {
            db.log_query(&format!("query {i}"), i, 10).unwrap();
        }

        let logs = db.get_recent_queries(3).unwrap();
        assert_eq!(logs.len(), 3);
    }

    #[test]
    fn test_get_recent_queries_empty() {
        let db = test_db();
        let logs = db.get_recent_queries(10).unwrap();
        assert!(logs.is_empty());
    }

    // ── Full Lifecycle ──────────────────────────────────────────────

    #[test]
    fn test_playbook_full_lifecycle() {
        let db = test_db();

        // Create
        let pb = db.create_playbook("SOP", "Body", "query").unwrap();
        assert_eq!(pb.title, "SOP");

        // Update
        let pb = db
            .update_playbook(&pb.id.to_string(), "Updated SOP", "New Body")
            .unwrap();
        assert_eq!(pb.title, "Updated SOP");
        assert_eq!(pb.description, "New Body");

        // Add citations
        let chunk1 = setup_chunk(&db);
        let chunk2 = setup_chunk(&db);
        db.add_citation(&pb.id.to_string(), &chunk1, "First", 0)
            .unwrap();
        db.add_citation(&pb.id.to_string(), &chunk2, "Second", 1)
            .unwrap();

        // Get with citations
        let pb = db.get_playbook(&pb.id.to_string()).unwrap();
        assert_eq!(pb.citations.len(), 2);

        // List
        let list = db.list_playbooks().unwrap();
        assert_eq!(list.len(), 1);

        // Delete (cascades)
        db.delete_playbook(&pb.id.to_string()).unwrap();
        assert!(matches!(
            db.get_playbook(&pb.id.to_string()),
            Err(CoreError::NotFound(_))
        ));
    }
}
