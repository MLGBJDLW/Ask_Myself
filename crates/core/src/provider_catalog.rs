//! Shared provider/model preset catalog.
//!
//! The desktop UI and backend both read `shared/provider-presets.json` so
//! provider defaults do not drift between TypeScript and Rust.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::conversation::memory::{ContextWindowAuthority, ResolvedContextWindow};
use crate::llm::ProviderType;
use crate::model_catalog::{
    load_builtin_catalog, merge_catalog, resolve_or_derive_endpoint_id, CapabilityProbeResult,
    CatalogMergeInput, DiscoveredModel, ModelCatalogSnapshot, ModelDescriptor, ModelLimits,
    NativeWebSearchCapability, MODEL_DESCRIPTOR_SCHEMA_VERSION,
};
use crate::provider_registry::provider_type_from_key;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetCapability {
    pub enabled: bool,
    #[serde(default)]
    pub default_tokens: Option<u32>,
    #[serde(default)]
    pub min_tokens: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub step: Option<u32>,
    #[serde(default)]
    pub allow_zero: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningCapability {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub effort_levels: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub effort_budget_exclusive: bool,
    #[serde(default)]
    pub thinking_budget: Option<ThinkingBudgetCapability>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(skip)]
    reasoning_declared: bool,
    #[serde(skip)]
    vision_declared: bool,
}

impl<'de> Deserialize<'de> for ProviderCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawProviderCapabilities {
            #[serde(default)]
            reasoning: Option<ReasoningCapability>,
            #[serde(default)]
            vision: Option<bool>,
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        let reasoning_declared = value.get("reasoning").is_some();
        let vision_declared = value.get("vision").is_some();
        let raw = RawProviderCapabilities::deserialize(value).map_err(serde::de::Error::custom)?;

