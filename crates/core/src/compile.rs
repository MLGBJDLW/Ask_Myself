//! Knowledge compilation layer — Karpathy-inspired "raw → compile → wiki" pipeline.
//! Automatically distills documents into structured summaries, entities, and relationships.

use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};

// ── Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub document_id: String,
    pub summary: String,
    pub key_points: Vec<String>,
    pub tags: Vec<String>,
    pub model_used: String,
    pub compiled_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub entity_type: EntityType,
    pub description: String,
    pub first_seen_doc: Option<String>,
    pub mention_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Concept,
    Person,
    Technology,
    Event,
    Organization,
    Place,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub document_id: String,
    pub summary: DocumentSummary,
    pub entities_found: usize,
    pub links_created: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileStats {
    pub total_docs: i64,
    pub compiled_docs: i64,
    pub total_entities: i64,
    pub total_links: i64,
}

// ── LLM Response Parsing ──

#[derive(Deserialize)]
struct LlmCompileOutput {
    summary: String,
    key_points: Vec<String>,
    tags: Vec<String>,
    entities: Vec<LlmEntity>,
}

#[derive(Deserialize)]
struct LlmEntity {
    name: String,
    entity_type: String,
    description: String,
    context: String,
    relations: Vec<LlmRelation>,
}

#[derive(Deserialize)]
struct LlmRelation {
    target: String,
    relation_type: String,
}

// ── Constants ──

const COMPILE_SYSTEM_PROMPT: &str = include_str!("../prompts/compile.md");

// ── Core Functions ──

/// Compile a single document: generate summary + extract entities + build relationships.
pub async fn compile_document(
    db: &Database,
    doc_id: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<CompileResult, CoreError> {
    // 1. Get document content (join chunks)
    let content = db.get_document_full_text(doc_id)?;
    if content.trim().is_empty() {
        return Err(CoreError::InvalidInput("Document has no content".into()));
    }

    // Truncate to ~8000 chars to stay within token limits
    let truncated = if content.len() > 8000 {
        &content[..8000]
    } else {
        &content
    };

    // 2. Call LLM to compile
    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, COMPILE_SYSTEM_PROMPT.to_string()),
            Message::text(Role::User, format!("Compile this document:\n\n{truncated}")),
        ],
        max_tokens: Some(2000),
        temperature: Some(0.2),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
    };

    let response = provider.complete(&request).await?;
    let output: LlmCompileOutput = serde_json::from_str(response.content.trim())
        .map_err(|e| CoreError::InvalidInput(format!("LLM returned invalid JSON: {e}")))?;

    // 3. Store summary
    let summary = db.upsert_document_summary(
        doc_id,
        &output.summary,
        &output.key_points,
        &output.tags,
        model,
    )?;

    // 4. Store entities and relationships
    let mut entities_found = 0;
    let mut links_created = 0;

    for llm_entity in &output.entities {
        let entity_type = parse_entity_type(&llm_entity.entity_type);
        let entity = db.upsert_entity(
            &llm_entity.name,
            &entity_type,
            &llm_entity.description,
            doc_id,
        )?;
        db.link_document_entity(doc_id, &entity.id, 1.0, &llm_entity.context)?;
        entities_found += 1;

        // Create relationships
        for rel in &llm_entity.relations {
            if let Ok(target) = db.find_entity_by_name(&rel.target) {
                db.upsert_entity_link(
                    &entity.id,
                    &target.id,
                    &rel.relation_type,
                    1.0,
                    Some(doc_id),
                )?;
                links_created += 1;
            }
        }
    }

    Ok(CompileResult {
        document_id: doc_id.to_string(),
        summary,
        entities_found,
        links_created,
    })
}

/// Progress information emitted during compilation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileProgress {
    pub current: usize,
    pub total: usize,
    pub document_id: String,
    pub document_title: Option<String>,
    pub phase: String,
}

/// Compile all documents that haven't been compiled yet.
pub async fn compile_pending(
    db: &Database,
    provider: &dyn LlmProvider,
    model: &str,
    limit: usize,
) -> Result<Vec<CompileResult>, CoreError> {
    compile_pending_with_progress(db, provider, model, limit, |_| {}).await
}

