//! Lint — knowledge base health checks: stale docs, orphans, contradictions, coverage gaps.

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub stale_documents: Vec<HealthIssue>,
    pub orphan_documents: Vec<HealthIssue>,
    pub low_coverage_entities: Vec<HealthIssue>,
    pub duplicate_candidates: Vec<HealthIssue>,
    pub total_issues: usize,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthIssue {
    pub id: String,
    pub check_type: CheckType,
    pub severity: Severity,
    pub target_doc_id: Option<String>,
    pub target_entity_id: Option<String>,
    pub description: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckType {
    Stale,
    Orphan,
    Gap,
    Duplicate,
    Contradiction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Database {
    /// Run all health checks (non-LLM ones). For contradiction detection, call
    /// `check_contradictions()` separately.
    pub fn run_health_check(&self, stale_days: u32) -> Result<HealthReport, CoreError> {
        let now = chrono::Utc::now().to_rfc3339();
        let stale = self.check_stale_documents(stale_days)?;
        let orphans = self.check_orphan_documents()?;
        let low_coverage = self.check_low_coverage_entities()?;
        let duplicates = self.check_duplicate_candidates()?;

        let total = stale.len() + orphans.len() + low_coverage.len() + duplicates.len();

        // Persist results
        for issue in stale
            .iter()
            .chain(orphans.iter())
            .chain(low_coverage.iter())
            .chain(duplicates.iter())
        {
            self.save_health_issue(issue)?;
        }

        Ok(HealthReport {
            stale_documents: stale,
            orphan_documents: orphans,
            low_coverage_entities: low_coverage,
            duplicate_candidates: duplicates,
            total_issues: total,
            checked_at: now,
        })
    }

    fn check_stale_documents(&self, days: u32) -> Result<Vec<HealthIssue>, CoreError> {
        let conn = self.conn();
        let threshold = format!("-{days} days");
        let mut stmt = conn.prepare(
            "SELECT id, path, modified_at FROM documents WHERE modified_at < datetime('now', ?1)",
        )?;
        let issues = stmt
            .query_map(rusqlite::params![threshold], |row| {
                let doc_id: String = row.get(0)?;
                let path: String = row.get(1)?;
                let updated: String = row.get(2)?;
                Ok(HealthIssue {
                    id: uuid::Uuid::new_v4().to_string(),
                    check_type: CheckType::Stale,
                    severity: if days > 180 {
                        Severity::Warning
                    } else {
                        Severity::Info
                    },
                    target_doc_id: Some(doc_id),
                    target_entity_id: None,
                    description: format!("Document '{path}' last updated: {updated}"),
                    suggestion: "Re-index or verify content is still current.".to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    fn check_orphan_documents(&self) -> Result<Vec<HealthIssue>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.path FROM documents d
             LEFT JOIN document_entities de ON d.id = de.document_id
             LEFT JOIN document_summaries ds ON d.id = ds.document_id
             WHERE de.entity_id IS NULL AND ds.id IS NULL",
        )?;
        let issues = stmt
            .query_map([], |row| {
                Ok(HealthIssue {
                    id: uuid::Uuid::new_v4().to_string(),
                    check_type: CheckType::Orphan,
                    severity: Severity::Info,
                    target_doc_id: Some(row.get(0)?),
                    target_entity_id: None,
                    description: format!(
                        "Document '{}' has no entities or summary — not compiled yet.",
                        row.get::<_, String>(1)?
                    ),
                    suggestion: "Run knowledge compilation to integrate this document.".to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    fn check_low_coverage_entities(&self) -> Result<Vec<HealthIssue>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.entity_type,
                    (SELECT COUNT(*) FROM document_entities de WHERE de.entity_id = e.id) as doc_count
             FROM entities e
             WHERE (SELECT COUNT(*) FROM document_entities de WHERE de.entity_id = e.id) <= 1
             ORDER BY e.mention_count DESC LIMIT 20",
        )?;
        let issues = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let etype: String = row.get(2)?;
                Ok(HealthIssue {
                    id: uuid::Uuid::new_v4().to_string(),
                    check_type: CheckType::Gap,
                    severity: Severity::Info,
                    target_doc_id: None,
                    target_entity_id: Some(row.get(0)?),
                    description: format!(
                        "Entity '{name}' ({etype}) has only 1 supporting document."
                    ),
                    suggestion: format!("Consider adding more sources about '{name}'."),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    fn check_duplicate_candidates(&self) -> Result<Vec<HealthIssue>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name, b.id, b.name FROM entities a, entities b
             WHERE a.id < b.id AND a.entity_type = b.entity_type
             AND LOWER(a.name) = LOWER(b.name)
             LIMIT 20",
        )?;
        let issues = stmt
            .query_map([], |row| {
                let name_a: String = row.get(1)?;
                let name_b: String = row.get(3)?;
                Ok(HealthIssue {
                    id: uuid::Uuid::new_v4().to_string(),
                    check_type: CheckType::Duplicate,
                    severity: Severity::Warning,
                    target_doc_id: None,
                    target_entity_id: Some(row.get(0)?),
                    description: format!("Potential duplicate entities: '{name_a}' and '{name_b}'"),
                    suggestion: "Consider merging these entities.".to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    fn save_health_issue(&self, issue: &HealthIssue) -> Result<(), CoreError> {
        let conn = self.conn();
        let check_type = serde_json::to_value(&issue.check_type)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let severity = serde_json::to_value(&issue.severity)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO health_checks (id, check_type, severity, target_doc_id, target_entity_id, description, suggestion) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                issue.id,
                check_type,
                severity,
                issue.target_doc_id,
                issue.target_entity_id,
                issue.description,
                issue.suggestion
            ],
        )?;
        Ok(())
    }

    pub fn get_unresolved_health_issues(&self) -> Result<Vec<HealthIssue>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, check_type, severity, target_doc_id, target_entity_id, description, suggestion FROM health_checks WHERE resolved = 0 ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END",
        )?;
        let issues = stmt
            .query_map([], |row| {
                let ct: String = row.get(1)?;
                let sv: String = row.get(2)?;
                Ok(HealthIssue {
                    id: row.get(0)?,
                    check_type: match ct.as_str() {
                        "stale" => CheckType::Stale,
                        "orphan" => CheckType::Orphan,
                        "gap" => CheckType::Gap,
                        "duplicate" => CheckType::Duplicate,
                        "contradiction" => CheckType::Contradiction,
                        _ => CheckType::Gap,
                    },
                    severity: match sv.as_str() {
                        "critical" => Severity::Critical,
                        "warning" => Severity::Warning,
                        _ => Severity::Info,
                    },
                    target_doc_id: row.get(3)?,
                    target_entity_id: row.get(4)?,
                    description: row.get(5)?,
                    suggestion: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(issues)
    }

    pub fn resolve_health_issue(&self, issue_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE health_checks SET resolved = 1 WHERE id = ?1",
            rusqlite::params![issue_id],
        )?;
        Ok(())
    }

    /// Get all documents linked to an entity (reverse lookup).
    fn get_entities_for_document_reverse(
        &self,
        entity_id: &str,
    ) -> Result<Vec<(String, String)>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.path FROM documents d JOIN document_entities de ON d.id = de.document_id WHERE de.entity_id = ?1",
        )?;
        let docs = stmt
            .query_map(rusqlite::params![entity_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(docs)
    }
}

/// Check contradictions between documents covering the same entity (requires LLM).
pub async fn check_contradictions(
    db: &Database,
    entity_id: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<Vec<HealthIssue>, CoreError> {
    let entity = db.get_entity_by_id(entity_id)?;
    let docs = db.get_entities_for_document_reverse(entity_id)?;

    if docs.len() < 2 {
        return Ok(Vec::new());
    }

    // Get summaries for each doc
    let mut doc_summaries = Vec::new();
    for (doc_id, path) in &docs {
        if let Ok(Some(summary)) = db.get_document_summary(doc_id) {
            doc_summaries.push(format!("Document '{path}': {}", summary.summary));
        }
    }

    if doc_summaries.len() < 2 {
        return Ok(Vec::new());
    }

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(
                Role::System,
                include_str!("../prompts/contradiction_check.md").to_string(),
            ),
            Message::text(
                Role::User,
                format!(
                    "Entity: {}\n\nDocuments:\n{}",
                    entity.name,
                    doc_summaries.join("\n\n")
                ),
            ),
        ],
        max_tokens: Some(1000),
        temperature: Some(0.1),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
    };

    let response = provider.complete(&request).await?;

    #[derive(Deserialize)]
    struct Contradiction {
        description: String,
        severity: String,
    }

    let contradictions: Vec<Contradiction> =
        serde_json::from_str(response.content.trim()).unwrap_or_default();

    Ok(contradictions
        .iter()
        .map(|c| HealthIssue {
            id: uuid::Uuid::new_v4().to_string(),
            check_type: CheckType::Contradiction,
            severity: match c.severity.as_str() {
                "critical" => Severity::Critical,
                "warning" => Severity::Warning,
                _ => Severity::Info,
            },
            target_doc_id: None,
            target_entity_id: Some(entity_id.to_string()),
            description: c.description.clone(),
            suggestion: format!(
                "Review documents referencing '{}' for consistency.",
                entity.name
            ),
        })
        .collect())
}
