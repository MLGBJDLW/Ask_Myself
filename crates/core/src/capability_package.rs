//! Native Nexa capability package layout.
//!
//! A capability package groups implementation and metadata for one coherent
//! ability. The same package path owns skills, commands, tools, hooks,
//! workflows, and tests so runtime discovery does not need per-feature path
//! conventions.

use crate::ecosystem::{ecosystem_surface_policy, EcosystemSurfaceKind};
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const NEXA_CAPABILITY_PACKAGES_DIR: &str = ".nexa/capabilities";
pub const NEXA_CAPABILITY_MANIFEST_FILE: &str = "capability.yaml";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityComponentKind {
    Skill,
    Command,
    Tool,
    Hook,
    Workflow,
    Test,
}

impl CapabilityComponentKind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Skill => "skills",
            Self::Command => "commands",
            Self::Tool => "tools",
            Self::Hook => "hooks",
            Self::Workflow => "workflows",
            Self::Test => "tests",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityComponentDirectory {
    pub kind: CapabilityComponentKind,
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackageLayout {
    pub root_dir: String,
    pub manifest_file: String,
    pub component_dirs: Vec<CapabilityComponentDirectory>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackagePermissions {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub execute: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub native_code: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackageManifest {
    pub id: String,
    pub name: String,
    pub surface: EcosystemSurfaceKind,
    pub description: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_surfaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_checks: Vec<String>,
    #[serde(default)]
    pub permissions: CapabilityPackagePermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackageEntry {
    pub package_id: String,
    pub component_id: String,
    pub kind: CapabilityComponentKind,
    pub path: String,
    pub built_in: bool,
}

pub fn validate_capability_manifest(
    manifest: &CapabilityPackageManifest,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if manifest.id.trim().is_empty() {
        errors.push("id is required".to_string());
    }
    if manifest.name.trim().is_empty() {
        errors.push("name is required".to_string());
    }
    if manifest.description.trim().is_empty() {
        errors.push("description is required".to_string());
    }
    if manifest.version == 0 {
        errors.push("version must be at least 1".to_string());
    }

    let policy =
        ecosystem_surface_policy(manifest.surface).expect("all ecosystem surfaces have policies");
    if manifest.permissions.native_code && !policy.native_code_allowed {
        errors.push(format!(
            "{} cannot declare nativeCode permission",
            policy.label
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn capability_packages_dir(project_root: impl AsRef<Path>) -> PathBuf {
    project_root.as_ref().join(NEXA_CAPABILITY_PACKAGES_DIR)
}

pub fn read_capability_manifest(
    manifest_path: impl AsRef<Path>,
) -> Result<CapabilityPackageManifest, CoreError> {
    let manifest_path = manifest_path.as_ref();
    let content = std::fs::read_to_string(manifest_path)?;
    let manifest =
        serde_yaml::from_str::<CapabilityPackageManifest>(&content).map_err(|error| {
            CoreError::Parse(format!(
                "Invalid capability manifest {}: {error}",
                manifest_path.display()
            ))
        })?;

    validate_capability_manifest(&manifest).map_err(|errors| {
        CoreError::InvalidInput(format!(
            "Invalid capability manifest {}: {}",
            manifest_path.display(),
            errors.join("; ")
        ))
    })?;

    Ok(manifest)
}

pub fn discover_capability_manifests(
    project_root: impl AsRef<Path>,
) -> Result<Vec<CapabilityPackageManifest>, CoreError> {
    let packages_dir = capability_packages_dir(project_root);
    if !packages_dir.exists() {
        return Ok(Vec::new());
    }

    let mut manifest_paths = Vec::new();
    for entry in std::fs::read_dir(&packages_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let manifest_path = entry.path().join(NEXA_CAPABILITY_MANIFEST_FILE);
            if manifest_path.is_file() {
                manifest_paths.push(manifest_path);
            }
        }
    }
    manifest_paths.sort();

    let mut manifests = Vec::with_capacity(manifest_paths.len());
    for manifest_path in manifest_paths {
        manifests.push(read_capability_manifest(manifest_path)?);
    }
    manifests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(manifests)
}

pub fn nexa_capability_package_layout() -> CapabilityPackageLayout {
    CapabilityPackageLayout {
        root_dir: NEXA_CAPABILITY_PACKAGES_DIR.to_string(),
        manifest_file: NEXA_CAPABILITY_MANIFEST_FILE.to_string(),
        component_dirs: [
            CapabilityComponentKind::Skill,
            CapabilityComponentKind::Command,
            CapabilityComponentKind::Tool,
            CapabilityComponentKind::Hook,
            CapabilityComponentKind::Workflow,
            CapabilityComponentKind::Test,
        ]
        .into_iter()
        .map(|kind| CapabilityComponentDirectory {
            kind,
            dir: kind.directory().to_string(),
        })
        .collect(),
    }
}

pub fn package_root(package_id: &str) -> String {
    format!(
        "{}/{}",
        NEXA_CAPABILITY_PACKAGES_DIR,
        normalize_relative_component(package_id)
    )
}

pub fn package_manifest_path(package_id: &str) -> String {
    format!(
        "{}/{}",
        package_root(package_id),
        NEXA_CAPABILITY_MANIFEST_FILE
    )
}

pub fn package_component_dir(package_id: &str, kind: CapabilityComponentKind) -> String {
    format!("{}/{}", package_root(package_id), kind.directory())
}

pub fn package_component_path(
    package_id: &str,
    kind: CapabilityComponentKind,
    component_relative_path: &str,
) -> String {
    format!(
        "{}/{}",
        package_component_dir(package_id, kind),
        normalize_relative_component(component_relative_path)
    )
}

pub fn package_entry(
    package_id: &str,
    kind: CapabilityComponentKind,
    component_id: &str,
    component_relative_path: &str,
    built_in: bool,
) -> CapabilityPackageEntry {
    CapabilityPackageEntry {
        package_id: package_id.to_string(),
        component_id: component_id.to_string(),
        kind,
        path: package_component_path(package_id, kind, component_relative_path),
        built_in,
    }
}

fn normalize_relative_component(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::EcosystemSurfaceKind;

    #[test]
    fn layout_unifies_all_capability_component_dirs() {
        let layout = nexa_capability_package_layout();
        let dirs = layout
            .component_dirs
            .iter()
            .map(|entry| entry.dir.as_str())
            .collect::<Vec<_>>();

        assert_eq!(layout.root_dir, ".nexa/capabilities");
        assert_eq!(layout.manifest_file, "capability.yaml");
        assert_eq!(
            dirs,
            ["skills", "commands", "tools", "hooks", "workflows", "tests"]
        );
    }

    #[test]
    fn component_paths_share_one_package_root() {
        assert_eq!(
            package_manifest_path("office-documents"),
            ".nexa/capabilities/office-documents/capability.yaml"
        );
        assert_eq!(
            package_component_path(
                "office-documents",
                CapabilityComponentKind::Skill,
                "pptx/SKILL.md"
            ),
            ".nexa/capabilities/office-documents/skills/pptx/SKILL.md"
        );
        assert_eq!(
            package_component_path(
                "office-documents",
                CapabilityComponentKind::Workflow,
                "deck/workflow.yaml"
            ),
            ".nexa/capabilities/office-documents/workflows/deck/workflow.yaml"
        );
    }

    #[test]
    fn package_entry_is_component_agnostic() {
        let entry = package_entry(
            "automation",
            CapabilityComponentKind::Hook,
            "before-run",
            "before-run/hook.yaml",
            false,
        );

        assert_eq!(entry.package_id, "automation");
        assert_eq!(entry.component_id, "before-run");
        assert_eq!(entry.kind, CapabilityComponentKind::Hook);
        assert_eq!(
            entry.path,
            ".nexa/capabilities/automation/hooks/before-run/hook.yaml"
        );
        assert!(!entry.built_in);
    }

    #[test]
    fn parses_capability_manifest_yaml_with_surface() {
        let manifest: CapabilityPackageManifest = serde_yaml::from_str(
            r#"
id: office-documents
name: Office Documents
surface: capability_package
description: Works with PPT, DOCX, XLSX, PDF, and HTML document flows.
version: 1
tools:
  - prepare_document_tools
  - get_document_info
settingsSurfaces:
  - office-runtime
workflows:
  - generate-presentation
runtimeChecks:
  - office-runtime
"#,
        )
        .expect("valid capability manifest yaml");

        assert_eq!(manifest.id, "office-documents");
        assert_eq!(manifest.surface, EcosystemSurfaceKind::CapabilityPackage);
        assert_eq!(
            manifest.tools,
            ["prepare_document_tools", "get_document_info"]
        );
        validate_capability_manifest(&manifest).expect("manifest should validate");
    }

    #[test]
    fn manifest_validation_rejects_native_code_except_native_plugin() {
        let mut manifest = CapabilityPackageManifest {
            id: "unsafe-connector".to_string(),
            name: "Unsafe Connector".to_string(),
            surface: EcosystemSurfaceKind::Connector,
            description: "test".to_string(),
            version: 1,
            tools: Vec::new(),
            skills: Vec::new(),
            workflows: Vec::new(),
            settings_surfaces: Vec::new(),
            runtime_checks: Vec::new(),
            permissions: CapabilityPackagePermissions {
                native_code: true,
                ..CapabilityPackagePermissions::default()
            },
        };

        assert!(validate_capability_manifest(&manifest).is_err());

        manifest.surface = EcosystemSurfaceKind::NativePlugin;
        validate_capability_manifest(&manifest).expect("native plugins may declare native code");
    }

    #[test]
    fn discovers_project_capability_manifests_from_standard_directory() {
        let dir = tempfile::tempdir().unwrap();
        let packages = capability_packages_dir(dir.path());
        std::fs::create_dir_all(packages.join("z-connector")).unwrap();
        std::fs::create_dir_all(packages.join("a-skill")).unwrap();
        std::fs::write(
            packages
                .join("z-connector")
                .join(NEXA_CAPABILITY_MANIFEST_FILE),
            r#"
id: z-connector
name: Z Connector
surface: connector
description: External connector package.
version: 1
permissions:
  read: true
  network: true
"#,
        )
        .unwrap();
        std::fs::write(
            packages.join("a-skill").join(NEXA_CAPABILITY_MANIFEST_FILE),
            r#"
id: a-skill
name: A Skill
surface: skill_package
description: Portable skill package.
version: 1
skills:
  - a-skill
"#,
        )
        .unwrap();

        let manifests = discover_capability_manifests(dir.path()).unwrap();

        assert_eq!(
            manifests
                .iter()
                .map(|manifest| manifest.id.as_str())
                .collect::<Vec<_>>(),
            ["a-skill", "z-connector"]
        );
        assert_eq!(manifests[0].surface, EcosystemSurfaceKind::SkillPackage);
        assert_eq!(manifests[1].surface, EcosystemSurfaceKind::Connector);
    }

    #[test]
    fn discover_returns_empty_when_capability_directory_is_absent() {
        let dir = tempfile::tempdir().unwrap();

        let manifests = discover_capability_manifests(dir.path()).unwrap();

        assert!(manifests.is_empty());
    }

    #[test]
    fn read_capability_manifest_reports_validation_errors() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join(NEXA_CAPABILITY_MANIFEST_FILE);
        std::fs::write(
            &manifest_path,
            r#"
id: bad
name: Bad
surface: connector
description: Invalid native connector.
version: 1
permissions:
  nativeCode: true
"#,
        )
        .unwrap();

        let error = read_capability_manifest(&manifest_path).unwrap_err();

        assert!(error.to_string().contains("cannot declare nativeCode"));
    }
}
