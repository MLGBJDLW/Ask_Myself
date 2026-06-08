//! Background knowledge consolidation runs and reviewable artifacts.

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::evolution::{
    CreateAgentProceduralMemoryInput, CreateSkillChangeProposalInput, SkillChangeAction,
};
use crate::knowledge_graph::EntityLink;
use crate::knowledge_loop::KnowledgeGap;
use crate::lint::{CheckType, HealthIssue, Severity};
use crate::personalization::MemorySource;
use crate::project_memory::CreateProjectMemoryInput;

const MAX_HEALTH_ARTIFACTS_PER_RUN: usize = 8;
const MAX_GAP_ARTIFACTS_PER_RUN: usize = 8;
const MAX_PROJECT_MEMORY_ARTIFACTS_PER_RUN: usize = 8;
const MAX_GRAPH_RELATION_ARTIFACTS_PER_RUN: usize = 8;
const MAX_ENTITY_MERGE_ARTIFACTS_PER_RUN: usize = 8;
const MAX_ARTIFACT_TEXT_CHARS: usize = 4_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDreamInput {
    pub trigger_kind: Option<String>,
    pub scope_json: Option<Value>,
    pub max_artifacts: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamRun {
    pub id: String,
    pub trigger_kind: String,
    pub scope_json: Value,
    pub status: String,
    pub phase: Option<String>,
    pub summary: Option<String>,
    pub stats_json: Value,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamRunEvent {
    pub id: String,
    pub run_id: String,
    pub event_type: String,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub payload_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamArtifact {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub status: String,
    pub title: String,
    pub summary: String,
    pub payload_json: Value,
    pub evidence_json: Value,
    pub application_json: Value,
    pub confidence: f32,
    pub review_required: bool,
    pub created_at: String,
    pub applied_at: Option<String>,
    pub rejected_at: Option<String>,
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDreamArtifactInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub payload_json: Option<Value>,
    pub evidence_json: Option<Value>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone)]
struct NewDreamArtifact {
    kind: String,
    title: String,
    summary: String,
    payload_json: Value,
    evidence_json: Value,
    confidence: f32,
    review_required: bool,
}

#[derive(Debug, Clone, Default)]
struct DreamScope {
    source_ids: Vec<String>,
    project_ids: Vec<String>,
}

impl DreamScope {
    fn from_json(value: &Value) -> Self {
        Self {
            source_ids: collect_scope_ids(value, "sourceIds", "sourceId"),
            project_ids: collect_scope_ids(value, "projectIds", "projectId"),
        }
    }

    fn source_limited(&self) -> bool {
        !self.source_ids.is_empty()
    }

    fn project_limited(&self) -> bool {
        !self.project_ids.is_empty()
    }

    fn includes_source(&self, source_id: &str) -> bool {
        !self.source_limited() || self.source_ids.iter().any(|id| id == source_id)
    }

    fn includes_project(&self, project_id: &str) -> bool {
        !self.project_limited() || self.project_ids.iter().any(|id| id == project_id)
    }
}

impl Database {
    pub fn start_dream_run(&self, input: StartDreamInput) -> Result<DreamRun, CoreError> {
        let run_id = Uuid::new_v4().to_string();
        let trigger_kind = normalize_trigger_kind(input.trigger_kind.as_deref())?;
        let scope_json = input.scope_json.unwrap_or_else(|| json!({}));
        let scope = DreamScope::from_json(&scope_json);
        let scope_raw = scope_json.to_string();

        {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO dream_runs
                 (id, trigger_kind, scope_json, status, phase, started_at)
                 VALUES (?1, ?2, ?3, 'running', 'planning', datetime('now'))",
                params![&run_id, &trigger_kind, &scope_raw],
            )?;
        }

        self.record_dream_run_event(
            &run_id,
            "phase",
            Some("running"),
            Some("Planning background consolidation"),
            json!({
                "phase": "planning",
                "triggerKind": trigger_kind,
                "scope": scope_json
            }),
        )?;

        let mut remaining_artifacts = input.max_artifacts.unwrap_or(24).clamp(1, 100);
        let project_memory_artifacts =
            self.create_project_memory_dream_artifacts(&run_id, &scope, &mut remaining_artifacts)?;
        let graph_relation_artifacts =
            self.create_graph_relation_dream_artifacts(&run_id, &scope, &mut remaining_artifacts)?;
        let entity_merge_artifacts =
            self.create_entity_merge_dream_artifacts(&run_id, &scope, &mut remaining_artifacts)?;
        let health_artifacts =
            self.create_health_dream_artifacts(&run_id, &scope, &mut remaining_artifacts)?;
        let gap_artifacts =
            self.create_gap_dream_artifacts(&run_id, &scope, &mut remaining_artifacts)?;
        let artifact_count = project_memory_artifacts
            + graph_relation_artifacts
            + entity_merge_artifacts
            + health_artifacts
            + gap_artifacts;
        let stats_json = json!({
            "artifactsCreated": artifact_count,
            "projectMemoryArtifacts": project_memory_artifacts,
            "graphRelationArtifacts": graph_relation_artifacts,
            "entityMergeArtifacts": entity_merge_artifacts,
            "healthArtifacts": health_artifacts,
            "knowledgeGapArtifacts": gap_artifacts,
            "reviewRequired": artifact_count,
            "mutationsApplied": 0,
            "sourceScopeCount": scope.source_ids.len(),
            "projectScopeCount": scope.project_ids.len()
        });
        let summary = if artifact_count == 0 {
            "No new review items were found.".to_string()
        } else {
            format!("Created {artifact_count} review item(s) for the insights inbox.")
        };

        {
            let conn = self.conn();
            conn.execute(
                "UPDATE dream_runs
                 SET status = 'completed',
                     phase = 'review',
                     summary = ?1,
                     stats_json = ?2,
                     finished_at = datetime('now')
                 WHERE id = ?3",
                params![&summary, stats_json.to_string(), &run_id],
            )?;
        }

        self.record_dream_run_event(
            &run_id,
            "completed",
            Some("completed"),
            Some(&summary),
            stats_json,
        )?;

        self.get_dream_run(&run_id)
    }

    pub fn get_dream_run(&self, run_id: &str) -> Result<DreamRun, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, trigger_kind, scope_json, status, phase, summary,
                    stats_json, error, created_at, started_at, finished_at
             FROM dream_runs
             WHERE id = ?1",
            params![run_id],
            dream_run_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Dream run {run_id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_dream_runs(&self, limit: usize) -> Result<Vec<DreamRun>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, trigger_kind, scope_json, status, phase, summary,
                    stats_json, error, created_at, started_at, finished_at
             FROM dream_runs
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![clamp_limit(limit)], dream_run_from_row)?;
        collect_rows(rows)
    }

    pub fn list_dream_run_events(&self, run_id: &str) -> Result<Vec<DreamRunEvent>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, event_type, status, summary, payload_json, created_at
             FROM dream_run_events
             WHERE run_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![run_id], dream_run_event_from_row)?;
        collect_rows(rows)
    }

    pub fn list_dream_artifacts(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DreamArtifact>, CoreError> {
        let status = normalize_optional_filter(status);
        let kind = normalize_optional_filter(kind);
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, kind, status, title, summary, payload_json,
                    evidence_json, application_json, confidence, review_required, created_at,
                    applied_at, rejected_at, undone_at
             FROM dream_artifacts
             WHERE (?1 IS NULL OR status = ?1)
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![status.as_deref(), kind.as_deref(), clamp_limit(limit)],
            dream_artifact_from_row,
        )?;
        collect_rows(rows)
    }

    pub fn get_dream_artifact(&self, id: &str) -> Result<DreamArtifact, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, run_id, kind, status, title, summary, payload_json,
                    evidence_json, application_json, confidence, review_required, created_at,
                    applied_at, rejected_at, undone_at
             FROM dream_artifacts
             WHERE id = ?1",
            params![id],
            dream_artifact_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Dream artifact {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn apply_dream_artifact(&self, id: &str) -> Result<DreamArtifact, CoreError> {
        let artifact = self.get_dream_artifact(id)?;
        ensure_pending(&artifact)?;
        let application_json = self.apply_dream_artifact_payload(&artifact)?;

        {
            let conn = self.conn();
            conn.execute(
                "UPDATE dream_artifacts
                 SET status = 'applied',
                     applied_at = datetime('now'),
                     application_json = ?2
                 WHERE id = ?1",
                params![id, application_json.to_string()],
            )?;
        }

        self.record_dream_run_event(
            &artifact.run_id,
            "artifact",
            Some("applied"),
            Some(&format!("Applied suggestion: {}", artifact.title)),
            json!({ "artifactId": id, "kind": artifact.kind, "application": application_json }),
        )?;
        self.get_dream_artifact(id)
    }

    pub fn update_dream_artifact(
        &self,
        id: &str,
        input: UpdateDreamArtifactInput,
    ) -> Result<DreamArtifact, CoreError> {
        let artifact = self.get_dream_artifact(id)?;
        ensure_pending(&artifact)?;

        let title = match input.title {
            Some(value) => normalize_artifact_text(&value, "Dream artifact title")?,
            None => artifact.title.clone(),
        };
        let summary = match input.summary {
            Some(value) => normalize_artifact_text(&value, "Dream artifact summary")?,
            None => artifact.summary.clone(),
        };
        let payload_json = input
            .payload_json
            .unwrap_or_else(|| artifact.payload_json.clone());
        let evidence_json = input
            .evidence_json
            .unwrap_or_else(|| artifact.evidence_json.clone());
        let confidence = input
            .confidence
            .unwrap_or(artifact.confidence)
            .clamp(0.0, 1.0);

        {
            let conn = self.conn();
            conn.execute(
                "UPDATE dream_artifacts
                 SET title = ?2,
                     summary = ?3,
                     payload_json = ?4,
                     evidence_json = ?5,
                     confidence = ?6
                 WHERE id = ?1",
                params![
                    id,
                    title,
                    summary,
                    payload_json.to_string(),
                    evidence_json.to_string(),
                    confidence
                ],
            )?;
        }

        self.record_dream_run_event(
            &artifact.run_id,
            "artifact",
            Some("updated"),
            Some(&format!("Edited suggestion: {}", artifact.title)),
            json!({ "artifactId": id, "kind": artifact.kind }),
        )?;
        self.get_dream_artifact(id)
    }

    pub fn undo_dream_artifact(&self, id: &str) -> Result<DreamArtifact, CoreError> {
        let artifact = self.get_dream_artifact(id)?;
        ensure_applied(&artifact)?;
        self.undo_dream_artifact_application(&artifact)?;

        {
            let conn = self.conn();
            conn.execute(
                "UPDATE dream_artifacts
                 SET status = 'undone',
                     undone_at = datetime('now')
                 WHERE id = ?1",
                params![id],
            )?;
        }

        self.record_dream_run_event(
            &artifact.run_id,
            "artifact",
            Some("undone"),
            Some(&format!("Undid suggestion: {}", artifact.title)),
            json!({ "artifactId": id, "kind": artifact.kind, "application": artifact.application_json }),
        )?;
        self.get_dream_artifact(id)
    }

    pub fn reject_dream_artifact(&self, id: &str) -> Result<DreamArtifact, CoreError> {
        let artifact = self.get_dream_artifact(id)?;
        ensure_pending(&artifact)?;

        {
            let conn = self.conn();
            conn.execute(
                "UPDATE dream_artifacts
                 SET status = 'rejected',
                     rejected_at = datetime('now')
                 WHERE id = ?1",
                params![id],
            )?;
        }

        self.record_dream_run_event(
            &artifact.run_id,
            "artifact",
            Some("rejected"),
            Some(&format!("Ignored suggestion: {}", artifact.title)),
            json!({ "artifactId": id, "kind": artifact.kind }),
        )?;
        self.get_dream_artifact(id)
    }

    fn create_health_dream_artifacts(
        &self,
        run_id: &str,
        scope: &DreamScope,
        remaining_budget: &mut usize,
    ) -> Result<usize, CoreError> {
        let cap = (*remaining_budget).min(MAX_HEALTH_ARTIFACTS_PER_RUN);
        if cap == 0 {
            return Ok(0);
        }
        let issues = self.get_unresolved_health_issues()?;
        let mut created = 0usize;
        for issue in issues {
            if created >= cap {
                break;
            }
            if !self.health_issue_matches_scope(&issue, scope)? {
                continue;
            }
            let artifact = health_issue_to_artifact(issue);
            if self.insert_dream_artifact(run_id, artifact)? {
                created += 1;
            }
        }
        *remaining_budget = remaining_budget.saturating_sub(created);
        Ok(created)
    }

    fn create_project_memory_dream_artifacts(
        &self,
        run_id: &str,
        scope: &DreamScope,
        remaining_budget: &mut usize,
    ) -> Result<usize, CoreError> {
        let cap = (*remaining_budget).min(MAX_PROJECT_MEMORY_ARTIFACTS_PER_RUN);
        if cap == 0 {
            return Ok(0);
        }
        let candidates = self.find_project_memory_candidates(scope)?;
        let mut created = 0usize;
        for candidate in candidates {
            if created >= cap {
                break;
            }
            if self.insert_dream_artifact(run_id, candidate)? {
                created += 1;
            }
        }
        *remaining_budget = remaining_budget.saturating_sub(created);
        Ok(created)
    }

    fn create_graph_relation_dream_artifacts(
        &self,
        run_id: &str,
        scope: &DreamScope,
        remaining_budget: &mut usize,
    ) -> Result<usize, CoreError> {
        let cap = (*remaining_budget).min(MAX_GRAPH_RELATION_ARTIFACTS_PER_RUN);
        if cap == 0 {
            return Ok(0);
        }
        let candidates = self.find_graph_relation_candidates(scope)?;
        let mut created = 0usize;
        for candidate in candidates {
            if created >= cap {
                break;
            }
            if self.insert_dream_artifact(run_id, candidate)? {
                created += 1;
            }
        }
        *remaining_budget = remaining_budget.saturating_sub(created);
        Ok(created)
    }

    fn create_entity_merge_dream_artifacts(
        &self,
        run_id: &str,
        scope: &DreamScope,
        remaining_budget: &mut usize,
    ) -> Result<usize, CoreError> {
        let cap = (*remaining_budget).min(MAX_ENTITY_MERGE_ARTIFACTS_PER_RUN);
        if cap == 0 {
            return Ok(0);
        }
        let candidates = self.find_entity_merge_candidates(scope)?;
        let mut created = 0usize;
        for candidate in candidates {
            if created >= cap {
                break;
            }
            if self.insert_dream_artifact(run_id, candidate)? {
                created += 1;
            }
        }
        *remaining_budget = remaining_budget.saturating_sub(created);
        Ok(created)
    }

    fn create_gap_dream_artifacts(
        &self,
        run_id: &str,
        _scope: &DreamScope,
        remaining_budget: &mut usize,
    ) -> Result<usize, CoreError> {
        let cap = (*remaining_budget).min(MAX_GAP_ARTIFACTS_PER_RUN);
        if cap == 0 {
            return Ok(0);
        }
        let gaps = self.get_knowledge_gaps(2)?;
        let mut created = 0usize;
        for gap in gaps {
            if created >= cap {
                break;
            }
            let artifact = knowledge_gap_to_artifact(gap);
            if self.insert_dream_artifact(run_id, artifact)? {
                created += 1;
            }
        }
        *remaining_budget = remaining_budget.saturating_sub(created);
        Ok(created)
    }

    fn find_project_memory_candidates(
        &self,
        scope: &DreamScope,
    ) -> Result<Vec<NewDreamArtifact>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.project_id, c.id, m.id, m.content, m.created_at
             FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE c.project_id IS NOT NULL
               AND m.role = 'user'
               AND TRIM(m.content) <> ''
               AND m.created_at > datetime('now', '-30 days')
             ORDER BY m.created_at DESC
             LIMIT 80",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            let (project_id, conversation_id, message_id, content, created_at) = row?;
            if !scope.includes_project(&project_id) {
                continue;
            }
            let Some((kind, title)) = classify_project_memory_candidate(&content) else {
                continue;
            };
            let memory_content = normalize_memory_candidate_content(&content);
            if memory_content.is_empty() {
                continue;
            }
            candidates.push(NewDreamArtifact {
                kind: "project_memory_candidate".to_string(),
                title: title.clone(),
                summary: format!(
                    "Suggested project memory from an explicit user message: {memory_content}"
                ),
                payload_json: json!({
                    "source": "conversation",
                    "projectId": project_id,
                    "conversationId": conversation_id,
                    "messageId": message_id,
                    "kind": kind,
                    "title": title,
                    "content": memory_content,
                    "conflictStatus": "clear"
                }),
                evidence_json: json!([{
                    "kind": "conversation_excerpt",
                    "conversationId": conversation_id,
                    "messageId": message_id,
                    "excerpt": memory_content,
                    "createdAt": created_at
                }]),
                confidence: 0.72,
                review_required: true,
            });
        }
        Ok(candidates)
    }

    fn find_entity_merge_candidates(
        &self,
        scope: &DreamScope,
    ) -> Result<Vec<NewDreamArtifact>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT
                a.id, a.name, a.entity_type, a.description, a.mention_count,
                b.id, b.name, b.entity_type, b.description, b.mention_count,
                da.source_id, db.source_id
             FROM entities a
             JOIN entities b
               ON a.id < b.id
              AND a.entity_type = b.entity_type
              AND LOWER(TRIM(a.name)) = LOWER(TRIM(b.name))
             LEFT JOIN documents da ON da.id = a.first_seen_doc
             LEFT JOIN documents db ON db.id = b.first_seen_doc
             WHERE TRIM(a.name) <> ''
             ORDER BY (a.mention_count + b.mention_count) DESC
             LIMIT 40",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            let (
                a_id,
                a_name,
                a_type,
                a_description,
                a_mentions,
                b_id,
                b_name,
                b_type,
                b_description,
                b_mentions,
                a_source_id,
                b_source_id,
            ) = row?;
            if scope.source_limited()
                && !a_source_id
                    .as_deref()
                    .is_some_and(|source_id| scope.includes_source(source_id))
                && !b_source_id
                    .as_deref()
                    .is_some_and(|source_id| scope.includes_source(source_id))
            {
                continue;
            }
            let (canonical_id, canonical_name, duplicate_id, duplicate_name) =
                if a_mentions >= b_mentions {
                    (a_id, a_name, b_id, b_name)
                } else {
                    (b_id, b_name, a_id, a_name)
                };
            let source_ids = [a_source_id, b_source_id]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            candidates.push(NewDreamArtifact {
                kind: "entity_merge_candidate".to_string(),
                title: format!("Review duplicate entity: {canonical_name}"),
                summary: format!(
                    "Two {a_type} entities differ only by casing or whitespace and may refer to the same concept."
                ),
                payload_json: json!({
                    "source": "entity_name_match",
                    "canonicalEntityId": canonical_id,
                    "canonicalEntityName": canonical_name,
                    "duplicateEntityId": duplicate_id,
                    "duplicateEntityName": duplicate_name,
                    "entityType": a_type,
                    "relationType": "same_as",
                    "strength": 0.95,
                    "sourceIds": source_ids
                }),
                evidence_json: json!([{
                    "kind": "entity_metadata",
                    "entityType": b_type,
                    "aDescription": truncate_evidence_excerpt(&a_description),
                    "bDescription": truncate_evidence_excerpt(&b_description),
                    "aMentions": a_mentions,
                    "bMentions": b_mentions
                }]),
                confidence: 0.95,
                review_required: true,
            });
        }
        Ok(candidates)
    }

    fn find_graph_relation_candidates(
        &self,
        scope: &DreamScope,
    ) -> Result<Vec<NewDreamArtifact>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT
                de1.document_id,
                COALESCE(NULLIF(d.title, ''), d.path, d.id) AS document_title,
                d.path,
                d.source_id,
                e1.id,
                e1.name,
                e1.entity_type,
                e2.id,
                e2.name,
                e2.entity_type,
                de1.context_snippet,
                de2.context_snippet,
                ((de1.relevance + de2.relevance) / 2.0) AS strength
             FROM document_entities de1
             JOIN document_entities de2
               ON de2.document_id = de1.document_id
              AND de1.entity_id < de2.entity_id
             JOIN entities e1 ON e1.id = de1.entity_id
             JOIN entities e2 ON e2.id = de2.entity_id
             JOIN documents d ON d.id = de1.document_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM entity_links el
                 WHERE (el.source_entity_id = de1.entity_id AND el.target_entity_id = de2.entity_id)
                    OR (el.source_entity_id = de2.entity_id AND el.target_entity_id = de1.entity_id)
             )
               AND TRIM(e1.name) <> ''
               AND TRIM(e2.name) <> ''
             ORDER BY d.modified_at DESC, strength DESC
             LIMIT 40",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, f64>(12)?,
            ))
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            let (
                document_id,
                document_title,
                document_path,
                source_id,
                source_entity_id,
                source_entity_name,
                source_entity_type,
                target_entity_id,
                target_entity_name,
                target_entity_type,
                source_context,
                target_context,
                strength,
            ) = row?;
            if !scope.includes_source(&source_id) {
                continue;
            }
            let strength = strength.clamp(0.35, 0.9);
            candidates.push(NewDreamArtifact {
                kind: "graph_relation_candidate".to_string(),
                title: format!("Connect {source_entity_name} and {target_entity_name}"),
                summary: format!(
                    "{source_entity_name} and {target_entity_name} appear together in {document_title}."
                ),
                payload_json: json!({
                    "source": "document_cooccurrence",
                    "sourceEntityId": source_entity_id,
                    "sourceEntityName": source_entity_name,
                    "sourceEntityType": source_entity_type,
                    "targetEntityId": target_entity_id,
                    "targetEntityName": target_entity_name,
                    "targetEntityType": target_entity_type,
                    "relationType": "related_to",
                    "strength": strength,
                    "evidenceDocId": document_id,
                    "sourceId": source_id
                }),
                evidence_json: json!([{
                    "kind": "document_cooccurrence",
                    "documentId": document_id,
                    "sourceId": source_id,
                    "title": document_title,
                    "path": document_path,
                    "sourceContext": truncate_evidence_excerpt(&source_context),
                    "targetContext": truncate_evidence_excerpt(&target_context)
                }]),
                confidence: strength as f32,
                review_required: true,
            });
        }
        Ok(candidates)
    }

    fn insert_dream_artifact(
        &self,
        run_id: &str,
        artifact: NewDreamArtifact,
    ) -> Result<bool, CoreError> {
        let id = Uuid::new_v4().to_string();
        let review_required: i32 = if artifact.review_required { 1 } else { 0 };
        let payload_raw = artifact.payload_json.to_string();
        let evidence_raw = artifact.evidence_json.to_string();
        let conn = self.conn();
        let duplicate_existing: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM dream_artifacts
                WHERE kind = ?1
                  AND payload_json = ?2
            )",
            params![&artifact.kind, &payload_raw],
            |row| row.get(0),
        )?;
        if duplicate_existing {
            return Ok(false);
        }

        conn.execute(
            "INSERT INTO dream_artifacts
             (id, run_id, kind, status, title, summary, payload_json,
              evidence_json, confidence, review_required)
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &id,
                run_id,
                &artifact.kind,
                &artifact.title,
                &artifact.summary,
                payload_raw,
                evidence_raw,
                artifact.confidence.clamp(0.0, 1.0),
                review_required
            ],
        )?;
        Ok(true)
    }

    fn apply_dream_artifact_payload(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        match artifact.kind.as_str() {
            "project_memory_candidate" => self.apply_project_memory_artifact(artifact),
            "user_memory_candidate" => self.apply_user_memory_artifact(artifact),
            "graph_relation_candidate" => self.apply_graph_relation_artifact(artifact),
            "entity_merge_candidate" => self.apply_entity_merge_artifact(artifact),
            "procedural_memory_candidate" => self.apply_procedural_memory_artifact(artifact),
            "skill_proposal_candidate" => self.apply_skill_proposal_artifact(artifact),
            "health_fix" => Ok(json!({
                "target": "health_check",
                "mode": "acknowledged",
                "healthCheckId": artifact.payload_json.get("healthCheckId").cloned().unwrap_or(Value::Null)
            })),
            "knowledge_gap" => Ok(json!({
                "target": "insights_inbox",
                "mode": "acknowledged",
                "topic": artifact.payload_json.get("topic").cloned().unwrap_or(Value::Null)
            })),
            other => Err(CoreError::InvalidInput(format!(
                "Dream artifact kind '{other}' has no applier"
            ))),
        }
    }

    fn apply_project_memory_artifact(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        let project_id = required_string(&artifact.payload_json, "projectId")?;
        let content = required_string(&artifact.payload_json, "content")?;
        let memory = self.create_project_memory(
            &project_id,
            &CreateProjectMemoryInput {
                kind: optional_string(&artifact.payload_json, "kind"),
                title: optional_string(&artifact.payload_json, "title"),
                content,
                pinned: optional_bool(&artifact.payload_json, "pinned"),
                source: Some("dream".to_string()),
                confidence: Some(artifact.confidence),
                expires_at: optional_string(&artifact.payload_json, "expiresAt"),
                conflict_status: optional_string(&artifact.payload_json, "conflictStatus"),
            },
        )?;
        Ok(json!({
            "target": "project_memory",
            "action": "created",
            "id": memory.id,
            "projectId": memory.project_id
        }))
    }

    fn apply_user_memory_artifact(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        let content = required_string(&artifact.payload_json, "content")?;
        let memory = self.create_user_memory_with_source(&content, MemorySource::Dream)?;
        Ok(json!({
            "target": "user_memory",
            "action": "created",
            "id": memory.id
        }))
    }

    fn apply_graph_relation_artifact(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        let source_entity_id = required_string(&artifact.payload_json, "sourceEntityId")?;
        let target_entity_id = required_string(&artifact.payload_json, "targetEntityId")?;
        let relation_type = required_string(&artifact.payload_json, "relationType")?;
        let evidence_doc_id = optional_string(&artifact.payload_json, "evidenceDocId");
        let strength =
            optional_f64(&artifact.payload_json, "strength").unwrap_or(artifact.confidence as f64);

        let existing =
            self.find_entity_link(&source_entity_id, &target_entity_id, &relation_type)?;
        self.upsert_entity_link(
            &source_entity_id,
            &target_entity_id,
            &relation_type,
            strength.clamp(0.0, 1.0),
            evidence_doc_id.as_deref(),
        )?;
        let applied =
            self.find_entity_link(&source_entity_id, &target_entity_id, &relation_type)?;
        let Some(link) = applied else {
            return Err(CoreError::Internal(
                "Graph relation was not found after applying dream artifact".to_string(),
            ));
        };
        Ok(json!({
            "target": "knowledge_graph",
            "action": if existing.is_some() { "updated" } else { "created" },
            "id": link.id,
            "sourceEntityId": link.source_entity_id,
            "targetEntityId": link.target_entity_id,
            "relationType": link.relation_type,
            "undoable": existing.is_none()
        }))
    }

    fn apply_entity_merge_artifact(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        let canonical_entity_id = required_string(&artifact.payload_json, "canonicalEntityId")?;
        let duplicate_entity_id = required_string(&artifact.payload_json, "duplicateEntityId")?;
        if canonical_entity_id == duplicate_entity_id {
            return Err(CoreError::InvalidInput(
                "Entity merge candidate requires two distinct entities".to_string(),
            ));
        }
        let relation_type = optional_string(&artifact.payload_json, "relationType")
            .unwrap_or_else(|| "same_as".to_string());
        let strength =
            optional_f64(&artifact.payload_json, "strength").unwrap_or(artifact.confidence as f64);
        let existing =
            self.find_entity_link(&canonical_entity_id, &duplicate_entity_id, &relation_type)?;
        self.upsert_entity_link(
            &canonical_entity_id,
            &duplicate_entity_id,
            &relation_type,
            strength.clamp(0.0, 1.0),
            None,
        )?;
        let applied =
            self.find_entity_link(&canonical_entity_id, &duplicate_entity_id, &relation_type)?;
        let Some(link) = applied else {
            return Err(CoreError::Internal(
                "Entity merge marker was not found after applying dream artifact".to_string(),
            ));
        };
        Ok(json!({
            "target": "entity_merge",
            "action": if existing.is_some() { "updated_same_as" } else { "created_same_as" },
            "id": link.id,
            "canonicalEntityId": canonical_entity_id,
            "duplicateEntityId": duplicate_entity_id,
            "relationType": link.relation_type,
            "undoable": existing.is_none()
        }))
    }

    fn apply_procedural_memory_artifact(
        &self,
        artifact: &DreamArtifact,
    ) -> Result<Value, CoreError> {
        let memory = self.create_agent_procedural_memory(&CreateAgentProceduralMemoryInput {
            title: required_string(&artifact.payload_json, "title")?,
            content: required_string(&artifact.payload_json, "content")?,
            tags: optional_string_array(&artifact.payload_json, "tags"),
            source: Some("dream".to_string()),
            confidence: Some(artifact.confidence),
        })?;
        Ok(json!({
            "target": "agent_procedural_memory",
            "action": "created",
            "id": memory.id
        }))
    }

    fn apply_skill_proposal_artifact(&self, artifact: &DreamArtifact) -> Result<Value, CoreError> {
        let action = optional_string(&artifact.payload_json, "action")
            .unwrap_or_else(|| "create".to_string());
        let proposal = self.create_skill_change_proposal(&CreateSkillChangeProposalInput {
            action: SkillChangeAction::try_from(action.as_str())?,
            skill_id: optional_string(&artifact.payload_json, "skillId"),
            name: optional_string(&artifact.payload_json, "name"),
            description: optional_string(&artifact.payload_json, "description").unwrap_or_default(),
            content: required_string(&artifact.payload_json, "content")?,
            resource_bundle: artifact
                .payload_json
                .get("resourceBundle")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|err| {
                    CoreError::InvalidInput(format!("Invalid skill proposal resourceBundle: {err}"))
                })?
                .unwrap_or_default(),
            rationale: optional_string(&artifact.payload_json, "rationale").unwrap_or_default(),
            conversation_id: optional_string(&artifact.payload_json, "conversationId"),
            source: "dream".to_string(),
            confidence: artifact.confidence,
            evidence: artifact.evidence_json.clone(),
        })?;
        Ok(json!({
            "target": "skill_change_proposal",
            "action": "created_pending_proposal",
            "id": proposal.id,
            "undoable": true
        }))
    }

    fn undo_dream_artifact_application(&self, artifact: &DreamArtifact) -> Result<(), CoreError> {
        let target = required_string(&artifact.application_json, "target")?;
        match target.as_str() {
            "project_memory" => {
                self.delete_project_memory(&required_string(&artifact.application_json, "id")?)
            }
            "user_memory" => {
                self.delete_user_memory(&required_string(&artifact.application_json, "id")?)
            }
            "agent_procedural_memory" => self.delete_agent_procedural_memory(&required_string(
                &artifact.application_json,
                "id",
            )?),
            "skill_change_proposal" => {
                self.reject_skill_change_proposal(&required_string(
                    &artifact.application_json,
                    "id",
                )?)?;
                Ok(())
            }
            "knowledge_graph" => {
                let id = required_string(&artifact.application_json, "id")?;
                let undoable = artifact
                    .application_json
                    .get("undoable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !undoable {
                    return Err(CoreError::InvalidInput(
                        "Cannot undo a dream graph relation that updated an existing edge"
                            .to_string(),
                    ));
                }
                self.delete_entity_link(&id)
            }
            "entity_merge" => {
                let id = required_string(&artifact.application_json, "id")?;
                let undoable = artifact
                    .application_json
                    .get("undoable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !undoable {
                    return Err(CoreError::InvalidInput(
                        "Cannot undo a dream entity merge marker that updated an existing same_as edge"
                            .to_string(),
                    ));
                }
                self.delete_entity_link(&id)
            }
            "health_check" | "insights_inbox" => Ok(()),
            other => Err(CoreError::InvalidInput(format!(
                "Dream application target '{other}' cannot be undone"
            ))),
        }
    }

    fn health_issue_matches_scope(
        &self,
        issue: &HealthIssue,
        scope: &DreamScope,
    ) -> Result<bool, CoreError> {
        if !scope.source_limited() {
            return Ok(true);
        }
        if let Some(doc_id) = issue.target_doc_id.as_deref() {
            return self.document_matches_source_scope(doc_id, scope);
        }
        if let Some(entity_id) = issue.target_entity_id.as_deref() {
            return self.entity_matches_source_scope(entity_id, scope);
        }
        Ok(false)
    }

    fn document_matches_source_scope(
        &self,
        document_id: &str,
        scope: &DreamScope,
    ) -> Result<bool, CoreError> {
        if !scope.source_limited() {
            return Ok(true);
        }
        let conn = self.conn();
        let source_id = conn.query_row(
            "SELECT source_id FROM documents WHERE id = ?1",
            params![document_id],
            |row| row.get::<_, String>(0),
        );
        match source_id {
            Ok(source_id) => Ok(scope.includes_source(&source_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(err) => Err(CoreError::Database(err)),
        }
    }

    fn entity_matches_source_scope(
        &self,
        entity_id: &str,
        scope: &DreamScope,
    ) -> Result<bool, CoreError> {
        if !scope.source_limited() {
            return Ok(true);
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT d.source_id
             FROM document_entities de
             JOIN documents d ON d.id = de.document_id
             WHERE de.entity_id = ?1
             UNION
             SELECT d.source_id
             FROM entities e
             JOIN documents d ON d.id = e.first_seen_doc
             WHERE e.id = ?1",
        )?;
        let rows = stmt.query_map(params![entity_id], |row| row.get::<_, String>(0))?;
        for row in rows {
            if scope.includes_source(&row?) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn find_entity_link(
        &self,
        source_entity_id: &str,
        target_entity_id: &str,
        relation_type: &str,
    ) -> Result<Option<EntityLink>, CoreError> {
        let conn = self.conn();
        match conn.query_row(
            "SELECT id, source_entity_id, target_entity_id, relation_type, strength, evidence_doc_id
             FROM entity_links
             WHERE source_entity_id = ?1 AND target_entity_id = ?2 AND relation_type = ?3",
            params![source_entity_id, target_entity_id, relation_type],
            entity_link_from_row,
        ) {
            Ok(link) => Ok(Some(link)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(CoreError::Database(err)),
        }
    }

    fn delete_entity_link(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM entity_links WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Entity link {id}")));
        }
        Ok(())
    }

    fn record_dream_run_event(
        &self,
        run_id: &str,
        event_type: &str,
        status: Option<&str>,
        summary: Option<&str>,
        payload_json: Value,
    ) -> Result<(), CoreError> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO dream_run_events
             (id, run_id, event_type, status, summary, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &id,
                run_id,
                event_type,
                status,
                summary,
                payload_json.to_string()
            ],
        )?;
        Ok(())
    }
}

fn health_issue_to_artifact(issue: HealthIssue) -> NewDreamArtifact {
    let check_type = check_type_string(&issue.check_type);
    let severity = severity_string(&issue.severity);
    let kind = match issue.check_type {
        CheckType::Gap => "knowledge_gap",
        CheckType::Duplicate => "health_fix",
        CheckType::Contradiction => "project_memory_candidate",
        CheckType::Stale | CheckType::Orphan => "health_fix",
    }
    .to_string();

    NewDreamArtifact {
        kind,
        title: health_artifact_title(&issue),
        summary: format!("{} {}", issue.description, issue.suggestion),
        payload_json: json!({
            "source": "health_check",
            "healthCheckId": issue.id,
            "checkType": check_type,
            "severity": severity,
            "targetDocId": issue.target_doc_id,
            "targetEntityId": issue.target_entity_id,
            "description": issue.description,
            "suggestion": issue.suggestion
        }),
        evidence_json: json!([{
            "kind": "health_check",
            "healthCheckId": issue.id,
            "targetDocId": issue.target_doc_id,
            "targetEntityId": issue.target_entity_id
        }]),
        confidence: health_confidence(&issue.severity),
        review_required: true,
    }
}

fn knowledge_gap_to_artifact(gap: KnowledgeGap) -> NewDreamArtifact {
    let confidence = if gap.avg_confidence < 1.0 {
        0.82
    } else if gap.avg_confidence < 2.0 {
        0.72
    } else {
        0.62
    };
    NewDreamArtifact {
        kind: "knowledge_gap".to_string(),
        title: format!("Add coverage for {}", gap.topic),
        summary: gap.suggestion,
        payload_json: json!({
            "source": "query_logs",
            "topic": gap.topic,
            "queryCount": gap.query_count,
            "avgResultCount": gap.avg_confidence
        }),
        evidence_json: json!([{
            "kind": "query_pattern",
            "topic": gap.topic,
            "queryCount": gap.query_count
        }]),
        confidence,
        review_required: true,
    }
}

fn classify_project_memory_candidate(content: &str) -> Option<(&'static str, String)> {
    let normalized = content.trim();
    let lower = normalized.to_ascii_lowercase();
    if contains_any(
        normalized,
        &[
            "决定",
            "决策",
            "以后都",
            "以后要",
            "记住",
            "we decided",
            "decision:",
            "decide that",
            "from now on",
        ],
    ) || lower.contains("we decided")
    {
        return Some(("decision", "Dreaming suggestion: decision".to_string()));
    }
    if contains_any(
        normalized,
        &[
            "约束",
            "必须",
            "不要",
            "不能",
            "constraint:",
            "must ",
            "must not",
            "never ",
            "do not ",
        ],
    ) {
        return Some(("constraint", "Dreaming suggestion: constraint".to_string()));
    }
    if contains_any(
        normalized,
        &[
            "待办",
            "TODO",
            "todo",
            "需要做",
            "下一步",
            "follow up",
            "next step",
        ],
    ) {
        return Some(("todo", "Dreaming suggestion: todo".to_string()));
    }
    if contains_any(
        normalized,
        &[
            "风格",
            "语气",
            "偏好",
            "style:",
            "tone:",
            "prefer",
            "preference",
        ],
    ) {
        return Some(("style", "Dreaming suggestion: style".to_string()));
    }
    None
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| lower.contains(&needle.to_ascii_lowercase()))
}

