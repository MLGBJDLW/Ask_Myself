//! PrepareDocumentToolsTool — checks or prepares the app-managed Office runtime.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::office_runtime;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/prepare_document_tools.json");

pub struct PrepareDocumentToolsTool;

#[derive(Deserialize)]
struct PrepareDocumentToolsArgs {
    action: String,
}

fn app_data_dir_from_db(db: &Database) -> Result<PathBuf, CoreError> {
    if let Some(parent) = db.db_path().and_then(|path| path.parent()) {
        return Ok(parent.to_path_buf());
    }
    let data_dir = dirs::data_dir()
        .ok_or_else(|| CoreError::Internal("Could not resolve app data directory".to_string()))?;
    Ok(data_dir.join(crate::APP_DIR))
}

fn format_readiness(readiness: &office_runtime::OfficeRuntimeReadiness) -> String {
    let mut lines = vec![
        format!("Status: {}", readiness.status),
        readiness.summary.clone(),
        format!("Managed environment: {}", readiness.app_managed_env_path),
    ];
    if let Some(path) = &readiness.python_path {
        lines.push(format!("Python: {path}"));
    }
    lines.push("Dependencies:".to_string());
    for dep in &readiness.dependencies {
        let required = if dep.required { "required" } else { "optional" };
        let version = dep
            .version
            .as_ref()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default();
        lines.push(format!(
            "- {} [{}]: {}{}",
            dep.label, required, dep.status, version
        ));
        if dep.status != "ready" {
            if let Some(detail) = &dep.detail {
                lines.push(format!("  {detail}"));
            }
        }
    }
    lines.join("\n")
}

#[async_trait]
impl Tool for PrepareDocumentToolsTool {
    fn name(&self) -> &str {
        "prepare_document_tools"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core, ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        args.get("action").and_then(|value| value.as_str()) == Some("prepare")
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        if !self.requires_confirmation(args) {
            return None;
        }
        Some("Prepare required Python document tools".to_string())
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: PrepareDocumentToolsArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid prepare_document_tools arguments: {e}"))
        })?;
        let app_data_dir = app_data_dir_from_db(db)?;

        match args.action.as_str() {
            "check" => {
                let readiness = office_runtime::check_office_runtime(&app_data_dir);
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format_readiness(&readiness),
                    is_error: false,
                    artifacts: serde_json::to_value(&readiness).ok(),
                })
            }
            "prepare" => {
                let call_id = call_id.to_string();
                tokio::task::spawn_blocking(move || {
                    let result = office_runtime::prepare_office_runtime(&app_data_dir)?;
                    let actions = result
                        .actions
                        .iter()
                        .map(|action| {
                            let detail = action
                                .detail
                                .as_ref()
                                .map(|value| format!(" — {value}"))
                                .unwrap_or_default();
                            format!("- {}: {}{}", action.name, action.status, detail)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let content = format!(
                        "Prepare result: {}\n\nActions:\n{}\n\n{}",
                        if result.success {
                            "success"
                        } else {
                            "incomplete"
                        },
                        actions,
                        format_readiness(&result.readiness)
                    );
                    Ok(ToolResult {
                        call_id,
                        content,
                        is_error: !result.success,
                        artifacts: serde_json::to_value(&result).ok(),
                    })
                })
                .await
                .map_err(|e| CoreError::Internal(format!("prepare_document_tools task: {e}")))?
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown prepare_document_tools action: {other}"
            ))),
        }
    }
}
