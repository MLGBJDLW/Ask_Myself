//! Knowledge loop — self-reinforcing flywheel: archive outputs, track gaps, suggest explorations.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::db::Database;
use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGap {
    pub topic: String,
    pub query_count: i64,
    pub avg_confidence: f64,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryTrend {
    pub topic: String,
    pub count: i64,
    pub first_queried: String,
    pub last_queried: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveResult {
    pub document_id: String,
    pub source: String,
    pub title: String,
}

impl Database {
    /// Archive an agent's answer as a new document in the knowledge base.
    pub fn archive_agent_output(
        &self,
        conversation_id: &str,
        turn_content: &str,
        title: &str,
        source_dir: &str,
    ) -> Result<ArchiveResult, CoreError> {
        let now = chrono::Utc::now().to_rfc3339();

        let (source_id, source_root): (String, String) = {
            let conn = self.conn();
            conn.query_row(
                "SELECT id, root_path FROM sources WHERE root_path = ?1",
                rusqlite::params![source_dir],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| CoreError::InvalidInput("Source directory is not registered".into()))?
        };

        // Sanitize the title for use as filename
        let mut safe_title: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        safe_title = safe_title.trim().to_string();
        if safe_title.is_empty() {
            safe_title = "untitled".to_string();
        }
        let filename = format!("{safe_title}.md");
        let file_path = std::path::Path::new(&source_root)
            .join("_kb_archive")
            .join(&filename);

        // Create directory if needed
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write the content with frontmatter
        let content = format!(
            "---\ntitle: {title}\nsource: conversation/{conversation_id}\narchived_at: {now}\ntype: kb_archive\n---\n\n{turn_content}"
        );
        std::fs::write(&file_path, &content)?;

        let path_str = file_path.to_string_lossy().to_string();
        let mut ingest_result = crate::ingest::ingest_single_file(self, &source_id, &file_path)?;
        let mut document = self.get_document_by_path(&path_str)?.ok_or_else(|| {
            CoreError::Internal(format!(
                "Archived document was not indexed after write: {path_str}"
            ))
        })?;

        if document_chunk_count(self, &document.0)? == 0 {
            let _ = self.delete_document_by_path(&path_str)?;
            ingest_result = crate::ingest::ingest_single_file(self, &source_id, &file_path)?;
            document = self.get_document_by_path(&path_str)?.ok_or_else(|| {
                CoreError::Internal(format!(
                    "Archived document was not indexed after stale-row repair: {path_str}"
                ))
            })?;
        }

        if !matches!(ingest_result, crate::ingest::IngestFileResult::Unchanged) {
            if let Err(e) = crate::ingest::embed_source(self, &source_id) {
                warn!("Archived document indexed without embeddings: {e}");
            }
        }

        Ok(ArchiveResult {
            document_id: document.0,
            source: path_str,
            title: title.to_string(),
        })
    }

    /// Identify knowledge gaps — topics frequently queried but with low search result quality.
    pub fn get_knowledge_gaps(&self, min_queries: i64) -> Result<Vec<KnowledgeGap>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT query_text, COUNT(*) as cnt, AVG(result_count) as avg_results
             FROM query_logs
             WHERE created_at > datetime('now', '-30 days')
             GROUP BY LOWER(query_text)
             HAVING cnt >= ?1 AND avg_results < 3
             ORDER BY cnt DESC
             LIMIT 20",
        )?;
        let gaps = stmt
            .query_map(rusqlite::params![min_queries], |row| {
                let topic: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                let avg: f64 = row.get::<_, f64>(2).unwrap_or(0.0);
                Ok(KnowledgeGap {
                    topic: topic.clone(),
                    query_count: count,
                    avg_confidence: avg,
                    suggestion: format!(
                        "Frequently queried ({count} times) but few results found. Consider adding content about '{topic}'."
                    ),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(gaps)
    }

    /// Get query trends — most popular topics in recent queries.
    pub fn get_query_trends(&self, days: u32) -> Result<Vec<QueryTrend>, CoreError> {
        let conn = self.conn();
        let threshold = format!("-{days} days");
        let mut stmt = conn.prepare(
            "SELECT query_text, COUNT(*) as cnt, MIN(created_at) as first_q, MAX(created_at) as last_q
             FROM query_logs
             WHERE created_at > datetime('now', ?1)
             GROUP BY LOWER(query_text)
             ORDER BY cnt DESC
             LIMIT 30",
        )?;
        let trends = stmt
            .query_map(rusqlite::params![threshold], |row| {
                Ok(QueryTrend {
                    topic: row.get(0)?,
                    count: row.get(1)?,
                    first_queried: row.get(2)?,
                    last_queried: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(trends)
    }

    /// Suggest exploration topics based on entity graph gaps and query patterns.
    pub fn suggest_explorations(&self, limit: usize) -> Result<Vec<String>, CoreError> {
        let conn = self.conn();
        let mut suggestions = Vec::new();

        // 1. Entities with high link count but few documents (well-connected but under-documented)
        let mut stmt = conn.prepare(
            "SELECT e.name, COUNT(DISTINCT el.id) as links, COUNT(DISTINCT de.document_id) as docs
             FROM entities e
             LEFT JOIN entity_links el ON e.id = el.source_entity_id OR e.id = el.target_entity_id
             LEFT JOIN document_entities de ON e.id = de.entity_id
             GROUP BY e.id
             HAVING links > 2 AND docs <= 1
             ORDER BY links DESC
             LIMIT ?1",
        )?;
        let well_connected: Vec<String> = stmt
            .query_map(rusqlite::params![limit as i64 / 2], |row| {
                let name: String = row.get(0)?;
                Ok(format!(
                    "Deep dive into '{name}' — well-connected but under-documented"
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        suggestions.extend(well_connected);

        // 2. Recent frequent queries with no entity match
        let mut stmt2 = conn.prepare(
            "SELECT ql.query_text, COUNT(*) as cnt
             FROM query_logs ql
             WHERE ql.created_at > datetime('now', '-14 days')
             AND NOT EXISTS (SELECT 1 FROM entities e WHERE LOWER(e.name) = LOWER(ql.query_text))
             GROUP BY LOWER(ql.query_text)
             HAVING cnt >= 2
             ORDER BY cnt DESC
             LIMIT ?1",
        )?;
        let unmatched: Vec<String> = stmt2
            .query_map(rusqlite::params![limit as i64 / 2], |row| {
                let query: String = row.get(0)?;
                let cnt: i64 = row.get(1)?;
                Ok(format!(
                    "Research '{query}' — queried {cnt} times but not yet a recognized concept"
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        suggestions.extend(unmatched);

        suggestions.truncate(limit);
        Ok(suggestions)
    }
}

fn document_chunk_count(db: &Database, document_id: &str) -> Result<i64, CoreError> {
    let conn = db.conn();
    conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
        rusqlite::params![document_id],
        |row| row.get(0),
    )
    .map_err(CoreError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    #[test]
    fn archive_agent_output_ingests_markdown_chunks_for_registered_source() {
        let db = Database::open_memory().expect("open db");
        let dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec!["**/*.md".to_string()],
                exclude_globs: Vec::new(),
                watch_enabled: false,
            })
            .expect("add source");

        let result = db
            .archive_agent_output(
                "conversation-1",
                "Reusable answer about a candle ritual and three named characters.",
                "Candle Theory",
                &source.root_path,
            )
            .expect("archive output");

        assert!(std::path::Path::new(&result.source).exists());
        let (doc_id, _) = db
            .get_document_by_path(&result.source)
            .expect("lookup document")
            .expect("document row");
        assert_eq!(doc_id, result.document_id);
        assert!(
            document_chunk_count(&db, &doc_id).expect("chunk count") > 0,
            "archived document should be parsed into retrievable chunks"
        );
        let full_text = db.get_document_full_text(&doc_id).expect("full text");
        assert!(full_text.contains("candle ritual"));
    }

    #[test]
    fn archive_agent_output_rejects_unregistered_source_directory() {
        let db = Database::open_memory().expect("open db");
        let dir = tempfile::tempdir().expect("tempdir");

        let err = db
            .archive_agent_output(
                "conversation-1",
                "content",
                "title",
                &dir.path().to_string_lossy(),
            )
            .expect_err("unregistered source should fail");

        assert!(err
            .to_string()
            .contains("Source directory is not registered"));
    }
}
