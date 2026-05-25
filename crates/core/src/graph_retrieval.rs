//! Graph-guided retrieval planning for knowledge-base search.

use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::models::{FileType, SearchFilters};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEntityHit {
    pub id: String,
    pub label: String,
    pub entity_type: String,
    pub score: f64,
    pub mention_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphDocumentHit {
    pub document_id: String,
    pub source_id: String,
    pub title: String,
    pub path: String,
    pub score: f64,
    pub matched_entities: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphRetrievalPlan {
    pub query_terms: Vec<String>,
    pub query_expansion_terms: Vec<String>,
    pub entities: Vec<GraphEntityHit>,
    pub documents: Vec<GraphDocumentHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphRetrievalReport {
    pub strategy: String,
    pub query: String,
    pub query_expansion_terms: Vec<String>,
    pub entities: Vec<GraphEntityHit>,
    pub candidate_documents: Vec<GraphDocumentHit>,
    pub expanded_chunk_ids: Vec<String>,
    pub boosted_chunk_ids: Vec<String>,
}

impl GraphRetrievalReport {
    pub fn from_plan(query: &str, plan: &GraphRetrievalPlan) -> Self {
        Self {
            strategy: "entity-document-expansion".to_string(),
            query: query.to_string(),
            query_expansion_terms: plan.query_expansion_terms.clone(),
            entities: plan.entities.clone(),
            candidate_documents: plan.documents.clone(),
            expanded_chunk_ids: Vec::new(),
            boosted_chunk_ids: Vec::new(),
        }
    }
}

pub fn merge_reports(
    query: String,
    reports: Vec<GraphRetrievalReport>,
) -> Option<GraphRetrievalReport> {
    if reports.is_empty() {
        return None;
    }

    let mut merged = GraphRetrievalReport {
        strategy: "multi-query entity-document-expansion".to_string(),
        query,
        ..GraphRetrievalReport::default()
    };
    let mut terms = HashSet::new();
    let mut entity_ids = HashSet::new();
    let mut doc_ids = HashSet::new();
    let mut expanded = HashSet::new();
    let mut boosted = HashSet::new();

    for report in reports {
        for term in report.query_expansion_terms {
            if terms.insert(term.to_lowercase()) {
                merged.query_expansion_terms.push(term);
            }
        }
        for entity in report.entities {
            if entity_ids.insert(entity.id.clone()) {
                merged.entities.push(entity);
            }
        }
        for doc in report.candidate_documents {
            if doc_ids.insert(doc.document_id.clone()) {
                merged.candidate_documents.push(doc);
            }
        }
        for chunk_id in report.expanded_chunk_ids {
            if expanded.insert(chunk_id.clone()) {
                merged.expanded_chunk_ids.push(chunk_id);
            }
        }
        for chunk_id in report.boosted_chunk_ids {
            if boosted.insert(chunk_id.clone()) {
                merged.boosted_chunk_ids.push(chunk_id);
            }
        }
    }

    Some(merged)
}

pub fn build_plan(
    db: &Database,
    query_text: &str,
    filters: &SearchFilters,
    document_limit: usize,
) -> Result<Option<GraphRetrievalPlan>, CoreError> {
    let query_terms = graph_query_terms(query_text);
    if query_terms.is_empty() || document_limit == 0 {
        return Ok(None);
    }

    let conn = db.conn();
    let mut where_parts = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut match_parts = Vec::new();

    for term in &query_terms {
        let pattern = format!("%{}%", escape_like(term));
        match_parts.push(
            "(LOWER(e.name) LIKE ? ESCAPE '\\' \
              OR LOWER(e.description) LIKE ? ESCAPE '\\' \
              OR LOWER(de.context_snippet) LIKE ? ESCAPE '\\' \
              OR LOWER(COALESCE(d.title, '')) LIKE ? ESCAPE '\\' \
              OR LOWER(d.path) LIKE ? ESCAPE '\\')"
                .to_string(),
        );
        for _ in 0..5 {
            params.push(Value::Text(pattern.clone()));
        }
    }
    where_parts.push(format!("({})", match_parts.join(" OR ")));

    if !filters.source_ids.is_empty() {
        where_parts.push(format!(
            "d.source_id IN ({})",
            repeat_placeholders(filters.source_ids.len())
        ));
        params.extend(
            filters
                .source_ids
                .iter()
                .map(|value| Value::Text(value.to_string())),
        );
    }

    if !filters.file_types.is_empty() {
        let mimes: Vec<String> = filters
            .file_types
            .iter()
            .flat_map(file_type_to_mimes)
            .collect();
        if !mimes.is_empty() {
            where_parts.push(format!(
                "d.mime_type IN ({})",
                repeat_placeholders(mimes.len())
            ));
            params.extend(mimes.into_iter().map(Value::Text));
        }
    }

    if let Some(ref from) = filters.date_from {
        where_parts.push("d.indexed_at >= ?".to_string());
        params.push(Value::Text(from.to_rfc3339()));
    }
    if let Some(ref to) = filters.date_to {
        where_parts.push("d.indexed_at <= ?".to_string());
        params.push(Value::Text(to.to_rfc3339()));
    }

    params.push(Value::Integer((document_limit * 8).clamp(8, 120) as i64));

    let sql = format!(
        "SELECT e.id, e.name, e.entity_type, e.description, e.mention_count,
                d.id, COALESCE(d.title, d.path), d.path, d.source_id,
                MAX(de.relevance) AS relevance,
                MAX(de.context_snippet) AS context_snippet
         FROM entities e
         JOIN document_entities de ON de.entity_id = e.id
         JOIN documents d ON d.id = de.document_id
         WHERE {}
         GROUP BY e.id, d.id
         ORDER BY relevance DESC, e.mention_count DESC, d.modified_at DESC
         LIMIT ?",
        where_parts.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(GraphRow {
                entity_id: row.get(0)?,
                label: row.get(1)?,
                entity_type: row.get(2)?,
                description: row.get(3)?,
                mention_count: row.get(4)?,
                document_id: row.get(5)?,
                title: row.get(6)?,
                path: row.get(7)?,
                source_id: row.get(8)?,
                relevance: row.get(9)?,
                context_snippet: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if rows.is_empty() {
        return Ok(None);
    }

    let mut entity_scores: HashMap<String, GraphEntityHit> = HashMap::new();
    let mut document_scores: HashMap<String, GraphDocumentAccumulator> = HashMap::new();

    for row in rows {
        let entity_score = score_entity_row(&row, &query_terms);
        entity_scores
            .entry(row.entity_id.clone())
            .and_modify(|hit| hit.score = hit.score.max(entity_score))
            .or_insert_with(|| GraphEntityHit {
                id: row.entity_id.clone(),
                label: row.label.clone(),
                entity_type: row.entity_type.clone(),
                score: entity_score,
                mention_count: row.mention_count,
            });

        let doc_score = score_document_row(&row, &query_terms, entity_score);
        let entry = document_scores
            .entry(row.document_id.clone())
            .or_insert_with(|| GraphDocumentAccumulator {
                document_id: row.document_id.clone(),
                source_id: row.source_id.clone(),
                title: row.title.clone(),
                path: row.path.clone(),
                score: 0.0,
                matched_entities: Vec::new(),
                reasons: Vec::new(),
            });
        entry.score += doc_score;
        push_unique(&mut entry.matched_entities, row.label.clone());
        let reason = strongest_reason(&row, &query_terms);
        if !reason.is_empty() {
            push_unique(&mut entry.reasons, reason);
        }
    }

    let mut entities: Vec<GraphEntityHit> = entity_scores.into_values().collect();
    entities.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entities.truncate(12);
    normalize_entity_scores(&mut entities);

    let mut documents: Vec<GraphDocumentHit> = document_scores
        .into_values()
        .map(GraphDocumentAccumulator::into_hit)
        .collect();
    documents.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    documents.truncate(document_limit);
    normalize_document_scores(&mut documents);

    let mut query_expansion_terms = Vec::new();
    for entity in entities.iter().take(6) {
        if !query_contains_term(query_text, &entity.label) {
            push_unique(&mut query_expansion_terms, entity.label.clone());
        }
    }
    for doc in documents.iter().take(3) {
        if !doc.title.trim().is_empty() && !query_contains_term(query_text, &doc.title) {
            push_unique(&mut query_expansion_terms, doc.title.clone());
        }
    }

    Ok(Some(GraphRetrievalPlan {
        query_terms,
        query_expansion_terms,
        entities,
        documents,
    }))
}

pub fn graph_query_terms(query_text: &str) -> Vec<String> {
    let lower = query_text.trim().to_lowercase();
    let mut terms = Vec::new();
    if !lower.is_empty() && lower.chars().count() <= 40 {
        push_unique(&mut terms, lower.clone());
    }

    for token in lower.split(is_query_separator) {
        let token = token.trim();
        if token.chars().count() >= 2 {
            push_unique(&mut terms, token.to_string());
        }
        if contains_cjk(token) {
            push_cjk_ngrams(token, 3, &mut terms);
            push_cjk_ngrams(token, 2, &mut terms);
        }
        if terms.len() >= 24 {
            break;
        }
    }

    terms.truncate(24);
    terms
}

fn score_entity_row(row: &GraphRow, query_terms: &[String]) -> f64 {
    let label = row.label.to_lowercase();
    let description = row.description.to_lowercase();
    let context = row.context_snippet.as_deref().unwrap_or("").to_lowercase();

    let mut score = 0.0;
    for term in query_terms {
        if label == *term {
            score += 1.3;
        } else if label.contains(term) || term.contains(&label) {
            score += 0.9;
        }
        if description.contains(term) {
            score += 0.45;
        }
        if context.contains(term) {
            score += 0.35;
        }
    }
    score + (row.mention_count as f64).ln_1p() * 0.05 + row.relevance * 0.3
}

fn score_document_row(row: &GraphRow, query_terms: &[String], entity_score: f64) -> f64 {
    let title = row.title.to_lowercase();
    let path = row.path.to_lowercase();
    let context = row.context_snippet.as_deref().unwrap_or("").to_lowercase();
    let mut score = entity_score * row.relevance.max(0.2);
    for term in query_terms {
        if title.contains(term) {
            score += 0.5;
        }
        if path.contains(term) {
            score += 0.2;
        }
        if context.contains(term) {
            score += 0.25;
        }
    }
    score
}

fn strongest_reason(row: &GraphRow, query_terms: &[String]) -> String {
    let label = row.label.to_lowercase();
    if query_terms
        .iter()
        .any(|term| label.contains(term) || term.contains(&label))
    {
        return format!("entity:{}", row.label);
    }
    if let Some(context) = row.context_snippet.as_deref() {
        let context_lc = context.to_lowercase();
        if query_terms.iter().any(|term| context_lc.contains(term)) {
            return format!("context:{}", row.label);
        }
    }
    format!("related-entity:{}", row.label)
}

fn normalize_entity_scores(entities: &mut [GraphEntityHit]) {
    let max = entities
        .iter()
        .map(|entity| entity.score)
        .fold(0.0_f64, f64::max);
    if max <= f64::EPSILON {
        return;
    }
    for entity in entities {
        entity.score = round3(entity.score / max);
    }
}

fn normalize_document_scores(documents: &mut [GraphDocumentHit]) {
    let max = documents
        .iter()
        .map(|document| document.score)
        .fold(0.0_f64, f64::max);
    if max <= f64::EPSILON {
        return;
    }
    for document in documents {
        document.score = round3(document.score / max);
    }
}

fn query_contains_term(query_text: &str, term: &str) -> bool {
    let query = query_text.to_lowercase();
    let term = term.to_lowercase();
    query.contains(term.trim())
}

fn push_cjk_ngrams(token: &str, size: usize, terms: &mut Vec<String>) {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < size {
        return;
    }
    for window in chars.windows(size) {
        let gram = window.iter().collect::<String>();
        push_unique(terms, gram);
        if terms.len() >= 24 {
            return;
        }
    }
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|ch| {
        ('\u{4E00}'..='\u{9FFF}').contains(&ch)
            || ('\u{3400}'..='\u{4DBF}').contains(&ch)
            || ('\u{3040}'..='\u{30FF}').contains(&ch)
            || ('\u{AC00}'..='\u{D7AF}').contains(&ch)
    })
}

fn is_query_separator(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。'
                | '？'
                | '！'
                | '；'
                | '：'
                | '、'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '“'
                | '”'
                | '‘'
                | '’'
        )
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn repeat_placeholders(count: usize) -> String {
    std::iter::repeat("?")
        .take(count)
        .collect::<Vec<_>>()
        .join(",")
}

fn file_type_to_mimes(ft: &FileType) -> Vec<String> {
    match ft {
        FileType::Markdown => vec!["text/markdown".to_string()],
        FileType::PlainText => vec!["text/plain".to_string()],
        FileType::Log => vec!["text/x-log".to_string()],
        FileType::Pdf => vec!["application/pdf".to_string()],
        FileType::Docx => vec![
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ],
        FileType::Excel => {
            vec!["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()]
        }
        FileType::Pptx => vec![
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string(),
        ],
        FileType::Image => vec![
            "image/jpeg".to_string(),
            "image/png".to_string(),
            "image/gif".to_string(),
            "image/webp".to_string(),
        ],
        FileType::Video => vec![
            "video/mp4".to_string(),
            "video/webm".to_string(),
            "video/quicktime".to_string(),
            "video/x-matroska".to_string(),
        ],
        FileType::Audio => vec![
            "audio/mpeg".to_string(),
            "audio/wav".to_string(),
            "audio/flac".to_string(),
            "audio/ogg".to_string(),
            "audio/aac".to_string(),
            "audio/mp4".to_string(),
            "audio/x-ms-wma".to_string(),
            "audio/opus".to_string(),
        ],
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    let normalized = value.trim();
    if normalized.is_empty()
        || values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(normalized))
    {
        return;
    }
    values.push(normalized.to_string());
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[derive(Debug)]
struct GraphRow {
    entity_id: String,
    label: String,
    entity_type: String,
    description: String,
    mention_count: i64,
    document_id: String,
    title: String,
    path: String,
    source_id: String,
    relevance: f64,
    context_snippet: Option<String>,
}

struct GraphDocumentAccumulator {
    document_id: String,
    source_id: String,
    title: String,
    path: String,
    score: f64,
    matched_entities: Vec<String>,
    reasons: Vec<String>,
}

impl GraphDocumentAccumulator {
    fn into_hit(self) -> GraphDocumentHit {
        GraphDocumentHit {
            document_id: self.document_id,
            source_id: self.source_id,
            title: self.title,
            path: self.path,
            score: self.score,
            matched_entities: self.matched_entities,
            reasons: self.reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::EntityType;
    use crate::sources::CreateSourceInput;

    #[test]
    fn graph_query_terms_adds_cjk_ngrams() {
        let terms = graph_query_terms("请找一下指示图如何影响检索");

        assert!(terms.iter().any(|term| term == "指示图"));
        assert!(terms.iter().any(|term| term == "检索"));
    }

    #[test]
    fn build_plan_finds_entity_documents_by_description() {
        let db = Database::open_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .unwrap();
        let doc_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, ?2, ?3, 'Mobile ADR', 'text/markdown', 1, datetime('now'), ?4)",
                rusqlite::params![
                    &doc_id,
                    &source.id,
                    dir.path().join("mobile.md").to_string_lossy(),
                    "hash"
                ],
            )
            .unwrap();
        let entity = db
            .upsert_entity(
                "Mobile Login",
                &EntityType::Concept,
                "authentication decision for mobile clients",
                &doc_id,
            )
            .unwrap();
        db.link_document_entity(&doc_id, &entity.id, 1.0, "PKCE rationale")
            .unwrap();

        let plan = build_plan(&db, "authentication decision", &SearchFilters::default(), 5)
            .unwrap()
            .unwrap();

        assert_eq!(plan.documents[0].document_id, doc_id);
        assert_eq!(plan.entities[0].label, "Mobile Login");
        assert!(plan
            .query_expansion_terms
            .iter()
            .any(|term| term == "Mobile Login"));
    }
}
