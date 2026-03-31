//! Database module — manages SQLite connections and schema migrations.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::CoreError;

/// Thread-safe wrapper around a SQLite connection.
///
/// On construction the connection is configured with production PRAGMAs
/// and all pending schema migrations are applied automatically.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: Option<PathBuf>,
}

impl Database {
    /// Open a file-backed database with WAL mode, PRAGMAs, and auto-migration.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let conn = Connection::open(path.as_ref())?;
        Self::configure_connection(&conn)?;
        crate::migrations::run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path.as_ref().to_path_buf()),
        })
    }

    /// Open an in-memory database for testing.
    pub fn open_memory() -> Result<Self, CoreError> {
        let conn = Connection::open_in_memory()?;
        Self::configure_connection(&conn)?;
        crate::migrations::run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
        })
    }

    /// Get a reference to the connection (locked).
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!(
                    "Database mutex was poisoned by an earlier panic; recovering inner connection"
                );
                poisoned.into_inner()
            }
        }
    }

    /// Return the file path of the database, if file-backed.
    pub fn db_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn configure_connection(conn: &Connection) -> Result<(), CoreError> {
        // Use prepare + query for each PRAGMA individually.
        // Some PRAGMAs return result rows (journal_mode, journal_size_limit)
        // while others don't — query() handles both cases gracefully.
        for pragma in [
            "PRAGMA journal_mode = WAL",
            "PRAGMA busy_timeout = 5000",
            "PRAGMA foreign_keys = ON",
            "PRAGMA synchronous = NORMAL",
            "PRAGMA cache_size = -64000",
            "PRAGMA temp_store = MEMORY",
            "PRAGMA mmap_size = 268435456",
            "PRAGMA journal_size_limit = 67108864",
        ] {
            let _ = conn.prepare(pragma)?.query([])?;
        }
        Ok(())
    }
}

