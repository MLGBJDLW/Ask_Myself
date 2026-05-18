use std::path::{Path, PathBuf};
use std::time::Instant;

use super::super::run_shell_contract::MAX_OUTPUT_BYTES;
use super::policy::collect_positional_args;
use super::shell_adapter::{bytes_to_clamped_string, RunShellOutput};

pub(super) fn is_native_filesystem_program(program: &str) -> bool {
    matches!(program, "pwd" | "ls" | "cat" | "mkdir" | "cp" | "mv")
}

fn resolve_native_path(cwd: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn display_native_path(path: &Path) -> String {
    path.display().to_string()
}

fn native_ls_path(path: &Path) -> Result<String, String> {
    if path.is_file() {
        return Ok(format!("{}\n", display_native_path(path)));
    }
    if !path.is_dir() {
        return Err(format!(
            "ls: '{}' is not a file or directory",
            path.display()
        ));
    }

    let mut entries = std::fs::read_dir(path)
        .map_err(|e| format!("ls: cannot read '{}': {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("ls: failed while reading '{}': {e}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut out = String::new();
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|e| format!("ls: cannot stat '{}': {e}", entry.path().display()))?;
        let mut name = entry.file_name().to_string_lossy().to_string();
        if file_type.is_dir() {
            name.push(std::path::MAIN_SEPARATOR);
        }
        out.push_str(&name);
        out.push('\n');
    }
    Ok(out)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("cp: cannot create directory '{}': {e}", dest.display()))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("cp: cannot read directory '{}': {e}", src.display()))?
    {
        let entry =
            entry.map_err(|e| format!("cp: failed while reading '{}': {e}", src.display()))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cp: cannot stat '{}': {e}", from.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to).map_err(|e| {
                format!(
                    "cp: cannot copy '{}' to '{}': {e}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

fn copy_one(src: &Path, dest: &Path, recursive: bool) -> Result<(), String> {
    if src.is_dir() {
        if !recursive {
            return Err(format!(
                "cp: '{}' is a directory; pass -r for recursive copy",
                src.display()
            ));
        }
        let target = if dest.is_dir() {
            dest.join(src.file_name().ok_or_else(|| {
                format!(
                    "cp: cannot determine directory name for '{}'",
                    src.display()
                )
            })?)
        } else {
            dest.to_path_buf()
        };
        return copy_dir_recursive(src, &target);
    }

    if !src.is_file() {
        return Err(format!("cp: source not found: '{}'", src.display()));
    }

    let target = if dest.is_dir() {
        dest.join(
            src.file_name()
                .ok_or_else(|| format!("cp: cannot determine file name for '{}'", src.display()))?,
        )
    } else {
        dest.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cp: cannot create directory '{}': {e}", parent.display()))?;
    }
    std::fs::copy(src, &target).map_err(|e| {
        format!(
            "cp: cannot copy '{}' to '{}': {e}",
            src.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn move_one(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("mv: source not found: '{}'", src.display()));
    }
    let target = if dest.is_dir() {
        dest.join(
            src.file_name()
                .ok_or_else(|| format!("mv: cannot determine file name for '{}'", src.display()))?,
        )
    } else {
        dest.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mv: cannot create directory '{}': {e}", parent.display()))?;
    }
    std::fs::rename(src, &target).map_err(|e| {
        format!(
            "mv: cannot move '{}' to '{}': {e}",
            src.display(),
            target.display()
        )
    })
}

pub(super) fn execute_native_filesystem_inner(
    program: &str,
    args: &[String],
    cwd: &Path,
) -> Result<RunShellOutput, String> {
    let started = Instant::now();
    let positionals = collect_positional_args(args);
    let mut stdout = String::new();

    match program {
        "pwd" => {
            stdout.push_str(&display_native_path(cwd));
            stdout.push('\n');
        }
        "ls" => {
            let paths: Vec<&str> = if positionals.is_empty() {
                vec!["."]
            } else {
                positionals
            };
            for (index, raw) in paths.iter().enumerate() {
                let path = resolve_native_path(cwd, raw);
                if paths.len() > 1 {
                    if index > 0 {
                        stdout.push('\n');
                    }
                    stdout.push_str(&format!("{}:\n", display_native_path(&path)));
                }
                stdout.push_str(&native_ls_path(&path)?);
            }
        }
        "cat" => {
            if positionals.is_empty() {
                return Err("cat requires at least one file path".to_string());
            }
            for raw in positionals {
                let path = resolve_native_path(cwd, raw);
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("cat: cannot read '{}': {e}", path.display()))?;
                let (text, truncated) = bytes_to_clamped_string(&bytes, MAX_OUTPUT_BYTES);
                stdout.push_str(&text);
                if truncated {
                    stdout.push_str("\n[... truncated to 64KB]");
                    break;
                }
                if !stdout.ends_with('\n') {
                    stdout.push('\n');
                }
            }
        }
        "mkdir" => {
            if positionals.is_empty() {
                return Err("mkdir requires at least one path".to_string());
            }
            for raw in positionals {
                let path = resolve_native_path(cwd, raw);
                std::fs::create_dir_all(&path)
                    .map_err(|e| format!("mkdir: cannot create '{}': {e}", path.display()))?;
                stdout.push_str(&format!(
                    "created directory: {}\n",
                    display_native_path(&path)
                ));
            }
        }
        "cp" => {
            if positionals.len() < 2 {
                return Err("cp requires at least one source and one destination path".to_string());
            }
            let recursive = args
                .iter()
                .any(|arg| matches!(arg.as_str(), "-r" | "-R" | "--recursive"));
            let dest = resolve_native_path(cwd, positionals[positionals.len() - 1]);
            if positionals.len() > 2 && !dest.is_dir() {
                return Err(
                    "cp: destination must be an existing directory for multiple sources"
                        .to_string(),
                );
            }
            for raw in &positionals[..positionals.len() - 1] {
                copy_one(&resolve_native_path(cwd, raw), &dest, recursive)?;
            }
        }
        "mv" => {
            if positionals.len() < 2 {
                return Err("mv requires at least one source and one destination path".to_string());
            }
            let dest = resolve_native_path(cwd, positionals[positionals.len() - 1]);
            if positionals.len() > 2 && !dest.is_dir() {
                return Err(
                    "mv: destination must be an existing directory for multiple sources"
                        .to_string(),
                );
            }
            for raw in &positionals[..positionals.len() - 1] {
                move_one(&resolve_native_path(cwd, raw), &dest)?;
            }
        }
        _ => return Err(format!("'{program}' is not a native filesystem command")),
    }

    let (stdout, truncated_stdout) = bytes_to_clamped_string(stdout.as_bytes(), MAX_OUTPUT_BYTES);
    Ok(RunShellOutput {
        exit_code: Some(0),
        stdout,
        stderr: String::new(),
        duration_ms: started.elapsed().as_millis(),
        truncated_stdout,
        truncated_stderr: false,
        killed_by_timeout: false,
    })
}

pub(super) async fn execute_native_filesystem(
    program: &str,
    args: &[String],
    cwd: &Path,
) -> Result<RunShellOutput, String> {
    let program = program.to_string();
    let args = args.to_vec();
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || execute_native_filesystem_inner(&program, &args, &cwd))
        .await
        .map_err(|e| format!("native filesystem command failed to join: {e}"))?
}
