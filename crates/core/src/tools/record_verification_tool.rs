//! RecordVerificationTool - stores structured verification outcomes.

use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/record_verification.json");

/// Tool that records what the agent checked before finishing a task.
pub struct RecordVerificationTool;

#[derive(Debug, Deserialize)]
struct RecordVerificationArgs {
    #[serde(default)]
    summary: Option<String>,
    checks: Vec<VerificationCheckInput>,
}

#[derive(Debug, Deserialize)]
struct VerificationCheckInput {
    name: String,
    status: VerificationStatus,
    #[serde(default)]
    details: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum VerificationStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
}

fn status_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Pending => "pending",
        VerificationStatus::Passed => "passed",
        VerificationStatus::Failed => "failed",
        VerificationStatus::Skipped => "skipped",
    }
}

fn overall_status(passed: usize, failed: usize, pending: usize) -> &'static str {
    if failed > 0 {
        "failed"
    } else if passed > 0 && pending == 0 {
        "passed"
    } else if passed > 0 {
        "partial"
    } else {
        "pending"
    }
}

#[async_trait]
impl Tool for RecordVerificationTool {
    fn name(&self) -> &str {
        "record_verification"
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
        let args: RecordVerificationArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid record_verification arguments: {e}"))
        })?;

        if args.checks.is_empty() {
            return Err(CoreError::InvalidInput(
                "record_verification requires at least one check".into(),
            ));
        }
        if args.checks.len() > 12 {
            return Err(CoreError::InvalidInput(
                "record_verification supports at most 12 checks".into(),
            ));
        }

        let summary = args.summary.map(|value| value.trim().to_string());
        let summary = summary.filter(|value| !value.is_empty());

        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut pending = 0usize;
        let mut skipped = 0usize;
        let mut checks = Vec::with_capacity(args.checks.len());

        for (idx, check) in args.checks.into_iter().enumerate() {
            let name = check.name.trim().to_string();
            if name.is_empty() {
                return Err(CoreError::InvalidInput(format!(
                    "record_verification check {} has an empty name",
                    idx + 1
                )));
            }

            match check.status {
                VerificationStatus::Passed => passed += 1,
                VerificationStatus::Failed => failed += 1,
                VerificationStatus::Pending => pending += 1,
                VerificationStatus::Skipped => skipped += 1,
            }

            checks.push(serde_json::json!({
                "name": name,
                "status": status_label(check.status),
                "details": check.details.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            }));
        }

        let overall = overall_status(passed, failed, pending);
        let total_checks = checks.len();
        let artifact = serde_json::json!({
            "kind": "verification",
            "summary": summary,
            "overallStatus": overall,
            "checks": checks,
            "counts": {
                "total": total_checks,
                "passed": passed,
                "failed": failed,
                "pending": pending,
                "skipped": skipped
            },
            "updatedAt": Utc::now().to_rfc3339(),
        });

        let mut content = format!(
            "Verification recorded: {overall}. {passed} passed, {failed} failed, {pending} pending, {skipped} skipped."
        );
        if let Some(summary) = artifact["summary"].as_str() {
            content.push_str(&format!("\nSummary: {summary}"));
        }

        if let Some(checks) = artifact["checks"].as_array() {
            content.push_str("\n\nChecks:");
            for check in checks {
                let name = check["name"].as_str().unwrap_or("Unnamed check");
                let status = check["status"].as_str().unwrap_or("pending");
                content.push_str(&format!("\n- [{status}] {name}"));
                if let Some(details) = check["details"].as_str() {
                    content.push_str(&format!(" -- {details}"));
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
    async fn record_verification_returns_status_summary() {
        let tool = RecordVerificationTool;
        let db = Database::open_memory().unwrap();
        let args = serde_json::json!({
            "summary": "Basic checks finished",
            "checks": [
                { "name": "Plan saved", "status": "passed" },
                { "name": "UI rendered", "status": "pending" }
            ]
        })
        .to_string();

        let result = tool.execute("call-1", &args, &db, &[]).await.unwrap();
        let artifact = result.artifacts.unwrap();

        assert_eq!(artifact["kind"], "verification");
        assert_eq!(artifact["overallStatus"], "partial");
        assert_eq!(artifact["counts"]["passed"], 1);
        assert_eq!(artifact["counts"]["pending"], 1);
    }
}
