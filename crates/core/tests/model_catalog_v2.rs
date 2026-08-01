use nexa_core::model_catalog::{
    load_builtin_catalog, merge_catalog, resolve_builtin_endpoint_id, resolve_saved_selection,
    select_implicit_default, AuthStyle, CapabilityProbeResult, CatalogCacheKey, CatalogMergeInput,
    CredentialKind, DiscoveredModel, DiscoveryStrategy, EndpointRegistry, EndpointTransport,
    HealthProbe, ModelAccess, ModelDescriptor, ModelLifecycle, ProductReadiness,
    ProviderDescriptor, ProviderEndpoint, SavedModelSelection, SelectionResolutionKind,
};

fn descriptor(id: &str) -> ModelDescriptor {
    ModelDescriptor::new(id, "provider", id)
}

#[test]
fn implicit_defaults_require_active_public_product_ready_models() {
    let mut preview = descriptor("preview");
    preview.lifecycle = ModelLifecycle::Preview;
    preview.product_readiness = ProductReadiness::ProductReady;
    preview.recommended = true;

    let mut gated = descriptor("gated");
    gated.lifecycle = ModelLifecycle::Gated;
    gated.product_readiness = ProductReadiness::ProductReady;
    gated.recommended = true;

    let mut unprobed = descriptor("unprobed");
    unprobed.product_readiness = ProductReadiness::Known;
    unprobed.recommended = true;

    let mut ready = descriptor("ready");
    ready.product_readiness = ProductReadiness::ProductReady;

    let models = [preview, gated, unprobed, ready];
    let selected =
        select_implicit_default(&models).expect("one product-ready model should remain eligible");
    assert_eq!(selected.id, "ready");
}

#[test]
fn removed_tombstones_override_live_discovery_and_stale_entries() {
    let mut removed = descriptor("retired-model");
    removed.lifecycle = ModelLifecycle::Removed;
    removed.replacement_model_id = Some("replacement-model".into());

    let snapshot = merge_catalog(CatalogMergeInput {
        provider_id: "provider",
        endpoint_id: "provider-main",
        curated: &[removed],
        discovered: Some(&[
            DiscoveredModel::new("retired-model", "provider-main", "us-east"),
            DiscoveredModel::new("new-model", "provider-main", "us-east"),
        ]),
        probes: &[],
        refreshed_at: "2026-08-01T00:00:00Z",
    });

    assert!(snapshot
        .models
        .iter()
        .all(|model| model.id != "retired-model"));
    assert_eq!(snapshot.models[0].id, "new-model");
    assert_eq!(snapshot.tombstones[0].id, "retired-model");
    assert_eq!(snapshot.tombstones[0].endpoint_ids, ["provider-main"]);
}

#[test]
fn passing_probe_promotes_discovered_model_to_callable_without_claiming_product_ready() {
    let discovered = [DiscoveredModel::new(
        "account-model",
        "provider-main",
        "us-east",
    )];
    let probes = [CapabilityProbeResult::passed(
        "account-model",
        "provider-main",
        "2026-08-01T00:00:00Z",
    )];

    let snapshot = merge_catalog(CatalogMergeInput {
        provider_id: "provider",
        endpoint_id: "provider-main",
        curated: &[],
        discovered: Some(&discovered),
        probes: &probes,
        refreshed_at: "2026-08-01T00:00:00Z",
    });

    assert_eq!(
        snapshot.models[0].product_readiness,
        ProductReadiness::Callable
    );
    assert!(!snapshot.models[0].is_implicit_default_eligible());
    assert_eq!(snapshot.models[0].available_to_credential, Some(true));
}

#[test]
fn cache_keys_isolate_endpoint_and_credential_without_storing_secrets() {
    let first = CatalogCacheKey::new("provider", "endpoint-a", "fingerprint-a");
    let second = CatalogCacheKey::new("provider", "endpoint-b", "fingerprint-a");
    let third = CatalogCacheKey::new("provider", "endpoint-a", "fingerprint-b");

    assert_ne!(first, second);
    assert_ne!(first, third);
    assert!(!first.to_string().contains("secret"));
}

#[test]
fn saved_selection_resolves_alias_then_removed_replacement() {
    let mut replacement = descriptor("replacement-model");
    replacement.aliases = vec!["replacement-alias".into()];
    replacement.product_readiness = ProductReadiness::ProductReady;

    let mut removed = descriptor("retired-model");
    removed.aliases = vec!["retired-alias".into()];
    removed.lifecycle = ModelLifecycle::Removed;
    removed.replacement_model_id = Some("replacement-model".into());

    let resolution = resolve_saved_selection(
        &SavedModelSelection::new("provider", None, "retired-alias"),
        &[replacement, removed],
    );

    assert_eq!(resolution.kind, SelectionResolutionKind::Replacement);
    assert_eq!(resolution.model_id, "replacement-model");
    assert!(resolution.requires_user_notice);
}

