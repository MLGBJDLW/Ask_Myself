use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::CoreError;
use crate::execution_environment::{
    review_execution_policy, ExecutionArtifact, ExecutionDecision, ExecutionDecisionKind,
    ExecutionEnvironment, ExecutionRequest,
};

use super::native_fs::{execute_native_filesystem, is_native_filesystem_program};
use super::shell_adapter::{execute_inner, resolve_program, RunShellOutput};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct LocalRunShellExecutionEnvironment;

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
