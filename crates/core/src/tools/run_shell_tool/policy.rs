use std::path::Path;

use crate::app_settings::ShellAccessMode;
use crate::models::Source;

use super::super::path_utils::{resolve_path_from_base_in_sources, PathKind};
use super::super::run_shell_contract::{
    command_args_mix_error, command_program_mix_error, program_not_allowed_message,
    shell_mix_error, shell_restricted_error, FORBIDDEN_ARG_SUBSTRINGS, GIT_FORBIDDEN_TOKENS,
    GIT_READONLY_SUBCOMMANDS, MAX_SINGLE_ARG_BYTES, MAX_STDIN_BYTES, MAX_TOTAL_ARGV_BYTES,
    PROGRAM_ALIASES, PROGRAM_WHITELIST,
};
use super::parser::{split_simple_command_string, RunShellArgs};
use super::shell_adapter::{parse_shell_selector, shell_invocation};

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
fn normalize_program_alias(program: &str) -> Option<&'static str> {
    PROGRAM_ALIASES
        .iter()
        .find(|(alias, _)| program_matches(program, alias))
        .map(|(_, canonical)| *canonical)
}

pub(super) fn validate_program(program: &str, mode: ShellAccessMode) -> Result<String, String> {
    if program.is_empty() {
        return Err("program must not be empty".to_string());
    }
    if program.contains('/') || program.contains('\\') {
        return Err(
            "program must be a bare name (no path separators); only whitelisted commands are allowed"
                .to_string(),
        );
    }
    if let Some(canonical) = normalize_program_alias(program) {
        return Ok(canonical.to_string());
    }
    for &canonical in PROGRAM_WHITELIST {
        if program_matches(program, canonical) {
            return Ok(canonical.to_string());
        }
    }
    if !mode.is_restricted() {
        return Ok(program.to_string());
    }
    Err(program_not_allowed_message(program))
}

fn normalize_invocation(
    program: &str,
    args: &[String],
    mode: ShellAccessMode,
) -> Result<(String, Vec<String>), String> {
    let canonical = validate_program(program, mode)?;
    if program_matches(&canonical, "pip") {
        let mut normalized_args = Vec::with_capacity(args.len() + 2);
        normalized_args.push("-m".to_string());
        normalized_args.push("pip".to_string());
        normalized_args.extend(args.iter().cloned());
        return Ok(("python".to_string(), normalized_args));
    }
    if program_matches(&canonical, "pip3") {
        let mut normalized_args = Vec::with_capacity(args.len() + 2);
        normalized_args.push("-m".to_string());
        normalized_args.push("pip".to_string());
        normalized_args.extend(args.iter().cloned());
        return Ok(("python3".to_string(), normalized_args));
    }
    Ok((canonical, args.to_vec()))
}

pub(super) fn normalize_run_shell_invocation(
    parsed: &RunShellArgs,
    mode: ShellAccessMode,
) -> Result<(String, Vec<String>), String> {
    let command = parsed
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let shell = parse_shell_selector(parsed.shell.as_ref())?;
    let program = parsed.program.as_deref().filter(|s| !s.is_empty());

    if let Some(shell) = shell {
        if mode.is_restricted() {
            return Err(shell_restricted_error().to_string());
        }
        if program.is_some() || !parsed.args.is_empty() {
            return Err(shell_mix_error().to_string());
        }
        let Some(command) = command else {
            return Err("run_shell.shell requires command".to_string());
        };
        return shell_invocation(shell, command);
    }

    match (command, program) {
        (Some(_), Some(_)) => Err(command_program_mix_error().to_string()),
        (Some(command), None) => {
            if !parsed.args.is_empty() {
                return Err(command_args_mix_error().to_string());
            }
            let parts = split_simple_command_string(command)?;
            let program = &parts[0];
            let args = parts[1..].to_vec();
            normalize_invocation(program, &args, mode)
        }
        (None, Some(program)) => normalize_invocation(program, &parsed.args, mode),
        (None, None) => Err("run_shell requires either command or program".to_string()),
    }
}

