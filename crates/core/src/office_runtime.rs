//! App-managed runtime checks for Python-backed Office document workflows.
//!
//! The advanced DOCX/XLSX/PPTX path uses Python libraries plus optional
//! LibreOffice/Poppler render helpers. This module keeps that dependency story
//! explicit, auditable, and local to the app.

use std::ffi::OsString;
#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

pub const OFFICE_PYTHON_BIN_DIR_ENV: &str = "NEXA_OFFICE_PYTHON_BIN_DIR";
pub const OFFICE_TOOLS_BIN_DIR_ENV: &str = "NEXA_OFFICE_TOOLS_BIN_DIR";

const OFFICE_ENV_DIR: &str = "runtimes/office-python";
const OFFICE_TOOLS_DIR: &str = "runtimes/office-tools";
const DOC_SCRIPT_SKILL: &str = "doc-script-editor";
#[cfg(windows)]
const POPPLER_RELEASE_API: &str =
    "https://api.github.com/repos/oschwartz10612/poppler-windows/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDependencyStatus {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub status: String,
    pub version: Option<String>,
    pub path: Option<String>,
    pub detail: Option<String>,
    pub install_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficeRuntimeReadiness {
    pub status: String,
    pub summary: String,
    pub python_path: Option<String>,
    pub app_managed_python_path: Option<String>,
    pub app_managed_env_path: String,
    pub skill_script_path: String,
    pub requirements_path: String,
    pub can_prepare: bool,
    pub can_install_python_packages: bool,
    pub needs_python_install: bool,
    pub python_download_url: String,
    pub dependencies: Vec<OfficeDependencyStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficePrepareAction {
    pub name: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficePrepareResult {
    pub success: bool,
    pub actions: Vec<OfficePrepareAction>,
    pub readiness: OfficeRuntimeReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficePrepareOptions {
    pub include_optional_tools: bool,
}

impl Default for OfficePrepareOptions {
    fn default() -> Self {
        Self {
            include_optional_tools: true,
        }
    }
}

#[derive(Debug, Clone)]
struct PythonCommand {
    program: OsString,
    prefix_args: Vec<OsString>,
    display: String,
}

impl PythonCommand {
    fn new(program: impl Into<OsString>) -> Self {
        let program = program.into();
        let display = program.to_string_lossy().to_string();
        Self {
            program,
            prefix_args: Vec::new(),
            display,
        }
    }

    fn with_prefix(program: impl Into<OsString>, prefix_args: Vec<OsString>) -> Self {
        let program = program.into();
        let display = format!(
            "{} {}",
            program.to_string_lossy(),
            prefix_args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        Self {
            program,
            prefix_args,
            display,
        }
    }

    fn run(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix_args)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.output()
    }
}

fn office_env_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(OFFICE_ENV_DIR)
}

fn office_tools_bin_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(OFFICE_TOOLS_DIR).join("bin")
}

fn office_python_path(app_data_dir: &Path) -> PathBuf {
    office_python_path_for_env(&office_env_dir(app_data_dir))
}

fn office_python_path_for_env(env_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        env_dir.join("Scripts").join("python.exe")
    } else {
        env_dir.join("bin").join("python")
    }
}

pub fn office_python_bin_dir_for_env(env_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        env_dir.join("Scripts")
    } else {
        env_dir.join("bin")
    }
}

pub fn configure_app_managed_python_env(app_data_dir: &Path) -> Option<PathBuf> {
    let env_dir = office_env_dir(app_data_dir);
    let python = office_python_path_for_env(&env_dir);
    let tools_bin = office_tools_bin_dir(app_data_dir);
    if tools_bin.exists() {
        std::env::set_var(OFFICE_TOOLS_BIN_DIR_ENV, &tools_bin);
    }
    if python.exists() {
        let bin_dir = office_python_bin_dir_for_env(&env_dir);
        std::env::set_var(OFFICE_PYTHON_BIN_DIR_ENV, &bin_dir);
        Some(bin_dir)
    } else {
        None
    }
}

