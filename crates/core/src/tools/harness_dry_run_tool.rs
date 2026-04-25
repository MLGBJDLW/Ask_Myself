//! HarnessDryRunTool - read-only readiness report for the local agent harness.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/agent_harness_dry_run.json");

pub struct HarnessDryRunTool;

#[derive(Debug, Deserialize)]
struct HarnessDryRunArgs {
    #[serde(default)]
    include_recent_events: Option<bool>,
    #[serde(default)]
    event_limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessCheck {
    name: String,
    status: String,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessCounts {
    agent_configs: usize,
    builtin_skills: usize,
    user_skills: usize,
    procedural_memories: usize,
    pending_skill_proposals: usize,
    open_evolution_events: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessDryRunReport {
    status: String,
    checks: Vec<HarnessCheck>,
    next_actions: Vec<String>,
    counts: HarnessCounts,
    trace_summary: serde_json::Value,
    recent_events: Option<serde_json::Value>,
}

fn check(name: &str, status: &str, detail: impl Into<String>) -> HarnessCheck {
    HarnessCheck {
        name: name.to_string(),
        status: status.to_string(),
        detail: detail.into(),
    }
}

#[async_trait]
impl Tool for HarnessDryRunTool {
    fn name(&self) -> &str {
        "agent_harness_dry_run"
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

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: HarnessDryRunArgs = if arguments.trim().is_empty() {
            HarnessDryRunArgs {
                include_recent_events: Some(true),
                event_limit: Some(5),
            }
        } else {
            serde_json::from_str(arguments).map_err(|e| {
                CoreError::InvalidInput(format!("Invalid agent_harness_dry_run arguments: {e}"))
            })?
        };

        let configs = db.list_agent_configs()?;
        let default_config = db.get_default_agent_config()?;
        let user_skills = db.list_skills()?;
        let builtin_skills = crate::skills::load_builtin_skills();
        let memories = db.list_agent_procedural_memories(100)?;
        let pending_proposals = db.list_skill_change_proposals(
            Some(crate::evolution::SkillProposalStatus::Pending),
            100,
        )?;
        let trace_summary = db.get_trace_summary()?;
        let open_events = db.list_agent_evolution_events(Some("open"), 100)?;

        let mut checks = Vec::new();
        let mut next_actions = Vec::new();

        if default_config.is_some() {
            checks.push(check(
                "default_model",
                "ready",
                "Default agent config is available.",
            ));
        } else {
            checks.push(check(
                "default_model",
                "blocked",
                "No default agent config is configured.",
            ));
            next_actions
                .push("Configure a default agent model before running agent tasks.".to_string());
        }

        checks.push(check(
            "skills",
            "ready",
            format!(
                "{} built-in skill(s), {} user skill(s).",
                builtin_skills.len(),
                user_skills.len()
            ),
        ));

        if memories.is_empty() {
            checks.push(check(
                "procedural_memory",
                "warning",
                "No agent procedural memories have been recorded yet.",
            ));
            next_actions.push(
                "After repeated tool/workflow lessons, record a procedural memory.".to_string(),
            );
        } else {
            checks.push(check(
                "procedural_memory",
                "ready",
                format!("{} procedural memory item(s) available.", memories.len()),
            ));
        }

        if pending_proposals.is_empty() {
            checks.push(check(
                "skill_proposals",
                "ready",
                "No pending skill proposals.",
            ));
        } else {
            checks.push(check(
                "skill_proposals",
                "warning",
                format!(
                    "{} pending skill proposal(s) need review.",
                    pending_proposals.len()
                ),
            ));
            next_actions.push("Review pending skill proposals before applying them.".to_string());
        }

        if trace_summary.total_sessions > 0 && trace_summary.success_rate < 0.75 {
            checks.push(check(
                "trace_success_rate",
                "warning",
                format!(
                    "Trace success rate is {:.0}%.",
                    trace_summary.success_rate * 100.0
                ),
            ));
            next_actions.push(
                "Inspect recent trace failures and convert repeated fixes into a skill or memory."
                    .to_string(),
            );
        } else {
            checks.push(check(
                "trace_success_rate",
                "ready",
                format!(
                    "{} trace session(s), success rate {:.0}%.",
                    trace_summary.total_sessions,
                    trace_summary.success_rate * 100.0
                ),
            ));
        }

        if !open_events.is_empty() {
            checks.push(check(
                "evolution_events",
                "warning",
                format!("{} open evolution event(s).", open_events.len()),
            ));
        }

        let status = if checks.iter().any(|c| c.status == "blocked") {
            "blocked"
        } else if checks.iter().any(|c| c.status == "warning") {
            "warning"
        } else {
            "ready"
        }
        .to_string();

        let recent_events = if args.include_recent_events.unwrap_or(true) {
            let events = db.list_agent_evolution_events(None, args.event_limit.unwrap_or(5))?;
            Some(serde_json::to_value(events)?)
        } else {
            None
        };

        let report = HarnessDryRunReport {
            status,
            checks,
            next_actions,
            counts: HarnessCounts {
                agent_configs: configs.len(),
                builtin_skills: builtin_skills.len(),
                user_skills: user_skills.len(),
                procedural_memories: memories.len(),
                pending_skill_proposals: pending_proposals.len(),
                open_evolution_events: open_events.len(),
            },
            trace_summary: serde_json::to_value(trace_summary)?,
            recent_events,
        };

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("Agent harness dry-run status: {}", report.status),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "agentHarnessDryRun",
                "report": report
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_reports_blocked_without_default_config() {
        let db = Database::open_memory().unwrap();
        let tool = HarnessDryRunTool;
        let result = tool.execute("call-1", "{}", &db, &[]).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("blocked"));
    }
}
