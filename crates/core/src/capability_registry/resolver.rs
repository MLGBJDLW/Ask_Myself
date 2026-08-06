use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::to_vec;

use crate::error::CoreError;
use crate::model_catalog::{
    load_builtin_catalog, normalize_endpoint_url, ModelDescriptor, ModelLifecycle, ModelModality,
    ProductReadiness,
};
use crate::provider_registry::provider_type_for_parts;
use crate::settings_schema_v2::{
    resolve_settings_v2, CapabilityBindingConstraintsV2, CapabilityBindingV2,
    CapabilityFallbackModeV2, ConnectionReferenceV2, ModelReferenceV2, ResolvedSettingV2,
    SettingOverrideV2, SettingsProfileV2, SettingsScopeKindV2, SettingsScopeV2,
};

use super::types::{
    CapabilityEligibility, CapabilityRegistryProjection, CapabilityRequirement, ConnectionHealth,
    ConnectionRecord, ModelDefinitionRecord, ModelTargetRecord, RegistryActivationRecord,
    ResolvedCapabilityRoute, ResolvedCapabilityRouteTarget, TargetAvailability,
    CAPABILITY_REGISTRY_SCHEMA_VERSION,
};

pub fn capability_requirement(capability_id: &str) -> CapabilityRequirement {
    let mut requirement = CapabilityRequirement::text();
    match capability_id.trim().to_ascii_lowercase().as_str() {
        "reasoning" => requirement.reasoning = true,
        "vision" => requirement.image_input = true,
        "image_generation" => requirement.image_output = true,
        "image_editing" => {
            requirement.image_input = true;
            requirement.image_output = true;
        }
        "video_generation" => {
            requirement.video_output = true;
            requirement.async_jobs = true;
        }
        "speech_to_text" => requirement.audio_input = true,
        "text_to_speech" => requirement.audio_output = true,
        "embedding" => {
            requirement.text_input = true;
            requirement.embedding_output = true;
        }
        "reranking" => requirement.text_input = true,
        _ => {}
    }
    requirement
}

pub fn build_registry_projection(
    all_profiles: &[SettingsProfileV2],
    selected_profiles: &[SettingsProfileV2],
    credential_health: &HashMap<String, ConnectionHealth>,
    activations: Vec<RegistryActivationRecord>,
) -> Result<CapabilityRegistryProjection, CoreError> {
    let catalog = load_builtin_catalog().map_err(CoreError::InvalidInput)?;
    let resolved = resolve_settings_v2(selected_profiles)?;
    let definitions = catalog
        .models
        .iter()
        .map(model_definition)
        .collect::<Result<Vec<_>, _>>()?;
    let definitions_by_id = definitions
        .iter()
        .map(|definition| (definition.id.clone(), definition.clone()))
        .collect::<HashMap<_, _>>();

    let mut aliases = HashMap::<String, String>::new();
    let mut connections = BTreeMap::<String, ConnectionRecord>::new();
    let selected_profile_ids = selected_profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<BTreeSet<_>>();
    for profile in all_profiles {
        for (connection_key, value) in &profile.overrides.connections {
            let SettingOverrideV2::Set { value } = value else {
                continue;
            };
            let record = connection_record(
                connection_key,
                value,
                &profile.scope,
                profile.revision,
                credential_health,
                &catalog,
            )?;
            if selected_profile_ids.contains(profile.id.as_str()) {
                let alias = value.id.trim().to_string();
                if aliases
                    .insert(alias.clone(), record.id.clone())
                    .is_some_and(|existing| existing != record.id)
                {
                    return Err(CoreError::Conflict(format!(
                        "Connection alias {alias} resolves to multiple identities in the selected scope chain"
                    )));
                }
            }
            match connections.get(&record.id) {
                Some(existing) if !same_connection_identity(existing, &record) => {
                    return Err(CoreError::Conflict(format!(
                        "Connection registry identity {} has conflicting definitions",
                        record.id
                    )));
                }
                Some(_) => {}
                None => {
                    connections.insert(record.id.clone(), record);
                }
            }
        }
    }

    let mut target_by_key = BTreeMap::<(String, String), ModelTargetRecord>::new();
    let mut definition_by_target = HashMap::<String, Option<ModelDefinitionRecord>>::new();
    for connection in connections.values() {
        for descriptor in catalog.models.iter().filter(|descriptor| {
            canonical_provider_id(&descriptor.provider_id, &catalog)
                == canonical_provider_id(&connection.provider_id, &catalog)
                && (descriptor
                    .endpoint_ids
                    .iter()
                    .any(|id| id == &connection.endpoint_id)
                    || connection.endpoint_id.contains(":custom-"))
        }) {
            let definition = model_definition(descriptor)?;
            let target = model_target(
                connection,
                &descriptor.id,
                Some(&definition),
                &connection.source,
                connection.source_revision,
                false,
            );
            definition_by_target.insert(target.id.clone(), Some(definition));
            target_by_key.insert(
                (connection.id.clone(), normalize_model_id(&descriptor.id)),
                target,
            );
        }
    }

    let mut capabilities = Vec::new();
    let mut effective_bindings = resolved.capabilities.clone();
    if !effective_bindings.contains_key("text_generation") {
        if let Some(model) = resolved.models.get("text") {
            effective_bindings.insert(
                "text_generation".to_string(),
                ResolvedSettingV2 {
                    value: model.value.clone().map(|primary| CapabilityBindingV2 {
                        primary: Some(primary),
                        fallbacks: Vec::new(),
                        fallback_mode: CapabilityFallbackModeV2::Disabled,
                        constraints: CapabilityBindingConstraintsV2::default(),
                        options: BTreeMap::new(),
                    }),
                    source: model.source.clone(),
                    source_revision: model.source_revision,
                    preset_origin: model.preset_origin.clone(),
                },
            );
        }
    }
    {
        let mut context = BindingResolutionContext {
            connections: &connections,
            aliases: &aliases,
            descriptors: &catalog.models,
            definitions_by_id: &definitions_by_id,
            target_by_key: &mut target_by_key,
            definition_by_target: &mut definition_by_target,
        };
        for (capability_id, binding) in effective_bindings {
            let Some(binding_value) = binding.value.as_ref() else {
                continue;
            };
            capabilities.push(resolve_binding(
                &mut context,
                &capability_id,
                binding_value,
                &binding.source,
                binding.source_revision,
            )?);
        }
    }

    Ok(CapabilityRegistryProjection {
        schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
        settings_revisions: resolved.revisions,
        connections: connections.into_values().collect(),
        model_definitions: definitions,
        model_targets: target_by_key.into_values().collect(),
        capabilities,
        activations,
    })
}