/// Compile all documents that haven't been compiled yet, with progress reporting.
pub async fn compile_pending_with_progress<F>(
    db: &Database,
    provider: &dyn LlmProvider,
    model: &str,
    limit: usize,
    on_progress: F,
) -> Result<Vec<CompileResult>, CoreError>
where
    F: Fn(&CompileProgress),
{
    let pending_ids = db.get_uncompiled_document_ids(limit)?;
    let total = pending_ids.len();
    let mut results = Vec::new();
    let per_doc_timeout = Duration::from_secs(120);

    for (i, doc_id) in pending_ids.iter().enumerate() {
        let title = db.get_document_title(doc_id).ok().flatten();
        on_progress(&CompileProgress {
            current: i + 1,
            total,
            document_id: doc_id.clone(),
            document_title: title.clone(),
            phase: "compiling".to_string(),
        });

        match timeout(
            per_doc_timeout,
            compile_document(db, doc_id, provider, model),
        )
        .await
        {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(e)) => {
                tracing::warn!("compile doc {doc_id}: {e}");
                on_progress(&CompileProgress {
                    current: i + 1,
                    total,
                    document_id: doc_id.clone(),
                    document_title: title.clone(),
                    phase: "error".to_string(),
                });
            }
            Err(_) => {
                tracing::warn!("compile doc {doc_id}: timed out after 120s");
                on_progress(&CompileProgress {
                    current: i + 1,
                    total,
                    document_id: doc_id.clone(),
                    document_title: title,
                    phase: "timeout".to_string(),
                });
            }
        }
    }

    Ok(results)
}

pub fn parse_entity_type(s: &str) -> EntityType {
    match s.to_lowercase().as_str() {
        "concept" => EntityType::Concept,
        "person" => EntityType::Person,
        "technology" => EntityType::Technology,
        "event" => EntityType::Event,
        "organization" => EntityType::Organization,
        "place" => EntityType::Place,
        _ => EntityType::Other,
    }
}

// ── Database Methods ──

impl Database {
    pub fn get_document_full_text(&self, doc_id: &str) -> Result<String, CoreError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT content FROM chunks WHERE document_id = ? ORDER BY chunk_index ASC")?;
        let chunks: Vec<String> = stmt
            .query_map(rusqlite::params![doc_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(chunks.join("\n\n"))
    }

    pub fn upsert_document_summary(
        &self,
        doc_id: &str,
        summary: &str,
        key_points: &[String],
        tags: &[String],
        model: &str,
    ) -> Result<DocumentSummary, CoreError> {
        let conn = self.conn();
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let key_points_json = serde_json::to_string(key_points).unwrap_or_default();
        let tags_json = serde_json::to_string(tags).unwrap_or_default();

        conn.execute(
            "INSERT INTO document_summaries (id, document_id, summary, key_points, tags, model_used, compiled_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(document_id) DO UPDATE SET
                summary = excluded.summary,
                key_points = excluded.key_points,
                tags = excluded.tags,
                model_used = excluded.model_used,
                updated_at = excluded.updated_at",
            rusqlite::params![id, doc_id, summary, key_points_json, tags_json, model, now],
        )?;

        Ok(DocumentSummary {
            id,
            document_id: doc_id.to_string(),
            summary: summary.to_string(),
            key_points: key_points.to_vec(),
            tags: tags.to_vec(),
            model_used: model.to_string(),
            compiled_at: now,
        })
    }

    pub fn upsert_entity(
        &self,
        name: &str,
        entity_type: &EntityType,
        description: &str,
        first_doc: &str,
    ) -> Result<Entity, CoreError> {
        let conn = self.conn();
        let type_str = serde_json::to_value(entity_type)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "other".to_string());
        let now = chrono::Utc::now().to_rfc3339();

        // Try to find existing
        let existing = conn.query_row(
            "SELECT id, mention_count FROM entities WHERE name = ?1 AND entity_type = ?2",
            rusqlite::params![name, type_str],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        );

        match existing {
            Ok((id, count)) => {
                conn.execute(
                    "UPDATE entities SET mention_count = ?1, description = CASE WHEN length(?2) > length(description) THEN ?2 ELSE description END, updated_at = ?3 WHERE id = ?4",
                    rusqlite::params![count + 1, description, now, id],
                )?;
                Ok(Entity {
                    id,
                    name: name.to_string(),
                    entity_type: entity_type.clone(),
                    description: description.to_string(),
                    first_seen_doc: Some(first_doc.to_string()),
                    mention_count: count + 1,
                    created_at: now,
                })
            }
            Err(_) => {
                let id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO entities (id, name, entity_type, description, first_seen_doc, mention_count, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                    rusqlite::params![id, name, type_str, description, first_doc, now],
                )?;
                Ok(Entity {
                    id,
                    name: name.to_string(),
                    entity_type: entity_type.clone(),
                    description: description.to_string(),
                    first_seen_doc: Some(first_doc.to_string()),
                    mention_count: 1,
                    created_at: now,
                })
            }
        }
    }

