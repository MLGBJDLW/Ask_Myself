use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::app_events::emit_app_event;

const TERMINAL_EVENT: &str = "terminal:event";
const MAX_TERMINAL_OUTPUT_CHARS: usize = 180_000;

#[derive(Clone, Default)]
pub struct TerminalState {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    active_by_conversation: Arc<Mutex<HashMap<String, String>>>,
}

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    shell: String,
    cwd: String,
    process_id: Option<u32>,
    conversation_id: Option<String>,
    output: Arc<Mutex<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStartInput {
    cwd: Option<String>,
    shell: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
    conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub process_id: Option<u32>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionSnapshot {
    pub session: TerminalSessionInfo,
    pub output: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEvent {
    session_id: String,
    kind: TerminalEventKind,
    data: Option<String>,
    exit_code: Option<u32>,
    signal: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum TerminalEventKind {
    Data,
    Exit,
    Error,
}

#[derive(Clone)]
struct ShellCandidate {
    label: &'static str,
    program: &'static str,
    args: &'static [&'static str],
}

#[tauri::command]
pub fn terminal_start_session_cmd(
    app_handle: AppHandle,
    state: State<'_, TerminalState>,
    input: TerminalStartInput,
) -> Result<TerminalSessionInfo, String> {
    let cwd = resolve_terminal_cwd(input.cwd)?;
    let conversation_id = normalize_conversation_id(input.conversation_id);
    let size = PtySize {
        rows: input.rows.unwrap_or(24).clamp(5, 200),
        cols: input.cols.unwrap_or(80).clamp(20, 400),
        pixel_width: 0,
        pixel_height: 0,
    };
    let shell = input
        .shell
        .unwrap_or_else(|| "default".to_string())
        .trim()
        .to_ascii_lowercase();
    let candidates = shell_candidates(&shell);
    let pty_system = native_pty_system();
    let mut last_error = None;

    for candidate in candidates {
        let pair = match pty_system.openpty(size) {
            Ok(pair) => pair,
            Err(err) => return Err(format!("failed to create terminal pty: {err}")),
        };
        let mut command = CommandBuilder::new(candidate.program);
        command.args(candidate.args);
        command.cwd(cwd.as_os_str());
        command.env("TERM", "xterm-256color");
        command.env("NEXA_TERMINAL", "1");

        let child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(err) => {
                last_error = Some(format!(
                    "failed to start {} ({}): {err}",
                    candidate.label, candidate.program
                ));
                continue;
            }
        };

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| format!("failed to attach terminal output: {err}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|err| format!("failed to attach terminal input: {err}"))?;
        if let Some(integration) = shell_integration_bootstrap(candidate.label) {
            writer
                .write_all(integration.as_bytes())
                .and_then(|_| writer.flush())
                .map_err(|err| format!("failed to enable terminal shell integration: {err}"))?;
        }
        let process_id = child.process_id();
        let session_id = Uuid::new_v4().to_string();
        let output = Arc::new(Mutex::new(String::new()));
        let session = TerminalSession {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            killer: Arc::new(Mutex::new(child.clone_killer())),
            shell: candidate.label.to_string(),
            cwd: cwd.display().to_string(),
            process_id,
            conversation_id: conversation_id.clone(),
            output: output.clone(),
        };

        {
            let mut sessions = state
                .sessions
                .lock()
                .map_err(|_| "terminal session state is unavailable".to_string())?;
            sessions.insert(session_id.clone(), session);
        }
        if let Some(conversation_id) = conversation_id.as_ref() {
            state
                .active_by_conversation
                .lock()
                .map_err(|_| "terminal activity mapping is unavailable".to_string())?
                .insert(conversation_id.clone(), session_id.clone());
        }

        spawn_terminal_reader(
            app_handle.clone(),
            state.sessions.clone(),
            session_id.clone(),
            reader,
            output,
        );
        spawn_terminal_waiter(
            app_handle,
            state.sessions.clone(),
            state.active_by_conversation.clone(),
            session_id.clone(),
            child,
        );

        return Ok(TerminalSessionInfo {
            id: session_id,
            shell: candidate.label.to_string(),
            cwd: cwd.display().to_string(),
            process_id,
            conversation_id,
        });
    }

    Err(last_error.unwrap_or_else(|| "failed to start terminal shell".to_string()))
}

#[tauri::command]
pub fn terminal_write_session_cmd(
    state: State<'_, TerminalState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    state.write_session(&session_id, &data)
}

#[tauri::command]
pub fn terminal_resize_session_cmd(
    state: State<'_, TerminalState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "terminal session state is unavailable".to_string())?;
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| "terminal session is no longer running".to_string())?;
    session
        .master
        .resize(PtySize {
            rows: rows.clamp(5, 200),
            cols: cols.clamp(20, 400),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| format!("failed to resize terminal: {err}"))
}

#[tauri::command]
pub fn terminal_close_session_cmd(
    state: State<'_, TerminalState>,
    session_id: String,
) -> Result<(), String> {
    state.close_session(&session_id)
}

#[tauri::command]
pub fn terminal_bind_session_cmd(
    state: State<'_, TerminalState>,
    session_id: String,
    conversation_id: Option<String>,
) -> Result<TerminalSessionInfo, String> {
    state.bind_session(&session_id, conversation_id)
}

#[tauri::command]
pub fn terminal_snapshot_session_cmd(
    state: State<'_, TerminalState>,
    session_id: String,
    max_chars: Option<usize>,
) -> Result<TerminalSessionSnapshot, String> {
    state.snapshot_session(&session_id, max_chars.unwrap_or(24_000))
}

#[tauri::command]
pub fn terminal_list_sessions_cmd(
    state: State<'_, TerminalState>,
) -> Result<Vec<TerminalSessionInfo>, String> {
    state.list_sessions()
}

#[tauri::command]
pub fn terminal_active_session_cmd(
    state: State<'_, TerminalState>,
    conversation_id: String,
) -> Result<Option<TerminalSessionInfo>, String> {
    state.active_session(&conversation_id)
}

fn spawn_terminal_reader(
    app_handle: AppHandle,
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    session_id: String,
    mut reader: Box<dyn Read + Send>,
    output: Arc<Mutex<String>>,
) {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buffer[..n]).into_owned();
                    if let Ok(mut output) = output.lock() {
                        append_terminal_output(&mut output, &data);
                    }
                    emit_app_event(
                        &app_handle,
                        TERMINAL_EVENT,
                        &TerminalEvent {
                            session_id: session_id.clone(),
                            kind: TerminalEventKind::Data,
                            data: Some(data),
                            exit_code: None,
                            signal: None,
                        },
                    );
                }
                Err(err) => {
                    let still_running = sessions
                        .lock()
                        .ok()
                        .is_some_and(|sessions| sessions.contains_key(&session_id));
                    if still_running {
                        emit_app_event(
                            &app_handle,
                            TERMINAL_EVENT,
                            &TerminalEvent {
                                session_id: session_id.clone(),
                                kind: TerminalEventKind::Error,
                                data: Some(format!("terminal read failed: {err}")),
                                exit_code: None,
                                signal: None,
                            },
                        );
                    }
                    break;
                }
            }
        }
    });
}