fn connection_record(
    connection_key: &str,
    value: &ConnectionReferenceV2,
    source: &SettingsScopeV2,
    source_revision: u64,
    credential_health: &HashMap<String, ConnectionHealth>,
    catalog: &crate::model_catalog::BuiltinModelCatalog,
) -> Result<ConnectionRecord, CoreError> {
    let base_url = sanitize_endpoint_url(value.base_url.as_deref())?;
    let provider_id = canonical_provider_id(&value.provider_id, catalog);
    let endpoint_id = resolve_endpoint_id(connection_key, value, &provider_id, &base_url, catalog)?;
    let endpoint_fingerprint = stable_id(
        "endpoint",
        &format!("{provider_id}|{endpoint_id}|{base_url}"),
    );
    let credential_ref = value
        .credential_ref
        .as_deref()
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .map(ToOwned::to_owned);
    let health = credential_ref
        .as_ref()
        .and_then(|reference| credential_health.get(reference))
        .copied()
        .unwrap_or_else(|| {
            let no_auth = catalog
                .endpoints
                .iter()
                .find(|endpoint| endpoint.id == endpoint_id)
                .is_some_and(|endpoint| {
                    endpoint.auth_style == crate::model_catalog::AuthStyle::None
                });
            if no_auth {
                ConnectionHealth::Configured
            } else {
                ConnectionHealth::Missing
            }
        });
    let connection_id = stable_id(
        "connection",
        &format!(
            "{}|{}",
            endpoint_fingerprint,
            credential_ref.as_deref().unwrap_or("anonymous")
        ),
    );
    Ok(ConnectionRecord {
        schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
        id: connection_id,
        revision: source_revision,
        adapter_provider_id: value.provider_id.trim().to_ascii_lowercase(),
        provider_id,
        endpoint_id,
        base_url,
        endpoint_fingerprint,
        credential_ref,
        enabled: true,
        health,
        source: source.clone(),
        source_revision,
    })
}