    pub fn find_entity_by_name(&self, name: &str) -> Result<Entity, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, entity_type, description, first_seen_doc, mention_count, created_at FROM entities WHERE name = ?1 COLLATE NOCASE",
            rusqlite::params![name],
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

    pub fn link_document_entity(
        &self,
        doc_id: &str,
        entity_id: &str,
        relevance: f64,
        context: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO document_entities (document_id, entity_id, relevance, context_snippet) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO UPDATE SET relevance = excluded.relevance, context_snippet = excluded.context_snippet",
            rusqlite::params![doc_id, entity_id, relevance, context],
        )?;
        Ok(())
    }

    pub fn upsert_entity_link(
        &self,
        source_id: &str,
        target_id: &str,
        relation_type: &str,
        strength: f64,
        evidence_doc: Option<&str>,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO entity_links (id, source_entity_id, target_entity_id, relation_type, strength, evidence_doc_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(source_entity_id, target_entity_id, relation_type) DO UPDATE SET strength = strength + 0.1",
            rusqlite::params![id, source_id, target_id, relation_type, strength, evidence_doc],
        )?;
        Ok(())
    }

    pub fn get_uncompiled_document_ids(&self, limit: usize) -> Result<Vec<String>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT d.id FROM documents d LEFT JOIN document_summaries ds ON d.id = ds.document_id WHERE ds.id IS NULL LIMIT ?1",
        )?;
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params![limit as i64], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn get_document_title(&self, doc_id: &str) -> Result<Option<String>, CoreError> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT title FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(title) => Ok(Some(title)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_document_summary(&self, doc_id: &str) -> Result<Option<DocumentSummary>, CoreError> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT id, document_id, summary, key_points, tags, model_used, compiled_at FROM document_summaries WHERE document_id = ?1",
            rusqlite::params![doc_id],
            |row| {
                let kp: String = row.get(3)?;
                let tags: String = row.get(4)?;
                Ok(DocumentSummary {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    summary: row.get(2)?,
                    key_points: serde_json::from_str(&kp).unwrap_or_default(),
                    tags: serde_json::from_str(&tags).unwrap_or_default(),
                    model_used: row.get(5)?,
                    compiled_at: row.get(6)?,
                })
            },
        );
        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_entities_for_document(&self, doc_id: &str) -> Result<Vec<Entity>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count, e.created_at FROM entities e JOIN document_entities de ON e.id = de.entity_id WHERE de.document_id = ?1 ORDER BY de.relevance DESC",
        )?;
        let entities = stmt
            .query_map(rusqlite::params![doc_id], |row| {
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

    pub fn get_compile_stats(&self) -> Result<CompileStats, CoreError> {
        let conn = self.conn();
        let total_docs: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let compiled_docs: i64 =
            conn.query_row("SELECT COUNT(*) FROM document_summaries", [], |r| r.get(0))?;
        let total_entities: i64 =
            conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;
        let total_links: i64 =
            conn.query_row("SELECT COUNT(*) FROM entity_links", [], |r| r.get(0))?;
        Ok(CompileStats {
            total_docs,
            compiled_docs,
            total_entities,
            total_links,
        })
    }
}
