use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model_catalog::ModelDescriptor;
use crate::settings_schema_v2::{
    CapabilityBindingConstraintsV2, CapabilityFallbackModeV2, SettingsRevisionV2, SettingsScopeV2,
};

pub const CAPABILITY_REGISTRY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionHealth {
    Unknown,
    Configured,
    Missing,
    Invalid,
    Expired,
}

impl ConnectionHealth {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Configured => "configured",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetAvailability {
    Unknown,
    Unavailable,
    Discoverable,
    Callable,
    ProductReady,
}

impl TargetAvailability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unavailable => "unavailable",
            Self::Discoverable => "discoverable",
            Self::Callable => "callable",
            Self::ProductReady => "product_ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRecord {
    pub schema_version: u16,
    pub id: String,
    pub revision: u64,
    /// Runtime adapter identity selected by the connection preset. It remains
    /// distinct from the canonical catalog provider namespace (for example,
    /// Qwen Token Plan versus Alibaba Model Studio).
    pub adapter_provider_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub base_url: String,
    pub endpoint_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    pub enabled: bool,
    pub health: ConnectionHealth,
    pub source: SettingsScopeV2,
    pub source_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefinitionRecord {
    pub id: String,
    pub revision: u64,
    pub descriptor_hash: String,
    pub descriptor: ModelDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTargetRecord {
    pub id: String,
    pub revision: u64,
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_definition_id: Option<String>,
    pub upstream_model_id: String,
    pub availability: TargetAvailability,
    pub source: SettingsScopeV2,
    pub source_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryReadMode {
    Legacy,
    Registry,
}

impl RegistryReadMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Registry => "registry",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "legacy" => Some(Self::Legacy),
            "registry" => Some(Self::Registry),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryActivationRecord {
    pub capability_id: String,
    pub scope: SettingsScopeV2,
    pub read_mode: RegistryReadMode,
    pub registry_revision: u64,
    pub parity_status: String,
    pub parity: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub text_input: bool,
    pub image_input: bool,
    pub audio_input: bool,
    pub image_output: bool,
    pub audio_output: bool,
    pub video_output: bool,
    pub embedding_output: bool,
    pub reasoning: bool,
    pub async_jobs: bool,
}

impl CapabilityRequirement {
    pub const fn text() -> Self {
        Self {
            text_input: true,
            image_input: false,
            audio_input: false,
            image_output: false,
            audio_output: false,
            video_output: false,
            embedding_output: false,
            reasoning: false,
            async_jobs: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityEligibility {
    pub eligible: bool,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapabilityRouteTarget {
    pub target: ModelTargetRecord,
    pub connection: ConnectionRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<ModelDefinitionRecord>,
    pub eligibility: CapabilityEligibility,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapabilityRoute {
    pub binding_id: String,
    pub binding_revision: u64,
    pub capability_id: String,
    pub source: SettingsScopeV2,
    pub source_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<ResolvedCapabilityRouteTarget>,
    #[serde(default)]
    pub fallbacks: Vec<ResolvedCapabilityRouteTarget>,
    pub fallback_mode: CapabilityFallbackModeV2,
    pub constraints: CapabilityBindingConstraintsV2,
    /// Capability-specific, secret-free policy frozen with the binding.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRegistryProjection {
    pub schema_version: u16,
    pub settings_revisions: Vec<SettingsRevisionV2>,
    pub connections: Vec<ConnectionRecord>,
    pub model_definitions: Vec<ModelDefinitionRecord>,
    pub model_targets: Vec<ModelTargetRecord>,
    pub capabilities: Vec<ResolvedCapabilityRoute>,
    pub activations: Vec<RegistryActivationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRouteTargetSnapshot {
    pub fallback_index: usize,
    pub target_id: String,
    pub target_revision: u64,
    pub connection_id: String,
    pub connection_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_definition_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_hash: Option<String>,
    pub adapter_provider_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRegistrySnapshot {
    pub schema_version: u16,
    pub settings_revisions: Vec<SettingsRevisionV2>,
    pub binding_id: String,
    pub binding_revision: u64,
    pub capability_id: String,
    pub target_id: String,
    pub target_revision: u64,
    pub connection_id: String,
    pub connection_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_definition_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_hash: Option<String>,
    pub fallback_index: usize,
    pub fallback_mode: CapabilityFallbackModeV2,
    #[serde(default)]
    pub constraints: CapabilityBindingConstraintsV2,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub adapter_provider_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    pub model_id: String,
    /// Frozen, policy-eligible automatic fallback plan. The selected target
    /// may advance only forward through this exact list and only before any
    /// response output is exposed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_targets: Vec<RuntimeRouteTargetSnapshot>,
}

#[derive(Debug, Clone)]
pub struct RuntimeCapabilityFallback {
    pub fallback_index: usize,
    pub provider_id: String,
    pub endpoint_id: String,
    pub provider_config: crate::llm::ProviderConfig,
    pub model_id: String,
}

/// Secret-bearing runtime value. It is intentionally not serializable; only
/// `snapshot` may cross an IPC, trace, or persistence boundary.
#[derive(Debug, Clone)]
pub struct RuntimeCapabilityResolution {
    pub provider_id: String,
    pub endpoint_id: String,
    pub provider_config: crate::llm::ProviderConfig,
    pub model_id: String,
    pub snapshot: RuntimeRegistrySnapshot,
    pub fallbacks: Vec<RuntimeCapabilityFallback>,
}
