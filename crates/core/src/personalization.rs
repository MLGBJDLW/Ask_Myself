//! User preference profile — builds personalization context from feedback history.

use crate::db::Database;
use crate::error::CoreError;

/// Build a concise preference summary from accumulated feedback data.
/// Returns a Markdown section to be appended to the system prompt.
/// If no meaningful feedback exists, returns an empty string.
pub fn build_preference_summary(db: &Database) -> Result<String, CoreError> {
    let (preferred_sources, avoided_sources, preferred_types, top_queries, total) = {
        let conn = db.conn();

        // 1. Top upvoted/pinned sources (by source root_path)
        let mut stmt = conn.prepare(
            "SELECT s.root_path, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             JOIN sources s ON d.source_id = s.id
             WHERE f.action IN ('upvote', 'pin')
             GROUP BY s.root_path
             ORDER BY cnt DESC
             LIMIT 5",
        )?;
        let preferred_sources: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // 2. Top downvoted sources
        let mut stmt2 = conn.prepare(
            "SELECT s.root_path, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             JOIN sources s ON d.source_id = s.id
             WHERE f.action = 'downvote'
             GROUP BY s.root_path
             ORDER BY cnt DESC
             LIMIT 3",
        )?;
        let avoided_sources: Vec<(String, i64)> = stmt2
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // 3. Top upvoted document types (by mime_type)
        let mut stmt3 = conn.prepare(
            "SELECT d.mime_type, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             WHERE f.action IN ('upvote', 'pin')
             GROUP BY d.mime_type
             ORDER BY cnt DESC
             LIMIT 5",
        )?;
        let preferred_types: Vec<(String, i64)> = stmt3
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // 4. Most frequently asked topics (from query_text)
        let mut stmt4 = conn.prepare(
            "SELECT query_text, COUNT(*) as cnt
             FROM feedback
             WHERE action IN ('upvote', 'pin')
             GROUP BY query_text
             ORDER BY cnt DESC
             LIMIT 8",
        )?;
        let top_queries: Vec<(String, i64)> = stmt4
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // 5. Total feedback count for minimum threshold
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM feedback", [], |row| row.get(0))?;

        (
            preferred_sources,
            avoided_sources,
            preferred_types,
            top_queries,
            total,
        )
    }; // conn and all statements dropped here

    // Need at least 3 feedback entries to generate a meaningful profile
    if total < 3 {
        return Ok(String::new());
    }

    let mut sections = Vec::new();

    if !preferred_sources.is_empty() {
        let items: Vec<String> = preferred_sources
            .iter()
            .map(|(path, _)| format!("  - {}", extract_dir_name(path)))
            .collect();
        sections.push(format!("Preferred sources:\n{}", items.join("\n")));
    }

    if !avoided_sources.is_empty() {
        let items: Vec<String> = avoided_sources
            .iter()
            .map(|(path, _)| format!("  - {}", extract_dir_name(path)))
            .collect();
        sections.push(format!("Less preferred sources:\n{}", items.join("\n")));
    }

    if !preferred_types.is_empty() {
        let types: Vec<&str> = preferred_types.iter().map(|(t, _)| t.as_str()).collect();
        sections.push(format!("Preferred content types: {}", types.join(", ")));
    }

    if !top_queries.is_empty() {
        let queries: Vec<&str> = top_queries.iter().map(|(q, _)| q.as_str()).collect();
        sections.push(format!("Frequently engaged topics: {}", queries.join(", ")));
    }

    if sections.is_empty() {
        return Ok(String::new());
    }

    Ok(format!(
        "\n## User Preferences (auto-generated from feedback)\n\n{}\n\nUse these preferences to prioritize and personalize your responses. Prefer content from preferred sources when multiple results are equally relevant.",
        sections.join("\n")
    ))
}

/// Returns source root_paths that the user has positively engaged with (upvote/pin).
/// Returns up to `limit` source paths ordered by engagement count.
pub fn get_preferred_source_paths(db: &Database, limit: usize) -> Result<Vec<String>, CoreError> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT s.root_path, COUNT(*) as cnt
         FROM feedback f
         JOIN chunks c ON c.id = f.chunk_id
         JOIN documents d ON c.document_id = d.id
         JOIN sources s ON d.source_id = s.id
         WHERE f.action IN ('upvote', 'pin')
         GROUP BY s.root_path
         ORDER BY cnt DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
        let path: String = row.get(0)?;
        Ok(path)
    })?;
    let mut paths = Vec::new();
    for row in rows {
        paths.push(row?);
    }
    Ok(paths)
}

/// Extract the last directory component from a path for display.
fn extract_dir_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::feedback::FeedbackAction;
    use rusqlite::params;

    fn setup_source_with_chunks(db: &Database, root_path: &str) -> Vec<String> {
        let source_id = uuid::Uuid::new_v4().to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', ?2, '[]', '[]', 0)",
            params![&source_id, root_path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'text/plain', 100, datetime('now'), 'hash')",
            params![&doc_id, &source_id, format!("{root_path}/doc.md")],
        )
        .unwrap();

        let mut chunk_ids = Vec::new();
        for i in 0..2 {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', 'content', 0, 7, 1, 1, ?4)",
                params![&cid, &doc_id, i, format!("hash{i}")],
            )
            .unwrap();
            chunk_ids.push(cid);
        }
        drop(conn);
        chunk_ids
    }

    #[test]
    fn test_get_preferred_source_paths() {
        let db = Database::open_memory().expect("open_memory");

        let chunks_a = setup_source_with_chunks(&db, "/home/user/notes");
        let chunks_b = setup_source_with_chunks(&db, "/home/user/docs");

        // 3 upvotes on source A, 1 on source B
        for cid in &chunks_a {
            db.add_feedback(cid, "query", FeedbackAction::Upvote)
                .unwrap();
        }
        db.add_feedback(&chunks_a[0], "query2", FeedbackAction::Pin)
            .unwrap();
        db.add_feedback(&chunks_b[0], "query", FeedbackAction::Upvote)
            .unwrap();

        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0], "/home/user/notes",
            "source with most engagement should be first"
        );
        assert_eq!(paths[1], "/home/user/docs");
    }

    #[test]
    fn test_get_preferred_source_paths_empty() {
        let db = Database::open_memory().expect("open_memory");
        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_get_preferred_source_paths_excludes_downvotes() {
        let db = Database::open_memory().expect("open_memory");
        let chunks = setup_source_with_chunks(&db, "/home/user/bad");

        // Only downvotes — should NOT appear
        db.add_feedback(&chunks[0], "query", FeedbackAction::Downvote)
            .unwrap();

        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert!(paths.is_empty());
    }
}
