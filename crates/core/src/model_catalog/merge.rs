use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::descriptor::normalize_id;
use super::{
    CapabilityProbeResult, DiscoveredModel, ModelAccess, ModelCatalogSource, ModelDescriptor,
    ModelLifecycle, ProductReadiness, MODEL_DESCRIPTOR_SCHEMA_VERSION,
};

pub struct CatalogMergeInput<'a> {
    pub provider_id: &'a str,
    pub endpoint_id: &'a str,
    pub curated: &'a [ModelDescriptor],
    pub discovered: Option<&'a [DiscoveredModel]>,
    pub probes: &'a [CapabilityProbeResult],
    pub refreshed_at: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogSnapshot {
    pub schema_version: u16,
    pub provider_id: String,
    pub endpoint_id: String,
    pub models: Vec<ModelDescriptor>,
    #[serde(default)]
    pub tombstones: Vec<ModelDescriptor>,
    pub refreshed_at: String,
    pub live_discovery_succeeded: bool,
    pub capability_probe_succeeded: bool,
}

pub fn merge_catalog(input: CatalogMergeInput<'_>) -> ModelCatalogSnapshot {
    let endpoint_id = input.endpoint_id.trim();
    let live = input.discovered.map(|models| {
        models
            .iter()
            .filter(|model| model.endpoint_id.eq_ignore_ascii_case(endpoint_id))
            .collect::<Vec<_>>()
    });
    let live_ids = live
        .as_ref()
        .map(|models| {
            models
                .iter()
                .map(|model| normalize_id(&model.id))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let tombstones = input
        .curated
        .iter()
        .filter(|model| model.lifecycle == ModelLifecycle::Removed)
        .cloned()
        .map(|mut model| {
            attach_endpoint(&mut model, endpoint_id);
            model
        })
        .collect::<Vec<_>>();
    let tombstone_ids = tombstones
        .iter()
        .flat_map(|model| std::iter::once(&model.id).chain(model.aliases.iter()))
        .map(|id| normalize_id(id))
        .collect::<HashSet<_>>();
    let curated_by_live_id = input
        .curated
        .iter()
        .filter(|model| model.lifecycle != ModelLifecycle::Removed)
        .flat_map(|model| {
            std::iter::once(model.id.as_str())
                .chain(model.aliases.iter().map(String::as_str))
                .map(move |id| (normalize_id(id), model))
        })
        .collect::<HashMap<_, _>>();

    let mut emitted = HashSet::new();
    let mut models = Vec::new();
    for curated in input
        .curated
        .iter()
        .filter(|model| model.lifecycle != ModelLifecycle::Removed)
    {
        let mut model = curated.clone();
        attach_endpoint(&mut model, endpoint_id);
        if input.discovered.is_some() {
            let available = std::iter::once(&model.id)
                .chain(model.aliases.iter())
                .map(|id| normalize_id(id))
                .any(|id| live_ids.contains(&id));
            model.available_to_credential = Some(available);
        }
        apply_probe(&mut model, input.probes, endpoint_id);
        emitted.insert(normalize_id(&model.id));
        models.push(model);
    }

    for discovered in live.into_iter().flatten() {
        let normalized = normalize_id(&discovered.id);
        if normalized.is_empty()
            || tombstone_ids.contains(&normalized)
            || curated_by_live_id.contains_key(&normalized)
            || !emitted.insert(normalized)
        {
            continue;
        }
        let mut model = ModelDescriptor::new(
            discovered.id.trim(),
            input.provider_id.trim(),
            discovered
                .display_name
                .as_deref()
                .unwrap_or_else(|| discovered.id.trim()),
        );
        model.source = ModelCatalogSource::Discovered;
        model.access = ModelAccess::AccountEnablement;
        model.product_readiness = ProductReadiness::Discoverable;
        model.available_to_credential = Some(true);
        model.last_verified_at = Some(input.refreshed_at.to_string());
        attach_endpoint(&mut model, endpoint_id);
        push_unique(&mut model.regions, discovered.region.trim());
        apply_probe(&mut model, input.probes, endpoint_id);
        models.push(model);
    }

    ModelCatalogSnapshot {
        schema_version: MODEL_DESCRIPTOR_SCHEMA_VERSION,
        provider_id: input.provider_id.trim().to_string(),
        endpoint_id: endpoint_id.to_string(),
        models,
        tombstones,
        refreshed_at: input.refreshed_at.to_string(),
        live_discovery_succeeded: input.discovered.is_some(),
        capability_probe_succeeded: input
            .probes
            .iter()
            .any(|probe| probe.endpoint_id.eq_ignore_ascii_case(endpoint_id) && probe.is_passed()),
    }
}

fn attach_endpoint(model: &mut ModelDescriptor, endpoint_id: &str) {
    push_unique(&mut model.endpoint_ids, endpoint_id);
    if let Some((kind, _)) = endpoint_id.split_once(':') {
        push_unique(&mut model.endpoint_kinds, kind);
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.is_empty()
        && !values
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(value))
    {
        values.push(value.to_string());
    }
}

fn apply_probe(model: &mut ModelDescriptor, probes: &[CapabilityProbeResult], endpoint_id: &str) {
    let Some(probe) = probes.iter().find(|probe| {
        probe.endpoint_id.eq_ignore_ascii_case(endpoint_id)
            && model.matches_id_or_alias(&probe.model_id)
    }) else {
        return;
    };
    if probe.is_passed() {
        model.available_to_credential = Some(true);
        if model.product_readiness < ProductReadiness::Callable {
            model.product_readiness = ProductReadiness::Callable;
        }
        model.last_verified_at = Some(probe.verified_at.clone());
        if let Some(capabilities) = probe.capabilities.as_ref() {
            capabilities.apply_to(&mut model.capabilities);
        }
    } else if model.source == ModelCatalogSource::Discovered {
        model.product_readiness = ProductReadiness::Discoverable;
    }
}
