//! Canonical `run_shell` tool contract.
//!
//! Keep model-facing prompt text, JSON schema, validation constants, and
//! recoverable error guidance in this module so the runtime and prompt cannot
//! drift apart.

use std::sync::OnceLock;

use serde_json::{json, Value};

pub(crate) const TOOL_NAME: &str = "run_shell";

pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(crate) const MIN_TIMEOUT_SECS: u64 = 1;
pub(crate) const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_SINGLE_ARG_BYTES: usize = 8 * 1024;
pub(crate) const MAX_TOTAL_ARGV_BYTES: usize = 32 * 1024;
pub(crate) const MAX_STDIN_BYTES: usize = 1024 * 1024;

pub(crate) const PROGRAM_WHITELIST: &[&str] = &[
    "python", "python3", "pip", "pip3", "node", "npm", "npx", "git", "pwd", "ls", "cat", "mkdir",
    "cp", "mv",
];

pub(crate) const PROGRAM_ALIASES: &[(&str, &str)] = &[("copy", "cp"), ("move", "mv")];

pub(crate) const GIT_READONLY_SUBCOMMANDS: &[&str] = &[
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

pub(crate) const GIT_FORBIDDEN_TOKENS: &[&str] = &[
    "push",
    "pull",
    "fetch",
    "commit",
    "reset",
    "merge",
    "rebase",
    "cherry-pick",
    "clone",
    "init",
    "add",
    "rm",
    "mv",
    "checkout",
    "switch",
    "restore",
    "am",
    "apply",
    "stash",
    "--set",
    "--unset",
    "--unset-all",
    "--add",
    "--replace-all",
];

pub(crate) const FORBIDDEN_ARG_SUBSTRINGS: &[&str] = &["\0"];

pub(crate) const SHELL_ENUM: &[&str] =
    &["none", "default", "powershell", "pwsh", "cmd", "bash", "sh"];

static TOOL_DESCRIPTION: OnceLock<String> = OnceLock::new();
static SYSTEM_PROMPT_SECTION: OnceLock<String> = OnceLock::new();
static ROUTE_GUIDANCE: OnceLock<String> = OnceLock::new();

pub(crate) fn tool_description() -> &'static str {
    TOOL_DESCRIPTION.get_or_init(|| {
        format!(
            "Execute a command with platform-aware safety controls. {invocation_modes} {direct_command} {shell_mode} {restricted_programs} {native_fs} {plain_text_tools} {large_payloads} {html_pptx} {timeouts} {windows_paths}",
            invocation_modes = invocation_modes_sentence(),
            direct_command = direct_command_sentence(),
            shell_mode = shell_mode_sentence(),
            restricted_programs = restricted_programs_sentence(),
            native_fs = native_filesystem_sentence(),
            plain_text_tools = plain_text_tools_sentence(),
            large_payloads = large_payload_sentence(),
            html_pptx = html_pptx_sentence(),
            timeouts = timeout_sentence(),
            windows_paths = windows_paths_sentence(),
        )
    })
}

