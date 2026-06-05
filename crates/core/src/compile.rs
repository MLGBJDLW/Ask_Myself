//! Knowledge compilation layer — Karpathy-inspired "raw → compile → wiki" pipeline.
//! Automatically distills documents into structured summaries, entities, and relationships.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, ProviderType, Role};

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
const COMPILE_INPUT_CHAR_BUDGET: usize = 12_000;

// ── Core Functions ──

/// Compile a single document: generate summary + extract entities + build relationships.
pub async fn compile_document(
    db: &Database,
    doc_id: &str,
    provider: &dyn LlmProvider,
    model: &str,
    provider_type: Option<ProviderType>,
) -> Result<CompileResult, CoreError> {
    // 1. Get document content (join chunks)
    let content = db.get_document_full_text(doc_id)?;
    if content.trim().is_empty() {
        return Err(CoreError::InvalidInput("Document has no content".into()));
    }

    let compile_input = build_compile_input_excerpt(&content, COMPILE_INPUT_CHAR_BUDGET);

    // 2. Call LLM to compile
    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, COMPILE_SYSTEM_PROMPT.to_string()),
            Message::text(
                Role::User,
                format!("Compile this document:\n\n{compile_input}"),
            ),
        ],
        max_tokens: Some(2000),
        temperature: Some(0.2),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type,
        parallel_tool_calls: true,
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

    // 3b. Index compiled output for FTS search
    db.upsert_summary_chunk(doc_id, &output.summary, &output.key_points, &output.tags)?;

    // 4. Store entities and relationships
    let mut entities_found = 0;
    let mut links_created = 0;

    let mut entity_ids_by_name: HashMap<String, String> = HashMap::new();
    for llm_entity in &output.entities {
        let entity_type = parse_entity_type(&llm_entity.entity_type);
        let entity = db.upsert_entity(
            &llm_entity.name,
            &entity_type,
            &llm_entity.description,
            doc_id,
        )?;
        db.link_document_entity(doc_id, &entity.id, 1.0, &llm_entity.context)?;
        entity_ids_by_name.insert(normalize_entity_lookup_name(&llm_entity.name), entity.id);
        entities_found += 1;
    }

    for llm_entity in &output.entities {
        let Some(source_id) = entity_ids_by_name
            .get(&normalize_entity_lookup_name(&llm_entity.name))
            .cloned()
        else {
            continue;
        };
        for rel in &llm_entity.relations {
            let target_id = entity_ids_by_name
                .get(&normalize_entity_lookup_name(&rel.target))
                .cloned()
                .or_else(|| {
                    db.find_entity_by_name(&rel.target)
                        .ok()
                        .map(|entity| entity.id)
                });

            if let Some(target_id) = target_id {
                db.upsert_entity_link(
                    &source_id,
                    &target_id,
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

fn normalize_entity_lookup_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn build_compile_input_excerpt(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    let head_budget = (max_chars as f32 * 0.45).round() as usize;
    let middle_budget = (max_chars as f32 * 0.20).round() as usize;
    let tail_budget = max_chars.saturating_sub(head_budget + middle_budget);
    let total_chars = content.chars().count();

    let head = take_chars(content, head_budget);
    let middle_start = total_chars.saturating_sub(middle_budget).saturating_div(2);
    let middle = skip_take_chars(content, middle_start, middle_budget);
    let tail_start = total_chars.saturating_sub(tail_budget);
    let tail = skip_take_chars(content, tail_start, tail_budget);

    format!(
        "## Document Excerpt\n\
         The source document is longer than the compile input budget. This excerpt preserves the beginning, middle, and end so conclusions are not based only on the opening section.\n\n\
         ### Beginning\n{head}\n\n\
         ### Middle\n{middle}\n\n\
         ### End\n{tail}"
    )
}

fn take_chars(content: &str, count: usize) -> String {
    content.chars().take(count).collect()
}

fn skip_take_chars(content: &str, skip: usize, count: usize) -> String {
    content.chars().skip(skip).take(count).collect()
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
    provider_type: Option<ProviderType>,
    limit: usize,
) -> Result<Vec<CompileResult>, CoreError> {
    compile_pending_with_progress(db, provider, model, provider_type, limit, |_| {}).await
}

/// Compile all documents that haven't been compiled yet, with progress reporting.
pub async fn compile_pending_with_progress<F>(
    db: &Database,
    provider: &dyn LlmProvider,
    model: &str,
    provider_type: Option<ProviderType>,
    limit: usize,
    on_progress: F,
) -> Result<Vec<CompileResult>, CoreError>
where
    F: Fn(&CompileProgress),
{
    let pending_ids = db.get_uncompiled_document_ids(limit)?;
    let total = pending_ids.len();
    let mut results = Vec::new();

    for (i, doc_id) in pending_ids.iter().enumerate() {
        let title = db.get_document_title(doc_id).ok().flatten();
        on_progress(&CompileProgress {
            current: i + 1,
            total,
            document_id: doc_id.clone(),
            document_title: title.clone(),
            phase: "compiling".to_string(),
        });

        match compile_document(db, doc_id, provider, model, provider_type).await {
            Ok(result) => results.push(result),
            Err(e) => {
                tracing::warn!("compile doc {doc_id}: {e}");
                on_progress(&CompileProgress {
                    current: i + 1,
                    total,
                    document_id: doc_id.clone(),
                    document_title: title.clone(),
                    phase: "error".to_string(),
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

    /// Insert (or replace) a synthetic chunk containing the compiled summary,
    /// key-points and tags so that FTS5 triggers make them searchable.
    /// Uses `chunk_index = -1` and `kind = 'summary'` to distinguish from
    /// content chunks.
    pub fn upsert_summary_chunk(
        &self,
        doc_id: &str,
        summary: &str,
        key_points: &[String],
        tags: &[String],
    ) -> Result<(), CoreError> {
        let mut parts = Vec::with_capacity(3);
        if !summary.is_empty() {
            parts.push(summary.to_string());
        }
        if !key_points.is_empty() {
            parts.push(key_points.join("\n"));
        }
        if !tags.is_empty() {
            parts.push(tags.join(", "));
        }
        let content = parts.join("\n\n");
        if content.trim().is_empty() {
            return Ok(());
        }

        let conn = self.conn();
        let id = uuid::Uuid::new_v4().to_string();
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let len = content.len() as i64;

        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, -1, 'summary', ?3, 0, ?4, 0, 0, ?5)
             ON CONFLICT(document_id, chunk_index) DO UPDATE SET
                content = excluded.content,
                end_offset = excluded.end_offset,
                content_hash = excluded.content_hash",
            rusqlite::params![id, doc_id, content, len, hash],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{CompletionResponse, FinishReason, StreamChunk, Usage};
    use crate::sources::CreateSourceInput;
    use futures::stream::BoxStream;

    struct StaticLlmProvider {
        content: String,
    }

    #[async_trait::async_trait]
    impl LlmProvider for StaticLlmProvider {
        fn name(&self) -> &str {
            "static"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(vec!["test-model".to_string()])
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            Ok(CompletionResponse {
                content: self.content.clone(),
                tool_calls: None,
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                thinking: None,
            })
        }

        async fn stream(
            &self,
            _request: &CompletionRequest,
        ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn insert_compile_doc(db: &Database, content: &str) -> String {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("add source");
        let doc_id = uuid::Uuid::new_v4().to_string();
        let doc_path = dir.path().join("chapter.md").to_string_lossy().to_string();
        db.conn()
            .execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, ?2, ?3, 'Chapter', 'text/markdown', 100, datetime('now'), 'doc-hash')",
                rusqlite::params![doc_id, source.id, doc_path],
            )
            .expect("insert document");
        db.conn()
            .execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, 0, 'text', ?3, 0, ?4, 1, 1, 'chunk-hash')",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    doc_id,
                    content,
                    content.len() as i64
                ],
            )
            .expect("insert chunk");
        doc_id
    }

    #[test]
    fn compile_excerpt_keeps_beginning_middle_and_end() {
        let content = format!(
            "{}\n{}\n{}",
            "BEGIN ".repeat(3000),
            "MIDDLE ".repeat(3000),
            "ENDMARK ".repeat(3000)
        );

        let excerpt = build_compile_input_excerpt(&content, 1200);

        assert!(excerpt.contains("### Beginning"));
        assert!(excerpt.contains("BEGIN"));
        assert!(excerpt.contains("### Middle"));
        assert!(excerpt.contains("MIDDLE"));
        assert!(excerpt.contains("### End"));
        assert!(excerpt.contains("ENDMARK"));
    }

    #[test]
    fn compile_excerpt_is_utf8_safe() {
        let content = format!(
            "{}{}{}",
            "开始".repeat(3000),
            "中段".repeat(3000),
            "结尾".repeat(3000)
        );

        let excerpt = build_compile_input_excerpt(&content, 999);

        assert!(excerpt.contains("开始"));
        assert!(excerpt.contains("中段"));
        assert!(excerpt.contains("结尾"));
    }

    #[test]
    fn compile_excerpt_returns_short_content_unchanged() {
        let content = "short document";
        assert_eq!(build_compile_input_excerpt(content, 1200), content);
    }

    #[tokio::test]
    async fn compile_document_links_relations_to_entities_later_in_same_output() {
        let db = Database::open_memory().expect("open memory");
        let doc_id = insert_compile_doc(&db, "Princess meets Dragon.");
        let provider = StaticLlmProvider {
            content: serde_json::json!({
                "summary": "A princess meets a dragon.",
                "key_points": ["Princess and Dragon are in the same scene."],
                "tags": ["novel"],
                "entities": [
                    {
                        "name": "Princess",
                        "entity_type": "person",
                        "description": "A protagonist.",
                        "context": "Princess meets Dragon.",
                        "relations": [
                            { "target": "Dragon", "relation_type": "enemy_of" }
                        ]
                    },
                    {
                        "name": "Dragon",
                        "entity_type": "person",
                        "description": "A rival.",
                        "context": "Princess meets Dragon.",
                        "relations": []
                    }
                ]
            })
            .to_string(),
        };

        let result = compile_document(&db, &doc_id, &provider, "test-model", None)
            .await
            .expect("compile document");

        assert_eq!(result.entities_found, 2);
        assert_eq!(result.links_created, 1);
        let document_entities: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM document_entities", [], |row| {
                row.get(0)
            })
            .expect("document entity count");
        assert_eq!(document_entities, 2);
        let entity_links: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM entity_links", [], |row| row.get(0))
            .expect("entity link count");
        assert_eq!(entity_links, 1);
    }
}
