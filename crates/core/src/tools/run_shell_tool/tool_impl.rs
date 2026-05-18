use std::path::{Path, PathBuf};

use crate::db::Database;
use crate::error::CoreError;
use async_trait::async_trait;

use super::super::path_utils::resolve_existing_directory_in_sources;
use super::super::run_shell_contract::{
    expected_format as run_shell_expected_format, invalid_arguments_message,
    tool_description as run_shell_tool_description, DEFAULT_TIMEOUT_SECS, TOOL_NAME,
};
use super::super::{scoped_sources, tool_contract_error_result, Tool, ToolCategory, ToolResult};
use super::file_tracking::{build_run_shell_file_changes, capture_file_snapshot};
use super::native_fs::{execute_native_filesystem, is_native_filesystem_program};
use super::parser::parse_run_shell_args;
use super::policy::{
    normalize_run_shell_invocation, validate_args, validate_scoped_args, validate_stdin,
};
use super::shell_adapter::{
    clamp_timeout, execute_inner, format_confirmation, format_output, format_shell_confirmation,
    parse_shell_selector, resolve_program,
};

pub struct RunShellTool;

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
            .unwrap_or("<unknown>");
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
        let shell_access_mode = db
            .load_app_config()
            .map(|cfg| cfg.shell_access_mode)
            .unwrap_or_default();

        let (canonical_program, normalized_args) =
            match normalize_run_shell_invocation(&parsed, shell_access_mode) {
                Ok(invocation) => invocation,
                Err(msg) => return Ok(error_result(call_id, msg)),
            };

        let resolved_program = resolve_program(&canonical_program);

        if let Err(msg) = validate_args(shell_access_mode, &canonical_program, &normalized_args) {
            return Ok(error_result(call_id, msg));
        }
        if let Err(msg) = validate_stdin(parsed.stdin.as_deref()) {
            return Ok(error_result(call_id, msg));
        }

        let timeout = clamp_timeout(parsed.timeout_secs);

        // Resolve cwd inside a registered source directory (blocking fs ops).
        let cwd_input = parsed.cwd.clone();
        let args_input = normalized_args.clone();
        let db_clone = db.clone();
        let scope_clone = source_scope.to_vec();
        let program = canonical_program.clone();
        let cwd_result: Result<PathBuf, String> = tokio::task::spawn_blocking(move || {
            if shell_access_mode.is_restricted() {
                let sources = scoped_sources(&db_clone, &scope_clone)
                    .map_err(|e| format!("failed to load sources: {e}"))?;
                if sources.is_empty() {
                    return Err("No sources registered. Add a source directory first.".to_string());
                }
                let cwd = resolve_existing_directory_in_sources(Path::new(&cwd_input), &sources)?;
                validate_scoped_args(shell_access_mode, &program, &args_input, &cwd, &sources)?;
                Ok(cwd)
            } else {
                if !Path::new(&cwd_input).is_absolute() {
                    let sources = scoped_sources(&db_clone, &scope_clone)
                        .map_err(|e| format!("failed to load sources: {e}"))?;
                    if !sources.is_empty() {
                        if let Ok(cwd) =
                            resolve_existing_directory_in_sources(Path::new(&cwd_input), &sources)
                        {
                            return Ok(cwd);
                        }
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

        let before_root = cwd_path.clone();
        let before_snapshot =
            tokio::task::spawn_blocking(move || capture_file_snapshot(&before_root))
                .await
                .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;

        let output = if is_native_filesystem_program(&canonical_program) {
            if parsed.stdin.is_some() {
                return Ok(error_result(
                    call_id,
                    "stdin is only supported for external programs, not native filesystem commands",
                ));
            }
            match execute_native_filesystem(&canonical_program, &normalized_args, &cwd_path).await {
                Ok(o) => o,
                Err(msg) => return Ok(error_result(call_id, msg)),
            }
        } else {
            match execute_inner(
                &resolved_program,
                &normalized_args,
                &cwd_path,
                timeout,
                parsed.stdin.as_deref(),
            )
            .await
            {
                Ok(o) => o,
                Err(msg) => return Ok(error_result(call_id, msg)),
            }
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

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error,
            artifacts: file_changes.map(|changes| changes.artifact),
        })
    }
}