pub(crate) fn parameters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": format!(
                    "Command string. Plain commands are parsed into exact argv without a shell. In ConfirmAll/Open modes, shell syntax such as ;, &&, |, redirection, command substitution, or multiline scripts automatically uses the platform default shell; Restricted mode still rejects shell syntax. You may also set shell explicitly. On Windows, path backslashes are preserved by the command parser; JSON strings still need escaped backslashes such as \"C:\\\\Users\\\\me\\\\script.py\". Do not provide command together with program or args."
                )
            },
            "shell": {
                "oneOf": [
                    { "type": "boolean" },
                    {
                        "type": "string",
                        "enum": SHELL_ENUM
                    }
                ],
                "default": false,
                "description": shell_parameter_description()
            },
            "program": {
                "type": "string",
                "description": format!(
                    "Program to execute when not using command. In restricted mode it must be one of: {programs}. pwd/ls/cat/mkdir/cp/mv run natively inside the app and work even when the OS has no matching external binary. pip/pip3 are normalized to python/python3 -m pip. Simple aliases copy->cp and move->mv are also accepted. In less-restricted shell access modes, any bare command name may be allowed. No shell interpreter is invoked automatically.",
                    programs = PROGRAM_WHITELIST.join(", ")
                )
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "default": [],
                "description": format!(
                    "Arguments passed directly as argv when using program (no shell interpretation). Defaults to []. Do not use args with command. For generated HTML/PPTX specs, use args like [\".../edit_doc.py\", \"--path\", \".../deck.pptx\", \"create_html_pptx\", \"--spec\", \"-\", \"--outdir\", \".../html_deck_project\", \"--mode\", \"hybrid\", \"--screenshot\", \"auto\"] and put the JSON spec in stdin. In restricted mode, git first arg must be a read-only subcommand ({git_subcommands}), and filesystem-command paths are resolved relative to cwd and must stay inside registered source roots.",
                    git_subcommands = GIT_READONLY_SUBCOMMANDS.join(", ")
                )
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory. Defaults deterministically to the first source in the active conversation scope. In restricted mode it must resolve inside a registered source directory; less-restricted modes may use any existing directory."
            },
            "timeout_secs": {
                "type": "integer",
                "description": timeout_parameter_description(),
                "default": DEFAULT_TIMEOUT_SECS,
                "minimum": 0
            },
            "background": {
                "type": "boolean",
                "default": false,
                "description": "Start a long-running command without waiting for it to exit. Use it for local web/API services and for any build, install, test, or migration run that can outlast the default timeout: the call returns a service_id immediately so you can keep working and poll with service_action=wait or status. ready_url is recommended but optional for services: when omitted, the tool discovers loopback URLs from bounded startup logs and otherwise returns after a stability check. Recognized server commands are automatically promoted to this managed mode even if background is omitted."
            },
            "ready_url": {
                "type": "string",
                "description": "Optional loopback HTTP(S) URL used to verify a background service, for example http://127.0.0.1:4173. Only localhost and loopback IP addresses are accepted. When omitted, run_shell tries to discover the URL from startup logs."
            },
            "ready_timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "maximum": 120,
                "default": 20,
                "description": "How long to poll ready_url while starting a background service."
            },
            "service_action": {
                "type": "string",
                "enum": ["run", "status", "wait", "stop"],
                "default": "run",
                "description": "Use status, wait, or stop with a service_id returned by an earlier background run. status takes one snapshot of health and log tails. wait polls until the process exits or timeout_secs elapses and then returns the exit status with log tails, so prefer it over guessing a sleep. Omit for ordinary commands."
            },
            "service_id": {
                "type": "string",
                "description": "Managed service identifier returned by a background run. Required for service_action status, wait, or stop."
            },
            "stdin": {
                "type": "string",
                "description": "Optional text written to the child process stdin. Use this for scripts or generated content that would exceed argv limits, e.g. program python with args [\"-\"] and stdin containing the script. For HTML-first PPTX generation, pass the JSON deck spec here while using --spec - in args. The payload is bounded and is not logged. Native filesystem commands do not accept stdin."
            }
        },
        "required": []
    })
}

pub(crate) fn system_prompt_section() -> &'static str {
    SYSTEM_PROMPT_SECTION.get_or_init(|| {
        format!(
            "## Tool Contract: run_shell\n\n- {project_tool}\n- {invocation_modes}\n- {direct_command} {shell_mode}\n- {restricted_programs}\n- {native_fs} {plain_text_tools}\n- {large_payloads}\n- {html_pptx}\n- {timeouts}",
            project_tool = "Use `project_tool list` / `project_tool describe` before ad hoc `run_shell` when a repository may define local lint, test, codegen, diagnostics, export, or validation workflows in `.nexa/tools` or `.agents/tools`. Use `project_tool run` only when the manifest clearly matches the task; include the current `manifestHash` returned by list/describe.",
            invocation_modes = prompt_invocation_modes_sentence(),
            direct_command = prompt_direct_command_sentence(),
            shell_mode = prompt_shell_mode_sentence(),
            restricted_programs = prompt_restricted_programs_sentence(),
            native_fs = prompt_native_filesystem_sentence(),
            plain_text_tools = prompt_plain_text_tools_sentence(),
            large_payloads = prompt_large_payload_sentence(),
            html_pptx = prompt_html_pptx_sentence(),
            timeouts = prompt_timeout_sentence(),
        )
    })
}

pub(crate) fn route_guidance() -> &'static str {
    ROUTE_GUIDANCE.get_or_init(|| {
        "Use `project_tool list` / `project_tool describe` before ad hoc `run_shell` when a repository may define local lint, test, codegen, diagnostics, export, or validation workflows; `project_tool run` must include the current manifestHash. When `run_shell` is needed, prefer `command` for simple commands, `program` plus `args` for exact argv or stdin-driven scripts, and explicit `shell` when the interpreter choice matters.".to_string()
    })
}