#[test]
fn endpoint_registry_resolves_provider_alias_and_normalized_base_url() {
    let endpoint = ProviderEndpoint {
        id: "alibaba-cn".into(),
        provider_id: "alibaba_model_studio".into(),
        region: "cn-beijing".into(),
        base_url_template: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
        api_style: "openai_chat".into(),
        transport: EndpointTransport::Http,
        auth_style: AuthStyle::Bearer,
        workspace_required: false,
        discovery_strategy: DiscoveryStrategy::OpenAiModels,
        health_probe: HealthProbe::Models,
    };
    let registry = EndpointRegistry::new(vec![ProviderDescriptor {
        id: "alibaba_model_studio".into(),
        display_name: "Alibaba Cloud Model Studio".into(),
        aliases: vec!["qwen".into(), "dashscope".into()],
        credential_kind: CredentialKind::ApiKey,
        documentation_ref: None,
        endpoints: vec![endpoint],
    }])
    .expect("valid endpoint registry");

    let resolved = registry
        .resolve(
            "qwen",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1/"),
            None,
        )
        .expect("legacy provider alias and trailing slash should resolve");
    assert_eq!(resolved.id, "alibaba-cn");
}

#[test]
fn account_enablement_is_never_an_implicit_default() {
    let mut model = descriptor("restricted");
    model.access = ModelAccess::AccountEnablement;
    model.product_readiness = ProductReadiness::ProductReady;
    model.recommended = true;

    assert!(!model.is_implicit_default_eligible());
}

#[test]
fn builtin_presets_project_every_surface_into_catalog_v2() {
    let catalog = load_builtin_catalog().expect("all built-in preset files should project");

    assert_eq!(catalog.endpoints.len(), 47);
    assert!(catalog.models.iter().all(|model| model.schema_version == 2));
    assert!(catalog
        .models
        .iter()
        .all(|model| !model.endpoint_ids.is_empty()));
    for expected in [
        "text",
        "image",
        "embedding",
        "speech_to_text",
        "text_to_speech",
    ] {
        assert!(
            catalog
                .endpoints
                .iter()
                .any(|endpoint| endpoint.id.starts_with(expected)),
            "missing {expected} endpoint projection"
        );
    }

    let qwen_image = catalog
        .models
        .iter()
        .find(|model| {
            model.id == "qwen-image-3.0-pro"
                && model
                    .endpoint_ids
                    .iter()
                    .any(|id| id == "image:qwen-dashscope-cn")
        })
        .expect("Qwen Image 3.0 should be projected");
    assert_eq!(qwen_image.lifecycle, ModelLifecycle::Preview);
    assert_eq!(qwen_image.access, ModelAccess::Application);
    assert!(!qwen_image.is_implicit_default_eligible());

    let suspicious_embedding = catalog
        .models
        .iter()
        .find(|model| model.id == "qwen3.7-text-embedding")
        .expect("legacy saved embedding id remains resolvable");
    assert_eq!(
        suspicious_embedding.product_readiness,
        ProductReadiness::Known
    );
    assert!(!suspicious_embedding.recommended);

    let recommended_embedding = catalog
        .models
        .iter()
        .find(|model| model.id == "text-embedding-v4")
        .expect("official embedding should be projected");
    assert!(recommended_embedding.recommended);
    assert!(recommended_embedding.is_implicit_default_eligible());
}

#[test]
fn legacy_provider_and_base_url_resolve_to_stable_endpoint_identity() {
    assert_eq!(
        resolve_builtin_endpoint_id(
            "text",
            "qwen",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1/"),
        )
        .as_deref(),
        Some("text:alibaba-model-studio")
    );
    assert_eq!(
        resolve_builtin_endpoint_id("text", "open_ai", None).as_deref(),
        Some("text:openai")
    );
}

#[test]
fn saved_provider_alias_resolves_with_endpoint_scope() {
    let catalog = load_builtin_catalog().expect("built-in catalog should project");
    let resolution = resolve_saved_selection(
        &SavedModelSelection::new(
            "qwen",
            Some("text:alibaba-model-studio".into()),
            "qwen3.7-max",
        ),
        &catalog.models,
    );

    assert_eq!(resolution.kind, SelectionResolutionKind::Unchanged);
    assert_eq!(resolution.provider_id, "alibaba_model_studio");
    assert_eq!(
        resolution.provider_endpoint_id.as_deref(),
        Some("text:alibaba-model-studio")
    );
}

#[test]
fn shared_schema_tracks_rust_wire_names_and_qwen_replacements() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../shared/model-catalog.schema.json"))
            .expect("shared model catalog schema should be valid JSON");
    let lifecycle = schema["$defs"]["modelLifecycle"]["enum"]
        .as_array()
        .expect("lifecycle should be an enum");
    for expected in [
        "active",
        "preview",
        "gated",
        "legacy",
        "deprecated",
        "removed",
    ] {
        assert!(lifecycle.iter().any(|value| value == expected));
    }

    let catalog = load_builtin_catalog().expect("built-in catalog should project");
    let retired = catalog
        .models
        .iter()
        .find(|model| {
            model.id == "qwen3-max-2026-01-23"
                && model
                    .endpoint_ids
                    .iter()
                    .any(|endpoint| endpoint == "text:alibaba-model-studio")
        })
        .expect("the saved Qwen3 Max identity should remain in the catalog");
    assert_eq!(retired.lifecycle, ModelLifecycle::Deprecated);
    assert_eq!(retired.replacement_model_id.as_deref(), Some("qwen3.7-max"));
    assert!(retired.aliases.iter().any(|alias| alias == "qwen3-max"));
}