impl TerminalState {
    pub fn active_session(
        &self,
        conversation_id: &str,
    ) -> Result<Option<TerminalSessionInfo>, String> {
        let session_id = self
            .active_by_conversation
            .lock()
            .map_err(|_| "terminal active-session state is unavailable".to_string())?
            .get(conversation_id)
            .cloned();
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        Ok(sessions
            .get(&session_id)
            .map(|session| session_info(&session_id, session)))
    }

    pub fn list_sessions(&self) -> Result<Vec<TerminalSessionInfo>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        let mut listed = sessions
            .iter()
            .map(|(id, session)| session_info(id, session))
            .collect::<Vec<_>>();
        listed.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(listed)
    }

    pub fn write_session(&self, session_id: &str, data: &str) -> Result<(), String> {
        let writer = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "terminal session state is unavailable".to_string())?;
            sessions
                .get(session_id)
                .map(|session| session.writer.clone())
                .ok_or_else(|| "terminal session is no longer running".to_string())?
        };
        let mut writer = writer
            .lock()
            .map_err(|_| "terminal input stream is unavailable".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|err| format!("failed to write terminal input: {err}"))
    }

    pub fn close_session(&self, session_id: &str) -> Result<(), String> {
        let session = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| "terminal session state is unavailable".to_string())?;
            sessions.remove(session_id)
        };
        if let Some(session) = session {
            if let Some(conversation_id) = session.conversation_id.as_ref() {
                if let Ok(mut active) = self.active_by_conversation.lock() {
                    if active
                        .get(conversation_id)
                        .is_some_and(|id| id == session_id)
                    {
                        active.remove(conversation_id);
                    }
                }
            }
            let mut killer = session
                .killer
                .lock()
                .map_err(|_| "terminal process handle is unavailable".to_string())?;
            if let Err(err) = killer.kill() {
                if !terminal_stop_succeeded(&err) {
                    return Err(format!("failed to stop terminal process: {err}"));
                }
            }
        }
        Ok(())
    }

    pub fn bind_session(
        &self,
        session_id: &str,
        conversation_id: Option<String>,
    ) -> Result<TerminalSessionInfo, String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| "terminal session is no longer running".to_string())?;
        let previous_conversation_id = session.conversation_id.clone();
        session.conversation_id = normalize_conversation_id(conversation_id);
        let info = session_info(session_id, session);
        drop(sessions);
        let mut active = self
            .active_by_conversation
            .lock()
            .map_err(|_| "terminal activity mapping is unavailable".to_string())?;
        if let Some(previous) = previous_conversation_id {
            if active.get(&previous).is_some_and(|id| id == session_id) {
                active.remove(&previous);
            }
        }
        if let Some(current) = info.conversation_id.as_ref() {
            active.insert(current.clone(), session_id.to_string());
        }
        Ok(info)
    }

    pub fn snapshot_session(
        &self,
        session_id: &str,
        max_chars: usize,
    ) -> Result<TerminalSessionSnapshot, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "terminal session is no longer running".to_string())?;
        let output = session
            .output
            .lock()
            .map_err(|_| "terminal output buffer is unavailable".to_string())?;
        Ok(TerminalSessionSnapshot {
            session: session_info(session_id, session),
            output: terminal_output_tail(&output, max_chars.clamp(1, MAX_TERMINAL_OUTPUT_CHARS)),
        })
    }

    pub fn snapshot_for_conversation(
        &self,
        conversation_id: &str,
        requested_session_id: Option<&str>,
        max_chars: usize,
    ) -> Result<TerminalSessionSnapshot, String> {
        let session_id = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "terminal session state is unavailable".to_string())?;
            if let Some(requested) = requested_session_id {
                let session = sessions
                    .get(requested)
                    .ok_or_else(|| "terminal session is no longer running".to_string())?;
                if session.conversation_id.as_deref() != Some(conversation_id) {
                    return Err(
                        "terminal session is not linked to the current conversation".to_string()
                    );
                }
                requested.to_string()
            } else {
                let active = self
                    .active_by_conversation
                    .lock()
                    .map_err(|_| "terminal activity mapping is unavailable".to_string())?;
                let session_id = active.get(conversation_id).cloned().ok_or_else(|| {
                    "no active terminal is linked to the current conversation".to_string()
                })?;
                if !sessions.contains_key(&session_id) {
                    return Err("the active terminal is no longer running".to_string());
                }
                session_id
            }
        };
        self.snapshot_session(&session_id, max_chars)
    }
}