fn resolve_endpoint_id(
    connection_key: &str,
    value: &ConnectionReferenceV2,
    provider_id: &str,
    base_url: &str,
    catalog: &crate::model_catalog::BuiltinModelCatalog,
) -> Result<String, CoreError> {
    if let Some(requested) = value
        .endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if let Some(endpoint) = catalog
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id.eq_ignore_ascii_case(requested))
        {
            if canonical_provider_id(&endpoint.provider_id, catalog) != provider_id {
                return Err(CoreError::InvalidInput(format!(
                    "Endpoint {requested} does not belong to provider {provider_id}"
                )));
            }
            let endpoint_url = normalize_endpoint_url(Some(&endpoint.base_url_template));
            if !base_url.is_empty() && endpoint_url != base_url {
                return Err(CoreError::InvalidInput(format!(
                    "Endpoint {requested} does not match the configured base URL"
                )));
            }
            return Ok(endpoint.id.clone());
        }
        if requested.contains(":custom-") {
            return Ok(requested.to_string());
        }
        return Err(CoreError::InvalidInput(format!(
            "Unknown built-in endpoint identity {requested}"
        )));
    }

    let surface = connection_surface(connection_key);
    let exact = catalog
        .endpoints
        .iter()
        .filter(|endpoint| {
            endpoint.id.starts_with(&format!("{surface}:"))
                && canonical_provider_id(&endpoint.provider_id, catalog) == provider_id
                && normalize_endpoint_url(Some(&endpoint.base_url_template)) == base_url
        })
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    Ok(crate::model_catalog::resolve_or_derive_endpoint_id(
        surface,
        provider_id,
        (!base_url.is_empty()).then_some(base_url),
    ))
}

fn connection_surface(connection_key: &str) -> &'static str {
    match connection_key.trim().to_ascii_lowercase().as_str() {
        "image_generation" | "image_editing" => "image",
        "speech_to_text" => "speech_to_text",
        "text_to_speech" => "text_to_speech",
        "embedding" => "embedding",
        _ => "text",
    }
}

fn scope_key(scope: &SettingsScopeV2) -> String {
    format!(
        "{}:{}",
        scope.kind.as_str(),
        scope.id.as_deref().unwrap_or("")
    )
}

struct BindingResolutionContext<'a> {
    connections: &'a BTreeMap<String, ConnectionRecord>,
    aliases: &'a HashMap<String, String>,
    descriptors: &'a [ModelDescriptor],
    definitions_by_id: &'a HashMap<String, ModelDefinitionRecord>,
    target_by_key: &'a mut BTreeMap<(String, String), ModelTargetRecord>,
    definition_by_target: &'a mut HashMap<String, Option<ModelDefinitionRecord>>,
}

fn resolve_binding(
    context: &mut BindingResolutionContext<'_>,
    capability_id: &str,
    binding: &CapabilityBindingV2,
    source: &SettingsScopeV2,
    source_revision: u64,
) -> Result<ResolvedCapabilityRoute, CoreError> {
    let mut seen = BTreeSet::new();
    let mut resolve = |reference: &ModelReferenceV2| {
        let connection = select_connection(reference, context.connections, context.aliases)?;
        let definition = find_model_definition(reference, &connection, context.descriptors)
            .map(model_definition)
            .transpose()?;
        let target_key = (
            connection.id.clone(),
            normalize_model_id(&reference.model_id),
        );
        let explicit_target = model_target(
            &connection,
            &reference.model_id,
            definition.as_ref(),
            source,
            source_revision,
            true,
        );
        let stored_target = context
            .target_by_key
            .entry(target_key)
            .or_insert_with(|| explicit_target.clone());
        // A selected binding plus a configured Connection and registered
        // runtime adapter is callable evidence. It is stronger than catalog
        // discovery, so only this explicit path may advance a Known target.
        if stored_target.availability == TargetAvailability::Unknown
            && explicit_target.availability == TargetAvailability::Callable
        {
            *stored_target = explicit_target;
        }
        let target = stored_target.clone();
        if reference
            .target_id
            .as_deref()
            .is_some_and(|expected| expected != target.id)
        {
            return Err(CoreError::Conflict(format!(
                "Model reference expected target {} but resolved {}",
                reference.target_id.as_deref().unwrap_or_default(),
                target.id
            )));
        }
        if reference
            .target_revision
            .is_some_and(|expected| expected != target.revision)
        {
            return Err(CoreError::Conflict(format!(
                "Model target {} revision changed from {} to {}",
                target.id,
                reference.target_revision.unwrap_or_default(),
                target.revision
            )));
        }
        if !seen.insert(target.id.clone()) {
            return Err(CoreError::InvalidInput(format!(
                "Capability {capability_id} repeats model target {}",
                target.id
            )));
        }
        let definition = definition.or_else(|| {
            target
                .model_definition_id
                .as_ref()
                .and_then(|id| context.definitions_by_id.get(id).cloned())
        });
        context
            .definition_by_target
            .insert(target.id.clone(), definition.clone());
        let eligibility =
            target_eligibility(capability_id, &connection, &target, definition.as_ref());
        Ok(ResolvedCapabilityRouteTarget {
            target,
            connection,
            definition,
            eligibility,
        })
    };

    let mut primary = binding.primary.as_ref().map(&mut resolve).transpose()?;
    let mut fallbacks = binding
        .fallbacks
        .iter()
        .map(&mut resolve)
        .collect::<Result<Vec<_>, _>>()?;
    for candidate in primary.iter_mut().chain(fallbacks.iter_mut()) {
        apply_intrinsic_binding_constraints(candidate, &binding.constraints);
    }
    if let Some(primary) = primary.as_ref() {
        for fallback in &mut fallbacks {
            apply_fallback_boundary_constraints(primary, fallback, binding);
        }
    }
    Ok(ResolvedCapabilityRoute {
        binding_id: stable_id("binding", &format!("{capability_id}|{}", scope_key(source))),
        binding_revision: source_revision,
        capability_id: capability_id.to_string(),
        source: source.clone(),
        source_revision,
        primary,
        fallbacks,
        fallback_mode: binding.fallback_mode,
        constraints: binding.constraints.clone(),
    })
}

