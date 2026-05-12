//! ProjectTool - discover and run source-scoped project-local tool manifests.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::Database;
use crate::error::CoreError;

use super::{
    file_access_policy, scope_is_active, Tool, ToolCategory, ToolDef, ToolOutput, ToolResult,
    TrustBoundary,
};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/project_tool.json");

const MANIFEST_DIRS: [&str; 2] = [".nexa/tools", ".agents/tools"];
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 1_800;
const MAX_OUTPUT_CHARS: usize = 16_000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectToolAction {
    List,
    Describe,
    Run,
}

#[derive(Debug, Deserialize)]
struct ProjectToolArgs {
    action: ProjectToolAction,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "empty_object")]
    arguments: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolManifest {
    name: String,
    description: String,
    #[serde(default = "default_parameters")]
    parameters: Value,
    #[serde(default)]
    command: Option<ProjectToolCommand>,
    #[serde(default)]
    access: ProjectToolAccess,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolCommand {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, alias = "timeout_secs")]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolAccess {
    #[serde(default = "default_true")]
    read: bool,
    #[serde(default)]
    write: bool,
    #[serde(default = "default_true")]
    execute: bool,
    #[serde(default)]
    network: bool,
}

impl Default for ProjectToolAccess {
    fn default() -> Self {
        Self {
            read: true,
            write: false,
            execute: true,
            network: false,
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectToolRecord {
    manifest: ProjectToolManifest,
    manifest_path: PathBuf,
    source_root: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolSummary {
    name: String,
    description: String,
    manifest_path: String,
    source_root: String,
    runnable: bool,
    access: ProjectToolAccess,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolManifestError {
    path: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectToolCatalog {
    kind: &'static str,
    tools: Vec<ProjectToolSummary>,
    errors: Vec<ProjectToolManifestError>,
}

pub struct ProjectTool;

#[async_trait]
impl Tool for ProjectTool {
    fn name(&self) -> &str {
        "project_tool"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        action_from_value(args).is_some_and(|action| matches!(action, ProjectToolAction::Run))
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        if !self.requires_confirmation(args) {
            return None;
        }
        let name = args
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<missing>");
        Some(format!(
            "Run project-local tool '{name}' from a source-scoped manifest."
        ))
    }

    fn is_read_only(&self, args: &serde_json::Value) -> bool {
        !self.requires_confirmation(args)
    }

    fn is_concurrency_safe(&self, args: &serde_json::Value) -> bool {
        !self.requires_confirmation(args)
    }

    fn resource_keys(&self, args: &serde_json::Value) -> Vec<String> {
        args.get("name")
            .and_then(|value| value.as_str())
            .map(|name| vec![format!("project-tool:{name}")])
            .unwrap_or_default()
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ProjectToolArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid project_tool arguments: {e}")))?;
        match args.action {
            ProjectToolAction::List => list_project_tools(call_id, db, source_scope),
            ProjectToolAction::Describe => describe_project_tool(call_id, db, source_scope, &args),
            ProjectToolAction::Run => run_project_tool(call_id, db, source_scope, args).await,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {}
    })
}

fn empty_object() -> Value {
    json!({})
}

fn action_from_value(args: &serde_json::Value) -> Option<ProjectToolAction> {
    match args.get("action").and_then(|value| value.as_str()) {
        Some("list") => Some(ProjectToolAction::List),
        Some("describe") => Some(ProjectToolAction::Describe),
        Some("run") => Some(ProjectToolAction::Run),
        _ => None,
    }
}

fn list_project_tools(
    call_id: &str,
    db: &Database,
    source_scope: &[String],
) -> Result<ToolResult, CoreError> {
    let (records, errors) = discover_project_tools(db, source_scope)?;
    let catalog = ProjectToolCatalog {
        kind: "projectToolCatalog",
        tools: records.iter().map(tool_summary).collect(),
        errors,
    };
    let output = ToolOutput {
        llm_content: format_project_tool_catalog(&catalog),
        display_content: format_project_tool_catalog(&catalog),
        data: Some(serde_json::to_value(&catalog)?),
        artifacts: Some(json!({
            "trustBoundary": TrustBoundary::local_source_evidence(scope_is_active(source_scope)),
            "manifestDirs": MANIFEST_DIRS,
        })),
        attachments: Vec::new(),
    };
    Ok(ToolResult::from_output(call_id, false, output))
}

fn describe_project_tool(
    call_id: &str,
    db: &Database,
    source_scope: &[String],
    args: &ProjectToolArgs,
) -> Result<ToolResult, CoreError> {
    let name = required_tool_name(args)?;
    let record = find_unique_project_tool(db, source_scope, name)?;
    let manifest_name = record.manifest.name.clone();
    let description = record.manifest.description.clone();
    let parameters = record.manifest.parameters.clone();
    let command = record.manifest.command.clone();
    let access = record.manifest.access.clone();
    let data = json!({
        "kind": "projectToolManifest",
        "name": manifest_name,
        "description": description,
        "parameters": parameters,
        "command": command,
        "access": access,
        "manifestPath": display_path(&record.manifest_path),
        "sourceRoot": display_path(&record.source_root),
    });
    let runnable = record.manifest.command.is_some();
    let content = format!(
        "Project tool '{}' ({})\n{}\nManifest: {}\nRunnable: {}",
        record.manifest.name,
        if record.manifest.access.write {
            "write-capable"
        } else {
            "read/execute"
        },
        record.manifest.description,
        display_path(&record.manifest_path),
        runnable
    );
    let output = ToolOutput {
        llm_content: content.clone(),
        display_content: content,
        data: Some(data),
        artifacts: Some(json!({
            "trustBoundary": TrustBoundary::local_source_evidence(scope_is_active(source_scope)),
        })),
        attachments: Vec::new(),
    };
    Ok(ToolResult::from_output(call_id, false, output))
}

async fn run_project_tool(
    call_id: &str,
    db: &Database,
    source_scope: &[String],
    args: ProjectToolArgs,
) -> Result<ToolResult, CoreError> {
    let name = required_tool_name(&args)?;
    let record = find_unique_project_tool(db, source_scope, name)?;
    let command = record.manifest.command.as_ref().ok_or_else(|| {
        CoreError::InvalidInput(format!("Project tool '{name}' does not define a command."))
    })?;
    let command_args = expand_command_args(&command.args, &args.arguments)?;
    let cwd = resolve_command_cwd(&record.source_root, command.cwd.as_deref())?;
    let timeout_secs = command
        .timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);

    let mut process = tokio::process::Command::new(&command.program);
    process
        .args(&command_args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output =
        match tokio::time::timeout(Duration::from_secs(timeout_secs), process.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Err(CoreError::Io(err));
            }
            Err(_) => {
                let output = ToolOutput::text(format!(
                    "Project tool '{name}' timed out after {timeout_secs}s."
                ));
                return Ok(ToolResult::from_output(call_id, true, output));
            }
        };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let (stdout, stdout_truncated) = truncate_output(stdout);
    let (stderr, stderr_truncated) = truncate_output(stderr);
    let exit_code = output.status.code();
    let success = output.status.success();
    let command_display = format_command_display(&command.program, &command_args);
    let content = format_project_tool_run(
        name,
        &command_display,
        exit_code,
        success,
        &stdout,
        &stderr,
        stdout_truncated || stderr_truncated,
    );
    let data = json!({
        "kind": "projectToolRun",
        "name": name,
        "command": {
            "program": command.program.clone(),
            "args": command_args,
            "cwd": display_path(&cwd),
            "timeoutSecs": timeout_secs,
        },
        "exitCode": exit_code,
        "success": success,
        "stdout": stdout,
        "stderr": stderr,
        "stdoutTruncated": stdout_truncated,
        "stderrTruncated": stderr_truncated,
        "manifestPath": display_path(&record.manifest_path),
        "sourceRoot": display_path(&record.source_root),
    });
    let output = ToolOutput {
        llm_content: content.clone(),
        display_content: content,
        data: Some(data),
        artifacts: Some(json!({
            "trustBoundary": TrustBoundary::local_source_evidence(scope_is_active(source_scope)),
            "manifestAccess": record.manifest.access.clone(),
        })),
        attachments: Vec::new(),
    };
    Ok(ToolResult::from_output(call_id, !success, output))
}

fn required_tool_name(args: &ProjectToolArgs) -> Result<&str, CoreError> {
    args.name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CoreError::InvalidInput("project_tool action requires 'name'.".to_string()))
}

fn discover_project_tools(
    db: &Database,
    source_scope: &[String],
) -> Result<(Vec<ProjectToolRecord>, Vec<ProjectToolManifestError>), CoreError> {
    let file_policy = file_access_policy(db, source_scope)?;
    let mut records = Vec::new();
    let mut errors = Vec::new();

    for source in file_policy.sources {
        let Ok(source_root) = std::fs::canonicalize(&source.root_path) else {
            continue;
        };
        for manifest_dir in MANIFEST_DIRS {
            let dir = source_root.join(manifest_dir);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match read_manifest(&path) {
                    Ok(manifest) => match validate_manifest(&manifest) {
                        Ok(()) => records.push(ProjectToolRecord {
                            manifest,
                            manifest_path: path,
                            source_root: source_root.clone(),
                        }),
                        Err(message) => errors.push(ProjectToolManifestError {
                            path: display_path(&path),
                            message,
                        }),
                    },
                    Err(message) => errors.push(ProjectToolManifestError {
                        path: display_path(&path),
                        message,
                    }),
                }
            }
        }
    }

    records.sort_by(|left, right| {
        left.manifest
            .name
            .cmp(&right.manifest.name)
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
    });
    Ok((records, errors))
}

fn find_unique_project_tool(
    db: &Database,
    source_scope: &[String],
    name: &str,
) -> Result<ProjectToolRecord, CoreError> {
    let (records, _) = discover_project_tools(db, source_scope)?;
    let matches = records
        .into_iter()
        .filter(|record| record.manifest.name == name)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Err(CoreError::NotFound(format!(
            "Project tool '{name}' was not found in .nexa/tools or .agents/tools manifests."
        ))),
        1 => Ok(matches.into_iter().next().expect("one record")),
        _ => Err(CoreError::InvalidInput(format!(
            "Project tool name '{name}' is ambiguous across source manifests."
        ))),
    }
}

fn read_manifest(path: &Path) -> Result<ProjectToolManifest, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read project tool manifest: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Invalid project tool manifest JSON: {e}"))
}

fn validate_manifest(manifest: &ProjectToolManifest) -> Result<(), String> {
    if !is_valid_project_tool_name(&manifest.name) {
        return Err(
            "Tool name must be 1-80 ASCII letters, digits, underscore, dash, dot, or colon."
                .to_string(),
        );
    }
    if manifest.description.trim().is_empty() {
        return Err("Tool description must not be empty.".to_string());
    }
    if let Some(command) = &manifest.command {
        if command.program.trim().is_empty() {
            return Err("Command program must not be empty.".to_string());
        }
        if command.program.contains('/') || command.program.contains('\\') {
            return Err("Command program must be a program name, not a path.".to_string());
        }
    }
    Ok(())
}

fn is_valid_project_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
}

