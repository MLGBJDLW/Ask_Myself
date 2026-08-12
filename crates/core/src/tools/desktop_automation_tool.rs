//! DesktopAutomationTool — controlled handoff to the user's visible desktop.
//!
//! This tool intentionally avoids raw coordinate/mouse/keyboard automation.
//! It provides narrow, auditable actions for opening or revealing source-scoped
//! files. Interactive HTTP(S) work belongs exclusively to `browser_session` so
//! the Agent and user share Nexa's policy-controlled Browser Workspace.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::execution_environment::{
    ExecutionEnvironment, ExecutionRequest, LocalDetachedProcessExecutionEnvironment,
};

use super::path_utils::{resolve_path_in_sources, PathKind};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/desktop_automation.json");

#[derive(Debug, Deserialize)]
struct DesktopAutomationArgs {
    action: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopAutomationArtifact {
    kind: &'static str,
    action: String,
    target: Option<String>,
    reason: Option<String>,
    launched: bool,
    source_scoped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_environment: Option<&'static str>,
}

pub struct DesktopAutomationTool;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopLaunchCommand {
    program: String,
    args: Vec<String>,
}

impl DesktopLaunchCommand {
    fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    fn into_execution_request(self, source_scope: &[String]) -> ExecutionRequest {
        ExecutionRequest::for_detached_local_tool(
            self.program,
            self.args,
            "desktop_automation",
            source_scope.to_vec(),
            false,
        )
    }
}

fn normalize_nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn launcher_command(target: &str) -> DesktopLaunchCommand {
    #[cfg(windows)]
    {
        DesktopLaunchCommand::new(
            "rundll32.exe",
            vec![
                "url.dll,FileProtocolHandler".to_string(),
                target.to_string(),
            ],
        )
    }
    #[cfg(target_os = "macos")]
    {
        DesktopLaunchCommand::new("open", vec![target.to_string()])
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        DesktopLaunchCommand::new("xdg-open", vec![target.to_string()])
    }
}

fn reveal_command(path: &Path) -> DesktopLaunchCommand {
    #[cfg(windows)]
    {
        DesktopLaunchCommand::new("explorer.exe", vec![format!("/select,{}", path.display())])
    }
    #[cfg(target_os = "macos")]
    {
        DesktopLaunchCommand::new(
            "open",
            vec!["-R".to_string(), path.to_string_lossy().to_string()],
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = path.parent().unwrap_or(path);
        DesktopLaunchCommand::new("xdg-open", vec![parent.to_string_lossy().to_string()])
    }
}

async fn spawn_detached(
    command: DesktopLaunchCommand,
    source_scope: &[String],
) -> Result<(), CoreError> {
    let environment = LocalDetachedProcessExecutionEnvironment;
    let request = command.into_execution_request(source_scope);
    environment.execute(request).await.map(|_| ())
}

fn resolve_source_path(
    db: &Database,
    source_scope: &[String],
    path: &str,
) -> Result<PathBuf, CoreError> {
    let sources = scoped_sources(db, source_scope)?;
    if sources.is_empty() {
        return Err(CoreError::InvalidInput(
            "No source directories are available in the current source scope.".to_string(),
        ));
    }
    resolve_path_in_sources(Path::new(path), &sources, PathKind::Any, false)
        .map_err(CoreError::InvalidInput)
}

fn artifact(
    args: &DesktopAutomationArgs,
    target: Option<String>,
    launched: bool,
    source_scoped: bool,
) -> serde_json::Value {
    serde_json::to_value(DesktopAutomationArtifact {
        kind: "desktopAutomation",
        action: args.action.clone(),
        target,
        reason: args.reason.clone(),
        launched,
        source_scoped,
        execution_environment: launched.then_some("local_detached_process"),
    })
    .unwrap_or_else(|_| serde_json::json!({ "kind": "desktopAutomation" }))
}

#[async_trait]
impl Tool for DesktopAutomationTool {
    fn name(&self) -> &str {
        "desktop_automation"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Automation, ToolCategory::DesktopInteract]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("desktop action");
        let target = args
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("<target not specified>");
        Some(format!(
            "Perform desktop automation action '{action}' for: {target}"
        ))
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
        let mut args: DesktopAutomationArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid desktop_automation arguments: {e}"))
        })?;
        args.action = args.action.trim().to_ascii_lowercase();
        args.path = normalize_nonempty(args.path);
        args.reason = normalize_nonempty(args.reason);

