//! User-owned Nexa extension home.
//!
//! The Tauri app-data directory remains the internal state store for SQLite,
//! caches, logs, indexes, and managed runtimes. User-authored declarations live
//! under one portable root (`~/.nexa` by default) so they can be inspected,
//! backed up, and shared without exposing internal state or credentials.

use crate::error::CoreError;
use crate::theme_resource_plugin::ThemeResourcePlugin;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

pub const NEXA_HOME_ENV: &str = "NEXA_HOME";
pub const NEXA_HOME_DIR: &str = ".nexa";
pub const USER_EXTENSION_LAYOUT_VERSION: u32 = 1;

const README_FILE: &str = "README.md";
const MCP_CONFIG_FILE: &str = "mcp.json";
const LEGACY_MCP_CONFIG_FILE: &str = "mcp-connectors.json";
const MAX_MIGRATION_FILES: usize = 10_000;
const MAX_MIGRATION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_THEME_FILE_BYTES: u64 = 1024 * 1024;

const README_CONTENT: &str = r#"# Nexa user extensions

This directory contains user-authored, portable Nexa declarations.

- `capabilities/`: capability packages and their manifests
- `skills/`: user skill folders containing `SKILL.md`
- `themes/`: declarative theme-resource JSON files
- `workflows/`: reusable workflow package declarations
- `connectors/mcp.json`: MCP connector declarations

Secrets do not belong here. Reference environment variables or use Nexa's
credential storage. Internal databases, caches, logs, indexes, downloaded
runtimes, and generated assets remain in the operating-system app-data folder.
"#;

#[derive(Debug, Clone)]
pub struct UserExtensionLayout {
    root: PathBuf,
    legacy_app_data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserExtensionLayoutView {
    pub version: u32,
    pub root: String,
    pub capabilities_dir: String,
    pub skills_dir: String,
    pub themes_dir: String,
    pub workflows_dir: String,
    pub connectors_dir: String,
    pub mcp_config_path: String,
    pub legacy_app_data_dir: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserExtensionMigrationReport {
    pub created_directories: u32,
    pub copied_files: u32,
    pub preserved_user_files: u32,
    pub skipped_links: u32,
    pub copied_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ThemeFileLoadReport {
    pub plugins: Vec<ThemeResourcePlugin>,
    pub warnings: Vec<String>,
}

impl UserExtensionLayout {
    pub fn discover(legacy_app_data_dir: impl AsRef<Path>) -> Result<Self, CoreError> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            CoreError::Internal("Could not resolve the user home directory for ~/.nexa".into())
        })?;
        let override_root = std::env::var_os(NEXA_HOME_ENV).map(PathBuf::from);
        Self::resolve(home_dir, legacy_app_data_dir, override_root)
    }