fn normalize_memory_candidate_content(content: &str) -> String {
    let mut normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    const PREFIXES: &[&str] = &[
        "请记住",
        "记住",
        "decision:",
        "constraint:",
        "todo:",
        "style:",
        "tone:",
    ];
    for prefix in PREFIXES {
        if normalized
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            normalized = normalized.chars().skip(prefix.chars().count()).collect();
            normalized = normalized
                .trim_start_matches([':', '：', ' ', '-'])
                .to_string();
            break;
        }
    }
    if normalized.chars().count() > 600 {
        normalized.chars().take(600).collect()
    } else {
        normalized
    }
}

fn normalize_artifact_text(value: &str, field: &str) -> Result<String, CoreError> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    if normalized.chars().count() > MAX_ARTIFACT_TEXT_CHARS {
        Ok(normalized.chars().take(MAX_ARTIFACT_TEXT_CHARS).collect())
    } else {
        Ok(normalized)
    }
}

fn truncate_evidence_excerpt(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > 360 {
        normalized.chars().take(360).collect()
    } else {
        normalized
    }
}

fn normalize_trigger_kind(trigger_kind: Option<&str>) -> Result<String, CoreError> {
    let value = trigger_kind.unwrap_or("manual").trim().to_ascii_lowercase();
    match value.as_str() {
        "manual" | "idle" | "after_scan" | "after_turn" | "schedule" => Ok(value),
        "" => Ok("manual".to_string()),
        other => Err(CoreError::InvalidInput(format!(
            "Unsupported dream trigger kind: {other}"
        ))),
    }
}