pub(crate) fn expected_format() -> Value {
    json!({
        "examples": {
            "simpleCommand": {
                "command": "git status --short",
                "cwd": "D:/workspace",
                "timeout_secs": DEFAULT_TIMEOUT_SECS
            },
            "shellCommand": {
                "command": "git status --short && git diff --stat",
                "shell": "default",
                "cwd": "D:/workspace",
                "timeout_secs": DEFAULT_TIMEOUT_SECS
            },
            "exactArgv": {
                "program": "python",
                "args": [
                    "crates/core/assets/skills/doc-script-editor/scripts/edit_doc.py",
                    "--path",
                    "D:/workspace/deck.pptx",
                    "create_html_pptx",
                    "--spec",
                    "-",
                    "--outdir",
                    "D:/workspace/html_deck_project",
                    "--mode",
                    "hybrid",
                    "--screenshot",
                    "auto"
                ],
                "cwd": "D:/workspace",
                "timeout_secs": DEFAULT_TIMEOUT_SECS,
                "stdin": "{ \"slides\": [/* HTML-first PPTX JSON spec */] }"
            },
            "backgroundService": {
                "command": "python -m http.server 8080",
                "cwd": "D:/workspace",
                "background": true,
                "ready_url": "http://127.0.0.1:8080",
                "ready_timeout_secs": 20
            }
        },
        "rules": [
            "Use command for simple one-line commands, or program plus args for exact argv control.",
            "Plain commands are parsed into exact argv without a shell. In ConfirmAll/Open modes, recognizable shell syntax automatically uses the platform default shell; set shell explicitly when interpreter choice matters.",
            "Restricted mode rejects shell syntax and explicit shell execution.",
            "Do not send command together with program or args.",
            "args must be an array of argv strings.",
            "For generated HTML/PPTX specs or large scripts, pass the payload in stdin and use --spec - or a stdin-reading program.",
            "Do not put raw HTML, JSON specs, or multiline scripts inside args or python -c.",
            "Long-running web/API servers are automatically promoted to managed background services. Prefer background=true and provide a loopback ready_url when known; otherwise run_shell discovers a URL from startup logs or returns a running service after a stability check. Check it with service_action=status before or after browser inspection, and stop it with service_action=stop when finished."
        ]
    })
}

pub(crate) fn invalid_arguments_message(err: impl std::fmt::Display) -> String {
    format!(
        "Invalid run_shell arguments: {err}. Use command or program/args JSON. For HTML/PPTX generation, pass large specs through stdin with --spec - instead of putting HTML or JSON inside args."
    )
}

pub(crate) fn program_not_allowed_message(program: &str) -> String {
    format!(
        "program '{program}' is not in the run_shell whitelist. Allowed: {}",
        allowed_programs_for_error().join(", ")
    )
}

pub(crate) fn unsupported_shell_selector_message(raw: &str) -> String {
    format!(
        "unsupported run_shell.shell '{raw}'. Use false/none, default, powershell, pwsh, cmd, bash, or sh"
    )
}

pub(crate) fn invalid_shell_selector_type_message() -> &'static str {
    "run_shell.shell must be a boolean or one of: none, default, powershell, pwsh, cmd, bash, sh"
}

pub(crate) fn command_shell_operator_error() -> &'static str {
    "Restricted run_shell.command only accepts a simple command string; shell operators like pipes, chains, redirection, backticks, and backgrounding are not supported. Use program/args for exact argv, or change shell access policy before intentional shell execution."
}

pub(crate) fn command_substitution_error() -> &'static str {
    "Restricted run_shell.command does not support shell command substitution. Use program/args or stdin instead."
}

pub(crate) fn shell_restricted_error() -> &'static str {
    "run_shell.shell requires ConfirmAll or Open shell access mode; it is not available in Restricted mode."
}

pub(crate) fn shell_mix_error() -> &'static str {
    "Use run_shell.shell only with command; do not combine shell with program or args."
}

pub(crate) fn command_program_mix_error() -> &'static str {
    "Use either run_shell.command or run_shell program/args, not both."
}

pub(crate) fn command_args_mix_error() -> &'static str {
    "Do not pass args together with run_shell.command; include arguments in command or use program/args."
}