impl Database {
    /// Get all chunks as `(chunk_id, content)` pairs.
    pub fn get_all_chunks(&self) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, content FROM chunks ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all chunks belonging to documents of a given source.
    pub fn get_chunks_for_source(
        &self,
        source_id: &str,
    ) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content FROM chunks c
             JOIN documents d ON c.document_id = d.id
             WHERE d.source_id = ?1
             ORDER BY c.id",
        )?;
        let rows = stmt.query_map(rusqlite::params![source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get chunks that do not yet have an embedding for the given model.
    pub fn get_chunks_without_embeddings(
        &self,
        model: &str,
    ) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content FROM chunks c
             LEFT JOIN embeddings e ON c.id = e.chunk_id AND e.model = ?1
             WHERE e.chunk_id IS NULL
             ORDER BY c.id",
        )?;
        let rows = stmt.query_map(rusqlite::params![model], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Delete all embeddings for a given model.
    pub fn delete_all_embeddings(&self, model: &str) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM embeddings WHERE model = ?1",
            rusqlite::params![model],
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn insert_source(conn: &Connection) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', ?2)",
            rusqlite::params![&id, format!("/tmp/src-{}", &id[..8])],
        )
        .expect("insert source");
        id
    }

    fn insert_document(conn: &Connection, source_id: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'Test Doc', 'text/plain', 1234, datetime('now'), 'hash123')",
            rusqlite::params![&id, source_id, format!("/tmp/doc-{}.md", &id[..8])],
        )
        .expect("insert document");
        id
    }

    fn insert_chunk(conn: &Connection, document_id: &str, content: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, 0, 'text', ?3, 0, ?4, 1, 10, 'chunkhash')",
            rusqlite::params![&id, document_id, content, content.len() as i64],
        )
        .expect("insert chunk");
        id
    }

    // ── tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_database_new_memory() {
        let db = Database::open_memory().expect("open_memory should succeed");
        let conn = db.conn();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        for expected in &[
            "sources",
            "documents",
            "chunks",
            "fts_chunks",
            "playbooks",
            "playbook_citations",
            "query_logs",
            "_migrations",
        ] {
            assert!(
                tables.contains(&expected.to_string()),
                "table '{}' should exist, got: {:?}",
                expected,
                tables
            );
        }
    }

    #[test]
    fn test_database_migrations_idempotent() {
        let _db1 = Database::open_memory().expect("first open_memory should succeed");
        let _db2 = Database::open_memory().expect("second open_memory should succeed");
    }

    #[test]
    fn test_conn_recovers_from_poisoned_mutex() {
        let db = Database::open_memory().expect("open_memory should succeed");
        let db_clone = db.clone();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = db_clone.conn.lock().expect("lock should succeed");
            panic!("poison the mutex");
        }));

        let conn = db.conn();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .expect("query after poison recovery should succeed");

        assert!(table_count > 0);
    }

    #[test]
    fn test_sources_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        // Create
        let id = insert_source(&conn);

        // Read
        let kind: String = conn
            .query_row("SELECT kind FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "local_folder");

        // Update
        conn.execute("UPDATE sources SET kind = 'remote' WHERE id = ?1", [&id])
            .unwrap();
        let kind: String = conn
            .query_row("SELECT kind FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "remote");

        // Delete
        conn.execute("DELETE FROM sources WHERE id = ?1", [&id])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_documents_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);

        // Read
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Test Doc");

        // Update
        conn.execute(
            "UPDATE documents SET title = 'Updated Doc' WHERE id = ?1",
            [&doc_id],
        )
        .unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Updated Doc");

        // Delete
        conn.execute("DELETE FROM documents WHERE id = ?1", [&doc_id])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_chunks_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "chunk body text");

        // Read & verify offsets
        let (content, start, end): (String, i64, i64) = conn
            .query_row(
                "SELECT content, start_offset, end_offset FROM chunks WHERE id = ?1",
                [&chunk_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "chunk body text");
        assert_eq!(start, 0);
        assert_eq!(end, "chunk body text".len() as i64);
    }

    #[test]
    fn test_fts5_insert_and_search() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(
            &conn,
            &doc_id,
            "the quick brown fox jumps over the lazy dog",
        );

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'quick'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS should find the inserted chunk");
    }

    #[test]
    fn test_fts5_auto_sync_on_delete() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "unique_sentinel_word_alpha");

        // Verify FTS has it
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'unique_sentinel_word_alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Delete chunk
        conn.execute("DELETE FROM chunks WHERE id = ?1", [&chunk_id])
            .unwrap();

        // FTS should no longer find it
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'unique_sentinel_word_alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "FTS should auto-remove on chunk delete");
    }

    #[test]
    fn test_fts5_auto_sync_on_update() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "original_sentinel_beta");

        // Update content
        conn.execute(
            "UPDATE chunks SET content = 'replacement_sentinel_gamma' WHERE id = ?1",
            [&chunk_id],
        )
        .unwrap();

        // Old content gone
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'original_sentinel_beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "FTS should not find old content after update");

        // New content present
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'replacement_sentinel_gamma'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS should find new content after update");
    }

    #[test]
    fn test_playbooks_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let id = new_id();

        // Create
        conn.execute(
            "INSERT INTO playbooks (id, title, body_md) VALUES (?1, 'My Playbook', '# Hello')",
            [&id],
        )
        .unwrap();

        // Read
        let title: String = conn
            .query_row("SELECT title FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "My Playbook");

        // Update
        conn.execute(
            "UPDATE playbooks SET title = 'Renamed Playbook' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        let title: String = conn
            .query_row("SELECT title FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "Renamed Playbook");

        // Delete
        conn.execute("DELETE FROM playbooks WHERE id = ?1", [&id])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_playbook_citations_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "cited chunk content");

        let playbook_id = new_id();
        conn.execute(
            "INSERT INTO playbooks (id, title, body_md) VALUES (?1, 'Citation PB', '')",
            [&playbook_id],
        )
        .unwrap();

        let citation_id = new_id();
        conn.execute(
            "INSERT INTO playbook_citations (id, playbook_id, chunk_id, sort_order, annotation)
             VALUES (?1, ?2, ?3, 1, 'important note')",
            rusqlite::params![&citation_id, &playbook_id, &chunk_id],
        )
        .unwrap();

        // Read back
        let annotation: String = conn
            .query_row(
                "SELECT annotation FROM playbook_citations WHERE id = ?1",
                [&citation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(annotation, "important note");
    }

    #[test]
    fn test_cascade_delete_source() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(&conn, &doc_id, "cascade test content");

        // Delete source — should cascade to documents and chunks
        conn.execute("DELETE FROM sources WHERE id = ?1", [&source_id])
            .unwrap();

        let doc_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE source_id = ?1",
                [&source_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc_count, 0, "documents should be cascade-deleted");

        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chunk_count, 0, "chunks should be cascade-deleted");
    }

    #[test]
    fn test_cascade_delete_document() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(&conn, &doc_id, "document cascade chunk");

        // Delete document — should cascade to chunks
        conn.execute("DELETE FROM documents WHERE id = ?1", [&doc_id])
            .unwrap();

        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chunk_count, 0, "chunks should be cascade-deleted");
    }

    #[test]
    fn test_query_logs_insert() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let id = new_id();
        conn.execute(
            "INSERT INTO query_logs (id, query_text, result_count, duration_ms)
             VALUES (?1, 'how to deploy?', 5, 42)",
            [&id],
        )
        .unwrap();

        let (query_text, result_count, duration): (String, i64, i64) = conn
            .query_row(
                "SELECT query_text, result_count, duration_ms FROM query_logs WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(query_text, "how to deploy?");
        assert_eq!(result_count, 5);
        assert_eq!(duration, 42);
    }
}

// ---------------------------------------------------------------------------
// Agent trace persistence
// ---------------------------------------------------------------------------

