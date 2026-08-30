use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use crate::activity::{
    ActivityEventKind, ActivityRuntime, ActivitySpec, ActivityState, ActivitySurface,
};
use crate::error::CoreError;
use crate::execution_environment::{ExecutionEnvironment, ExecutionRequest};
use async_trait::async_trait;
use regex::Regex;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::super::path_utils::resolve_existing_directory_in_sources;
use super::super::run_shell_contract::{
    expected_format as run_shell_expected_format, invalid_arguments_message,
    tool_description as run_shell_tool_description, DEFAULT_TIMEOUT_SECS, TOOL_NAME,
};
use super::super::{scoped_sources, tool_contract_error_result, Tool, ToolCategory, ToolResult};
use super::environment::{apply_isolated_process_sandbox, LocalRunShellExecutionEnvironment};
use super::file_tracking::{build_run_shell_file_changes, capture_file_snapshot, FileSnapshot};
use super::native_fs::is_native_filesystem_program;
use super::parser::parse_run_shell_args;
use super::policy::{
    normalize_run_shell_invocation, validate_args, validate_scoped_args, validate_stdin,
};
use super::shell_adapter::{
    clamp_timeout, format_confirmation, format_output, format_shell_confirmation,
    parse_shell_selector, spawn_background_process, ProcessTreeGuard, RunShellOutput,
};

pub struct RunShellTool;

/// A conversation-scoped, process-liveness-checked permission for a managed
/// local HTTP service. Consumers must treat this as an ephemeral snapshot and
/// replace previously installed permissions when they refresh it.
#[derive(Debug, Clone)]
pub struct ManagedLoopbackPermit {
    pub service_id: String,
    pub origin: String,
    pub host: String,
    pub port: u16,
    pub process_id: Option<u32>,
    service_instance_id: String,
    lease: Arc<ManagedLoopbackLeaseState>,
}

impl ManagedLoopbackPermit {
    pub fn service_instance_id(&self) -> &str {
        &self.service_instance_id
    }

    pub fn is_live(&self) -> bool {
        self.lease.is_live()
    }
}

impl PartialEq for ManagedLoopbackPermit {
    fn eq(&self, other: &Self) -> bool {
        self.service_id == other.service_id
            && self.service_instance_id == other.service_instance_id
            && self.origin == other.origin
            && self.host == other.host
            && self.port == other.port
            && self.process_id == other.process_id
    }
}

impl Eq for ManagedLoopbackPermit {}

impl std::hash::Hash for ManagedLoopbackPermit {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.service_id.hash(state);
        self.service_instance_id.hash(state);
        self.origin.hash(state);
        self.host.hash(state);
        self.port.hash(state);
        self.process_id.hash(state);
    }
}

#[derive(Debug)]
struct ManagedLoopbackLeaseState {
    live: AtomicBool,
    expires_at: StdMutex<Instant>,
    ttl: Duration,
}

impl ManagedLoopbackLeaseState {
    fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
            && self
                .expires_at
                .lock()
                .map(|expires_at| Instant::now() <= *expires_at)
                .unwrap_or(false)
    }

    fn refresh(&self) {
        if self.live.load(Ordering::Acquire) {
            if let Ok(mut expires_at) = self.expires_at.lock() {
                *expires_at = Instant::now() + self.ttl;
            }
        }
    }

    fn revoke(&self) {
        self.live.store(false, Ordering::Release);
    }
}

/// The managed process owns this issuer; browser consumers receive only
/// cloned permits sharing its renewable liveness lease.
#[derive(Debug, Clone)]
pub struct ManagedLoopbackPermitIssuer {
    service_id: String,
    service_instance_id: String,
    process_id: Option<u32>,
    lease: Arc<ManagedLoopbackLeaseState>,
}

impl ManagedLoopbackPermitIssuer {
    pub fn new(service_id: impl Into<String>, process_id: Option<u32>) -> Self {
        Self::new_with_ttl(service_id, process_id, MANAGED_LOOPBACK_LEASE_TTL)
    }

    #[doc(hidden)]
    pub fn new_with_ttl(
        service_id: impl Into<String>,
        process_id: Option<u32>,
        ttl: Duration,
    ) -> Self {
        let ttl = ttl.max(Duration::from_millis(1));
        Self {
            service_id: service_id.into(),
            service_instance_id: uuid::Uuid::new_v4().to_string(),
            process_id,
            lease: Arc::new(ManagedLoopbackLeaseState {
                live: AtomicBool::new(true),
                expires_at: StdMutex::new(Instant::now() + ttl),
                ttl,
            }),
        }
    }

    pub fn issue(
        &self,
        origin: impl Into<String>,
        host: impl Into<String>,
        port: u16,
    ) -> ManagedLoopbackPermit {
        self.refresh();
        ManagedLoopbackPermit {
            service_id: self.service_id.clone(),
            origin: origin.into(),
            host: host.into(),
            port,
            process_id: self.process_id,
            service_instance_id: self.service_instance_id.clone(),
            lease: Arc::clone(&self.lease),
        }
    }

    pub fn refresh(&self) {
        self.lease.refresh();
    }

    pub fn revoke(&self) {
        self.lease.revoke();
    }
}

const AUTO_SERVICE_SETTLE_MS: u64 = 1_500;
const MAX_SERVICE_LOG_BYTES: usize = 32 * 1024;
const SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SERVICE_LOG_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 3;
const MAX_WAIT_TIMEOUT_SECS: u64 = 3;
const MANAGED_LOOPBACK_LEASE_TTL: Duration = Duration::from_secs(3);

