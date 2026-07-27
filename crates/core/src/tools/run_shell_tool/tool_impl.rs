use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::db::Database;
use crate::error::CoreError;
use crate::execution_environment::{ExecutionEnvironment, ExecutionRequest};
use async_trait::async_trait;

use super::super::path_utils::resolve_existing_directory_in_sources;
use super::super::run_shell_contract::{
    expected_format as run_shell_expected_format, invalid_arguments_message,
    tool_description as run_shell_tool_description, DEFAULT_TIMEOUT_SECS, TOOL_NAME,
};
use super::super::{scoped_sources, tool_contract_error_result, Tool, ToolCategory, ToolResult};
use super::environment::LocalRunShellExecutionEnvironment;
use super::file_tracking::{build_run_shell_file_changes, capture_file_snapshot};
use super::parser::parse_run_shell_args;
use super::policy::{
    normalize_run_shell_invocation, validate_args, validate_scoped_args, validate_stdin,
};
use super::shell_adapter::{
    clamp_timeout, format_confirmation, format_output, format_shell_confirmation,
    parse_shell_selector, spawn_background_process, RunShellOutput,
};

pub struct RunShellTool;

const DEFAULT_READY_TIMEOUT_SECS: u64 = 20;
const MAX_READY_TIMEOUT_SECS: u64 = 120;

struct ManagedService {
    child: tokio::process::Child,
    process_id: Option<u32>,
    program: String,
    ready_url: reqwest::Url,
    started_at: Instant,
}

static MANAGED_SERVICES: OnceLock<tokio::sync::Mutex<HashMap<String, ManagedService>>> =
    OnceLock::new();
static READINESS_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn managed_services() -> &'static tokio::sync::Mutex<HashMap<String, ManagedService>> {
    MANAGED_SERVICES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn readiness_client() -> &'static reqwest::Client {
    READINESS_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build run_shell readiness client")
    })
}

