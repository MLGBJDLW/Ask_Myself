//! Indexing module — FTS5 index management utilities.
//!
//! The FTS5 table `fts_chunks` is kept in sync with `chunks` via SQL
//! triggers (see migration v002).  This module provides rebuild,
//! optimize, integrity-check, and statistics helpers.

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

/// Aggregate statistics about the search index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub total_sources: i64,
    pub total_documents: i64,
    pub total_chunks: i64,
    pub fts_rows: i64,
    /// `true` when `total_chunks == fts_rows`.
    pub is_synced: bool,
}

impl Database {
    /// Drop and recreate FTS5 content from the `chunks` table.
    ///
    /// Equivalent to the FTS5 built-in `'rebuild'` command.
    pub fn rebuild_fts_index(&self) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute("INSERT INTO fts_chunks(fts_chunks) VALUES('rebuild')", [])?;
        Ok(())
    }

    /// Collect aggregate counts for sources, documents, chunks, and FTS rows.
    pub fn get_index_stats(&self) -> Result<IndexStats, CoreError> {
        let conn = self.conn();

        let total_sources: i64 =
            conn.query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))?;
        let total_documents: i64 =
            conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let total_chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let fts_rows: i64 = conn.query_row("SELECT COUNT(*) FROM fts_chunks", [], |r| r.get(0))?;

        Ok(IndexStats {
            total_sources,
            total_documents,
            total_chunks,
            fts_rows,
            is_synced: total_chunks == fts_rows,
        })
    }

    /// Run the FTS5 `'optimize'` command to merge internal b-tree segments.
    pub fn optimize_fts_index(&self) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute("INSERT INTO fts_chunks(fts_chunks) VALUES('optimize')", [])?;
        Ok(())
    }

    /// Run FTS5 integrity-check.
    ///
    /// Returns `true` when the index is consistent with the content
    /// table, `false` otherwise.
    // TODO: integrate — FTS5 integrity diagnostic, not yet exposed as Tauri command
    pub fn integrity_check(&self) -> Result<bool, CoreError> {
        let conn = self.conn();
        let result = conn.execute(
            "INSERT INTO fts_chunks(fts_chunks, rank) VALUES('integrity-check', 1)",
            [],
        );
        match result {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(_, _)) => Ok(false),
            Err(e) => Err(CoreError::Database(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers (mirror the ones in db.rs tests) ────────────────────────

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn setup() -> Database {
        Database::open_memory().expect("open in-memory db")
    }

    fn seed_data(db: &Database, chunk_count: usize) {
        let conn = db.conn();
        let src_id = new_id();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', '/tmp/src')",
            rusqlite::params![&src_id],
        )
        .unwrap();

        let doc_id = new_id();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, '/tmp/doc.md', 'Doc', 'text/plain', 100, datetime('now'), 'hash')",
            rusqlite::params![&doc_id, &src_id],
        )
        .unwrap();

        for i in 0..chunk_count {
            let cid = new_id();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', ?4, 0, 100, 1, 10, 'chash')",
                rusqlite::params![&cid, &doc_id, i as i64, format!("chunk content {i}")],
            )
            .unwrap();
        }
    }

    // ── tests ───────────────────────────────────────────────────────────

    #[test]
    fn stats_empty_db() {
        let db = setup();
        let stats = db.get_index_stats().unwrap();

        assert_eq!(stats.total_sources, 0);
        assert_eq!(stats.total_documents, 0);
        assert_eq!(stats.total_chunks, 0);
        assert_eq!(stats.fts_rows, 0);
        assert!(stats.is_synced);
    }

    #[test]
    fn stats_after_ingest() {
        let db = setup();
        seed_data(&db, 5);

        let stats = db.get_index_stats().unwrap();
        assert_eq!(stats.total_sources, 1);
        assert_eq!(stats.total_documents, 1);
        assert_eq!(stats.total_chunks, 5);
        assert_eq!(stats.fts_rows, 5);
        assert!(stats.is_synced);
    }

    #[test]
    fn rebuild_fts_index_succeeds() {
        let db = setup();
        seed_data(&db, 3);

        db.rebuild_fts_index().unwrap();

        let stats = db.get_index_stats().unwrap();
        assert_eq!(stats.total_chunks, 3);
        assert_eq!(stats.fts_rows, 3);
        assert!(stats.is_synced);
    }

    #[test]
    fn optimize_fts_index_succeeds() {
        let db = setup();
        seed_data(&db, 4);

        db.optimize_fts_index().unwrap();

        // Optimize is a no-error operation; verify counts unchanged.
        let stats = db.get_index_stats().unwrap();
        assert_eq!(stats.total_chunks, 4);
        assert!(stats.is_synced);
    }

    #[test]
    fn integrity_check_passes() {
        let db = setup();
        seed_data(&db, 2);

        assert!(db.integrity_check().unwrap());
    }
}
