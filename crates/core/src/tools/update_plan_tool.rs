//! UpdatePlanTool - stores a concise execution plan for the current task.

use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/update_plan.json");

/// Tool that lets the agent expose an explicit step-by-step plan to the UI.
pub struct UpdatePlanTool;

#[derive(Debug, Deserialize)]
struct UpdatePlanArgs {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    explanation: Option<String>,
    steps: Vec<PlanStepInput>,
}

#[derive(Debug, Deserialize)]
struct PlanStepInput {
    #[serde(default)]
    id: Option<String>,
    title: String,
    status: PlanStepStatus,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

fn status_marker(status: PlanStepStatus) -> &'static str {
    match status {
        PlanStepStatus::Pending => "[ ]",
        PlanStepStatus::InProgress => "[-]",
        PlanStepStatus::Completed => "[x]",
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: UpdatePlanArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid update_plan arguments: {e}")))?;

        if args.steps.is_empty() {
            return Err(CoreError::InvalidInput(
                "update_plan requires at least one step".into(),
            ));
        }
        if args.steps.len() > 12 {
            return Err(CoreError::InvalidInput(
                "update_plan supports at most 12 steps".into(),
            ));
        }

        let title = args.title.map(|value| value.trim().to_string());
        let title = title.filter(|value| !value.is_empty());
        let explanation = args.explanation.map(|value| value.trim().to_string());
        let explanation = explanation.filter(|value| !value.is_empty());

        let mut seen_in_progress = false;
        let mut normalized_steps = Vec::with_capacity(args.steps.len());
        let mut completed = 0usize;
        let mut in_progress = 0usize;
        let mut pending = 0usize;

        for (idx, step) in args.steps.into_iter().enumerate() {
            let step_title = step.title.trim().to_string();
            if step_title.is_empty() {
                return Err(CoreError::InvalidInput(format!(
                    "update_plan step {} has an empty title",
                    idx + 1
                )));
            }

            let mut status = step.status;
            if status == PlanStepStatus::InProgress {
                if seen_in_progress {
                    status = PlanStepStatus::Pending;
                } else {
                    seen_in_progress = true;
                }
            }

            match status {
                PlanStepStatus::Completed => completed += 1,
                PlanStepStatus::InProgress => in_progress += 1,
                PlanStepStatus::Pending => pending += 1,
            }

            normalized_steps.push(serde_json::json!({
                "id": step.id.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
                "title": step_title,
                "status": match status {
                    PlanStepStatus::Pending => "pending",
                    PlanStepStatus::InProgress => "in_progress",
                    PlanStepStatus::Completed => "completed",
                },
                "notes": step.notes.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            }));
        }

        let counts = serde_json::json!({
            "total": normalized_steps.len(),
            "completed": completed,
            "inProgress": in_progress,
            "pending": pending,
        });

        let artifact = serde_json::json!({
            "kind": "plan",
            "title": title,
            "explanation": explanation,
            "steps": normalized_steps,
            "counts": counts,
            "updatedAt": Utc::now().to_rfc3339(),
        });

        let mut content = format!(
            "Plan updated: {completed}/{} completed.",
            artifact["steps"].as_array().map_or(0, |steps| steps.len())
        );
        if in_progress > 0 {
            content.push_str(&format!(" {in_progress} in progress."));
        }
        if pending > 0 {
            content.push_str(&format!(" {pending} pending."));
        }

        if let Some(title) = artifact["title"].as_str() {
            content.push_str(&format!("\nTitle: {title}"));
        }
        if let Some(explanation) = artifact["explanation"].as_str() {
            content.push_str(&format!("\nExplanation: {explanation}"));
        }

        if let Some(steps) = artifact["steps"].as_array() {
            content.push_str("\n\nSteps:");
            for step in steps {
                let status = match step["status"].as_str() {
                    Some("completed") => PlanStepStatus::Completed,
                    Some("in_progress") => PlanStepStatus::InProgress,
                    _ => PlanStepStatus::Pending,
                };
                let title = step["title"].as_str().unwrap_or("Untitled step");
                content.push_str(&format!("\n- {} {}", status_marker(status), title));
                if let Some(notes) = step["notes"].as_str() {
                    content.push_str(&format!(" -- {notes}"));
                }
            }
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(artifact),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn update_plan_returns_structured_artifact() {
        let tool = UpdatePlanTool;
        let db = Database::open_memory().unwrap();
        let args = serde_json::json!({
            "title": "Ship task board",
            "steps": [
                { "title": "Inspect codebase", "status": "completed" },
                { "title": "Implement backend", "status": "in_progress" },
                { "title": "Add UI", "status": "pending" }
            ]
        })
        .to_string();

        let result = tool.execute("call-1", &args, &db, &[]).await.unwrap();
        let artifact = result.artifacts.unwrap();

        assert_eq!(artifact["kind"], "plan");
        assert_eq!(artifact["counts"]["completed"], 1);
        assert_eq!(artifact["counts"]["inProgress"], 1);
        assert_eq!(artifact["counts"]["pending"], 1);
    }
}