pub(super) fn managed_wait_budget_secs(requested: Option<u64>) -> u64 {
    match requested {
        None | Some(0) => DEFAULT_WAIT_TIMEOUT_SECS,
        Some(seconds) => seconds.clamp(1, MAX_WAIT_TIMEOUT_SECS),
    }
}

#[derive(Clone, Default)]
struct ManagedServiceLogs {
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

struct ManagedService {
    child: tokio::process::Child,
    process_tree: ProcessTreeGuard,
    activity_id: String,
    process_id: Option<u32>,
    program: String,
    ready_url: Option<reqwest::Url>,
    logs: Arc<tokio::sync::Mutex<ManagedServiceLogs>>,
    auto_promoted: bool,
    started_at: Instant,
    activity_runtime: ActivityRuntime,
    cwd: PathBuf,
    before_snapshot: FileSnapshot,
    conversation_id: Option<String>,
    stdout_task: Option<tokio::task::JoinHandle<()>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    loopback_permit_issuer: ManagedLoopbackPermitIssuer,
}

fn new_process_activity_id() -> String {
    format!("process_{}", uuid::Uuid::new_v4())
}

#[derive(Clone)]
struct CompletedService {
    result: ToolResult,
    conversation_id: Option<String>,
}

#[derive(Clone)]
struct FinalizingService {
    conversation_id: Option<String>,
}

static MANAGED_SERVICES: OnceLock<tokio::sync::Mutex<HashMap<String, ManagedService>>> =
    OnceLock::new();
static COMPLETED_SERVICES: OnceLock<tokio::sync::Mutex<HashMap<String, CompletedService>>> =
    OnceLock::new();
static FINALIZING_SERVICES: OnceLock<tokio::sync::Mutex<HashMap<String, FinalizingService>>> =
    OnceLock::new();
static READINESS_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn managed_services() -> &'static tokio::sync::Mutex<HashMap<String, ManagedService>> {
    MANAGED_SERVICES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// Return exact loopback endpoints owned by `conversation_id` whose managed
/// process is still running and has reached a discoverable ready URL.
///
/// The snapshot deliberately excludes unowned, exited, errored and not-yet-
/// ready services. Callers should refresh and replace their prior snapshot
/// rather than accumulating permissions.
pub async fn managed_loopback_permits(conversation_id: &str) -> Vec<ManagedLoopbackPermit> {
    if conversation_id.is_empty() {
        return Vec::new();
    }

    let mut services = managed_services().lock().await;
    let mut permits = services
        .iter_mut()
        .filter_map(|(_service_id, service)| {
            if service.conversation_id.as_deref() != Some(conversation_id) {
                return None;
            }
            if !matches!(service.child.try_wait(), Ok(None)) {
                service.loopback_permit_issuer.revoke();
                return None;
            }
            let ready_url = service.ready_url.as_ref()?;
            Some(service.loopback_permit_issuer.issue(
                ready_url.origin().ascii_serialization(),
                ready_url.host_str()?.to_string(),
                ready_url.port_or_known_default()?,
            ))
        })
        .collect::<Vec<_>>();
    permits.sort_unstable_by(|left, right| left.service_id.cmp(&right.service_id));
    permits
}

fn completed_services() -> &'static tokio::sync::Mutex<HashMap<String, CompletedService>> {
    COMPLETED_SERVICES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn finalizing_services() -> &'static tokio::sync::Mutex<HashMap<String, FinalizingService>> {
    FINALIZING_SERVICES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

pub(super) fn belongs_to_conversation(
    owner: &Option<String>,
    conversation_id: Option<&str>,
) -> bool {
    owner.as_deref() == conversation_id
}

async fn mark_service_finalizing(service_id: &str, service: &ManagedService) {
    finalizing_services().lock().await.insert(
        service_id.to_string(),
        FinalizingService {
            conversation_id: service.conversation_id.clone(),
        },
    );
}

async fn cache_completed_service(
    service_id: &str,
    result: ToolResult,
    conversation_id: Option<String>,
) {
    let mut completed = completed_services().lock().await;
    if completed.len() >= 128 {
        if let Some(evicted) = completed.keys().next().cloned() {
            completed.remove(&evicted);
        }
    }
    completed.insert(
        service_id.to_string(),
        CompletedService {
            result,
            conversation_id,
        },
    );
    drop(completed);
    finalizing_services().lock().await.remove(service_id);
}

async fn inactive_service_result(
    call_id: &str,
    service_id: &str,
    conversation_id: Option<&str>,
) -> Option<ToolResult> {
    if let Some(completed) = completed_services().lock().await.get(service_id).cloned() {
        if !belongs_to_conversation(&completed.conversation_id, conversation_id) {
            return Some(error_result(
                call_id,
                "managed service belongs to a different conversation",
            ));
        }
        let mut result = completed.result;
        result.call_id = call_id.to_string();
        return Some(result);
    }
    let finalizing = finalizing_services()
        .lock()
        .await
        .get(service_id)
        .cloned()?;
    if !belongs_to_conversation(&finalizing.conversation_id, conversation_id) {
        return Some(error_result(
            call_id,
            "managed service belongs to a different conversation",
        ));
    }
    Some(ToolResult {
        call_id: call_id.to_string(),
        content: format!(
            "Managed service {service_id} has exited and is finalizing logs and file changes. Observe it again shortly for the completed result."
        ),
        is_error: false,
        artifacts: Some(serde_json::json!({
            "kind": "managedService",
            "serviceId": service_id,
            "status": "finalizing",
        })),
    })
}

fn spawn_service_monitor(service_id: String) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut services = managed_services().lock().await;
            let status = {
                let Some(service) = services.get_mut(&service_id) else {
                    return;
                };
                service.loopback_permit_issuer.refresh();
                service.child.try_wait()
            };
            match status {
                Ok(Some(status)) => {
                    let service = services
                        .remove(&service_id)
                        .expect("managed service disappeared while locked");
                    service.loopback_permit_issuer.revoke();
                    mark_service_finalizing(&service_id, &service).await;
                    drop(services);
                    let _ =
                        finalize_exited_service(&service_id, &service_id, service, status).await;
                    return;
                }
                Ok(None) => drop(services),
                Err(error) => {
                    let service = services
                        .remove(&service_id)
                        .expect("managed service disappeared while locked");
                    service.loopback_permit_issuer.revoke();
                    mark_service_finalizing(&service_id, &service).await;
                    drop(services);
                    service.process_tree.terminate();
                    let _ = service.activity_runtime.transition(
                        &service.activity_id,
                        ActivityState::Failed,
                        serde_json::json!({ "error": error.to_string() }),
                    );
                    cache_completed_service(
                        &service_id,
                        error_result(
                            &service_id,
                            format!("failed to inspect managed process: {error}"),
                        ),
                        service.conversation_id.clone(),
                    )
                    .await;
                    return;
                }
            }
        }
    });
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

