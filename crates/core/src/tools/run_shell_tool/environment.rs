use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::CoreError;
use crate::execution_environment::{
    review_execution_policy, ExecutionArtifact, ExecutionBackendKind, ExecutionDecision,
    ExecutionDecisionKind, ExecutionEnvironment, ExecutionRequest,
};

use super::native_fs::{execute_native_filesystem, is_native_filesystem_program};
use super::shell_adapter::{execute_inner, resolve_program, RunShellOutput};

#[cfg(any(target_os = "windows", target_os = "linux"))]
const BUBBLEWRAP_WORKSPACE: &str = "/tmp/workspace";

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct LocalRunShellExecutionEnvironment;

pub(super) fn apply_isolated_process_sandbox(
    request: &mut ExecutionRequest,
    requested_worktree_root: &Path,
    cwd: &Path,
) -> Result<(), CoreError> {
    let worktree_root = std::fs::canonicalize(requested_worktree_root)?;
    let managed_base = std::fs::canonicalize(std::env::temp_dir().join("nexa-code-ultra"))?;
    let managed_relative = worktree_root.strip_prefix(&managed_base).map_err(|_| {
        CoreError::InvalidInput(
            "Code Ultra refused a process sandbox outside its managed worktree root.".to_string(),
        )
    })?;
    if managed_relative.components().count() != 1 {
        return Err(CoreError::InvalidInput(
            "Code Ultra process sandbox root must be one direct managed worktree.".to_string(),
        ));
    }
    let canonical_cwd = std::fs::canonicalize(cwd)?;
    let relative_cwd = canonical_cwd.strip_prefix(&worktree_root).map_err(|_| {
        CoreError::InvalidInput(
            "Code Ultra process cwd is outside the isolated worktree.".to_string(),
        )
    })?;

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    apply_bubblewrap_sandbox(
        request,
        requested_worktree_root,
        &worktree_root,
        relative_cwd,
    )?;
    #[cfg(target_os = "macos")]
    apply_macos_sandbox(request, &worktree_root)?;
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return Err(CoreError::InvalidInput(
        "Code Ultra process sandbox is unavailable on this operating system.".to_string(),
    ));

    request.sandbox.backend = ExecutionBackendKind::Container;
    request.sandbox.network_allowed = false;
    request.sandbox.allowed_programs.clear();
    request.sandbox.denied_programs.clear();
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn apply_bubblewrap_sandbox(
    request: &mut ExecutionRequest,
    requested_worktree_root: &Path,
    canonical_worktree_root: &Path,
    relative_cwd: &Path,
) -> Result<(), CoreError> {
    let host_root = host_path_for_bubblewrap(canonical_worktree_root)?;
    let sandbox_cwd = sandbox_path(relative_cwd);
    let sandbox_program = rewrite_path_into_sandbox(
        &request.program,
        requested_worktree_root,
        canonical_worktree_root,
    )?;
    let sandbox_args = request
        .args
        .iter()
        .map(|argument| {
            rewrite_argument_into_sandbox(
                argument,
                requested_worktree_root,
                canonical_worktree_root,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut wrapper_args = vec![
        "--die-with-parent".to_string(),
        "--unshare-all".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--bind".to_string(),
        host_root,
        BUBBLEWRAP_WORKSPACE.to_string(),
        "--chdir".to_string(),
        sandbox_cwd,
        "--setenv".to_string(),
        "HOME".to_string(),
        "/tmp".to_string(),
        "--".to_string(),
        sandbox_program,
    ];
    wrapper_args.extend(sandbox_args);

    #[cfg(target_os = "windows")]
    {
        request.program = "wsl.exe".to_string();
        request.args = std::iter::once("--exec".to_string())
            .chain(std::iter::once("bwrap".to_string()))
            .chain(wrapper_args)
            .collect();
    }
    #[cfg(target_os = "linux")]
    {
        request.program = "bwrap".to_string();
        request.args = wrapper_args;
    }
    request.cwd = Some(canonical_worktree_root.display().to_string());
    Ok(())
}

#[cfg(target_os = "windows")]
fn host_path_for_bubblewrap(path: &Path) -> Result<String, CoreError> {
    windows_path_to_wsl(path)
}

#[cfg(target_os = "linux")]
fn host_path_for_bubblewrap(path: &Path) -> Result<String, CoreError> {
    Ok(path.display().to_string())
}

#[cfg(target_os = "windows")]
fn windows_path_to_wsl(path: &Path) -> Result<String, CoreError> {
    let raw = path.to_string_lossy();
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(raw.as_ref());
    let bytes = raw.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Err(CoreError::InvalidInput(format!(
            "Code Ultra cannot map Windows sandbox path '{raw}' into WSL."
        )));
    }
    let drive = char::from(bytes[0]).to_ascii_lowercase();
    let suffix = raw[2..].replace('\\', "/");
    Ok(format!("/mnt/{drive}{suffix}"))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn sandbox_path(relative: &Path) -> String {
    let suffix = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if suffix.is_empty() {
        BUBBLEWRAP_WORKSPACE.to_string()
    } else {
        format!("{BUBBLEWRAP_WORKSPACE}/{suffix}")
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn rewrite_path_into_sandbox(
    value: &str,
    requested_worktree_root: &Path,
    canonical_worktree_root: &Path,
) -> Result<String, CoreError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Ok(value.to_string());
    }
    let relative = path
        .strip_prefix(requested_worktree_root)
        .or_else(|_| path.strip_prefix(canonical_worktree_root))
        .map_err(|_| {
            CoreError::InvalidInput(format!(
                "Code Ultra rejected absolute process path '{value}' outside the isolated worktree."
            ))
        })?;
    Ok(sandbox_path(relative))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn rewrite_argument_into_sandbox(
    argument: &str,
    requested_worktree_root: &Path,
    canonical_worktree_root: &Path,
) -> Result<String, CoreError> {
    if let Some((flag, value)) = argument.split_once('=') {
        let rewritten =
            rewrite_path_into_sandbox(value, requested_worktree_root, canonical_worktree_root)?;
        return Ok(format!("{flag}={rewritten}"));
    }
    rewrite_path_into_sandbox(argument, requested_worktree_root, canonical_worktree_root)
}

#[cfg(target_os = "macos")]
fn apply_macos_sandbox(
    request: &mut ExecutionRequest,
    worktree_root: &Path,
) -> Result<(), CoreError> {
    let escaped_root = worktree_root
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let profile = format!(
        "(version 1) (allow default) (deny file-write*) (allow file-write* (subpath \"{escaped_root}\") (subpath \"/tmp\") (subpath \"/private/tmp\"))"
    );
    let program = std::mem::take(&mut request.program);
    let args = std::mem::take(&mut request.args);
    request.program = "sandbox-exec".to_string();
    request.args = vec!["-p".to_string(), profile, program];
    request.args.extend(args);
    Ok(())
}

#[async_trait]
impl ExecutionEnvironment for LocalRunShellExecutionEnvironment {
    fn id(&self) -> &str {
        "local_run_shell"
    }

    async fn review(&self, request: &ExecutionRequest) -> Result<ExecutionDecision, CoreError> {
        Ok(review_execution_policy(request))
    }

    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionArtifact, CoreError> {
        let decision = self.review(&request).await?;
        if decision.kind == ExecutionDecisionKind::Denied {
            return Err(CoreError::InvalidInput(decision.reason.clone()));
        }

        let cwd =
            request.cwd.as_deref().map(PathBuf::from).ok_or_else(|| {
                CoreError::InvalidInput("execution request requires cwd".to_string())
            })?;
        let output = execute_run_shell_request(&request, &cwd).await?;
        Ok(artifact_from_output(decision, output))
    }
}

async fn execute_run_shell_request(
    request: &ExecutionRequest,
    cwd: &Path,
) -> Result<RunShellOutput, CoreError> {
    if is_native_filesystem_program(&request.program) {
        if request.stdin.is_some() {
            return Err(CoreError::InvalidInput(
                "stdin is only supported for external programs, not native filesystem commands"
                    .to_string(),
            ));
        }
        return execute_native_filesystem(&request.program, &request.args, cwd)
            .await
            .map_err(CoreError::InvalidInput);
    }

    let resolved_program = resolve_program(&request.program);
    execute_inner(
        &resolved_program,
        &request.args,
        cwd,
        timeout_secs(request),
        request.stdin.as_deref(),
    )
    .await
    .map_err(CoreError::InvalidInput)
}

fn timeout_secs(request: &ExecutionRequest) -> u64 {
    match request.sandbox.timeout_ms {
        Some(0) => 0,
        Some(ms) => ms.saturating_add(999) / 1000,
        None => crate::tools::run_shell_contract::DEFAULT_TIMEOUT_SECS,
    }
}

fn artifact_from_output(decision: ExecutionDecision, output: RunShellOutput) -> ExecutionArtifact {
    ExecutionArtifact {
        decision,
        stdout: output.stdout,
        stderr: output.stderr,
        exit_status: output.exit_code,
        process_id: None,
        duration_ms: u64::try_from(output.duration_ms).unwrap_or(u64::MAX),
        timed_out: output.killed_by_timeout,
        changed_files: Vec::new(),
        stdout_truncated: output.truncated_stdout,
        stderr_truncated: output.truncated_stderr,
    }
}