    pub fn resolve(
        home_dir: impl AsRef<Path>,
        legacy_app_data_dir: impl AsRef<Path>,
        override_root: Option<PathBuf>,
    ) -> Result<Self, CoreError> {
        let root = override_root.unwrap_or_else(|| home_dir.as_ref().join(NEXA_HOME_DIR));
        if !root.is_absolute() {
            return Err(CoreError::InvalidInput(format!(
                "{NEXA_HOME_ENV} must resolve to an absolute path"
            )));
        }
        Ok(Self {
            root,
            legacy_app_data_dir: legacy_app_data_dir.as_ref().to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn capabilities_dir(&self) -> PathBuf {
        self.root.join("capabilities")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.root.join("themes")
    }

    pub fn workflows_dir(&self) -> PathBuf {
        self.root.join("workflows")
    }

    pub fn connectors_dir(&self) -> PathBuf {
        self.root.join("connectors")
    }

    pub fn mcp_config_path(&self) -> PathBuf {
        self.connectors_dir().join(MCP_CONFIG_FILE)
    }

    pub fn view(&self) -> UserExtensionLayoutView {
        UserExtensionLayoutView {
            version: USER_EXTENSION_LAYOUT_VERSION,
            root: display_path(&self.root),
            capabilities_dir: display_path(&self.capabilities_dir()),
            skills_dir: display_path(&self.skills_dir()),
            themes_dir: display_path(&self.themes_dir()),
            workflows_dir: display_path(&self.workflows_dir()),
            connectors_dir: display_path(&self.connectors_dir()),
            mcp_config_path: display_path(&self.mcp_config_path()),
            legacy_app_data_dir: display_path(&self.legacy_app_data_dir),
        }
    }

    /// Create the user-owned layout and non-destructively seed it from the
    /// legacy app-data projections. Existing `.nexa` files always win; legacy
    /// files remain in place as a rollback source.
    pub fn bootstrap(&self) -> Result<UserExtensionMigrationReport, CoreError> {
        let mut report = UserExtensionMigrationReport::default();
        for directory in [
            self.root.clone(),
            self.capabilities_dir(),
            self.skills_dir(),
            self.themes_dir(),
            self.workflows_dir(),
            self.connectors_dir(),
        ] {
            ensure_directory(&directory, &mut report)?;
        }

        copy_file_if_missing(
            &self.legacy_app_data_dir.join(LEGACY_MCP_CONFIG_FILE),
            &self.mcp_config_path(),
            &mut report,
        )?;
        copy_directory_if_missing(
            &self.legacy_app_data_dir.join("skills").join("user"),
            &self.skills_dir(),
            &mut report,
        )?;
        write_bytes_if_missing(
            &self.root.join(README_FILE),
            README_CONTENT.as_bytes(),
            &mut report,
        )?;
        Ok(report)
    }

    pub fn write_theme_plugin(&self, plugin: ThemeResourcePlugin) -> Result<(), CoreError> {
        let plugin = plugin.normalize()?;
        fs::create_dir_all(self.themes_dir())?;
        let encoded = serde_json::to_vec_pretty(&plugin)?;
        atomic_write(&self.theme_path(&plugin.id), &encoded)
    }

    pub fn remove_theme_plugin(&self, theme_id: &str) -> Result<(), CoreError> {
        let path = self.theme_path(theme_id);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                CoreError::Internal(format!(
                    "Failed to remove user theme file {}: {error}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    pub fn load_theme_plugins(&self) -> Result<ThemeFileLoadReport, CoreError> {
        let mut report = ThemeFileLoadReport::default();
        if !self.themes_dir().is_dir() {
            return Ok(report);
        }
        let mut entries = fs::read_dir(self.themes_dir())?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report
                        .warnings
                        .push(format!("Could not inspect {}: {error}", path.display()));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if metadata.len() > MAX_THEME_FILE_BYTES {
                report
                    .warnings
                    .push(format!("Theme file exceeds 1 MiB: {}", path.display()));
                continue;
            }
            let result = fs::read(&path)
                .map_err(CoreError::from)
                .and_then(|bytes| {
                    serde_json::from_slice::<ThemeResourcePlugin>(&bytes).map_err(CoreError::from)
                })
                .and_then(ThemeResourcePlugin::normalize);
            match result {
                Ok(plugin)
                    if path.file_stem().and_then(|value| value.to_str())
                        == Some(plugin.id.as_str()) =>
                {
                    report.plugins.push(plugin)
                }
                Ok(plugin) => report.warnings.push(format!(
                    "Rejected user theme {}: file name must be {}.json",
                    path.display(),
                    plugin.id
                )),
                Err(error) => report
                    .warnings
                    .push(format!("Rejected user theme {}: {error}", path.display())),
            }
        }
        report.plugins.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(report)
    }

    fn theme_path(&self, theme_id: &str) -> PathBuf {
        let safe_id = theme_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        self.themes_dir().join(format!("{safe_id}.json"))
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn ensure_directory(
    path: &Path,
    report: &mut UserExtensionMigrationReport,
) -> Result<(), CoreError> {
    if path.exists() {
        if !path.is_dir() {
            return Err(CoreError::InvalidInput(format!(
                "Nexa extension path is not a directory: {}",
                path.display()
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(path)?;
    report.created_directories = report.created_directories.saturating_add(1);
    Ok(())
}

fn copy_directory_if_missing(
    source: &Path,
    target: &Path,
    report: &mut UserExtensionMigrationReport,
) -> Result<(), CoreError> {
    if !source.is_dir() {
        return Ok(());
    }
    let mut seen_files = 0usize;
    let mut seen_bytes = 0u64;
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|error| CoreError::Internal(error.to_string()))?;
        let relative = entry.path().strip_prefix(source).map_err(|error| {
            CoreError::Internal(format!("Failed to scope legacy extension path: {error}"))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let destination = target.join(relative);
        if entry.file_type().is_symlink() {
            report.skipped_links = report.skipped_links.saturating_add(1);
            report.warnings.push(format!(
                "Skipped legacy extension symlink: {}",
                entry.path().display()
            ));
            continue;
        }
        if entry.file_type().is_dir() {
            ensure_directory(&destination, report)?;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        seen_files += 1;
        if seen_files > MAX_MIGRATION_FILES {
            return Err(CoreError::InvalidInput(
                "Legacy extension migration exceeds 10,000 files".into(),
            ));
        }
        let bytes = entry
            .metadata()
            .map_err(|error| CoreError::Internal(error.to_string()))?
            .len();
        seen_bytes = seen_bytes.saturating_add(bytes);
        if seen_bytes > MAX_MIGRATION_BYTES {
            return Err(CoreError::InvalidInput(
                "Legacy extension migration exceeds 512 MiB".into(),
            ));
        }
        copy_file_if_missing(entry.path(), &destination, report)?;
    }
    Ok(())
}

fn copy_file_if_missing(
    source: &Path,
    target: &Path,
    report: &mut UserExtensionMigrationReport,
) -> Result<(), CoreError> {
    if !source.is_file() {
        return Ok(());
    }
    if target.exists() {
        report.preserved_user_files = report.preserved_user_files.saturating_add(1);
        return Ok(());
    }
    let mut bytes = Vec::new();
    File::open(source)?.read_to_end(&mut bytes)?;
    write_bytes_if_missing(target, &bytes, report)
}

fn write_bytes_if_missing(
    target: &Path,
    bytes: &[u8],
    report: &mut UserExtensionMigrationReport,
) -> Result<(), CoreError> {
    if target.exists() {
        report.preserved_user_files = report.preserved_user_files.saturating_add(1);
        return Ok(());
    }
    atomic_write(target, bytes)?;
    report.copied_files = report.copied_files.saturating_add(1);
    report.copied_bytes = report.copied_bytes.saturating_add(bytes.len() as u64);
    Ok(())
}

fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let parent = target.parent().ok_or_else(|| {
        CoreError::InvalidInput(format!(
            "Extension file has no parent: {}",
            target.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("extension");
    let temporary = parent.join(format!(".{file_name}.nexa-tmp-{}", Uuid::new_v4().simple()));
    let result = (|| -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if target.exists() {
            let backup = parent.join(format!(
                ".{file_name}.nexa-backup-{}",
                Uuid::new_v4().simple()
            ));
            fs::rename(target, &backup)?;
            if let Err(error) = fs::rename(&temporary, target) {
                let _ = fs::rename(&backup, target);
                return Err(error.into());
            }
            let _ = fs::remove_file(backup);
        } else {
            fs::rename(&temporary, target)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_resource_plugin::ThemeResourceDefinition;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn layout_uses_one_user_owned_dot_nexa_root() {
        let home = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let layout = UserExtensionLayout::resolve(home.path(), app_data.path(), None).unwrap();
        assert_eq!(layout.root(), home.path().join(".nexa"));
        assert_eq!(layout.skills_dir(), home.path().join(".nexa/skills"));
        assert_eq!(
            layout.mcp_config_path(),
            home.path().join(".nexa/connectors/mcp.json")
        );
    }

    #[test]
    fn relative_nexa_home_override_fails_closed() {
        let home = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let error = UserExtensionLayout::resolve(
            home.path(),
            app_data.path(),
            Some(PathBuf::from("relative/.nexa")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn bootstrap_copies_legacy_extensions_without_overwriting_user_files() {
        let home = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        fs::create_dir_all(app_data.path().join("skills/user/skill-1")).unwrap();
        fs::write(
            app_data.path().join("skills/user/skill-1/SKILL.md"),
            "legacy skill",
        )
        .unwrap();
        fs::write(
            app_data.path().join(LEGACY_MCP_CONFIG_FILE),
            "{\"version\":1,\"connectors\":{}}",
        )
        .unwrap();
        let layout = UserExtensionLayout::resolve(home.path(), app_data.path(), None).unwrap();
        fs::create_dir_all(layout.skills_dir().join("skill-1")).unwrap();
        fs::write(layout.skills_dir().join("skill-1/SKILL.md"), "user edit").unwrap();

        let report = layout.bootstrap().unwrap();
        assert_eq!(
            fs::read_to_string(layout.skills_dir().join("skill-1/SKILL.md")).unwrap(),
            "user edit"
        );
        assert!(layout.mcp_config_path().is_file());
        assert!(app_data.path().join(LEGACY_MCP_CONFIG_FILE).is_file());
        assert!(report.preserved_user_files >= 1);
    }

    #[test]
    fn theme_files_roundtrip_through_the_validated_contract() {
        let home = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let layout = UserExtensionLayout::resolve(home.path(), app_data.path(), None).unwrap();
        layout.bootstrap().unwrap();
        let plugin = ThemeResourcePlugin {
            manifest_version: 2,
            kind: "theme-resource".into(),
            id: "theme-user-test".into(),
            name: "User Test".into(),
            description: None,
            theme: ThemeResourceDefinition {
                base_theme: "dark".into(),
                mode: "dark".into(),
                colors: BTreeMap::new(),
                effects: Default::default(),
                typography: Default::default(),
                motion: Default::default(),
                brand: Default::default(),
                content: Default::default(),
                components: Default::default(),
                background: Default::default(),
            },
        };
        layout.write_theme_plugin(plugin.clone()).unwrap();
        assert_eq!(layout.load_theme_plugins().unwrap().plugins, vec![plugin]);
        fs::copy(
            layout.themes_dir().join("theme-user-test.json"),
            layout.themes_dir().join("wrong-name.json"),
        )
        .unwrap();
        let mismatched = layout.load_theme_plugins().unwrap();
        assert_eq!(mismatched.plugins.len(), 1);
        assert_eq!(mismatched.warnings.len(), 1);
        layout.remove_theme_plugin("theme-user-test").unwrap();
        assert!(layout.load_theme_plugins().unwrap().plugins.is_empty());
    }
}
