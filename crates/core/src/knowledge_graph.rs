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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraphDocumentRef {
    pub document_id: String,
    pub title: String,
    pub path: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraphNode {
    pub id: String,
    pub label: String,
    pub entity_type: String,
    pub description: String,
    pub mention_count: i64,
    pub document_count: i64,
    pub link_count: i64,
    pub first_seen_doc: Option<String>,
    pub documents: Vec<KnowledgeGraphDocumentRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub strength: f64,
    pub evidence_doc_id: Option<String>,
    pub evidence_title: Option<String>,
    pub evidence_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeGraph {
    pub nodes: Vec<KnowledgeGraphNode>,
    pub edges: Vec<KnowledgeGraphEdge>,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub scope_label: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KnowledgeGraphQuery {
    pub limit: usize,
    pub source_id: Option<String>,
    pub path_prefix: Option<String>,
    pub entity_types: Vec<String>,
    pub relation_types: Vec<String>,
    pub min_strength: Option<f64>,
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

    /// Get a UI-ready, evidence-scoped relationship graph.
    pub fn get_knowledge_graph(
        &self,
        query: KnowledgeGraphQuery,
    ) -> Result<KnowledgeGraph, CoreError> {
        use rusqlite::types::Value;

        let conn = self.conn();
        let limit = query.limit.clamp(1, 250);
        let source_root = match query.source_id.as_deref() {
            Some(source_id) if !source_id.trim().is_empty() => Some(
                conn.query_row(
                    "SELECT root_path FROM sources WHERE id = ?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        CoreError::NotFound(format!("Source not found: {source_id}"))
                    }
                    other => CoreError::Database(other),
                })?,
            ),
            _ => None,
        };
        let path_pattern =
            scoped_path_pattern(source_root.as_deref(), query.path_prefix.as_deref());

        let mut where_parts = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if let Some(source_id) = query
            .source_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            where_parts.push("d.source_id = ?".to_string());
            params.push(Value::Text(source_id.to_string()));
        }
        if let Some(pattern) = path_pattern.as_deref() {
            where_parts.push("d.path LIKE ?".to_string());
            params.push(Value::Text(pattern.to_string()));
        }
        if !query.entity_types.is_empty() {
            let placeholders = repeat_placeholders(query.entity_types.len());
            where_parts.push(format!("e.entity_type IN ({placeholders})"));
            params.extend(
                query
                    .entity_types
                    .iter()
                    .map(|value| Value::Text(value.clone())),
            );
        }

        let where_sql = if where_parts.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_parts.join(" AND "))
        };

        let sql = format!(
            "SELECT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count,
                    COUNT(DISTINCT de.document_id) AS document_count,
                    (SELECT COUNT(*) FROM entity_links el WHERE el.source_entity_id = e.id OR el.target_entity_id = e.id) AS link_count
             FROM entities e
             JOIN document_entities de ON e.id = de.entity_id
             JOIN documents d ON d.id = de.document_id
             {where_sql}
             GROUP BY e.id
             ORDER BY document_count DESC, e.mention_count DESC, e.name COLLATE NOCASE
             LIMIT ?",
        );
        params.push(Value::Integer(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let nodes_seed: Vec<KnowledgeGraphNode> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(KnowledgeGraphNode {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    entity_type: row.get(2)?,
                    description: row.get(3)?,
                    first_seen_doc: row.get(4)?,
                    mention_count: row.get(5)?,
                    document_count: row.get(6)?,
                    link_count: row.get(7)?,
                    documents: Vec::new(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let node_ids: Vec<String> = nodes_seed.iter().map(|node| node.id.clone()).collect();
        let mut nodes = Vec::with_capacity(nodes_seed.len());
        for mut node in nodes_seed {
            node.documents = query_entity_documents(
                &conn,
                &node.id,
                query.source_id.as_deref(),
                path_pattern.as_deref(),
                5,
            )?;
            nodes.push(node);
        }

        let edges = if node_ids.is_empty() {
            Vec::new()
        } else {
            query_graph_edges(
                &conn,
                &node_ids,
                query.source_id.as_deref(),
                path_pattern.as_deref(),
                &query.relation_types,
                query.min_strength.unwrap_or(0.0),
            )?
        };

        let scope_label = query.source_id.as_ref().map(|source_id| {
            match query
                .path_prefix
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                Some(path) => format!("{source_id}:{path}"),
                None => source_id.clone(),
            }
        });
        let total_nodes = nodes.len();
        let total_edges = edges.len();

        Ok(KnowledgeGraph {
            nodes,
            edges,
            total_nodes,
            total_edges,
            scope_label,
        })
    }
}

fn repeat_placeholders(count: usize) -> String {
    std::iter::repeat("?")
        .take(count)
        .collect::<Vec<_>>()
        .join(",")
}

fn scoped_path_pattern(source_root: Option<&str>, path_prefix: Option<&str>) -> Option<String> {
    let prefix = path_prefix?.trim();
    if prefix.is_empty() {
        return None;
    }

    let absolute = if let Some(root) = source_root {
        std::path::Path::new(root)
            .join(prefix)
            .to_string_lossy()
            .to_string()
    } else {
        prefix.to_string()
    };
    let trimmed = absolute.trim_end_matches(|c| c == '/' || c == '\\');
    Some(format!("{trimmed}%"))
}

fn query_entity_documents(
    conn: &rusqlite::Connection,
    entity_id: &str,
    source_id: Option<&str>,
    path_pattern: Option<&str>,
    limit: usize,
) -> Result<Vec<KnowledgeGraphDocumentRef>, CoreError> {
    use rusqlite::types::Value;

    let mut where_parts = vec!["de.entity_id = ?".to_string()];
    let mut params = vec![Value::Text(entity_id.to_string())];
    if let Some(source_id) = source_id.filter(|value| !value.trim().is_empty()) {
        where_parts.push("d.source_id = ?".to_string());
        params.push(Value::Text(source_id.to_string()));
    }
    if let Some(pattern) = path_pattern {
        where_parts.push("d.path LIKE ?".to_string());
        params.push(Value::Text(pattern.to_string()));
    }
    params.push(Value::Integer(limit as i64));

    let sql = format!(
        "SELECT d.id, COALESCE(d.title, d.path), d.path, d.source_id
         FROM document_entities de
         JOIN documents d ON d.id = de.document_id
         WHERE {}
         ORDER BY de.relevance DESC, d.modified_at DESC
         LIMIT ?",
        where_parts.join(" AND "),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(KnowledgeGraphDocumentRef {
                document_id: row.get(0)?,
                title: row.get(1)?,
                path: row.get(2)?,
                source_id: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::EntityType;
    use crate::sources::CreateSourceInput;

    fn insert_doc(db: &Database, source_id: &str, path: &str, title: &str) -> String {
        let doc_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, ?2, ?3, ?4, 'text/markdown', 100, datetime('now'), ?5)",
                rusqlite::params![doc_id, source_id, path, title, format!("hash-{title}")],
            )
            .expect("insert document");
        doc_id
    }

    #[test]
    fn scoped_graph_filters_nodes_edges_and_documents() {
        let db = Database::open_memory().expect("open memory");
        let dir = tempfile::tempdir().expect("tempdir");
        let other_dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("add source");
        let other_source = db
            .add_source(CreateSourceInput {
                root_path: other_dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("add other source");

        let scoped_doc_path = dir.path().join("novel").join("chapter-1.md");
        let scoped_doc = insert_doc(
            &db,
            &source.id,
            &scoped_doc_path.to_string_lossy(),
            "Chapter 1",
        );
        let other_doc_path = other_dir.path().join("notes.md");
        let other_doc = insert_doc(
            &db,
            &other_source.id,
            &other_doc_path.to_string_lossy(),
            "Other Notes",
        );

        let hero = db
            .upsert_entity("Lin", &EntityType::Person, "Lead character", &scoped_doc)
            .expect("hero");
        let city = db
            .upsert_entity("Mirror City", &EntityType::Place, "Main city", &scoped_doc)
            .expect("city");
        let outside = db
            .upsert_entity(
                "External Topic",
                &EntityType::Concept,
                "Outside",
                &other_doc,
            )
            .expect("outside");

        db.link_document_entity(&scoped_doc, &hero.id, 1.0, "Lin arrives")
            .expect("link hero");
        db.link_document_entity(&scoped_doc, &city.id, 1.0, "Mirror City")
            .expect("link city");
        db.link_document_entity(&other_doc, &outside.id, 1.0, "External Topic")
            .expect("link outside");
        db.upsert_entity_link(&hero.id, &city.id, "located_in", 1.0, Some(&scoped_doc))
            .expect("edge");
        db.upsert_entity_link(&hero.id, &outside.id, "related_to", 1.0, Some(&other_doc))
            .expect("outside edge");

        let graph = db
            .get_knowledge_graph(KnowledgeGraphQuery {
                limit: 20,
                source_id: Some(source.id.clone()),
                path_prefix: Some("novel".to_string()),
                ..KnowledgeGraphQuery::default()
            })
            .expect("graph");

        assert_eq!(graph.total_nodes, 2);
        assert_eq!(graph.total_edges, 1);
        assert!(graph.nodes.iter().any(|node| node.label == "Lin"));
        assert!(graph.nodes.iter().any(|node| node.label == "Mirror City"));
        assert!(!graph
            .nodes
            .iter()
            .any(|node| node.label == "External Topic"));
        assert_eq!(graph.edges[0].relation_type, "located_in");
        assert_eq!(graph.nodes[0].documents.len(), 1);
    }

    #[test]
    fn graph_respects_entity_and_relation_filters() {
        let db = Database::open_memory().expect("open memory");
        let dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("add source");
        let doc_path = dir.path().join("chapter.md");
        let doc = insert_doc(&db, &source.id, &doc_path.to_string_lossy(), "Chapter");
        let character = db
            .upsert_entity("Ada", &EntityType::Person, "A person", &doc)
            .expect("person");
        let place = db
            .upsert_entity("Archive", &EntityType::Place, "A place", &doc)
            .expect("place");
        db.link_document_entity(&doc, &character.id, 1.0, "Ada")
            .expect("link character");
        db.link_document_entity(&doc, &place.id, 1.0, "Archive")
            .expect("link place");
        db.upsert_entity_link(&character.id, &place.id, "located_in", 1.0, Some(&doc))
            .expect("edge");

        let person_only = db
            .get_knowledge_graph(KnowledgeGraphQuery {
                limit: 20,
                entity_types: vec!["person".to_string()],
                ..KnowledgeGraphQuery::default()
            })
            .expect("person graph");
        assert_eq!(person_only.total_nodes, 1);
        assert_eq!(person_only.nodes[0].label, "Ada");
        assert_eq!(person_only.total_edges, 0);

        let no_matching_relation = db
            .get_knowledge_graph(KnowledgeGraphQuery {
                limit: 20,
                relation_types: vec!["enemy_of".to_string()],
                ..KnowledgeGraphQuery::default()
            })
            .expect("relation graph");
        assert_eq!(no_matching_relation.total_nodes, 2);
        assert_eq!(no_matching_relation.total_edges, 0);
    }
}

fn query_graph_edges(
    conn: &rusqlite::Connection,
    node_ids: &[String],
    source_id: Option<&str>,
    path_pattern: Option<&str>,
    relation_types: &[String],
    min_strength: f64,
) -> Result<Vec<KnowledgeGraphEdge>, CoreError> {
    use rusqlite::types::Value;

    let source_placeholders = repeat_placeholders(node_ids.len());
    let target_placeholders = repeat_placeholders(node_ids.len());
    let mut where_parts = vec![
        format!("el.source_entity_id IN ({source_placeholders})"),
        format!("el.target_entity_id IN ({target_placeholders})"),
        "el.strength >= ?".to_string(),
    ];
    let mut params: Vec<Value> = node_ids.iter().map(|id| Value::Text(id.clone())).collect();
    params.extend(node_ids.iter().map(|id| Value::Text(id.clone())));
    params.push(Value::Real(min_strength));

    if let Some(source_id) = source_id.filter(|value| !value.trim().is_empty()) {
        where_parts.push("ed.source_id = ?".to_string());
        params.push(Value::Text(source_id.to_string()));
    }
    if let Some(pattern) = path_pattern {
        where_parts.push("ed.path LIKE ?".to_string());
        params.push(Value::Text(pattern.to_string()));
    }
    if !relation_types.is_empty() {
        where_parts.push(format!(
            "el.relation_type IN ({})",
            repeat_placeholders(relation_types.len())
        ));
        params.extend(
            relation_types
                .iter()
                .map(|value| Value::Text(value.clone())),
        );
    }

    let sql = format!(
        "SELECT el.id, el.source_entity_id, el.target_entity_id, el.relation_type, el.strength,
                el.evidence_doc_id, ed.title, ed.path
         FROM entity_links el
         LEFT JOIN documents ed ON ed.id = el.evidence_doc_id
         WHERE {}
         ORDER BY el.strength DESC, el.relation_type COLLATE NOCASE",
        where_parts.join(" AND "),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(KnowledgeGraphEdge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                relation_type: row.get(3)?,
                strength: row.get(4)?,
                evidence_doc_id: row.get(5)?,
                evidence_title: row.get(6)?,
                evidence_path: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