fn validate_ready_url(raw: &str) -> Result<reqwest::Url, String> {
    let raw = raw.trim();
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

fn append_bounded_log(target: &mut String, bytes: &[u8]) -> bool {
    target.push_str(&String::from_utf8_lossy(bytes));
    if target.len() <= MAX_SERVICE_LOG_BYTES {
        return false;
    }
    let mut start = target.len() - MAX_SERVICE_LOG_BYTES;
    while start < target.len() && !target.is_char_boundary(start) {
        start += 1;
    }
    target.drain(..start);
    true
}

fn collect_service_output<R>(
    mut reader: R,
    logs: Arc<tokio::sync::Mutex<ManagedServiceLogs>>,
    stdout: bool,
    activity_runtime: ActivityRuntime,
    activity_id: String,
) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0_u8; 2_048];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let mut logs = logs.lock().await;
                    if stdout {
                        if append_bounded_log(&mut logs.stdout, &buffer[..read]) {
                            logs.stdout_truncated = true;
                        }
                    } else if append_bounded_log(&mut logs.stderr, &buffer[..read]) {
                        logs.stderr_truncated = true;
                    }
                    drop(logs);
                    let kind = if stdout {
                        ActivityEventKind::StdoutChunk
                    } else {
                        ActivityEventKind::StderrChunk
                    };
                    let _ = activity_runtime.append(
                        &activity_id,
                        kind,
                        serde_json::json!({ "data": data }),
                    );
                }
            }
        }
    })
}

fn local_url_regex() -> &'static Regex {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    URL_RE.get_or_init(|| {
        Regex::new(
            r#"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])(?::\d{1,5})?(?:/[^\s\"'<>]*)?"#,
        )
        .expect("valid managed-service URL regex")
    })
}

fn discover_ready_url(stdout: &str, stderr: &str) -> Option<reqwest::Url> {
    local_url_regex()
        .find_iter(&format!("{stdout}\n{stderr}"))
        .filter_map(|matched| {
            let candidate = matched
                .as_str()
                .trim_end_matches(['.', ',', ')', ']', ';'])
                .replace("0.0.0.0", "127.0.0.1")
                .replace("[::]", "[::1]");
            validate_ready_url(&candidate).ok()
        })
        .next()
}

async fn service_log_snapshot(
    logs: &Arc<tokio::sync::Mutex<ManagedServiceLogs>>,
) -> ManagedServiceLogs {
    let logs = logs.lock().await;
    logs.clone()
}

async fn await_service_output_task(task: Option<tokio::task::JoinHandle<()>>, timeout: Duration) {
    let Some(mut task) = task else {
        return;
    };
    if tokio::time::timeout(timeout, &mut task).await.is_err() {
        task.abort();
        let _ = task.await;
    }
}

async fn drain_service_output_tasks(
    stdout_task: Option<tokio::task::JoinHandle<()>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    timeout: Duration,
) {
    tokio::join!(
        await_service_output_task(stdout_task, timeout),
        await_service_output_task(stderr_task, timeout),
    );
}

async fn readiness_probe(url: &reqwest::Url) -> bool {
    readiness_client().get(url.clone()).send().await.is_ok()
}

#[cfg(test)]
fn launches_python_server_script(program: &str, args: &[String]) -> bool {
    let is_python_launcher = matches!(program, "python" | "python3" | "py")
        || program
            .strip_prefix("python3.")
            .is_some_and(|version| version.chars().all(|ch| ch.is_ascii_digit()));
    if !is_python_launcher {
        return false;
    }

    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "-c" | "-m" => return false,
            "-w" | "-x" | "--check-hash-based-pycs" => index += 2,
            "--" => {
                return args
                    .get(index + 1)
                    .and_then(|value| value.rsplit(['/', '\\']).next())
                    .is_some_and(|name| name == "server.py");
            }
            value if value.starts_with('-') => index += 1,
            value => {
                return value
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|name| name == "server.py");
            }
        }
    }
    false
}