        Ok(Self {
            reasoning: raw.reasoning,
            vision: raw.vision,
            reasoning_declared,
            vision_declared,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tag_key: Option<String>,
    #[serde(default)]
    pub recommended: Option<bool>,
    #[serde(default)]
    pub capabilities: Option<ProviderCapabilities>,
    #[serde(default)]
    pub source: Option<ModelCatalogSource>,
    #[serde(default)]
    pub status: Option<ModelLifecycleStatus>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub supports_structured_output: Option<bool>,
    #[serde(default)]
    pub native_web_search: Option<NativeWebSearchCapability>,
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelCatalogSource {
    Official,
    Discovered,
    Curated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycleStatus {
    Active,
    Preview,
    Gated,
    Legacy,
    Deprecated,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelCatalogEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tag_key: Option<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub capabilities: Option<ProviderCapabilities>,
    pub source: ModelCatalogSource,
    pub status: ModelLifecycleStatus,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub supports_structured_output: Option<bool>,
    #[serde(default)]
    pub native_web_search: Option<NativeWebSearchCapability>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelCatalogSnapshot {
    #[serde(default = "model_descriptor_schema_version")]
    pub schema_version: u16,
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub endpoint_id: String,
    pub models: Vec<ProviderModelCatalogEntry>,
    #[serde(default)]
    pub descriptors: Vec<ModelDescriptor>,
    #[serde(default)]
    pub tombstones: Vec<ModelDescriptor>,
    pub refreshed_at: String,
    pub live_discovery_succeeded: bool,
    #[serde(default)]
    pub capability_probe_succeeded: bool,
}

const fn model_descriptor_schema_version() -> u16 {
    MODEL_DESCRIPTOR_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub models: Vec<ProviderModelPreset>,
    pub requires_api_key: bool,
    pub icon: String,
    pub description: String,
    #[serde(default)]
    pub capabilities: Option<ProviderCapabilities>,
    #[serde(default)]
    pub native_web_search: Option<NativeWebSearchCapability>,
}

const PROVIDER_PRESETS_JSON: &str = include_str!("../../../shared/provider-presets.json");

pub fn load_provider_presets() -> Result<Vec<ProviderPreset>, serde_json::Error> {
    serde_json::from_str(PROVIDER_PRESETS_JSON)
}

fn is_alibaba_beijing_workspace_endpoint(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().trim_end_matches('/') != "/compatible-mode/v1"
    {
        return false;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let suffix = ".cn-beijing.maas.aliyuncs.com";
    let Some(workspace_id) = host.strip_suffix(suffix) else {
        return false;
    };
    !workspace_id.is_empty()
        && !workspace_id.contains('.')
        && !matches!(workspace_id, "trial" | "token-plan")
}

pub fn find_provider_preset(provider: &str, base_url: Option<&str>) -> Option<ProviderPreset> {
    let presets = load_provider_presets().ok()?;
    let provider = provider.trim();
    let normalized_base_url = normalize_base_url(base_url);
    let lookup_provider = provider;

    if !normalized_base_url.is_empty() {
        if let Some(exact) = presets.iter().find(|preset| {
            preset.provider == lookup_provider
                && normalize_base_url(Some(&preset.base_url)) == normalized_base_url
        }) {
            return Some(exact.clone());
        }
        // DeepSeek documents both the origin and its OpenAI-compatible `/v1`
        // path. Treat only that exact path variant as the same trusted
        // endpoint; arbitrary paths, ports, schemes, and hosts still fail.
        if lookup_provider == "deep_seek" {
            if let Some(exact) = presets.iter().find(|preset| {
                preset.provider == lookup_provider
                    && format!(
                        "{}/v1",
                        normalize_base_url(Some(&preset.base_url)).trim_end_matches('/')
                    ) == normalized_base_url.trim_end_matches('/')
            }) {
                return Some(exact.clone());
            }
        }
        if lookup_provider == "alibaba_model_studio"
            && is_alibaba_beijing_workspace_endpoint(&normalized_base_url)
        {
            return presets
                .iter()
                .find(|preset| preset.id == "alibaba-model-studio")
                .cloned();
        }
        // A provider label never authorizes projecting a trusted catalog onto
        // an unknown, user-edited, HTTP, or non-standard-port endpoint.
        return None;
    }

    let mut provider_matches = presets
        .into_iter()
        .filter(|preset| preset.provider == lookup_provider)
        .collect::<Vec<_>>();
    if let Some(default_index) = provider_matches.iter().position(|preset| {
        preset.id == lookup_provider || preset.id.replace('-', "_") == lookup_provider
    }) {
        return Some(provider_matches.swap_remove(default_index));
    }
    if provider_matches.len() == 1 {
        provider_matches.pop()
    } else {
        None
    }
}

/// Resolve the context capacity owned by one configured provider route.
///
/// The provider endpoint and model ID form one capability identity. A model
/// alias on an edited or custom endpoint must therefore remain
/// provider-managed instead of borrowing a global model-family guess. Explicit
/// per-run overrides remain authoritative regardless of the endpoint.
pub fn resolve_endpoint_model_context_window(
    provider: &str,
    base_url: Option<&str>,
    model: &str,
    context_window_override: Option<u32>,
) -> ResolvedContextWindow {
    if let Some(capacity_tokens) = context_window_override {
        return ResolvedContextWindow {
            capacity_tokens: Some(capacity_tokens),
            authority: ContextWindowAuthority::UserOverride,
        };
    }

    let normalized_model = normalize_endpoint_model_id(model);
    let capacity_tokens = find_provider_preset(provider, base_url)
        .and_then(|preset| {
            preset
                .models
                .into_iter()
                .find(|candidate| normalize_endpoint_model_id(&candidate.id) == normalized_model)
        })
        .and_then(|model| model.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok());

    ResolvedContextWindow {
        capacity_tokens,
        authority: if capacity_tokens.is_some() {
            ContextWindowAuthority::Catalog
        } else {
            ContextWindowAuthority::ProviderManaged
        },
    }
}

pub fn preset_model_ids(provider: &str, base_url: Option<&str>) -> Vec<String> {
    find_provider_preset(provider, base_url)
        .map(|preset| preset.models.into_iter().map(|model| model.id).collect())
        .unwrap_or_default()
}

/// Merge the provider's account-scoped live model list with the curated
/// metadata overlay. Live IDs determine what the account can use right now;
/// curated entries supply stable labels and verified capabilities, while
/// curated-only models remain as offline fallbacks. Explicit tombstones are
/// omitted from both paths.
pub fn build_effective_model_catalog(
    provider: &str,
    base_url: Option<&str>,
    live_model_ids: Option<Vec<String>>,
    verified_model_id: Option<&str>,
    refreshed_at: impl Into<String>,
) -> ProviderModelCatalogSnapshot {
    let refreshed_at = refreshed_at.into();
    let live_model_ids = live_model_ids.map(|mut model_ids| {
        if let Some(verified_model_id) = verified_model_id
            .map(str::trim)
            .filter(|model_id| !model_id.is_empty())
        {
            let verified = normalize_model_id(verified_model_id);
            if !model_ids
                .iter()
                .any(|model_id| normalize_model_id(model_id) == verified)
            {
                // A successful completion is stronger account-scoped evidence
                // than an incomplete provider listing.
                model_ids.push(verified_model_id.to_string());
            }
        }
        model_ids
    });
    let preset = find_provider_preset(provider, base_url);
    let curated_models = preset
        .as_ref()
        .map(|preset| preset.models.as_slice())
        .unwrap_or_default();
    let default_regions = infer_regions(base_url);
    let tombstones = curated_models
        .iter()
        .filter(|model| model.status == Some(ModelLifecycleStatus::Removed))
        .map(|model| normalize_model_id(&model.id))
        .collect::<HashSet<_>>();
    let mut emitted = HashSet::new();
    let mut models = Vec::new();

    if let Some(live_models) = live_model_ids.as_ref() {
        let live_ids = live_models
            .iter()
            .map(|model| normalize_model_id(model))
            .collect::<HashSet<_>>();

        // Keep verified/recommended entries stable at the top when they are
        // available to this account, then retain the provider's order for
        // everything discovered dynamically.
        for curated in curated_models {
            let normalized = normalize_model_id(&curated.id);
            if live_ids.contains(&normalized)
                && !tombstones.contains(&normalized)
                && emitted.insert(normalized)
            {
                models.push(catalog_entry_from_preset(
                    curated,
                    &default_regions,
                    Some(&refreshed_at),
                ));
            }
        }

        for discovered in live_models {
            let normalized = normalize_model_id(discovered);
            if normalized.is_empty()
                || tombstones.contains(&normalized)
                || !emitted.insert(normalized)
            {
                continue;
            }
            models.push(ProviderModelCatalogEntry {
                id: discovered.trim().to_string(),
                name: discovered.trim().to_string(),
                tag_key: None,
                recommended: false,
                capabilities: None,
                source: ModelCatalogSource::Discovered,
                status: ModelLifecycleStatus::Active,
                regions: default_regions.clone(),
                last_verified_at: Some(refreshed_at.clone()),
                modalities: vec!["text".to_string()],
                supports_tools: Some(false),
                supports_structured_output: Some(false),
                native_web_search: None,
                reasoning_efforts: Vec::new(),
            });
        }
    }

    // Offline fallback and models omitted by a provider's incomplete listing.
    for curated in curated_models {
        let normalized = normalize_model_id(&curated.id);
        if tombstones.contains(&normalized) || !emitted.insert(normalized) {
            continue;
        }
        models.push(catalog_entry_from_preset(curated, &default_regions, None));
    }

    let descriptor_snapshot = build_descriptor_snapshot(
        provider,
        base_url,
        live_model_ids.as_deref(),
        verified_model_id,
        &refreshed_at,
    );

    ProviderModelCatalogSnapshot {
        schema_version: descriptor_snapshot.schema_version,
        provider: provider.trim().to_string(),
        base_url: base_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        endpoint_id: descriptor_snapshot.endpoint_id,
        models,
        descriptors: descriptor_snapshot.models,
        tombstones: descriptor_snapshot.tombstones,
        refreshed_at,
        live_discovery_succeeded: live_model_ids.is_some(),
        capability_probe_succeeded: descriptor_snapshot.capability_probe_succeeded,
    }
}

fn build_descriptor_snapshot(
    provider: &str,
    base_url: Option<&str>,
    live_model_ids: Option<&[String]>,
    verified_model_id: Option<&str>,
    refreshed_at: &str,
) -> ModelCatalogSnapshot {
    let endpoint_id = resolve_or_derive_endpoint_id("text", provider, base_url);
    let builtin = load_builtin_catalog().ok();
    let inherited_endpoint_id =
        find_provider_preset(provider, base_url).map(|preset| format!("text:{}", preset.id));
    let curated = builtin
        .as_ref()
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter(|model| {
                    model.endpoint_ids.iter().any(|id| id == &endpoint_id)
                        || inherited_endpoint_id.as_ref().is_some_and(|inherited| {
                            model.endpoint_ids.iter().any(|id| id == inherited)
                        })
                })
                .cloned()
                .map(|mut model| {
                    if !model.endpoint_ids.iter().any(|id| id == &endpoint_id) {
                        model.endpoint_ids = vec![endpoint_id.clone()];
                    }
                    model
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let canonical_provider_id = curated
        .first()
        .map(|model| model.provider_id.as_str())
        .unwrap_or_else(|| provider.trim());
    let region = builtin
        .as_ref()
        .and_then(|catalog| catalog.endpoints.iter().find(|item| item.id == endpoint_id))
        .map(|endpoint| endpoint.region.as_str())
        .unwrap_or_else(|| {
            if normalize_base_url(base_url).contains("dashscope-intl") {
                "ap-southeast-1"
            } else if normalize_base_url(base_url).contains("dashscope")
                || normalize_base_url(base_url).contains(".cn-beijing.maas.aliyuncs.com")
            {
                "cn-beijing"
            } else {
                "global"
            }
        });
    let discovered = live_model_ids.map(|ids| {
        ids.iter()
            .map(|id| DiscoveredModel::new(id, &endpoint_id, region))
            .collect::<Vec<_>>()
    });
    let probes = verified_model_id
        .map(str::trim)
        .filter(|model_id| !model_id.is_empty())
        .map(|model_id| CapabilityProbeResult::passed(model_id, &endpoint_id, refreshed_at))
        .into_iter()
        .collect::<Vec<_>>();

    merge_catalog(CatalogMergeInput {
        provider_id: canonical_provider_id,
        endpoint_id: &endpoint_id,
        curated: &curated,
        discovered: discovered.as_deref(),
        probes: &probes,
        refreshed_at,
    })
}

fn catalog_entry_from_preset(
    model: &ProviderModelPreset,
    default_regions: &[String],
    discovered_at: Option<&str>,
) -> ProviderModelCatalogEntry {
    let capabilities = model.capabilities.clone();
    let modalities = if model.modalities.is_empty() {
        let mut values = vec!["text".to_string()];
        if capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.vision)
            == Some(true)
        {
            values.push("image".to_string());
        }
        values
    } else {
        model.modalities.clone()
    };
    let reasoning_efforts = capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.reasoning.as_ref())
        .map(|reasoning| reasoning.effort_levels.clone())
        .unwrap_or_default();

    ProviderModelCatalogEntry {
        id: model.id.clone(),
        name: model.name.clone(),
        tag_key: model.tag_key.clone(),
        recommended: model.recommended.unwrap_or(false),
        capabilities,
        source: model.source.unwrap_or(ModelCatalogSource::Curated),
        status: model.status.unwrap_or_else(|| infer_lifecycle(model)),
        regions: if model.regions.is_empty() {
            default_regions.to_vec()
        } else {
            model.regions.clone()
        },
        last_verified_at: discovered_at
            .map(ToOwned::to_owned)
            .or_else(|| model.last_verified_at.clone()),
        modalities,
        supports_tools: model.supports_tools,
        supports_structured_output: model.supports_structured_output,
        native_web_search: model.native_web_search,
        reasoning_efforts,
    }
}

fn infer_lifecycle(model: &ProviderModelPreset) -> ModelLifecycleStatus {
    if model
        .tag_key
        .as_deref()
        .is_some_and(|tag| tag.eq_ignore_ascii_case("providers.tagPreview"))
        || model.id.to_ascii_lowercase().contains("preview")
    {
        ModelLifecycleStatus::Preview
    } else {
        ModelLifecycleStatus::Active
    }
}

fn infer_regions(base_url: Option<&str>) -> Vec<String> {
    let base_url = normalize_base_url(base_url);
    let is_z_ai_international = reqwest::Url::parse(&base_url).ok().is_some_and(|url| {
        url.scheme() == "https" && url.host_str().is_some_and(|host| host == "api.z.ai")
    });
    if base_url.contains("dashscope-intl") || is_z_ai_international {
        vec!["international".to_string()]
    } else if base_url.contains("dashscope") || base_url.contains("maas.aliyuncs.com") {
        vec!["cn-beijing".to_string()]
    } else {
        Vec::new()
    }
}

pub fn model_capabilities_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<ProviderCapabilities> {
    let model = normalize_model_id(model);
    if model.is_empty() {
        return None;
    }

    load_provider_presets()
        .ok()?
        .into_iter()
        .find_map(|preset| {
            let preset_provider_type = provider_type_from_key(&preset.provider)?;
            if preset_provider_type != provider_type {
                return None;
            }
            let model_preset = preset
                .models
                .iter()
                .find(|candidate| normalize_model_id(&candidate.id) == model)?;
            Some(merge_capabilities(
                preset.capabilities.as_ref(),
                model_preset.capabilities.as_ref(),
            ))
        })
}

pub fn model_limits_from_catalog(provider_type: ProviderType, model: &str) -> Option<ModelLimits> {
    let normalized_model = normalize_model_id(model);
    if normalized_model.is_empty() {
        return None;
    }
    if let Some(limits) = load_provider_presets()
        .ok()?
        .into_iter()
        .find_map(|preset| {
            (provider_type_from_key(&preset.provider) == Some(provider_type))
                .then_some(preset)
                .and_then(|preset| {
                    preset
                        .models
                        .into_iter()
                        .find(|candidate| normalize_model_id(&candidate.id) == normalized_model)
                })
                .and_then(|model| {
                    (model.context_tokens.is_some() || model.max_output_tokens.is_some()).then_some(
                        ModelLimits {
                            context_tokens: model.context_tokens,
                            max_output_tokens: model.max_output_tokens,
                            ..ModelLimits::default()
                        },
                    )
                })
        })
    {
        return Some(limits);
    }
    let catalog = load_builtin_catalog().ok()?;
    catalog.models.into_iter().find_map(|descriptor| {
        let model_matches = normalize_model_id(&descriptor.id) == normalized_model
            || descriptor
                .aliases
                .iter()
                .any(|alias| normalize_model_id(alias) == normalized_model);
        if !model_matches {
            return None;
        }
        let provider_matches = descriptor.endpoint_ids.iter().any(|endpoint_id| {
            catalog.endpoints.iter().any(|endpoint| {
                endpoint.id == *endpoint_id
                    && provider_type_from_key(&endpoint.provider_id) == Some(provider_type)
            })
        });
        provider_matches.then_some(descriptor.limits)
    })
}

/// Resolve context size by model id from the shared built-in catalog when the
/// provider is not available at the call site. Exact, full-limit lookups should
/// continue to use [`model_limits_from_catalog`].
pub fn model_context_tokens_from_shared_catalog(model: &str) -> Option<u64> {
    let normalized_model = normalize_model_id(model);
    if normalized_model.is_empty() {
        return None;
    }
    static SHARED_MODEL_CONTEXT_TOKENS: OnceLock<HashMap<String, u64>> = OnceLock::new();
    SHARED_MODEL_CONTEXT_TOKENS
        .get_or_init(|| {
            let mut context_tokens_by_model = HashMap::<String, u64>::new();
            let Ok(catalog) = load_builtin_catalog() else {
                return context_tokens_by_model;
            };
            for descriptor in catalog.models {
                let Some(context_tokens) = descriptor.limits.context_tokens else {
                    continue;
                };
                for id in std::iter::once(descriptor.id).chain(descriptor.aliases) {
                    context_tokens_by_model
                        .entry(normalize_model_id(&id))
                        .or_insert(context_tokens);
                }
            }
            context_tokens_by_model
        })
        .get(&normalized_model)
        .copied()
}

pub fn model_supports_reasoning_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<bool> {
    model_capabilities_from_catalog(provider_type, model)
        .map(|capabilities| capabilities.reasoning.is_some())
}

pub fn model_supports_vision_from_catalog(
    provider_type: ProviderType,
    model: &str,
) -> Option<bool> {
    model_capabilities_from_catalog(provider_type, model)
        .and_then(|capabilities| capabilities.vision)
}

fn merge_capabilities(
    provider_capabilities: Option<&ProviderCapabilities>,
    model_capabilities: Option<&ProviderCapabilities>,
) -> ProviderCapabilities {
    let mut merged = ProviderCapabilities::default();

    if let Some(capabilities) = provider_capabilities {
        if capabilities.reasoning_declared {
            merged.reasoning = capabilities.reasoning.clone();
            merged.reasoning_declared = true;
        }
        if capabilities.vision_declared {
            merged.vision = capabilities.vision;
            merged.vision_declared = true;
        }
    }

    if let Some(capabilities) = model_capabilities {
        if capabilities.reasoning_declared {
            merged.reasoning = capabilities.reasoning.clone();
            merged.reasoning_declared = true;
        }
        if capabilities.vision_declared {
            merged.vision = capabilities.vision;
            merged.vision_declared = true;
        }
    }

    merged
}

fn normalize_base_url(base_url: Option<&str>) -> String {
    crate::model_catalog::normalize_endpoint_url(base_url)
}

fn normalize_model_id(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

fn normalize_endpoint_model_id(model: &str) -> String {
    model
        .trim()
        .to_ascii_lowercase()
        .split([':', '~'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_context_resolution_preserves_route_and_user_authority() {
        assert_eq!(
            resolve_endpoint_model_context_window(
                "openrouter",
                Some("https://openrouter.ai/api/v1"),
                "z-ai/glm-5.3:free",
                None,
            ),
            ResolvedContextWindow {
                capacity_tokens: Some(1_048_576),
                authority: ContextWindowAuthority::Catalog,
            }
        );
        for (provider, endpoint) in [
            ("open_ai", "https://private.example/v1"),
            ("custom", "https://private.example/v1"),
        ] {
            assert_eq!(
                resolve_endpoint_model_context_window(provider, Some(endpoint), "gpt-5.6", None),
                ResolvedContextWindow {
                    capacity_tokens: None,
                    authority: ContextWindowAuthority::ProviderManaged,
                }
            );
        }
        assert_eq!(
            resolve_endpoint_model_context_window(
                "custom",
                Some("https://private.example/v1"),
                "gpt-5.6",
                Some(750_000),
            ),
            ResolvedContextWindow {
                capacity_tokens: Some(750_000),
                authority: ContextWindowAuthority::UserOverride,
            }
        );
    }

    #[test]
    fn catalog_v2_exposes_verified_gemini_context_and_output_limits() {
        let limits = model_limits_from_catalog(ProviderType::Google, "gemini-2.5-pro")
            .expect("Gemini limits");

        assert_eq!(limits.context_tokens, Some(1_048_576));
        assert_eq!(limits.max_output_tokens, Some(65_536));
    }

    #[test]
    fn deepseek_catalog_uses_v4_models() {
        let deepseek = find_provider_preset("deep_seek", Some("https://api.deepseek.com/v1"))
            .expect("deepseek preset should match by provider fallback");
        let ids = deepseek
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"deepseek-v4-pro"));
        assert!(ids.contains(&"deepseek-v4-flash"));
        assert!(!ids.contains(&"deepseek-reasoner"));
        assert!(!ids.contains(&"deepseek-chat"));

        let pro = deepseek
            .models
            .iter()
            .find(|model| model.id == "deepseek-v4-pro")
            .expect("deepseek-v4-pro should be listed");
        let flash = deepseek
            .models
            .iter()
            .find(|model| model.id == "deepseek-v4-flash")
            .expect("deepseek-v4-flash should be listed");
        let vision = deepseek
            .models
            .iter()
            .find(|model| model.id == "deepseek-v4-flash-vision-exp")
            .expect("deepseek-v4-flash-vision-exp should be listed");
        assert!(
            deepseek.native_web_search.is_none(),
            "Flash-only search must not leak through the provider preset"
        );
        for model in [pro, flash, vision] {
            assert!(
                model
                    .native_web_search
                    .expect("current DeepSeek V4 models should expose Responses search")
                    .can_mix_client_tools
            );
        }
        let reasoning = pro
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("deepseek-v4-pro should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec!["low".to_string(), "high".to_string(), "max".to_string()]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn openai_catalog_defaults_to_gpt_56() {
        let openai = find_provider_preset("open_ai", Some("https://api.openai.com/v1"))
            .expect("openai preset should match");
        let ids = openai
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"gpt-5.6"));
        assert!(ids.contains(&"gpt-5.6-sol"));
        assert!(ids.contains(&"gpt-5.6-terra"));
        assert!(ids.contains(&"gpt-5.6-luna"));
        assert!(ids.contains(&"gpt-5.4"));
        assert!(ids.contains(&"gpt-5.4-mini"));
        assert!(ids.contains(&"gpt-5.4-nano"));

        let gpt_56 = openai
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6")
            .expect("gpt-5.6 should be listed");
        assert_eq!(gpt_56.recommended, Some(true));

        let reasoning = gpt_56
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("gpt-5.6 should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "none".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
                "max".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn anthropic_catalog_defaults_to_fable_5() {
        let anthropic = find_provider_preset("anthropic", Some("https://api.anthropic.com/v1"))
            .expect("anthropic preset should match");
        let ids = anthropic
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"claude-fable-5"));
        assert!(ids.contains(&"claude-mythos-5"));
        assert!(ids.contains(&"claude-sonnet-5"));
        assert!(ids.contains(&"claude-opus-4-8"));
        assert!(ids.contains(&"claude-opus-4-7"));
        assert!(ids.contains(&"claude-sonnet-4-6"));

        let fable_5 = anthropic
            .models
            .iter()
            .find(|model| model.id == "claude-fable-5")
            .expect("claude-fable-5 should be listed");
        assert_eq!(fable_5.recommended, Some(true));

        let reasoning = fable_5
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("claude-fable-5 should expose reasoning capability");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
                "max".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn alibaba_model_studio_catalog_routes_qwen_and_third_party_models() {
        let qwen = find_provider_preset(
            "alibaba_model_studio",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        )
        .expect("Alibaba Model Studio preset should match");
        let ids = qwen
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"qwen3.8-max"));
        assert!(ids.contains(&"qwen3.8-flash"));
        assert!(ids.contains(&"qwen3.8-2.4t-a95b"));
        assert!(ids.contains(&"qwen3.8-27b"));
        assert!(ids.contains(&"kimi-k2.7-code"));
        assert!(ids.contains(&"glm-5.2"));
        assert!(ids.contains(&"MiniMax-M2.5"));
        assert!(ids.contains(&"qwen3.7-plus"));
        assert!(ids.contains(&"qwen3.7-max-2026-06-08"));
        assert!(ids.contains(&"qwen3.7-max-2026-05-20"));
        assert!(ids.contains(&"MiniMax/MiniMax-M3"));
        assert!(ids.contains(&"MiniMax/MiniMax-M2.7"));
        assert!(ids.contains(&"xiaomi/mimo-v2.5-pro"));
        assert!(ids.contains(&"kimi/kimi-k3"));
        assert!(ids.contains(&"qwen3-coder-flash"));
        assert!(ids.contains(&"glm-5.2-fast-preview"));
        assert!(ids.contains(&"ZHIPU/GLM-5.3"));
        assert!(ids.contains(&"qwen3.6-plus"));
        assert!(!ids.contains(&"qwen3.8-max-preview"));

        let qwen38 = qwen
            .models
            .iter()
            .find(|model| model.id == "qwen3.8-max")
            .expect("formal qwen3.8-max should be listed");
        assert_eq!(qwen38.recommended, Some(true));
        assert_eq!(qwen38.modalities, vec!["text", "image", "video"]);
        let qwen38_reasoning = qwen38
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("qwen3.8-max should expose endpoint-scoped reasoning");
        assert_eq!(qwen38_reasoning.mode.as_deref(), Some("optional"));
        assert!(qwen38_reasoning.effort_budget_exclusive);
        assert_eq!(qwen38_reasoning.default_effort.as_deref(), Some("low"));
        assert_eq!(
            qwen38_reasoning
                .thinking_budget
                .as_ref()
                .and_then(|budget| budget.max_tokens),
            Some(262_144)
        );

        let zhipu_direct = qwen
            .models
            .iter()
            .find(|model| model.id == "ZHIPU/GLM-5.3")
            .expect("Zhipu direct-supply GLM-5.3 should be listed");
        assert_eq!(zhipu_direct.regions, ["cn-beijing"]);
        assert_eq!(zhipu_direct.context_tokens, Some(1_048_576));
        assert_eq!(zhipu_direct.max_output_tokens, Some(131_072));
        assert_eq!(zhipu_direct.supports_tools, Some(true));
        assert_eq!(zhipu_direct.supports_structured_output, None);
        let zhipu_reasoning = zhipu_direct
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("Zhipu direct-supply GLM-5.3 should expose reasoning");
        assert_eq!(zhipu_reasoning.mode.as_deref(), Some("always"));
        assert_eq!(zhipu_reasoning.effort_levels, ["low", "high", "max"]);
        assert_eq!(zhipu_reasoning.default_effort.as_deref(), Some("max"));

        let workspace_url = "https://workspace123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";
        assert_eq!(
            find_provider_preset("alibaba_model_studio", Some(workspace_url))
                .expect("trusted workspace endpoint should inherit PAYG metadata")
                .id,
            "alibaba-model-studio"
        );
        let workspace_snapshot = build_effective_model_catalog(
            "alibaba_model_studio",
            Some(workspace_url),
            Some(vec!["ZHIPU/GLM-5.3".to_string()]),
            Some("ZHIPU/GLM-5.3"),
            "2026-08-27T00:00:00Z",
        );
        let workspace_glm = workspace_snapshot
            .descriptors
            .iter()
            .find(|model| model.id == "ZHIPU/GLM-5.3")
            .expect("workspace descriptor should retain curated GLM capabilities");
        assert_eq!(workspace_glm.regions, ["cn-beijing"]);
        assert_eq!(workspace_glm.limits.context_tokens, Some(1_048_576));
        assert_eq!(
            workspace_glm
                .capabilities
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.default_effort.as_deref()),
            Some("max")
        );
        assert_eq!(
            workspace_glm.endpoint_ids,
            vec![workspace_snapshot.endpoint_id.clone()]
        );

        for endpoint in [
            "http://workspace123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://trial.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://workspace123.cn-beijing.maas.aliyuncs.com.evil.example/compatible-mode/v1",
        ] {
            assert!(
                find_provider_preset("alibaba_model_studio", Some(endpoint)).is_none(),
                "{endpoint} must not inherit the PAYG workspace catalog"
            );
        }

        let qwen37 = qwen
            .models
            .iter()
            .find(|model| model.id == "qwen3.7-max")
            .expect("qwen3.7-max should be listed");
        assert_eq!(qwen37.recommended, Some(true));
        assert_eq!(qwen37.tag_key.as_deref(), Some("providers.tagLatest"));
        assert_eq!(
            qwen37
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.vision),
            Some(false)
        );

        let token_plan = find_provider_preset(
            "qwen",
            Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/"),
        )
        .expect("Qwen Token Plan preset should match its dedicated endpoint");
        assert_eq!(token_plan.id, "qwen-token-plan-cn");
        assert_eq!(token_plan.models.len(), 3);
        assert_eq!(token_plan.models[0].id, "qwen3.8-max");
        assert_eq!(token_plan.models[0].recommended, Some(true));
        assert_eq!(token_plan.models[0].max_output_tokens, Some(131_072));
        assert_eq!(
            token_plan.models[0].last_verified_at.as_deref(),
            Some("2026-08-28")
        );
        assert_eq!(token_plan.models[1].id, "qwen3.8-flash");
        assert_eq!(token_plan.models[1].recommended, Some(true));
        let flash_reasoning = token_plan.models[1]
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("Qwen3.8 Flash should expose its hybrid-thinking contract");
        assert_eq!(flash_reasoning.mode.as_deref(), Some("optional"));
        assert!(flash_reasoning
            .effort_levels
            .iter()
            .any(|effort| effort == "none"));
        assert_eq!(token_plan.models[2].id, "qwen3.8-max-preview");
        assert_eq!(
            token_plan.models[2].status,
            Some(ModelLifecycleStatus::Removed)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::Qwen, "qwen3.8-flash"),
            Some(true)
        );
        let global_token_plan = find_provider_preset(
            "qwen",
            Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
        )
        .expect("global Token Plan should keep its own endpoint identity");
        assert_eq!(global_token_plan.id, "qwen-token-plan-global");
        assert_eq!(global_token_plan.models[0].id, "qwen3.8-max");
        assert_eq!(global_token_plan.models[0].max_output_tokens, Some(131_072));
        assert_eq!(global_token_plan.models[1].id, "qwen3.8-flash");
        assert_eq!(
            find_provider_preset("alibaba_model_studio", None)
                .expect("Alibaba Model Studio should keep its pay-as-you-go default")
                .id,
            "alibaba-model-studio"
        );
        for preset_id in ["qwen-cloud-intl", "alibaba-model-studio"] {
            let preset = load_provider_presets()
                .unwrap()
                .into_iter()
                .find(|preset| preset.id == preset_id)
                .unwrap();
            let flagship = preset
                .models
                .iter()
                .find(|model| model.id == "qwen3.8-max")
                .unwrap();
            assert_eq!(flagship.max_output_tokens, Some(131_072));
            assert_eq!(flagship.last_verified_at.as_deref(), Some("2026-08-28"));
        }

        let migrated_qwen = find_provider_preset(
            "qwen",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        );
        assert!(migrated_qwen.is_none());

        let qwen_cloud = find_provider_preset(
            "alibaba_model_studio",
            Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        )
        .expect("QwenCloud international preset should match its pay-as-you-go endpoint");
        assert_eq!(qwen_cloud.id, "qwen-cloud-intl");
        assert_eq!(qwen_cloud.models[0].id, "qwen3.8-max");
        let flash = qwen_cloud
            .models
            .iter()
            .find(|model| model.id == "qwen3.8-flash")
            .expect("Qwen3.8 Flash should be listed for QwenCloud");
        assert_eq!(flash.recommended, Some(true));
        assert_eq!(
            flash
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.vision),
            Some(true)
        );
    }

    #[test]
    fn google_catalog_defaults_to_latest_stable_gemini_models() {
        let google = find_provider_preset(
            "google",
            Some("https://generativelanguage.googleapis.com/v1beta"),
        )
        .expect("Google preset should match");
        let ids = google
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"gemini-3.7-flash"));
        assert!(ids.contains(&"gemini-3.6-flash"));
        assert!(ids.contains(&"gemini-3.5-flash-lite"));
        assert!(!ids.contains(&"gemini-3-pro-preview"));
        assert!(!ids.contains(&"gemini-2.0-flash"));
        assert!(!ids.contains(&"gemini-2.0-flash-lite"));

