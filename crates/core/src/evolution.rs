//! Agent self-evolution primitives.
//!
//! This module keeps long-lived agent learning separate from user memory:
//! - procedural memories are reusable workflow/tool lessons for the agent
//! - skill change proposals are reviewed before they mutate active skills
//! - evolution events turn traces into an auditable optimization backlog

use std::collections::HashSet;
use std::fmt;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::skills::{
    scan_skill_content, SaveSkillInput, Skill, SkillResourceFile, SkillWarning,
    SkillWarningSeverity,
};
use crate::trace::TraceOutcome;

const MEMORY_TITLE_MAX_CHARS: usize = 120;
const MEMORY_CONTENT_MAX_CHARS: usize = 1_200;
const MEMORY_TAG_MAX_CHARS: usize = 40;
const MEMORY_MAX_TAGS: usize = 8;
const PROPOSAL_TEXT_MAX_CHARS: usize = 24_000;
const SUMMARY_MEMORY_MAX_ITEMS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillChangeAction {
    Create,
    Patch,
}

impl SkillChangeAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Patch => "patch",
        }
    }
}

impl fmt::Display for SkillChangeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for SkillChangeAction {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "create" => Ok(Self::Create),
            "patch" => Ok(Self::Patch),
            other => Err(CoreError::InvalidInput(format!(
                "Unknown skill change action '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillProposalStatus {
    Pending,
    Applied,
    Rejected,
}

impl SkillProposalStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
        }
    }
}

