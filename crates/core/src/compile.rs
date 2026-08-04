//! Knowledge compilation layer — Karpathy-inspired "raw → compile → wiki" pipeline.
//! Automatically distills documents into structured summaries, entities, and relationships.

use std::collections::{HashMap, HashSet};

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

pub struct EntityLinkEvidence<'a> {
    pub strength: f64,
    pub evidence_doc: Option<&'a str>,
    pub evidence_snippet: Option<&'a str>,
    pub confidence: Option<f64>,
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
    #[serde(default)]
    aliases: Vec<String>,
    entity_type: String,
    description: String,
    context: String,
    #[serde(default)]
    relations: Vec<LlmRelation>,
}

#[derive(Deserialize)]
struct LlmRelation {
    target: String,
    relation_type: String,
    evidence: Option<String>,
    confidence: Option<f64>,
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
        reasoning_enabled: None,
        reasoning_effort: None,
        provider_type,
        routing_session_id: None,
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
        let entity = db.upsert_entity_with_aliases(
            &llm_entity.name,
            &llm_entity.aliases,
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
                let strength = rel.confidence.unwrap_or(1.0).clamp(0.1, 1.0);
                db.upsert_entity_link_with_evidence(
                    &source_id,
                    &target_id,
                    &normalize_relation_type(&rel.relation_type),
                    EntityLinkEvidence {
                        strength,
                        evidence_doc: Some(doc_id),
                        evidence_snippet: rel.evidence.as_deref(),
                        confidence: rel.confidence,
                    },
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
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_lowercase()
}

fn normalize_entity_display_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_relation_type(relation_type: &str) -> String {
    let normalized = relation_type
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let collapsed = normalized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if collapsed.is_empty() {
        "related_to".to_string()
    } else {
        collapsed
    }
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

fn entity_type_key(entity_type: &EntityType) -> &'static str {
    match entity_type {
        EntityType::Concept => "concept",
        EntityType::Person => "person",
        EntityType::Technology => "technology",
        EntityType::Event => "event",
        EntityType::Organization => "organization",
        EntityType::Place => "place",
        EntityType::Other => "other",
    }
}

fn entity_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        name: row.get(1)?,
        entity_type: parse_entity_type(&row.get::<_, String>(2)?),
        description: row.get(3)?,
        first_seen_doc: row.get(4)?,
        mention_count: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn normalized_entity_aliases(name: &str, aliases: &[String]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    std::iter::once(name)
        .chain(aliases.iter().map(String::as_str))
        .filter_map(|alias| {
            let display = normalize_entity_display_name(alias);
            let normalized = normalize_entity_lookup_name(&display);
            if display.is_empty() || normalized.is_empty() || !seen.insert(normalized.clone()) {
                None
            } else {
                Some((display, normalized))
            }
        })
        .collect()
}

fn find_entity_by_aliases(
    conn: &rusqlite::Connection,
    aliases: &[(String, String)],
    entity_type: &str,
) -> Result<Option<Entity>, CoreError> {
    for (_, normalized_alias) in aliases {
        let found = conn.query_row(
            "SELECT DISTINCT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count, e.created_at
             FROM entities e
             LEFT JOIN entity_aliases ea ON ea.entity_id = e.id
             WHERE e.entity_type = ?2
               AND (ea.normalized_alias = ?1 OR lower(trim(e.name)) = ?1)
             ORDER BY e.mention_count DESC, e.name COLLATE NOCASE
             LIMIT 1",
            rusqlite::params![normalized_alias, entity_type],
            entity_from_row,
        );
        match found {
            Ok(entity) => return Ok(Some(entity)),
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(None)
}

fn insert_entity_aliases(
    conn: &rusqlite::Connection,
    entity_id: &str,
    entity_type: &str,
    aliases: &[(String, String)],
) -> Result<(), CoreError> {
    for (alias, normalized_alias) in aliases {
        conn.execute(
            "INSERT OR IGNORE INTO entity_aliases (entity_id, alias, normalized_alias, entity_type)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![entity_id, alias, normalized_alias, entity_type],
        )?;
    }
    Ok(())
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
        self.upsert_entity_with_aliases(name, &[], entity_type, description, first_doc)
    }

    pub fn upsert_entity_with_aliases(
        &self,
        name: &str,
        aliases: &[String],
        entity_type: &EntityType,
        description: &str,
        first_doc: &str,
    ) -> Result<Entity, CoreError> {
        let conn = self.conn();
        let type_str = entity_type_key(entity_type);
        let now = chrono::Utc::now().to_rfc3339();
        let canonical_name = normalize_entity_display_name(name);
        let alias_pairs = normalized_entity_aliases(&canonical_name, aliases);

        if canonical_name.is_empty() {
            return Err(CoreError::InvalidInput(
                "Entity name cannot be empty".into(),
            ));
        }

        let existing = find_entity_by_aliases(&conn, &alias_pairs, type_str)?;

        match existing {
            Some(mut entity) => {
                conn.execute(
                    "UPDATE entities
                     SET mention_count = mention_count + 1,
                         description = CASE WHEN length(?1) > length(description) THEN ?1 ELSE description END,
                         updated_at = ?2
                     WHERE id = ?3",
                    rusqlite::params![description, now, entity.id],
                )?;
                insert_entity_aliases(&conn, &entity.id, type_str, &alias_pairs)?;
                entity.mention_count += 1;
                if description.len() > entity.description.len() {
                    entity.description = description.to_string();
                }
                Ok(entity)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO entities (id, name, entity_type, description, first_seen_doc, mention_count, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                    rusqlite::params![id, canonical_name, type_str, description, first_doc, now],
                )?;
                insert_entity_aliases(&conn, &id, type_str, &alias_pairs)?;
                Ok(Entity {
                    id,
                    name: canonical_name,
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
        let normalized = normalize_entity_lookup_name(name);
        conn.query_row(
            "SELECT DISTINCT e.id, e.name, e.entity_type, e.description, e.first_seen_doc, e.mention_count, e.created_at
             FROM entities e
             LEFT JOIN entity_aliases ea ON ea.entity_id = e.id
             WHERE ea.normalized_alias = ?1 OR lower(trim(e.name)) = ?1
             ORDER BY e.mention_count DESC, e.name COLLATE NOCASE
             LIMIT 1",
            rusqlite::params![normalized],
            entity_from_row,
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
        self.upsert_entity_link_with_evidence(
            source_id,
            target_id,
            relation_type,
            EntityLinkEvidence {
                strength,
                evidence_doc,
                evidence_snippet: None,
                confidence: None,
            },
        )
    }

    pub fn upsert_entity_link_with_evidence(
        &self,
        source_id: &str,
        target_id: &str,
        relation_type: &str,
        evidence: EntityLinkEvidence<'_>,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let id = uuid::Uuid::new_v4().to_string();
        let normalized_relation_type = normalize_relation_type(relation_type);
        let clamped_strength = evidence.strength.clamp(0.0, 1.0);
        let clamped_confidence = evidence.confidence.map(|value| value.clamp(0.0, 1.0));
        let evidence_snippet = evidence.evidence_snippet.unwrap_or("").trim();
        conn.execute(
            "INSERT INTO entity_links (
                id, source_entity_id, target_entity_id, relation_type, strength,
                evidence_doc_id, evidence_snippet, confidence
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(source_entity_id, target_entity_id, relation_type) DO UPDATE SET
                strength = MIN(1.0, MAX(entity_links.strength, excluded.strength) + 0.1),
                evidence_doc_id = COALESCE(excluded.evidence_doc_id, entity_links.evidence_doc_id),
                evidence_snippet = CASE
                    WHEN length(excluded.evidence_snippet) > length(COALESCE(entity_links.evidence_snippet, ''))
                    THEN excluded.evidence_snippet
                    ELSE entity_links.evidence_snippet
                END,
                confidence = CASE
                    WHEN entity_links.confidence IS NULL THEN excluded.confidence
                    WHEN excluded.confidence IS NULL THEN entity_links.confidence
                    ELSE MAX(entity_links.confidence, excluded.confidence)
                END",
            rusqlite::params![
                id,
                source_id,
                target_id,
                normalized_relation_type,
                clamped_strength,
                evidence.evidence_doc,
                evidence_snippet,
                clamped_confidence
            ],
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

    #[tokio::test]
    async fn compile_document_resolves_aliases_and_stores_relation_evidence() {
        let db = Database::open_memory().expect("open memory");
        let doc_id = insert_compile_doc(&db, "PKCE protects OAuth login.");
        let provider = StaticLlmProvider {
            content: serde_json::json!({
                "summary": "PKCE protects OAuth login.",
                "key_points": ["Proof Key for Code Exchange is used by OAuth."],
                "tags": ["security"],
                "entities": [
                    {
                        "name": "Proof Key for Code Exchange",
                        "aliases": ["PKCE"],
                        "entity_type": "technology",
                        "description": "An OAuth security extension.",
                        "context": "PKCE protects OAuth login.",
                        "relations": [
                            {
                                "target": "OAuth",
                                "relation_type": "protects",
                                "evidence": "PKCE protects OAuth login",
                                "confidence": 0.92
                            }
                        ]
                    },
                    {
                        "name": "OAuth",
                        "entity_type": "technology",
                        "description": "An authorization protocol.",
                        "context": "PKCE protects OAuth login.",
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
        let pkce = db.find_entity_by_name("PKCE").expect("alias lookup");
        assert_eq!(pkce.name, "Proof Key for Code Exchange");
        let (snippet, confidence): (String, f64) = db
            .conn()
            .query_row(
                "SELECT evidence_snippet, confidence FROM entity_links WHERE relation_type = 'protects'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("relation evidence");
        assert_eq!(snippet, "PKCE protects OAuth login");
        assert!((confidence - 0.92).abs() < f64::EPSILON);
    }
}
