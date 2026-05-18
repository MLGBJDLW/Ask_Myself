use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::super::run_shell_contract::{
    invalid_shell_selector_type_message, unsupported_shell_selector_message, DEFAULT_TIMEOUT_SECS,
    MAX_OUTPUT_BYTES, MIN_TIMEOUT_SECS,
};

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

#[derive(serde::Serialize)]
pub(super) struct RunShellOutput {
    pub(super) exit_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) duration_ms: u128,
    pub(super) truncated_stdout: bool,
    pub(super) truncated_stderr: bool,
    pub(super) killed_by_timeout: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandShell {
    Default,
    PowerShell,
    Pwsh,
    Cmd,
    Bash,
    Sh,
}

impl CommandShell {
    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::PowerShell => "powershell",
            Self::Pwsh => "pwsh",
            Self::Cmd => "cmd",
            Self::Bash => "bash",
            Self::Sh => "sh",
        }
    }
}

pub(super) fn parse_shell_selector(value: Option<&Value>) -> Result<Option<CommandShell>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Bool(false) => Ok(None),
        Value::Bool(true) => Ok(Some(CommandShell::Default)),
        Value::String(raw) => {
            let selector = raw.trim().to_ascii_lowercase();
            match selector.as_str() {
                "" | "none" | "argv" | "direct" | "false" => Ok(None),
                "true" | "default" | "system" | "shell" => Ok(Some(CommandShell::Default)),
                "powershell" | "windows_powershell" | "windows-powershell" => {
                    Ok(Some(CommandShell::PowerShell))
                }
                "pwsh" | "powershell7" | "powershell-core" | "powershell_core" => {
                    Ok(Some(CommandShell::Pwsh))
                }
                "cmd" | "cmd.exe" => Ok(Some(CommandShell::Cmd)),
                "bash" => Ok(Some(CommandShell::Bash)),
                "sh" => Ok(Some(CommandShell::Sh)),
                _ => Err(unsupported_shell_selector_message(raw)),
            }
        }
        _ => Err(invalid_shell_selector_type_message().to_string()),
    }
}

pub(super) fn shell_invocation(
    shell: CommandShell,
    command: &str,
) -> Result<(String, Vec<String>), String> {
    match shell {
        CommandShell::Default => default_shell_invocation(command),
        CommandShell::PowerShell => Ok((
            powershell_program().to_string(),
            powershell_args(command, false),
        )),
        CommandShell::Pwsh => Ok(("pwsh".to_string(), powershell_args(command, true))),
        CommandShell::Cmd => cmd_shell_invocation(command),
        CommandShell::Bash => Ok((
            "bash".to_string(),
            vec!["-lc".to_string(), command.to_string()],
        )),
        CommandShell::Sh => Ok((
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )),
    }
}

#[cfg(windows)]
fn default_shell_invocation(command: &str) -> Result<(String, Vec<String>), String> {
    Ok((
        powershell_program().to_string(),
        powershell_args(command, false),
    ))
}

#[cfg(not(windows))]
fn default_shell_invocation(command: &str) -> Result<(String, Vec<String>), String> {
    Ok((
        "sh".to_string(),
        vec!["-c".to_string(), command.to_string()],
    ))
}

#[cfg(windows)]
fn powershell_program() -> &'static str {
    "powershell.exe"
}

#[cfg(not(windows))]
fn powershell_program() -> &'static str {
    "pwsh"
}

fn powershell_args(command: &str, is_pwsh: bool) -> Vec<String> {
    let mut args = vec![
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
    ];
    if cfg!(windows) && !is_pwsh {
        args.push("-ExecutionPolicy".to_string());
        args.push("Bypass".to_string());
    }
    args.push("-Command".to_string());
    args.push(command.to_string());
    args
}

#[cfg(windows)]
fn cmd_shell_invocation(command: &str) -> Result<(String, Vec<String>), String> {
    Ok((
        "cmd.exe".to_string(),
        vec![
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            command.to_string(),
        ],
    ))
}

#[cfg(not(windows))]
fn cmd_shell_invocation(_command: &str) -> Result<(String, Vec<String>), String> {
    Err("cmd shell is only available on Windows".to_string())
}

pub(super) fn resolve_program(program: &str) -> String {
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
        } else if program == "python3"
            && StdCommand::new("which")
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
    program.to_string()
}

/// Return effective timeout in seconds. 0 means no per-command timeout.
pub(super) fn clamp_timeout(requested: Option<u64>) -> u64 {
    match requested {
        None => DEFAULT_TIMEOUT_SECS,
        Some(0) => 0,
        Some(v) if v < MIN_TIMEOUT_SECS => MIN_TIMEOUT_SECS,
        Some(v) => v,
    }
}