#[cfg(test)]
pub(super) fn looks_like_persistent_service(program: &str, args: &[String]) -> bool {
    let program = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let normalized_args: Vec<String> = args.iter().map(|arg| arg.to_ascii_lowercase()).collect();
    let joined = normalized_args.join(" ");

    joined.contains("http.server")
        || launches_python_server_script(&program, &normalized_args)
        || joined.contains("manage.py runserver")
        || joined.contains("uvicorn")
        || joined.contains("flask run")
        || joined.contains("fastapi dev")
        || joined.contains("fastapi run")
        || joined.contains("streamlit run")
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

struct ManagedServiceRequest<'a> {
    call_id: &'a str,
    program: &'a str,
    args: &'a [String],
    cwd: &'a Path,
    requested_ready_url: Option<reqwest::Url>,
    auto_promoted: bool,
    activity_runtime: ActivityRuntime,
    conversation_id: Option<&'a str>,
    before_snapshot: FileSnapshot,
}

async fn start_managed_service(request: ManagedServiceRequest<'_>) -> ToolResult {
    let ManagedServiceRequest {
        call_id,
        program,
        args,
        cwd,
        requested_ready_url,
        auto_promoted,
        activity_runtime,
        conversation_id,
        before_snapshot,
    } = request;
    if let Some(ready_url) = requested_ready_url.as_ref() {
        if readiness_probe(ready_url).await {
            return error_result(
                call_id,
                format!(
                    "ready_url {ready_url} is already responding; choose a free port or check the existing service before starting another process"
                ),
            );
        }
    }

    let activity_id = new_process_activity_id();
    let mut activity_spec = ActivitySpec::new(ActivitySurface::Process, TOOL_NAME)
        .with_activity_id(&activity_id)
        .with_cwd(cwd.display().to_string());
    if let Some(conversation_id) = conversation_id {
        activity_spec = activity_spec.with_conversation_id(conversation_id);
    }
    if let Err(error) = activity_runtime.start(activity_spec) {
        return error_result(
            call_id,
            format!("failed to start process activity: {error}"),
        );
    }

    let (mut child, process_tree) = match spawn_background_process(program, args, cwd) {
        Ok(spawned) => spawned,
        Err(error) => {
            let _ = activity_runtime.transition(
                &activity_id,
                ActivityState::Failed,
                serde_json::json!({ "error": error }),
            );
            return error_result(call_id, error);
        }
    };
    let logs = Arc::new(tokio::sync::Mutex::new(ManagedServiceLogs::default()));
    let process_id = child.id();
    let loopback_permit_issuer = ManagedLoopbackPermitIssuer::new(call_id, process_id);
    let _ = activity_runtime.append(
        &activity_id,
        ActivityEventKind::CommandStarted,
        serde_json::json!({
            "program": program,
            "args": args,
            "processId": process_id,
        }),
    );
    let stdout_task = child.stdout.take().map(|stdout| {
        collect_service_output(
            stdout,
            Arc::clone(&logs),
            true,
            activity_runtime.clone(),
            activity_id.clone(),
        )
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        collect_service_output(
            stderr,
            Arc::clone(&logs),
            false,
            activity_runtime.clone(),
            activity_id.clone(),
        )
    });
    let started_at = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The leader may have exited while descendants still hold its pipes.
                // Terminate the whole tree before draining so finalization cannot hang.
                process_tree.terminate();
                drain_service_output_tasks(stdout_task, stderr_task, SERVICE_LOG_DRAIN_TIMEOUT)
                    .await;
                let log_snapshot = service_log_snapshot(&logs).await;
                let service = ManagedService {
                    child,
                    process_tree,
                    activity_id,
                    process_id,
                    program: program.to_string(),
                    ready_url: requested_ready_url,
                    logs,
                    auto_promoted,
                    started_at,
                    activity_runtime,
                    cwd: cwd.to_path_buf(),
                    before_snapshot,
                    conversation_id: conversation_id.map(str::to_string),
                    stdout_task: None,
                    stderr_task: None,
                    loopback_permit_issuer,
                };
                return exited_service_result(call_id, call_id, &service, status, &log_snapshot)
                    .await;
            }
            Ok(None) => {}
            Err(error) => {
                process_tree.terminate();
                let _ = child.kill().await;
                let _ = activity_runtime.transition(
                    &activity_id,
                    ActivityState::Failed,
                    serde_json::json!({ "error": error.to_string() }),
                );
                return error_result(
                    call_id,
                    format!("failed to inspect background service: {error}"),
                );
            }
        }

        let log_snapshot = service_log_snapshot(&logs).await;
        let ready_url = requested_ready_url
            .clone()
            .or_else(|| discover_ready_url(&log_snapshot.stdout, &log_snapshot.stderr));
        if let Some(ready_url) = ready_url.as_ref() {
            if readiness_probe(ready_url).await {
                let _ = activity_runtime.append(
                    &activity_id,
                    ActivityEventKind::ReadyUrl,
                    serde_json::json!({ "url": ready_url.as_str() }),
                );
                let _ = activity_runtime.transition(
                    &activity_id,
                    ActivityState::Ready,
                    serde_json::json!({ "url": ready_url.as_str() }),
                );
                managed_services().lock().await.insert(
                    call_id.to_string(),
                    ManagedService {
                        child,
                        process_tree,
                        activity_id: activity_id.clone(),
                        process_id,
                        program: program.to_string(),
                        ready_url: Some(ready_url.clone()),
                        logs,
                        auto_promoted,
                        started_at,
                        activity_runtime: activity_runtime.clone(),
                        cwd: cwd.to_path_buf(),
                        before_snapshot,
                        conversation_id: conversation_id.map(str::to_string),
                        stdout_task,
                        stderr_task,
                        loopback_permit_issuer,
                    },
                );
                spawn_service_monitor(call_id.to_string());
                return ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "Background service is ready at {ready_url}. service_id: {call_id}; process_id: {}. Recheck with service_action=status, block on completion with service_action=wait, and stop it with service_action=stop when finished.",
                    process_id.map_or_else(|| "unknown".to_string(), |id| id.to_string()),
                ),
                is_error: false,
                artifacts: Some(serde_json::json!({
                    "kind": "managedService",
                    "activityId": activity_id,
                    "cursor": activity_runtime.get(&activity_id).map(|record| record.last_event_seq),
                    "serviceId": call_id,
                    "processId": process_id,
                    "status": "ready",
                    "readyUrl": ready_url.as_str(),
                    "program": program,
                    "autoPromoted": auto_promoted,
                    "stdoutTail": log_snapshot.stdout,
                    "stderrTail": log_snapshot.stderr,
                    "stdoutTruncated": log_snapshot.stdout_truncated,
                    "stderrTruncated": log_snapshot.stderr_truncated,
                })),
                };
            }
        }

        if started_at.elapsed() >= Duration::from_millis(AUTO_SERVICE_SETTLE_MS) {
            managed_services().lock().await.insert(
                call_id.to_string(),
                ManagedService {
                    child,
                    process_tree,
                    activity_id: activity_id.clone(),
                    process_id,
                    program: program.to_string(),
                    ready_url: ready_url.clone(),
                    logs,
                    auto_promoted,
                    started_at,
                    activity_runtime: activity_runtime.clone(),
                    cwd: cwd.to_path_buf(),
                    before_snapshot,
                    conversation_id: conversation_id.map(str::to_string),
                    stdout_task,
                    stderr_task,
                    loopback_permit_issuer,
                },
            );
            spawn_service_monitor(call_id.to_string());
            return ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "Long-running command is now a managed background service. service_id: {call_id}; process_id: {}. {} The command call is complete; keep working and poll with service_action=wait to be handed the exit status and logs as soon as it finishes, service_action=status for a snapshot, and service_action=stop to end it. Continue with browser_evidence_capture when a URL is available.",
                    process_id.map_or_else(|| "unknown".to_string(), |id| id.to_string()),
                    ready_url.as_ref().map_or_else(
                        || "No loopback URL was discovered yet; status will keep checking startup logs.".to_string(),
                        |url| format!("Discovered URL: {url}."),
                    ),
                ),
                is_error: false,
                artifacts: Some(serde_json::json!({
                    "kind": "managedService",
                    "activityId": activity_id,
                    "cursor": activity_runtime.get(&activity_id).map(|record| record.last_event_seq),
                    "serviceId": call_id,
                    "processId": process_id,
                    "status": "running",
                    "readyUrl": ready_url.as_ref().map(reqwest::Url::as_str),
                    "program": program,
                    "autoPromoted": auto_promoted,
                    "stdoutTail": log_snapshot.stdout,
                    "stderrTail": log_snapshot.stderr,
                    "stdoutTruncated": log_snapshot.stdout_truncated,
                    "stderrTruncated": log_snapshot.stderr_truncated,
                })),
            };
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn status_service(
    call_id: &str,
    service_id: &str,
    conversation_id: Option<&str>,
) -> ToolResult {
    let mut registry = managed_services().lock().await;
    let Some(service) = registry.get_mut(service_id) else {
        drop(registry);
        if let Some(result) = inactive_service_result(call_id, service_id, conversation_id).await {
            return result;
        }
        return error_result(
            call_id,
            format!("managed service '{service_id}' was not found or has already stopped"),
        );
    };
    if !belongs_to_conversation(&service.conversation_id, conversation_id) {
        return error_result(
            call_id,
            "managed service belongs to a different conversation",
        );
    }
    service.loopback_permit_issuer.refresh();
    let process_status = service.child.try_wait();
    match process_status {
        Ok(Some(status)) => {
            let service = registry
                .remove(service_id)
                .expect("managed service disappeared while locked");
            service.loopback_permit_issuer.revoke();
            mark_service_finalizing(service_id, &service).await;
            drop(registry);
            finalize_exited_service(call_id, service_id, service, status).await
        }
        Err(error) => {
            service.loopback_permit_issuer.revoke();
            error_result(
                call_id,
                format!("failed to inspect service {service_id}: {error}"),
            )
        }
        Ok(None) => {
            let log_snapshot = service_log_snapshot(&service.logs).await;
            if service.ready_url.is_none() {
                service.ready_url = discover_ready_url(&log_snapshot.stdout, &log_snapshot.stderr);
            }
            let ready_url = service.ready_url.clone();
            let process_id = service.process_id;
            let program = service.program.clone();
            let auto_promoted = service.auto_promoted;
            let uptime_ms = service.started_at.elapsed().as_millis() as u64;
            let activity_id = service.activity_id.clone();
            let cursor = service
                .activity_runtime
                .get(&activity_id)
                .map(|record| record.last_event_seq);
            drop(registry);
            let healthy = match ready_url.as_ref() {
                Some(url) => Some(readiness_probe(url).await),
                None => None,
            };
            ToolResult {
                call_id: call_id.to_string(),
                content: match (healthy, ready_url.as_ref()) {
                    (Some(true), Some(url)) => format!(
                        "Managed service {service_id} is running and healthy at {url}."
                    ),
                    (Some(false), Some(url)) => format!(
                        "Managed service {service_id} is still running, but {url} is not responding.\nstdout tail:\n{}\nstderr tail:\n{}",
                        log_snapshot.stdout,
                        log_snapshot.stderr,
                    ),
                    _ => format!(
                        "Managed service {service_id} is running, but no loopback URL has appeared in its logs yet.\nstdout tail:\n{}\nstderr tail:\n{}",
                        log_snapshot.stdout,
                        log_snapshot.stderr,
                    ),
                },
                is_error: healthy == Some(false),
                artifacts: Some(serde_json::json!({
                    "kind": "managedService",
                    "activityId": activity_id,
                    "cursor": cursor,
                    "serviceId": service_id,
                    "processId": process_id,
                    "status": match healthy { Some(true) => "ready", Some(false) => "unhealthy", None => "running" },
                    "readyUrl": ready_url.as_ref().map(reqwest::Url::as_str),
                    "program": program,
                    "autoPromoted": auto_promoted,
                    "uptimeMs": uptime_ms,
                    "stdoutTail": log_snapshot.stdout,
                    "stderrTail": log_snapshot.stderr,
                    "stdoutTruncated": log_snapshot.stdout_truncated,
                    "stderrTruncated": log_snapshot.stderr_truncated,
                })),
            }
        }
    }
}