fn command_success(cmd: &PythonCommand, args: &[&str]) -> bool {
    cmd.run(args)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn find_existing_python(app_data_dir: &Path) -> Option<PythonCommand> {
    let managed = office_python_path(app_data_dir);
    if managed.exists() {
        return Some(PythonCommand::new(managed.into_os_string()));
    }

    if let Some(explicit) = std::env::var_os("NEXA_PYTHON") {
        let cmd = PythonCommand::new(explicit);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    for name in ["python", "python3"] {
        let cmd = PythonCommand::new(name);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    if cfg!(windows) {
        let cmd = PythonCommand::with_prefix("py", vec![OsString::from("-3")]);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    None
}

fn find_system_python_for_venv() -> Option<PythonCommand> {
    if let Some(explicit) = std::env::var_os("NEXA_PYTHON") {
        let cmd = PythonCommand::new(explicit);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    for name in ["python", "python3"] {
        let cmd = PythonCommand::new(name);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    if cfg!(windows) {
        let cmd = PythonCommand::with_prefix("py", vec![OsString::from("-3")]);
        if command_success(&cmd, &["--version"]) {
            return Some(cmd);
        }
    }

    None
}

fn read_python_version(cmd: &PythonCommand) -> Option<String> {
    let output = cmd.run(&["--version"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    Some(text.trim().trim_start_matches("Python ").to_string())
}

fn command_status_success(program: impl AsRef<std::ffi::OsStr>, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_program_path(
    app_data_dir: &Path,
    names: &[&str],
    extra_paths: &[PathBuf],
) -> Option<String> {
    let managed_bin = office_tools_bin_dir(app_data_dir);
    for name in names {
        let candidate = managed_bin.join(name);
        if candidate.exists() && command_status_success(&candidate, "--version") {
            return Some(candidate.display().to_string());
        }
    }

    if let Some(env_bin) = std::env::var_os(OFFICE_TOOLS_BIN_DIR_ENV) {
        let env_bin = PathBuf::from(env_bin);
        for name in names {
            let candidate = env_bin.join(name);
            if candidate.exists() && command_status_success(&candidate, "--version") {
                return Some(candidate.display().to_string());
            }
        }
    }

    for path in extra_paths {
        if path.exists() && command_status_success(path, "--version") {
            return Some(path.display().to_string());
        }
    }

    for name in names {
        if command_status_success(name, "--version") {
            return Some((*name).to_string());
        }
    }
    None
}

fn libreoffice_extra_paths() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut paths = Vec::new();
        for env_key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(env_key) {
                let root = PathBuf::from(root);
                paths.push(root.join("LibreOffice").join("program").join("soffice.com"));
                paths.push(root.join("LibreOffice").join("program").join("soffice.exe"));
            }
        }
        paths
    }
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        )]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Vec::new()
    }
}

fn check_python_module(
    python: Option<&PythonCommand>,
    id: &str,
    module: &str,
    required: bool,
) -> OfficeDependencyStatus {
    let Some(python) = python else {
        return OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "python-package".to_string(),
            required,
            status: "missing".to_string(),
            version: None,
            path: None,
            detail: Some("Python is not available yet".to_string()),
            install_hint: Some(format!("python -m pip install {id}")),
        };
    };

    let code = format!("import {module} as m; print(getattr(m, '__version__', 'unknown'))");
    match python.run(&["-c", &code]) {
        Ok(output) if output.status.success() => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "python-package".to_string(),
            required,
            status: "ready".to_string(),
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            path: None,
            detail: None,
            install_hint: None,
        },
        Ok(output) => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "python-package".to_string(),
            required,
            status: "missing".to_string(),
            version: None,
            path: None,
            detail: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            install_hint: Some(format!("python -m pip install {id}")),
        },
        Err(e) => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "python-package".to_string(),
            required,
            status: "broken".to_string(),
            version: None,
            path: None,
            detail: Some(e.to_string()),
            install_hint: Some(format!("python -m pip install {id}")),
        },
    }
}

fn check_binary(
    app_data_dir: &Path,
    id: &str,
    names: &[&str],
    extra_paths: &[PathBuf],
    required: bool,
    detail: &str,
    install_hint: &str,
) -> OfficeDependencyStatus {
    match find_program_path(app_data_dir, names, extra_paths) {
        Some(path) => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "system-binary".to_string(),
            required,
            status: "ready".to_string(),
            version: None,
            path: Some(path),
            detail: None,
            install_hint: None,
        },
        None => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "system-binary".to_string(),
            required,
            status: "missing".to_string(),
            version: None,
            path: None,
            detail: Some(detail.to_string()),
            install_hint: Some(install_hint.to_string()),
        },
    }
}