fn apply_fallback_boundary_constraints(
    primary: &ResolvedCapabilityRouteTarget,
    fallback: &mut ResolvedCapabilityRouteTarget,
    binding: &CapabilityBindingV2,
) {
    if binding.constraints.require_same_connection
        && fallback.connection.id != primary.connection.id
    {
        mark_ineligible(fallback, "cross_connection_fallback_requires_consent");
    } else if !binding.constraints.allow_cross_provider
        && fallback.connection.provider_id != primary.connection.provider_id
    {
        mark_ineligible(fallback, "cross_provider_fallback_requires_consent");
    }
    if fallback.eligibility.eligible
        && !binding.constraints.allow_cross_region
        && fallback.connection.id != primary.connection.id
    {
        let primary_regions = candidate_regions(primary);
        let fallback_regions = candidate_regions(fallback);
        if primary_regions.is_empty()
            || fallback_regions.is_empty()
            || primary_regions.is_disjoint(&fallback_regions)
        {
            mark_ineligible(fallback, "cross_region_fallback_requires_consent");
        }
    }
    if fallback.eligibility.eligible
        && fallback.connection.id != primary.connection.id
        && binding.fallback_mode == CapabilityFallbackModeV2::Automatic
        && binding.constraints.data_classes.iter().any(|class| {
            matches!(
                class.trim().to_ascii_lowercase().as_str(),
                "confidential" | "restricted"
            )
        })
    {
        mark_ineligible(fallback, "sensitive_data_cross_connection_requires_consent");
    }
    if fallback.eligibility.eligible
        && fallback.connection.id != primary.connection.id
        && is_local_connection(&fallback.connection) != is_local_connection(&primary.connection)
    {
        mark_ineligible(fallback, "local_cloud_boundary_requires_consent");
    }
}

fn apply_intrinsic_binding_constraints(
    candidate: &mut ResolvedCapabilityRouteTarget,
    constraints: &CapabilityBindingConstraintsV2,
) {
    if constraints.requires_streaming
        && crate::llm::provider_uses_non_streaming_fallback(
            provider_type_for_parts(
                &candidate.connection.adapter_provider_id,
                (!candidate.connection.base_url.is_empty())
                    .then_some(candidate.connection.base_url.as_str()),
            ),
            &candidate.target.upstream_model_id,
        )
    {
        mark_ineligible(candidate, "streaming_required_but_unsupported");
    }

    if !constraints.allowed_regions.is_empty() {
        let allowed = constraints
            .allowed_regions
            .iter()
            .map(|region| region.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let candidate_regions = candidate_regions(candidate);
        if candidate_regions.is_empty() || candidate_regions.is_disjoint(&allowed) {
            mark_ineligible(candidate, "region_not_allowed");
        }
    }

    if let Some(maximum) = constraints.max_cost_class.as_deref() {
        let actual = candidate
            .definition
            .as_ref()
            .and_then(|definition| definition.descriptor.pricing_ref.as_deref())
            .and_then(cost_class_from_pricing_ref);
        if actual.is_none_or(|actual| cost_class_rank(actual) > cost_class_rank(maximum)) {
            mark_ineligible(candidate, "cost_class_unverified_or_exceeded");
        }
    }
}

fn candidate_regions(candidate: &ResolvedCapabilityRouteTarget) -> BTreeSet<String> {
    candidate
        .definition
        .as_ref()
        .into_iter()
        .flat_map(|definition| definition.descriptor.regions.iter())
        .map(|region| region.trim().to_ascii_lowercase())
        .filter(|region| !region.is_empty())
        .collect()
}

fn cost_class_from_pricing_ref(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase();
    ["free", "low", "medium", "high"]
        .into_iter()
        .find(|class| normalized == *class || normalized.starts_with(&format!("{class}:")))
}

fn cost_class_rank(value: &str) -> u8 {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => 0,
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        _ => u8::MAX,
    }
}