async fn manage_service(
    call_id: &str,
    action: &str,
    service_id: &str,
    conversation_id: Option<&str>,
) -> ToolResult {
    if action == "status" {
        return status_service(call_id, service_id, conversation_id).await;
    }
    if action != "stop" {
        return error_result(call_id, format!("unsupported service action '{action}'"));
    }

    let mut registry = managed_services().lock().await;
    let Some(mut service) = registry.remove(service_id) else {
        drop(registry);
        if let Some(result) = inactive_service_result(call_id, service_id, conversation_id).await {
            return result;
        }
        return error_result(
            call_id,
            format!("managed service '{service_id}' was not found or has already stopped"),
        );
    };
    if !belongs_to_conversation(&service.conversation_id, conversation_id) {
        registry.insert(service_id.to_string(), service);
        return error_result(
            call_id,
            "managed service belongs to a different conversation",
        );
    }
    service.loopback_permit_issuer.revoke();
    mark_service_finalizing(service_id, &service).await;
    drop(registry);

    service.process_tree.terminate();
    let kill_error = service.child.kill().await.err();
    let _ = service.child.wait().await;
    drain_service_output_tasks(
        service.stdout_task.take(),
        service.stderr_task.take(),
        SERVICE_LOG_DRAIN_TIMEOUT,
    )
    .await;
    let log_snapshot = service_log_snapshot(&service.logs).await;
    let _ = service.activity_runtime.transition(
        &service.activity_id,
        ActivityState::Cancelled,
        serde_json::json!({ "reason": "stopped_by_tool" }),
    );
    let result = ToolResult {
        call_id: call_id.to_string(),
        content: kill_error.map_or_else(
            || format!("Stopped managed service {service_id}."),
            |error| format!("Managed service {service_id} had already exited: {error}"),
        ),
        is_error: false,
        artifacts: Some(serde_json::json!({
            "kind": "managedService",
            "activityId": service.activity_id,
            "cursor": service.activity_runtime.get(&service.activity_id).map(|record| record.last_event_seq),
            "serviceId": service_id,
            "processId": service.process_id,
            "status": "stopped",
            "readyUrl": service.ready_url.as_ref().map(reqwest::Url::as_str),
            "program": service.program,
            "autoPromoted": service.auto_promoted,
            "stdoutTail": log_snapshot.stdout,
            "stderrTail": log_snapshot.stderr,
            "stdoutTruncated": log_snapshot.stdout_truncated,
            "stderrTruncated": log_snapshot.stderr_truncated,
        })),
    };
    cache_completed_service(service_id, result.clone(), service.conversation_id.clone()).await;
    result
}

