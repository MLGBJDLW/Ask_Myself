//! App-managed runtime checks for Python-backed Office document workflows.
//!
//! The advanced DOCX/XLSX/PPTX path uses Python libraries. This module keeps
//! that dependency story explicit, auditable, and local to the app.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CoreError;

pub const OFFICE_PYTHON_BIN_DIR_ENV: &str = "NEXA_OFFICE_PYTHON_BIN_DIR";
pub const OPENXML_VALIDATOR_ENV: &str = "NEXA_OPENXML_VALIDATOR";
pub const OFFICE_ADDIN_MANIFEST_ENV: &str = "NEXA_OFFICE_ADDIN_MANIFEST";
pub const PPTXGENJS_NODE_ENV: &str = "NEXA_PPTXGENJS_NODE";
pub const PPTXGENJS_MODULE_ROOT_ENV: &str = "NEXA_PPTXGENJS_MODULE_ROOT";

const OFFICE_ENV_DIR: &str = "runtimes/office-python";
const DOC_SCRIPT_SKILL: &str = "doc-script-editor";
#[cfg(windows)]
const SEM_FAILCRITICALERRORS: u32 = 0x0001;
#[cfg(windows)]
const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
#[cfg(windows)]
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

#[cfg(windows)]
extern "system" {
    fn SetErrorMode(u_mode: u32) -> u32;
}

#[cfg(windows)]
fn with_suppressed_process_error_dialogs<T>(f: impl FnOnce() -> T) -> T {
    let previous = unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };
    let result = f();
    unsafe {
        SetErrorMode(previous);
    }
    result
}

#[cfg(not(windows))]
fn with_suppressed_process_error_dialogs<T>(f: impl FnOnce() -> T) -> T {
    f()
}

fn apply_quiet_command_options(cmd: &mut Command) {
    crate::background_process::configure_std_background(cmd);
}

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
        apply_quiet_command_options(&mut cmd);
        with_suppressed_process_error_dialogs(|| cmd.output())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficeArtifactExecution {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

async fn run_python_with_input(
    python: &PythonCommand,
    args: &[String],
    cwd: &Path,
    input: &str,
    timeout: Duration,
    integrity_root: Option<&Path>,
) -> Result<std::process::Output, CoreError> {
    use tokio::io::AsyncWriteExt;

    let mut command = tokio::process::Command::new(&python.program);
    command
        .args(&python.prefix_args)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(root) = integrity_root {
        command.env("NEXA_OFFICE_INTEGRITY_ROOT", root);
    }
    apply_quiet_command_options(command.as_std_mut());
    let mut child = command
        .spawn()
        .map_err(|error| CoreError::Internal(format!("Office artifact engine failed: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).await.map_err(|error| {
            CoreError::Internal(format!("Office artifact stdin failed: {error}"))
        })?;
    }
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            CoreError::Internal(format!(
                "Office artifact engine exceeded its {} second watchdog",
                timeout.as_secs()
            ))
        })?
        .map_err(|error| CoreError::Internal(format!("Office artifact engine failed: {error}")))
}

