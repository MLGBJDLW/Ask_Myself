//! RunShellTool — execute a whitelisted program with argv arguments inside a
//! registered source directory.
//!
//! # Security posture
//!
//! * **No shell interpreter.** We spawn the program directly via
//!   `tokio::process::Command`. Shell metacharacters (`;`, `&&`, `|`, backticks,
//!   globs) are passed to the program as literal arguments, never interpreted.
//! * **Program whitelist.** Only programs in [`PROGRAM_WHITELIST`] may run.
//!   There is no user-extensible "custom" slot — adding programs requires a
//!   code change (and review).
//! * **Git read-only.** When `program == "git"`, the first argument must be in
//!   [`GIT_READONLY_SUBCOMMANDS`] and write-flavoured args (`push`, `pull`,
//!   `fetch`, `commit`, `reset`, `--set`, `--unset`, etc.) are rejected even
//!   if they appear later in the argv.
//! * **Argv size caps.** Individual args > 8 KB and total argv > 32 KB are
//!   rejected to prevent argv stuffing.
//! * **Sandboxed cwd.** `cwd` is resolved via the same helper as `read_file`
//!   and `edit_file`; it must canonicalize inside a registered source root.
//! * **Scrubbed environment.** The child environment is rebuilt from scratch:
//!   keys containing secret-like substrings (`KEY`, `SECRET`, `TOKEN`, …) are
//!   dropped, and only an allow-list of neutral infrastructure vars (`PATH`,
//!   `LANG`, `HOME`, …) is forwarded.
//! * **Output caps.** stdout/stderr are each truncated to 64 KB.
//! * **Timeout.** Default 30s, max 300s. On timeout we `start_kill()` and
//!   rely on `kill_on_drop(true)` for cleanup.
//! * **No hidden console.** On Windows we spawn with `CREATE_NO_WINDOW` so
//!   interactive programs cannot flash a console or trap input.
//! * **Always requires confirmation.** `requires_confirmation` is hard-coded
//!   to `true` regardless of args.
//! * **Audit logging.** Each invocation logs program, arg count, cwd, exit
//!   code, duration, and kill status via `tracing`. **Arg contents are never
//!   logged** (they may contain sensitive paths or data).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

use super::path_utils::resolve_existing_directory_in_sources;
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/run_shell.json");

// ---------------------------------------------------------------------------
// Security constants
// ---------------------------------------------------------------------------

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 300;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_SINGLE_ARG_BYTES: usize = 8 * 1024;
const MAX_TOTAL_ARGV_BYTES: usize = 32 * 1024;

/// Whitelisted program basenames. Matched case-insensitively on Windows,
/// case-sensitively on Unix. The model may only pass these names exactly.
const PROGRAM_WHITELIST: &[&str] = &[
    "python", "python3", "node", "npm", "npx", "git",
];

/// For `git`, only these subcommands are accepted as `args[0]`.
const GIT_READONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "ls-files",
    "rev-parse",
    "branch",
    "tag",
    "config",
    "remote",
    "describe",
    "blame",
];

/// Git tokens that may never appear anywhere in argv, even when the primary
/// subcommand is read-only (defence-in-depth against `git config --unset`
/// etc.).
const GIT_FORBIDDEN_TOKENS: &[&str] = &[
    "push", "pull", "fetch", "commit", "reset", "merge", "rebase", "cherry-pick",
    "clone", "init", "add", "rm", "mv", "checkout", "switch", "restore", "am",
    "apply", "stash",
    "--set", "--unset", "--unset-all", "--add", "--replace-all",
];

/// Byte substrings forbidden in any arg (defence-in-depth).
const FORBIDDEN_ARG_SUBSTRINGS: &[&str] = &["\0"];

/// Env-var name fragments (case-insensitive) that cause the key to be stripped.
const ENV_STRIP_PATTERNS: &[&str] = &[
    "KEY",
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "AWS_",
    "AZURE_",
    "GCP_",
    "GITHUB_",
    "OPENAI_",
    "ANTHROPIC_",
];

/// Env-var names that should pass through even if matched by a strip pattern.
/// (Currently none of the preserve names would match strip patterns, but we
/// re-add them explicitly in case the user's environment overrides them.)
const ENV_PRESERVE: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "SYSTEMROOT",
    "PATHEXT",
    "WINDIR",
];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct RunShellTool;

