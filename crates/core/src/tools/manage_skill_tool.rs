//! ManageSkillTool - controlled skill self-evolution.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::evolution::{CreateSkillChangeProposalInput, SkillChangeAction, SkillProposalStatus};

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_skill.json");

pub struct ManageSkillTool;

#[derive(Debug, Deserialize)]
struct ManageSkillArgs {
    action: String,
    #[serde(default)]
    proposal_id: Option<String>,
    #[serde(default)]
    skill_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn parse_status(value: Option<&str>) -> Result<Option<SkillProposalStatus>, CoreError> {
    value.map(SkillProposalStatus::try_from).transpose()
}

fn missing(field: &str, action: &str) -> CoreError {
    CoreError::InvalidInput(format!(
        "{field} is required for manage_skill action '{action}'"
    ))
}

#[async_trait]
impl Tool for ManageSkillTool {
    fn name(&self) -> &str {
        "manage_skill"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core, ToolCategory::Knowledge]
    }

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        args.get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|action| action == "apply_proposal")
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args.get("action")?.as_str()?;
        if action != "apply_proposal" {
            return None;
        }
        let id = args
            .get("proposal_id")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        Some(format!(
            "Apply skill change proposal {id}. This will create or update an active user skill."
        ))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ManageSkillArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid manage_skill arguments: {e}")))?;

        let action = args.action.trim();
        match action {
            "propose_create" | "propose_patch" => {
                let is_patch = action == "propose_patch";
                let content = args.content.ok_or_else(|| missing("content", action))?;
                let proposal =
                    db.create_skill_change_proposal(&CreateSkillChangeProposalInput {
                        action: if is_patch {
                            SkillChangeAction::Patch
                        } else {
                            SkillChangeAction::Create
                        },
                        skill_id: args.skill_id,
                        name: args.name,
                        description: args.description.unwrap_or_default(),
                        content,
                        resource_bundle: Vec::new(),
                        rationale: args.rationale.unwrap_or_default(),
                        conversation_id: None,
                    })?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "Skill change proposal created: {} ({:?}). Status: pending. Warnings: {}.",
                        proposal.id,
                        proposal.action,
                        proposal.warnings.len()
                    ),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillChangeProposal",
                        "proposal": proposal
                    })),
                })
            }
            "list_proposals" => {
                let status = parse_status(args.status.as_deref())?;
                let proposals = db.list_skill_change_proposals(status, args.limit.unwrap_or(10))?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Found {} skill change proposal(s).", proposals.len()),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillChangeProposalList",
                        "proposals": proposals
                    })),
                })
            }
            "list_skills" => {
                let mut skills = crate::skills::load_builtin_skills();
                skills.extend(db.list_skills()?);
                let limit = args.limit.unwrap_or(50).min(100);
                let summaries = skills
                    .into_iter()
                    .take(limit)
                    .map(|skill| {
                        serde_json::json!({
                            "id": skill.id,
                            "name": skill.name,
                            "description": skill.description,
                            "enabled": skill.enabled,
                            "builtin": skill.builtin,
                            "resources": skill.resources,
                        })
                    })
                    .collect::<Vec<_>>();
                let list_text = summaries
                    .iter()
                    .map(|skill| {
                        let id = skill.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let name = skill.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let enabled = skill
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let builtin = skill
                            .get("builtin")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let description = skill
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        format!(
                            "- id: {id} | name: {name} | enabled: {enabled} | builtin: {builtin}\n  description: {description}"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Found {} skill(s):\n{}", summaries.len(), list_text),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillList",
                        "skills": summaries
                    })),
                })
            }
            "view_skill" => {
                let skill_id = args.skill_id.ok_or_else(|| missing("skill_id", action))?;
                let mut skills = crate::skills::load_builtin_skills();
                skills.extend(db.list_skills()?);
                let skill = skills
                    .into_iter()
                    .find(|skill| {
                        skill.id == skill_id
                            || skill.name == skill_id
                            || skill.id.strip_prefix("builtin-") == Some(skill_id.as_str())
                    })
                    .ok_or_else(|| CoreError::NotFound(format!("Skill {skill_id}")))?;
                let resources = if skill.resources.is_empty() {
                    "Resources: none".to_string()
                } else {
                    format!(
                        "Resources:\n{}",
                        skill
                            .resources
                            .iter()
                            .map(|resource| {
                                format!(
                                    "- {} ({:?}, {} bytes)",
                                    resource.path, resource.kind, resource.bytes
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                let content = format!(
                    "Skill: {} ({})\nEnabled: {}\nBuiltin: {}\nDescription: {}\n{}\n\nContent:\n{}",
                    skill.name,
                    skill.id,
                    skill.enabled,
                    skill.builtin,
                    skill.description,
                    resources,
                    skill.content
                );
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content,
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skill",
                        "skill": skill
                    })),
                })
            }
            "view_proposal" => {
                let proposal_id = args
                    .proposal_id
                    .ok_or_else(|| missing("proposal_id", action))?;
                let proposal = db.get_skill_change_proposal(&proposal_id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "Skill change proposal {} is {:?}.",
                        proposal.id, proposal.status
                    ),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillChangeProposal",
                        "proposal": proposal
                    })),
                })
            }
            "apply_proposal" => {
                let proposal_id = args
                    .proposal_id
                    .ok_or_else(|| missing("proposal_id", action))?;
                let applied = db.apply_skill_change_proposal(&proposal_id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "Skill proposal applied. Active skill: {} ({})",
                        applied.skill.name, applied.skill.id
                    ),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "appliedSkillChange",
                        "applied": applied
                    })),
                })
            }
            "reject_proposal" => {
                let proposal_id = args
                    .proposal_id
                    .ok_or_else(|| missing("proposal_id", action))?;
                let proposal = db.reject_skill_change_proposal(&proposal_id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Skill proposal rejected: {}", proposal.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillChangeProposal",
                        "proposal": proposal
                    })),
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown manage_skill action '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_loads() {
        let tool = ManageSkillTool;
        assert_eq!(tool.name(), "manage_skill");
        assert!(tool.description().contains("skill"));
        assert_eq!(tool.parameters_schema()["type"], "object");
    }

    #[tokio::test]
    async fn list_and_view_skills() {
        let db = Database::open_memory().unwrap();
        let tool = ManageSkillTool;
        let list_args = serde_json::json!({
            "action": "list_skills",
            "limit": 3
        });
        let listed = tool
            .execute("call-list", &list_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!listed.is_error);
        assert_eq!(listed.artifacts.as_ref().unwrap()["kind"], "skillList");
        assert!(
            listed.content.contains("builtin-visual-explanations"),
            "list output should include skill ids for follow-up view_skill calls: {}",
            listed.content
        );

        let view_args = serde_json::json!({
            "action": "view_skill",
            "skill_id": "builtin-evidence-first"
        });
        let viewed = tool
            .execute("call-view", &view_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!viewed.is_error);
        assert_eq!(viewed.artifacts.as_ref().unwrap()["kind"], "skill");
        assert_eq!(
            viewed.artifacts.as_ref().unwrap()["skill"]["id"],
            "builtin-evidence-first"
        );
        assert!(
            viewed.content.contains("##") || viewed.content.contains("Guidance"),
            "view output should include skill body, not just metadata: {}",
            viewed.content
        );

        let slug_view_args = serde_json::json!({
            "action": "view_skill",
            "skill_id": "doc-script-editor"
        });
        let slug_viewed = tool
            .execute("call-view-slug", &slug_view_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!slug_viewed.is_error);
        assert!(slug_viewed.content.contains("builtin-doc-script-editor"));
    }

    #[tokio::test]
    async fn propose_and_apply_skill() {
        let db = Database::open_memory().unwrap();
        let tool = ManageSkillTool;
        let args = serde_json::json!({
            "action": "propose_create",
            "name": "Tool Retry Discipline",
            "description": "Recover from malformed tool arguments.",
            "content": "When a tool returns expectedFormat, inspect it before retrying.",
            "rationale": "Repeated JSON contract failures."
        });
        let result = tool
            .execute("call-1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error);

        let proposals = db
            .list_skill_change_proposals(Some(SkillProposalStatus::Pending), 10)
            .unwrap();
        assert_eq!(proposals.len(), 1);

        let apply_args = serde_json::json!({
            "action": "apply_proposal",
            "proposal_id": proposals[0].id
        });
        let applied = tool
            .execute("call-2", &apply_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!applied.is_error);
        assert_eq!(db.list_skills().unwrap().len(), 1);
    }
}