fn resolve_command_cwd(source_root: &Path, cwd: Option<&str>) -> Result<PathBuf, CoreError> {
    let raw = cwd.unwrap_or(".");
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(CoreError::InvalidInput(
            "Project tool command cwd must be relative to the source root.".to_string(),
        ));
    }
    let resolved = std::fs::canonicalize(source_root.join(path))?;
    if !resolved.starts_with(source_root) {
        return Err(CoreError::InvalidInput(
            "Project tool command cwd must stay within the source root.".to_string(),
        ));
    }
    if !resolved.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Project tool command cwd is not a directory: {}",
            display_path(&resolved)
        )));
    }
    Ok(resolved)
}

fn expand_command_args(templates: &[String], values: &Value) -> Result<Vec<String>, CoreError> {
    let values = values.as_object().ok_or_else(|| {
        CoreError::InvalidInput("project_tool 'arguments' must be a JSON object.".to_string())
    })?;
    templates
        .iter()
        .map(|template| expand_arg_template(template, values))
        .collect()
}

fn expand_arg_template(
    template: &str,
    values: &serde_json::Map<String, Value>,
) -> Result<String, CoreError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let (prefix, after_start) = rest.split_at(start);
        output.push_str(prefix);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            return Err(CoreError::InvalidInput(format!(
                "Unclosed placeholder in project tool arg template: {template}"
            )));
        };
        let key = after_start[..end].trim();
        let value = values.get(key).ok_or_else(|| {
            CoreError::InvalidInput(format!("Missing project tool argument '{key}'."))
        })?;
        output.push_str(&json_scalar_to_arg(key, value)?);
        rest = &after_start[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

fn json_scalar_to_arg(key: &str, value: &Value) -> Result<String, CoreError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(CoreError::InvalidInput(format!(
            "Project tool argument '{key}' must be a string, number, boolean, or null."
        ))),
    }
}

