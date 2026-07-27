//! ManageSkillTool - controlled skill self-evolution.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::evolution::{CreateSkillChangeProposalInput, SkillChangeAction, SkillProposalStatus};
use crate::execution_environment::{
    LocalProcessExecutionEnvironment, SkillResourceHelperExecutionRequest,
};
use crate::skills::{Skill, SkillResourceEncoding, SkillResourceFile, SkillResourceKind};

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
    resource_path: Option<String>,
    #[serde(default)]
    helper_args: Vec<String>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    resource_bundle: Vec<SkillResourceFile>,
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

fn load_skill_for_action(db: &Database, skill_id: &str) -> Result<Skill, CoreError> {
    let mut skills = crate::skills::load_builtin_skills();
    skills.extend(db.list_skills()?);
    skills
        .into_iter()
        .find(|skill| {
            skill.id == skill_id
                || skill.name == skill_id
                || skill.id.strip_prefix("builtin-") == Some(skill_id)
        })
        .ok_or_else(|| CoreError::NotFound(format!("Skill {skill_id}")))
}

fn find_skill_resource<'a>(
    skill: &'a Skill,
    resource_path: &str,
) -> Result<(&'a SkillResourceFile, String), CoreError> {
    let normalized_path = crate::skills::normalize_skill_resource_path(resource_path)
        .map_err(|err| CoreError::InvalidInput(format!("Invalid skill resource path: {err}")))?;
    let resource = skill
        .resource_bundle
        .iter()
        .find(|resource| resource.path == normalized_path)
        .ok_or_else(|| {
            CoreError::NotFound(format!(
                "Skill resource {} in {}",
                normalized_path, skill.id
            ))
        })?;
    Ok((resource, normalized_path))
}

fn program_for_skill_script(path: &str) -> Result<&'static str, CoreError> {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("py") => Ok("python"),
        Some("js" | "mjs" | "cjs") => Ok("node"),
        _ => Err(CoreError::InvalidInput(format!(
            "Skill resource helper only supports script resources ending in .py, .js, .mjs, or .cjs: {path}"
        ))),
    }
}

fn write_skill_resource_bundle(
    root: &Path,
    resources: &[SkillResourceFile],
) -> Result<(), CoreError> {
    for resource in resources {
        let normalized =
            crate::skills::normalize_skill_resource_path(&resource.path).map_err(|err| {
                CoreError::InvalidInput(format!("Invalid skill resource path: {err}"))
            })?;
        let target = root.join(&normalized);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = match resource.encoding {
            SkillResourceEncoding::Utf8 => resource.content.as_bytes().to_vec(),
            SkillResourceEncoding::Base64 => base64::engine::general_purpose::STANDARD
                .decode(&resource.content)
                .map_err(|err| {
                    CoreError::InvalidInput(format!(
                        "Invalid base64 skill resource {}: {err}",
                        resource.path
                    ))
                })?,
        };
        fs::write(target, bytes)?;
    }
    Ok(())
}

