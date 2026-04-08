//! Wiki — auto-generated knowledge index, Map of Content (MOC), and hot concepts.

use serde::{Deserialize, Serialize};

use crate::compile::{parse_entity_type, Entity, EntityType};
use crate::db::Database;
use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiIndex {
    pub by_type: std::collections::HashMap<String, Vec<EntityEntry>>,
    pub total_entities: usize,
    pub total_documents: usize,
    pub compiled_documents: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityEntry {
    pub entity: Entity,
    pub document_count: i64,
    pub link_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapOfContent {
    pub topic: String,
    pub related_entities: Vec<Entity>,
    pub documents: Vec<DocumentRef>,
    pub sub_topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRef {
    pub document_id: i64,
    pub title: String,
    pub summary: Option<String>,
    pub relevance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotConcept {
    pub entity: Entity,
    pub score: f64,
    pub recent_queries: i64,
}

impl Database {
    /// Generate the full wiki index, organized by entity type.
    pub fn generate_wiki_index(&self) -> Result<WikiIndex, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count, e.created_at,
                    (SELECT COUNT(*) FROM document_entities de WHERE de.entity_id = e.id) as doc_count,
                    (SELECT COUNT(*) FROM entity_links el WHERE el.source_entity_id = e.id OR el.target_entity_id = e.id) as link_count
             FROM entities e ORDER BY e.mention_count DESC",
        )?;
        let entries: Vec<EntityEntry> = stmt
            .query_map([], |row| {
                Ok(EntityEntry {
                    entity: Entity {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        entity_type: parse_entity_type(&row.get::<_, String>(2)?),
                        description: row.get(3)?,
                        first_seen_doc: row.get(4)?,
                        mention_count: row.get(5)?,
                        created_at: row.get(6)?,
                    },
                    document_count: row.get(7)?,
                    link_count: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut by_type: std::collections::HashMap<String, Vec<EntityEntry>> =
            std::collections::HashMap::new();
        for entry in &entries {
            let type_key = serde_json::to_value(&entry.entity.entity_type)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "other".to_string());
            by_type.entry(type_key).or_default().push(entry.clone());
        }

        let total_docs: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let compiled_docs: i64 =
            conn.query_row("SELECT COUNT(*) FROM document_summaries", [], |r| r.get(0))?;

        Ok(WikiIndex {
            total_entities: entries.len(),
            total_documents: total_docs as usize,
            compiled_documents: compiled_docs as usize,
            by_type,
        })
    }

    /// Generate a Map of Content for a specific topic/entity.
    pub fn generate_moc(&self, topic: &str) -> Result<MapOfContent, CoreError> {
        let conn = self.conn();
        // Find the topic entity
        let entity = self.find_entity_by_name(topic)?;

        // Get related entities
        let links = self.get_entity_links(&entity.id)?;
        let mut related_ids: Vec<String> = links
            .iter()
            .map(|l| {
                if l.source_entity_id == entity.id {
                    l.target_entity_id.clone()
                } else {
                    l.source_entity_id.clone()
                }
            })
            .collect();
        related_ids.sort();
        related_ids.dedup();

        let mut related_entities = Vec::new();
        for rid in &related_ids {
            if let Ok(e) = self.get_entity_by_id(rid) {
                related_entities.push(e);
            }
        }

        // Get documents linked to this entity
        let mut stmt = conn.prepare(
            "SELECT d.id, d.path, de.relevance, ds.summary
             FROM documents d
             JOIN document_entities de ON d.id = de.document_id
             LEFT JOIN document_summaries ds ON d.id = ds.document_id
             WHERE de.entity_id = ?1 ORDER BY de.relevance DESC",
        )?;
        let documents: Vec<DocumentRef> = stmt
            .query_map(rusqlite::params![entity.id], |row| {
                let path: String = row.get(1)?;
                let title = std::path::Path::new(&path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(path);
                Ok(DocumentRef {
                    document_id: row.get(0)?,
                    title,
                    summary: row.get(3)?,
                    relevance: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Sub-topics: related entities that are concepts
        let sub_topics: Vec<String> = related_entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Concept)
            .map(|e| e.name.clone())
            .collect();

        Ok(MapOfContent {
            topic: topic.to_string(),
            related_entities,
            documents,
            sub_topics,
        })
    }

    /// Get the hottest concepts based on mention count + recent query frequency.
    pub fn get_hot_concepts(&self, limit: usize) -> Result<Vec<HotConcept>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count, e.created_at,
                    COALESCE((SELECT COUNT(*) FROM query_logs ql WHERE LOWER(ql.query_text) = LOWER(e.name) AND ql.created_at > datetime('now', '-7 days')), 0) as recent_queries
             FROM entities e
             ORDER BY (e.mention_count * 1.0 + COALESCE((SELECT COUNT(*) FROM query_logs ql WHERE LOWER(ql.query_text) = LOWER(e.name) AND ql.created_at > datetime('now', '-7 days')), 0) * 2.0) DESC
             LIMIT ?1",
        )?;
        let concepts = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let mention_count: i64 = row.get(5)?;
                let recent_queries: i64 = row.get(7)?;
                Ok(HotConcept {
                    entity: Entity {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        entity_type: parse_entity_type(&row.get::<_, String>(2)?),
                        description: row.get(3)?,
                        first_seen_doc: row.get(4)?,
                        mention_count,
                        created_at: row.get(6)?,
                    },
                    score: mention_count as f64 + recent_queries as f64 * 2.0,
                    recent_queries,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(concepts)
    }
}