async fn exited_service_result(
    call_id: &str,
    service_id: &str,
    service: &ManagedService,
    status: std::process::ExitStatus,
    logs: &ManagedServiceLogs,
) -> ToolResult {
    let exit_code = status.code();
    let state = if status.success() {
        ActivityState::Completed
    } else {
        ActivityState::Failed
    };
    let _ = service.activity_runtime.append(
        &service.activity_id,
        ActivityEventKind::CommandFinished,
        serde_json::json!({ "exitCode": exit_code }),
    );
    let _ = service.activity_runtime.transition(
        &service.activity_id,
        state,
        serde_json::json!({ "exitCode": exit_code }),
    );

    let after_root = service.cwd.clone();
    let after_snapshot = tokio::task::spawn_blocking(move || capture_file_snapshot(&after_root))
        .await
        .ok();
    let file_changes = after_snapshot.as_ref().and_then(|after_snapshot| {
        build_run_shell_file_changes(&service.cwd, &service.before_snapshot, after_snapshot)
    });
    let output = RunShellOutput {
        exit_code,
        stdout: logs.stdout.clone(),
        stderr: logs.stderr.clone(),
        duration_ms: service.started_at.elapsed().as_millis(),
        truncated_stdout: logs.stdout_truncated,
        truncated_stderr: logs.stderr_truncated,
        killed_by_timeout: false,
    };
    let mut content = format_output(&output);
    if let Some(changes) = &file_changes {
        content.push_str("\n── file changes ──\n");
        content.push_str(&changes.summary);
        content.push('\n');
    }
    let mut artifacts = file_changes
        .map(|changes| changes.artifact)
        .unwrap_or_else(|| serde_json::json!({ "kind": "managedService" }));
    if let Some(object) = artifacts.as_object_mut() {
        object.insert(
            "activityId".to_string(),
            serde_json::json!(service.activity_id),
        );
        object.insert(
            "cursor".to_string(),
            serde_json::json!(service
                .activity_runtime
                .get(&service.activity_id)
                .map(|record| record.last_event_seq)),
        );
        object.insert(
            "execution".to_string(),
            serde_json::json!({
                "exitCode": exit_code,
                "durationMs": output.duration_ms as u64,
                "timedOut": false,
            }),
        );
        object.insert("serviceId".to_string(), serde_json::json!(service_id));
        object.insert(
            "processId".to_string(),
            serde_json::json!(service.process_id),
        );
        object.insert("status".to_string(), serde_json::json!("exited"));
        object.insert("program".to_string(), serde_json::json!(service.program));
        object.insert(
            "autoPromoted".to_string(),
            serde_json::json!(service.auto_promoted),
        );
    }
    ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: !status.success(),
        artifacts: Some(artifacts),
    }
}

