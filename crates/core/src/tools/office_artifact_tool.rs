//! OfficeArtifactTool — transactional DOCX/XLSX/PPTX candidate lifecycle.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::db::Database;
use crate::error::CoreError;
use crate::office_runtime;

use super::path_utils::resolve_existing_directory_for_file_access;
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/office_artifact.json");

pub struct OfficeArtifactTool;

#[derive(Debug, Deserialize)]
struct OfficeArtifactArgs {
    action: String,
    workspace_root: String,
    #[serde(default)]
    request: Option<Value>,
    #[serde(default)]
    candidate_id: Option<String>,
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    receipt_id: Option<String>,
}

fn app_data_dir_from_db(db: &Database) -> Result<PathBuf, CoreError> {
    if let Some(parent) = db.db_path().and_then(|path| path.parent()) {
        return Ok(parent.to_path_buf());
    }
    let data_dir = dirs::data_dir()
        .ok_or_else(|| CoreError::Internal("Could not resolve app data directory".to_string()))?;
    Ok(data_dir.join(crate::APP_DIR))
}

fn engine_arguments(args: &OfficeArtifactArgs) -> Result<(Vec<String>, String), CoreError> {
    let action = args.action.trim().to_ascii_lowercase();
    let mut command = vec!["--action".to_string(), action.clone()];
    let mut request_json = String::new();
    match action.as_str() {
        "capabilities" => {}
        "assess" | "execute" => {
            let request = args.request.as_ref().ok_or_else(|| {
                CoreError::InvalidInput(format!("office_artifact {action} requires request"))
            })?;
            request_json = serde_json::to_string(request).map_err(|error| {
                CoreError::InvalidInput(format!("Invalid Office artifact request: {error}"))
            })?;
            command.extend(["--request".to_string(), "-".to_string()]);
        }
        "decide" => {
            let candidate_id = args.candidate_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact decide requires candidate_id".to_string())
            })?;
            let decision = args.decision.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact decide requires decision".to_string())
            })?;
            command.extend([
                "--candidate-id".to_string(),
                candidate_id.to_string(),
                "--decision".to_string(),
                decision.to_string(),
            ]);
        }
        "restore" => {
            let receipt_id = args.receipt_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact restore requires receipt_id".to_string())
            })?;
            command.extend(["--receipt-id".to_string(), receipt_id.to_string()]);
        }
        other => {
            return Err(CoreError::InvalidInput(format!(
                "Unknown office_artifact action: {other}"
            )))
        }
    }
    Ok((command, request_json))
}

fn resolve_workspace(
    requested: &str,
    db: &Database,
    source_scope: &[String],
) -> Result<PathBuf, CoreError> {
    let policy = file_access_policy(db, source_scope)?;
    resolve_existing_directory_for_file_access(
        Path::new(requested),
        &policy.sources,
        policy.allow_unregistered_absolute_paths,
    )
    .map_err(CoreError::InvalidInput)
}

#[async_trait]
impl Tool for OfficeArtifactTool {
    fn name(&self) -> &str {
        "office_artifact"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem, ToolCategory::Process]
    }

    fn requires_confirmation(&self, args: &Value) -> bool {
        matches!(
            args.get("action").and_then(Value::as_str),
            Some("execute" | "decide" | "restore")
        )
    }

    fn confirmation_message(&self, args: &Value) -> Option<String> {
        self.requires_confirmation(args).then(|| {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("update");
            format!("Run transactional Office artifact action: {action}")
        })
    }

    fn is_read_only(&self, args: &Value) -> bool {
        matches!(
            args.get("action").and_then(Value::as_str),
            Some("capabilities" | "assess")
        )
    }

    fn resource_keys(&self, args: &Value) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(root) = args.get("workspace_root").and_then(Value::as_str) {
            keys.push(format!("file:{}", root.trim().replace('\\', "/")));
        }
        if let Some(request) = args.get("request") {
            for field in ["source", "destination"] {
                if let Some(path) = request.get(field).and_then(Value::as_str) {
                    keys.push(format!("file:{}", path.trim().replace('\\', "/")));
                }
            }
        }
        if let Some(id) = args
            .get("candidate_id")
            .or_else(|| args.get("receipt_id"))
            .and_then(Value::as_str)
        {
            keys.push(format!("office-artifact:{id}"));
        }
        keys.sort();
        keys.dedup();
        keys
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let args: OfficeArtifactArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid office_artifact arguments: {error}"))
        })?;
        let workspace = resolve_workspace(&args.workspace_root, db, source_scope)?;
        let app_data = app_data_dir_from_db(db)?;
        let (engine_args, request_json) = engine_arguments(&args)?;
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            let execution = office_runtime::execute_office_artifact_engine(
                &app_data,
                &workspace,
                &engine_args,
                &request_json,
            )?;
            let artifacts = serde_json::from_str::<Value>(&execution.stdout)
                .ok()
                .or_else(|| serde_json::to_value(&execution).ok());
            let content = if execution.stdout.is_empty() {
                execution.stderr.clone()
            } else if execution.stderr.is_empty() {
                execution.stdout.clone()
            } else {
                format!("{}\n\nDiagnostics:\n{}", execution.stdout, execution.stderr)
            };
            Ok(ToolResult {
                call_id,
                content,
                is_error: !execution.success,
                artifacts,
            })
        })
        .await
        .map_err(|error| CoreError::Internal(format!("office_artifact task: {error}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_definition_exposes_candidate_lifecycle() {
        let tool = OfficeArtifactTool;
        let schema = tool.parameters_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.contains(&json!("execute")));
        assert!(actions.contains(&json!("decide")));
        assert!(actions.contains(&json!("restore")));
    }

    #[test]
    fn lifecycle_actions_have_correct_mutability() {
        let tool = OfficeArtifactTool;
        assert!(tool.is_read_only(&json!({"action": "assess"})));
        assert!(!tool.requires_confirmation(&json!({"action": "capabilities"})));
        assert!(tool.requires_confirmation(&json!({"action": "execute"})));
        assert!(tool.requires_confirmation(&json!({"action": "decide"})));
    }

    #[test]
    fn command_builder_requires_action_specific_identifiers() {
        let error = engine_arguments(&OfficeArtifactArgs {
            action: "decide".to_string(),
            workspace_root: ".".to_string(),
            request: None,
            candidate_id: None,
            decision: None,
            receipt_id: None,
        })
        .unwrap_err();
        assert!(error.to_string().contains("candidate_id"));
    }
}