fn is_local_connection(connection: &ConnectionRecord) -> bool {
    matches!(
        connection.adapter_provider_id.as_str(),
        "ollama" | "lm_studio" | "lmstudio"
    ) || connection.base_url.starts_with("http://localhost")
        || connection.base_url.starts_with("http://127.")
        || connection.base_url.starts_with("http://[::1]")
}

fn mark_ineligible(candidate: &mut ResolvedCapabilityRouteTarget, reason: &str) {
    candidate.eligibility.eligible = false;
    if !candidate
        .eligibility
        .reason_codes
        .iter()
        .any(|existing| existing == reason)
    {
        candidate.eligibility.reason_codes.push(reason.to_string());
    }
}

fn select_connection(
    reference: &ModelReferenceV2,
    connections: &BTreeMap<String, ConnectionRecord>,
    aliases: &HashMap<String, String>,
) -> Result<ConnectionRecord, CoreError> {
    if let Some(id) = reference.connection_id.as_deref() {
        let canonical_id = aliases.get(id).map(String::as_str).unwrap_or(id);
        let connection = connections.get(canonical_id).ok_or_else(|| {
            CoreError::InvalidInput(format!("Model target references unknown connection {id}"))
        })?;
        ensure_reference_matches_connection(reference, connection)?;
        return Ok(connection.clone());
    }
    let provider = normalize_provider_id(&reference.provider_id);
    let endpoint = reference.endpoint_id.as_deref().map(str::trim);
    let matches = connections
        .values()
        .filter(|connection| {
            normalize_provider_id(&connection.provider_id) == provider
                && endpoint
                    .is_none_or(|endpoint| endpoint.eq_ignore_ascii_case(&connection.endpoint_id))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(CoreError::InvalidInput(format!(
            "Model {} resolves to {} connections; select a stable connectionId",
            reference.model_id,
            matches.len()
        )));
    }
    Ok(matches[0].clone())
}

fn ensure_reference_matches_connection(
    reference: &ModelReferenceV2,
    connection: &ConnectionRecord,
) -> Result<(), CoreError> {
    if normalize_provider_id(&reference.provider_id)
        != normalize_provider_id(&connection.provider_id)
    {
        return Err(CoreError::InvalidInput(format!(
            "Model target provider {} does not match connection {}",
            reference.provider_id, connection.id
        )));
    }
    if reference
        .endpoint_id
        .as_deref()
        .is_some_and(|endpoint| !endpoint.eq_ignore_ascii_case(&connection.endpoint_id))
    {
        return Err(CoreError::InvalidInput(format!(
            "Model target endpoint does not match connection {}",
            connection.id
        )));
    }
    Ok(())
}

fn find_model_definition<'a>(
    reference: &ModelReferenceV2,
    connection: &ConnectionRecord,
    descriptors: &'a [ModelDescriptor],
) -> Option<&'a ModelDescriptor> {
    descriptors.iter().find(|descriptor| {
        normalize_provider_id(&descriptor.provider_id)
            == normalize_provider_id(&connection.provider_id)
            && (descriptor.id.eq_ignore_ascii_case(&reference.model_id)
                || descriptor
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(&reference.model_id)))
            && (descriptor
                .endpoint_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&connection.endpoint_id))
                || connection.endpoint_id.contains(":custom-"))
    })
}