fn derive_status(has_python: bool, dependencies: &[OfficeDependencyStatus]) -> (String, String) {
    if !has_python {
        return (
            "blocked".to_string(),
            "Python is not installed. Basic Office fallback tools remain available, but advanced Office workflows need Python.".to_string(),
        );
    }

    let missing_required = dependencies
        .iter()
        .any(|dep| dep.required && dep.status != "ready");
    if missing_required {
        return (
            "missing".to_string(),
            "Python is available, but required Office packages are missing.".to_string(),
        );
    }

    let missing_optional = dependencies
        .iter()
        .any(|dep| !dep.required && dep.status != "ready");
    if missing_optional {
        return (
            "degraded".to_string(),
            "Core Office editing is ready. Rendering, conversion, or formula recalculation may need LibreOffice and Poppler.".to_string(),
        );
    }

    (
        "ready".to_string(),
        "All Python-backed Office document tools are ready.".to_string(),
    )
}

pub fn check_office_runtime(app_data_dir: &Path) -> OfficeRuntimeReadiness {
    let env_dir = office_env_dir(app_data_dir);
    let managed_python = office_python_path_for_env(&env_dir);
    let python = find_existing_python(app_data_dir);
    let python_version = python.as_ref().and_then(read_python_version);
    let python_path = python.as_ref().map(|cmd| cmd.display.clone()).or_else(|| {
        if managed_python.exists() {
            Some(managed_python.display().to_string())
        } else {
            None
        }
    });

    let mut dependencies = Vec::new();
    dependencies.push(OfficeDependencyStatus {
        id: "python".to_string(),
        label: "Python 3".to_string(),
        kind: "runtime".to_string(),
        required: true,
        status: if python.is_some() { "ready" } else { "missing" }.to_string(),
        version: python_version,
        path: python_path.clone(),
        detail: if python.is_none() {
            Some("Install Python 3.10+ or run the app-managed setup again after Python is installed.".to_string())
        } else {
            None
        },
        install_hint: if python.is_none() {
            Some("https://www.python.org/downloads/".to_string())
        } else {
            None
        },
    });
    dependencies.push(check_python_module(
        python.as_ref(),
        "python-docx",
        "docx",
        true,
    ));
    dependencies.push(check_python_module(
        python.as_ref(),
        "openpyxl",
        "openpyxl",
        true,
    ));
    dependencies.push(check_python_module(
        python.as_ref(),
        "python-pptx",
        "pptx",
        true,
    ));
    dependencies.push(check_python_module(python.as_ref(), "pypdf", "pypdf", true));
    dependencies.push(check_binary(
        app_data_dir,
        "LibreOffice",
        &["soffice", "soffice.com", "soffice.exe", "libreoffice"],
        &libreoffice_extra_paths(),
        false,
        "LibreOffice enables Office-to-PDF conversion, rendering QA, and Excel recalculation.",
        "Click Prepare to install via winget/Homebrew when available, or install LibreOffice manually.",
    ));
    dependencies.push(check_binary(
        app_data_dir,
        "Poppler",
        &["pdftoppm", "pdftoppm.exe"],
        &[],
        false,
        "Poppler enables DOCX/PPTX/PDF page image rendering.",
        "Click Prepare to install an app-managed Poppler on Windows, or install Poppler manually.",
    ));

    let (status, summary) = derive_status(python.is_some(), &dependencies);
    let skill_dir = crate::skills::builtin_skill_dir(app_data_dir, DOC_SCRIPT_SKILL);
    let system_python = find_system_python_for_venv();

    OfficeRuntimeReadiness {
        status,
        summary,
        python_path,
        app_managed_python_path: managed_python
            .exists()
            .then(|| managed_python.display().to_string()),
        app_managed_env_path: env_dir.display().to_string(),
        skill_script_path: skill_dir
            .join("scripts")
            .join("edit_doc.py")
            .display()
            .to_string(),
        requirements_path: skill_dir
            .join("scripts")
            .join("requirements.txt")
            .display()
            .to_string(),
        can_prepare: system_python.is_some() || managed_python.exists(),
        can_install_python_packages: python.is_some() || system_python.is_some(),
        needs_python_install: system_python.is_none() && !managed_python.exists(),
        python_download_url: "https://www.python.org/downloads/".to_string(),
        dependencies,
    }
}

