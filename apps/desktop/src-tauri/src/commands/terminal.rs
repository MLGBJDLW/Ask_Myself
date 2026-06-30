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

#[derive(Default)]
pub struct TerminalState {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
}

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    shell: String,
    cwd: String,
    process_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStartInput {
    cwd: Option<String>,
    shell: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    id: String,
    shell: String,
    cwd: String,
    process_id: Option<u32>,
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
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| format!("failed to attach terminal input: {err}"))?;
        let process_id = child.process_id();
        let session_id = Uuid::new_v4().to_string();
        let session = TerminalSession {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            killer: Arc::new(Mutex::new(child.clone_killer())),
            shell: candidate.label.to_string(),
            cwd: cwd.display().to_string(),
            process_id,
        };

        {
            let mut sessions = state
                .sessions
                .lock()
                .map_err(|_| "terminal session state is unavailable".to_string())?;
            sessions.insert(session_id.clone(), session);
        }

        spawn_terminal_reader(
            app_handle.clone(),
            state.sessions.clone(),
            session_id.clone(),
            reader,
        );
        spawn_terminal_waiter(
            app_handle,
            state.sessions.clone(),
            session_id.clone(),
            child,
        );

        return Ok(TerminalSessionInfo {
            id: session_id,
            shell: candidate.label.to_string(),
            cwd: cwd.display().to_string(),
            process_id,
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
    let writer = {
        let sessions = state
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        sessions
            .get(&session_id)
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
    let session = {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "terminal session state is unavailable".to_string())?;
        sessions.remove(&session_id)
    };
    if let Some(session) = session {
        let mut killer = session
            .killer
            .lock()
            .map_err(|_| "terminal process handle is unavailable".to_string())?;
        killer
            .kill()
            .map_err(|err| format!("failed to stop terminal process: {err}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn terminal_list_sessions_cmd(
    state: State<'_, TerminalState>,
) -> Result<Vec<TerminalSessionInfo>, String> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "terminal session state is unavailable".to_string())?;
    Ok(sessions
        .iter()
        .map(|(id, session)| TerminalSessionInfo {
            id: id.clone(),
            shell: session.shell.clone(),
            cwd: session.cwd.clone(),
            process_id: session.process_id,
        })
        .collect())
}

fn spawn_terminal_reader(
    app_handle: AppHandle,
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    session_id: String,
    mut reader: Box<dyn Read + Send>,
) {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buffer[..n]).into_owned();
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

fn spawn_terminal_waiter(
    app_handle: AppHandle,
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    session_id: String,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || {
        let result = child.wait();
        if let Ok(mut sessions) = sessions.lock() {
            sessions.remove(&session_id);
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