fn session_info(id: &str, session: &TerminalSession) -> TerminalSessionInfo {
    TerminalSessionInfo {
        id: id.to_string(),
        shell: session.shell.clone(),
        cwd: session.cwd.clone(),
        process_id: session.process_id,
        conversation_id: session.conversation_id.clone(),
    }
}

fn normalize_conversation_id(value: Option<String>) -> Option<String> {
    value
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

fn append_terminal_output(output: &mut String, data: &str) {
    output.push_str(data);
    if output.len() <= MAX_TERMINAL_OUTPUT_CHARS {
        return;
    }
    let tail = terminal_output_tail(output, MAX_TERMINAL_OUTPUT_CHARS);
    *output = tail;
}

fn terminal_output_tail(output: &str, max_chars: usize) -> String {
    if output.len() <= max_chars {
        return output.to_string();
    }
    let mut start = output.len().saturating_sub(max_chars);
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }
    output[start..].to_string()
}

fn terminal_stop_succeeded(error: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // portable-pty 0.9's Windows ChildKiller reports the successful
        // TerminateProcess branch as Err(last_os_error()). The common result is
        // OS error 0 (ERROR_SUCCESS), but the last-error slot may also be stale.
        // Reaching this Err branch therefore means TerminateProcess succeeded.
        let _ = error;
        true
    }
    #[cfg(not(windows))]
    {
        error.raw_os_error() == Some(0)
            || error
                .to_string()
                .to_ascii_lowercase()
                .contains("os error 0")
    }
}