fn tool_summary(record: &ProjectToolRecord) -> ProjectToolSummary {
    ProjectToolSummary {
        name: record.manifest.name.clone(),
        description: record.manifest.description.clone(),
        manifest_path: display_path(&record.manifest_path),
        source_root: display_path(&record.source_root),
        runnable: record.manifest.command.is_some(),
        access: record.manifest.access.clone(),
    }
}

fn format_project_tool_catalog(catalog: &ProjectToolCatalog) -> String {
    let mut text = format!("Found {} project tool manifest(s).", catalog.tools.len());
    if !catalog.errors.is_empty() {
        text.push_str(&format!(
            " {} manifest(s) had validation errors.",
            catalog.errors.len()
        ));
    }
    for tool in &catalog.tools {
        text.push_str(&format!(
            "\n{} - {} ({})",
            tool.name,
            tool.description,
            if tool.runnable {
                "runnable"
            } else {
                "metadata only"
            }
        ));
    }
    for error in &catalog.errors {
        text.push_str(&format!(
            "\nInvalid manifest {}: {}",
            error.path, error.message
        ));
    }
    text
}

fn format_project_tool_run(
    name: &str,
    command_display: &str,
    exit_code: Option<i32>,
    success: bool,
    stdout: &str,
    stderr: &str,
    truncated: bool,
) -> String {
    let status = if success { "succeeded" } else { "failed" };
    let mut text = format!(
        "Project tool '{name}' {status} with exit code {}.\nCommand: {command_display}",
        exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "<signal>".to_string())
    );
    if !stdout.is_empty() {
        text.push_str(&format!("\n\nstdout:\n{stdout}"));
    }
    if !stderr.is_empty() {
        text.push_str(&format!("\n\nstderr:\n{stderr}"));
    }
    if truncated {
        text.push_str("\n\nOutput was truncated.");
    }
    text
}