fn target_eligibility(
    capability_id: &str,
    connection: &ConnectionRecord,
    target: &ModelTargetRecord,
    definition: Option<&ModelDefinitionRecord>,
) -> CapabilityEligibility {
    let mut reasons = Vec::new();
    if !connection.enabled {
        reasons.push("connection_disabled".to_string());
    }
    if matches!(
        connection.health,
        ConnectionHealth::Missing | ConnectionHealth::Invalid | ConnectionHealth::Expired
    ) {
        reasons.push("credential_unavailable".to_string());
    }
    if matches!(
        target.availability,
        TargetAvailability::Unknown
            | TargetAvailability::Unavailable
            | TargetAvailability::Discoverable
    ) {
        reasons.push("target_not_callable".to_string());
    }
    if let Some(definition) = definition {
        if definition.descriptor.lifecycle == ModelLifecycle::Removed {
            reasons.push("model_removed".to_string());
        }
        if !descriptor_supports(
            &definition.descriptor,
            capability_requirement(capability_id),
        ) {
            reasons.push("operation_unsupported".to_string());
        }
    } else {
        reasons.push("descriptor_unverified".to_string());
    }
    CapabilityEligibility {
        eligible: reasons
            .iter()
            .all(|reason| reason == "descriptor_unverified"),
        reason_codes: reasons,
    }
}

fn descriptor_supports(descriptor: &ModelDescriptor, requirement: CapabilityRequirement) -> bool {
    let has_input = |modality| descriptor.input_modalities.contains(&modality);
    let has_output = |modality| descriptor.output_modalities.contains(&modality);
    (!requirement.text_input || has_input(ModelModality::Text))
        && (!requirement.image_input
            || has_input(ModelModality::Image)
            || descriptor.capabilities.vision)
        && (!requirement.audio_input || has_input(ModelModality::Audio))
        && (!requirement.image_output || has_output(ModelModality::Image))
        && (!requirement.audio_output || has_output(ModelModality::Audio))
        && (!requirement.video_output || has_output(ModelModality::Video))
        && (!requirement.embedding_output || has_output(ModelModality::Embedding))
        && (!requirement.reasoning || descriptor.capabilities.reasoning.is_some())
        && (!requirement.async_jobs || descriptor.capabilities.async_jobs)
}

fn model_definition(descriptor: &ModelDescriptor) -> Result<ModelDefinitionRecord, CoreError> {
    let descriptor_json = to_vec(descriptor)?;
    Ok(ModelDefinitionRecord {
        id: stable_id(
            "model",
            &format!(
                "{}|{}",
                normalize_provider_id(&descriptor.provider_id),
                normalize_model_id(&descriptor.id)
            ),
        ),
        revision: 1,
        descriptor_hash: blake3::hash(&descriptor_json).to_hex().to_string(),
        descriptor: descriptor.clone(),
    })
}

fn model_target(
    connection: &ConnectionRecord,
    upstream_model_id: &str,
    definition: Option<&ModelDefinitionRecord>,
    source: &SettingsScopeV2,
    source_revision: u64,
    explicit: bool,
) -> ModelTargetRecord {
    let availability = match definition.map(|value| value.descriptor.product_readiness) {
        Some(ProductReadiness::ProductReady) => TargetAvailability::ProductReady,
        Some(ProductReadiness::Callable) => TargetAvailability::Callable,
        Some(ProductReadiness::Discoverable) => TargetAvailability::Discoverable,
        Some(ProductReadiness::Known)
            if explicit && connection.health == ConnectionHealth::Configured =>
        {
            TargetAvailability::Callable
        }
        Some(ProductReadiness::Known) => TargetAvailability::Unknown,
        None if explicit && connection.health == ConnectionHealth::Configured => {
            TargetAvailability::Callable
        }
        _ => TargetAvailability::Unknown,
    };
    ModelTargetRecord {
        id: stable_id(
            "target",
            &format!(
                "{}|{}",
                connection.id,
                normalize_model_id(upstream_model_id)
            ),
        ),
        revision: source_revision,
        connection_id: connection.id.clone(),
        model_definition_id: definition.map(|value| value.id.clone()),
        upstream_model_id: upstream_model_id.trim().to_string(),
        availability,
        source: source.clone(),
        source_revision,
    }
}

fn same_connection_identity(left: &ConnectionRecord, right: &ConnectionRecord) -> bool {
    left.adapter_provider_id == right.adapter_provider_id
        && left.provider_id == right.provider_id
        && left.endpoint_id == right.endpoint_id
        && left.base_url == right.base_url
        && left.credential_ref == right.credential_ref
}

fn sanitize_endpoint_url(value: Option<&str>) -> Result<String, CoreError> {
    let raw = value.unwrap_or_default().trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    let url = reqwest::Url::parse(raw)
        .map_err(|_| CoreError::InvalidInput("Connection endpoint URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CoreError::InvalidInput(
            "Connection endpoint must use HTTP or HTTPS".to_string(),
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CoreError::InvalidInput(
            "Connection endpoints cannot contain userinfo, query, or fragment data".to_string(),
        ));
    }
    Ok(normalize_endpoint_url(Some(raw)))
}