fn spawn_terminal_waiter(
    app_handle: AppHandle,
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    active_by_conversation: Arc<Mutex<HashMap<String, String>>>,
    session_id: String,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || {
        let result = child.wait();
        if let Ok(mut sessions) = sessions.lock() {
            let conversation_id = sessions
                .remove(&session_id)
                .and_then(|session| session.conversation_id);
            if let (Some(conversation_id), Ok(mut active)) =
                (conversation_id, active_by_conversation.lock())
            {
                if active
                    .get(&conversation_id)
                    .is_some_and(|active_id| active_id == &session_id)
                {
                    active.remove(&conversation_id);
                }
            }
        }
        let (exit_code, signal, data) = match result {
            Ok(status) => (
                Some(status.exit_code()),
                status.signal().map(ToString::to_string),
                None,
            ),
            Err(err) => (None, None, Some(format!("terminal wait failed: {err}"))),
        };
        emit_app_event(
            &app_handle,
            TERMINAL_EVENT,
            &TerminalEvent {
                session_id,
                kind: if data.is_some() {
                    TerminalEventKind::Error
                } else {
                    TerminalEventKind::Exit
                },
                data,
                exit_code,
                signal,
            },
        );
    });
}

fn shell_integration_bootstrap(shell: &str) -> Option<String> {
    if shell.contains("PowerShell") {
        return Some(
            "$global:NexaOriginalPrompt=${function:prompt}; function global:prompt { $nexaExit=if ($null -eq $global:LASTEXITCODE) { 0 } else { $global:LASTEXITCODE }; [Console]::Write(\"`e]633;D;$nexaExit`a`e]633;P;Cwd=$($PWD.Path)`a`e]633;A`a\"); if ($global:NexaOriginalPrompt) { & $global:NexaOriginalPrompt } else { \"PS $($PWD.Path)> \" } }\r"
                .to_string(),
        );
    }
    if matches!(shell, "Bash" | "Git Bash" | "Zsh" | "Default Shell" | "sh") {
        return Some(
            r#"PROMPT_COMMAND='__nexa_exit=$?; printf "\033]633;D;%s\007\033]633;P;Cwd=%s\007\033]633;A\007" "$__nexa_exit" "$PWD"'"#
                .to_string()
                + "\n",
        );
    }
    None
}

fn resolve_terminal_cwd(input: Option<String>) -> Result<PathBuf, String> {
    let cwd = match input {
        Some(raw) if !raw.trim().is_empty() => PathBuf::from(raw.trim()),
        _ => std::env::current_dir()
            .map_err(|err| format!("failed to resolve current directory: {err}"))?,
    };
    let cwd = std::fs::canonicalize(&cwd).map_err(|err| {
        format!(
            "failed to resolve terminal directory '{}': {err}",
            cwd.display()
        )
    })?;
    if !cwd.is_dir() {
        return Err(format!(
            "terminal directory '{}' is not a folder",
            cwd.display()
        ));
    }
    Ok(normalize_terminal_cwd_for_interactive_shell(cwd))
}

#[cfg(windows)]
fn normalize_terminal_cwd_for_interactive_shell(cwd: PathBuf) -> PathBuf {
    let Some(normalized) = strip_windows_verbatim_prefix(&cwd) else {
        return cwd;
    };
    if normalized.is_dir() {
        normalized
    } else {
        cwd
    }
}

