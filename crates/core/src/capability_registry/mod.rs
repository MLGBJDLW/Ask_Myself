//! Authoritative connection, model-target, and capability routing registry.
//!
//! Settings V2 owns scoped user choices. This module canonicalizes those
//! choices into secret-free registry records, validates capability routes,
//! resolves credentials only at the provider boundary, and pins the exact
//! revision set used by a task run. Legacy rows remain the credential owner
//! and rollback source during the compatibility window.

mod resolver;
mod storage;
mod types;

pub use resolver::{build_registry_projection, capability_requirement};
pub(crate) use storage::{migrate_registry_on_open, sync_registry_in_transaction};
pub use types::{
    CapabilityEligibility, CapabilityRegistryProjection, CapabilityRequirement, ConnectionHealth,
    ConnectionRecord, ModelDefinitionRecord, ModelTargetRecord, RegistryActivationRecord,
    RegistryReadMode, RegistryScope, ResolvedCapabilityRoute, ResolvedCapabilityRouteTarget,
    RuntimeCapabilityFallback, RuntimeCapabilityResolution, RuntimeRegistrySnapshot,
    RuntimeRouteTargetSnapshot, TargetAvailability, CAPABILITY_REGISTRY_SCHEMA_VERSION,
};
