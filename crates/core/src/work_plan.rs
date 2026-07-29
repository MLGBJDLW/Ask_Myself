//! Planner -> executor -> review structure for mutation-heavy work.

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::tools::ToolInvocation;

pub const WORK_PLAN_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlanStage {
    Planner,
    Executor,
    Reviewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlanRisk {
    Low,
    Medium,
    High,
}

impl From<ApprovalRisk> for WorkPlanRisk {
    fn from(value: ApprovalRisk) -> Self {
        match value {
            ApprovalRisk::Low => Self::Low,
            ApprovalRisk::Medium => Self::Medium,
            ApprovalRisk::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkPlanTarget {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkPlanStep {
    pub id: String,
    pub stage: WorkPlanStage,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationWorkPlan {
    pub version: u16,
    pub title: String,
    pub tool_name: String,
    pub operation_kind: String,
    pub risk: WorkPlanRisk,
    pub requires_review: bool,
    pub review_reason: String,
    pub targets: Vec<WorkPlanTarget>,
    pub steps: Vec<WorkPlanStep>,
}

impl MutationWorkPlan {
    pub fn from_tool_invocation(invocation: &ToolInvocation) -> Option<Self> {
        let mutating = invocation.access_profile.can_write || invocation.access_profile.can_execute;
        if !mutating {
            return None;
        }

        let targets = invocation
            .capabilities
            .resource_keys
            .iter()
            .map(|key| {
                let (kind, value) = key
                    .split_once(':')
                    .map(|(kind, value)| (kind.to_string(), value.to_string()))
                    .unwrap_or_else(|| ("resource".to_string(), key.clone()));
                WorkPlanTarget { kind, value }
            })
            .collect::<Vec<_>>();
        let operation_kind = operation_kind_for_tool(&invocation.tool_name);
        let requires_review = invocation.access_profile.needs_approval
            || invocation.access_profile.risk_level != ApprovalRisk::Low
            || invocation.capabilities.destructive;
        let review_reason = if requires_review {
            invocation.access_profile.risk_reason.clone()
        } else {
            "Low-risk mutation can be reviewed from the generated task trace.".to_string()
        };

        Some(Self {
            version: WORK_PLAN_VERSION,
            title: format!("{} via {}", operation_kind, invocation.tool_name),
            tool_name: invocation.tool_name.clone(),
            operation_kind,
            risk: invocation.access_profile.risk_level.into(),
            requires_review,
            review_reason,
            targets,
            steps: vec![
                WorkPlanStep {
                    id: "plan".to_string(),
                    stage: WorkPlanStage::Planner,
                    title: "Describe target changes and expected result".to_string(),
                    status: "queued".to_string(),
                },
                WorkPlanStep {
                    id: "execute".to_string(),
                    stage: WorkPlanStage::Executor,
                    title: "Apply the approved change".to_string(),
                    status: "queued".to_string(),
                },
                WorkPlanStep {
                    id: "review".to_string(),
                    stage: WorkPlanStage::Reviewer,
                    title: "Verify output, artifacts, and rollback notes".to_string(),
                    status: "queued".to_string(),
                },
            ],
        })
    }
}

fn operation_kind_for_tool(tool_name: &str) -> String {
    match tool_name {
        "edit_file" | "multi_edit" => "file edit",
        "create_file" | "write_note" => "file creation",
        "compile_document" => "document compilation",
        "prepare_document_tools" => "document tooling",
        "run_shell" => "command execution",
        "spawn_subagent" | "spawn_subagent_batch" => "delegated work",
        name if name.starts_with("mcp__") => "connector mutation",
        _ => "mutation",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalRisk;
    use crate::tools::{
        ToolAccessProfile, ToolInputStreamingMode, ToolInterruptBehavior, ToolRenderKind,
        ToolRunCapabilities,
    };

    #[test]
    fn mutation_tool_gets_planner_executor_review_steps() {
        let invocation = ToolInvocation {
            call_id: "call-1".to_string(),
            tool_name: "edit_file".to_string(),
            owner: crate::plugins::capability_owner_for_tool("edit_file"),
            arguments: serde_json::json!({ "path": "notes.md" }),
            capabilities: ToolRunCapabilities {
                input_streaming: ToolInputStreamingMode::None,
                render_kind: ToolRenderKind::FileChange,
                read_only: false,
                destructive: true,
                concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::Block,
                resource_keys: vec!["file:notes.md".to_string()],
            },
            access_profile: ToolAccessProfile {
                category: "filesystem".to_string(),
                can_read: true,
                can_write: true,
                can_execute: false,
                can_access_network: false,
                needs_approval: true,
                risk_level: ApprovalRisk::High,
                risk_reason: "modifies a file".to_string(),
            },
            wait_for_previous: false,
        };

        let plan = MutationWorkPlan::from_tool_invocation(&invocation).unwrap();

        assert_eq!(plan.version, WORK_PLAN_VERSION);
        assert!(plan.requires_review);
        assert_eq!(plan.targets[0].value, "notes.md");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].stage, WorkPlanStage::Planner);
        assert_eq!(plan.steps[2].stage, WorkPlanStage::Reviewer);
    }

    #[test]
    fn readonly_tool_does_not_need_mutation_plan() {
        let invocation = ToolInvocation {
            call_id: "call-1".to_string(),
            tool_name: "search_knowledge_base".to_string(),
            owner: crate::plugins::capability_owner_for_tool("search_knowledge_base"),
            arguments: serde_json::json!({ "query": "notes" }),
            capabilities: ToolRunCapabilities {
                input_streaming: ToolInputStreamingMode::None,
                render_kind: ToolRenderKind::Search,
                read_only: true,
                destructive: false,
                concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Block,
                resource_keys: Vec::new(),
            },
            access_profile: ToolAccessProfile {
                category: "knowledge".to_string(),
                can_read: true,
                can_write: false,
                can_execute: false,
                can_access_network: false,
                needs_approval: false,
                risk_level: ApprovalRisk::Low,
                risk_reason: "read only".to_string(),
            },
            wait_for_previous: false,
        };

        assert!(MutationWorkPlan::from_tool_invocation(&invocation).is_none());
    }
}