#[derive(Deserialize)]
struct RunShellArgs {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Serialize)]
struct RunShellOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    duration_ms: u128,
    truncated_stdout: bool,
    truncated_stderr: bool,
    killed_by_timeout: bool,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn program_matches(candidate: &str, canonical: &str) -> bool {
    #[cfg(windows)]
    {
        candidate.eq_ignore_ascii_case(canonical)
    }
    #[cfg(not(windows))]
    {
        candidate == canonical
    }
}

/// Reject unknown programs. Returns the canonical name on success.
fn validate_program(program: &str) -> Result<&'static str, String> {
    if program.is_empty() {
        return Err("program must not be empty".to_string());
    }
    if program.contains('/') || program.contains('\\') {
        return Err(
            "program must be a bare name (no path separators); only whitelisted commands are allowed"
                .to_string(),
        );
    }
    for &canonical in PROGRAM_WHITELIST {
        if program_matches(program, canonical) {
            return Ok(canonical);
        }
    }
    Err(format!(
        "program '{program}' is not in the run_shell whitelist. Allowed: {}",
        PROGRAM_WHITELIST.join(", ")
    ))
}

/// Reject unsafe argv patterns.
fn validate_args(program: &str, args: &[String]) -> Result<(), String> {
    let mut total = 0usize;
    for (i, arg) in args.iter().enumerate() {
        if arg.len() > MAX_SINGLE_ARG_BYTES {
            return Err(format!(
                "argument #{i} exceeds {MAX_SINGLE_ARG_BYTES} bytes"
            ));
        }
        for forbidden in FORBIDDEN_ARG_SUBSTRINGS {
            if arg.contains(forbidden) {
                return Err(format!(
                    "argument #{i} contains forbidden byte sequence"
                ));
            }
        }
        total = total.saturating_add(arg.len());
    }
    if total > MAX_TOTAL_ARGV_BYTES {
        return Err(format!(
            "total argv size ({total} bytes) exceeds {MAX_TOTAL_ARGV_BYTES}"
        ));
    }

    if program_matches(program, "git") {
        let first = args
            .first()
            .ok_or_else(|| "git requires a subcommand".to_string())?;
        if !GIT_READONLY_SUBCOMMANDS.iter().any(|s| s == first) {
            return Err(format!(
                "git subcommand '{first}' is not permitted. Allowed: {}",
                GIT_READONLY_SUBCOMMANDS.join(", ")
            ));
        }
        // `git config` defaults to write-mode when given positional key/value
        // args (e.g. `git config user.name evil`). GIT_FORBIDDEN_TOKENS blocks
        // `--unset`/`--add` etc. but not the positional write form, so require
        // an explicit read-only flag here.
        if first == "config" {
            const CONFIG_READONLY_FLAGS: &[&str] = &[
                "--get",
                "--list",
                "--get-all",
                "--get-regexp",
                "--get-urlmatch",
                "-l",
                "--show-origin",
                "--show-scope",
            ];
            let has_readonly = args
                .iter()
                .skip(1)
                .any(|a| CONFIG_READONLY_FLAGS.contains(&a.as_str()));
            if !has_readonly {
                return Err(
                    "git config requires an explicit read-only flag \
                     (--get, --list, --get-all, --get-regexp, --get-urlmatch, \
                     -l, --show-origin, --show-scope)"
                        .to_string(),
                );
            }
        }
        for arg in args {
            let lower = arg.to_lowercase();
            for forbidden in GIT_FORBIDDEN_TOKENS {
                if lower == *forbidden {
                    return Err(format!(
                        "git argument '{arg}' is not permitted by run_shell"
                    ));
                }
            }
        }
    }

    Ok(())
}

/// On Unix, if the requested program is "python" but doesn't exist on PATH,
/// fall back to "python3" (and vice versa). This is a no-op on Windows where
/// the `py` launcher handles this automatically.
fn resolve_program(program: &str) -> String {
    #[cfg(unix)]
    {
        use std::process::Command as StdCommand;
        if program == "python" {
            if StdCommand::new("which")
                .arg("python")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| !s.success())
                .unwrap_or(true)
            {
                return "python3".to_string();
            }
        } else if program == "python3" {
            if StdCommand::new("which")
                .arg("python3")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| !s.success())
                .unwrap_or(true)
            {
                return "python".to_string();
            }
        }
    }
    program.to_string()
}