fn normalize_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn ensure_pending(artifact: &DreamArtifact) -> Result<(), CoreError> {
    if artifact.status == "pending" {
        return Ok(());
    }
    Err(CoreError::InvalidInput(format!(
        "Dream artifact {} is already {}",
        artifact.id, artifact.status
    )))
}

fn ensure_applied(artifact: &DreamArtifact) -> Result<(), CoreError> {
    if artifact.status == "applied" {
        return Ok(());
    }
    Err(CoreError::InvalidInput(format!(
        "Dream artifact {} is not applied; current status is {}",
        artifact.id, artifact.status
    )))
}

fn required_string(payload: &Value, key: &str) -> Result<String, CoreError> {
    optional_string(payload, key)
        .ok_or_else(|| CoreError::InvalidInput(format!("Dream artifact payload missing '{key}'")))
}

fn optional_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn optional_bool(payload: &Value, key: &str) -> Option<bool> {
    payload.get(key).and_then(Value::as_bool)
}

fn optional_f64(payload: &Value, key: &str) -> Option<f64> {
    payload.get(key).and_then(Value::as_f64)
}

fn optional_string_array(payload: &Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn health_artifact_title(issue: &HealthIssue) -> String {
    match issue.check_type {
        CheckType::Stale => "Review stale document".to_string(),
        CheckType::Orphan => "Compile unanalyzed document".to_string(),
        CheckType::Gap => "Improve thin knowledge coverage".to_string(),
        CheckType::Duplicate => "Review possible duplicate entity".to_string(),
        CheckType::Contradiction => "Review possible knowledge conflict".to_string(),
    }
}

fn health_confidence(severity: &Severity) -> f32 {
    match severity {
        Severity::Critical => 0.9,
        Severity::Warning => 0.78,
        Severity::Info => 0.66,
    }
}

fn check_type_string(check_type: &CheckType) -> &'static str {
    match check_type {
        CheckType::Stale => "stale",
        CheckType::Orphan => "orphan",
        CheckType::Gap => "gap",
        CheckType::Duplicate => "duplicate",
        CheckType::Contradiction => "contradiction",
    }
}

