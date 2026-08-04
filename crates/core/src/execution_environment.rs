//! Execution Environment and Sandbox Policy contract.
//!
//! Tools, workflows, connectors, and skill resources should express execution
//! through this interface so policy decisions stay visible and reusable.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::app_settings::ShellAccessMode;
use crate::approval::ApprovalRisk;
use crate::error::CoreError;

pub const EXECUTION_ENVIRONMENT_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendKind {
    LocalRestricted,
    LocalOpen,
    Container,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionCallerIdentity {
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub package_id: Option<String>,
    #[serde(default)]
    pub skill_id: Option<String>,
    #[serde(default)]
    pub workflow_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    pub backend: ExecutionBackendKind,
    #[serde(default)]
    pub source_scope: Vec<String>,
    #[serde(default)]
    pub network_allowed: bool,
    #[serde(default)]
    pub shell_access_mode: ShellAccessMode,
    #[serde(default)]
    pub allowed_programs: Vec<String>,
    #[serde(default)]
    pub denied_programs: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub output_limit_bytes: Option<usize>,
    #[serde(default = "default_true")]
    pub capture_file_changes: bool,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            backend: ExecutionBackendKind::LocalRestricted,
            source_scope: Vec::new(),
            network_allowed: false,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: Vec::new(),
            denied_programs: Vec::new(),
            timeout_ms: Some(300_000),
            output_limit_bytes: Some(128 * 1024),
            capture_file_changes: true,
        }
    }
}

impl SandboxPolicy {
    pub fn for_run_shell(shell_access_mode: ShellAccessMode, source_scope: Vec<String>) -> Self {
        let restricted = shell_access_mode.is_restricted();
        Self {
            backend: if restricted {
                ExecutionBackendKind::LocalRestricted
            } else {
                ExecutionBackendKind::LocalOpen
            },
            source_scope,
            network_allowed: matches!(shell_access_mode, ShellAccessMode::Open),
            shell_access_mode,
            allowed_programs: if restricted {
                crate::tools::run_shell_contract::PROGRAM_WHITELIST
                    .iter()
                    .map(|program| (*program).to_string())
                    .collect()
            } else {
                Vec::new()
            },
            denied_programs: Vec::new(),
            timeout_ms: Some(crate::tools::run_shell_contract::DEFAULT_TIMEOUT_SECS * 1000),
            output_limit_bytes: Some(crate::tools::run_shell_contract::MAX_OUTPUT_BYTES),
            capture_file_changes: true,
        }
    }

    pub fn for_project_tool(
        source_scope: Vec<String>,
        network_allowed: bool,
        timeout_secs: u64,
    ) -> Self {
        Self {
            backend: ExecutionBackendKind::LocalRestricted,
            source_scope,
            network_allowed,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: Vec::new(),
            denied_programs: Vec::new(),
            timeout_ms: Some(timeout_secs.saturating_mul(1000)),
            output_limit_bytes: None,
            capture_file_changes: false,
        }
    }

    pub fn for_workflow_action(
        source_scope: Vec<String>,
        network_allowed: bool,
        timeout_secs: u64,
    ) -> Self {
        Self {
            backend: ExecutionBackendKind::LocalRestricted,
            source_scope,
            network_allowed,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: Vec::new(),
            denied_programs: Vec::new(),
            timeout_ms: Some(timeout_secs.saturating_mul(1000)),
            output_limit_bytes: Some(128 * 1024),
            capture_file_changes: true,
        }
    }

    pub fn for_connector_helper(
        source_scope: Vec<String>,
        network_allowed: bool,
        timeout_secs: u64,
        allowed_program: String,
    ) -> Self {
        Self {
            backend: ExecutionBackendKind::LocalRestricted,
            source_scope,
            network_allowed,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: vec![allowed_program],
            denied_programs: Vec::new(),
            timeout_ms: Some(timeout_secs.saturating_mul(1000)),
            output_limit_bytes: Some(128 * 1024),
            capture_file_changes: false,
        }
    }