async fn finalize_exited_service(
    call_id: &str,
    service_id: &str,
    mut service: ManagedService,
    status: std::process::ExitStatus,
) -> ToolResult {
    // A descendant can outlive the leader while retaining inherited stdout or
    // stderr. Kill the process tree first, then bound the pipe-drain wait.
    service.process_tree.terminate();
    drain_service_output_tasks(
        service.stdout_task.take(),
        service.stderr_task.take(),
        SERVICE_LOG_DRAIN_TIMEOUT,
    )
    .await;
    let log_snapshot = service_log_snapshot(&service.logs).await;
    let stored_result =
        exited_service_result(service_id, service_id, &service, status, &log_snapshot).await;
    cache_completed_service(
        service_id,
        stored_result.clone(),
        service.conversation_id.clone(),
    )
    .await;
    let mut result = stored_result;
    result.call_id = call_id.to_string();
    result
}

/// Poll a managed service until it exits or the wait budget runs out.
///
/// This is the "check back in a moment" loop: the agent gets the final exit
/// status and logs as soon as the process finishes instead of guessing how long
/// to block, and gets a still-running snapshot when the budget elapses so it can
/// keep working and poll again later.
async fn wait_for_service(
    call_id: &str,
    service_id: &str,
    wait_timeout_secs: u64,
    conversation_id: Option<&str>,
) -> ToolResult {
    let deadline = Instant::now() + Duration::from_secs(wait_timeout_secs);
    loop {
        let mut finalizing_result = None;
        {
            let mut registry = managed_services().lock().await;
            match registry.remove(service_id) {
                None => {
                    drop(registry);
                    let Some(result) =
                        inactive_service_result(call_id, service_id, conversation_id).await
                    else {
                        return error_result(
                            call_id,
                            format!(
                                "managed service '{service_id}' was not found or has already stopped"
                            ),
                        );
                    };
                    let is_finalizing = result
                        .artifacts
                        .as_ref()
                        .and_then(|artifacts| artifacts.get("status"))
                        .and_then(serde_json::Value::as_str)
                        == Some("finalizing");
                    if is_finalizing {
                        finalizing_result = Some(result);
                    } else {
                        return result;
                    }
                }
                Some(mut service) => {
                    if !belongs_to_conversation(&service.conversation_id, conversation_id) {
                        registry.insert(service_id.to_string(), service);
                        return error_result(
                            call_id,
                            "managed service belongs to a different conversation",
                        );
                    }
                    service.loopback_permit_issuer.refresh();
                    match service.child.try_wait() {
                        Ok(Some(status)) => {
                            service.loopback_permit_issuer.revoke();
                            mark_service_finalizing(service_id, &service).await;
                            drop(registry);
                            return finalize_exited_service(call_id, service_id, service, status)
                                .await;
                        }
                        Err(error) => {
                            service.loopback_permit_issuer.revoke();
                            registry.insert(service_id.to_string(), service);
                            return error_result(
                                call_id,
                                format!("failed to inspect service {service_id}: {error}"),
                            );
                        }
                        Ok(None) => {
                            registry.insert(service_id.to_string(), service);
                        }
                    }
                }
            }
        }

        if Instant::now() >= deadline {
            let mut result = match finalizing_result {
                Some(result) => result,
                None => manage_service(call_id, "status", service_id, conversation_id).await,
            };
            result.content = format!(
                "Still running after waiting {wait_timeout_secs}s. Continue with other work and poll again with service_action=\"wait\" or service_action=\"status\"; use service_action=\"stop\" to end it.\n{}",
                result.content
            );
            return result;
        }
        tokio::time::sleep(SERVICE_POLL_INTERVAL).await;
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

#[cfg(test)]
mod review_regression_tests {
    use super::*;

    #[test]
    fn bounded_service_logs_report_when_the_head_is_discarded() {
        let mut output = String::new();
        assert!(!append_bounded_log(&mut output, b"short output"));
        assert!(append_bounded_log(
            &mut output,
            &vec![b'x'; MAX_SERVICE_LOG_BYTES + 128]
        ));
        assert!(output.len() <= MAX_SERVICE_LOG_BYTES);
    }

    #[test]
    fn process_activity_ids_are_independent_of_provider_call_ids() {
        let first = new_process_activity_id();
        let second = new_process_activity_id();

        assert!(first.starts_with("process_"));
        assert_ne!(first, second);
        assert_ne!(first, "call_0");
    }

    #[tokio::test]
    async fn finalizing_services_remain_discoverable_until_completion_is_cached() {
        let service_id = format!("finalizing-test-{}", uuid::Uuid::new_v4());
        let conversation_id = Some("conversation-1".to_string());
        finalizing_services().lock().await.insert(
            service_id.clone(),
            FinalizingService {
                conversation_id: conversation_id.clone(),
            },
        );

        let finalizing =
            inactive_service_result("status-call", &service_id, Some("conversation-1"))
                .await
                .expect("finalizing service should remain discoverable");
        assert!(!finalizing.is_error);
        assert_eq!(
            finalizing.artifacts.as_ref().unwrap()["status"],
            "finalizing"
        );

        let completed_result = ToolResult {
            call_id: service_id.clone(),
            content: "completed".to_string(),
            is_error: false,
            artifacts: Some(serde_json::json!({ "status": "exited" })),
        };
        cache_completed_service(&service_id, completed_result, conversation_id).await;

        assert!(!finalizing_services().lock().await.contains_key(&service_id));
        let completed =
            inactive_service_result("later-status-call", &service_id, Some("conversation-1"))
                .await
                .expect("completed service should remain discoverable");
        assert_eq!(completed.call_id, "later-status-call");
        assert_eq!(completed.content, "completed");
        completed_services().lock().await.remove(&service_id);
    }

    #[tokio::test]
    async fn log_drain_aborts_pipe_collectors_that_never_reach_eof() {
        let blocked = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        drain_service_output_tasks(Some(blocked), None, Duration::from_millis(10)).await;
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
        &[ToolCategory::Process]
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
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id,
            activity_runtime,
            ..
        } = context;
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
        let isolation_sandbox = parsed.isolation_sandbox.clone();
        let process_isolation_enabled = isolation_sandbox.is_some();
        if let Some(ready_timeout_secs) = parsed.ready_timeout_secs {
            tracing::debug!(
                ready_timeout_secs,
                "run_shell ignored deprecated ready_timeout_secs; readiness is activity-driven"
            );
        }
        let service_action = parsed
            .service_action
            .as_deref()
            .unwrap_or("run")
            .trim()
            .to_ascii_lowercase();
        if !matches!(service_action.as_str(), "run" | "status" | "wait" | "stop") {
            return Ok(error_result(
                call_id,
                "run_shell service_action must be run, status, wait, or stop",
            ));
        }
        if service_action != "run" {
            if isolation_sandbox.is_some() {
                return Ok(error_result(
                    call_id,
                    "Code Ultra isolation cannot manage a process created outside this sandboxed call.",
                ));
            }
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
            if service_action == "wait" {
                let wait_timeout_secs = managed_wait_budget_secs(parsed.timeout_secs);
                return Ok(wait_for_service(
                    call_id,
                    service_id,
                    wait_timeout_secs,
                    conversation_id,
                )
                .await);
            }
            return Ok(manage_service(call_id, &service_action, service_id, conversation_id).await);
        }
        if parsed.service_id.is_some() {
            return Ok(error_result(
                call_id,
                "service_id is only valid with service_action=status, wait, or stop",
            ));
        }
        if isolation_sandbox.is_some() && parsed.background {
            return Ok(error_result(
                call_id,
                "Code Ultra isolation does not allow detached processes.",
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
        let auto_promoted = isolation_sandbox.is_none()
            && !parsed.background
            && parsed.stdin.is_none()
            && !is_native_filesystem_program(&canonical_program);
        let managed_background =
            isolation_sandbox.is_none() && (parsed.background || auto_promoted);
        if managed_background && parsed.stdin.is_some() {
            return Ok(error_result(
                call_id,
                "background run_shell does not accept stdin",
            ));
        }
        let ready_url = if managed_background {
            match parsed.ready_url.as_deref() {
                Some(raw) => match validate_ready_url(raw) {
                    Ok(url) => Some(url),
                    Err(message) => return Ok(error_result(call_id, message)),
                },
                None => None,
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

        let before_root = cwd_path.clone();
        let before_snapshot =
            tokio::task::spawn_blocking(move || capture_file_snapshot(&before_root))
                .await
                .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;

        if managed_background {
            return Ok(start_managed_service(ManagedServiceRequest {
                call_id,
                program: &canonical_program,
                args: &normalized_args,
                cwd: &cwd_path,
                requested_ready_url: ready_url,
                auto_promoted,
                activity_runtime: activity_runtime.cloned().unwrap_or_default(),
                conversation_id,
                before_snapshot,
            })
            .await);
        }

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
        if let Some(isolation) = isolation_sandbox {
            apply_isolated_process_sandbox(
                &mut execution_request,
                Path::new(&isolation.worktree_root),
                &cwd_path,
            )?;
        }
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
                    "program": canonical_program,
                    "args": normalized_args,
                    "cwd": cwd_path,
                    "filesystemSandboxed": process_isolation_enabled,
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
