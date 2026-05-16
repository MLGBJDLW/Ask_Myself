//! Native Nexa capability package layout.
//!
//! A capability package groups implementation and metadata for one coherent
//! ability. The same package path owns skills, commands, tools, hooks,
//! workflows, and tests so runtime discovery does not need per-feature path
//! conventions.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackageEntry {
    pub package_id: String,
    pub component_id: String,
    pub kind: CapabilityComponentKind,
    pub path: String,
    pub built_in: bool,
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
}