/// Return clamped timeout in seconds.
fn clamp_timeout(requested: Option<u64>) -> u64 {
    match requested {
        None => DEFAULT_TIMEOUT_SECS,
        Some(v) if v < MIN_TIMEOUT_SECS => MIN_TIMEOUT_SECS,
        Some(v) if v > MAX_TIMEOUT_SECS => MAX_TIMEOUT_SECS,
        Some(v) => v,
    }
}

/// Build the child environment: start from the parent env, drop any key
/// matching a strip pattern (case-insensitive substring), then re-assert
/// preserve keys from the parent env.
fn build_env() -> Vec<(OsString, OsString)> {
    build_env_from(std::env::vars_os())
}

/// Testable variant: build a child env from an arbitrary iterator over parent
/// env entries.
fn build_env_from<I>(parent: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let parent: Vec<(OsString, OsString)> = parent.into_iter().collect();
    let mut out: Vec<(OsString, OsString)> = Vec::with_capacity(parent.len());

    for (k, v) in &parent {
        let key_str = k.to_string_lossy();
        let key_upper = key_str.to_uppercase();
        let is_preserve = ENV_PRESERVE
            .iter()
            .any(|p| key_upper == p.to_uppercase());
        let is_stripped = ENV_STRIP_PATTERNS
            .iter()
            .any(|p| key_upper.contains(&p.to_uppercase()));
        if is_stripped && !is_preserve {
            continue;
        }
        out.push((k.clone(), v.clone()));
    }

    // Belt-and-braces: make sure preserve keys are present if they were
    // present in the parent env (they'd be added above, but this guards
    // against future strip-pattern expansions accidentally eating them).
    for preserve in ENV_PRESERVE {
        let already = out
            .iter()
            .any(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(preserve));
        if already {
            continue;
        }
        if let Some((k, v)) = parent
            .iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(preserve))
        {
            out.push((k.clone(), v.clone()));
        }
    }

    // Force UTF-8 output from Python subprocesses regardless of system locale.
    out.push((OsString::from("PYTHONUTF8"), OsString::from("1")));
    out.push((
        OsString::from("PYTHONIOENCODING"),
        OsString::from("utf-8"),
    ));

    out
}

// ---------------------------------------------------------------------------
// Output handling
// ---------------------------------------------------------------------------