#[cfg(test)]
pub(crate) fn tool_definition_snapshot() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": tool_description(),
        "parameters": parameters_schema()
    })
}

fn allowed_programs_for_error() -> Vec<&'static str> {
    PROGRAM_WHITELIST
        .iter()
        .copied()
        .chain(PROGRAM_ALIASES.iter().map(|(alias, _)| *alias))
        .collect()
}

fn invocation_modes_sentence() -> &'static str {
    "You can pass either `command` for short one-line commands like `git status --short`, or `program` plus `args` for exact argv control."
}

fn prompt_invocation_modes_sentence() -> &'static str {
    "Use `run_shell` only when a dedicated file/project tool is not the better fit. Use `command` for simple cross-platform commands and `program` plus `args` for exact argv control or large stdin-driven scripts."
}

fn direct_command_sentence() -> &'static str {
    "Plain `command` input is parsed into exact argv without a shell. In ConfirmAll/Open modes, recognizable shell syntax automatically uses the platform default shell; Restricted mode continues to reject it."
}

fn prompt_direct_command_sentence() -> &'static str {
    "Plain `command` input is parsed into exact argv without a shell. In ConfirmAll/Open modes, recognizable shell syntax automatically uses the platform default shell; Restricted mode rejects shell syntax."
}

fn shell_mode_sentence() -> &'static str {
    "Set `shell` explicitly when the interpreter choice matters. Shell execution is rejected in Restricted access and only works in ConfirmAll/Open."
}

fn prompt_shell_mode_sentence() -> &'static str {
    "Set `shell` when interpreter choice matters; explicit shell mode is rejected in Restricted access and, when allowed, uses PowerShell on Windows for `shell: \"default\"` and `sh` on Unix-like systems."
}

fn restricted_programs_sentence() -> String {
    format!(
        "In the default restricted mode, only whitelisted commands are allowed: {}, read-only git, and scoped filesystem commands like pwd/ls/cat/mkdir/cp/mv.",
        PROGRAM_WHITELIST
            .iter()
            .copied()
            .filter(|program| *program != "git" && !matches!(*program, "pwd" | "ls" | "cat" | "mkdir" | "cp" | "mv"))
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn prompt_restricted_programs_sentence() -> String {
    format!(
        "In Restricted mode, non-shell `run_shell` is limited to whitelisted programs (`{}`), read-only `git`, and scoped filesystem commands (`pwd`, `ls`, `cat`, `mkdir`, `cp`, `mv`); filesystem paths must stay inside registered sources.",
        PROGRAM_WHITELIST
            .iter()
            .copied()
            .filter(|program| !matches!(*program, "git" | "pwd" | "ls" | "cat" | "mkdir" | "cp" | "mv"))
            .collect::<Vec<_>>()
            .join("`, `")
    )
}

fn native_filesystem_sentence() -> &'static str {
    "Simple filesystem commands are implemented natively by the app, so use them directly for directory creation, copying, and moving instead of writing one-off Python snippets."
}

fn prompt_native_filesystem_sentence() -> &'static str {
    "Use app-native filesystem tools directly for simple directory/copy/move work."
}

fn plain_text_tools_sentence() -> &'static str {
    "For plain-text read/create/edit/list work, prefer read_file, read_files, create_file, edit_file, and list_dir over Python. Reserve Python for real scripts, structured document workflows, parsing/transforms, or operations that need libraries."
}

fn prompt_plain_text_tools_sentence() -> &'static str {
    "Do not write Python just to `mkdir`, list files, print a file, copy, move, create, or edit a plain-text path; reserve Python for real scripts, structured document work, parsing/transforms, or operations needing libraries."
}

fn large_payload_sentence() -> &'static str {
    "For large scripts or generated text, pass the content through `stdin` and use a program form that reads stdin (for example python with args [\"-\"]); do not stuff large content into argv."
}

fn prompt_large_payload_sentence() -> &'static str {
    "Do not pass large generated scripts or long file contents through `run_shell.args` or `python -c`; pass them through `run_shell.stdin` with a stdin-reading program, or use the appropriate file/document tool."
}

fn html_pptx_sentence() -> &'static str {
    "For HTML-first PPTX/deck generation, call the renderer with `--spec -` and put the generated JSON spec in `stdin`; never put raw HTML/CSS/JSON deck specs inside `args` or `python -c`."
}