pub async fn execute_office_artifact_engine(
    app_data_dir: &Path,
    workspace_root: &Path,
    arguments: &[String],
    request_json: &str,
) -> Result<OfficeArtifactExecution, CoreError> {
    let python = find_existing_python(app_data_dir).ok_or_else(|| {
        CoreError::InvalidInput(
            "Office artifact engine requires Python; run prepare_document_tools first".to_string(),
        )
    })?;
    let engine = crate::skills::builtin_skill_dir(app_data_dir, DOC_SCRIPT_SKILL)
        .join("scripts")
        .join("office_artifact_engine.py");
    if !engine.is_file() {
        return Err(CoreError::Internal(format!(
            "Office artifact engine script is missing: {}",
            engine.display()
        )));
    }
    if !workspace_root.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Office artifact workspace is not a directory: {}",
            workspace_root.display()
        )));
    }
    let mut owned_args = vec![engine.display().to_string()];
    owned_args.extend(arguments.iter().cloned());
    let output = run_python_with_input(
        &python,
        &owned_args,
        workspace_root,
        request_json,
        Duration::from_secs(15 * 60),
        Some(&app_data_dir.join("office-artifact-integrity")),
    )
    .await?;
    Ok(OfficeArtifactExecution {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn office_env_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(OFFICE_ENV_DIR)
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
    if python.exists() {
        let bin_dir = office_python_bin_dir_for_env(&env_dir);
        std::env::set_var(OFFICE_PYTHON_BIN_DIR_ENV, &bin_dir);
        Some(bin_dir)
    } else {
        None
    }
}

pub fn configure_bundled_openxml_validator(
    app_data_dir: &Path,
    resource_dir: &Path,
) -> Result<Option<PathBuf>, CoreError> {
    let filename = if cfg!(windows) {
        "Nexa.OpenXml.Validator.exe"
    } else {
        "Nexa.OpenXml.Validator"
    };
    let source = resource_dir.join("openxml-validator").join(filename);
    if !source.is_file() {
        return Ok(None);
    }
    let destination_dir = app_data_dir.join("runtimes/openxml-validator/3.5.1");
    std::fs::create_dir_all(&destination_dir).map_err(CoreError::Io)?;
    let destination = destination_dir.join(filename);
    let should_copy = match (std::fs::metadata(&source), std::fs::metadata(&destination)) {
        (Ok(source_meta), Ok(destination_meta)) => source_meta.len() != destination_meta.len(),
        (Ok(_), Err(_)) => true,
        (Err(error), _) => return Err(CoreError::Io(error)),
    };
    if should_copy {
        std::fs::copy(&source, &destination).map_err(CoreError::Io)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&destination)
            .map_err(CoreError::Io)?
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&destination, permissions).map_err(CoreError::Io)?;
    }
    std::env::set_var(OPENXML_VALIDATOR_ENV, &destination);
    Ok(Some(destination))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PptxGenRuntimeManifest {
    kind: String,
    manifest_version: u8,
    runtime_version: String,
    node_version: String,
    node_file: String,
    module_root: String,
    modules: Vec<String>,
    files: Vec<PptxGenRuntimeFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PptxGenRuntimeFile {
    path: String,
    size: u64,
    sha256: String,
}

fn checked_runtime_relative_path(value: &str) -> Result<PathBuf, CoreError> {
    if value.is_empty() || value.contains('\\') || value.contains(':') {
        return Err(CoreError::InvalidInput(format!(
            "PptxGenJS runtime path is not portable: {value}"
        )));
    }
    let path = PathBuf::from(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CoreError::InvalidInput(format!(
            "PptxGenJS runtime path is not a normal relative path: {value}"
        )));
    }
    Ok(path)
}

fn file_sha256(path: &Path) -> Result<String, CoreError> {
    let bytes = std::fs::read(path).map_err(CoreError::Io)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn verify_pptxgenjs_runtime(
    root: &Path,
    manifest: &PptxGenRuntimeManifest,
) -> Result<(), CoreError> {
    if manifest.kind != "nexaPptxGenJsRuntime"
        || manifest.manifest_version != 1
        || manifest.runtime_version != "4.0.1-nexa.1"
        || !manifest.node_version.starts_with('v')
        || manifest.files.is_empty()
        || manifest.files.len() > 2_048
        || manifest.modules.is_empty()
    {
        return Err(CoreError::InvalidInput(
            "PptxGenJS runtime manifest identity or bounds are invalid".to_string(),
        ));
    }
    let mut total_size = 0_u64;
    let mut seen = std::collections::HashSet::new();
    for file in &manifest.files {
        let relative = checked_runtime_relative_path(&file.path)?;
        if !seen.insert(relative.clone())
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CoreError::InvalidInput(format!(
                "PptxGenJS runtime file identity is invalid: {}",
                file.path
            )));
        }
        total_size = total_size.checked_add(file.size).ok_or_else(|| {
            CoreError::InvalidInput("PptxGenJS runtime size overflow".to_string())
        })?;
        if total_size > 256 * 1024 * 1024 {
            return Err(CoreError::InvalidInput(
                "PptxGenJS runtime exceeds the 256 MiB package budget".to_string(),
            ));
        }
        let candidate = root.join(relative);
        let metadata = std::fs::symlink_metadata(&candidate).map_err(CoreError::Io)?;
        if !metadata.file_type().is_file()
            || metadata.len() != file.size
            || file_sha256(&candidate)? != file.sha256.to_ascii_lowercase()
        {
            return Err(CoreError::InvalidInput(format!(
                "PptxGenJS runtime file failed SHA-256 verification: {}",
                candidate.display()
            )));
        }
    }
    let node = checked_runtime_relative_path(&manifest.node_file)?;
    let modules = checked_runtime_relative_path(&manifest.module_root)?;
    if !seen.contains(&node) || !root.join(modules).is_dir() {
        return Err(CoreError::InvalidInput(
            "PptxGenJS runtime node or module root is not manifest-bound".to_string(),
        ));
    }
    Ok(())
}