#[cfg(not(windows))]
fn normalize_terminal_cwd_for_interactive_shell(cwd: PathBuf) -> PathBuf {
    cwd
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: &std::path::Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let prefix = match components.next()? {
        Component::Prefix(prefix) => prefix,
        _ => return None,
    };

    let mut normalized = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:\\", drive as char)),
        Prefix::VerbatimUNC(server, share) => PathBuf::from(format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        )),
        _ => return None,
    };

    for component in components {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => normalized.push(".."),
            Component::Prefix(_) => {}
        }
    }

    Some(normalized)
}

fn shell_candidates(requested: &str) -> Vec<ShellCandidate> {
    #[cfg(windows)]
    {
        match requested {
            "powershell" | "pwsh" => vec![
                ShellCandidate {
                    label: "PowerShell",
                    program: "pwsh.exe",
                    args: &["-NoLogo"],
                },
                ShellCandidate {
                    label: "Windows PowerShell",
                    program: "powershell.exe",
                    args: &["-NoLogo"],
                },
            ],
            "cmd" | "command" => vec![ShellCandidate {
                label: "Command Prompt",
                program: "cmd.exe",
                args: &[],
            }],
            "bash" | "sh" => vec![
                ShellCandidate {
                    label: "Bash",
                    program: "bash.exe",
                    args: &["--login"],
                },
                ShellCandidate {
                    label: "Git Bash",
                    program: "sh.exe",
                    args: &["--login"],
                },
            ],
            _ => vec![
                ShellCandidate {
                    label: "PowerShell",
                    program: "pwsh.exe",
                    args: &["-NoLogo"],
                },
                ShellCandidate {
                    label: "Windows PowerShell",
                    program: "powershell.exe",
                    args: &["-NoLogo"],
                },
                ShellCandidate {
                    label: "Command Prompt",
                    program: "cmd.exe",
                    args: &[],
                },
            ],
        }
    }

    #[cfg(not(windows))]
    {
        match requested {
            "bash" => vec![ShellCandidate {
                label: "Bash",
                program: "bash",
                args: &["--login"],
            }],
            "sh" => vec![ShellCandidate {
                label: "sh",
                program: "sh",
                args: &[],
            }],
            "zsh" => vec![ShellCandidate {
                label: "Zsh",
                program: "zsh",
                args: &["--login"],
            }],
            _ => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
                vec![
                    ShellCandidate {
                        label: "Default Shell",
                        program: Box::leak(shell.into_boxed_str()),
                        args: &["-l"],
                    },
                    ShellCandidate {
                        label: "sh",
                        program: "sh",
                        args: &[],
                    },
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_output_tail_preserves_utf8_boundaries() {
        assert_eq!(terminal_output_tail("abc终端", 4), "端");
        assert_eq!(terminal_output_tail("abc", 20), "abc");
    }

    #[cfg(windows)]
    #[test]
    fn accepts_portable_pty_windows_success_error() {
        let error = std::io::Error::from_raw_os_error(0);
        assert!(terminal_stop_succeeded(&error));
    }

    #[cfg(windows)]
    #[test]
    fn strips_verbatim_disk_prefix_for_interactive_shells() {
        let input = PathBuf::from(r"\\?\D:\Apps\ask_myself\apps\desktop\src-tauri");
        let normalized = strip_windows_verbatim_prefix(&input)
            .expect("verbatim disk paths should be convertible to normal DOS paths");

        assert_eq!(
            normalized,
            PathBuf::from(r"D:\Apps\ask_myself\apps\desktop\src-tauri")
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolved_terminal_cwd_is_powershell_friendly_when_possible() {
        let current = std::env::current_dir().expect("current directory should exist");
        let canonical =
            std::fs::canonicalize(&current).expect("current directory should be canonicalizable");

        let resolved = normalize_terminal_cwd_for_interactive_shell(canonical);

        assert!(resolved.is_dir());
        assert!(
            !resolved.display().to_string().starts_with(r"\\?\"),
            "interactive PowerShell sessions should not start in verbatim paths"
        );
    }
}