fn canonical_provider_id(
    provider_or_alias: &str,
    catalog: &crate::model_catalog::BuiltinModelCatalog,
) -> String {
    let normalized = normalize_provider_id(provider_or_alias);
    catalog
        .providers
        .iter()
        .find(|provider| {
            normalize_provider_id(&provider.id) == normalized
                || provider
                    .aliases
                    .iter()
                    .any(|alias| normalize_provider_id(alias) == normalized)
        })
        .map(|provider| provider.id.clone())
        .unwrap_or(normalized)
}

fn normalize_provider_id(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "open_ai" => "openai".to_string(),
        "deep_seek" => "deepseek".to_string(),
        "lm_studio" => "lmstudio".to_string(),
        "qwen" | "dashscope" | "alibaba" => "alibaba_model_studio".to_string(),
        other => other.to_string(),
    }
}

fn normalize_model_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(crate) fn stable_id(namespace: &str, identity: &str) -> String {
    let digest = blake3::hash(identity.as_bytes()).to_hex();
    format!("{namespace}:{}", &digest[..24])
}

pub(crate) fn selected_profile_chain(
    profiles: &[SettingsProfileV2],
    scope: &super::types::RegistryScope,
) -> Vec<SettingsProfileV2> {
    let mut selected = profiles
        .iter()
        .filter(|profile| match profile.scope.kind {
            SettingsScopeKindV2::Application => true,
            SettingsScopeKindV2::Workspace => {
                scope.workspace_id.as_deref() == profile.scope.id.as_deref()
            }
            SettingsScopeKindV2::Agent => scope.agent_id.as_deref() == profile.scope.id.as_deref(),
            SettingsScopeKindV2::Task => scope.task_id.as_deref() == profile.scope.id.as_deref(),
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by_key(|profile| match profile.scope.kind {
        SettingsScopeKindV2::Application => 0,
        SettingsScopeKindV2::Workspace => 1,
        SettingsScopeKindV2::Agent => 2,
        SettingsScopeKindV2::Task => 3,
    });
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_candidate(
        connection_id: &str,
        base_url: &str,
        model: &str,
        regions: &[&str],
        pricing_ref: Option<&str>,
    ) -> ResolvedCapabilityRouteTarget {
        let mut descriptor = ModelDescriptor::new(model, "openai", model);
        descriptor.regions = regions.iter().map(|region| (*region).to_string()).collect();
        descriptor.pricing_ref = pricing_ref.map(str::to_string);
        let definition = model_definition(&descriptor).unwrap();
        let connection = ConnectionRecord {
            schema_version: 1,
            id: connection_id.to_string(),
            revision: 1,
            adapter_provider_id: "open_ai".to_string(),
            provider_id: "openai".to_string(),
            endpoint_id: format!("text:{connection_id}"),
            base_url: base_url.to_string(),
            endpoint_fingerprint: format!("endpoint:{connection_id}"),
            credential_ref: Some(format!("legacy-agent-config:{connection_id}")),
            enabled: true,
            health: ConnectionHealth::Configured,
            source: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Agent,
                id: Some(connection_id.to_string()),
            },
            source_revision: 1,
        };
        let target = model_target(
            &connection,
            model,
            Some(&definition),
            &connection.source,
            1,
            true,
        );
        let eligibility =
            target_eligibility("text_generation", &connection, &target, Some(&definition));
        ResolvedCapabilityRouteTarget {
            target,
            connection,
            definition: Some(definition),
            eligibility,
        }
    }

    #[test]
    fn endpoint_sanitization_rejects_secret_bearing_components() {
        for value in [
            "https://user:secret@example.com/v1",
            "https://example.com/v1?api_key=secret",
            "https://example.com/v1#secret",
            "file:///tmp/provider",
        ] {
            assert!(sanitize_endpoint_url(Some(value)).is_err(), "{value}");
        }
        assert_eq!(
            sanitize_endpoint_url(Some("https://EXAMPLE.com/v1/")).unwrap(),
            "https://example.com/v1"
        );
    }

    #[test]
    fn capability_predicates_keep_operations_separate() {
        let mut image = ModelDescriptor::new("image", "openai", "Image");
        image.output_modalities = vec![ModelModality::Image];
        assert!(descriptor_supports(
            &image,
            capability_requirement("image_generation")
        ));
        assert!(!descriptor_supports(
            &image,
            capability_requirement("text_to_speech")
        ));
    }

    #[test]
    fn configured_targets_do_not_promote_discovery_to_callable() {
        let descriptor = ModelDescriptor::new("known-model", "openai", "Known model");
        let mut descriptor = descriptor;
        descriptor.product_readiness = ProductReadiness::Discoverable;
        let definition = model_definition(&descriptor).unwrap();
        let connection = ConnectionRecord {
            schema_version: 1,
            id: "connection:test".to_string(),
            revision: 1,
            adapter_provider_id: "open_ai".to_string(),
            provider_id: "openai".to_string(),
            endpoint_id: "text:openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            endpoint_fingerprint: "endpoint:test".to_string(),
            credential_ref: Some("legacy-agent-config:test".to_string()),
            enabled: true,
            health: ConnectionHealth::Configured,
            source: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Agent,
                id: Some("test".to_string()),
            },
            source_revision: 1,
        };
        let target = model_target(
            &connection,
            &descriptor.id,
            Some(&definition),
            &connection.source,
            1,
            true,
        );

        assert_eq!(target.availability, TargetAvailability::Discoverable);
        let eligibility =
            target_eligibility("text_generation", &connection, &target, Some(&definition));
        assert!(!eligibility.eligible);
        assert!(eligibility
            .reason_codes
            .contains(&"target_not_callable".to_string()));
    }

    #[test]
    fn intrinsic_constraints_fail_closed_on_region_streaming_and_cost() {
        let mut candidate = route_candidate(
            "primary",
            "https://api.openai.com/v1",
            "gpt-5.5-pro-test",
            &["cn-beijing"],
            Some("high:official"),
        );
        let constraints = CapabilityBindingConstraintsV2 {
            requires_streaming: true,
            allowed_regions: vec!["us-east-1".to_string()],
            max_cost_class: Some("low".to_string()),
            ..CapabilityBindingConstraintsV2::default()
        };

        apply_intrinsic_binding_constraints(&mut candidate, &constraints);

        for reason in [
            "streaming_required_but_unsupported",
            "region_not_allowed",
            "cost_class_unverified_or_exceeded",
        ] {
            assert!(candidate
                .eligibility
                .reason_codes
                .contains(&reason.to_string()));
        }
    }

    #[test]
    fn fallback_constraints_recheck_region_data_and_local_cloud_boundaries() {
        let primary = route_candidate(
            "primary",
            "https://api.openai.com/v1",
            "gpt-4.1",
            &["us-east-1"],
            None,
        );
        let fallback = route_candidate(
            "fallback",
            "https://api.openai.com/v1",
            "gpt-4.1-mini",
            &["cn-beijing"],
            None,
        );
        let binding = |constraints| CapabilityBindingV2 {
            primary: None,
            fallbacks: Vec::new(),
            fallback_mode: CapabilityFallbackModeV2::Automatic,
            constraints,
            options: BTreeMap::new(),
        };

        let mut region_blocked = fallback.clone();
        apply_fallback_boundary_constraints(
            &primary,
            &mut region_blocked,
            &binding(CapabilityBindingConstraintsV2 {
                require_same_connection: false,
                allow_cross_provider: true,
                allow_cross_region: false,
                ..CapabilityBindingConstraintsV2::default()
            }),
        );
        assert!(region_blocked
            .eligibility
            .reason_codes
            .contains(&"cross_region_fallback_requires_consent".to_string()));

        let mut data_blocked = fallback.clone();
        apply_fallback_boundary_constraints(
            &primary,
            &mut data_blocked,
            &binding(CapabilityBindingConstraintsV2 {
                require_same_connection: false,
                allow_cross_provider: true,
                allow_cross_region: true,
                data_classes: vec!["confidential".to_string()],
                ..CapabilityBindingConstraintsV2::default()
            }),
        );
        assert!(data_blocked
            .eligibility
            .reason_codes
            .contains(&"sensitive_data_cross_connection_requires_consent".to_string()));

        let mut local = route_candidate(
            "local",
            "http://localhost:11434/v1",
            "gpt-4.1-mini",
            &["us-east-1"],
            None,
        );
        apply_fallback_boundary_constraints(
            &primary,
            &mut local,
            &binding(CapabilityBindingConstraintsV2 {
                require_same_connection: false,
                allow_cross_provider: true,
                allow_cross_region: true,
                ..CapabilityBindingConstraintsV2::default()
            }),
        );
        assert!(local
            .eligibility
            .reason_codes
            .contains(&"local_cloud_boundary_requires_consent".to_string()));
    }
}
