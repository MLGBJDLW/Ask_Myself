//! Knowledge graph — entity relationship network with traversal and clustering.

use serde::{Deserialize, Serialize};

use crate::compile::{parse_entity_type, Entity};
use crate::db::Database;
use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityLink {
    pub id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub relation_type: String,
    pub strength: f64,
    pub evidence_doc_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityNode {
    pub entity: Entity,
    pub links: Vec<EntityLink>,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeMap {
    pub entities: Vec<Entity>,
    pub links: Vec<EntityLink>,
    pub total_entities: usize,
    pub total_links: usize,
}

impl Database {
    /// Get entities related to a given entity, up to specified depth.
    pub fn get_related_entities(
        &self,
        entity_id: &str,
        max_depth: u32,
    ) -> Result<Vec<EntityNode>, CoreError> {
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut result: Vec<EntityNode> = Vec::new();
        let mut frontier: Vec<(String, u32)> = vec![(entity_id.to_string(), 0)];

        while let Some((eid, depth)) = frontier.pop() {
            if depth > max_depth || visited.contains(&eid) {
                continue;
            }
            visited.insert(eid.clone());

            if let Ok(entity) = self.get_entity_by_id(&eid) {
                let links = self.get_entity_links(&eid)?;
                for link in &links {
                    let next = if link.source_entity_id == eid {
                        &link.target_entity_id
                    } else {
                        &link.source_entity_id
                    };
                    if !visited.contains(next) {
                        frontier.push((next.clone(), depth + 1));
                    }
                }
                result.push(EntityNode {
                    entity,
                    links,
                    depth,
                });
            }
        }

        Ok(result)
    }

    /// Find shortest path between two entities (BFS).
    pub fn find_entity_path(
        &self,
        from_id: &str,
        to_id: &str,
    ) -> Result<Option<Vec<Entity>>, CoreError> {
        use std::collections::{HashMap, VecDeque};
        let mut visited: HashMap<String, String> = HashMap::new(); // child -> parent
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(from_id.to_string());
        visited.insert(from_id.to_string(), String::new());

        while let Some(current) = queue.pop_front() {
            if current == to_id {
                // Reconstruct path
                let mut path = Vec::new();
                let mut c = to_id.to_string();
                while !c.is_empty() {
                    if let Ok(entity) = self.get_entity_by_id(&c) {
                        path.push(entity);
                    }
                    c = visited.get(&c).cloned().unwrap_or_default();
                }
                path.reverse();
                return Ok(Some(path));
            }

            let links = self.get_entity_links(&current)?;
            for link in links {
                let next = if link.source_entity_id == current {
                    link.target_entity_id
                } else {
                    link.source_entity_id
                };
                if !visited.contains_key(&next) {
                    visited.insert(next.clone(), current.clone());
                    queue.push_back(next);
                }
            }
        }

        Ok(None)
    }

    /// Get the full knowledge map (limited to top N entities by mention count).
    pub fn get_knowledge_map(&self, limit: usize) -> Result<KnowledgeMap, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, entity_type, description, first_seen_doc, mention_count, created_at FROM entities ORDER BY mention_count DESC LIMIT ?1",
        )?;
        let entities: Vec<Entity> = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(Entity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: parse_entity_type(&row.get::<_, String>(2)?),
                    description: row.get(3)?,
                    first_seen_doc: row.get(4)?,
                    mention_count: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let entity_ids: Vec<String> = entities.iter().map(|e| e.id.clone()).collect();
        let links = if entity_ids.is_empty() {
            Vec::new()
        } else {
            // Build query with the correct number of parameters
            let placeholders: Vec<String> =
                (1..=entity_ids.len()).map(|i| format!("?{i}")).collect();
            let ph = placeholders.join(",");
            let offset = entity_ids.len();
            let placeholders2: Vec<String> = (1..=entity_ids.len())
                .map(|i| format!("?{}", i + offset))
                .collect();
            let ph2 = placeholders2.join(",");
            let sql = format!(
                "SELECT id, source_entity_id, target_entity_id, relation_type, strength, evidence_doc_id FROM entity_links WHERE source_entity_id IN ({ph}) OR target_entity_id IN ({ph2})"
            );
            let mut stmt = conn.prepare(&sql)?;
            // Double the params for both IN clauses
            let mut all_params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
            for id in &entity_ids {
                all_params.push(id as &dyn rusqlite::types::ToSql);
            }
            for id in &entity_ids {
                all_params.push(id as &dyn rusqlite::types::ToSql);
            }
            let rows = stmt
                .query_map(all_params.as_slice(), |row| {
                    Ok(EntityLink {
                        id: row.get(0)?,
                        source_entity_id: row.get(1)?,
                        target_entity_id: row.get(2)?,
                        relation_type: row.get(3)?,
                        strength: row.get(4)?,
                        evidence_doc_id: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };

        let total_entities = entities.len();
        let total_links = links.len();
        Ok(KnowledgeMap {
            entities,
            links,
            total_entities,
            total_links,
        })
    }

    pub fn get_entity_by_id(&self, entity_id: &str) -> Result<Entity, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, entity_type, description, first_seen_doc, mention_count, created_at FROM entities WHERE id = ?1",
            rusqlite::params![entity_id],
            |row| {
                Ok(Entity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: parse_entity_type(&row.get::<_, String>(2)?),
                    description: row.get(3)?,
                    first_seen_doc: row.get(4)?,
                    mention_count: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(|_| CoreError::NotFound("Entity not found".into()))
    }

    pub fn get_entity_links(&self, entity_id: &str) -> Result<Vec<EntityLink>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, source_entity_id, target_entity_id, relation_type, strength, evidence_doc_id FROM entity_links WHERE source_entity_id = ?1 OR target_entity_id = ?1",
        )?;
        let links = stmt
            .query_map(rusqlite::params![entity_id], |row| {
                Ok(EntityLink {
                    id: row.get(0)?,
                    source_entity_id: row.get(1)?,
                    target_entity_id: row.get(2)?,
                    relation_type: row.get(3)?,
                    strength: row.get(4)?,
                    evidence_doc_id: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(links)
    }

    pub fn search_entities(&self, query: &str) -> Result<Vec<Entity>, CoreError> {
        let conn = self.conn();
        let pattern = format!("%{query}%");
        let mut stmt = conn.prepare(
            "SELECT id, name, entity_type, description, first_seen_doc, mention_count, created_at FROM entities WHERE name LIKE ?1 OR description LIKE ?1 ORDER BY mention_count DESC LIMIT 20",
        )?;
        let entities = stmt
            .query_map(rusqlite::params![pattern], |row| {
                Ok(Entity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: parse_entity_type(&row.get::<_, String>(2)?),
                    description: row.get(3)?,
                    first_seen_doc: row.get(4)?,
                    mention_count: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entities)
    }
}