fn format_command_display(program: &str, args: &[String]) -> String {
    let mut parts = vec![program.to_string()];
    parts.extend(args.iter().map(|arg| {
        if arg.contains(char::is_whitespace) {
            format!("{arg:?}")
        } else {
            arg.clone()
        }
    }));
    parts.join(" ")
}

fn truncate_output(text: String) -> (String, bool) {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return (text, false);
    }
    let truncated = text.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    (format!("{truncated}\n...[truncated]"), true)
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    fn setup_db_with_source(root: &Path) -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("register source root");
        db
    }

    fn write_manifest(root: &Path, name: &str, body: &str) {
        let dir = root.join(".nexa").join("tools");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), body).unwrap();
    }

    #[tokio::test]
    async fn lists_and_describes_project_tool_manifests() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "lint",
            r#"{
                "name": "lint",
                "description": "Run the project lint check",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    }
                },
                "command": {
                    "program": "npm",
                    "args": ["run", "lint", "--", "{{path}}"],
                    "cwd": ".",
                    "timeoutSecs": 60
                },
                "access": { "read": true, "write": false, "execute": true, "network": false }
            }"#,
        );

        let db = setup_db_with_source(dir.path());
        let tool = ProjectTool;
        let list_args = json!({ "action": "list" });
        let list_result = tool
            .execute("project-tools-1", &list_args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(
            !list_result.is_error,
            "unexpected error: {}",
            list_result.content
        );
        assert!(list_result.llm_context_content().contains("lint"));
        assert_eq!(
            list_result.artifacts.as_ref().unwrap()["data"]["tools"][0]["name"],
            "lint"
        );

        let describe_args = json!({ "action": "describe", "name": "lint" });
        let describe_result = tool
            .execute("project-tools-2", &describe_args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!describe_result.is_error);
        assert_eq!(
            describe_result.artifacts.as_ref().unwrap()["data"]["kind"],
            "projectToolManifest"
        );
    }

    #[test]
    fn expands_command_args_without_shell_interpolation() {
        let templates = vec![
            "run".to_string(),
            "{{path}}".to_string(),
            "--fix={{fix}}".to_string(),
        ];
        let values = json!({ "path": "src/main.rs", "fix": false });

        let expanded = expand_command_args(&templates, &values).unwrap();

        assert_eq!(expanded, vec!["run", "src/main.rs", "--fix=false"]);
    }

    #[test]
    fn rejects_missing_or_non_scalar_command_args() {
        let missing = expand_command_args(&["{{path}}".to_string()], &json!({})).unwrap_err();
        assert!(missing
            .to_string()
            .contains("Missing project tool argument"));

        let nested = expand_command_args(&["{{paths}}".to_string()], &json!({ "paths": ["src"] }))
            .unwrap_err();
        assert!(nested.to_string().contains("must be a string"));
    }

    #[test]
    fn run_action_requires_confirmation_and_resource_key() {
        let tool = ProjectTool;
        let args = json!({ "action": "run", "name": "lint" });

        assert!(tool.requires_confirmation(&args));
        assert!(!tool.is_read_only(&args));
        assert_eq!(tool.resource_keys(&args), vec!["project-tool:lint"]);
    }
}
