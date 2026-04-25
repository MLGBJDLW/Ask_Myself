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
