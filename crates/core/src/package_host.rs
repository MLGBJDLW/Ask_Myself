//! Package Host lifecycle contract.
//!
//! The Package Host decides which package-owned capabilities, connectors,
//! skills, workflows, and future native plugins are available to a runtime
//! session. Host surfaces should consume assembled registries, not rediscover
//! package state independently.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::capability_package::{CapabilityPackageManifest, CapabilityPackagePermissions};
use crate::ecosystem::EcosystemSurfaceKind;

pub const PACKAGE_HOST_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackageSurfaceKind {
    Capability,
    Connector,
    Skill,
    Workflow,
    NativePlugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageLifecycleState {
    Discovered,
    Validated,
    Enabled,
    Disabled,
    Unhealthy,
    Blocked,
}

impl PackageLifecycleState {
    pub fn is_runtime_visible(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageHealthState {
    Healthy,
    Warning,
    Unhealthy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagePermission {
    pub key: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageComponent {
    pub id: String,
    pub package_id: String,
    pub kind: PackageSurfaceKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostRecord {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    pub state: PackageLifecycleState,
    pub health: PackageHealthState,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<PackagePermission>,
    #[serde(default)]
    pub components: Vec<PackageComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostSnapshot {
    pub version: u16,
    #[serde(default)]
    pub records: Vec<PackageHostRecord>,
}

impl PackageHostSnapshot {
    pub fn new(records: Vec<PackageHostRecord>) -> Self {
        Self {
            version: PACKAGE_HOST_CONTRACT_VERSION,
            records,
        }
    }

    pub fn runtime_components(&self) -> Vec<&PackageComponent> {
        self.records
            .iter()
            .filter(|record| record.state.is_runtime_visible())
            .flat_map(|record| record.components.iter())
            .filter(|component| component.enabled)
            .collect()
    }

    pub fn validate(&self) -> Result<(), PackageHostContractError> {
        if self.version != PACKAGE_HOST_CONTRACT_VERSION {
            return Err(PackageHostContractError::UnsupportedVersion {
                version: self.version,
            });
        }

        let mut package_ids = HashSet::new();
        let mut component_ids = HashSet::new();
        for record in &self.records {
            if !package_ids.insert(record.id.clone()) {
                return Err(PackageHostContractError::DuplicatePackage {
                    package_id: record.id.clone(),
                });
            }
            for component in &record.components {
                if component.package_id != record.id {
                    return Err(PackageHostContractError::ComponentPackageMismatch {
                        package_id: record.id.clone(),
                        component_id: component.id.clone(),
                    });
                }
                if !component_ids.insert((component.kind, component.id.clone())) {
                    return Err(PackageHostContractError::DuplicateComponent {
                        component_id: component.id.clone(),
                    });
                }
            }
        }

        for record in &self.records {
            for dependency in &record.dependencies {
                if !package_ids.contains(dependency) {
                    return Err(PackageHostContractError::MissingDependency {
                        package_id: record.id.clone(),
                        dependency_id: dependency.clone(),
                    });
                }
            }
        }

        Ok(())
    }
}

pub fn package_host_snapshot_from_manifests(
    manifests: &[CapabilityPackageManifest],
) -> PackageHostSnapshot {
    let mut records = manifests
        .iter()
        .map(package_host_record_from_manifest)
        .collect::<Vec<_>>();
    records.sort_by(|a, b| a.id.cmp(&b.id));
    PackageHostSnapshot::new(records)
}

fn package_host_record_from_manifest(manifest: &CapabilityPackageManifest) -> PackageHostRecord {
    PackageHostRecord {
        id: manifest.id.clone(),
        version: Some(manifest.version.to_string()),
        state: PackageLifecycleState::Enabled,
        health: PackageHealthState::Healthy,
        dependencies: Vec::new(),
        permissions: package_permissions_from_manifest(&manifest.permissions),
        components: package_components_from_manifest(manifest),
    }
}

fn package_permissions_from_manifest(
    permissions: &CapabilityPackagePermissions,
) -> Vec<PackagePermission> {
    [
        ("read", permissions.read),
        ("write", permissions.write),
        ("execute", permissions.execute),
        ("network", permissions.network),
        ("native_code", permissions.native_code),
    ]
    .into_iter()
    .filter(|(_, enabled)| *enabled)
    .map(|(key, _)| PackagePermission {
        key: key.to_string(),
        description: format!("Package requests {key} permission"),
    })
    .collect()
}

fn package_components_from_manifest(manifest: &CapabilityPackageManifest) -> Vec<PackageComponent> {
    let mut components = Vec::new();
    let package_surface = package_surface_kind_from_manifest(manifest.surface);
    if package_surface != PackageSurfaceKind::Capability {
        components.push(PackageComponent {
            id: manifest.id.clone(),
            package_id: manifest.id.clone(),
            kind: package_surface,
            enabled: true,
        });
    }
    components.extend(
        manifest
            .tools
            .iter()
            .map(|id| package_component(id, &manifest.id, PackageSurfaceKind::Capability)),
    );
    components.extend(
        manifest
            .skills
            .iter()
            .map(|id| package_component(id, &manifest.id, PackageSurfaceKind::Skill)),
    );
    components.extend(
        manifest
            .workflows
            .iter()
            .map(|id| package_component(id, &manifest.id, PackageSurfaceKind::Workflow)),
    );
    if components.is_empty() {
        components.push(PackageComponent {
            id: manifest.id.clone(),
            package_id: manifest.id.clone(),
            kind: package_surface,
            enabled: true,
        });
    }
    components
}

fn package_component(id: &str, package_id: &str, kind: PackageSurfaceKind) -> PackageComponent {
    PackageComponent {
        id: id.to_string(),
        package_id: package_id.to_string(),
        kind,
        enabled: true,
    }
}

fn package_surface_kind_from_manifest(surface: EcosystemSurfaceKind) -> PackageSurfaceKind {
    match surface {
        EcosystemSurfaceKind::Connector => PackageSurfaceKind::Connector,
        EcosystemSurfaceKind::SkillPackage => PackageSurfaceKind::Skill,
        EcosystemSurfaceKind::WorkflowPackage => PackageSurfaceKind::Workflow,
        EcosystemSurfaceKind::NativePlugin => PackageSurfaceKind::NativePlugin,
        EcosystemSurfaceKind::CorePlatform
        | EcosystemSurfaceKind::CapabilityPackage
        | EcosystemSurfaceKind::Adapter
        | EcosystemSurfaceKind::HostSurface => PackageSurfaceKind::Capability,
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PackageHostContractError {
    #[error("unsupported package host contract version {version}")]
    UnsupportedVersion { version: u16 },
    #[error("duplicate package id {package_id}")]
    DuplicatePackage { package_id: String },
    #[error("duplicate component id {component_id}")]
    DuplicateComponent { component_id: String },
    #[error("component {component_id} does not belong to package {package_id}")]
    ComponentPackageMismatch {
        package_id: String,
        component_id: String,
    },
    #[error("package {package_id} depends on missing package {dependency_id}")]
    MissingDependency {
        package_id: String,
        dependency_id: String,
    },
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(id: &str, package_id: &str, kind: PackageSurfaceKind) -> PackageComponent {
        PackageComponent {
            id: id.to_string(),
            package_id: package_id.to_string(),
            kind,
            enabled: true,
        }
    }

    #[test]
    fn disabled_packages_disappear_from_runtime_components() {
        let snapshot = PackageHostSnapshot::new(vec![
            PackageHostRecord {
                id: "pkg-enabled".to_string(),
                version: Some("1.0.0".to_string()),
                state: PackageLifecycleState::Enabled,
                health: PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: vec![component(
                    "skill-a",
                    "pkg-enabled",
                    PackageSurfaceKind::Skill,
                )],
            },
            PackageHostRecord {
                id: "pkg-disabled".to_string(),
                version: Some("1.0.0".to_string()),
                state: PackageLifecycleState::Disabled,
                health: PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: vec![component(
                    "workflow-b",
                    "pkg-disabled",
                    PackageSurfaceKind::Workflow,
                )],
            },
        ]);

        snapshot.validate().unwrap();
        let components = snapshot.runtime_components();

        assert_eq!(components.len(), 1);
        assert_eq!(components[0].id, "skill-a");
    }

    #[test]
    fn validation_rejects_missing_dependencies() {
        let snapshot = PackageHostSnapshot::new(vec![PackageHostRecord {
            id: "pkg-a".to_string(),
            version: None,
            state: PackageLifecycleState::Enabled,
            health: PackageHealthState::Healthy,
            dependencies: vec!["missing".to_string()],
            permissions: Vec::new(),
            components: Vec::new(),
        }]);

        assert_eq!(
            snapshot.validate().unwrap_err(),
            PackageHostContractError::MissingDependency {
                package_id: "pkg-a".to_string(),
                dependency_id: "missing".to_string()
            }
        );
    }

    #[test]
    fn snapshot_from_ecosystem_manifests_is_valid_and_componentized() {
        let manifests = crate::ecosystem::builtin_ecosystem_manifests();
        let snapshot = package_host_snapshot_from_manifests(&manifests);

        snapshot.validate().unwrap();
        assert!(snapshot.records.iter().any(|record| {
            record.id == "builtin-skills"
                && record.components.iter().any(|component| {
                    component.kind == PackageSurfaceKind::Skill && component.id == "builtin-skills"
                })
        }));
        assert!(snapshot.records.iter().any(|record| {
            record.id == "mcp-connectors"
                && record.components.iter().any(|component| {
                    component.kind == PackageSurfaceKind::Connector
                        && component.id == "mcp-connectors"
                })
        }));
        assert!(snapshot.runtime_components().iter().any(|component| {
            component.package_id == "office-documents" && component.id == "compile_document"
        }));
    }
}