pub fn configure_bundled_pptxgenjs_runtime(
    app_data_dir: &Path,
    resource_dir: &Path,
) -> Result<Option<(PathBuf, PathBuf)>, CoreError> {
    let source = resource_dir.join("pptxgenjs-runtime");
    let manifest_path = source.join("runtime-manifest.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest: PptxGenRuntimeManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).map_err(CoreError::Io)?).map_err(
            |error| CoreError::InvalidInput(format!("Invalid PptxGenJS runtime manifest: {error}")),
        )?;
    verify_pptxgenjs_runtime(&source, &manifest)?;

    let parent = app_data_dir.join("runtimes/pptxgenjs");
    std::fs::create_dir_all(&parent).map_err(CoreError::Io)?;
    let destination = parent.join(&manifest.runtime_version);
    if destination.exists() {
        let installed_manifest: PptxGenRuntimeManifest = serde_json::from_slice(
            &std::fs::read(destination.join("runtime-manifest.json")).map_err(CoreError::Io)?,
        )
        .map_err(|error| {
            CoreError::InvalidInput(format!("Invalid installed PptxGenJS manifest: {error}"))
        })?;
        verify_pptxgenjs_runtime(&destination, &installed_manifest)?;
    } else {
        let staging = parent.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&staging).map_err(CoreError::Io)?;
        for file in &manifest.files {
            let relative = checked_runtime_relative_path(&file.path)?;
            let target = staging.join(&relative);
            if let Some(target_parent) = target.parent() {
                std::fs::create_dir_all(target_parent).map_err(CoreError::Io)?;
            }
            std::fs::copy(source.join(&relative), &target).map_err(CoreError::Io)?;
        }
        std::fs::copy(&manifest_path, staging.join("runtime-manifest.json"))
            .map_err(CoreError::Io)?;
        verify_pptxgenjs_runtime(&staging, &manifest)?;
        std::fs::rename(&staging, &destination).map_err(CoreError::Io)?;
    }

    let node = destination.join(checked_runtime_relative_path(&manifest.node_file)?);
    let module_root = destination.join(checked_runtime_relative_path(&manifest.module_root)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&node)
            .map_err(CoreError::Io)?
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&node, permissions).map_err(CoreError::Io)?;
    }
    std::env::set_var(PPTXGENJS_NODE_ENV, &node);
    std::env::set_var(PPTXGENJS_MODULE_ROOT_ENV, &module_root);
    Ok(Some((node, module_root)))
}