    pub fn for_skill_resource_helper(
        source_scope: Vec<String>,
        timeout_secs: u64,
        allowed_program: String,
    ) -> Self {
        Self {
            backend: ExecutionBackendKind::LocalRestricted,
            source_scope,
            network_allowed: false,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: vec![allowed_program],
            denied_programs: Vec::new(),
            timeout_ms: Some(timeout_secs.saturating_mul(1000)),
            output_limit_bytes: Some(128 * 1024),
            capture_file_changes: false,
        }
    }

    pub fn for_detached_local_tool(
        source_scope: Vec<String>,
        network_allowed: bool,
        allowed_program: String,
    ) -> Self {
        Self {
            backend: ExecutionBackendKind::LocalOpen,
            source_scope,
            network_allowed,
            shell_access_mode: ShellAccessMode::Restricted,
            allowed_programs: vec![allowed_program],
            denied_programs: Vec::new(),
            timeout_ms: Some(0),
            output_limit_bytes: Some(0),
            capture_file_changes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub version: u16,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub environment: Vec<(String, String)>,
    #[serde(default)]
    pub network_intent: bool,
    #[serde(default)]
    pub expected_writes: Vec<String>,
    pub caller: ExecutionCallerIdentity,
    pub sandbox: SandboxPolicy,
}

impl ExecutionRequest {
    pub fn local_tool(program: impl Into<String>, args: Vec<String>, tool_name: &str) -> Self {
        Self {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            program: program.into(),
            args,
            stdin: None,
            cwd: None,
            environment: Vec::new(),
            network_intent: false,
            expected_writes: Vec::new(),
            caller: ExecutionCallerIdentity {
                tool_name: Some(tool_name.to_string()),
                package_id: None,
                skill_id: None,
                workflow_id: None,
            },
            sandbox: SandboxPolicy::default(),
        }
    }

    pub fn local_tool_with_policy(
        program: impl Into<String>,
        args: Vec<String>,
        tool_name: &str,
        sandbox: SandboxPolicy,
    ) -> Self {
        Self {
            sandbox,
            ..Self::local_tool(program, args, tool_name)
        }
    }

    pub fn for_run_shell(
        program: impl Into<String>,
        args: Vec<String>,
        shell_access_mode: ShellAccessMode,
        source_scope: Vec<String>,
    ) -> Self {
        Self::local_tool_with_policy(
            program,
            args,
            "run_shell",
            SandboxPolicy::for_run_shell(shell_access_mode, source_scope),
        )
    }

    pub fn for_project_tool(
        program: impl Into<String>,
        args: Vec<String>,
        source_scope: Vec<String>,
        network_intent: bool,
        timeout_secs: u64,
    ) -> Self {
        let mut request = Self::local_tool_with_policy(
            program,
            args,
            "project_tool",
            SandboxPolicy::for_project_tool(source_scope, network_intent, timeout_secs),
        );
        request.network_intent = network_intent;
        request
    }

    pub fn for_detached_local_tool(
        program: impl Into<String>,
        args: Vec<String>,
        tool_name: &str,
        source_scope: Vec<String>,
        network_intent: bool,
    ) -> Self {
        let program = program.into();
        let mut request = Self::local_tool_with_policy(
            program.clone(),
            args,
            tool_name,
            SandboxPolicy::for_detached_local_tool(source_scope, network_intent, program),
        );
        request.network_intent = network_intent;
        request
    }

    pub fn for_workflow_action(
        program: impl Into<String>,
        args: Vec<String>,
        workflow_id: impl Into<String>,
        package_id: Option<String>,
        source_scope: Vec<String>,
        network_intent: bool,
        timeout_secs: u64,
    ) -> Self {
        let mut request = Self::local_tool_with_policy(
            program,
            args,
            "workflow_action",
            SandboxPolicy::for_workflow_action(source_scope, network_intent, timeout_secs),
        );
        request.caller.workflow_id = Some(workflow_id.into());
        request.caller.package_id = package_id;
        request.network_intent = network_intent;
        request
    }

    pub fn for_connector_helper(
        program: impl Into<String>,
        args: Vec<String>,
        package_id: impl Into<String>,
        source_scope: Vec<String>,
        network_intent: bool,
        timeout_secs: u64,
    ) -> Self {
        let program = program.into();
        let mut request = Self::local_tool_with_policy(
            program.clone(),
            args,
            "connector_helper",
            SandboxPolicy::for_connector_helper(
                source_scope,
                network_intent,
                timeout_secs,
                program,
            ),
        );
        request.caller.package_id = Some(package_id.into());
        request.network_intent = network_intent;
        request
    }

    pub fn for_skill_resource_helper(
        program: impl Into<String>,
        args: Vec<String>,
        skill_id: impl Into<String>,
        source_scope: Vec<String>,
        timeout_secs: u64,
    ) -> Self {
        let program = program.into();
        let mut request = Self::local_tool_with_policy(
            program.clone(),
            args,
            "skill_resource_helper",
            SandboxPolicy::for_skill_resource_helper(source_scope, timeout_secs, program),
        );
        request.caller.skill_id = Some(skill_id.into());
        request
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionDecisionKind {
    Allowed,
    RequiresApproval,
    Denied,
    Amended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionDecision {
    pub kind: ExecutionDecisionKind,
    pub reason: String,
    pub risk: ApprovalRisk,
    #[serde(default)]
    pub amended_args: Option<Vec<String>>,
    pub permission_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionArtifact {
    pub decision: ExecutionDecision,
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    pub duration_ms: u64,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
}

#[async_trait]
pub trait ExecutionEnvironment: Send + Sync {
    fn id(&self) -> &str;

    async fn review(&self, request: &ExecutionRequest) -> Result<ExecutionDecision, CoreError>;

    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionArtifact, CoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowActionExecutionRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub workflow_id: String,
    pub package_id: Option<String>,
    pub source_scope: Vec<String>,
    pub network_intent: bool,
    pub timeout_secs: u64,
    pub stdin: Option<String>,
    pub environment: Vec<(String, String)>,
    pub expected_writes: Vec<String>,
}

impl WorkflowActionExecutionRequest {
    pub fn into_execution_request(self) -> ExecutionRequest {
        let mut request = ExecutionRequest::for_workflow_action(
            self.program,
            self.args,
            self.workflow_id,
            self.package_id,
            self.source_scope,
            self.network_intent,
            self.timeout_secs,
        );
        apply_local_process_fields(
            &mut request,
            self.cwd,
            self.stdin,
            self.environment,
            self.expected_writes,
        );
        request
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorHelperExecutionRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub package_id: String,
    pub source_scope: Vec<String>,
    pub network_intent: bool,
    pub timeout_secs: u64,
    pub stdin: Option<String>,
    pub environment: Vec<(String, String)>,
    pub expected_writes: Vec<String>,
}

impl ConnectorHelperExecutionRequest {
    pub fn into_execution_request(self) -> ExecutionRequest {
        let mut request = ExecutionRequest::for_connector_helper(
            self.program,
            self.args,
            self.package_id,
            self.source_scope,
            self.network_intent,
            self.timeout_secs,
        );
        apply_local_process_fields(
            &mut request,
            self.cwd,
            self.stdin,
            self.environment,
            self.expected_writes,
        );
        request
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceHelperExecutionRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub skill_id: String,
    pub source_scope: Vec<String>,
    pub timeout_secs: u64,
    pub stdin: Option<String>,
    pub environment: Vec<(String, String)>,
    pub expected_writes: Vec<String>,
}

impl SkillResourceHelperExecutionRequest {
    pub fn into_execution_request(self) -> ExecutionRequest {
        let mut request = ExecutionRequest::for_skill_resource_helper(
            self.program,
            self.args,
            self.skill_id,
            self.source_scope,
            self.timeout_secs,
        );
        apply_local_process_fields(
            &mut request,
            self.cwd,
            self.stdin,
            self.environment,
            self.expected_writes,
        );
        request
    }
}

fn apply_local_process_fields(
    request: &mut ExecutionRequest,
    cwd: PathBuf,
    stdin: Option<String>,
    environment: Vec<(String, String)>,
    expected_writes: Vec<String>,
) {
    request.cwd = Some(cwd.display().to_string());
    request.stdin = stdin;
    request.environment = environment;
    request.expected_writes = expected_writes;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalProcessExecutionEnvironment;

impl LocalProcessExecutionEnvironment {
    pub async fn execute_workflow_action(
        &self,
        request: WorkflowActionExecutionRequest,
    ) -> Result<ExecutionArtifact, CoreError> {
        self.execute(request.into_execution_request()).await
    }

    pub async fn execute_connector_helper(
        &self,
        request: ConnectorHelperExecutionRequest,
    ) -> Result<ExecutionArtifact, CoreError> {
        self.execute(request.into_execution_request()).await
    }

    pub async fn execute_skill_resource_helper(
        &self,
        request: SkillResourceHelperExecutionRequest,
    ) -> Result<ExecutionArtifact, CoreError> {
        self.execute(request.into_execution_request()).await
    }
}

#[async_trait]
impl ExecutionEnvironment for LocalProcessExecutionEnvironment {
    fn id(&self) -> &str {
        "local_process"
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
        let started = Instant::now();
        let mut command = tokio::process::Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(&cwd)
            .stdin(if request.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &request.environment {
            command.env(key, value);
        }
        crate::background_process::configure_tokio_background(&mut command);

        let mut child = command.spawn()?;
        let stdin_task = if let Some(stdin) = request.stdin.clone() {
            let mut child_stdin = child
                .stdin
                .take()
                .ok_or_else(|| CoreError::Internal("failed to open child stdin".to_string()))?;
            Some(tokio::spawn(async move {
                child_stdin.write_all(stdin.as_bytes()).await?;
                child_stdin.shutdown().await
            }))
        } else {
            None
        };

        let wait = child.wait_with_output();
        let result = match request.sandbox.timeout_ms {
            Some(0) | None => Ok(wait.await),
            Some(timeout_ms) => {
                tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait).await
            }
        };

        match result {
            Ok(Ok(output)) => {
                if let Some(task) = stdin_task {
                    match task.await {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
                        Ok(Err(err)) => return Err(CoreError::Io(err)),
                        Err(err) => {
                            return Err(CoreError::Internal(format!(
                                "stdin writer task failed: {err}"
                            )))
                        }
                    }
                }
                let (stdout, stdout_truncated) =
                    output_text(output.stdout, request.sandbox.output_limit_bytes);
                let (stderr, stderr_truncated) =
                    output_text(output.stderr, request.sandbox.output_limit_bytes);
                Ok(ExecutionArtifact {
                    decision,
                    stdout,
                    stderr,
                    exit_status: output.status.code(),
                    process_id: None,
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    timed_out: false,
                    changed_files: Vec::new(),
                    stdout_truncated,
                    stderr_truncated,
                })
            }
            Ok(Err(err)) => Err(CoreError::Io(err)),
            Err(_) => {
                if let Some(task) = stdin_task {
                    task.abort();
                }
                Ok(ExecutionArtifact {
                    decision,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_status: None,
                    process_id: None,
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    timed_out: true,
                    changed_files: Vec::new(),
                    stdout_truncated: false,
                    stderr_truncated: false,
                })
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalDetachedProcessExecutionEnvironment;

#[async_trait]
impl ExecutionEnvironment for LocalDetachedProcessExecutionEnvironment {
    fn id(&self) -> &str {
        "local_detached_process"
    }

    async fn review(&self, request: &ExecutionRequest) -> Result<ExecutionDecision, CoreError> {
        Ok(review_execution_policy(request))
    }

    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionArtifact, CoreError> {
        let decision = self.review(&request).await?;
        if decision.kind == ExecutionDecisionKind::Denied {
            return Err(CoreError::InvalidInput(decision.reason.clone()));
        }

        let started = Instant::now();
        let mut command = tokio::process::Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false);
        if let Some(cwd) = request.cwd.as_deref() {
            command.current_dir(cwd);
        }
        for (key, value) in &request.environment {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .map_err(|e| CoreError::Internal(format!("Failed to launch desktop action: {e}")))?;
        let process_id = child.id();
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        Ok(ExecutionArtifact {
            decision,
            stdout: String::new(),
            stderr: String::new(),
            exit_status: None,
            process_id,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            timed_out: false,
            changed_files: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        })
    }
}

fn output_text(bytes: Vec<u8>, limit: Option<usize>) -> (String, bool) {
    let Some(limit) = limit else {
        return (String::from_utf8_lossy(&bytes).into_owned(), false);
    };
    if bytes.len() <= limit {
        return (String::from_utf8_lossy(&bytes).into_owned(), false);
    }

    let mut end = limit;
    while end > 0 && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    (String::from_utf8_lossy(&bytes[..end]).into_owned(), true)
}

pub fn deterministic_execution_permission_key(request: &ExecutionRequest) -> String {
    let caller = &request.caller;
    format!(
        "exec:{}:{}:{}:{}:{}",
        caller.tool_name.as_deref().unwrap_or("-"),
        caller.package_id.as_deref().unwrap_or("-"),
        caller.skill_id.as_deref().unwrap_or("-"),
        caller.workflow_id.as_deref().unwrap_or("-"),
        request.program
    )
}

pub fn review_execution_policy(request: &ExecutionRequest) -> ExecutionDecision {
    let permission_key = deterministic_execution_permission_key(request);
    let program = request.program.trim().to_ascii_lowercase();

    if request.version != EXECUTION_ENVIRONMENT_CONTRACT_VERSION {
        return ExecutionDecision {
            kind: ExecutionDecisionKind::Denied,
            reason: "unsupported execution request version".to_string(),
            risk: ApprovalRisk::High,
            amended_args: None,
            permission_key,
        };
    }

    if request
        .sandbox
        .denied_programs
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(&program))
    {
        return ExecutionDecision {
            kind: ExecutionDecisionKind::Denied,
            reason: "program is denied by sandbox policy".to_string(),
            risk: ApprovalRisk::High,
            amended_args: None,
            permission_key,
        };
    }

    if !request.sandbox.allowed_programs.is_empty()
        && !request
            .sandbox
            .allowed_programs
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&program))
    {
        return ExecutionDecision {
            kind: ExecutionDecisionKind::Denied,
            reason: "program is outside the sandbox allowed program list".to_string(),
            risk: ApprovalRisk::High,
            amended_args: None,
            permission_key,
        };
    }

    if request.network_intent && !request.sandbox.network_allowed {
        return ExecutionDecision {
            kind: ExecutionDecisionKind::RequiresApproval,
            reason: "execution requests network access outside the sandbox policy".to_string(),
            risk: ApprovalRisk::High,
            amended_args: None,
            permission_key,
        };
    }

    let high_risk_program = matches!(
        program.as_str(),
        "powershell" | "pwsh" | "cmd" | "sh" | "bash"
    );
    if high_risk_program || request.sandbox.shell_access_mode.requires_confirmation() {
        return ExecutionDecision {
            kind: ExecutionDecisionKind::RequiresApproval,
            reason: "execution uses a shell or confirmation-required shell access mode".to_string(),
            risk: ApprovalRisk::High,
            amended_args: None,
            permission_key,
        };
    }

    ExecutionDecision {
        kind: ExecutionDecisionKind::Allowed,
        reason: "execution request is allowed by deterministic sandbox policy".to_string(),
        risk: ApprovalRisk::Low,
        amended_args: None,
        permission_key,
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_test_program() -> String {
        std::env::current_exe()
            .expect("current test executable")
            .display()
            .to_string()
    }

    fn test_helper_args() -> Vec<String> {
        vec!["--help".to_string()]
    }

    fn display_temp_scope(dir: &tempfile::TempDir) -> Vec<String> {
        vec![dir.path().display().to_string()]
    }

    #[test]
    fn permission_key_includes_caller_identity() {
        let mut request =
            ExecutionRequest::local_tool("git", vec!["status".to_string()], "run_shell");
        request.caller.package_id = Some("pkg-a".to_string());
        request.caller.skill_id = Some("skill-a".to_string());

        assert_eq!(
            deterministic_execution_permission_key(&request),
            "exec:run_shell:pkg-a:skill-a:-:git"
        );
    }

    #[test]
    fn deterministic_policy_denies_denied_program() {
        let mut request = ExecutionRequest::local_tool("rm", vec!["-rf".to_string()], "run_shell");
        request.sandbox.denied_programs = vec!["rm".to_string()];

        let decision = review_execution_policy(&request);

        assert_eq!(decision.kind, ExecutionDecisionKind::Denied);
        assert_eq!(decision.risk, ApprovalRisk::High);
    }

    #[test]
    fn deterministic_policy_denies_program_outside_allowed_list() {
        let mut request = ExecutionRequest::local_tool("curl", vec![], "run_shell");
        request.sandbox.allowed_programs = vec!["git".to_string(), "python".to_string()];

        let decision = review_execution_policy(&request);

        assert_eq!(decision.kind, ExecutionDecisionKind::Denied);
        assert_eq!(decision.risk, ApprovalRisk::High);
        assert!(decision.reason.contains("allowed program list"));
    }

    #[test]
    fn deterministic_policy_requires_approval_for_network_intent() {
        let mut request = ExecutionRequest::local_tool(
            "curl",
            vec!["https://example.com".to_string()],
            "run_shell",
        );
        request.network_intent = true;

        let decision = review_execution_policy(&request);

        assert_eq!(decision.kind, ExecutionDecisionKind::RequiresApproval);
        assert_eq!(decision.risk, ApprovalRisk::High);
    }

    #[test]
    fn run_shell_restricted_policy_mirrors_program_whitelist() {
        let request = ExecutionRequest::for_run_shell(
            "git",
            vec!["status".to_string()],
            ShellAccessMode::Restricted,
            vec!["source-1".to_string()],
        );

        assert_eq!(
            request.sandbox.backend,
            ExecutionBackendKind::LocalRestricted
        );
        assert_eq!(
            request.sandbox.allowed_programs,
            crate::tools::run_shell_contract::PROGRAM_WHITELIST
        );
        assert_eq!(request.sandbox.source_scope, vec!["source-1".to_string()]);
        assert!(!request.sandbox.network_allowed);
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn run_shell_open_policy_leaves_programs_unbounded_for_existing_open_mode() {
        let request =
            ExecutionRequest::for_run_shell("curl", vec![], ShellAccessMode::Open, Vec::new());

        assert_eq!(request.sandbox.backend, ExecutionBackendKind::LocalOpen);
        assert!(request.sandbox.allowed_programs.is_empty());
        assert!(request.sandbox.network_allowed);
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn project_tool_policy_carries_source_network_and_timeout() {
        let request = ExecutionRequest::for_project_tool(
            "cargo",
            vec!["--version".to_string()],
            vec!["source-1".to_string()],
            true,
            120,
        );

        assert_eq!(request.caller.tool_name.as_deref(), Some("project_tool"));
        assert_eq!(
            request.sandbox.backend,
            ExecutionBackendKind::LocalRestricted
        );
        assert_eq!(request.sandbox.source_scope, vec!["source-1".to_string()]);
        assert!(request.network_intent);
        assert!(request.sandbox.network_allowed);
        assert_eq!(request.sandbox.timeout_ms, Some(120_000));
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn workflow_action_policy_carries_workflow_package_and_timeout() {
        let request = ExecutionRequest::for_workflow_action(
            "python",
            vec!["workflow.py".to_string()],
            "workflow-1",
            Some("pkg-workflows".to_string()),
            vec!["source-1".to_string()],
            false,
            90,
        );

        assert_eq!(request.caller.tool_name.as_deref(), Some("workflow_action"));
        assert_eq!(request.caller.workflow_id.as_deref(), Some("workflow-1"));
        assert_eq!(request.caller.package_id.as_deref(), Some("pkg-workflows"));
        assert_eq!(request.sandbox.source_scope, vec!["source-1".to_string()]);
        assert!(!request.network_intent);
        assert!(!request.sandbox.network_allowed);
        assert_eq!(request.sandbox.timeout_ms, Some(90_000));
        assert!(request.sandbox.capture_file_changes);
        assert_eq!(
            deterministic_execution_permission_key(&request),
            "exec:workflow_action:pkg-workflows:-:workflow-1:python"
        );
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn connector_helper_policy_scopes_network_and_program() {
        let mut request = ExecutionRequest::for_connector_helper(
            "node",
            vec!["helper.js".to_string()],
            "pkg-connector",
            vec!["source-1".to_string()],
            true,
            30,
        );

        assert_eq!(
            request.caller.tool_name.as_deref(),
            Some("connector_helper")
        );
        assert_eq!(request.caller.package_id.as_deref(), Some("pkg-connector"));
        assert_eq!(request.sandbox.allowed_programs, vec!["node".to_string()]);
        assert!(request.network_intent);
        assert!(request.sandbox.network_allowed);
        assert_eq!(request.sandbox.timeout_ms, Some(30_000));
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );

        request.program = "python".to_string();
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Denied
        );
    }

    #[test]
    fn skill_resource_helper_policy_is_program_scoped_and_network_closed() {
        let mut request = ExecutionRequest::for_skill_resource_helper(
            "python",
            vec!["render_asset.py".to_string()],
            "skill-1",
            vec!["source-1".to_string()],
            20,
        );

        assert_eq!(
            request.caller.tool_name.as_deref(),
            Some("skill_resource_helper")
        );
        assert_eq!(request.caller.skill_id.as_deref(), Some("skill-1"));
        assert_eq!(request.sandbox.allowed_programs, vec!["python".to_string()]);
        assert!(!request.network_intent);
        assert!(!request.sandbox.network_allowed);
        assert!(!request.sandbox.capture_file_changes);
        assert_eq!(request.sandbox.timeout_ms, Some(20_000));
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::Allowed
        );

        request.network_intent = true;
        assert_eq!(
            review_execution_policy(&request).kind,
            ExecutionDecisionKind::RequiresApproval
        );
    }

    #[tokio::test]
    async fn local_process_executes_workflow_action_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = LocalProcessExecutionEnvironment;
        let artifact = environment
            .execute_workflow_action(WorkflowActionExecutionRequest {
                program: current_test_program(),
                args: test_helper_args(),
                cwd: dir.path().to_path_buf(),
                workflow_id: "workflow-1".to_string(),
                package_id: Some("pkg-workflows".to_string()),
                source_scope: display_temp_scope(&dir),
                network_intent: false,
                timeout_secs: 5,
                stdin: None,
                environment: vec![("NEXA_EXEC_TEST".to_string(), "workflow".to_string())],
                expected_writes: vec![dir.path().display().to_string()],
            })
            .await
            .expect("execute workflow action");

        assert_eq!(artifact.decision.kind, ExecutionDecisionKind::Allowed);
        assert_eq!(artifact.exit_status, Some(0));
        assert!(artifact
            .decision
            .permission_key
            .contains("exec:workflow_action:pkg-workflows:-:workflow-1:"));
    }

    #[tokio::test]
    async fn local_process_executes_connector_helper_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = LocalProcessExecutionEnvironment;
        let artifact = environment
            .execute_connector_helper(ConnectorHelperExecutionRequest {
                program: current_test_program(),
                args: test_helper_args(),
                cwd: dir.path().to_path_buf(),
                package_id: "pkg-connector".to_string(),
                source_scope: display_temp_scope(&dir),
                network_intent: true,
                timeout_secs: 5,
                stdin: None,
                environment: Vec::new(),
                expected_writes: Vec::new(),
            })
            .await
            .expect("execute connector helper");

        assert_eq!(artifact.decision.kind, ExecutionDecisionKind::Allowed);
        assert_eq!(artifact.exit_status, Some(0));
        assert!(artifact
            .decision
            .permission_key
            .contains("exec:connector_helper:pkg-connector:-:-:"));
    }

    #[tokio::test]
    async fn local_process_executes_skill_resource_helper_adapter_with_closed_network() {
        let dir = tempfile::tempdir().expect("tempdir");
        let helper_request = SkillResourceHelperExecutionRequest {
            program: current_test_program(),
            args: test_helper_args(),
            cwd: dir.path().to_path_buf(),
            skill_id: "skill-1".to_string(),
            source_scope: display_temp_scope(&dir),
            timeout_secs: 5,
            stdin: None,
            environment: Vec::new(),
            expected_writes: Vec::new(),
        };
        let execution_request = helper_request.clone().into_execution_request();
        assert!(!execution_request.network_intent);
        assert!(!execution_request.sandbox.network_allowed);
        assert_eq!(
            execution_request.caller.tool_name.as_deref(),
            Some("skill_resource_helper")
        );

        let environment = LocalProcessExecutionEnvironment;
        let artifact = environment
            .execute_skill_resource_helper(helper_request)
            .await
            .expect("execute skill resource helper");

        assert_eq!(artifact.decision.kind, ExecutionDecisionKind::Allowed);
        assert_eq!(artifact.exit_status, Some(0));
        assert!(artifact
            .decision
            .permission_key
            .contains("exec:skill_resource_helper:-:skill-1:-:"));
    }
}
