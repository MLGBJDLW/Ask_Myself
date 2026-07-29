//! Package Host lifecycle contract.
//!
//! The Package Host decides which package-owned capabilities, connectors,
//! skills, workflows, and future native plugins are available to a runtime
//! session. Host surfaces should consume assembled registries, not rediscover
//! package state independently.

use std::collections::{HashMap, HashSet};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::capability_package::{CapabilityPackageManifest, CapabilityPackagePermissions};
use crate::db::Database;
use crate::ecosystem::EcosystemSurfaceKind;
use crate::error::CoreError;
use crate::tools::{default_tool_registry, ToolRegistry};

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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Validated => "validated",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Unhealthy => "unhealthy",
            Self::Blocked => "blocked",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim() {
            "discovered" => Some(Self::Discovered),
            "validated" => Some(Self::Validated),
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            "unhealthy" => Some(Self::Unhealthy),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageHealthState {
    Healthy,
    Warning,
    Unhealthy,
}

impl PackageHealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Unhealthy => "unhealthy",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim() {
            "healthy" => Some(Self::Healthy),
            "warning" => Some(Self::Warning),
            "unhealthy" => Some(Self::Unhealthy),
            _ => None,
        }
    }
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

impl PackageHostRecord {
    pub fn is_runtime_visible(&self) -> bool {
        self.state.is_runtime_visible() && self.health != PackageHealthState::Unhealthy
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostSnapshot {
    pub version: u16,
    #[serde(default)]
    pub records: Vec<PackageHostRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostStateRecord {
    pub package_id: String,
    pub lifecycle_state: PackageLifecycleState,
    pub health_state: PackageHealthState,
    pub updated_at: String,
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
            .filter(|record| record.is_runtime_visible())
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

pub trait PackageHost: Send + Sync {
    fn snapshot(&self) -> Result<PackageHostSnapshot, PackageHostContractError>;

    fn runtime_package_context(
        &self,
    ) -> Result<crate::runtime::RuntimePackageContext, PackageHostContractError> {
        let snapshot = self.snapshot()?;
        Ok(crate::runtime::RuntimePackageContext::from_package_host_snapshot(&snapshot))
    }
}

/// Fully assembled runtime capabilities for one Package Host snapshot.
///
/// The registry and its ownership map are produced together so callers cannot
/// instantiate tools and apply package policy as unrelated steps.
pub struct RuntimeCapabilitySet {
    pub tools: ToolRegistry,
    pub package_context: crate::runtime::RuntimePackageContext,
    pub tool_owners: HashMap<String, String>,
    pub excluded_tools: Vec<String>,
}

/// The single package-aware entry point for constructing runtime tools.
pub struct PackageRuntimeAssembler {
    snapshot: PackageHostSnapshot,
}

impl PackageRuntimeAssembler {
    pub fn from_host(host: &impl PackageHost) -> Result<Self, PackageHostContractError> {
        Self::new(host.snapshot()?)
    }

    pub fn database_builtin(db: &Database) -> Result<Self, PackageHostContractError> {
        Self::from_host(&DatabasePackageHost::builtin(db))
    }

    pub fn new(snapshot: PackageHostSnapshot) -> Result<Self, PackageHostContractError> {
        snapshot.validate()?;
        Ok(Self { snapshot })
    }

    pub fn snapshot(&self) -> &PackageHostSnapshot {
        &self.snapshot
    }

    pub fn builtin_tool_registry(&self) -> ToolRegistry {
        default_tool_registry()
    }

    pub fn assemble_builtin_capabilities(
        &self,
    ) -> Result<RuntimeCapabilitySet, PackageHostContractError> {
        self.assemble_tool_registry(self.builtin_tool_registry())
    }

    pub fn assemble_tool_registry(
        &self,
        registry: ToolRegistry,
    ) -> Result<RuntimeCapabilitySet, PackageHostContractError> {
        let mut allowed_names = Vec::new();
        let mut excluded_tools = Vec::new();
        let mut tool_owners = HashMap::new();

        for tool_name in registry.tool_names() {
            let owner = self.tool_owner(&tool_name).ok_or_else(|| {
                PackageHostContractError::UnownedRuntimeTool {
                    tool_name: tool_name.clone(),
                }
            })?;
            tool_owners.insert(tool_name.clone(), owner.id.clone());
            if owner.is_runtime_visible() && self.owner_exposes_tool(owner, &tool_name) {
                allowed_names.push(tool_name);
            } else {
                excluded_tools.push(tool_name);
            }
        }

        Ok(RuntimeCapabilitySet {
            tools: registry.filtered(&allowed_names),
            package_context: crate::runtime::RuntimePackageContext::from_package_host_snapshot(
                &self.snapshot,
            ),
            tool_owners,
            excluded_tools,
        })
    }

    pub fn visible_tool_names(
        &self,
        names: Vec<String>,
    ) -> Result<Vec<String>, PackageHostContractError> {
        let mut visible = Vec::new();
        for name in names {
            let owner = self.tool_owner(&name).ok_or_else(|| {
                PackageHostContractError::UnownedRuntimeTool {
                    tool_name: name.clone(),
                }
            })?;
            if owner.is_runtime_visible() && self.owner_exposes_tool(owner, &name) {
                visible.push(name);
            }
        }
        Ok(visible)
    }

    fn tool_owner(&self, tool_name: &str) -> Option<&PackageHostRecord> {
        self.snapshot.records.iter().find(|record| {
            record.components.iter().any(|component| {
                component.kind == PackageSurfaceKind::Capability && component.id == tool_name
            }) || (record.id == "mcp-connectors"
                && (tool_name == "mcp_tool" || tool_name.starts_with("mcp__")))
        })
    }

    fn owner_exposes_tool(&self, owner: &PackageHostRecord, tool_name: &str) -> bool {
        owner.components.iter().any(|component| {
            component.kind == PackageSurfaceKind::Capability
                && component.id == tool_name
                && component.enabled
        }) || (owner.id == "mcp-connectors"
            && (tool_name == "mcp_tool" || tool_name.starts_with("mcp__")))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinPackageHost;

impl PackageHost for BuiltinPackageHost {
    fn snapshot(&self) -> Result<PackageHostSnapshot, PackageHostContractError> {
        let manifests = crate::ecosystem::builtin_ecosystem_manifests();
        let snapshot = package_host_snapshot_from_manifests(&manifests);
        snapshot.validate()?;
        Ok(snapshot)
    }
}

pub struct DatabasePackageHost<'a, H = BuiltinPackageHost> {
    db: &'a Database,
    base: H,
}

impl<'a> DatabasePackageHost<'a, BuiltinPackageHost> {
    pub fn builtin(db: &'a Database) -> Self {
        Self {
            db,
            base: BuiltinPackageHost,
        }
    }
}

impl<'a, H> DatabasePackageHost<'a, H> {
    pub fn new(db: &'a Database, base: H) -> Self {
        Self { db, base }
    }
}

impl<H> PackageHost for DatabasePackageHost<'_, H>
where
    H: PackageHost,
{
    fn snapshot(&self) -> Result<PackageHostSnapshot, PackageHostContractError> {
        let mut snapshot = self.base.snapshot()?;
        let states = self.db.list_package_host_states().map_err(|err| {
            PackageHostContractError::StateStore {
                message: err.to_string(),
            }
        })?;
        apply_package_host_state(&mut snapshot, &states);
        snapshot.validate()?;
        Ok(snapshot)
    }
}

pub fn builtin_package_host_snapshot() -> Result<PackageHostSnapshot, PackageHostContractError> {
    BuiltinPackageHost.snapshot()
}

pub fn builtin_runtime_package_context(
) -> Result<crate::runtime::RuntimePackageContext, PackageHostContractError> {
    BuiltinPackageHost.runtime_package_context()
}

pub fn database_backed_builtin_package_host_snapshot(
    db: &Database,
) -> Result<PackageHostSnapshot, PackageHostContractError> {
    DatabasePackageHost::builtin(db).snapshot()
}

pub fn database_backed_builtin_runtime_package_context(
    db: &Database,
) -> Result<crate::runtime::RuntimePackageContext, PackageHostContractError> {
    DatabasePackageHost::builtin(db).runtime_package_context()
}

pub fn apply_package_host_state(
    snapshot: &mut PackageHostSnapshot,
    states: &[PackageHostStateRecord],
) {
    let states = states
        .iter()
        .map(|state| (state.package_id.as_str(), state))
        .collect::<HashMap<_, _>>();
    for record in &mut snapshot.records {
        if let Some(state) = states.get(record.id.as_str()) {
            record.state = state.lifecycle_state;
            record.health = state.health_state;
        }
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

impl Database {
    pub fn upsert_package_host_state(
        &self,
        package_id: &str,
        lifecycle_state: PackageLifecycleState,
        health_state: PackageHealthState,
    ) -> Result<PackageHostStateRecord, CoreError> {
        let package_id = package_id.trim();
        if package_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "package_id must not be empty".to_string(),
            ));
        }

        let conn = self.conn();
        conn.execute(
            "INSERT INTO package_host_state
             (package_id, lifecycle_state, health_state, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(package_id) DO UPDATE SET
                lifecycle_state = excluded.lifecycle_state,
                health_state = excluded.health_state,
                updated_at = datetime('now')",
            rusqlite::params![package_id, lifecycle_state.as_str(), health_state.as_str()],
        )?;
        drop(conn);
        self.get_package_host_state(package_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Package host state {package_id}")))
    }

    pub fn set_package_host_package_enabled(
        &self,
        package_id: &str,
        enabled: bool,
    ) -> Result<PackageHostStateRecord, CoreError> {
        let existing = self.get_package_host_state(package_id)?;
        let health_state = existing
            .as_ref()
            .map(|state| state.health_state)
            .unwrap_or(PackageHealthState::Healthy);
        let lifecycle_state = match (enabled, health_state) {
            (false, _) => PackageLifecycleState::Disabled,
            (true, PackageHealthState::Unhealthy) => PackageLifecycleState::Unhealthy,
            (true, _) => PackageLifecycleState::Enabled,
        };
        self.upsert_package_host_state(package_id, lifecycle_state, health_state)
    }

    pub fn set_package_host_package_health(
        &self,
        package_id: &str,
        health_state: PackageHealthState,
    ) -> Result<PackageHostStateRecord, CoreError> {
        let existing = self.get_package_host_state(package_id)?;
        let lifecycle_state = match (
            existing.as_ref().map(|state| state.lifecycle_state),
            health_state,
        ) {
            (Some(PackageLifecycleState::Disabled), _) => PackageLifecycleState::Disabled,
            (Some(PackageLifecycleState::Blocked), _) => PackageLifecycleState::Blocked,
            (_, PackageHealthState::Unhealthy) => PackageLifecycleState::Unhealthy,
            (Some(PackageLifecycleState::Unhealthy), _) => PackageLifecycleState::Enabled,
            (Some(state), _) => state,
            (None, _) => PackageLifecycleState::Enabled,
        };
        self.upsert_package_host_state(package_id, lifecycle_state, health_state)
    }

    pub fn get_package_host_state(
        &self,
        package_id: &str,
    ) -> Result<Option<PackageHostStateRecord>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT package_id, lifecycle_state, health_state, updated_at
             FROM package_host_state
             WHERE package_id = ?1",
            rusqlite::params![package_id.trim()],
            package_host_state_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
    }

    pub fn list_package_host_states(&self) -> Result<Vec<PackageHostStateRecord>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT package_id, lifecycle_state, health_state, updated_at
             FROM package_host_state
             ORDER BY package_id",
        )?;
        let rows = stmt.query_map([], package_host_state_from_row)?;
        let mut states = Vec::new();
        for row in rows {
            states.push(row?);
        }
        Ok(states)
    }
}

fn package_host_state_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<PackageHostStateRecord, rusqlite::Error> {
    let lifecycle_wire = row.get::<_, String>(1)?;
    let health_wire = row.get::<_, String>(2)?;
    let lifecycle_state = PackageLifecycleState::from_wire(&lifecycle_wire).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(
            1,
            "lifecycle_state".to_string(),
            rusqlite::types::Type::Text,
        )
    })?;
    let health_state = PackageHealthState::from_wire(&health_wire).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(
            2,
            "health_state".to_string(),
            rusqlite::types::Type::Text,
        )
    })?;
    Ok(PackageHostStateRecord {
        package_id: row.get(0)?,
        lifecycle_state,
        health_state,
        updated_at: row.get(3)?,
    })
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
    #[error("runtime tool {tool_name} has no package owner")]
    UnownedRuntimeTool { tool_name: String },
    #[error("package host state store error: {message}")]
    StateStore { message: String },
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
    fn unhealthy_packages_disappear_from_runtime_components() {
        let snapshot = PackageHostSnapshot::new(vec![PackageHostRecord {
            id: "pkg-unhealthy".to_string(),
            version: Some("1.0.0".to_string()),
            state: PackageLifecycleState::Enabled,
            health: PackageHealthState::Unhealthy,
            dependencies: Vec::new(),
            permissions: Vec::new(),
            components: vec![component(
                "workflow-b",
                "pkg-unhealthy",
                PackageSurfaceKind::Workflow,
            )],
        }]);

        snapshot.validate().unwrap();

        assert!(snapshot.runtime_components().is_empty());
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
    fn runtime_assembler_assigns_every_builtin_tool_to_one_package() {
        let assembler = PackageRuntimeAssembler::from_host(&BuiltinPackageHost)
            .expect("builtin package snapshot");
        let registry = assembler.builtin_tool_registry();
        let expected_count = registry.tool_names().len();

        let capabilities = assembler
            .assemble_tool_registry(registry)
            .expect("builtin tools must all have package owners");

        assert_eq!(capabilities.tool_owners.len(), expected_count);
        assert_eq!(
            capabilities.tools.tool_names().len() + capabilities.excluded_tools.len(),
            expected_count
        );
    }

    #[test]
    fn runtime_assembler_rejects_unowned_dynamic_tools() {
        let assembler = PackageRuntimeAssembler::from_host(&BuiltinPackageHost)
            .expect("builtin package snapshot");

        assert_eq!(
            assembler
                .visible_tool_names(vec!["unowned_tool".to_string()])
                .unwrap_err(),
            PackageHostContractError::UnownedRuntimeTool {
                tool_name: "unowned_tool".to_string(),
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

    #[test]
    fn builtin_package_host_adapter_validates_snapshot() {
        let host = BuiltinPackageHost;
        let snapshot = host.snapshot().unwrap();

        assert!(snapshot.records.iter().any(|record| {
            record.id == "builtin-workflows" && record.state == PackageLifecycleState::Enabled
        }));
        assert!(snapshot.runtime_components().iter().any(|component| {
            component.package_id == "mcp-connectors"
                && component.kind == PackageSurfaceKind::Connector
        }));
    }

    #[test]
    fn builtin_package_host_adapter_projects_runtime_context() {
        let context = builtin_runtime_package_context().unwrap();

        assert!(context.disabled_package_ids.is_empty());
        assert!(context
            .enabled_package_ids
            .contains(&"builtin-skills".to_string()));
        assert!(context
            .enabled_package_ids
            .contains(&"builtin-workflows".to_string()));
        assert!(context
            .enabled_package_ids
            .contains(&"mcp-connectors".to_string()));
    }

    #[test]
    fn package_host_state_round_trips_through_database() {
        let db = Database::open_memory().unwrap();

        let saved = db
            .upsert_package_host_state(
                "office-documents",
                PackageLifecycleState::Disabled,
                PackageHealthState::Warning,
            )
            .unwrap();
        let loaded = db
            .get_package_host_state("office-documents")
            .unwrap()
            .unwrap();

        assert_eq!(saved.package_id, "office-documents");
        assert_eq!(loaded.lifecycle_state, PackageLifecycleState::Disabled);
        assert_eq!(loaded.health_state, PackageHealthState::Warning);
        assert!(!loaded.updated_at.trim().is_empty());
    }

    #[test]
    fn database_package_host_applies_disabled_state_to_runtime_context() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();

        let context = database_backed_builtin_runtime_package_context(&db).unwrap();

        assert!(context
            .disabled_package_ids
            .contains(&"office-documents".to_string()));
        assert!(!context
            .enabled_package_ids
            .contains(&"office-documents".to_string()));
    }

    #[test]
    fn database_package_host_applies_unhealthy_state_to_snapshot() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_health("mcp-connectors", PackageHealthState::Unhealthy)
            .unwrap();

        let snapshot = database_backed_builtin_package_host_snapshot(&db).unwrap();
        let record = snapshot
            .records
            .iter()
            .find(|record| record.id == "mcp-connectors")
            .unwrap();

        assert_eq!(record.state, PackageLifecycleState::Unhealthy);
        assert_eq!(record.health, PackageHealthState::Unhealthy);
        assert!(!snapshot.runtime_components().iter().any(|component| {
            component.package_id == "mcp-connectors"
                && component.kind == PackageSurfaceKind::Connector
        }));
    }

    #[test]
    fn package_host_health_update_does_not_reenable_disabled_package() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();
        db.set_package_host_package_health("office-documents", PackageHealthState::Healthy)
            .unwrap();

        let record = db
            .get_package_host_state("office-documents")
            .unwrap()
            .unwrap();

        assert_eq!(record.lifecycle_state, PackageLifecycleState::Disabled);
        assert_eq!(record.health_state, PackageHealthState::Healthy);
    }

    #[test]
    fn package_host_enable_does_not_restore_unhealthy_package() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_health("mcp-connectors", PackageHealthState::Unhealthy)
            .unwrap();
        db.set_package_host_package_enabled("mcp-connectors", true)
            .unwrap();

        let context = database_backed_builtin_runtime_package_context(&db).unwrap();

        assert!(context
            .disabled_package_ids
            .contains(&"mcp-connectors".to_string()));
        assert!(!context
            .enabled_package_ids
            .contains(&"mcp-connectors".to_string()));
    }
}
