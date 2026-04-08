//! Knowledge loop — self-reinforcing flywheel: archive outputs, track gaps, suggest explorations.

use serde::{Deserialize, Serialize};

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
    pub document_id: i64,
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
        let conn = self.conn();
        let now = chrono::Utc::now().to_rfc3339();

        // Validate source_dir is a registered source
        let source_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sources WHERE path = ?1",
            rusqlite::params![source_dir],
            |row| row.get(0),
        )?;
        if source_count == 0 {
            return Err(CoreError::InvalidInput(
                "Source directory is not registered".into(),
            ));
        }

        // Sanitize the title for use as filename
        let safe_title: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let filename = format!("{safe_title}.md");
        let file_path = std::path::Path::new(source_dir)
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

        // Insert as document (it will be picked up by watcher/re-scan, but also insert directly)
        let path_str = file_path.to_string_lossy().to_string();
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        conn.execute(
            "INSERT OR IGNORE INTO documents (path, source_id, content_hash, mime_type, size_bytes, indexed_at, updated_at)
             VALUES (?1, (SELECT id FROM sources WHERE ?1 LIKE path || '%' LIMIT 1), ?2, 'text/markdown', ?3, ?4, ?4)",
            rusqlite::params![path_str, hash, content.len() as i64, now],
        )?;

        let doc_id = conn.query_row(
            "SELECT id FROM documents WHERE path = ?1",
            rusqlite::params![path_str],
            |row| row.get::<_, i64>(0),
        )?;

        Ok(ArchiveResult {
            document_id: doc_id,
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