        let latest = google
            .models
            .first()
            .expect("Gemini 3.7 Flash should be listed first");
        assert_eq!(latest.recommended, Some(true));
        assert_eq!(latest.source, Some(ModelCatalogSource::Official));
        let limits = model_limits_from_catalog(ProviderType::Google, &latest.id)
            .expect("Gemini 3.7 Flash limits should project from the shared catalog");
        assert_eq!(limits.context_tokens, Some(1_048_576));
        assert_eq!(limits.max_output_tokens, Some(65_536));
        let reasoning = latest
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("Gemini 3.7 Flash should expose reasoning controls");
        assert_eq!(
            reasoning.effort_levels,
            vec!["low".to_string(), "medium".to_string(), "high".to_string(),]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("medium"));
        assert_eq!(
            reasoning
                .thinking_budget
                .as_ref()
                .map(|budget| budget.enabled),
            Some(false)
        );
    }

    #[test]
    fn openrouter_catalog_uses_openrouter_provider_type() {
        let openrouter = find_provider_preset("openrouter", Some("https://openrouter.ai/api/v1"))
            .expect("openrouter preset should match");
        let ids = openrouter
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&"~anthropic/claude-fable-latest"));
        assert!(ids.contains(&"anthropic/claude-fable-5"));
        assert!(ids.contains(&"openai/gpt-5.6-sol"));
        assert!(ids.contains(&"x-ai/grok-4.6"));
        assert!(ids.contains(&"x-ai/grok-4.5"));
        assert!(ids.contains(&"anthropic/claude-sonnet-5"));
        assert!(ids.contains(&"z-ai/glm-5.3"));
        assert!(ids.contains(&"z-ai/glm-5.3-flash"));
        assert!(ids.contains(&"z-ai/glm-5.2"));
        assert!(ids.contains(&"moonshotai/kimi-k2.7-code"));
        assert!(ids.contains(&"qwen/qwen3.7-plus"));
        assert!(ids.contains(&"x-ai/grok-build-0.1"));
        assert!(ids.contains(&"anthropic/claude-sonnet-4.6"));
        let codex = openrouter
            .models
            .iter()
            .find(|model| model.id == "openai/gpt-5.3-codex")
            .expect("OpenRouter should include duplicate native Codex model");
        assert_eq!(codex.tag_key.as_deref(), Some("providers.tagCoding"));

        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenRouter, "x-ai/grok-4.3"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.7-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.7-max"),
            Some(false)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "z-ai/glm-5.3"),
            Some(false)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenRouter, "z-ai/glm-5.3-flash"),
            Some(true)
        );
        for model in ["z-ai/glm-5.3", "z-ai/glm-5.3-flash"] {
            let limits = model_limits_from_catalog(ProviderType::OpenRouter, model)
                .expect("OpenRouter GLM-5.3 route should expose limits");
            assert_eq!(limits.context_tokens, Some(1_048_576));
            assert_eq!(limits.max_output_tokens, Some(131_072));
            let reasoning = model_capabilities_from_catalog(ProviderType::OpenRouter, model)
                .and_then(|capabilities| capabilities.reasoning)
                .expect("OpenRouter GLM-5.3 route should expose reasoning");
            assert_eq!(reasoning.mode.as_deref(), Some("always"));
            assert_eq!(reasoning.effort_levels, ["low", "high", "max"]);
            assert_eq!(reasoning.default_effort.as_deref(), Some("max"));
        }
    }

    #[test]
    fn openai_compatible_presets_match_exact_base_urls() {
        let xai = find_provider_preset("open_ai", Some("https://api.x.ai/v1/"))
            .expect("xAI preset should match its exact base URL");
        assert_eq!(
            xai.models.first().map(|model| model.id.as_str()),
            Some("grok-4.6")
        );
        let grok46 = xai
            .models
            .first()
            .expect("Grok 4.6 should lead the direct xAI catalog");
        let reasoning = grok46
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("Grok 4.6 should expose reasoning controls");
        assert_eq!(
            reasoning.effort_levels,
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ]
        );
        assert_eq!(reasoning.default_effort.as_deref(), Some("high"));
        let limits = model_limits_from_catalog(ProviderType::OpenAi, "grok-4.6")
            .expect("Grok 4.6 limits should come from the shared catalog");
        assert_eq!(limits.context_tokens, Some(500_000));
        assert_eq!(limits.max_output_tokens, None);

        let minimax = find_provider_preset("open_ai", Some("https://api.minimax.io/v1"))
            .expect("MiniMax preset should match its exact base URL");
        assert_eq!(
            minimax.models.first().map(|model| model.id.as_str()),
            Some("MiniMax-M3")
        );

        let zhipu = find_provider_preset("zhipu", Some("https://open.bigmodel.cn/api/paas/v4"))
            .expect("Zhipu preset should match");
        let glm52 = zhipu
            .models
            .iter()
            .find(|model| model.id == "glm-5.2")
            .expect("GLM-5.2 should remain available as a legacy option");
        assert_eq!(glm52.recommended, Some(false));
        let glm52_reasoning = glm52
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("GLM-5.2 should expose reasoning controls");
        assert!(glm52_reasoning.effort_levels.contains(&"max".to_string()));

        let glm53 = zhipu
            .models
            .iter()
            .find(|model| model.id == "glm-5.3")
            .expect("the released GLM-5.3 model should be discoverable");
        assert_eq!(glm53.source, Some(ModelCatalogSource::Official));
        assert_eq!(glm53.status, Some(ModelLifecycleStatus::Active));
        assert_eq!(glm53.recommended, Some(true));
        let reasoning = glm53
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("GLM-5.3 should expose its always-on reasoning contract");
        assert_eq!(reasoning.mode.as_deref(), Some("always"));
        assert_eq!(reasoning.effort_levels, ["low", "high", "max"]);
        assert_eq!(reasoning.default_effort.as_deref(), Some("max"));
        let limits = model_limits_from_catalog(ProviderType::Zhipu, "glm-5.3")
            .expect("GLM-5.3 limits should project from the shared catalog");
        assert_eq!(limits.context_tokens, Some(1_000_000));
        assert_eq!(limits.max_output_tokens, Some(131_072));
        let glm53_flash = zhipu
            .models
            .iter()
            .find(|model| model.id == "glm-5.3-flash")
            .expect("GLM-5.3-Flash should be listed for the Model API");
        assert_eq!(glm53_flash.context_tokens, Some(1_000_000));
        assert_eq!(glm53_flash.max_output_tokens, Some(131_072));
        assert_eq!(
            glm53_flash
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.vision),
            Some(true)
        );
        let flash_reasoning = glm53_flash
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.reasoning.as_ref())
            .expect("GLM-5.3-Flash should expose mandatory reasoning");
        assert_eq!(flash_reasoning.mode.as_deref(), Some("always"));
        assert_eq!(flash_reasoning.effort_levels, ["low", "high", "max"]);

        let zhipu_international =
            find_provider_preset("zhipu", Some("https://api.z.ai/api/paas/v4"))
                .expect("international Z.ai Model API should have a distinct preset");
        assert_eq!(zhipu_international.id, "zhipu-intl");
        assert_ne!(zhipu_international.id, zhipu.id);
        for model_id in ["glm-5.3", "glm-5.3-flash"] {
            let international_model = zhipu_international
                .models
                .iter()
                .find(|model| model.id == model_id)
                .unwrap_or_else(|| panic!("missing international Z.ai model {model_id}"));
            assert_eq!(international_model.regions, ["international"]);
            assert_eq!(international_model.context_tokens, Some(1_000_000));
            assert_eq!(international_model.max_output_tokens, Some(131_072));
            assert_eq!(
                international_model
                    .capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.reasoning.as_ref())
                    .map(|reasoning| reasoning.effort_levels.as_slice()),
                Some(["low", "high", "max"].map(str::to_string).as_slice())
            );
        }
        assert_eq!(
            find_provider_preset("zhipu", None)
                .expect("legacy Zhipu identity should retain its China default")
                .id,
            "zhipu"
        );
        let international_snapshot = build_effective_model_catalog(
            "zhipu",
            Some("https://api.z.ai/api/paas/v4"),
            None,
            None,
            "2026-08-27T00:00:00Z",
        );
        assert_eq!(international_snapshot.endpoint_id, "text:zhipu-intl");
        assert!(international_snapshot.descriptors.iter().all(|descriptor| {
            descriptor.endpoint_ids == ["text:zhipu-intl"]
                && descriptor.regions == ["international"]
        }));
        let snapshot = build_effective_model_catalog(
            "zhipu",
            Some("https://open.bigmodel.cn/api/paas/v4"),
            None,
            None,
            "2026-08-22T00:00:00Z",
        );
        let glm53_descriptor = snapshot
            .descriptors
            .iter()
            .find(|model| model.id == "glm-5.3")
            .expect("GLM-5.3 descriptor should be available through the public Model API");
        assert_eq!(glm53_descriptor.available_to_credential, Some(true));
        assert_eq!(
            glm53_descriptor.product_readiness,
            crate::model_catalog::ProductReadiness::ProductReady
        );

        for base_url in [
            "https://open.bigmodel.cn/api/coding/paas/v4",
            "https://api.z.ai/api/coding/paas/v4",
        ] {
            assert!(
                find_provider_preset("zhipu", Some(base_url)).is_none(),
                "Coding Plan is restricted to officially supported clients"
            );
        }

        for (provider, base_url) in [
            (
                "qwen",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            ),
            (
                "qwen",
                "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            ),
            ("siliconflow", "https://api.siliconflow.cn/v1"),
        ] {
            let preset = find_provider_preset(provider, Some(base_url)).expect("known preset");
            assert!(preset
                .models
                .iter()
                .all(|model| !model.id.contains("glm-5.3")));
        }
    }

    #[test]
    fn provider_catalog_drives_vision_capabilities() {
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenAi, "gpt-5.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::DeepSeek, "deepseek-v4-pro"),
            Some(false)
        );
        assert_eq!(
            model_supports_vision_from_catalog(
                ProviderType::DeepSeek,
                "deepseek-v4-flash-vision-exp"
            ),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::AlibabaModelStudio, "qwen3-vl-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::AlibabaModelStudio, "qwen3.6-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::Doubao, "doubao-seed-2.0-pro"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::OpenAi, "grok-4.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_vision_from_catalog(ProviderType::LmStudio, "custom-vl-model"),
            None
        );
    }

    #[test]
    fn provider_catalog_drives_reasoning_capabilities() {
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "gpt-5.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "gpt-5.5-pro"),
            Some(false)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::DeepSeek, "deepseek-v4-pro"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::Anthropic, "claude-fable-5"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::AlibabaModelStudio, "qwen3.6-plus"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::OpenAi, "grok-4.6"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(
                ProviderType::OpenAi,
                "grok-4.20-0309-non-reasoning"
            ),
            Some(false)
        );
        assert_eq!(
            model_supports_reasoning_from_catalog(ProviderType::LmStudio, "custom-reasoner"),
            None
        );
    }

    #[test]
    fn qwen_token_plan_runtime_identity_resolves_full_qwen38_limits() {
        let limits = model_limits_from_catalog(ProviderType::Qwen, "qwen3.8-max")
            .expect("Token Plan Qwen identity should resolve canonical catalog limits");
        assert_eq!(limits.context_tokens, Some(1_000_000));
        assert_eq!(limits.max_output_tokens, Some(131_072));
        assert!(
            model_limits_from_catalog(ProviderType::AlibabaModelStudio, "qwen3.8-max-preview")
                .is_none(),
            "Token Plan preview limits must not leak into Model Studio"
        );

        let builtin = load_builtin_catalog().expect("built-in catalog");
        for endpoint_id in ["text:qwen-token-plan-cn", "text:qwen-token-plan-global"] {
            let endpoint = builtin
                .endpoints
                .iter()
                .find(|endpoint| endpoint.id == endpoint_id)
                .expect("Token Plan endpoint");
            assert_eq!(endpoint.provider_id, "qwen");
        }

        let openrouter_qwen =
            model_limits_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.8-max")
                .expect("OpenRouter Qwen3.8 should expose its real window");
        assert_eq!(openrouter_qwen.context_tokens, Some(1_000_000));

        let openrouter_flash =
            model_limits_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.8-flash")
                .expect("OpenRouter Qwen3.8 Flash should expose its endpoint limits");
        assert_eq!(openrouter_flash.context_tokens, Some(1_000_000));
        assert_eq!(openrouter_flash.max_output_tokens, Some(131_072));

        let openrouter_open =
            model_limits_from_catalog(ProviderType::OpenRouter, "qwen/qwen3.8-2.4t-a95b")
                .expect("OpenRouter Qwen3.8 open flagship should expose its endpoint limits");
        assert_eq!(openrouter_open.context_tokens, Some(1_048_576));
        assert_eq!(openrouter_open.max_output_tokens, Some(131_072));

        for model in ["google/gemini-3.7-flash", "google/gemini-3.6-flash"] {
            let limits = model_limits_from_catalog(ProviderType::OpenRouter, model)
                .expect("OpenRouter Gemini route should expose verified limits");
            assert_eq!(limits.context_tokens, Some(1_048_576));
            assert_eq!(limits.max_output_tokens, Some(65_536));
            assert_eq!(
                model_supports_vision_from_catalog(ProviderType::OpenRouter, model),
                Some(true)
            );
        }

        let openrouter_kimi =
            model_limits_from_catalog(ProviderType::OpenRouter, "moonshotai/kimi-k3")
                .expect("OpenRouter Kimi K3 should expose its real window");
        assert_eq!(openrouter_kimi.context_tokens, Some(1_048_576));

        let routed_kimi =
            model_limits_from_catalog(ProviderType::AlibabaModelStudio, "kimi/kimi-k3")
                .expect("Alibaba Kimi K3 should expose its real window");
        assert_eq!(routed_kimi.context_tokens, Some(1_048_576));
    }

    #[test]
    fn delegated_worker_limit_lookup_covers_representative_verified_families() {
        for (provider, model) in [
            (ProviderType::OpenAi, "gpt-5.6"),
            (ProviderType::Anthropic, "claude-fable-5"),
            (ProviderType::Google, "gemini-3.7-flash"),
            (ProviderType::DeepSeek, "deepseek-v4-pro"),
            (ProviderType::Moonshot, "kimi-k3"),
            (ProviderType::Qwen, "qwen3.8-max"),
            (ProviderType::Qwen, "qwen3.8-flash"),
            (ProviderType::AlibabaModelStudio, "qwen3.8-max"),
            (ProviderType::AlibabaModelStudio, "qwen3.8-27b"),
            (ProviderType::Zhipu, "glm-5.3"),
            (ProviderType::OpenRouter, "moonshotai/kimi-k3"),
        ] {
            let limits = model_limits_from_catalog(provider, model).unwrap_or_else(|| {
                panic!("missing delegated worker limits for {provider:?}:{model}")
            });
            assert!(
                limits.context_tokens.is_some_and(|tokens| tokens > 0),
                "missing context limit for {provider:?}:{model}"
            );
            assert!(
                limits.max_output_tokens.is_some_and(|tokens| tokens > 0),
                "missing output limit for {provider:?}:{model}"
            );
        }
    }

    #[test]
    fn effective_catalog_keeps_live_unknown_models_with_conservative_capabilities() {
        let snapshot = build_effective_model_catalog(
            "alibaba_model_studio",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            Some(vec![
                "qwen3.7-max".to_string(),
                "account-only-model".to_string(),
                "account-only-model".to_string(),
            ]),
            None,
            "2026-07-31T00:00:00Z",
        );

        assert!(snapshot.live_discovery_succeeded);
        let known = snapshot
            .models
            .iter()
            .find(|model| model.id == "qwen3.7-max")
            .expect("known live model should retain curated metadata");
        assert_eq!(known.source, ModelCatalogSource::Official);
        assert_eq!(known.modalities, vec!["text".to_string()]);
        assert_eq!(
            known.last_verified_at.as_deref(),
            Some("2026-07-31T00:00:00Z")
        );

        let discovered = snapshot
            .models
            .iter()
            .find(|model| model.id == "account-only-model")
            .expect("unknown live model must remain selectable");
        assert_eq!(discovered.source, ModelCatalogSource::Discovered);
        assert_eq!(discovered.supports_tools, Some(false));
        assert_eq!(discovered.supports_structured_output, Some(false));
        assert!(discovered.reasoning_efforts.is_empty());
        assert_eq!(
            snapshot
                .models
                .iter()
                .filter(|model| model.id == "account-only-model")
                .count(),
            1
        );
    }

    #[test]
    fn effective_catalog_falls_back_to_curated_models_when_listing_fails() {
        let snapshot = build_effective_model_catalog(
            "open_ai",
            Some("https://api.openai.com/v1"),
            None,
            None,
            "2026-07-31T00:00:00Z",
        );

        assert!(!snapshot.live_discovery_succeeded);
        assert!(snapshot.models.iter().any(|model| model.id == "gpt-5.6"));
        assert!(snapshot
            .models
            .iter()
            .all(|model| model.source != ModelCatalogSource::Discovered));
    }

    #[test]
    fn successful_probe_keeps_a_model_available_when_listing_omits_it() {
        let snapshot = build_effective_model_catalog(
            "alibaba_model_studio",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            Some(vec!["account-only-model".to_string()]),
            Some("qwen3.7-max"),
            "2026-07-31T00:00:00Z",
        );

        let tested = snapshot
            .descriptors
            .iter()
            .find(|model| model.id == "qwen3.7-max")
            .expect("the tested curated model should remain in the descriptor snapshot");
        assert_eq!(tested.available_to_credential, Some(true));
        assert_eq!(
            tested.product_readiness,
            crate::model_catalog::ProductReadiness::Callable
        );
        assert_eq!(
            tested.last_verified_at.as_deref(),
            Some("2026-07-31T00:00:00Z")
        );
        assert!(snapshot
            .models
            .iter()
            .any(|model| model.id == "qwen3.7-max"));
    }
}