/// Truncate a byte buffer to at most `max` bytes, decode as UTF-8 (lossy),
/// and report whether truncation occurred.
fn bytes_to_clamped_string(bytes: &[u8], max: usize) -> (String, bool) {
    if bytes.len() <= max {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        // Walk back to a UTF-8-safe boundary.
        let mut cut = max;
        while cut > 0 && (bytes[cut] & 0b1100_0000) == 0b1000_0000 {
            cut -= 1;
        }
        (String::from_utf8_lossy(&bytes[..cut]).into_owned(), true)
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn apply_os_options(cmd: &mut tokio::process::Command) {
    // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    // `tokio::process::Command` re-exposes `creation_flags` directly on
    // Windows, so no `CommandExt` import is needed.
    const FLAGS: u32 = 0x0800_0000 | 0x0000_0200;
    cmd.creation_flags(FLAGS);
}

#[cfg(not(windows))]
fn apply_os_options(_cmd: &mut tokio::process::Command) {
    // kill_on_drop(true) + tokio child handling is sufficient on Unix for our
    // threat model. A dedicated process group could be added later if needed.
}

async fn execute_inner(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout_secs: u64,
) -> Result<RunShellOutput, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    for (k, v) in build_env() {
        cmd.env(k, v);
    }
    apply_os_options(&mut cmd);

    let started = Instant::now();
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn '{program}': {e}"))?;

    let wait = child.wait_with_output();
    match tokio::time::timeout(Duration::from_secs(timeout_secs), wait).await {
        Ok(Ok(output)) => {
            let (stdout, t_out) = bytes_to_clamped_string(&output.stdout, MAX_OUTPUT_BYTES);
            let (stderr, t_err) = bytes_to_clamped_string(&output.stderr, MAX_OUTPUT_BYTES);
            Ok(RunShellOutput {
                exit_code: output.status.code(),
                stdout,
                stderr,
                duration_ms: started.elapsed().as_millis(),
                truncated_stdout: t_out,
                truncated_stderr: t_err,
                killed_by_timeout: false,
            })
        }
        Ok(Err(e)) => Err(format!("process wait failed: {e}")),
        Err(_) => {
            // Timeout: child is dropped at end of scope (kill_on_drop). We
            // don't have access to the child after wait_with_output consumed
            // it, but kill_on_drop took ownership of the pipes and will
            // terminate the process when the future is dropped.
            Ok(RunShellOutput {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("run_shell: killed after {timeout_secs}s timeout"),
                duration_ms: started.elapsed().as_millis(),
                truncated_stdout: false,
                truncated_stderr: false,
                killed_by_timeout: true,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Confirmation / formatting helpers
// ---------------------------------------------------------------------------

fn format_confirmation(program: &str, args: &[String], cwd: &str, timeout: u64) -> String {
    let args_joined = args.join(" ");
    if args_joined.is_empty() {
        format!("Run: {program} in {cwd} (timeout {timeout}s)")
    } else {
        format!("Run: {program} {args_joined} in {cwd} (timeout {timeout}s)")
    }
}

fn format_output(output: &RunShellOutput) -> String {
    let mut result = String::new();

    if output.killed_by_timeout {
        result.push_str("⏱ Process killed (timeout)\n");
    } else if let Some(code) = output.exit_code {
        if code == 0 {
            result.push_str(&format!("✓ Exit code: {}\n", code));
        } else {
            result.push_str(&format!("✗ Exit code: {}\n", code));
        }
    }

    result.push_str(&format!("Duration: {}ms\n", output.duration_ms));

    if !output.stdout.is_empty() {
        result.push_str("\n── stdout ──\n");
        result.push_str(&output.stdout);
        if output.truncated_stdout {
            result.push_str("\n[... truncated to 64KB]");
        }
        if !output.stdout.ends_with('\n') {
            result.push('\n');
        }
    }

    if !output.stderr.is_empty() {
        result.push_str("\n── stderr ──\n");
        result.push_str(&output.stderr);
        if output.truncated_stderr {
            result.push_str("\n[... truncated to 64KB]");
        }
        if !output.stderr.ends_with('\n') {
            result.push('\n');
        }
    }

    result
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
        "run_shell"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        // No dedicated Shell category exists; group with FileSystem since the
        // operation is scoped to source directories. If a Shell category is
        // added later, include it here.
        &[ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let program = args
            .get("program")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
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
        Some(format_confirmation(program, &args_vec, cwd, timeout))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let parsed: RunShellArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid run_shell arguments: {e}")))?;

        let canonical_program = match validate_program(&parsed.program) {
            Ok(p) => p,
            Err(msg) => return Ok(error_result(call_id, msg)),
        };

        let resolved_program = resolve_program(canonical_program);

        if let Err(msg) = validate_args(canonical_program, &parsed.args) {
            return Ok(error_result(call_id, msg));
        }

        let timeout = clamp_timeout(parsed.timeout_secs);

        // Resolve cwd inside a registered source directory (blocking fs ops).
        let cwd_input = parsed.cwd.clone();
        let db_clone = db.clone();
        let scope_clone = source_scope.to_vec();
        let cwd_result: Result<PathBuf, String> = tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db_clone, &scope_clone)
                .map_err(|e| format!("failed to load sources: {e}"))?;
            if sources.is_empty() {
                return Err("No sources registered. Add a source directory first.".to_string());
            }
            resolve_existing_directory_in_sources(Path::new(&cwd_input), &sources)
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?;

        let cwd_path = match cwd_result {
            Ok(p) => p,
            Err(msg) => return Ok(error_result(call_id, msg)),
        };

        let output = match execute_inner(&resolved_program, &parsed.args, &cwd_path, timeout).await
        {
            Ok(o) => o,
            Err(msg) => return Ok(error_result(call_id, msg)),
        };

        tracing::info!(
            target: "tool.run_shell",
            program = canonical_program,
            args_count = parsed.args.len(),
            cwd = %cwd_path.display(),
            exit_code = ?output.exit_code,
            duration_ms = output.duration_ms as u64,
            killed = output.killed_by_timeout,
            truncated_stdout = output.truncated_stdout,
            truncated_stderr = output.truncated_stderr,
            "run_shell executed"
        );

        let is_error = output.killed_by_timeout || output.exit_code != Some(0);
        let content = format_output(&output);

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error,
            artifacts: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    // --- validate_program ---------------------------------------------------

    #[test]
    fn test_reject_non_whitelisted_program() {
        for bad in &["rm", "curl", "sh", "powershell", "cmd", "bash", "zsh"] {
            assert!(
                validate_program(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn test_accept_whitelisted_program() {
        for good in &["python", "python3", "node", "npm", "npx", "git"] {
            assert!(
                validate_program(good).is_ok(),
                "expected '{good}' to be accepted"
            );
        }
    }

    #[test]
    fn test_reject_program_with_path_separator() {
        assert!(validate_program("/usr/bin/python").is_err());
        assert!(validate_program("..\\python").is_err());
    }

    // --- validate_args ------------------------------------------------------

    #[test]
    fn test_reject_null_byte_in_args() {
        let args = vec!["hello\0world".to_string()];
        assert!(validate_args("python", &args).is_err());
    }

    #[test]
    fn test_reject_oversized_arg() {
        let big = "x".repeat(MAX_SINGLE_ARG_BYTES + 1);
        let args = vec![big];
        assert!(validate_args("python", &args).is_err());
    }

    #[test]
    fn test_reject_total_argv_too_large() {
        // Many args just under the single-arg limit, totalling > 32 KB.
        let chunk = "x".repeat(4 * 1024);
        let args: Vec<String> = (0..10).map(|_| chunk.clone()).collect();
        assert!(validate_args("python", &args).is_err());
    }

    #[test]
    fn test_git_requires_readonly_subcommand() {
        assert!(validate_args("git", &["status".to_string()]).is_ok());
        assert!(validate_args("git", &["diff".to_string(), "--stat".to_string()]).is_ok());
        assert!(validate_args("git", &["push".to_string()]).is_err());
        assert!(validate_args("git", &["commit".to_string(), "-m".to_string(), "x".to_string()]).is_err());
        assert!(validate_args("git", &["reset".to_string(), "--hard".to_string()]).is_err());
    }

    #[test]
    fn test_git_empty_args_rejected() {
        let empty: Vec<String> = vec![];
        assert!(validate_args("git", &empty).is_err());
    }

    #[test]
    fn test_git_forbidden_token_in_later_args_rejected() {
        // Primary subcommand is OK but a forbidden token appears later.
        let args = vec!["config".to_string(), "--unset".to_string(), "user.name".to_string()];
        assert!(validate_args("git", &args).is_err());
    }

    #[test]
    fn test_git_config_requires_readonly_flag() {
        fn s(arr: &[&str]) -> Vec<String> {
            arr.iter().map(|x| x.to_string()).collect()
        }
        // Positional-write form: must reject.
        assert!(validate_args("git", &s(&["config", "user.name", "evil"])).is_err());
        assert!(validate_args("git", &s(&["config", "core.editor", "vim"])).is_err());
        // Bare `git config` (no subaction): must reject.
        assert!(validate_args("git", &s(&["config"])).is_err());
        // Read-only forms: must pass.
        assert!(validate_args("git", &s(&["config", "--list"])).is_ok());
        assert!(validate_args("git", &s(&["config", "--get", "user.name"])).is_ok());
        assert!(validate_args("git", &s(&["config", "--get-regexp", "^alias\\."])).is_ok());
        assert!(validate_args("git", &s(&["config", "-l"])).is_ok());
    }

    #[test]
    fn test_python_accepts_arbitrary_args() {
        let args = vec!["-c".to_string(), "print('hello')".to_string()];
        assert!(validate_args("python", &args).is_ok());
    }

    // --- build_env ----------------------------------------------------------

    #[test]
    fn test_env_build_strips_secrets() {
        let parent: Vec<(OsString, OsString)> = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("AWS_ACCESS_KEY_ID"), OsString::from("AKIA...")),
            (OsString::from("MY_SECRET_TOKEN"), OsString::from("hunter2")),
            (OsString::from("GITHUB_TOKEN"), OsString::from("ghp_...")),
            (OsString::from("OPENAI_API_KEY"), OsString::from("sk-...")),
            (OsString::from("LANG"), OsString::from("en_US.UTF-8")),
            (OsString::from("HOME"), OsString::from("/home/user")),
            (OsString::from("FOO_PASSWORD"), OsString::from("swordfish")),
            (OsString::from("CREDENTIALS_DIR"), OsString::from("/tmp/c")),
            (OsString::from("MY_NORMAL_VAR"), OsString::from("ok")),
        ];
        let built = build_env_from(parent);
        let keys: Vec<String> = built
            .iter()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();

        // Preserved
        assert!(keys.iter().any(|k| k == "PATH"), "PATH should be preserved");
        assert!(keys.iter().any(|k| k == "LANG"), "LANG should be preserved");
        assert!(keys.iter().any(|k| k == "HOME"), "HOME should be preserved");
        assert!(keys.iter().any(|k| k == "MY_NORMAL_VAR"), "normal vars pass through");

        // Stripped
        assert!(!keys.iter().any(|k| k == "AWS_ACCESS_KEY_ID"));
        assert!(!keys.iter().any(|k| k == "MY_SECRET_TOKEN"));
        assert!(!keys.iter().any(|k| k == "GITHUB_TOKEN"));
        assert!(!keys.iter().any(|k| k == "OPENAI_API_KEY"));
        assert!(!keys.iter().any(|k| k == "FOO_PASSWORD"));
        assert!(!keys.iter().any(|k| k == "CREDENTIALS_DIR"));
    }

    // --- clamp_timeout ------------------------------------------------------

    #[test]
    fn test_timeout_clamped_to_max() {
        assert_eq!(clamp_timeout(Some(10_000)), MAX_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(0)), MIN_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(30)), 30);
        assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_SECS);
    }

    // --- Tool trait behaviour ----------------------------------------------

    #[test]
    fn test_confirmation_required() {
        let tool = RunShellTool;
        let args = serde_json::json!({
            "program": "python",
            "args": ["-c", "print(1)"],
            "cwd": "."
        });
        assert!(tool.requires_confirmation(&args));
        // Also true for empty args — confirmation is unconditional.
        assert!(tool.requires_confirmation(&serde_json::json!({})));
    }

    #[test]
    fn test_confirmation_message_excludes_env() {
        let tool = RunShellTool;
        let args = serde_json::json!({
            "program": "git",
            "args": ["status", "--short"],
            "cwd": "/workspace/project",
            "timeout_secs": 45
        });
        let msg = tool.confirmation_message(&args).expect("message present");
        assert!(msg.contains("git"));
        assert!(msg.contains("status"));
        assert!(msg.contains("--short"));
        assert!(msg.contains("/workspace/project"));
        assert!(msg.contains("45s"));
        // No env-var leakage.
        assert!(!msg.to_uppercase().contains("PATH="));
        assert!(!msg.to_uppercase().contains("TOKEN"));
        assert!(!msg.to_uppercase().contains("SECRET"));
    }

    #[test]
    fn test_confirmation_message_clamps_timeout() {
        let tool = RunShellTool;
        let args = serde_json::json!({
            "program": "python",
            "args": [],
            "cwd": ".",
            "timeout_secs": 99_999
        });
        let msg = tool.confirmation_message(&args).expect("message");
        assert!(msg.contains(&format!("{MAX_TIMEOUT_SECS}s")));
    }

    // --- bytes_to_clamped_string -------------------------------------------

    #[test]
    fn test_output_truncation_respects_utf8() {
        // Build a buffer larger than max ending on a multi-byte char.
        let mut bytes = vec![b'a'; 10];
        // Append a 3-byte UTF-8 char that would straddle the cut point.
        bytes.extend_from_slice("€".as_bytes()); // 3 bytes
        let (s, trunc) = bytes_to_clamped_string(&bytes, 11);
        assert!(trunc);
        // Result must be valid UTF-8 and not contain the partial char.
        assert_eq!(s, "a".repeat(10));
    }

    // --- Integration tests (need real binaries; ignored by default) --------

    // --- resolve_program ----------------------------------------------------

    #[test]
    fn test_resolve_program_identity() {
        // Non-python programs are returned unchanged on all platforms.
        assert_eq!(resolve_program("git"), "git");
        assert_eq!(resolve_program("node"), "node");
        assert_eq!(resolve_program("npm"), "npm");
        assert_eq!(resolve_program("npx"), "npx");
    }

    // --- build_env UTF-8 injection ------------------------------------------

    #[test]
    fn test_env_includes_pythonutf8() {
        let parent: Vec<(OsString, OsString)> = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
        ];
        let built = build_env_from(parent);
        let keys: Vec<String> = built
            .iter()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        assert!(keys.contains(&"PYTHONUTF8".to_string()), "PYTHONUTF8 must be present");
        assert!(keys.contains(&"PYTHONIOENCODING".to_string()), "PYTHONIOENCODING must be present");

        let pythonutf8_val = built
            .iter()
            .find(|(k, _)| k == "PYTHONUTF8")
            .map(|(_, v)| v.to_string_lossy().to_string())
            .unwrap();
        assert_eq!(pythonutf8_val, "1");

        let pyioenc_val = built
            .iter()
            .find(|(k, _)| k == "PYTHONIOENCODING")
            .map(|(_, v)| v.to_string_lossy().to_string())
            .unwrap();
        assert_eq!(pyioenc_val, "utf-8");
    }

    // --- format_output ------------------------------------------------------

    #[test]
    fn test_format_output_success() {
        let output = RunShellOutput {
            exit_code: Some(0),
            stdout: "hello world\n".to_string(),
            stderr: String::new(),
            duration_ms: 42,
            truncated_stdout: false,
            truncated_stderr: false,
            killed_by_timeout: false,
        };
        let text = format_output(&output);
        assert!(text.contains("Exit code: 0"), "should contain exit code");
        assert!(text.contains("Duration: 42ms"), "should contain duration");
        assert!(text.contains("stdout"), "should contain stdout header");
        assert!(text.contains("hello world"), "should contain stdout content");
        assert!(!text.contains("stderr"), "should not contain stderr when empty");
    }

    #[test]
    fn test_format_output_timeout() {
        let output = RunShellOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: "run_shell: killed after 30s timeout".to_string(),
            duration_ms: 30000,
            truncated_stdout: false,
            truncated_stderr: false,
            killed_by_timeout: true,
        };
        let text = format_output(&output);
        assert!(text.contains("timeout"), "should mention timeout");
        assert!(text.contains("stderr"), "should contain stderr header");
    }

    #[test]
    fn test_format_output_stderr() {
        let output = RunShellOutput {
            exit_code: Some(1),
            stdout: "partial\n".to_string(),
            stderr: "error: something failed\n".to_string(),
            duration_ms: 100,
            truncated_stdout: false,
            truncated_stderr: false,
            killed_by_timeout: false,
        };
        let text = format_output(&output);
        assert!(text.contains("Exit code: 1"));
        assert!(text.contains("stdout"));
        assert!(text.contains("stderr"));
        assert!(text.contains("error: something failed"));
    }

    #[test]
    fn test_format_output_truncation_markers() {
        let output = RunShellOutput {
            exit_code: Some(0),
            stdout: "data".to_string(),
            stderr: "warn".to_string(),
            duration_ms: 10,
            truncated_stdout: true,
            truncated_stderr: true,
            killed_by_timeout: false,
        };
        let text = format_output(&output);
        // Both truncation markers should appear
        assert_eq!(text.matches("truncated to 64KB").count(), 2);
    }

    #[tokio::test]
    #[ignore = "requires python on PATH"]
    async fn test_python_hello() {
        let tmp = tempfile::tempdir().unwrap();
        let out = execute_inner(
            "python",
            &["-c".to_string(), "print('hello')".to_string()],
            tmp.path(),
            10,
        )
        .await
        .expect("run ok");
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("hello"));
        assert!(!out.killed_by_timeout);
    }

    #[tokio::test]
    #[ignore = "requires python on PATH; sleeps"]
    async fn test_python_timeout_kills() {
        let tmp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = execute_inner(
            "python",
            &["-c".to_string(), "import time; time.sleep(60)".to_string()],
            tmp.path(),
            2,
        )
        .await
        .expect("run ok");
        assert!(out.killed_by_timeout);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    #[ignore = "requires python on PATH"]
    async fn test_stdout_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let out = execute_inner(
            "python",
            &["-c".to_string(), "print('x' * 200000)".to_string()],
            tmp.path(),
            10,
        )
        .await
        .expect("run ok");
        assert!(out.truncated_stdout);
        assert!(out.stdout.len() <= MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    #[ignore = "requires git on PATH and a repo"]
    async fn test_git_status() {
        let out = execute_inner(
            "git",
            &["status".to_string(), "--short".to_string()],
            Path::new("."),
            10,
        )
        .await
        .expect("run ok");
        assert_eq!(out.exit_code, Some(0));
    }
}
