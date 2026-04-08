//! HealthCheckTool — run knowledge base health diagnostics.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/run_health_check.json");

#[derive(Deserialize)]
struct HealthCheckArgs {
    #[serde(default = "default_check_type")]
    check_type: String,
    #[serde(default = "default_stale_days")]
    stale_days: u32,
}

fn default_check_type() -> String {
    "all".to_string()
}

fn default_stale_days() -> u32 {
    90
}

pub struct HealthCheckTool;

#[async_trait]
impl Tool for HealthCheckTool {
    fn name(&self) -> &str {
        "run_health_check"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Knowledge]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: HealthCheckArgs = serde_json::from_str(arguments).unwrap_or(HealthCheckArgs {
            check_type: default_check_type(),
            stale_days: default_stale_days(),
        });

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            let report = db.run_health_check(args.stale_days)?;

            let format_issues = |issues: &[crate::lint::HealthIssue], label: &str| -> String {
                if issues.is_empty() {
                    format!("### {label}\nNo issues found.\n")
                } else {
                    let items: Vec<String> = issues
                        .iter()
                        .map(|i| {
                            format!(
                                "- [{:?}] {}\n  → {}",
                                i.severity, i.description, i.suggestion,
                            )
                        })
                        .collect();
                    format!(
                        "### {label} ({} issues)\n{}\n",
                        issues.len(),
                        items.join("\n")
                    )
                }
            };

            let content = match args.check_type.as_str() {
                "stale" => format_issues(&report.stale_documents, "Stale Documents"),
                "orphan" => format_issues(&report.orphan_documents, "Orphan Documents"),
                "duplicate" => format_issues(&report.duplicate_candidates, "Duplicate Candidates"),
                "gap" => format_issues(&report.low_coverage_entities, "Low Coverage Entities"),
                _ => {
                    // "all"
                    let mut sections = Vec::new();
                    sections.push(format!(
                        "**Knowledge Base Health Report** — {} total issues\n",
                        report.total_issues,
                    ));
                    sections.push(format_issues(&report.stale_documents, "Stale Documents"));
                    sections.push(format_issues(&report.orphan_documents, "Orphan Documents"));
                    sections.push(format_issues(
                        &report.duplicate_candidates,
                        "Duplicate Candidates",
                    ));
                    sections.push(format_issues(
                        &report.low_coverage_entities,
                        "Low Coverage Entities",
                    ));
                    sections.join("\n")
                }
            };

            Ok(ToolResult {
                call_id,
                content,
                is_error: false,
                artifacts: None,
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}
