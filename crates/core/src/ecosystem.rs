//! Stable vocabulary for Nexa ecosystem surfaces.
//!
//! This module keeps extension language explicit. Most external integration
//! work should land as connectors, skills, workflows, or adapters before Nexa
//! needs a native plugin runtime.

use crate::capability_package::CapabilityPackageManifest;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EcosystemSurfaceKind {
    CorePlatform,
    CapabilityPackage,
    Connector,
    SkillPackage,
    WorkflowPackage,
    Adapter,
    HostSurface,
    NativePlugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcosystemSurfacePolicy {
    pub kind: EcosystemSurfaceKind,
    pub label: &'static str,
    pub purpose: &'static str,
    pub external_by_default: bool,
    pub native_code_allowed: bool,
}

pub const ECOSYSTEM_SURFACE_POLICIES: &[EcosystemSurfacePolicy] = &[
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::CorePlatform,
        label: "Core Platform",
        purpose: "Required host runtime, trust model, persistence, approvals, and local knowledge foundation.",
        external_by_default: false,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::CapabilityPackage,
        label: "Capability Package",
        purpose: "Coherent Nexa ability that owns tools, settings, workflows, checks, and tests.",
        external_by_default: false,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::Connector,
        label: "Connector",
        purpose: "External service, process, or data source access, with MCP as the first interface.",
        external_by_default: true,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::SkillPackage,
        label: "Skill Package",
        purpose: "Portable instructions, references, examples, and resources used by existing tools.",
        external_by_default: true,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::WorkflowPackage,
        label: "Workflow Package",
        purpose: "User-facing task template that composes tools, skills, connectors, and approvals.",
        external_by_default: true,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::Adapter,
        label: "Adapter",
        purpose: "Replaceable backend implementation behind a stable host interface.",
        external_by_default: false,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::HostSurface,
        label: "Host Surface",
        purpose: "Product shell such as Desktop, CLI, IDE extension, or browser extension.",
        external_by_default: false,
        native_code_allowed: false,
    },
    EcosystemSurfacePolicy {
        kind: EcosystemSurfaceKind::NativePlugin,
        label: "Native Plugin",
        purpose: "Last-resort isolated code, hook, or UI extension when safer surfaces are insufficient.",
        external_by_default: false,
        native_code_allowed: true,
    },
];

pub fn ecosystem_surface_policy(
    kind: EcosystemSurfaceKind,
) -> Option<&'static EcosystemSurfacePolicy> {
    ECOSYSTEM_SURFACE_POLICIES
        .iter()
        .find(|policy| policy.kind == kind)
}

pub fn builtin_ecosystem_manifests() -> Vec<CapabilityPackageManifest> {
    let mut manifests = crate::plugins::builtin_capability_manifests();
    manifests.push(crate::skills::package::builtin_skill_package_manifest());
    manifests.push(crate::skills::package::builtin_workflow_package_manifest());
    manifests.sort_by(|a, b| a.id.cmp(&b.id));
    manifests
}

pub fn ecosystem_manifests(
    project_root: impl AsRef<Path>,
) -> Result<Vec<CapabilityPackageManifest>, CoreError> {
    let mut manifests = builtin_ecosystem_manifests();
    manifests.extend(crate::capability_package::discover_capability_manifests(
        project_root,
    )?);
    manifests.sort_by(|a, b| a.id.cmp(&b.id));

    if let Some(duplicate) = manifests.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(CoreError::InvalidInput(format!(
            "Duplicate capability package id: {}",
            duplicate[0].id
        )));
    }

    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_skill_and_workflow_are_external_without_native_code() {
        for kind in [
            EcosystemSurfaceKind::Connector,
            EcosystemSurfaceKind::SkillPackage,
            EcosystemSurfaceKind::WorkflowPackage,
        ] {
            let policy = ecosystem_surface_policy(kind).expect("missing ecosystem policy");
            assert!(policy.external_by_default);
            assert!(!policy.native_code_allowed);
        }
    }

    #[test]
    fn native_plugin_is_the_only_surface_that_allows_native_code() {
        for policy in ECOSYSTEM_SURFACE_POLICIES {
            assert_eq!(
                policy.native_code_allowed,
                policy.kind == EcosystemSurfaceKind::NativePlugin
            );
        }
    }

    #[test]
    fn builtin_ecosystem_manifests_cover_tools_skills_and_workflows() {
        let manifests = builtin_ecosystem_manifests();

        assert!(manifests.iter().any(|manifest| {
            manifest.id == "office-documents"
                && manifest.surface == EcosystemSurfaceKind::CapabilityPackage
        }));
        assert!(manifests.iter().any(|manifest| {
            manifest.id == "mcp-connectors" && manifest.surface == EcosystemSurfaceKind::Connector
        }));
        assert!(manifests.iter().any(|manifest| {
            manifest.id == "builtin-skills"
                && manifest.surface == EcosystemSurfaceKind::SkillPackage
        }));
        assert!(manifests.iter().any(|manifest| {
            manifest.id == "builtin-workflows"
                && manifest.surface == EcosystemSurfaceKind::WorkflowPackage
        }));

        for manifest in &manifests {
            crate::capability_package::validate_capability_manifest(manifest)
                .expect("builtin ecosystem manifests should validate");
        }
    }

    #[test]
    fn ecosystem_manifests_include_project_local_packages() {
        let dir = tempfile::tempdir().unwrap();
        let packages = crate::capability_package::capability_packages_dir(dir.path());
        std::fs::create_dir_all(packages.join("local-connector")).unwrap();
        std::fs::write(
            packages
                .join("local-connector")
                .join(crate::capability_package::NEXA_CAPABILITY_MANIFEST_FILE),
            r#"
id: local-connector
name: Local Connector
surface: connector
description: Project-local connector package.
version: 1
permissions:
  read: true
  network: true
"#,
        )
        .unwrap();

        let manifests = ecosystem_manifests(dir.path()).unwrap();
        let ids = manifests
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"builtin-skills"));
        assert!(ids.contains(&"builtin-workflows"));
        assert!(ids.contains(&"local-connector"));
        assert!(ids.contains(&"mcp-connectors"));
    }

    #[test]
    fn ecosystem_manifests_reject_duplicate_package_ids() {
        let dir = tempfile::tempdir().unwrap();
        let packages = crate::capability_package::capability_packages_dir(dir.path());
        std::fs::create_dir_all(packages.join("mcp-connectors")).unwrap();
        std::fs::write(
            packages
                .join("mcp-connectors")
                .join(crate::capability_package::NEXA_CAPABILITY_MANIFEST_FILE),
            r#"
id: mcp-connectors
name: Shadow Connector
surface: connector
description: Attempts to shadow the built-in connector package.
version: 1
"#,
        )
        .unwrap();

        let error = ecosystem_manifests(dir.path()).unwrap_err();

        assert!(error
            .to_string()
            .contains("Duplicate capability package id: mcp-connectors"));
    }
}