/// Reject unsafe argv patterns.
pub(super) fn validate_args(
    mode: ShellAccessMode,
    program: &str,
    args: &[String],
) -> Result<(), String> {
    let mut total = 0usize;
    for (i, arg) in args.iter().enumerate() {
        if arg.len() > MAX_SINGLE_ARG_BYTES {
            return Err(format!(
                "argument #{i} exceeds {MAX_SINGLE_ARG_BYTES} bytes"
            ));
        }
        for forbidden in FORBIDDEN_ARG_SUBSTRINGS {
            if arg.contains(forbidden) {
                return Err(format!("argument #{i} contains forbidden byte sequence"));
            }
        }
        total = total.saturating_add(arg.len());
    }
    if total > MAX_TOTAL_ARGV_BYTES {
        return Err(format!(
            "total argv size ({total} bytes) exceeds {MAX_TOTAL_ARGV_BYTES}"
        ));
    }

    if !mode.is_restricted() {
        return Ok(());
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
                return Err("git config requires an explicit read-only flag \
                     (--get, --list, --get-all, --get-regexp, --get-urlmatch, \
                     -l, --show-origin, --show-scope)"
                    .to_string());
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

    if program_matches(program, "pwd") && !args.is_empty() {
        return Err("pwd does not accept any arguments".to_string());
    }

    Ok(())
}

pub(super) fn validate_stdin(stdin: Option<&str>) -> Result<(), String> {
    let Some(input) = stdin else {
        return Ok(());
    };
    let bytes = input.len();
    if bytes > MAX_STDIN_BYTES {
        return Err(format!(
            "stdin payload ({bytes} bytes) exceeds {MAX_STDIN_BYTES}"
        ));
    }
    Ok(())
}

pub(super) fn collect_positional_args(args: &[String]) -> Vec<&str> {
    let mut positionals = Vec::new();
    let mut after_double_dash = false;

    for arg in args {
        if after_double_dash {
            positionals.push(arg.as_str());
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            continue;
        }
        positionals.push(arg.as_str());
    }

    positionals
}

pub(super) fn validate_scoped_args(
    mode: ShellAccessMode,
    program: &str,
    args: &[String],
    cwd: &Path,
    sources: &[Source],
) -> Result<(), String> {
    if !mode.is_restricted() {
        return Ok(());
    }

    let positionals = collect_positional_args(args);

    match program {
        "ls" => {
            for path in positionals {
                resolve_path_from_base_in_sources(
                    Path::new(path),
                    cwd,
                    sources,
                    PathKind::Any,
                    false,
                )?;
            }
        }
        "cat" => {
            if positionals.is_empty() {
                return Err("cat requires at least one file path".to_string());
            }
            for path in positionals {
                resolve_path_from_base_in_sources(
                    Path::new(path),
                    cwd,
                    sources,
                    PathKind::File,
                    false,
                )?;
            }
        }
        "mkdir" => {
            if positionals.is_empty() {
                return Err("mkdir requires at least one path".to_string());
            }
            for path in positionals {
                resolve_path_from_base_in_sources(
                    Path::new(path),
                    cwd,
                    sources,
                    PathKind::Directory,
                    true,
                )?;
            }
        }
        "cp" | "mv" => {
            if positionals.len() < 2 {
                return Err(format!(
                    "{program} requires at least one source and one destination path"
                ));
            }
            for path in &positionals[..positionals.len() - 1] {
                resolve_path_from_base_in_sources(
                    Path::new(path),
                    cwd,
                    sources,
                    PathKind::Any,
                    false,
                )?;
            }
            resolve_path_from_base_in_sources(
                Path::new(positionals[positionals.len() - 1]),
                cwd,
                sources,
                PathKind::Any,
                true,
            )?;
        }
        _ => {}
    }

    Ok(())
}