impl Database {
    /// Persist a completed agent trace.
    pub fn save_agent_trace(&self, trace: &crate::trace::AgentTrace) -> Result<(), CoreError> {
        let trace_json = serde_json::to_string(trace)
            .map_err(|e| CoreError::Internal(format!("serialize agent trace: {e}")))?;
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO agent_traces
             (id, conversation_id, started_at, finished_at, model_id,
              total_iterations, total_tool_calls, total_input_tokens, total_output_tokens,
              peak_context_usage_pct, tools_offered, cache_hit, outcome, error_message, trace_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                trace.id,
                trace.conversation_id,
                trace.started_at.to_rfc3339(),
                trace.finished_at.map(|t| t.to_rfc3339()),
                trace.model_id,
                trace.total_iterations,
                trace.total_tool_calls,
                trace.total_input_tokens as i64,
                trace.total_output_tokens as i64,
                trace.peak_context_usage_pct,
                trace.tools_offered,
                trace.cache_hit as i32,
                trace.outcome.to_string(),
                trace.error_message,
                trace_json,
            ],
        )?;
        Ok(())
    }

    /// Retrieve all traces for a conversation.
    pub fn get_agent_traces(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<crate::trace::AgentTrace>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT trace_json FROM agent_traces WHERE conversation_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            row.get::<_, String>(0)
        })?;
        let mut traces = Vec::new();
        for row in rows {
            let json = row?;
            let trace: crate::trace::AgentTrace = serde_json::from_str(&json)
                .map_err(|e| CoreError::Internal(format!("deserialize agent trace: {e}")))?;
            traces.push(trace);
        }
        Ok(traces)
    }

    /// Retrieve the most recent traces across all conversations.
    pub fn get_recent_traces(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::trace::AgentTrace>, CoreError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT trace_json FROM agent_traces ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut traces = Vec::new();
        for row in rows {
            let json = row?;
            let trace: crate::trace::AgentTrace = serde_json::from_str(&json)
                .map_err(|e| CoreError::Internal(format!("deserialize agent trace: {e}")))?;
            traces.push(trace);
        }
        Ok(traces)
    }

    /// Compute aggregated analytics across all agent traces.
    pub fn get_trace_summary(&self) -> Result<crate::trace::TraceSummary, CoreError> {
        let conn = self.conn();

        // Aggregate numeric stats in one query.
        let mut stmt = conn.prepare(
            "SELECT
                COUNT(*) AS total_sessions,
                COALESCE(SUM(total_tool_calls), 0) AS total_tool_calls,
                COALESCE(SUM(total_input_tokens), 0) AS total_input_tokens,
                COALESCE(SUM(total_output_tokens), 0) AS total_output_tokens,
                COALESCE(AVG(total_iterations), 0) AS avg_iterations,
                COALESCE(AVG(total_tool_calls), 0) AS avg_tools,
                COALESCE(AVG(peak_context_usage_pct), 0) AS avg_context,
                COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0) AS success_count,
                COALESCE(SUM(CASE WHEN cache_hit = 1 THEN 1 ELSE 0 END), 0) AS cache_hit_count,
                COALESCE(SUM(CASE WHEN started_at >= datetime('now', '-7 days') THEN 1 ELSE 0 END), 0) AS sessions_7d,
                COALESCE(SUM(CASE WHEN started_at >= datetime('now', '-7 days') THEN total_input_tokens + total_output_tokens ELSE 0 END), 0) AS tokens_7d
             FROM agent_traces",
        )?;

        let (
            total_sessions,
            total_tool_calls,
            total_input_tokens,
            total_output_tokens,
            avg_iterations,
            avg_tools,
            avg_context,
            success_count,
            cache_hit_count,
            sessions_7d,
            tokens_7d,
        ) = stmt.query_row([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, i64>(7)? as u64,
                row.get::<_, i64>(8)? as u64,
                row.get::<_, i64>(9)? as u64,
                row.get::<_, i64>(10)? as u64,
            ))
        })?;

        let success_rate = if total_sessions > 0 {
            success_count as f64 / total_sessions as f64
        } else {
            0.0
        };
        let cache_hit_rate = if total_sessions > 0 {
            cache_hit_count as f64 / total_sessions as f64
        } else {
            0.0
        };

        // Top tools: extract from trace_json steps (limit scan to 200 most recent).
        let mut tool_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut stmt2 =
            conn.prepare("SELECT trace_json FROM agent_traces ORDER BY created_at DESC LIMIT 200")?;
        let rows2 = stmt2.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows2 {
            let json = row?;
            if let Ok(trace) = serde_json::from_str::<crate::trace::AgentTrace>(&json) {
                for step in &trace.steps {
                    if let Some(ref name) = step.tool_name {
                        *tool_counts.entry(name.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut top_tools: Vec<(String, u64)> = tool_counts.into_iter().collect();
        top_tools.sort_by(|a, b| b.1.cmp(&a.1));
        top_tools.truncate(10);

        Ok(crate::trace::TraceSummary {
            total_sessions,
            total_tool_calls,
            total_input_tokens,
            total_output_tokens,
            avg_iterations_per_session: avg_iterations,
            avg_tools_per_session: avg_tools,
            avg_context_usage_pct: avg_context,
            success_rate,
            cache_hit_rate,
            top_tools,
            sessions_last_7_days: sessions_7d,
            tokens_last_7_days: tokens_7d,
        })
    }
}