pub fn configure_bundled_office_addin(
    app_data_dir: &Path,
    resource_dir: &Path,
) -> Result<Option<PathBuf>, CoreError> {
    let source_dir = resource_dir.join("office-addin");
    let manifest = source_dir.join("manifest.xml");
    if !manifest.is_file() {
        return Ok(None);
    }
    let destination_dir = app_data_dir.join("runtimes/office-addin/1.0.0");
    std::fs::create_dir_all(&destination_dir).map_err(CoreError::Io)?;
    for filename in [
        "manifest.xml",
        "taskpane.html",
        "taskpane.js",
        "support.html",
        "icon.png",
        "README.md",
    ] {
        let source = source_dir.join(filename);
        if !source.is_file() {
            return Err(CoreError::Internal(format!(
                "Bundled Office.js add-in resource is missing: {}",
                source.display()
            )));
        }
        std::fs::copy(&source, destination_dir.join(filename)).map_err(CoreError::Io)?;
    }
    let installed_manifest = destination_dir.join("manifest.xml");
    std::env::set_var(OFFICE_ADDIN_MANIFEST_ENV, &installed_manifest);
    Ok(Some(installed_manifest))
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

fn check_python_module(
    python: Option<&PythonCommand>,
    id: &str,
    module: &str,
    required: bool,
    expected_version: &str,
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
            install_hint: Some(format!("python -m pip install {id}=={expected_version}")),
        };
    };

    let code = format!("import {module} as m; print(getattr(m, '__version__', 'unknown'))");
    match python.run(&["-c", &code]) {
        Ok(output) if output.status.success() => {
            let actual_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if actual_version != expected_version {
                return OfficeDependencyStatus {
                    id: id.to_string(),
                    label: id.to_string(),
                    kind: "python-package".to_string(),
                    required,
                    status: "version-mismatch".to_string(),
                    version: Some(actual_version.clone()),
                    path: None,
                    detail: Some(format!(
                        "Expected pinned version {expected_version}, found {actual_version}"
                    )),
                    install_hint: Some(format!(
                        "python -m pip install --upgrade --force-reinstall {id}=={expected_version}"
                    )),
                };
            }
            OfficeDependencyStatus {
                id: id.to_string(),
                label: id.to_string(),
                kind: "python-package".to_string(),
                required,
                status: "ready".to_string(),
                version: Some(actual_version),
                path: None,
                detail: None,
                install_hint: None,
            }
        }
        Ok(output) => OfficeDependencyStatus {
            id: id.to_string(),
            label: id.to_string(),
            kind: "python-package".to_string(),
            required,
            status: "missing".to_string(),
            version: None,
            path: None,
            detail: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            install_hint: Some(format!("python -m pip install {id}=={expected_version}")),
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
            install_hint: Some(format!("python -m pip install {id}=={expected_version}")),
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
            "Core Office editing is ready. Some optional document helpers are unavailable."
                .to_string(),
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
        "1.2.0",
    ));
    dependencies.push(check_python_module(
        python.as_ref(),
        "openpyxl",
        "openpyxl",
        true,
        "3.1.5",
    ));
    dependencies.push(check_python_module(
        python.as_ref(),
        "python-pptx",
        "pptx",
        true,
        "1.0.2",
    ));
    dependencies.push(check_python_module(
        python.as_ref(),
        "pypdf",
        "pypdf",
        true,
        "6.10.0",
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
            .join("requirements.lock")
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

pub fn prepare_office_runtime(app_data_dir: &Path) -> Result<OfficePrepareResult, CoreError> {
    prepare_office_runtime_with_options(app_data_dir, "")
}

pub fn prepare_office_runtime_with_options(
    app_data_dir: &Path,
    _ghproxy_base: &str,
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
        .join("requirements.lock");
    match run_step(
        &managed,
        &[
            "-m",
            "pip",
            "install",
            "--require-hashes",
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

    #[tokio::test]
    async fn office_artifact_python_watchdog_times_out_and_kills_child() {
        let Some(python) = find_system_python_for_venv() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let args = vec!["-c".to_string(), "import time; time.sleep(5)".to_string()];
        let error = run_python_with_input(
            &python,
            &args,
            root.path(),
            "",
            Duration::from_millis(25),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("watchdog"));
    }

    #[test]
    fn bundled_openxml_validator_is_copied_to_app_owned_runtime() {
        let app_data = tempfile::tempdir().unwrap();
        let resources = tempfile::tempdir().unwrap();
        let filename = if cfg!(windows) {
            "Nexa.OpenXml.Validator.exe"
        } else {
            "Nexa.OpenXml.Validator"
        };
        let source_dir = resources.path().join("openxml-validator");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(filename), b"validator-binary").unwrap();

        let configured = configure_bundled_openxml_validator(app_data.path(), resources.path())
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&configured).unwrap(), b"validator-binary");
        assert_eq!(
            std::env::var_os(OPENXML_VALIDATOR_ENV),
            Some(configured.into_os_string())
        );
    }

    #[test]
    fn bundled_office_addin_is_copied_as_a_closed_resource_set() {
        let app_data = tempfile::tempdir().unwrap();
        let resources = tempfile::tempdir().unwrap();
        let source = resources.path().join("office-addin");
        std::fs::create_dir_all(&source).unwrap();
        for filename in [
            "manifest.xml",
            "taskpane.html",
            "taskpane.js",
            "support.html",
            "icon.png",
            "README.md",
        ] {
            std::fs::write(source.join(filename), filename.as_bytes()).unwrap();
        }
        let manifest = configure_bundled_office_addin(app_data.path(), resources.path())
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "manifest.xml");
        assert_eq!(
            std::env::var_os(OFFICE_ADDIN_MANIFEST_ENV),
            Some(manifest.into_os_string())
        );
    }

    #[test]
    fn bundled_pptxgenjs_runtime_is_hash_verified_and_copied() {
        let app_data = tempfile::tempdir().unwrap();
        let resources = tempfile::tempdir().unwrap();
        let source = resources.path().join("pptxgenjs-runtime");
        std::fs::create_dir_all(source.join("node_modules/pptxgenjs")).unwrap();
        let node_name = if cfg!(windows) { "node.exe" } else { "node" };
        let fixtures = [
            (node_name, b"node-binary".as_slice()),
            ("node_modules/pptxgenjs/package.json", b"{}".as_slice()),
        ];
        let mut files = Vec::new();
        for (relative, content) in fixtures {
            let path = source.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
            files.push(serde_json::json!({
                "path": relative.replace('\\', "/"),
                "size": content.len(),
                "sha256": file_sha256(&path).unwrap(),
            }));
        }
        let manifest = serde_json::json!({
            "kind": "nexaPptxGenJsRuntime",
            "manifestVersion": 1,
            "runtimeVersion": "4.0.1-nexa.1",
            "nodeVersion": "v24.0.0",
            "nodeFile": node_name,
            "moduleRoot": "node_modules",
            "modules": ["pptxgenjs"],
            "files": files,
        });
        std::fs::write(
            source.join("runtime-manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let (node, modules) =
            configure_bundled_pptxgenjs_runtime(app_data.path(), resources.path())
                .unwrap()
                .unwrap();
        assert_eq!(std::fs::read(&node).unwrap(), b"node-binary");
        assert!(modules.join("pptxgenjs/package.json").is_file());
        assert_eq!(
            std::env::var_os(PPTXGENJS_NODE_ENV),
            Some(node.into_os_string())
        );

        std::fs::write(modules.join("pptxgenjs/package.json"), b"tampered").unwrap();
        assert!(configure_bundled_pptxgenjs_runtime(app_data.path(), resources.path()).is_err());
    }
}
