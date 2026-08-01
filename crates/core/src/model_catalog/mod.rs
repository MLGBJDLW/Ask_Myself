//! Canonical provider, endpoint, and model catalog contracts.
//!
//! Catalog v2 deliberately separates model identity and capability metadata
//! from the endpoint that serves it. Modality-specific preset readers project
//! into these types while their legacy shapes remain available during the
//! migration window.

mod cache;
mod descriptor;
mod discovery;
mod lifecycle;
mod merge;
mod probe;
mod projection;
mod provider_endpoint;

pub use cache::{CatalogCacheKey, LastGoodCatalogCache};
pub use descriptor::{
    ModelAccess, ModelCapabilities, ModelCatalogSource, ModelDescriptor, ModelLifecycle,
    ModelLimits, ModelModality, ProductReadiness, ReasoningCapability, ThinkingBudgetCapability,
    MODEL_DESCRIPTOR_SCHEMA_VERSION,
};
pub use discovery::DiscoveredModel;
pub use lifecycle::{
    resolve_saved_selection, select_implicit_default, SavedModelSelection,
    SavedModelSelectionResolution, SelectionResolutionKind,
};
pub use merge::{merge_catalog, CatalogMergeInput, ModelCatalogSnapshot};
pub use probe::{CapabilityProbeResult, CapabilityProbeStatus, VerifiedModelCapabilities};
pub use projection::{
    load_builtin_catalog, resolve_builtin_endpoint_id, resolve_or_derive_endpoint_id,
    BuiltinModelCatalog,
};
pub use provider_endpoint::{
    AuthStyle, CredentialKind, DiscoveryStrategy, EndpointRegistry, EndpointTransport, HealthProbe,
    ProviderDescriptor, ProviderEndpoint,
};