/// Build the child environment: start from the parent env, drop any key
/// matching a strip pattern (case-insensitive substring), then re-assert
/// preserve keys from the parent env.
pub(super) fn build_env() -> Vec<(OsString, OsString)> {
    build_env_from(std::env::vars_os())
}

/// Testable variant: build a child env from an arbitrary iterator over parent
/// env entries.
pub(super) fn build_env_from<I>(parent: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let parent: Vec<(OsString, OsString)> = parent.into_iter().collect();
    let mut out: Vec<(OsString, OsString)> = Vec::with_capacity(parent.len());

    for (k, v) in &parent {
        let key_str = k.to_string_lossy();
        let key_upper = key_str.to_uppercase();
        let is_preserve = ENV_PRESERVE.iter().any(|p| key_upper == p.to_uppercase());
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

    prepend_path_from_parent_env(
        &mut out,
        &parent,
        crate::office_runtime::OFFICE_PYTHON_BIN_DIR_ENV,
    );

    // Force UTF-8 output from Python subprocesses regardless of system locale.
    out.push((OsString::from("PYTHONUTF8"), OsString::from("1")));
    out.push((OsString::from("PYTHONIOENCODING"), OsString::from("utf-8")));

    out
}

fn prepend_path_from_parent_env(
    env: &mut Vec<(OsString, OsString)>,
    parent: &[(OsString, OsString)],
    key: &str,
) {
    let Some((_, bin_dir)) = parent
        .iter()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(key))
    else {
        return;
    };
    if bin_dir.is_empty() {
        return;
    }
    let bin = PathBuf::from(bin_dir);
    if !bin.exists() {
        return;
    }

    let mut paths = vec![bin];
    let existing_path = env
        .iter()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone());
    if let Some(existing) = existing_path {
        paths.extend(std::env::split_paths(&existing));
    }
    let Ok(joined) = std::env::join_paths(paths) else {
        return;
    };

    if let Some((_, value)) = env
        .iter_mut()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
    {
        *value = joined;
    } else {
        env.push((OsString::from("PATH"), joined));
    }
}

// ---------------------------------------------------------------------------
// Output handling
// ---------------------------------------------------------------------------

/// Truncate a byte buffer to at most `max` bytes, decode as UTF-8 (lossy),
/// and report whether truncation occurred.
pub(super) fn bytes_to_clamped_string(bytes: &[u8], max: usize) -> (String, bool) {
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

pub(super) async fn execute_inner(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout_secs: u64,
    stdin: Option<&str>,
) -> Result<RunShellOutput, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    for (k, v) in build_env() {
        cmd.env(k, v);
    }
    apply_os_options(&mut cmd);

    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn '{program}': {e}"))?;

    let stdin_task = if let Some(input) = stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open child stdin".to_string())?;
        let input = input.to_string();
        Some(tokio::spawn(async move {
            child_stdin.write_all(input.as_bytes()).await?;
            child_stdin.shutdown().await
        }))
    } else {
        None
    };

    let wait = child.wait_with_output();
    let result = if timeout_secs == 0 {
        Ok(wait.await)
    } else {
        tokio::time::timeout(Duration::from_secs(timeout_secs), wait).await
    };

    match result {
        Ok(Ok(output)) => {
            if let Some(task) = stdin_task {
                match task.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Ok(Err(e)) => return Err(format!("failed to write stdin: {e}")),
                    Err(e) => return Err(format!("stdin writer task failed: {e}")),
                }
            }
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
            if let Some(task) = stdin_task {
                task.abort();
            }
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

pub(super) fn format_confirmation(
    program: &str,
    args: &[String],
    cwd: &str,
    timeout: u64,
    stdin_bytes: Option<usize>,
) -> String {
    let args_joined = args.join(" ");
    let timeout_label = if timeout == 0 {
        "no timeout".to_string()
    } else {
        format!("timeout {timeout}s")
    };
    let stdin_note = stdin_bytes
        .map(|bytes| format!(", stdin {bytes} bytes"))
        .unwrap_or_default();
    if args_joined.is_empty() {
        format!("Run: {program} in {cwd} ({timeout_label}{stdin_note})")
    } else {
        format!("Run: {program} {args_joined} in {cwd} ({timeout_label}{stdin_note})")
    }
}

pub(super) fn format_shell_confirmation(
    shell: CommandShell,
    command: &str,
    cwd: &str,
    timeout: u64,
    stdin_bytes: Option<usize>,
) -> String {
    let timeout_label = if timeout == 0 {
        "no timeout".to_string()
    } else {
        format!("timeout {timeout}s")
    };
    let stdin_note = stdin_bytes
        .map(|bytes| format!(", stdin {bytes} bytes"))
        .unwrap_or_default();
    format!(
        "Run in {} shell: {command} in {cwd} ({timeout_label}{stdin_note})",
        shell.label()
    )
}

pub(super) fn format_output(output: &RunShellOutput) -> String {
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