fn validate_ready_url(raw: Option<&str>) -> Result<reqwest::Url, String> {
    let raw = raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "background run_shell requires ready_url".to_string())?;
    let url = reqwest::Url::parse(raw)
        .map_err(|error| format!("invalid run_shell ready_url: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("run_shell ready_url must use http or https".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "run_shell ready_url must include a host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback {
        return Err(
            "run_shell ready_url must target localhost or a loopback IP address".to_string(),
        );
    }
    Ok(url)
}

async fn readiness_probe(url: &reqwest::Url) -> bool {
    readiness_client().get(url.clone()).send().await.is_ok()
}

fn looks_like_persistent_service(program: &str, args: &[String]) -> bool {
    let program = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let normalized_args: Vec<String> = args.iter().map(|arg| arg.to_ascii_lowercase()).collect();
    let joined = normalized_args.join(" ");

    joined.contains("http.server")
        || joined.contains("uvicorn")
        || joined.contains("flask run")
        || joined.contains("webpack serve")
        || joined.contains("next dev")
        || matches!(program.as_str(), "vite" | "uvicorn" | "gunicorn")
        || (matches!(
            program.as_str(),
            "npm" | "npm.cmd" | "pnpm" | "yarn" | "bun"
        ) && normalized_args.windows(2).any(|pair| {
            pair[0] == "run" && matches!(pair[1].as_str(), "dev" | "start" | "serve" | "preview")
        }))
        || (matches!(program.as_str(), "npx" | "npx.cmd")
            && normalized_args
                .first()
                .is_some_and(|arg| matches!(arg.as_str(), "vite" | "webpack" | "next")))
}

async fn start_managed_service(
    call_id: &str,
    program: &str,
    args: &[String],
    cwd: &Path,
    ready_url: reqwest::Url,
    ready_timeout_secs: u64,
) -> ToolResult {
    if readiness_probe(&ready_url).await {
        return error_result(
            call_id,
            format!(
                "ready_url {ready_url} is already responding; choose a free port or check the existing service before starting another process"
            ),
        );
    }

    let mut child = match spawn_background_process(program, args, cwd) {
        Ok(child) => child,
        Err(error) => return error_result(call_id, error),
    };
    let process_id = child.id();
    let started_at = Instant::now();
    let deadline = started_at + Duration::from_secs(ready_timeout_secs);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return error_result(
                    call_id,
                    format!(
                        "background service exited before {ready_url} became ready (status: {status})"
                    ),
                );
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill().await;
                return error_result(
                    call_id,
                    format!("failed to inspect background service: {error}"),
                );
            }
        }

        if readiness_probe(&ready_url).await {
            managed_services().lock().await.insert(
                call_id.to_string(),
                ManagedService {
                    child,
                    process_id,
                    program: program.to_string(),
                    ready_url: ready_url.clone(),
                    started_at,
                },
            );
            return ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "Background service is ready at {ready_url}. service_id: {call_id}; process_id: {}. Recheck with service_action=status and stop it with service_action=stop when finished.",
                    process_id.map_or_else(|| "unknown".to_string(), |id| id.to_string()),
                ),
                is_error: false,
                artifacts: Some(serde_json::json!({
                    "kind": "managedService",
                    "serviceId": call_id,
                    "processId": process_id,
                    "status": "ready",
                    "readyUrl": ready_url.as_str(),
                    "program": program,
                })),
            };
        }

        if Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return error_result(
                call_id,
                format!(
                    "background service did not become ready at {ready_url} within {ready_timeout_secs}s and was stopped"
                ),
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn manage_service(call_id: &str, action: &str, service_id: &str) -> ToolResult {
    let mut registry = managed_services().lock().await;
    let Some(mut service) = registry.remove(service_id) else {
        return error_result(
            call_id,
            format!("managed service '{service_id}' was not found or has already stopped"),
        );
    };

    if action == "stop" {
        let kill_error = service.child.kill().await.err();
        let _ = service.child.wait().await;
        return ToolResult {
            call_id: call_id.to_string(),
            content: kill_error.map_or_else(
                || format!("Stopped managed service {service_id}."),
                |error| format!("Managed service {service_id} had already exited: {error}"),
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "managedService",
                "serviceId": service_id,
                "processId": service.process_id,
                "status": "stopped",
                "readyUrl": service.ready_url.as_str(),
                "program": service.program,
            })),
        };
    }

    match service.child.try_wait() {
        Ok(Some(status)) => ToolResult {
            call_id: call_id.to_string(),
            content: format!("Managed service {service_id} exited with status {status}."),
            is_error: true,
            artifacts: Some(serde_json::json!({
                "kind": "managedService",
                "serviceId": service_id,
                "processId": service.process_id,
                "status": "exited",
                "readyUrl": service.ready_url.as_str(),
                "program": service.program,
            })),
        },
        Err(error) => error_result(
            call_id,
            format!("failed to inspect service {service_id}: {error}"),
        ),
        Ok(None) => {
            let healthy = readiness_probe(&service.ready_url).await;
            let uptime_ms = service.started_at.elapsed().as_millis() as u64;
            let result = ToolResult {
                call_id: call_id.to_string(),
                content: if healthy {
                    format!(
                        "Managed service {service_id} is running and healthy at {}.",
                        service.ready_url
                    )
                } else {
                    format!(
                        "Managed service {service_id} is still running, but {} is not responding.",
                        service.ready_url
                    )
                },
                is_error: !healthy,
                artifacts: Some(serde_json::json!({
                    "kind": "managedService",
                    "serviceId": service_id,
                    "processId": service.process_id,
                    "status": if healthy { "ready" } else { "unhealthy" },
                    "readyUrl": service.ready_url.as_str(),
                    "program": service.program,
                    "uptimeMs": uptime_ms,
                })),
            };
            registry.insert(service_id.to_string(), service);
            result
        }
    }
}

fn error_result(call_id: &str, msg: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: msg.into(),
        is_error: true,
        artifacts: None,
    }
}