        match args.action.as_str() {
            "open_path" => {
                let path = args.path.as_deref().ok_or_else(|| {
                    CoreError::InvalidInput("open_path requires a non-empty path".to_string())
                })?;
                let canonical = resolve_source_path(db, source_scope, path)?;
                let target = canonical.to_string_lossy().to_string();
                let launch_target = target.clone();
                spawn_detached(launcher_command(&launch_target), source_scope).await?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Opened local path: {}", canonical.display()),
                    is_error: false,
                    artifacts: Some(artifact(&args, Some(target), true, true)),
                })
            }
            "reveal_path" => {
                let path = args.path.as_deref().ok_or_else(|| {
                    CoreError::InvalidInput("reveal_path requires a non-empty path".to_string())
                })?;
                let canonical = resolve_source_path(db, source_scope, path)?;
                let target = canonical.to_string_lossy().to_string();
                let reveal_target = canonical.clone();
                spawn_detached(reveal_command(&reveal_target), source_scope).await?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Revealed local path: {}", canonical.display()),
                    is_error: false,
                    artifacts: Some(artifact(&args, Some(target), true, true)),
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unsupported desktop_automation action '{other}'."
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    #[test]
    fn launcher_command_builds_platform_opener() {
        let command = launcher_command("workspace/note.txt");

        #[cfg(windows)]
        {
            assert_eq!(command.program, "rundll32.exe");
            assert_eq!(
                command.args,
                vec![
                    "url.dll,FileProtocolHandler".to_string(),
                    "workspace/note.txt".to_string()
                ]
            );
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.program, "open");
            assert_eq!(command.args, vec!["workspace/note.txt".to_string()]);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert_eq!(command.program, "xdg-open");
            assert_eq!(command.args, vec!["workspace/note.txt".to_string()]);
        }
    }

    #[test]
    fn reveal_command_builds_platform_opener() {
        let path = Path::new("workspace").join("note.txt");
        let command = reveal_command(&path);

        #[cfg(windows)]
        {
            assert_eq!(command.program, "explorer.exe");
            assert_eq!(command.args.len(), 1);
            assert!(command.args[0].starts_with("/select,"));
            assert!(command.args[0].contains("note.txt"));
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.program, "open");
            assert_eq!(
                command.args,
                vec!["-R".to_string(), path.to_string_lossy().to_string()]
            );
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert_eq!(command.program, "xdg-open");
            assert_eq!(command.args, vec!["workspace".to_string()]);
        }
    }

    #[tokio::test]
    async fn detached_execution_request_carries_desktop_policy() {
        let source_scope = vec!["source-1".to_string()];
        let request = launcher_command("workspace/note.txt").into_execution_request(&source_scope);
        let environment = LocalDetachedProcessExecutionEnvironment;
        let decision = environment.review(&request).await.unwrap();

        assert_eq!(environment.id(), "local_detached_process");
        assert_eq!(
            request.caller.tool_name.as_deref(),
            Some("desktop_automation")
        );
        assert_eq!(
            request.sandbox.backend,
            crate::execution_environment::ExecutionBackendKind::LocalOpen
        );
        assert_eq!(request.sandbox.source_scope, source_scope);
        assert_eq!(
            request.sandbox.allowed_programs,
            vec![request.program.clone()]
        );
        assert!(!request.network_intent);
        assert!(!request.sandbox.network_allowed);
        assert!(!request.sandbox.capture_file_changes);
        assert_eq!(
            decision.kind,
            crate::execution_environment::ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn fixed_wait_action_is_not_advertised() {
        let tool = DesktopAutomationTool;
        let actions = tool.parameters_schema()["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .clone();
        assert!(!actions.iter().any(|action| action == "wait"));
        assert!(!actions.iter().any(|action| action == "open_url"));
        assert!(!actions.iter().any(|action| action == "web_search"));
        assert!(tool.requires_confirmation(&serde_json::json!({
            "action": "open_path"
        })));
    }

    #[test]
    fn path_resolution_respects_source_scope() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "hello").unwrap();
        let db = Database::open_memory().unwrap();
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: false,
            })
            .unwrap();

        let resolved = resolve_source_path(&db, &[source.id], "note.txt").unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());

        let err = resolve_source_path(&db, &["other-source".to_string()], "note.txt").unwrap_err();
        assert!(err.to_string().contains("No source directories"));
    }
}