fn push_action(
    actions: &mut Vec<OfficePrepareAction>,
    name: impl Into<String>,
    status: impl Into<String>,
    detail: Option<String>,
) {
    actions.push(OfficePrepareAction {
        name: name.into(),
        status: status.into(),
        detail,
    });
}

fn run_step(cmd: &PythonCommand, args: &[&str]) -> Result<String, String> {
    let output = cmd
        .run(args)
        .map_err(|e| format!("failed to spawn {}: {e}", cmd.display))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn run_system_step(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to spawn {program}: {e}"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
        } else {
            Ok(stdout)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn tool_available(app_data_dir: &Path, names: &[&str], extra_paths: &[PathBuf]) -> bool {
    find_program_path(app_data_dir, names, extra_paths).is_some()
}

fn prepare_optional_office_tools(
    app_data_dir: &Path,
    ghproxy_base: &str,
    actions: &mut Vec<OfficePrepareAction>,
) {
    prepare_poppler(app_data_dir, ghproxy_base, actions);
    prepare_libreoffice(app_data_dir, actions);
}

fn prepare_poppler(
    app_data_dir: &Path,
    #[cfg(windows)] ghproxy_base: &str,
    #[cfg(not(windows))] _ghproxy_base: &str,
    actions: &mut Vec<OfficePrepareAction>,
) {
    if tool_available(app_data_dir, &["pdftoppm", "pdftoppm.exe"], &[]) {
        push_action(
            actions,
            "install-poppler",
            "skipped",
            Some("Poppler is already available.".to_string()),
        );
        return;
    }

    #[cfg(windows)]
    {
        match download_poppler_windows(app_data_dir, ghproxy_base) {
            Ok(path) => {
                std::env::set_var(OFFICE_TOOLS_BIN_DIR_ENV, office_tools_bin_dir(app_data_dir));
                push_action(
                    actions,
                    "install-poppler",
                    "ok",
                    Some(format!(
                        "Installed app-managed Poppler at {}",
                        path.display()
                    )),
                );
            }
            Err(download_err) => {
                if command_status_success("winget", "--version") {
                    match run_system_step(
                        "winget",
                        &[
                            "install",
                            "--id",
                            "oschwartz10612.Poppler",
                            "--exact",
                            "--source",
                            "winget",
                            "--accept-source-agreements",
                            "--accept-package-agreements",
                            "--disable-interactivity",
                        ],
                    ) {
                        Ok(detail) => {
                            push_action(actions, "install-poppler", "ok", Some(detail));
                            return;
                        }
                        Err(winget_err) => {
                            push_action(
                                actions,
                                "install-poppler",
                                "warning",
                                Some(format!(
                                    "App-managed download failed: {download_err}; winget failed: {winget_err}"
                                )),
                            );
                            return;
                        }
                    }
                }
                push_action(
                    actions,
                    "install-poppler",
                    "warning",
                    Some(format!(
                        "App-managed download failed: {download_err}. Install Poppler manually or install winget package oschwartz10612.Poppler."
                    )),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if command_status_success("brew", "--version") {
            match run_system_step("brew", &["install", "poppler"]) {
                Ok(detail) => {
                    push_action(actions, "install-poppler", "ok", Some(detail));
                    return;
                }
                Err(detail) => {
                    push_action(actions, "install-poppler", "warning", Some(detail));
                    return;
                }
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        push_action(
            actions,
            "install-poppler",
            "skipped",
            Some("Automatic Poppler install is not supported on this platform yet. Install poppler-utils with your system package manager.".to_string()),
        );
    }

    #[cfg(target_os = "macos")]
    push_action(
        actions,
        "install-poppler",
        "skipped",
        Some("Homebrew is not available. Install Poppler manually with Homebrew or your preferred package manager.".to_string()),
    );
}

fn prepare_libreoffice(app_data_dir: &Path, actions: &mut Vec<OfficePrepareAction>) {
    if tool_available(
        app_data_dir,
        &["soffice", "soffice.com", "soffice.exe", "libreoffice"],
        &libreoffice_extra_paths(),
    ) {
        push_action(
            actions,
            "install-libreoffice",
            "skipped",
            Some("LibreOffice is already available.".to_string()),
        );
        return;
    }

    #[cfg(windows)]
    {
        if command_status_success("winget", "--version") {
            match run_system_step(
                "winget",
                &[
                    "install",
                    "--id",
                    "TheDocumentFoundation.LibreOffice",
                    "--exact",
                    "--source",
                    "winget",
                    "--accept-source-agreements",
                    "--accept-package-agreements",
                    "--disable-interactivity",
                ],
            ) {
                Ok(detail) => {
                    push_action(actions, "install-libreoffice", "ok", Some(detail));
                    return;
                }
                Err(detail) => {
                    push_action(actions, "install-libreoffice", "warning", Some(detail));
                    return;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if command_status_success("brew", "--version") {
            match run_system_step("brew", &["install", "--cask", "libreoffice"]) {
                Ok(detail) => {
                    push_action(actions, "install-libreoffice", "ok", Some(detail));
                    return;
                }
                Err(detail) => {
                    push_action(actions, "install-libreoffice", "warning", Some(detail));
                    return;
                }
            }
        }
    }

    push_action(
        actions,
        "install-libreoffice",
        "skipped",
        Some("Automatic LibreOffice install is unavailable. Install LibreOffice manually for Office-to-PDF conversion, rendering QA, and Excel recalculation.".to_string()),
    );
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    assets: Vec<GitHubAsset>,
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(windows)]
fn download_poppler_windows(app_data_dir: &Path, ghproxy_base: &str) -> Result<PathBuf, CoreError> {
    let tools_bin = office_tools_bin_dir(app_data_dir);
    std::fs::create_dir_all(&tools_bin).map_err(CoreError::Io)?;
    let pdftoppm = tools_bin.join("pdftoppm.exe");
    if pdftoppm.exists() {
        return Ok(pdftoppm);
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(15))
        .user_agent("Nexa Office runtime")
        .build()
        .map_err(|e| CoreError::Internal(format!("Office HTTP client error: {e}")))?;

    let release: GitHubRelease = client
        .get(POPPLER_RELEASE_API)
        .send()
        .and_then(|resp| resp.error_for_status())
        .map_err(|e| CoreError::Internal(format!("Failed to query Poppler release: {e}")))?
        .json()
        .map_err(|e| CoreError::Internal(format!("Failed to parse Poppler release: {e}")))?;

    let asset = release
        .assets
        .iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.ends_with(".zip") && name.contains("release")
        })
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|asset| asset.name.to_ascii_lowercase().ends_with(".zip"))
        })
        .ok_or_else(|| CoreError::Internal("No Poppler Windows zip asset found".to_string()))?;

    let archive_path = tools_bin.join("poppler-download.zip");
    let mirror_url = mirror_github_download_url(&asset.browser_download_url, ghproxy_base);
    download_file_with_fallback(
        &client,
        &asset.browser_download_url,
        mirror_url.as_deref(),
        &archive_path,
    )?;
    extract_poppler_windows_bin(&archive_path, &tools_bin)?;
    let _ = std::fs::remove_file(&archive_path);

    if !pdftoppm.exists() {
        return Err(CoreError::Internal(
            "Poppler archive did not contain pdftoppm.exe".to_string(),
        ));
    }
    Ok(pdftoppm)
}

#[cfg(windows)]
fn mirror_github_download_url(url: &str, ghproxy_base: &str) -> Option<String> {
    let base = ghproxy_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(format!("{base}/{url}"))
}

#[cfg(windows)]
fn download_file_with_fallback(
    client: &reqwest::blocking::Client,
    primary_url: &str,
    mirror_url: Option<&str>,
    dest: &Path,
) -> Result<(), CoreError> {
    match download_file(client, primary_url, dest) {
        Ok(()) => Ok(()),
        Err(primary_err) => {
            let _ = std::fs::remove_file(dest);
            if let Some(mirror) = mirror_url {
                download_file(client, mirror, dest).map_err(|mirror_err| {
                    CoreError::Internal(format!("primary: {primary_err}; mirror: {mirror_err}"))
                })
            } else {
                Err(primary_err)
            }
        }
    }
}

#[cfg(windows)]
fn download_file(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
) -> Result<(), CoreError> {
    let resp = client
        .get(url)
        .send()
        .and_then(|resp| resp.error_for_status())
        .map_err(|e| CoreError::Internal(format!("Failed to download {url}: {e}")))?;

    let mut reader = std::io::BufReader::new(resp);
    let mut file = File::create(dest).map_err(CoreError::Io)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(CoreError::Io)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(CoreError::Io)?;
    }
    Ok(())
}

#[cfg(windows)]
fn extract_poppler_windows_bin(archive_path: &Path, dest_bin: &Path) -> Result<(), CoreError> {
    let file = File::open(archive_path).map_err(CoreError::Io)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Internal(format!("Failed to read Poppler zip: {e}")))?;
    let mut found_pdftoppm = false;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::Internal(format!("Failed to read Poppler zip entry: {e}")))?;
        let entry_name = entry.name().replace('\\', "/");
        let Some(file_name) = entry_name.rsplit('/').next() else {
            continue;
        };
        if file_name.is_empty()
            || !(entry_name.contains("/Library/bin/") || entry_name.starts_with("Library/bin/"))
        {
            continue;
        }

        let dest = dest_bin.join(file_name);
        let mut out = File::create(&dest).map_err(CoreError::Io)?;
        std::io::copy(&mut entry, &mut out).map_err(CoreError::Io)?;
        if file_name.eq_ignore_ascii_case("pdftoppm.exe") {
            found_pdftoppm = true;
        }
    }

    if found_pdftoppm {
        Ok(())
    } else {
        Err(CoreError::Internal(
            "pdftoppm.exe was not found in Poppler zip".to_string(),
        ))
    }
}

pub fn prepare_office_runtime(app_data_dir: &Path) -> Result<OfficePrepareResult, CoreError> {
    prepare_office_runtime_with_options(app_data_dir, "")
}

pub fn prepare_office_runtime_with_options(
    app_data_dir: &Path,
    ghproxy_base: &str,
) -> Result<OfficePrepareResult, CoreError> {
    prepare_office_runtime_with_prepare_options(
        app_data_dir,
        ghproxy_base,
        OfficePrepareOptions::default(),
    )
}

pub fn prepare_office_runtime_with_prepare_options(
    app_data_dir: &Path,
    ghproxy_base: &str,
    options: OfficePrepareOptions,
) -> Result<OfficePrepareResult, CoreError> {
    crate::skills::materialize_skills_to_disk(app_data_dir)?;

    let env_dir = office_env_dir(app_data_dir);
    let managed_python = office_python_path_for_env(&env_dir);
    let mut actions = Vec::new();

    if !managed_python.exists() {
        let Some(system_python) = find_system_python_for_venv() else {
            let readiness = check_office_runtime(app_data_dir);
            push_action(
                &mut actions,
                "python",
                "blocked",
                Some(
                    "Python 3 is not installed. Install Python first, then run Prepare again."
                        .to_string(),
                ),
            );
            return Ok(OfficePrepareResult {
                success: false,
                actions,
                readiness,
            });
        };

        if let Some(parent) = env_dir.parent() {
            std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
        }
        match run_step(&system_python, &["-m", "venv", &env_dir.to_string_lossy()]) {
            Ok(detail) => push_action(&mut actions, "create-venv", "ok", Some(detail)),
            Err(detail) => {
                let readiness = check_office_runtime(app_data_dir);
                push_action(&mut actions, "create-venv", "failed", Some(detail));
                return Ok(OfficePrepareResult {
                    success: false,
                    actions,
                    readiness,
                });
            }
        }
    } else {
        push_action(
            &mut actions,
            "create-venv",
            "skipped",
            Some("App-managed Office Python environment already exists.".to_string()),
        );
    }

    let managed = PythonCommand::new(managed_python.clone().into_os_string());
    let _ = run_step(&managed, &["-m", "ensurepip", "--upgrade"]);
    match run_step(&managed, &["-m", "pip", "install", "--upgrade", "pip"]) {
        Ok(detail) => push_action(&mut actions, "upgrade-pip", "ok", Some(detail)),
        Err(detail) => push_action(&mut actions, "upgrade-pip", "warning", Some(detail)),
    }

    let requirements = crate::skills::builtin_skill_dir(app_data_dir, DOC_SCRIPT_SKILL)
        .join("scripts")
        .join("requirements.txt");
    match run_step(
        &managed,
        &[
            "-m",
            "pip",
            "install",
            "-r",
            &requirements.to_string_lossy(),
        ],
    ) {
        Ok(detail) => push_action(&mut actions, "install-office-packages", "ok", Some(detail)),
        Err(detail) => {
            let readiness = check_office_runtime(app_data_dir);
            push_action(
                &mut actions,
                "install-office-packages",
                "failed",
                Some(detail),
            );
            return Ok(OfficePrepareResult {
                success: false,
                actions,
                readiness,
            });
        }
    }

    if options.include_optional_tools {
        prepare_optional_office_tools(app_data_dir, ghproxy_base, &mut actions);
    } else {
        push_action(
            &mut actions,
            "optional-office-tools",
            "skipped",
            Some("Skipped optional LibreOffice/Poppler setup.".to_string()),
        );
    }
    configure_app_managed_python_env(app_data_dir);
    let readiness = check_office_runtime(app_data_dir);
    let success = matches!(readiness.status.as_str(), "ready" | "degraded");
    Ok(OfficePrepareResult {
        success,
        actions,
        readiness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(id: &str, required: bool, status: &str) -> OfficeDependencyStatus {
        OfficeDependencyStatus {
            id: id.into(),
            label: id.into(),
            kind: "test".into(),
            required,
            status: status.into(),
            version: None,
            path: None,
            detail: None,
            install_hint: None,
        }
    }

    #[test]
    fn derives_blocked_without_python() {
        let deps = vec![dep("python", true, "missing")];
        let (status, summary) = derive_status(false, &deps);
        assert_eq!(status, "blocked");
        assert!(summary.contains("Python"));
    }

    #[test]
    fn derives_missing_when_required_package_missing() {
        let deps = vec![
            dep("python", true, "ready"),
            dep("python-docx", true, "missing"),
        ];
        let (status, _) = derive_status(true, &deps);
        assert_eq!(status, "missing");
    }

    #[test]
    fn derives_degraded_when_only_optional_tools_missing() {
        let deps = vec![
            dep("python", true, "ready"),
            dep("python-docx", true, "ready"),
            dep("LibreOffice", false, "missing"),
        ];
        let (status, summary) = derive_status(true, &deps);
        assert_eq!(status, "degraded");
        assert!(summary.contains("Core Office editing is ready"));
    }

    #[test]
    fn office_python_path_uses_platform_layout() {
        let env = PathBuf::from("runtime");
        let path = office_python_path_for_env(&env);
        let rendered = path.to_string_lossy();
        if cfg!(windows) {
            assert!(rendered.ends_with("runtime\\Scripts\\python.exe"));
        } else {
            assert!(rendered.ends_with("runtime/bin/python"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn extracts_poppler_library_bin_from_windows_zip() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("poppler.zip");
        {
            let file = File::create(&archive_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("poppler-25.0/Library/bin/pdftoppm.exe", options)
                .unwrap();
            zip.write_all(b"fake exe").unwrap();
            zip.start_file("poppler-25.0/Library/bin/libpoppler.dll", options)
                .unwrap();
            zip.write_all(b"fake dll").unwrap();
            zip.start_file("poppler-25.0/share/readme.txt", options)
                .unwrap();
            zip.write_all(b"skip").unwrap();
            zip.finish().unwrap();
        }

        let dest = dir.path().join("bin");
        std::fs::create_dir_all(&dest).unwrap();
        extract_poppler_windows_bin(&archive_path, &dest).unwrap();
        assert!(dest.join("pdftoppm.exe").exists());
        assert!(dest.join("libpoppler.dll").exists());
        assert!(!dest.join("readme.txt").exists());
    }
}