fn severity_string(severity: &Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

fn clamp_limit(limit: usize) -> i64 {
    limit.clamp(1, 200) as i64
}

fn dream_run_from_row(row: &rusqlite::Row<'_>) -> Result<DreamRun, rusqlite::Error> {
    let scope_raw: String = row.get(2)?;
    let stats_raw: String = row.get(6)?;
    Ok(DreamRun {
        id: row.get(0)?,
        trigger_kind: row.get(1)?,
        scope_json: parse_json_or(&scope_raw, json!({})),
        status: row.get(3)?,
        phase: row.get(4)?,
        summary: row.get(5)?,
        stats_json: parse_json_or(&stats_raw, json!({})),
        error: row.get(7)?,
        created_at: row.get(8)?,
        started_at: row.get(9)?,
        finished_at: row.get(10)?,
    })
}

fn dream_run_event_from_row(row: &rusqlite::Row<'_>) -> Result<DreamRunEvent, rusqlite::Error> {
    let payload_raw: String = row.get(5)?;
    Ok(DreamRunEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        event_type: row.get(2)?,
        status: row.get(3)?,
        summary: row.get(4)?,
        payload_json: parse_json_or(&payload_raw, json!({})),
        created_at: row.get(6)?,
    })
}

fn dream_artifact_from_row(row: &rusqlite::Row<'_>) -> Result<DreamArtifact, rusqlite::Error> {
    let payload_raw: String = row.get(6)?;
    let evidence_raw: String = row.get(7)?;
    let application_raw: String = row.get(8)?;
    Ok(DreamArtifact {
        id: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        status: row.get(3)?,
        title: row.get(4)?,
        summary: row.get(5)?,
        payload_json: parse_json_or(&payload_raw, json!({})),
        evidence_json: parse_json_or(&evidence_raw, json!([])),
        application_json: parse_json_or(&application_raw, json!({})),
        confidence: row.get(9)?,
        review_required: row.get::<_, i32>(10)? != 0,
        created_at: row.get(11)?,
        applied_at: row.get(12)?,
        rejected_at: row.get(13)?,
        undone_at: row.get(14)?,
    })
}