fn prompt_html_pptx_sentence() -> &'static str {
    "For HTML-first PPTX generation, pass `--spec -` in `run_shell.args` and put the generated JSON spec in `run_shell.stdin`; never put raw HTML/CSS/JSON deck content inside `args`."
}

fn timeout_sentence() -> String {
    format!(
        "Output is capped at 64 KB per stream. Default timeout {DEFAULT_TIMEOUT_SECS}s; a timed-out command is killed but still returns the output it produced. Never sit on a long blocking timeout: for anything that may outlast the default, pass background=true and poll the returned service_id with service_action=wait (returns as soon as the process exits) or service_action=status. Local web/API servers are automatically promoted to managed background services; provide a loopback ready_url when known, or let the tool discover it from startup logs. Set timeout_secs to 0 only for a finite foreground install/download/build that must not be interrupted. The broader agent turn timeout can still stop foreground runs unless Settings disables or raises it."
    )
}

fn prompt_timeout_sentence() -> String {
    format!(
        "Output is capped at 64 KB per stream; default timeout {DEFAULT_TIMEOUT_SECS}s, and a timed-out command still returns the output it produced. Do not block on a long timeout and do not fake waiting with sleep commands: run anything that may exceed the default with `background: true`, then poll the returned `service_id` with `service_action: \"wait\"` (returns the moment the process exits, and returns a running snapshot when its own budget elapses so you can keep working) or `service_action: \"status\"` for a single snapshot. Prefer a loopback `ready_url` for local web/API servers; recognized servers are automatically promoted and can discover a URL from startup logs when it is omitted. Continue after the managed-service result, re-check with `status` around browser checks, and call `service_action: \"stop\"` when finished."
    )
}

fn windows_paths_sentence() -> &'static str {
    "For Windows paths in JSON strings, escape backslashes (for example \"E:\\\\Starting\\\\script.py\") or use forward slashes."
}

fn shell_parameter_description() -> &'static str {
    "Optional explicit shell mode for command. false/none/direct means no shell. true/default uses PowerShell on Windows and sh on Unix-like systems. powershell uses Windows PowerShell on Windows or pwsh elsewhere; pwsh uses PowerShell Core; cmd is Windows-only; bash uses bash -lc; sh uses sh -c. Shell mode permits shell syntax such as &&, pipes, redirects, variables, and command substitution, but is rejected in Restricted shell access mode and should only be used when shell interpretation is intentional."
}

fn timeout_parameter_description() -> String {
    format!(
        "Timeout in seconds for a foreground run. Default {DEFAULT_TIMEOUT_SECS}. A timed-out command is killed and still reports the output it produced. Set 0 only for a finite long install/download/build, never for a web/API server; use managed background mode plus service_action=wait for anything long-running. With service_action=wait this is the polling budget instead (default 30, max 900, 0 means the 900s cap). The broader agent turn timeout can still stop the run if it is not raised or disabled."
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn assert_snapshot(path: &str, actual: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        if std::env::var_os("UPDATE_RUN_SHELL_CONTRACT_SNAPSHOTS").is_some() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create snapshot dir");
            }
            std::fs::write(&path, actual).expect("write snapshot");
            return;
        }

        let expected = std::fs::read_to_string(&path).expect("read snapshot");
        assert_eq!(
            expected.trim_end().replace("\r\n", "\n"),
            actual.trim_end().replace("\r\n", "\n"),
            "snapshot mismatch: {}",
            path.display()
        );
    }

    #[test]
    fn run_shell_tool_definition_snapshot_matches_contract() {
        let actual = serde_json::to_string_pretty(&tool_definition_snapshot()).unwrap() + "\n";
        assert_snapshot(
            "src/tools/snapshots/run_shell_tool_definition.json",
            &actual,
        );
    }

    #[test]
    fn run_shell_prompt_section_snapshot_matches_contract() {
        let actual = format!("{}\n", system_prompt_section());
        assert_snapshot("src/tools/snapshots/run_shell_system_prompt.md", &actual);
    }

    #[test]
    fn run_shell_error_format_snapshot_matches_contract() {
        let actual = serde_json::to_string_pretty(&json!({
            "message": invalid_arguments_message("expected value at line 1 column 1"),
            "expectedFormat": expected_format()
        }))
        .unwrap()
            + "\n";
        assert_snapshot("src/tools/snapshots/run_shell_error_format.json", &actual);
    }
}