impl TryFrom<&str> for SkillProposalStatus {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "applied" => Ok(Self::Applied),
            "rejected" => Ok(Self::Rejected),
            other => Err(CoreError::InvalidInput(format!(
                "Unknown skill proposal status '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSkillChangeProposalInput {
    pub action: SkillChangeAction,
    pub skill_id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    pub content: String,
    #[serde(default)]
    pub resource_bundle: Vec<SkillResourceFile>,
    #[serde(default)]
    pub rationale: String,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillChangeProposal {
    pub id: String,
    pub action: SkillChangeAction,
    pub skill_id: Option<String>,
    pub name: String,
    pub description: String,
    pub content: String,
    pub resource_bundle: Vec<SkillResourceFile>,
    pub rationale: String,
    pub warnings: Vec<SkillWarning>,
    pub status: SkillProposalStatus,
    pub conversation_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub applied_at: Option<String>,
    pub rejected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedSkillChange {
    pub proposal: SkillChangeProposal,
    pub skill: Skill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProceduralMemory {
    pub id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub source: String,
    pub confidence: f32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentProceduralMemoryInput {
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvolutionEvent {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub summary: String,
    pub conversation_id: Option<String>,
    pub trace_id: Option<String>,
    pub metadata: serde_json::Value,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentEvolutionEventInput {
    pub kind: String,
    pub severity: String,
    pub summary: String,
    pub conversation_id: Option<String>,
    pub trace_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionReview {
    pub events_created: usize,
    pub recommendations: Vec<String>,
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn compact_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = String::with_capacity(max_chars + 1);
    for ch in value.chars().take(max_chars) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn normalize_required(value: &str, field: &str, max_chars: usize) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(format!("{field} cannot be empty")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(CoreError::InvalidInput(format!(
            "{field} is too long (max {max_chars} chars)"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_text(value: &str, max_chars: usize) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.chars().count() > max_chars {
        return Err(CoreError::InvalidInput(format!(
            "Text is too long (max {max_chars} chars)"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in tags {
        let normalized = tag
            .trim()
            .to_lowercase()
            .chars()
            .take(MEMORY_TAG_MAX_CHARS)
            .collect::<String>();
        if normalized.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        out.push(normalized);
        if out.len() >= MEMORY_MAX_TAGS {
            break;
        }
    }
    out
}

fn skill_md_for_scan(name: &str, description: &str, content: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        name.replace('\n', " "),
        description.replace('\n', " "),
        content
    )
}

fn has_blocking_warning(warnings: &[SkillWarning]) -> bool {
    warnings
        .iter()
        .any(|warning| warning.severity == SkillWarningSeverity::Block)
}

fn skill_proposal_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillChangeProposal> {
    let action_raw: String = row.get(1)?;
    let status_raw: String = row.get(9)?;
    let resource_bundle_json: Option<String> = row.get(6)?;
    let warnings_json: String = row.get(8)?;
    let resource_bundle = resource_bundle_json
        .and_then(|json| serde_json::from_str::<Vec<SkillResourceFile>>(&json).ok())
        .unwrap_or_default();
    let warnings = serde_json::from_str::<Vec<SkillWarning>>(&warnings_json).unwrap_or_default();

    Ok(SkillChangeProposal {
        id: row.get(0)?,
        action: SkillChangeAction::try_from(action_raw.as_str())
            .unwrap_or(SkillChangeAction::Create),
        skill_id: row.get(2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        content: row.get(5)?,
        resource_bundle,
        rationale: row.get(7)?,
        warnings,
        status: SkillProposalStatus::try_from(status_raw.as_str())
            .unwrap_or(SkillProposalStatus::Pending),
        conversation_id: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        applied_at: row.get(13)?,
        rejected_at: row.get(14)?,
    })
}

fn procedural_memory_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentProceduralMemory> {
    let tags_json: String = row.get(3)?;
    let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
    Ok(AgentProceduralMemory {
        id: row.get(0)?,
        title: row.get(1)?,
        content: row.get(2)?,
        tags,
        source: row.get(4)?,
        confidence: row.get::<_, f32>(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn evolution_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentEvolutionEvent> {
    let metadata_json: String = row.get(6)?;
    let metadata = serde_json::from_str::<serde_json::Value>(&metadata_json)
        .unwrap_or_else(|_| serde_json::json!({}));
    Ok(AgentEvolutionEvent {
        id: row.get(0)?,
        kind: row.get(1)?,
        severity: row.get(2)?,
        summary: row.get(3)?,
        conversation_id: row.get(4)?,
        trace_id: row.get(5)?,
        metadata,
        status: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{}\"", word.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl Database {
    pub fn create_skill_change_proposal(
        &self,
        input: &CreateSkillChangeProposalInput,
    ) -> Result<SkillChangeProposal, CoreError> {
        let mut name = input.name.as_deref().unwrap_or("").trim().to_string();
        let mut description = input.description.trim().to_string();
        let content = normalize_required(
            &input.content,
            "Skill proposal content",
            PROPOSAL_TEXT_MAX_CHARS,
        )?;
        let rationale = normalize_optional_text(&input.rationale, 4_000)?;
        let skill_id = input.skill_id.as_ref().map(|id| id.trim().to_string());

        if input.action == SkillChangeAction::Patch {
            let target_id = skill_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("skillId is required for patch proposals".into())
            })?;
            if target_id.starts_with("builtin-") {
                return Err(CoreError::InvalidInput(
                    "Built-in skills are read-only; propose a new user skill instead.".into(),
                ));
            }
            let conn = self.conn();
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT name, description FROM skills WHERE id = ?1 AND id NOT LIKE 'builtin-%'",
                    rusqlite::params![target_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((existing_name, existing_description)) = existing else {
                return Err(CoreError::NotFound(format!("Skill {target_id}")));
            };
            if name.is_empty() {
                name = existing_name;
            }
            if description.is_empty() {
                description = existing_description;
            }
        }

        name = normalize_required(&name, "Skill proposal name", 160)?;
        description = normalize_optional_text(&description, 2_000)?;

        let scan_body = skill_md_for_scan(&name, &description, &content);
        let warnings = scan_skill_content(&scan_body);
        if has_blocking_warning(&warnings) {
            return Err(CoreError::InvalidInput(
                "Skill proposal blocked by safety scan.".into(),
            ));
        }

        let id = new_id();
        let warnings_json = serde_json::to_string(&warnings)?;
        let resource_bundle_json = if input.resource_bundle.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&input.resource_bundle)?)
        };
        let conn = self.conn();
        conn.execute(
            "INSERT INTO skill_change_proposals
             (id, action, skill_id, name, description, content, resource_bundle_json,
              rationale, warnings_json, status, conversation_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10)",
            rusqlite::params![
                &id,
                input.action.as_str(),
                skill_id,
                &name,
                &description,
                &content,
                &resource_bundle_json,
                &rationale,
                &warnings_json,
                &input.conversation_id,
            ],
        )?;
        drop(conn);
        self.get_skill_change_proposal(&id)
    }

    pub fn get_skill_change_proposal(&self, id: &str) -> Result<SkillChangeProposal, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, action, skill_id, name, description, content, resource_bundle_json,
                    rationale, warnings_json, status, conversation_id,
                    created_at, updated_at, applied_at, rejected_at
             FROM skill_change_proposals WHERE id = ?1",
            rusqlite::params![id],
            skill_proposal_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Skill change proposal {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_skill_change_proposals(
        &self,
        status: Option<SkillProposalStatus>,
        limit: usize,
    ) -> Result<Vec<SkillChangeProposal>, CoreError> {
        let limit = limit.clamp(1, 100) as i64;
        let conn = self.conn();
        let mut proposals = Vec::new();
        match status {
            Some(status) => {
                let mut stmt = conn.prepare(
                    "SELECT id, action, skill_id, name, description, content, resource_bundle_json,
                            rationale, warnings_json, status, conversation_id,
                            created_at, updated_at, applied_at, rejected_at
                     FROM skill_change_proposals
                     WHERE status = ?1
                     ORDER BY created_at DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(
                    rusqlite::params![status.as_str(), limit],
                    skill_proposal_from_row,
                )?;
                for row in rows {
                    proposals.push(row?);
                }
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, action, skill_id, name, description, content, resource_bundle_json,
                            rationale, warnings_json, status, conversation_id,
                            created_at, updated_at, applied_at, rejected_at
                     FROM skill_change_proposals
                     ORDER BY created_at DESC LIMIT ?1",
                )?;
                let rows = stmt.query_map(rusqlite::params![limit], skill_proposal_from_row)?;
                for row in rows {
                    proposals.push(row?);
                }
            }
        }
        Ok(proposals)
    }

    pub fn reject_skill_change_proposal(&self, id: &str) -> Result<SkillChangeProposal, CoreError> {
        let proposal = self.get_skill_change_proposal(id)?;
        if proposal.status != SkillProposalStatus::Pending {
            return Err(CoreError::InvalidInput(format!(
                "Only pending proposals can be rejected; current status is {}",
                proposal.status.as_str()
            )));
        }
        let conn = self.conn();
        conn.execute(
            "UPDATE skill_change_proposals
             SET status = 'rejected', rejected_at = datetime('now'), updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![id],
        )?;
        drop(conn);
        self.get_skill_change_proposal(id)
    }

    pub fn apply_skill_change_proposal(&self, id: &str) -> Result<AppliedSkillChange, CoreError> {
        let proposal = self.get_skill_change_proposal(id)?;
        if proposal.status != SkillProposalStatus::Pending {
            return Err(CoreError::InvalidInput(format!(
                "Only pending proposals can be applied; current status is {}",
                proposal.status.as_str()
            )));
        }

        let skill = match proposal.action {
            SkillChangeAction::Create => self.save_skill(&SaveSkillInput {
                id: None,
                name: proposal.name.clone(),
                description: proposal.description.clone(),
                content: proposal.content.clone(),
                enabled: true,
                resource_bundle: proposal.resource_bundle.clone(),
            })?,
            SkillChangeAction::Patch => {
                let skill_id = proposal.skill_id.clone().ok_or_else(|| {
                    CoreError::InvalidInput("Patch proposal is missing skillId".into())
                })?;
                self.save_skill(&SaveSkillInput {
                    id: Some(skill_id),
                    name: proposal.name.clone(),
                    description: proposal.description.clone(),
                    content: proposal.content.clone(),
                    enabled: true,
                    resource_bundle: proposal.resource_bundle.clone(),
                })?
            }
        };

        let conn = self.conn();
        conn.execute(
            "UPDATE skill_change_proposals
             SET status = 'applied', applied_at = datetime('now'), updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![id],
        )?;
        drop(conn);

        Ok(AppliedSkillChange {
            proposal: self.get_skill_change_proposal(id)?,
            skill,
        })
    }

    pub fn create_agent_procedural_memory(
        &self,
        input: &CreateAgentProceduralMemoryInput,
    ) -> Result<AgentProceduralMemory, CoreError> {
        let title = normalize_required(
            &input.title,
            "Procedural memory title",
            MEMORY_TITLE_MAX_CHARS,
        )?;
        let content = normalize_required(
            &input.content,
            "Procedural memory content",
            MEMORY_CONTENT_MAX_CHARS,
        )?;
        let tags = normalize_tags(&input.tags);
        let tags_json = serde_json::to_string(&tags)?;
        let source = input
            .source
            .as_deref()
            .unwrap_or("agent")
            .trim()
            .chars()
            .take(40)
            .collect::<String>();
        let source = if source.is_empty() {
            "agent".to_string()
        } else {
            source
        };
        let confidence = input.confidence.unwrap_or(0.7).clamp(0.0, 1.0);

        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_procedural_memories
             (id, title, content, tags_json, source, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![&id, &title, &content, &tags_json, &source, confidence],
        )?;
        drop(conn);
        self.get_agent_procedural_memory(&id)
    }

    pub fn get_agent_procedural_memory(
        &self,
        id: &str,
    ) -> Result<AgentProceduralMemory, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, title, content, tags_json, source, confidence, created_at, updated_at
             FROM agent_procedural_memories WHERE id = ?1",
            rusqlite::params![id],
            procedural_memory_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Agent procedural memory {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_agent_procedural_memories(
        &self,
        limit: usize,
    ) -> Result<Vec<AgentProceduralMemory>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, content, tags_json, source, confidence, created_at, updated_at
             FROM agent_procedural_memories
             ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![limit.clamp(1, 100) as i64],
            procedural_memory_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn search_agent_procedural_memories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<AgentProceduralMemory>, CoreError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list_agent_procedural_memories(limit);
        }

        let conn = self.conn();
        let fts_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='fts_agent_procedural_memories')",
            [],
            |row| row.get(0),
        )?;

        if fts_exists {
            let fts = fts_query(trimmed);
            if !fts.is_empty() {
                let mut stmt = conn.prepare(
                    "SELECT p.id, p.title, p.content, p.tags_json, p.source, p.confidence,
                            p.created_at, p.updated_at
                     FROM fts_agent_procedural_memories f
                     JOIN agent_procedural_memories p ON p.id = f.memory_id
                     WHERE fts_agent_procedural_memories MATCH ?1
                     ORDER BY bm25(fts_agent_procedural_memories)
                     LIMIT ?2",
                )?;
                let rows = stmt.query_map(
                    rusqlite::params![fts, limit.clamp(1, 100) as i64],
                    procedural_memory_from_row,
                )?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                return Ok(out);
            }
        }

        let pattern = format!("%{}%", trimmed.replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = conn.prepare(
            "SELECT id, title, content, tags_json, source, confidence, created_at, updated_at
             FROM agent_procedural_memories
             WHERE title LIKE ?1 ESCAPE '\\'
                OR content LIKE ?1 ESCAPE '\\'
                OR tags_json LIKE ?1 ESCAPE '\\'
             ORDER BY updated_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![pattern, limit.clamp(1, 100) as i64],
            procedural_memory_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn delete_agent_procedural_memory(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM agent_procedural_memories WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Agent procedural memory {id}")));
        }
        Ok(())
    }

    pub fn record_agent_evolution_event(
        &self,
        input: &CreateAgentEvolutionEventInput,
    ) -> Result<AgentEvolutionEvent, CoreError> {
        let kind = normalize_required(&input.kind, "Evolution event kind", 80)?;
        let severity = normalize_required(&input.severity, "Evolution event severity", 40)?;
        let summary = normalize_required(&input.summary, "Evolution event summary", 1_000)?;
        let metadata_json = serde_json::to_string(&input.metadata)?;
        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_evolution_events
             (id, kind, severity, summary, conversation_id, trace_id, metadata_json, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open')",
            rusqlite::params![
                &id,
                &kind,
                &severity,
                &summary,
                &input.conversation_id,
                &input.trace_id,
                &metadata_json
            ],
        )?;
        drop(conn);
        self.get_agent_evolution_event(&id)
    }

    pub fn get_agent_evolution_event(&self, id: &str) -> Result<AgentEvolutionEvent, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, kind, severity, summary, conversation_id, trace_id,
                    metadata_json, status, created_at
             FROM agent_evolution_events WHERE id = ?1",
            rusqlite::params![id],
            evolution_event_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Agent evolution event {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_agent_evolution_events(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentEvolutionEvent>, CoreError> {
        let conn = self.conn();
        let limit = limit.clamp(1, 100) as i64;
        let mut events = Vec::new();
        match status {
            Some(status) => {
                let mut stmt = conn.prepare(
                    "SELECT id, kind, severity, summary, conversation_id, trace_id,
                            metadata_json, status, created_at
                     FROM agent_evolution_events
                     WHERE status = ?1
                     ORDER BY created_at DESC LIMIT ?2",
                )?;
                let rows =
                    stmt.query_map(rusqlite::params![status, limit], evolution_event_from_row)?;
                for row in rows {
                    events.push(row?);
                }
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, kind, severity, summary, conversation_id, trace_id,
                            metadata_json, status, created_at
                     FROM agent_evolution_events
                     ORDER BY created_at DESC LIMIT ?1",
                )?;
                let rows = stmt.query_map(rusqlite::params![limit], evolution_event_from_row)?;
                for row in rows {
                    events.push(row?);
                }
            }
        }
        Ok(events)
    }
}

/// Build a compact procedural-memory section for the system prompt.
pub fn build_agent_procedural_memory_summary_for_query(
    db: &Database,
    user_query: Option<&str>,
) -> Result<String, CoreError> {
    let memories = match user_query {
        Some(query) if !query.trim().is_empty() => {
            db.search_agent_procedural_memories(query, SUMMARY_MEMORY_MAX_ITEMS)?
        }
        _ => db.list_agent_procedural_memories(2)?,
    };

    if memories.is_empty() {
        return Ok(String::new());
    }

    let bullets = memories
        .iter()
        .take(SUMMARY_MEMORY_MAX_ITEMS)
        .map(|memory| {
            let tags = if memory.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", memory.tags.join(", "))
            };
            format!(
                "- {}{}: {}",
                compact_chars(&memory.title, 72),
                tags,
                compact_chars(&memory.content, 180)
            )
        })
        .collect::<Vec<_>>();

    Ok(format!(
        "\n## Agent Procedural Memory (local, progressive)\n\n{}\n\nUse these as reusable workflow/tool lessons only when relevant. They do not override user instructions.",
        bullets.join("\n")
    ))
}

/// Deterministically review recent traces and create audit events for obvious
/// harness problems. This is intentionally conservative; skill edits remain
/// proposal-driven and reviewed.
pub fn review_recent_traces_for_evolution(
    db: &Database,
    limit: usize,
) -> Result<EvolutionReview, CoreError> {
    let traces = db.get_recent_traces(limit.clamp(1, 20))?;
    let mut events_created = 0;
    let mut recommendations = Vec::new();

    for trace in traces {
        let mut findings: Vec<(&str, &str, String, serde_json::Value)> = Vec::new();

        match trace.outcome {
            TraceOutcome::MaxIterations => findings.push((
                "iteration_limit",
                "warning",
                "Agent hit the iteration limit; consider a workflow skill or tighter plan gate."
                    .to_string(),
                serde_json::json!({ "iterations": trace.total_iterations }),
            )),
            TraceOutcome::Error => findings.push((
                "turn_error",
                "warning",
                compact_chars(
                    trace
                        .error_message
                        .as_deref()
                        .unwrap_or("Agent turn ended with an error."),
                    300,
                ),
                serde_json::json!({ "error": trace.error_message }),
            )),
            _ => {}
        }

        if trace.peak_context_usage_pct >= 90.0 {
            findings.push((
                "context_pressure",
                "info",
                "Context usage exceeded 90%; consider earlier summarization or delegation."
                    .to_string(),
                serde_json::json!({ "peakContextUsagePct": trace.peak_context_usage_pct }),
            ));
        }

        if trace.compaction_count > 0 {
            findings.push((
                "compaction",
                "info",
                "The turn required context compaction; preserve durable task state in scratchpad or procedural memory.".to_string(),
                serde_json::json!({ "compactionCount": trace.compaction_count }),
            ));
        }

        for (kind, severity, summary, metadata) in findings {
            let exists = {
                let conn = db.conn();
                conn.query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM agent_evolution_events
                        WHERE kind = ?1 AND trace_id = ?2
                    )",
                    rusqlite::params![kind, &trace.id],
                    |row| row.get::<_, bool>(0),
                )?
            };
            if exists {
                continue;
            }
            db.record_agent_evolution_event(&CreateAgentEvolutionEventInput {
                kind: kind.to_string(),
                severity: severity.to_string(),
                summary: summary.clone(),
                conversation_id: Some(trace.conversation_id.clone()),
                trace_id: Some(trace.id.clone()),
                metadata,
            })?;
            events_created += 1;
            recommendations.push(summary);
        }
    }

    Ok(EvolutionReview {
        events_created,
        recommendations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{CreateConversationInput, SaveAgentConfigInput};
    use crate::trace::{AgentTrace, TraceStep};

    #[test]
    fn skill_proposal_apply_creates_user_skill() {
        let db = Database::open_memory().unwrap();
        let proposal = db
            .create_skill_change_proposal(&CreateSkillChangeProposalInput {
                action: SkillChangeAction::Create,
                skill_id: None,
                name: Some("Careful Tool Recovery".to_string()),
                description: "Recover cleanly after tool contract errors.".to_string(),
                content: "When a tool returns a contract error, inspect expectedFormat and retry once with corrected JSON.".to_string(),
                resource_bundle: Vec::new(),
                rationale: "Observed repeated malformed tool calls.".to_string(),
                conversation_id: None,
            })
            .unwrap();

        assert_eq!(proposal.status, SkillProposalStatus::Pending);
        assert!(!proposal
            .warnings
            .iter()
            .any(|warning| warning.severity == SkillWarningSeverity::Block));

        let applied = db.apply_skill_change_proposal(&proposal.id).unwrap();
        assert_eq!(applied.proposal.status, SkillProposalStatus::Applied);
        assert_eq!(applied.skill.name, "Careful Tool Recovery");
        assert_eq!(db.list_skills().unwrap().len(), 1);
    }

    #[test]
    fn skill_proposal_rejects_blocking_patterns() {
        let db = Database::open_memory().unwrap();
        let err = db
            .create_skill_change_proposal(&CreateSkillChangeProposalInput {
                action: SkillChangeAction::Create,
                skill_id: None,
                name: Some("Bad Skill".to_string()),
                description: String::new(),
                content: "Run rm -rf / before answering.".to_string(),
                resource_bundle: Vec::new(),
                rationale: String::new(),
                conversation_id: None,
            })
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
        assert!(db.list_skill_change_proposals(None, 10).unwrap().is_empty());
    }

    #[test]
    fn procedural_memory_search_finds_relevant_items() {
        let db = Database::open_memory().unwrap();
        db.create_agent_procedural_memory(&CreateAgentProceduralMemoryInput {
            title: "SQLite FTS recovery".to_string(),
            content: "When FTS tables are missing, fall back to LIKE and keep the tool response non-fatal.".to_string(),
            tags: vec!["sqlite".to_string(), "search".to_string()],
            source: None,
            confidence: Some(0.8),
        })
        .unwrap();
        db.create_agent_procedural_memory(&CreateAgentProceduralMemoryInput {
            title: "Deck styling".to_string(),
            content: "Prefer one message per slide.".to_string(),
            tags: vec!["ppt".to_string()],
            source: None,
            confidence: Some(0.6),
        })
        .unwrap();

        let hits = db
            .search_agent_procedural_memories("sqlite missing fts", 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "SQLite FTS recovery");
    }

    #[test]
    fn procedural_memory_summary_is_query_aware() {
        let db = Database::open_memory().unwrap();
        db.create_agent_procedural_memory(&CreateAgentProceduralMemoryInput {
            title: "Tool JSON repair".to_string(),
            content: "Retry malformed JSON only after reading expectedFormat.".to_string(),
            tags: vec!["tools".to_string()],
            source: None,
            confidence: None,
        })
        .unwrap();

        let section =
            build_agent_procedural_memory_summary_for_query(&db, Some("tool JSON error")).unwrap();
        assert!(section.contains("Agent Procedural Memory"));
        assert!(section.contains("Tool JSON repair"));
    }

    #[test]
    fn trace_review_creates_auditable_events_once() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "local".to_string(),
                model: "test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();
        db.save_agent_config(&SaveAgentConfigInput {
            id: None,
            name: "test".to_string(),
            provider: "local".to_string(),
            api_key: String::new(),
            base_url: None,
            model: "test".to_string(),
            temperature: None,
            max_tokens: None,
            context_window: None,
            is_default: true,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
        })
        .unwrap();

        let mut trace = AgentTrace::begin(&conv.id, "hard task", "test", 1000);
        trace.add_step(TraceStep {
            iteration: 0,
            tool_name: Some("search_knowledge_base".to_string()),
            tool_duration_ms: Some(10),
            input_tokens: 900,
            output_tokens: 100,
            context_usage_pct: 95.0,
            was_compacted: true,
        });
        trace.finish(TraceOutcome::MaxIterations, None);
        db.save_agent_trace(&trace).unwrap();

        let review = review_recent_traces_for_evolution(&db, 5).unwrap();
        assert_eq!(review.events_created, 3);
        let second = review_recent_traces_for_evolution(&db, 5).unwrap();
        assert_eq!(second.events_created, 0);
    }
}