fn entity_link_from_row(row: &rusqlite::Row<'_>) -> Result<EntityLink, rusqlite::Error> {
    Ok(EntityLink {
        id: row.get(0)?,
        source_entity_id: row.get(1)?,
        target_entity_id: row.get(2)?,
        relation_type: row.get(3)?,
        strength: row.get(4)?,
        evidence_doc_id: row.get(5)?,
    })
}

fn parse_json_or(raw: &str, fallback: Value) -> Value {
    serde_json::from_str(raw).unwrap_or(fallback)
}

fn collect_scope_ids(value: &Value, array_key: &str, single_key: &str) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(single) = value.get(single_key).and_then(Value::as_str) {
        let trimmed = single.trim();
        if !trimmed.is_empty() {
            ids.push(trimmed.to_string());
        }
    }
    if let Some(items) = value.get(array_key).and_then(Value::as_array) {
        for item in items {
            let Some(raw) = item.as_str() else {
                continue;
            };
            let trimmed = raw.trim();
            if trimmed.is_empty() || ids.iter().any(|id| id == trimmed) {
                continue;
            }
            ids.push(trimmed.to_string());
        }
    }
    ids
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>, CoreError>
where
    F: FnMut(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::EntityType;
    use crate::evolution::SkillProposalStatus;
    use crate::project::CreateProjectInput;

    #[test]
    fn dream_run_creates_reviewable_query_gap_artifact() {
        let db = Database::open_memory().expect("open memory db");
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO query_logs (id, query_text, result_count)
                 VALUES ('q1', 'memory provenance', 0)",
                [],
            )
            .expect("insert query 1");
            conn.execute(
                "INSERT INTO query_logs (id, query_text, result_count)
                 VALUES ('q2', 'memory provenance', 1)",
                [],
            )
            .expect("insert query 2");
        }

        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        assert_eq!(run.status, "completed");
        assert_eq!(run.stats_json["knowledgeGapArtifacts"], 1);

        let artifacts = db
            .list_dream_artifacts(Some("pending"), Some("knowledge_gap"), 10)
            .expect("list artifacts");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].status, "pending");
        assert_eq!(artifacts[0].payload_json["source"], "query_logs");

        let second = db
            .start_dream_run(StartDreamInput::default())
            .expect("start second dream run");
        assert_eq!(second.stats_json["artifactsCreated"], 0);
    }

    #[test]
    fn dream_run_plans_project_memory_candidates_from_explicit_project_messages() {
        let db = Database::open_memory().expect("open memory db");
        let project = db
            .create_project(&CreateProjectInput {
                name: "Product".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .expect("create project");
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO conversations (id, provider, model, project_id)
                 VALUES ('conv-1', 'openai', 'gpt-test', ?1)",
                rusqlite::params![project.id],
            )
            .expect("insert conversation");
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES ('msg-1', 'conv-1', 'user', 'Decision: always explain tradeoffs before implementation.', 1)",
                [],
            )
            .expect("insert message");
        }

        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        assert_eq!(run.stats_json["projectMemoryArtifacts"], 1);

        let artifacts = db
            .list_dream_artifacts(Some("pending"), Some("project_memory_candidate"), 10)
            .expect("list artifacts");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].payload_json["kind"], "decision");
        assert_eq!(artifacts[0].evidence_json[0]["messageId"], "msg-1");
    }

    #[test]
    fn dream_run_respects_project_scope_for_project_memory_candidates() {
        let db = Database::open_memory().expect("open memory db");
        let project_a = db
            .create_project(&CreateProjectInput {
                name: "Project A".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .expect("create project a");
        let project_b = db
            .create_project(&CreateProjectInput {
                name: "Project B".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .expect("create project b");
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO conversations (id, provider, model, project_id)
                 VALUES ('conv-a', 'openai', 'gpt-test', ?1)",
                rusqlite::params![project_a.id],
            )
            .expect("insert conversation a");
            conn.execute(
                "INSERT INTO conversations (id, provider, model, project_id)
                 VALUES ('conv-b', 'openai', 'gpt-test', ?1)",
                rusqlite::params![project_b.id],
            )
            .expect("insert conversation b");
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES ('msg-a', 'conv-a', 'user', 'Decision: Project A uses compact summaries.', 1)",
                [],
            )
            .expect("insert message a");
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES ('msg-b', 'conv-b', 'user', 'Decision: Project B uses detailed evidence notes.', 1)",
                [],
            )
            .expect("insert message b");
        }

        let run = db
            .start_dream_run(StartDreamInput {
                trigger_kind: None,
                scope_json: Some(json!({ "projectIds": [project_b.id.clone()] })),
                max_artifacts: None,
            })
            .expect("start scoped dream run");
        assert_eq!(run.stats_json["projectMemoryArtifacts"], 1);
        assert_eq!(run.stats_json["projectScopeCount"], 1);

        let artifacts = db
            .list_dream_artifacts(Some("pending"), Some("project_memory_candidate"), 10)
            .expect("list project candidates");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].payload_json["projectId"], project_b.id);
        assert_eq!(artifacts[0].evidence_json[0]["messageId"], "msg-b");
    }

    #[test]
    fn dream_run_does_not_recreate_applied_project_memory_candidate() {
        let db = Database::open_memory().expect("open memory db");
        let project = db
            .create_project(&CreateProjectInput {
                name: "Product".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .expect("create project");
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO conversations (id, provider, model, project_id)
                 VALUES ('conv-1', 'openai', 'gpt-test', ?1)",
                rusqlite::params![project.id],
            )
            .expect("insert conversation");
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES ('msg-1', 'conv-1', 'user', 'Constraint: keep all generated reports in Markdown.', 1)",
                [],
            )
            .expect("insert message");
        }

        db.start_dream_run(StartDreamInput::default())
            .expect("start first dream run");
        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("project_memory_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        db.apply_dream_artifact(&artifact.id)
            .expect("apply project memory");

        let second = db
            .start_dream_run(StartDreamInput::default())
            .expect("start second dream run");
        assert_eq!(second.stats_json["projectMemoryArtifacts"], 0);
        assert_eq!(
            db.list_dream_artifacts(None, Some("project_memory_candidate"), 10)
                .expect("list all project memory artifacts")
                .len(),
            1
        );
    }

    #[test]
    fn dream_artifact_review_flow_is_explicit() {
        let db = Database::open_memory().expect("open memory db");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "health_fix".to_string(),
                title: "Review orphan document".to_string(),
                summary: "Document needs compilation.".to_string(),
                payload_json: json!({ "source": "test" }),
                evidence_json: json!([]),
                confidence: 0.7,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), None, 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply artifact");
        assert_eq!(applied.status, "applied");

        let err = db
            .reject_dream_artifact(&artifact.id)
            .expect_err("already-applied artifact cannot be rejected");
        assert!(err.to_string().contains("already applied"));
    }

    #[test]
    fn pending_dream_artifact_can_be_edited_before_apply() {
        let db = Database::open_memory().expect("open memory db");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "user_memory_candidate".to_string(),
                title: "Remember preference".to_string(),
                summary: "User prefers short answers.".to_string(),
                payload_json: json!({ "content": "Prefers short answers." }),
                evidence_json: json!([{"kind": "conversation_excerpt", "excerpt": "short please"}]),
                confidence: 0.7,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("user_memory_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let edited = db
            .update_dream_artifact(
                &artifact.id,
                UpdateDreamArtifactInput {
                    title: Some("Remember concise Chinese answers".to_string()),
                    summary: Some("User prefers concise answers in Chinese.".to_string()),
                    payload_json: Some(json!({ "content": "Prefers concise answers in Chinese." })),
                    evidence_json: None,
                    confidence: Some(0.92),
                },
            )
            .expect("edit pending artifact");
        assert_eq!(edited.title, "Remember concise Chinese answers");
        assert_eq!(
            edited.payload_json["content"],
            "Prefers concise answers in Chinese."
        );
        assert_eq!(edited.confidence, 0.92);

        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply edited artifact");
        assert_eq!(applied.application_json["target"], "user_memory");
        let memories = db.list_user_memories().expect("list memories");
        assert_eq!(memories[0].content, "Prefers concise answers in Chinese.");
    }

    #[test]
    fn dream_run_generates_graph_relation_candidates_from_document_cooccurrence() {
        let db = Database::open_memory().expect("open memory db");
        let (doc_id, entity_a, entity_b) = {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO sources (id, root_path) VALUES ('source-1', 'C:/knowledge')",
                [],
            )
            .expect("insert source");
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES ('doc-1', 'source-1', 'C:/knowledge/a.md', 'A', 'text/markdown', 10, datetime('now'), 'hash-1')",
                [],
            )
            .expect("insert document");
            drop(conn);
            let a = db
                .upsert_entity("Aster", &EntityType::Concept, "A concept", "doc-1")
                .expect("entity a");
            let b = db
                .upsert_entity("Beacon", &EntityType::Concept, "Another concept", "doc-1")
                .expect("entity b");
            db.link_document_entity("doc-1", &a.id, 0.9, "Aster describes the routing rule.")
                .expect("link a");
            db.link_document_entity(
                "doc-1",
                &b.id,
                0.8,
                "Beacon shares the same routing context.",
            )
            .expect("link b");
            ("doc-1".to_string(), a, b)
        };

        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        assert_eq!(run.stats_json["graphRelationArtifacts"], 1);
        let artifacts = db
            .list_dream_artifacts(Some("pending"), Some("graph_relation_candidate"), 10)
            .expect("list graph candidates");
        assert_eq!(artifacts.len(), 1);
        let source_entity_id = artifacts[0].payload_json["sourceEntityId"]
            .as_str()
            .expect("source entity id");
        let target_entity_id = artifacts[0].payload_json["targetEntityId"]
            .as_str()
            .expect("target entity id");
        assert_ne!(source_entity_id, target_entity_id);
        assert!([source_entity_id, target_entity_id].contains(&entity_a.id.as_str()));
        assert!([source_entity_id, target_entity_id].contains(&entity_b.id.as_str()));
        assert_eq!(artifacts[0].payload_json["evidenceDocId"], doc_id);
        assert_eq!(artifacts[0].payload_json["relationType"], "related_to");
    }

    #[test]
    fn dream_run_respects_source_scope_for_graph_relation_candidates() {
        let db = Database::open_memory().expect("open memory db");
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO sources (id, root_path) VALUES ('source-1', 'C:/knowledge/a')",
                [],
            )
            .expect("insert source 1");
            conn.execute(
                "INSERT INTO sources (id, root_path) VALUES ('source-2', 'C:/knowledge/b')",
                [],
            )
            .expect("insert source 2");
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES ('doc-1', 'source-1', 'C:/knowledge/a/a.md', 'A', 'text/markdown', 10, datetime('now'), 'hash-1')",
                [],
            )
            .expect("insert doc 1");
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES ('doc-2', 'source-2', 'C:/knowledge/b/b.md', 'B', 'text/markdown', 10, datetime('now'), 'hash-2')",
                [],
            )
            .expect("insert doc 2");
            drop(conn);

            let a1 = db
                .upsert_entity("Atlas", &EntityType::Concept, "Source one concept", "doc-1")
                .expect("source one entity a");
            let b1 = db
                .upsert_entity(
                    "Bridge",
                    &EntityType::Concept,
                    "Source one related concept",
                    "doc-1",
                )
                .expect("source one entity b");
            db.link_document_entity("doc-1", &a1.id, 0.9, "Atlas appears with Bridge.")
                .expect("link source one a");
            db.link_document_entity("doc-1", &b1.id, 0.9, "Bridge appears with Atlas.")
                .expect("link source one b");

            let a2 = db
                .upsert_entity(
                    "Cipher",
                    &EntityType::Concept,
                    "Source two concept",
                    "doc-2",
                )
                .expect("source two entity a");
            let b2 = db
                .upsert_entity(
                    "Delta",
                    &EntityType::Concept,
                    "Source two related concept",
                    "doc-2",
                )
                .expect("source two entity b");
            db.link_document_entity("doc-2", &a2.id, 0.9, "Cipher appears with Delta.")
                .expect("link source two a");
            db.link_document_entity("doc-2", &b2.id, 0.9, "Delta appears with Cipher.")
                .expect("link source two b");
        }

        let run = db
            .start_dream_run(StartDreamInput {
                trigger_kind: None,
                scope_json: Some(json!({ "sourceIds": ["source-2"] })),
                max_artifacts: None,
            })
            .expect("start scoped dream run");
        assert_eq!(run.stats_json["graphRelationArtifacts"], 1);
        assert_eq!(run.stats_json["sourceScopeCount"], 1);

        let artifacts = db
            .list_dream_artifacts(Some("pending"), Some("graph_relation_candidate"), 10)
            .expect("list graph candidates");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].payload_json["sourceId"], "source-2");
        assert_eq!(artifacts[0].payload_json["evidenceDocId"], "doc-2");
    }

    #[test]
    fn entity_merge_candidate_applies_and_undoes_same_as_marker() {
        let db = Database::open_memory().expect("open memory db");
        let (canonical, duplicate) = {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO sources (id, root_path) VALUES ('source-1', 'C:/knowledge')",
                [],
            )
            .expect("insert source");
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES ('doc-1', 'source-1', 'C:/knowledge/a.md', 'A', 'text/markdown', 10, datetime('now'), 'hash-1')",
                [],
            )
            .expect("insert document");
            drop(conn);

            let canonical = db
                .upsert_entity("Aster", &EntityType::Concept, "Primary concept", "doc-1")
                .expect("canonical entity");
            db.upsert_entity(
                "Aster",
                &EntityType::Concept,
                "Primary concept with another mention",
                "doc-1",
            )
            .expect("increment canonical mention count");
            let duplicate = db
                .upsert_entity(
                    "aster",
                    &EntityType::Concept,
                    "Lowercase duplicate",
                    "doc-1",
                )
                .expect("duplicate entity");
            (canonical, duplicate)
        };

        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        assert_eq!(run.stats_json["entityMergeArtifacts"], 1);

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("entity_merge_candidate"), 10)
            .expect("list entity merge candidates")
            .pop()
            .expect("entity merge candidate");
        assert_eq!(artifact.payload_json["canonicalEntityId"], canonical.id);
        assert_eq!(artifact.payload_json["duplicateEntityId"], duplicate.id);
        assert_eq!(artifact.payload_json["relationType"], "same_as");

        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply entity merge marker");
        assert_eq!(applied.application_json["target"], "entity_merge");
        assert_eq!(applied.application_json["undoable"], true);
        assert!(db
            .find_entity_link(&canonical.id, &duplicate.id, "same_as")
            .expect("find applied same_as link")
            .is_some());

        db.undo_dream_artifact(&artifact.id)
            .expect("undo entity merge marker");
        assert!(db
            .find_entity_link(&canonical.id, &duplicate.id, "same_as")
            .expect("find link after undo")
            .is_none());
    }

    #[test]
    fn project_memory_candidate_applies_and_undoes_through_review_layer() {
        let db = Database::open_memory().expect("open memory db");
        let project = db
            .create_project(&CreateProjectInput {
                name: "Novel".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .expect("create project");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "project_memory_candidate".to_string(),
                title: "Remember decision".to_string(),
                summary: "Project uses first person narration.".to_string(),
                payload_json: json!({
                    "projectId": project.id.clone(),
                    "kind": "decision",
                    "title": "Narration",
                    "content": "Use first person narration for draft scenes.",
                    "conflictStatus": "clear"
                }),
                evidence_json: json!([{"kind": "conversation_excerpt", "excerpt": "We decided first person."}]),
                confidence: 0.84,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("project_memory_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply project memory");
        assert_eq!(applied.status, "applied");
        assert_eq!(applied.application_json["target"], "project_memory");

        let memories = db
            .list_project_memories(&project.id)
            .expect("list memories");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].source, "dream");
        assert_eq!(memories[0].kind, "decision");

        let undone = db
            .undo_dream_artifact(&artifact.id)
            .expect("undo project memory");
        assert_eq!(undone.status, "undone");
        assert!(db
            .list_project_memories(&project.id)
            .expect("list after undo")
            .is_empty());
    }

    #[test]
    fn user_memory_candidate_applies_with_dream_source() {
        let db = Database::open_memory().expect("open memory db");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "user_memory_candidate".to_string(),
                title: "Remember answer preference".to_string(),
                summary: "User prefers concise answers.".to_string(),
                payload_json: json!({
                    "content": "Prefers concise answers in Chinese."
                }),
                evidence_json: json!([{"kind": "conversation_excerpt", "excerpt": "请简洁回答。"}]),
                confidence: 0.9,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("user_memory_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply user memory");
        assert_eq!(applied.application_json["target"], "user_memory");

        let memories = db.list_user_memories().expect("list user memories");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].source, MemorySource::Dream);
    }

    #[test]
    fn graph_relation_candidate_applies_and_undoes_new_edge() {
        let db = Database::open_memory().expect("open memory db");
        let (doc_id, entity_a, entity_b) = {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO sources (id, root_path) VALUES ('source-1', 'C:/knowledge')",
                [],
            )
            .expect("insert source");
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES ('doc-1', 'source-1', 'C:/knowledge/a.md', 'A', 'text/markdown', 10, datetime('now'), 'hash-1')",
                [],
            )
            .expect("insert document");
            drop(conn);
            let a = db
                .upsert_entity("Aster", &EntityType::Concept, "A concept", "doc-1")
                .expect("entity a");
            let b = db
                .upsert_entity("Beacon", &EntityType::Concept, "Another concept", "doc-1")
                .expect("entity b");
            ("doc-1".to_string(), a, b)
        };

        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "graph_relation_candidate".to_string(),
                title: "Connect concepts".to_string(),
                summary: "Aster supports Beacon in the source document.".to_string(),
                payload_json: json!({
                    "sourceEntityId": entity_a.id.clone(),
                    "targetEntityId": entity_b.id.clone(),
                    "relationType": "supports",
                    "strength": 0.8,
                    "evidenceDocId": doc_id
                }),
                evidence_json: json!([{"kind": "document", "documentId": "doc-1"}]),
                confidence: 0.8,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("graph_relation_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply relation");
        assert_eq!(applied.application_json["target"], "knowledge_graph");
        assert_eq!(applied.application_json["undoable"], true);
        assert!(db
            .find_entity_link(&entity_a.id, &entity_b.id, "supports")
            .expect("find applied link")
            .is_some());

        db.undo_dream_artifact(&artifact.id).expect("undo relation");
        assert!(db
            .find_entity_link(&entity_a.id, &entity_b.id, "supports")
            .expect("find after undo")
            .is_none());
    }

    #[test]
    fn procedural_memory_candidate_applies_and_undoes() {
        let db = Database::open_memory().expect("open memory db");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "procedural_memory_candidate".to_string(),
                title: "Prefer evidence-first checks".to_string(),
                summary: "A reusable workflow lesson from repeated trace review.".to_string(),
                payload_json: json!({
                    "title": "Evidence-first verification",
                    "content": "Before marking work complete, verify each requirement against authoritative current-state evidence.",
                    "tags": ["verification", "workflow"]
                }),
                evidence_json: json!([{"kind": "trace_pattern", "traceId": "trace-1"}]),
                confidence: 0.86,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("procedural_memory_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply procedural memory");
        assert_eq!(
            applied.application_json["target"],
            "agent_procedural_memory"
        );

        let memories = db
            .list_agent_procedural_memories(10)
            .expect("list agent memories");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].source, "dream");

        db.undo_dream_artifact(&artifact.id)
            .expect("undo procedural memory");
        assert!(db
            .list_agent_procedural_memories(10)
            .expect("list after undo")
            .is_empty());
    }

    #[test]
    fn skill_proposal_candidate_creates_pending_review_and_undo_rejects_it() {
        let db = Database::open_memory().expect("open memory db");
        let run = db
            .start_dream_run(StartDreamInput::default())
            .expect("start dream run");
        db.insert_dream_artifact(
            &run.id,
            NewDreamArtifact {
                kind: "skill_proposal_candidate".to_string(),
                title: "Create verification skill".to_string(),
                summary: "Capture a repeatable verification workflow as a reviewed skill proposal."
                    .to_string(),
                payload_json: json!({
                    "action": "create",
                    "name": "Evidence-first Verification",
                    "description": "Verify implementation work against explicit requirements.",
                    "content": "Always enumerate requirements, inspect implementation evidence, and run focused checks before reporting completion.",
                    "rationale": "Repeated successful traces used this workflow."
                }),
                evidence_json: json!([{"kind": "trace_pattern", "traceId": "trace-1"}]),
                confidence: 0.84,
                review_required: true,
            },
        )
        .expect("insert artifact");

        let artifact = db
            .list_dream_artifacts(Some("pending"), Some("skill_proposal_candidate"), 10)
            .expect("list pending")
            .pop()
            .expect("artifact");
        let applied = db
            .apply_dream_artifact(&artifact.id)
            .expect("apply skill proposal candidate");
        assert_eq!(applied.application_json["target"], "skill_change_proposal");

        let proposals = db
            .list_skill_change_proposals(Some(SkillProposalStatus::Pending), 10)
            .expect("list pending proposals");
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].source, "dream");

        db.undo_dream_artifact(&artifact.id)
            .expect("undo skill proposal candidate");
        let rejected = db
            .list_skill_change_proposals(Some(SkillProposalStatus::Rejected), 10)
            .expect("list rejected proposals");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].id, proposals[0].id);
    }
}
