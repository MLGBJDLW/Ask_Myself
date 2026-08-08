//! Reviewable event/claim graph and deterministic narrative planning.

use std::collections::HashSet;

use rusqlite::{params, types::Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

const MAX_GRAPH_TEXT_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NarrativeQueryIntent {
    Factual,
    Temporal,
    Causal,
    Comparative,
    DecisionTrace,
    OpenLoop,
}

impl NarrativeQueryIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Factual => "factual",
            Self::Temporal => "temporal",
            Self::Causal => "causal",
            Self::Comparative => "comparative",
            Self::DecisionTrace => "decision_trace",
            Self::OpenLoop => "open_loop",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeClaim {
    pub id: String,
    pub project_id: Option<String>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub claim_status: String,
    pub review_state: String,
    pub confidence: f64,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub provenance: serde_json::Value,
    pub evidence_refs: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl KnowledgeClaim {
    pub fn statement(&self) -> String {
        format!("{} {} {}", self.subject, self.predicate, self.object)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEvent {
    pub id: String,
    pub project_id: Option<String>,
    pub event_kind: String,
    pub title: String,
    pub description: String,
    pub confidence: f64,
    pub review_state: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub provenance: serde_json::Value,
    pub evidence_refs: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKnowledgeClaimInput {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub claim_status: Option<String>,
    pub review_state: Option<String>,
    pub confidence: Option<f64>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub provenance: Option<serde_json::Value>,
    pub source_ref: Option<String>,
    pub source_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKnowledgeEventInput {
    pub event_kind: String,
    pub title: String,
    pub description: Option<String>,
    pub review_state: Option<String>,
    pub confidence: Option<f64>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub provenance: Option<serde_json::Value>,
    pub source_ref: Option<String>,
    pub source_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NarrativeEvidencePlan {
    pub query: String,
    pub intent: NarrativeQueryIntent,
    pub narrative_mode: String,
    pub core_conclusion: String,
    pub event_sequence: Vec<KnowledgeEvent>,
    pub supporting_claims: Vec<KnowledgeClaim>,
    pub opposing_claims: Vec<KnowledgeClaim>,
    pub superseded_claims: Vec<KnowledgeClaim>,
    pub open_questions: Vec<KnowledgeClaim>,
}

fn clamp_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_GRAPH_TEXT_CHARS {
        return trimmed.to_string();
    }
    trimmed.chars().take(MAX_GRAPH_TEXT_CHARS).collect()
}

fn normalize_review_state(value: Option<&str>) -> String {
    match value.unwrap_or("needs_review").trim() {
        "accepted" => "accepted",
        "rejected" => "rejected",
        _ => "needs_review",
    }
    .to_string()
}

fn normalize_claim_status(value: Option<&str>) -> String {
    match value.unwrap_or("active").trim() {
        "contested" => "contested",
        "superseded" => "superseded",
        _ => "active",
    }
    .to_string()
}

fn clamp_confidence(value: Option<f64>) -> f64 {
    value.unwrap_or(0.75).clamp(0.0, 1.0)
}

fn optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect())
}

fn claim_from_row(row: &rusqlite::Row<'_>) -> Result<KnowledgeClaim, rusqlite::Error> {
    let provenance: String = row.get(10)?;
    Ok(KnowledgeClaim {
        id: row.get(0)?,
        project_id: row.get(1)?,
        subject: row.get(2)?,
        predicate: row.get(3)?,
        object: row.get(4)?,
        claim_status: row.get(5)?,
        review_state: row.get(6)?,
        confidence: row.get(7)?,
        valid_from: row.get(8)?,
        valid_to: row.get(9)?,
        provenance: serde_json::from_str(&provenance).unwrap_or_else(|_| serde_json::json!({})),
        evidence_refs: Vec::new(),
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn event_from_row(row: &rusqlite::Row<'_>) -> Result<KnowledgeEvent, rusqlite::Error> {
    let provenance: String = row.get(9)?;
    Ok(KnowledgeEvent {
        id: row.get(0)?,
        project_id: row.get(1)?,
        event_kind: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        confidence: row.get(5)?,
        review_state: row.get(6)?,
        valid_from: row.get(7)?,
        valid_to: row.get(8)?,
        provenance: serde_json::from_str(&provenance).unwrap_or_else(|_| serde_json::json!({})),
        evidence_refs: Vec::new(),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn require_text(value: &str, field: &str) -> Result<String, CoreError> {
    let value = clamp_text(value);
    if value.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "Knowledge {field} must not be empty"
        )));
    }
    Ok(value)
}

impl Database {
    pub fn create_knowledge_claim(
        &self,
        project_id: Option<&str>,
        input: &CreateKnowledgeClaimInput,
    ) -> Result<KnowledgeClaim, CoreError> {
        if let Some(project_id) = project_id {
            let _ = self.get_project(project_id)?;
        }
        let id = Uuid::new_v4().to_string();
        let subject = require_text(&input.subject, "claim subject")?;
        let predicate = require_text(&input.predicate, "claim predicate")?;
        let object = require_text(&input.object, "claim object")?;
        let status = normalize_claim_status(input.claim_status.as_deref());
        let review_state = normalize_review_state(input.review_state.as_deref());
        let provenance_value = input
            .provenance
            .clone()
            .unwrap_or_else(|| serde_json::json!({ "kind": "explicit_input" }));
        let provenance = serde_json::to_string(&provenance_value)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO knowledge_claims
                 (id, project_id, subject, predicate, object, claim_status, review_state,
                  confidence, valid_from, valid_to, provenance_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                project_id,
                subject,
                predicate,
                object,
                status,
                review_state,
                clamp_confidence(input.confidence),
                optional_text(input.valid_from.as_deref()),
                optional_text(input.valid_to.as_deref()),
                provenance
            ],
        )?;
        if let Some(source_ref) = optional_text(input.source_ref.as_deref()) {
            tx.execute(
                "INSERT INTO knowledge_evidence
                     (id, project_id, claim_id, source_type, source_ref, excerpt)
                 VALUES (?1, ?2, ?3, 'explicit_reference', ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    project_id,
                    id,
                    source_ref,
                    clamp_text(input.source_excerpt.as_deref().unwrap_or(""))
                ],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_knowledge_claim(&id)
    }

    pub fn create_knowledge_event(
        &self,
        project_id: Option<&str>,
        input: &CreateKnowledgeEventInput,
    ) -> Result<KnowledgeEvent, CoreError> {
        if let Some(project_id) = project_id {
            let _ = self.get_project(project_id)?;
        }
        let id = Uuid::new_v4().to_string();
        let event_kind = require_text(&input.event_kind, "event kind")?;
        let title = require_text(&input.title, "event title")?;
        let description = clamp_text(input.description.as_deref().unwrap_or(""));
        let review_state = normalize_review_state(input.review_state.as_deref());
        let provenance_value = input
            .provenance
            .clone()
            .unwrap_or_else(|| serde_json::json!({ "kind": "explicit_input" }));
        let provenance = serde_json::to_string(&provenance_value)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO knowledge_events
                 (id, project_id, event_kind, title, description, confidence, review_state,
                  valid_from, valid_to, provenance_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                project_id,
                event_kind,
                title,
                description,
                clamp_confidence(input.confidence),
                review_state,
                optional_text(input.valid_from.as_deref()),
                optional_text(input.valid_to.as_deref()),
                provenance
            ],
        )?;
        if let Some(source_ref) = optional_text(input.source_ref.as_deref()) {
            tx.execute(
                "INSERT INTO knowledge_evidence
                     (id, project_id, event_id, source_type, source_ref, excerpt)
                 VALUES (?1, ?2, ?3, 'explicit_reference', ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    project_id,
                    id,
                    source_ref,
                    clamp_text(input.source_excerpt.as_deref().unwrap_or(""))
                ],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_knowledge_event(&id)
    }

    pub fn get_knowledge_claim(&self, id: &str) -> Result<KnowledgeClaim, CoreError> {
        let conn = self.conn();
        let mut claim = conn
            .query_row(
                "SELECT id, project_id, subject, predicate, object, claim_status, review_state,
                        confidence, valid_from, valid_to, provenance_json, created_at, updated_at
                 FROM knowledge_claims WHERE id = ?1",
                params![id],
                claim_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Knowledge claim {id}"))
                }
                other => CoreError::Database(other),
            })?;
        claim.evidence_refs = evidence_refs_for(&conn, "claim_id", id)?;
        Ok(claim)
    }

    pub fn get_knowledge_event(&self, id: &str) -> Result<KnowledgeEvent, CoreError> {
        let conn = self.conn();
        let mut event = conn
            .query_row(
                "SELECT id, project_id, event_kind, title, description, confidence, review_state,
                        valid_from, valid_to, provenance_json, created_at, updated_at
                 FROM knowledge_events WHERE id = ?1",
                params![id],
                event_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Knowledge event {id}"))
                }
                other => CoreError::Database(other),
            })?;
        event.evidence_refs = evidence_refs_for(&conn, "event_id", id)?;
        Ok(event)
    }

    pub fn review_knowledge_claim(
        &self,
        id: &str,
        review_state: &str,
    ) -> Result<KnowledgeClaim, CoreError> {
        let review_state = normalize_review_state(Some(review_state));
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE knowledge_claims SET review_state = ?1,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?2",
            params![review_state, id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Knowledge claim {id}")));
        }
        drop(conn);
        self.get_knowledge_claim(id)
    }

    pub fn list_project_knowledge_claims(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<KnowledgeClaim>, CoreError> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, project_id, subject, predicate, object, claim_status, review_state,
                    confidence, valid_from, valid_to, provenance_json, created_at, updated_at
             FROM knowledge_claims WHERE project_id = ?1 AND review_state <> 'rejected'
             ORDER BY updated_at DESC, id DESC LIMIT ?2",
        )?;
        let mut claims = statement
            .query_map(
                params![project_id, limit.clamp(1, 200) as i64],
                claim_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for claim in &mut claims {
            claim.evidence_refs = evidence_refs_for(&conn, "claim_id", &claim.id)?;
        }
        Ok(claims)
    }

    pub fn build_project_narrative_plan(
        &self,
        project_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<NarrativeEvidencePlan, CoreError> {
        let intent = classify_narrative_query(query);
        let terms = narrative_query_terms(query);
        let mut claims = query_claims(self, project_id, &terms, limit.clamp(1, 30) * 3)?;
        let mut events = query_events(self, project_id, &terms, limit.clamp(1, 30))?;
        claims.sort_by(|left, right| {
            claim_relevance(right, &terms)
                .cmp(&claim_relevance(left, &terms))
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        claims.truncate(limit.clamp(1, 30));
        events.sort_by(|left, right| {
            left.valid_from
                .cmp(&right.valid_from)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });

        let mut supporting_claims = Vec::new();
        let mut opposing_claims = Vec::new();
        let mut superseded_claims = Vec::new();
        let mut open_questions = Vec::new();
        for claim in claims {
            if claim.review_state == "needs_review" {
                open_questions.push(claim);
            } else if claim.claim_status == "superseded" {
                superseded_claims.push(claim);
            } else if claim.claim_status == "contested" {
                opposing_claims.push(claim);
            } else if claim.review_state == "accepted" {
                supporting_claims.push(claim);
            }
        }
        let core_conclusion = supporting_claims
            .first()
            .map(KnowledgeClaim::statement)
            .unwrap_or_else(|| {
                "No accepted claim directly answers this query; review the evidence and open questions."
                    .to_string()
            });
        Ok(NarrativeEvidencePlan {
            query: query.to_string(),
            intent,
            narrative_mode: narrative_mode(intent).to_string(),
            core_conclusion,
            event_sequence: events,
            supporting_claims,
            opposing_claims,
            superseded_claims,
            open_questions,
        })
    }
}

fn evidence_refs_for(
    conn: &rusqlite::Connection,
    column: &str,
    id: &str,
) -> Result<Vec<String>, CoreError> {
    debug_assert!(matches!(column, "claim_id" | "event_id"));
    let sql = format!(
        "SELECT source_type || ':' || source_ref FROM knowledge_evidence
         WHERE {column} = ?1 ORDER BY created_at ASC"
    );
    let mut statement = conn.prepare(&sql)?;
    let refs = statement
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(refs)
}

fn narrative_query_terms(query: &str) -> Vec<String> {
    let lower = query.trim().to_lowercase();
    let mut terms = HashSet::new();
    if !lower.is_empty() && lower.chars().count() <= 80 {
        terms.insert(lower.clone());
    }
    for term in lower.split(|character: char| !character.is_alphanumeric() && character != '_') {
        if term.chars().count() >= 2 {
            terms.insert(term.to_string());
        }
    }
    terms.into_iter().take(16).collect()
}

fn query_claims(
    db: &Database,
    project_id: &str,
    terms: &[String],
    limit: usize,
) -> Result<Vec<KnowledgeClaim>, CoreError> {
    let conn = db.conn();
    let mut values = vec![Value::Text(project_id.to_string())];
    let predicate = if terms.is_empty() {
        "1 = 1".to_string()
    } else {
        let mut parts = Vec::new();
        for term in terms {
            parts.push(
                "LOWER(subject || ' ' || predicate || ' ' || object) LIKE ? ESCAPE '\\'"
                    .to_string(),
            );
            values.push(Value::Text(format!("%{}%", escape_like(term))));
        }
        format!("({})", parts.join(" OR "))
    };
    values.push(Value::Integer(limit as i64));
    let sql = format!(
        "SELECT id, project_id, subject, predicate, object, claim_status, review_state,
                confidence, valid_from, valid_to, provenance_json, created_at, updated_at
         FROM knowledge_claims WHERE project_id = ? AND review_state <> 'rejected' AND {predicate}
         ORDER BY updated_at DESC LIMIT ?"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut claims = statement
        .query_map(rusqlite::params_from_iter(values.iter()), claim_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    for claim in &mut claims {
        claim.evidence_refs = evidence_refs_for(&conn, "claim_id", &claim.id)?;
    }
    Ok(claims)
}

fn query_events(
    db: &Database,
    project_id: &str,
    terms: &[String],
    limit: usize,
) -> Result<Vec<KnowledgeEvent>, CoreError> {
    let conn = db.conn();
    let mut values = vec![Value::Text(project_id.to_string())];
    let predicate = if terms.is_empty() {
        "1 = 1".to_string()
    } else {
        let mut parts = Vec::new();
        for term in terms {
            parts.push(
                "LOWER(event_kind || ' ' || title || ' ' || description) LIKE ? ESCAPE '\\'"
                    .to_string(),
            );
            values.push(Value::Text(format!("%{}%", escape_like(term))));
        }
        format!("({})", parts.join(" OR "))
    };
    values.push(Value::Integer(limit as i64));
    let sql = format!(
        "SELECT id, project_id, event_kind, title, description, confidence, review_state,
                valid_from, valid_to, provenance_json, created_at, updated_at
         FROM knowledge_events WHERE project_id = ? AND review_state <> 'rejected' AND {predicate}
         ORDER BY COALESCE(valid_from, created_at) ASC LIMIT ?"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut events = statement
        .query_map(rusqlite::params_from_iter(values.iter()), event_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    for event in &mut events {
        event.evidence_refs = evidence_refs_for(&conn, "event_id", &event.id)?;
    }
    Ok(events)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn claim_relevance(claim: &KnowledgeClaim, terms: &[String]) -> usize {
    let statement = claim.statement().to_lowercase();
    terms
        .iter()
        .filter(|term| statement.contains(term.as_str()))
        .count()
}

pub fn classify_narrative_query(query: &str) -> NarrativeQueryIntent {
    let query = query.to_lowercase();
    let classes = [
        (
            NarrativeQueryIntent::DecisionTrace,
            [
                "decision",
                "decide",
                "choose",
                "chose",
                "selected",
                "rationale",
                "决策",
                "决定",
                "为何选择",
            ]
            .as_slice(),
        ),
        (
            NarrativeQueryIntent::OpenLoop,
            [
                "open task",
                "remaining",
                "unresolved",
                "next",
                "待办",
                "未解决",
                "下一步",
            ]
            .as_slice(),
        ),
        (
            NarrativeQueryIntent::Causal,
            ["why", "cause", "because", "导致", "原因", "为什么"].as_slice(),
        ),
        (
            NarrativeQueryIntent::Comparative,
            ["compare", "versus", "difference", "对比", "比较", "区别"].as_slice(),
        ),
        (
            NarrativeQueryIntent::Temporal,
            [
                "when",
                "timeline",
                "before",
                "after",
                "何时",
                "时间线",
                "之前",
                "之后",
            ]
            .as_slice(),
        ),
    ];
    classes
        .into_iter()
        .find_map(|(intent, markers)| {
            markers
                .iter()
                .any(|marker| query.contains(marker))
                .then_some(intent)
        })
        .unwrap_or(NarrativeQueryIntent::Factual)
}

fn narrative_mode(intent: NarrativeQueryIntent) -> &'static str {
    match intent {
        NarrativeQueryIntent::Factual => "claim_first",
        NarrativeQueryIntent::Temporal => "chronological",
        NarrativeQueryIntent::Causal => "cause_to_effect",
        NarrativeQueryIntent::Comparative => "side_by_side",
        NarrativeQueryIntent::DecisionTrace => "evidence_to_decision",
        NarrativeQueryIntent::OpenLoop => "open_items_first",
    }
}

pub fn build_narrative_evidence_section(plan: &NarrativeEvidencePlan) -> String {
    let has_content = !plan.event_sequence.is_empty()
        || !plan.supporting_claims.is_empty()
        || !plan.opposing_claims.is_empty()
        || !plan.superseded_claims.is_empty()
        || !plan.open_questions.is_empty();
    if !has_content {
        return String::new();
    }
    let mut lines = vec![
        "## Event and Claim Narrative Evidence".to_string(),
        String::new(),
        format!(
            "Query intent: {}; narrative mode: {}.",
            plan.intent.as_str(),
            plan.narrative_mode
        ),
        format!("Core conclusion: {}", plan.core_conclusion),
    ];
    append_events(&mut lines, "Event sequence", &plan.event_sequence);
    append_claims(
        &mut lines,
        "Supporting accepted claims",
        &plan.supporting_claims,
    );
    append_claims(
        &mut lines,
        "Opposing or contested claims",
        &plan.opposing_claims,
    );
    append_claims(&mut lines, "Superseded claims", &plan.superseded_claims);
    append_claims(
        &mut lines,
        "Open questions requiring review",
        &plan.open_questions,
    );
    lines.join("\n")
}

fn append_claims(lines: &mut Vec<String>, label: &str, claims: &[KnowledgeClaim]) {
    if claims.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for claim in claims {
        lines.push(format!(
            "- [{} / confidence {:.2}] {} (evidence: {})",
            claim.review_state,
            claim.confidence,
            claim.statement(),
            if claim.evidence_refs.is_empty() {
                "none recorded".to_string()
            } else {
                claim.evidence_refs.join(", ")
            }
        ));
    }
}

fn append_events(lines: &mut Vec<String>, label: &str, events: &[KnowledgeEvent]) {
    if events.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for event in events {
        lines.push(format!(
            "- [{} / {}] {} — {} (evidence: {})",
            event.valid_from.as_deref().unwrap_or("time unknown"),
            event.review_state,
            event.title,
            event.description,
            if event.evidence_refs.is_empty() {
                "none recorded".to_string()
            } else {
                event.evidence_refs.join(", ")
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::CreateProjectInput;

    fn project(db: &Database) -> String {
        db.create_project(&CreateProjectInput {
            name: "Narrative".into(),
            description: None,
            icon: None,
            color: None,
            system_prompt: None,
            source_scope: None,
        })
        .unwrap()
        .id
    }

    fn claim(subject: &str, object: &str, review_state: &str) -> CreateKnowledgeClaimInput {
        CreateKnowledgeClaimInput {
            subject: subject.into(),
            predicate: "uses".into(),
            object: object.into(),
            claim_status: None,
            review_state: Some(review_state.into()),
            confidence: Some(0.9),
            valid_from: None,
            valid_to: None,
            provenance: Some(serde_json::json!({ "kind": "test" })),
            source_ref: Some("document:architecture.md".into()),
            source_excerpt: Some("Primary evidence".into()),
        }
    }

    #[test]
    fn classifies_narrative_queries() {
        assert_eq!(
            classify_narrative_query("Why did we choose SQLite?"),
            NarrativeQueryIntent::DecisionTrace
        );
        assert_eq!(
            classify_narrative_query("What remains unresolved?"),
            NarrativeQueryIntent::OpenLoop
        );
        assert_eq!(
            classify_narrative_query("Show the timeline after launch"),
            NarrativeQueryIntent::Temporal
        );
    }

    #[test]
    fn narrative_separates_accepted_contested_superseded_and_reviewable_claims() {
        let db = Database::open_memory().unwrap();
        let project_id = project(&db);
        let accepted = db
            .create_knowledge_claim(Some(&project_id), &claim("Nexa", "SQLite", "accepted"))
            .unwrap();
        let mut contested = claim("Nexa", "remote graph", "accepted");
        contested.claim_status = Some("contested".into());
        db.create_knowledge_claim(Some(&project_id), &contested)
            .unwrap();
        let mut superseded = claim("Nexa", "legacy graph", "accepted");
        superseded.claim_status = Some("superseded".into());
        db.create_knowledge_claim(Some(&project_id), &superseded)
            .unwrap();
        db.create_knowledge_claim(
            Some(&project_id),
            &claim("Nexa", "experimental graph", "needs_review"),
        )
        .unwrap();

        let plan = db
            .build_project_narrative_plan(&project_id, "Nexa graph SQLite", 10)
            .unwrap();
        assert_eq!(plan.supporting_claims, vec![accepted]);
        assert_eq!(plan.opposing_claims.len(), 1);
        assert_eq!(plan.superseded_claims.len(), 1);
        assert_eq!(plan.open_questions.len(), 1);
        let section = build_narrative_evidence_section(&plan);
        assert!(section.contains("explicit_reference:document:architecture.md"));
        assert!(section.contains("Open questions requiring review"));
    }

    #[test]
    fn review_transition_is_explicit_and_project_scoped() {
        let db = Database::open_memory().unwrap();
        let project_id = project(&db);
        let pending = db
            .create_knowledge_claim(
                Some(&project_id),
                &claim("Claim", "pending", "needs_review"),
            )
            .unwrap();
        assert_eq!(pending.review_state, "needs_review");
        let accepted = db.review_knowledge_claim(&pending.id, "accepted").unwrap();
        assert_eq!(accepted.review_state, "accepted");
        assert_eq!(
            db.list_project_knowledge_claims(&project_id, 20).unwrap(),
            vec![accepted]
        );
    }
}