// ---------------------------------------------------------------------------
// Tool trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for RunShellTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        run_shell_tool_description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        super::super::run_shell_contract::parameters_schema()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        // No dedicated Shell category exists; group with FileSystem since the
        // operation is scoped to source directories. If a Shell category is
        // added later, include it here.
        &[ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|command| !command.is_empty());
        let shell = parse_shell_selector(args.get("shell")).ok().flatten();
        let args_vec: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("<active source root>");
        let timeout = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .map(|t| clamp_timeout(Some(t)))
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        let stdin_bytes = args.get("stdin").and_then(|v| v.as_str()).map(str::len);
        if let Some(command) = command {
            if let Some(shell) = shell {
                Some(format_shell_confirmation(
                    shell,
                    command,
                    cwd,
                    timeout,
                    stdin_bytes,
                ))
            } else {
                Some(format_confirmation(command, &[], cwd, timeout, stdin_bytes))
            }
        } else {
            let program = args
                .get("program")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            Some(format_confirmation(
                program,
                &args_vec,
                cwd,
                timeout,
                stdin_bytes,
            ))
        }
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let parsed = match parse_run_shell_args(arguments) {
            Ok(parsed) => parsed,
            Err(err) => {
                return Ok(tool_contract_error_result(
                    call_id,
                    "invalid_run_shell_arguments",
                    invalid_arguments_message(err),
                    run_shell_expected_format(),
                ));
            }
        };
        let service_action = parsed
            .service_action
            .as_deref()
            .unwrap_or("run")
            .trim()
            .to_ascii_lowercase();
        if !matches!(service_action.as_str(), "run" | "status" | "stop") {
            return Ok(error_result(
                call_id,
                "run_shell service_action must be run, status, or stop",
            ));
        }
        if service_action != "run" {
            let Some(service_id) = parsed
                .service_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(error_result(
                    call_id,
                    format!("service_action={service_action} requires service_id"),
                ));
            };
            return Ok(manage_service(call_id, &service_action, service_id).await);
        }
        if parsed.service_id.is_some() {
            return Ok(error_result(
                call_id,
                "service_id is only valid with service_action=status or service_action=stop",
            ));
        }
        let shell_access_mode = db
            .load_app_config()
            .map(|cfg| cfg.shell_access_mode)
            .unwrap_or_default();

        let (canonical_program, normalized_args) =
            match normalize_run_shell_invocation(&parsed, shell_access_mode) {
                Ok(invocation) => invocation,
                Err(msg) => return Ok(error_result(call_id, msg)),
            };

        if let Err(msg) = validate_args(shell_access_mode, &canonical_program, &normalized_args) {
            return Ok(error_result(call_id, msg));
        }
        if let Err(msg) = validate_stdin(parsed.stdin.as_deref()) {
            return Ok(error_result(call_id, msg));
        }

        let timeout = clamp_timeout(parsed.timeout_secs);
        if !parsed.background && looks_like_persistent_service(&canonical_program, &normalized_args)
        {
            return Ok(error_result(
                call_id,
                "Long-running local servers cannot run in the foreground because they block the agent. Retry with background=true and a loopback ready_url; use the returned service_id for later status and stop calls.",
            ));
        }
        if !parsed.background && parsed.ready_url.is_some() {
            return Ok(error_result(call_id, "ready_url requires background=true"));
        }
        if parsed.background && parsed.stdin.is_some() {
            return Ok(error_result(
                call_id,
                "background run_shell does not accept stdin",
            ));
        }
        let ready_url = if parsed.background {
            match validate_ready_url(parsed.ready_url.as_deref()) {
                Ok(url) => Some(url),
                Err(message) => return Ok(error_result(call_id, message)),
            }
        } else {
            None
        };

        // Resolve cwd inside a registered source directory (blocking fs ops).
        let cwd_input = parsed
            .cwd
            .clone()
            .map(|cwd| cwd.trim().to_string())
            .filter(|cwd| !cwd.is_empty());
        let args_input = normalized_args.clone();
        let db_clone = db.clone();
        let scope_clone = source_scope.to_vec();
        let program = canonical_program.clone();
        let cwd_result: Result<PathBuf, String> = tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db_clone, &scope_clone)
                .map_err(|e| format!("failed to load sources: {e}"))?;
            let cwd_input = cwd_input.unwrap_or_else(|| {
                sources
                    .first()
                    .map(|source| source.root_path.clone())
                    .unwrap_or_default()
            });
            if cwd_input.is_empty() {
                return Err(
                    "run_shell.cwd was omitted and no active source root is available. Add a source or pass cwd explicitly."
                        .to_string(),
                );
            }
            if shell_access_mode.is_restricted() {
                if sources.is_empty() {
                    return Err("No sources registered. Add a source directory first.".to_string());
                }
                let cwd = resolve_existing_directory_in_sources(Path::new(&cwd_input), &sources)?;
                validate_scoped_args(shell_access_mode, &program, &args_input, &cwd, &sources)?;
                Ok(cwd)
            } else {
                if !Path::new(&cwd_input).is_absolute() && !sources.is_empty() {
                    if let Ok(cwd) =
                        resolve_existing_directory_in_sources(Path::new(&cwd_input), &sources)
                    {
                        return Ok(cwd);
                    }
                }
                let cwd = std::fs::canonicalize(Path::new(&cwd_input))
                    .map_err(|e| format!("Cannot resolve directory '{}': {e}", cwd_input))?;
                if !cwd.is_dir() {
                    return Err(format!("'{}' is not a directory.", cwd_input));
                }
                Ok(cwd)
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;

        let cwd_path = match cwd_result {
            Ok(p) => p,
            Err(msg) => return Ok(error_result(call_id, msg)),
        };

        if let Some(ready_url) = ready_url {
            let ready_timeout_secs = parsed
                .ready_timeout_secs
                .unwrap_or(DEFAULT_READY_TIMEOUT_SECS)
                .clamp(1, MAX_READY_TIMEOUT_SECS);
            return Ok(start_managed_service(
                call_id,
                &canonical_program,
                &normalized_args,
                &cwd_path,
                ready_url,
                ready_timeout_secs,
            )
            .await);
        }

        let before_root = cwd_path.clone();
        let before_snapshot =
            tokio::task::spawn_blocking(move || capture_file_snapshot(&before_root))
                .await
                .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;

        let environment = LocalRunShellExecutionEnvironment;
        let mut execution_request = ExecutionRequest::for_run_shell(
            canonical_program.clone(),
            normalized_args.clone(),
            shell_access_mode,
            source_scope.to_vec(),
        );
        execution_request.cwd = Some(cwd_path.display().to_string());
        execution_request.stdin = parsed.stdin.clone();
        execution_request.sandbox.timeout_ms = Some(timeout.saturating_mul(1000));
        let execution_artifact = match environment.execute(execution_request).await {
            Ok(artifact) => artifact,
            Err(CoreError::InvalidInput(msg)) => return Ok(error_result(call_id, msg)),
            Err(err) => return Err(err),
        };
        let output = RunShellOutput {
            exit_code: execution_artifact.exit_status,
            stdout: execution_artifact.stdout,
            stderr: execution_artifact.stderr,
            duration_ms: execution_artifact.duration_ms as u128,
            truncated_stdout: execution_artifact.stdout_truncated,
            truncated_stderr: execution_artifact.stderr_truncated,
            killed_by_timeout: execution_artifact.timed_out,
        };

        let after_root = cwd_path.clone();
        let after_snapshot =
            tokio::task::spawn_blocking(move || capture_file_snapshot(&after_root))
                .await
                .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;
        let file_changes =
            build_run_shell_file_changes(&cwd_path, &before_snapshot, &after_snapshot);

        tracing::info!(
            target: "tool.run_shell",
            program = canonical_program,
            execution_environment = environment.id(),
            args_count = normalized_args.len(),
            cwd = %cwd_path.display(),
            exit_code = ?output.exit_code,
            duration_ms = output.duration_ms as u64,
            killed = output.killed_by_timeout,
            truncated_stdout = output.truncated_stdout,
            truncated_stderr = output.truncated_stderr,
            stdin_bytes = parsed.stdin.as_deref().map(str::len).unwrap_or(0),
            "run_shell executed"
        );

        let is_error = output.killed_by_timeout || output.exit_code != Some(0);
        let mut content = format_output(&output);
        if let Some(changes) = &file_changes {
            if !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("\n── file changes ──\n");
            content.push_str(&changes.summary);
            content.push('\n');
        }

        let execution_code = if output.killed_by_timeout {
            Some("command_timeout")
        } else if output.exit_code != Some(0) {
            Some("command_exit_nonzero")
        } else {
            None
        };
        let mut artifacts = file_changes
            .map(|changes| changes.artifact)
            .unwrap_or_else(|| serde_json::json!({ "kind": "commandExecution" }));
        if let Some(object) = artifacts.as_object_mut() {
            object.insert(
                "execution".to_string(),
                serde_json::json!({
                    "exitCode": output.exit_code,
                    "durationMs": output.duration_ms as u64,
                    "timedOut": output.killed_by_timeout,
                    "stdoutTruncated": output.truncated_stdout,
                    "stderrTruncated": output.truncated_stderr,
                    "errorCode": execution_code,
                    "retryable": is_error,
                    "recovery": if is_error {
                        "inspect the preserved output tail, then correct the command, cwd, timeout, or failing code before retrying"
                    } else {
                        "none"
                    }
                }),
            );
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error,
            artifacts: Some(artifacts),
        })
    }
}
