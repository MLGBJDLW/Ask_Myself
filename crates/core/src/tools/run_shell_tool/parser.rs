use serde_json::Value;

use super::super::run_shell_contract::{command_shell_operator_error, command_substitution_error};

#[derive(serde::Deserialize)]
pub(super) struct RunShellArgs {
    #[serde(default)]
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) shell: Option<Value>,
    #[serde(default)]
    pub(super) program: Option<String>,
    #[serde(default)]
    pub(super) args: Vec<String>,
    pub(super) cwd: String,
    #[serde(default)]
    pub(super) timeout_secs: Option<u64>,
    #[serde(default)]
    pub(super) stdin: Option<String>,
}

fn repair_invalid_json_string_escapes(input: &str) -> String {
    let mut repaired = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if !in_string {
            if ch == '"' {
                in_string = true;
            }
            repaired.push(ch);
            continue;
        }

        if escaped {
            if matches!(ch, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') {
                repaired.push(ch);
            } else {
                repaired.push('\\');
                repaired.push(ch);
            }
            escaped = false;
            continue;
        }

        match ch {
            '\\' => {
                repaired.push('\\');
                escaped = true;
            }
            '"' => {
                in_string = false;
                repaired.push(ch);
            }
            _ => repaired.push(ch),
        }
    }

    if escaped {
        repaired.push('\\');
    }

    repaired
}

pub(super) fn parse_run_shell_args(arguments: &str) -> Result<RunShellArgs, serde_json::Error> {
    match serde_json::from_str(arguments) {
        Ok(parsed) => Ok(parsed),
        Err(first_err) => {
            let repaired = repair_invalid_json_string_escapes(arguments);
            if repaired == arguments {
                Err(first_err)
            } else {
                serde_json::from_str(&repaired)
            }
        }
    }
}

pub(super) fn split_simple_command_string(command: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut token_started = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    push_double_quoted_backslash(&mut chars, &mut current)?;
                } else {
                    current.push(ch);
                }
            }
            Some(_) => unreachable!(),
            None => match ch {
                '\'' | '"' => {
                    token_started = true;
                    quote = Some(ch);
                }
                '\n' | '\r' => {
                    return Err("run_shell.command must be a single-line command".to_string());
                }
                c if c.is_whitespace() => {
                    if token_started {
                        parts.push(std::mem::take(&mut current));
                        token_started = false;
                    }
                }
                '\\' => {
                    token_started = true;
                    push_unquoted_backslash(&mut chars, &mut current)?;
                }
                '|' | ';' | '<' | '>' | '`' | '&' => {
                    return Err(command_shell_operator_error().to_string());
                }
                '$' if matches!(chars.peek(), Some('(')) => {
                    return Err(command_substitution_error().to_string());
                }
                _ => {
                    token_started = true;
                    current.push(ch);
                }
            },
        }
    }

    if let Some(ch) = quote {
        return Err(format!("command string has an unclosed {ch} quote"));
    }
    if token_started {
        parts.push(current);
    }
    if parts.is_empty() {
        return Err("run_shell requires either command or program".to_string());
    }

    Ok(parts)
}

fn push_unquoted_backslash(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    current: &mut String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = chars;
        current.push('\\');
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let next = chars
            .next()
            .ok_or_else(|| "command string ends with an unfinished escape".to_string())?;
        current.push(next);
        Ok(())
    }
}

fn push_double_quoted_backslash(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    current: &mut String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        if matches!(chars.peek(), Some('"')) {
            chars.next();
            current.push('"');
        } else {
            current.push('\\');
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let next = chars
            .next()
            .ok_or_else(|| "command string ends with an unfinished escape".to_string())?;
        current.push(next);
        Ok(())
    }
}