fn helper_temp_dir(skill_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nexa-skill-helper-{}-{}",
        skill_id.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-"),
        uuid::Uuid::new_v4()
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
            .is_some_and(|action| action == "apply_proposal" || action == "run_resource_helper")
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args.get("action")?.as_str()?;
        match action {
            "apply_proposal" => {
                let id = args
                    .get("proposal_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<missing>");
                Some(format!(
                    "Apply skill change proposal {id}. This will create or update an active user skill."
                ))
            }
            "run_resource_helper" => {
                let skill_id = args
                    .get("skill_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<missing>");
                let resource_path = args
                    .get("resource_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<missing>");
                Some(format!(
                    "Run skill resource helper {resource_path} from {skill_id}. This executes a bundled script with network access disabled."
                ))
            }
            _ => None,
        }
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
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
                        resource_bundle: args.resource_bundle,
                        rationale: args.rationale.unwrap_or_default(),
                        conversation_id: None,
                        source: "manual".to_string(),
                        confidence: 0.7,
                        evidence: serde_json::json!([]),
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
                            "shortDescription": skill.interface.short_description,
                            "enabled": skill.enabled,
                            "builtin": skill.builtin,
                            "sourcePath": skill.source_path,
                            "policy": skill.policy,
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
            "view_skill" | "activate_skill" => {
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
                let source_path = skill.source_path.as_deref().unwrap_or("unmaterialized");
                let short_description = if skill.interface.short_description.trim().is_empty() {
                    skill.description.as_str()
                } else {
                    skill.interface.short_description.as_str()
                };
                let content = format!(
                    "Skill: {} ({})\nEnabled: {}\nBuiltin: {}\nSource: {}\nShort description: {}\nDescription: {}\nPolicy: implicit={}\n{}\n\nContent:\n{}",
                    skill.name,
                    skill.id,
                    skill.enabled,
                    skill.builtin,
                    source_path,
                    short_description,
                    skill.description,
                    skill.policy.allow_implicit_invocation,
                    resources,
                    skill.content
                );
                let artifact_kind = if action == "activate_skill" {
                    "skillActivation"
                } else {
                    "skill"
                };
                let artifacts = if action == "activate_skill" {
                    let activation = crate::skills::build_skill_activation_envelope(
                        &skill,
                        Some("manage_skill.activate_skill"),
                        None,
                        true,
                    );
                    serde_json::json!({
                        "kind": artifact_kind,
                        "skill": &skill,
                        "activation": activation
                    })
                } else {
                    serde_json::json!({
                        "kind": artifact_kind,
                        "skill": &skill
                    })
                };
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content,
                    is_error: false,
                    artifacts: Some(artifacts),
                })
            }
            "view_resource" => {
                let skill_id = args.skill_id.ok_or_else(|| missing("skill_id", action))?;
                let resource_path = args
                    .resource_path
                    .ok_or_else(|| missing("resource_path", action))?;
                let skill = load_skill_for_action(db, &skill_id)?;
                let (resource, _) = find_skill_resource(&skill, &resource_path)?;
                let content = match resource.encoding {
                    crate::skills::SkillResourceEncoding::Utf8 => format!(
                        "Skill resource: {} ({})\nKind: {:?}\nEncoding: utf8\n\n{}",
                        resource.path, skill.id, resource.kind, resource.content
                    ),
                    crate::skills::SkillResourceEncoding::Base64 => format!(
                        "Skill resource: {} ({})\nKind: {:?}\nEncoding: base64\n\n{}",
                        resource.path, skill.id, resource.kind, resource.content
                    ),
                };
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content,
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "skillResource",
                        "skillId": &skill.id,
                        "resource": {
                            "path": &resource.path,
                            "kind": &resource.kind,
                            "encoding": &resource.encoding
                        }
                    })),
                })
            }
            "run_resource_helper" => {
                let skill_id = args.skill_id.ok_or_else(|| missing("skill_id", action))?;
                let resource_path = args
                    .resource_path
                    .ok_or_else(|| missing("resource_path", action))?;
                let skill = load_skill_for_action(db, &skill_id)?;
                let (resource, normalized_path) = find_skill_resource(&skill, &resource_path)?;
                let resource_kind = resource.kind.clone();
                let resource_encoding = resource.encoding.clone();
                if resource.kind != SkillResourceKind::Script {
                    return Err(CoreError::InvalidInput(format!(
                        "Skill resource helper can only execute script resources: {} is {:?}",
                        resource.path, resource.kind
                    )));
                }
                if resource.encoding != SkillResourceEncoding::Utf8 {
                    return Err(CoreError::InvalidInput(format!(
                        "Skill resource helper requires utf8 script resources: {}",
                        resource.path
                    )));
                }

                let program = program_for_skill_script(&normalized_path)?;
                let temp_dir = helper_temp_dir(&skill.id);
                fs::create_dir_all(&temp_dir)?;
                let helper_result = async {
                    write_skill_resource_bundle(&temp_dir, &skill.resource_bundle)?;
                    let script_path = temp_dir.join(&normalized_path);
                    let mut helper_args = vec![script_path.display().to_string()];
                    helper_args.extend(args.helper_args.clone());
                    let environment = LocalProcessExecutionEnvironment;
                    environment
                        .execute_skill_resource_helper(SkillResourceHelperExecutionRequest {
                            program: program.to_string(),
                            args: helper_args,
                            cwd: temp_dir.clone(),
                            skill_id: skill.id.clone(),
                            source_scope: source_scope.to_vec(),
                            timeout_secs: args.timeout_secs.unwrap_or(30),
                            stdin: args.stdin.clone(),
                            environment: Vec::new(),
                            expected_writes: Vec::new(),
                        })
                        .await
                }
                .await;
                let cleanup_result = fs::remove_dir_all(&temp_dir);
                let artifact = helper_result?;
                if let Err(err) = cleanup_result {
                    return Err(CoreError::Io(err));
                }
                let status = artifact
                    .exit_status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| {
                        if artifact.timed_out {
                            "timed_out".to_string()
                        } else {
                            "unknown".to_string()
                        }
                    });
                let mut content = format!(
                    "Skill resource helper: {} ({})\nProgram: {}\nExit: {}\n",
                    normalized_path, skill.id, program, status
                );
                if !artifact.stdout.is_empty() {
                    content.push_str("\nstdout:\n");
                    content.push_str(&artifact.stdout);
                }
                if !artifact.stderr.is_empty() {
                    content.push_str("\nstderr:\n");
                    content.push_str(&artifact.stderr);
                }
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content,
                    is_error: artifact.timed_out || artifact.exit_status != Some(0),
                    artifacts: Some(serde_json::json!({
                        "kind": "skillResourceHelperExecution",
                        "skillId": &skill.id,
                        "resource": {
                            "path": normalized_path,
                            "kind": resource_kind,
                            "encoding": resource_encoding
                        },
                        "execution": artifact
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

        let activate_args = serde_json::json!({
            "action": "activate_skill",
            "skill_id": "builtin-pptx-presentation-design"
        });
        let activated = tool
            .execute("call-activate", &activate_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!activated.is_error);
        assert_eq!(
            activated.artifacts.as_ref().unwrap()["kind"],
            "skillActivation"
        );
        assert_eq!(
            activated.artifacts.as_ref().unwrap()["activation"]["skillId"],
            "builtin-pptx-presentation-design"
        );
        assert_eq!(
            activated.artifacts.as_ref().unwrap()["activation"]["version"],
            crate::skills::SKILL_ACTIVATION_ENVELOPE_VERSION
        );

        let resource_args = serde_json::json!({
            "action": "view_resource",
            "skill_id": "builtin-pptx-presentation-design",
            "resource_path": "references/pptx-playbook.md"
        });
        let resource = tool
            .execute("call-resource", &resource_args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!resource.is_error);
        assert_eq!(
            resource.artifacts.as_ref().unwrap()["kind"],
            "skillResource"
        );
        assert!(resource.content.contains("pptx-playbook.md"));

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
    async fn run_resource_helper_executes_script_resource_through_execution_environment() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();
        let saved = db
            .save_skill(&crate::skills::SaveSkillInput {
                id: None,
                name: "Helper skill".into(),
                description: "Runs a bundled helper script.".into(),
                content: "Use scripts/helper.js when helper execution is required.".into(),
                enabled: true,
                resource_bundle: vec![crate::skills::SkillResourceFile {
                    path: "scripts/helper.js".into(),
                    kind: crate::skills::SkillResourceKind::Script,
                    encoding: crate::skills::SkillResourceEncoding::Utf8,
                    content: "const fs = require('fs');\nconst stdin = fs.readFileSync(0, 'utf8').trim();\nconsole.log(`helper:${process.argv.slice(2).join(',')}:${stdin}`);\n".into(),
                }],
            })
            .unwrap();
        let tool = ManageSkillTool;
        let args = serde_json::json!({
            "action": "run_resource_helper",
            "skill_id": saved.id,
            "resource_path": "scripts/helper.js",
            "helper_args": ["alpha"],
            "stdin": "payload"
        });

        assert!(tool.requires_confirmation(&args));
        let result = tool
            .execute("call-helper", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "helper failed: {}", result.content);
        assert!(result.content.contains("helper:alpha:payload"));
        let artifacts = result.artifacts.as_ref().unwrap();
        assert_eq!(artifacts["kind"], "skillResourceHelperExecution");
        assert_eq!(artifacts["resource"]["path"], "scripts/helper.js");
        assert_eq!(artifacts["execution"]["decision"]["kind"], "allowed");
        assert!(artifacts["execution"]["decision"]["permissionKey"]
            .as_str()
            .unwrap()
            .contains("exec:skill_resource_helper:-:"));
        assert!(artifacts["execution"]["decision"]["permissionKey"]
            .as_str()
            .unwrap()
            .ends_with(":node"));
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

    #[tokio::test]
    async fn proposal_preserves_large_skill_body_and_resources() {
        let db = Database::open_memory().unwrap();
        let tool = ManageSkillTool;
        let large_body = format!(
            "# Workflow\n\n{}",
            "Keep this step precise.\n".repeat(16_000)
        );
        assert!(large_body.len() > 256 * 1024);
        let args = serde_json::json!({
            "action": "propose_create",
            "name": "Large reviewed skill",
            "description": "Exercises a complete workflow with bundled references.",
            "content": large_body,
            "resource_bundle": [{
                "path": "references/checklist.md",
                "kind": "reference",
                "encoding": "utf8",
                "content": "# Checklist\n\n- Verify the result.\n"
            }]
        });

        let proposed = tool
            .execute("call-large", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!proposed.is_error, "proposal failed: {}", proposed.content);

        let proposal_id = proposed.artifacts.as_ref().unwrap()["proposal"]["id"]
            .as_str()
            .unwrap();
        let proposal = db.get_skill_change_proposal(proposal_id).unwrap();
        assert!(proposal.content.len() > 256 * 1024);
        assert_eq!(proposal.resource_bundle.len(), 1);
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning.code == "size.too_large"));

        let applied = db.apply_skill_change_proposal(proposal_id).unwrap();
        assert_eq!(applied.skill.resource_bundle.len(), 1);
        assert!(applied.skill.content.len() > 256 * 1024);
    }
}
