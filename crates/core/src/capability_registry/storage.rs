use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ProviderConfig;
use crate::model_catalog::{load_builtin_catalog, normalize_endpoint_url};
use crate::provider_registry::provider_type_for_parts;
use crate::settings_schema_v2::{
    resolve_settings_v2, CapabilityFallbackModeV2, SettingsProfileV2, SettingsScopeKindV2,
    SettingsScopeV2,
};

use super::resolver::{build_registry_projection, selected_profile_chain, stable_id};
use super::types::{
    CapabilityRegistryProjection, ConnectionHealth, ConnectionRecord, ModelDefinitionRecord,
    RegistryActivationRecord, RegistryReadMode, RegistryScope, RuntimeCapabilityResolution,
    RuntimeRegistrySnapshot, CAPABILITY_REGISTRY_SCHEMA_VERSION,
};

pub(crate) fn migrate_registry_on_open(
    conn: &mut Connection,
) -> Result<CapabilityRegistryProjection, CoreError> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let projection = sync_registry_in_transaction(&transaction)?;
    transaction.commit()?;
    Ok(projection)
}

pub(crate) fn sync_registry_in_transaction(
    transaction: &Transaction<'_>,
) -> Result<CapabilityRegistryProjection, CoreError> {
    if !settings_v2_active(transaction)? {
        return empty_projection();
    }
    let profiles = read_profiles(transaction)?;
    let health = credential_health(transaction)?;
    let scopes = terminal_scopes(&profiles);
    let mut merged_projection = empty_projection()?;
    let mut merged_capabilities = BTreeMap::new();
    let mut merged_targets = BTreeMap::new();

    transaction.execute("UPDATE provider_connections SET enabled = 0", [])?;
    for scope in scopes {
        let selected = selected_profile_chain(&profiles, &scope);
        let projection = build_registry_projection(&profiles, &selected, &health, Vec::new())?;
        persist_projection(transaction, &projection)?;
        for capability in &projection.capabilities {
            let scope = capability.source.clone();
            let (parity_status, parity) = legacy_shadow_parity(transaction, capability)?;
            let read_mode = if registry_runtime_supported(&capability.capability_id)
                && parity_status == "matched"
                && capability
                    .primary
                    .as_ref()
                    .is_some_and(|candidate| candidate.eligibility.eligible)
            {
                RegistryReadMode::Registry
            } else {
                RegistryReadMode::Legacy
            };
            upsert_activation(
                transaction,
                &capability.capability_id,
                &scope,
                read_mode,
                capability.source_revision,
                parity_status,
                &parity,
            )?;
            merged_capabilities.insert(
                (
                    capability.capability_id.clone(),
                    scope_key(&capability.source),
                ),
                capability.clone(),
            );
        }
        for target in projection.model_targets {
            merged_targets.insert(target.id.clone(), target);
        }
        merged_projection
            .settings_revisions
            .extend(projection.settings_revisions);
        merged_projection.connections = projection.connections;
        merged_projection.model_definitions = projection.model_definitions;
    }
    merged_projection.settings_revisions.sort_by(|left, right| {
        scope_key(&left.scope)
            .cmp(&scope_key(&right.scope))
            .then(left.profile_id.cmp(&right.profile_id))
    });
    merged_projection
        .settings_revisions
        .dedup_by(|left, right| {
            left.profile_id == right.profile_id && left.revision == right.revision
        });
    merged_projection.model_targets = merged_targets.into_values().collect();
    merged_projection.capabilities = merged_capabilities.into_values().collect();
    merged_projection.activations = read_activations(transaction)?;
    persist_builtin_snapshot(transaction, &merged_projection.model_definitions)?;
    Ok(merged_projection)
}

impl Database {
    pub fn capability_registry_projection(
        &self,
        scope: &RegistryScope,
    ) -> Result<CapabilityRegistryProjection, CoreError> {
        let conn = self.conn();
        if !settings_v2_active(&conn)? {
            return empty_projection();
        }
        let profiles = read_profiles(&conn)?;
        let selected = selected_profile_chain(&profiles, scope);
        let health = credential_health(&conn)?;
        let activations = applicable_activations(&conn, scope)?;
        build_registry_projection(&profiles, &selected, &health, activations)
    }

    /// Resolve one activated capability. `None` means the durable read pointer
    /// remains on the legacy runtime and callers must use their compatibility
    /// path. Secret values exist only in the returned `ProviderConfig`.
    pub fn resolve_runtime_capability(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
    ) -> Result<Option<RuntimeCapabilityResolution>, CoreError> {
        let conn = self.conn();
        resolve_current_runtime_capability(&conn, scope, capability_id)
    }

    /// Atomically loads an immutable task pin or resolves the durable registry
    /// binding, materializes its credential, and pins that exact route.
    pub fn resolve_or_pin_task_runtime_capability(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        run_id: &str,
    ) -> Result<Option<RuntimeCapabilityResolution>, CoreError> {
        let mut conn = self.conn();
        let transaction =
            conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT snapshot_json FROM agent_task_registry_snapshots
                 WHERE run_id = ?1 AND capability_id = ?2",
                params![run_id, capability_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let snapshot: RuntimeRegistrySnapshot = serde_json::from_str(&existing)?;
            let resolution = materialize_pinned_resolution(&transaction, snapshot)?;
            transaction.commit()?;
            return Ok(Some(resolution));
        }

        let Some(resolution) =
            resolve_current_runtime_capability(&transaction, scope, capability_id)?
        else {
            transaction.commit()?;
            return Ok(None);
        };
        let snapshot_json = serde_json::to_string(&resolution.snapshot)?;
        let snapshot_hash = blake3::hash(snapshot_json.as_bytes()).to_hex().to_string();
        transaction.execute(
            "INSERT INTO agent_task_registry_snapshots
             (run_id, capability_id, schema_version, snapshot_hash, snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                capability_id,
                CAPABILITY_REGISTRY_SCHEMA_VERSION,
                snapshot_hash,
                snapshot_json,
            ],
        )?;
        transaction.commit()?;
        Ok(Some(resolution))
    }

    pub fn pin_task_registry_snapshot(
        &self,
        run_id: &str,
        snapshot: &RuntimeRegistrySnapshot,
    ) -> Result<(), CoreError> {
        let snapshot_json = serde_json::to_string(snapshot)?;
        let snapshot_hash = blake3::hash(snapshot_json.as_bytes()).to_hex().to_string();
        let conn = self.conn();
        let existing: Option<String> = conn
            .query_row(
                "SELECT snapshot_hash FROM agent_task_registry_snapshots
                 WHERE run_id = ?1 AND capability_id = ?2",
                params![run_id, &snapshot.capability_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing == snapshot_hash {
                return Ok(());
            }
            return Err(CoreError::Conflict(format!(
                "Agent task run {run_id} already pinned a different registry snapshot"
            )));
        }
        conn.execute(
            "INSERT INTO agent_task_registry_snapshots
             (run_id, capability_id, schema_version, snapshot_hash, snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                &snapshot.capability_id,
                CAPABILITY_REGISTRY_SCHEMA_VERSION,
                snapshot_hash,
                snapshot_json
            ],
        )?;
        Ok(())
    }

    pub fn get_task_registry_snapshot(
        &self,
        run_id: &str,
        capability_id: &str,
    ) -> Result<Option<RuntimeRegistrySnapshot>, CoreError> {
        let conn = self.conn();
        let json: Option<String> = conn
            .query_row(
                "SELECT snapshot_json FROM agent_task_registry_snapshots
                 WHERE run_id = ?1 AND capability_id = ?2",
                params![run_id, capability_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(CoreError::Serialization)
    }

    pub fn set_registry_read_mode(
        &self,
        capability_id: &str,
        scope: &SettingsScopeV2,
        mode: RegistryReadMode,
        expected_revision: u64,
    ) -> Result<RegistryActivationRecord, CoreError> {
        if mode == RegistryReadMode::Registry && !registry_runtime_supported(capability_id) {
            return Err(CoreError::InvalidInput(format!(
                "Capability {capability_id} does not have a registry-backed runtime adapter"
            )));
        }
        let conn = self.conn();
        let current = read_activation(&conn, capability_id, scope)?.ok_or_else(|| {
            CoreError::NotFound(format!(
                "Registry activation {capability_id}:{}",
                scope_key(scope)
            ))
        })?;
        if mode == RegistryReadMode::Registry && current.parity_status != "matched" {
            return Err(CoreError::InvalidInput(format!(
                "Capability {capability_id} cannot activate until shadow parity is matched"
            )));
        }
        if mode == RegistryReadMode::Registry {
            let route = read_persisted_binding(&conn, capability_id, scope)?.ok_or_else(|| {
                CoreError::NotFound(format!(
                    "Persisted capability binding {capability_id}:{}",
                    scope_key(scope)
                ))
            })?;
            if route
                .primary
                .as_ref()
                .is_none_or(|candidate| !candidate.eligibility.eligible)
            {
                return Err(CoreError::InvalidInput(format!(
                    "Capability {capability_id} has no eligible primary target"
                )));
            }
        }
        if current.registry_revision != expected_revision {
            return Err(CoreError::Conflict(format!(
                "Registry activation revision changed from {expected_revision} to {}",
                current.registry_revision
            )));
        }
        let affected = conn.execute(
            "UPDATE registry_activation_state
             SET read_mode = ?4,
                 registry_revision = registry_revision + 1,
                 activated_at = CASE WHEN ?4 = 'registry' THEN datetime('now') ELSE activated_at END,
                 rolled_back_at = CASE WHEN ?4 = 'legacy' THEN datetime('now') ELSE NULL END,
                 updated_at = datetime('now')
             WHERE capability_id = ?1 AND scope_kind = ?2 AND scope_id = ?3
               AND registry_revision = ?5",
            params![
                capability_id,
                scope.kind.as_str(),
                scope.id.as_deref().unwrap_or(""),
                mode.as_str(),
                expected_revision
            ],
        )?;
        if affected != 1 {
            return Err(CoreError::Conflict(format!(
                "Registry activation revision changed from {expected_revision}"
            )));
        }
        read_activation(&conn, capability_id, scope)?.ok_or_else(|| {
            CoreError::Internal("Registry activation disappeared after update".to_string())
        })
    }
}

fn resolve_current_runtime_capability(
    conn: &Connection,
    scope: &RegistryScope,
    capability_id: &str,
) -> Result<Option<RuntimeCapabilityResolution>, CoreError> {
    if !settings_v2_active(conn)? {
        return Ok(None);
    }
    let activation = applicable_activations(conn, scope)?
        .into_iter()
        .filter(|activation| activation.capability_id == capability_id)
        .max_by_key(|activation| settings_scope_rank(activation.scope.kind));
    let Some(activation) = activation else {
        return Ok(None);
    };
    if activation.read_mode != RegistryReadMode::Registry || activation.parity_status != "matched" {
        return Ok(None);
    }
    let route =
        read_persisted_binding(conn, capability_id, &activation.scope)?.ok_or_else(|| {
            CoreError::NotFound(format!(
                "Persisted capability binding {capability_id}:{}",
                scope_key(&activation.scope)
            ))
        })?;
    let activated_binding_revision = activation
        .parity
        .get("bindingRevision")
        .and_then(serde_json::Value::as_u64);
    if activated_binding_revision != Some(route.binding_revision) {
        return Err(CoreError::Conflict(format!(
            "Capability {capability_id} activation does not match binding revision {}",
            route.binding_revision
        )));
    }
    let profiles = read_profiles(conn)?;
    let selected_profiles = selected_profile_chain(&profiles, scope);
    let settings_revisions = resolve_settings_v2(&selected_profiles)?.revisions;
    let (fallback_index, selected, fallback_reason) = select_runtime_target(&route)?;
    materialize_route_resolution(
        conn,
        &route,
        selected,
        fallback_index,
        fallback_reason,
        settings_revisions,
    )
    .map(Some)
}

fn select_runtime_target(
    route: &super::types::ResolvedCapabilityRoute,
) -> Result<
    (
        usize,
        &super::types::ResolvedCapabilityRouteTarget,
        Option<String>,
    ),
    CoreError,
> {
    if let Some(primary) = route
        .primary
        .as_ref()
        .filter(|candidate| candidate.eligibility.eligible)
    {
        return Ok((0, primary, None));
    }
    match route.fallback_mode {
        CapabilityFallbackModeV2::Disabled => Err(CoreError::InvalidInput(format!(
            "Capability {} primary is unavailable and fallback is disabled",
            route.capability_id
        ))),
        CapabilityFallbackModeV2::Ask => Err(CoreError::InvalidInput(format!(
            "Capability {} requires user consent before fallback",
            route.capability_id
        ))),
        CapabilityFallbackModeV2::Automatic => route
            .fallbacks
            .iter()
            .enumerate()
            .find(|(_, candidate)| candidate.eligibility.eligible)
            .map(|(index, candidate)| {
                (
                    index + 1,
                    candidate,
                    Some("primary_ineligible_automatic_fallback".to_string()),
                )
            })
            .ok_or_else(|| {
                let reasons = route
                    .primary
                    .iter()
                    .chain(route.fallbacks.iter())
                    .flat_map(|candidate| candidate.eligibility.reason_codes.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                CoreError::InvalidInput(format!(
                    "Capability {} has no policy-eligible fallback target: {reasons}",
                    route.capability_id
                ))
            }),
    }
}

fn materialize_route_resolution(
    conn: &Connection,
    route: &super::types::ResolvedCapabilityRoute,
    selected: &super::types::ResolvedCapabilityRouteTarget,
    fallback_index: usize,
    fallback_reason: Option<String>,
    settings_revisions: Vec<crate::settings_schema_v2::SettingsRevisionV2>,
) -> Result<RuntimeCapabilityResolution, CoreError> {
    let (connection_revision, target_revision) = validate_persisted_candidate(conn, selected)?;
    let credential = resolve_connection_credential(conn, &selected.connection)?;
    let provider_config = ProviderConfig {
        provider_type: provider_type_for_parts(
            &selected.connection.adapter_provider_id,
            (!selected.connection.base_url.is_empty())
                .then_some(selected.connection.base_url.as_str()),
        ),
        base_url: (!selected.connection.base_url.is_empty())
            .then(|| selected.connection.base_url.clone()),
        api_key: credential,
        org_id: None,
        timeout_secs: None,
    };
    let snapshot = RuntimeRegistrySnapshot {
        schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
        settings_revisions,
        binding_id: route.binding_id.clone(),
        binding_revision: route.binding_revision,
        capability_id: route.capability_id.clone(),
        target_id: selected.target.id.clone(),
        target_revision,
        connection_id: selected.connection.id.clone(),
        connection_revision,
        model_definition_id: selected.target.model_definition_id.clone(),
        descriptor_hash: selected
            .definition
            .as_ref()
            .map(|definition| definition.descriptor_hash.clone()),
        fallback_index,
        fallback_mode: route.fallback_mode,
        fallback_reason,
        adapter_provider_id: selected.connection.adapter_provider_id.clone(),
        provider_id: selected.connection.provider_id.clone(),
        endpoint_id: selected.connection.endpoint_id.clone(),
        base_url: selected.connection.base_url.clone(),
        credential_ref: selected.connection.credential_ref.clone(),
        model_id: selected.target.upstream_model_id.clone(),
    };
    Ok(RuntimeCapabilityResolution {
        provider_id: selected.connection.adapter_provider_id.clone(),
        endpoint_id: selected.connection.endpoint_id.clone(),
        provider_config,
        model_id: selected.target.upstream_model_id.clone(),
        snapshot,
    })
}

fn validate_persisted_candidate(
    conn: &Connection,
    candidate: &super::types::ResolvedCapabilityRouteTarget,
) -> Result<(u64, u64), CoreError> {
    let connection_revision: Option<u64> = conn
        .query_row(
            "SELECT revision FROM provider_connections
         WHERE id = ?1 AND enabled = 1
           AND adapter_provider_id = ?2 AND provider_id = ?3
           AND endpoint_id = ?4 AND base_url = ?5
           AND credential_ref IS ?6",
            params![
                &candidate.connection.id,
                &candidate.connection.adapter_provider_id,
                &candidate.connection.provider_id,
                &candidate.connection.endpoint_id,
                &candidate.connection.base_url,
                &candidate.connection.credential_ref,
            ],
            |row| row.get(0),
        )
        .optional()?;
    let connection_revision = connection_revision.ok_or_else(|| {
        CoreError::Conflict(format!(
            "Capability target {} references a stale or disabled connection revision",
            candidate.target.id
        ))
    })?;
    let target_revision: Option<u64> = conn
        .query_row(
            "SELECT revision FROM model_targets
         WHERE id = ?1 AND connection_id = ?2 AND upstream_model_id = ?3
           AND availability <> 'unavailable'",
            params![
                &candidate.target.id,
                &candidate.target.connection_id,
                &candidate.target.upstream_model_id,
            ],
            |row| row.get(0),
        )
        .optional()?;
    let target_revision = target_revision.ok_or_else(|| {
        CoreError::Conflict(format!(
            "Capability target {} references a stale model-target revision",
            candidate.target.id
        ))
    })?;
    Ok((connection_revision, target_revision))
}

fn materialize_pinned_resolution(
    conn: &Connection,
    snapshot: RuntimeRegistrySnapshot,
) -> Result<RuntimeCapabilityResolution, CoreError> {
    let frozen_connection = ConnectionRecord {
        schema_version: snapshot.schema_version,
        id: snapshot.connection_id.clone(),
        revision: snapshot.connection_revision,
        adapter_provider_id: snapshot.adapter_provider_id.clone(),
        provider_id: snapshot.provider_id.clone(),
        endpoint_id: snapshot.endpoint_id.clone(),
        base_url: snapshot.base_url.clone(),
        endpoint_fingerprint: String::new(),
        credential_ref: snapshot.credential_ref.clone(),
        enabled: true,
        health: ConnectionHealth::Configured,
        source: SettingsScopeV2 {
            kind: SettingsScopeKindV2::Task,
            id: None,
        },
        source_revision: snapshot.connection_revision,
    };
    let credential = resolve_connection_credential(conn, &frozen_connection)?;
    let provider_config = ProviderConfig {
        provider_type: provider_type_for_parts(
            &snapshot.adapter_provider_id,
            (!snapshot.base_url.is_empty()).then_some(snapshot.base_url.as_str()),
        ),
        base_url: (!snapshot.base_url.is_empty()).then(|| snapshot.base_url.clone()),
        api_key: credential,
        org_id: None,
        timeout_secs: None,
    };
    Ok(RuntimeCapabilityResolution {
        provider_id: snapshot.adapter_provider_id.clone(),
        endpoint_id: snapshot.endpoint_id.clone(),
        provider_config,
        model_id: snapshot.model_id.clone(),
        snapshot,
    })
}

fn settings_scope_rank(kind: SettingsScopeKindV2) -> u8 {
    match kind {
        SettingsScopeKindV2::Application => 0,
        SettingsScopeKindV2::Workspace => 1,
        SettingsScopeKindV2::Agent => 2,
        SettingsScopeKindV2::Task => 3,
    }
}

fn persist_projection(
    transaction: &Transaction<'_>,
    projection: &CapabilityRegistryProjection,
) -> Result<(), CoreError> {
    for connection in &projection.connections {
        transaction.execute(
            "INSERT INTO provider_connections (
                 id, schema_version, revision, provider_id, adapter_provider_id,
                 endpoint_id, base_url,
                 endpoint_fingerprint, credential_ref, enabled, health_status,
                 source_kind, source_id, source_revision, source_fingerprint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                 revision = CASE
                     WHEN provider_connections.source_fingerprint <> excluded.source_fingerprint
                     THEN provider_connections.revision + 1
                     ELSE provider_connections.revision
                 END,
                 provider_id = excluded.provider_id,
                 adapter_provider_id = excluded.adapter_provider_id,
                 endpoint_id = excluded.endpoint_id,
                 base_url = excluded.base_url,
                 endpoint_fingerprint = excluded.endpoint_fingerprint,
                 credential_ref = excluded.credential_ref,
                 enabled = 1,
                 health_status = excluded.health_status,
                 source_kind = excluded.source_kind,
                 source_id = excluded.source_id,
                 source_revision = excluded.source_revision,
                 source_fingerprint = excluded.source_fingerprint,
                 updated_at = datetime('now')",
            params![
                &connection.id,
                connection.schema_version,
                connection.revision,
                &connection.provider_id,
                &connection.adapter_provider_id,
                &connection.endpoint_id,
                &connection.base_url,
                &connection.endpoint_fingerprint,
                &connection.credential_ref,
                connection.health.as_str(),
                connection.source.kind.as_str(),
                connection.source.id.as_deref().unwrap_or(""),
                connection.source_revision,
                connection_source_fingerprint(connection),
            ],
        )?;
    }
    for definition in &projection.model_definitions {
        let descriptor_json = serde_json::to_string(&definition.descriptor)?;
        transaction.execute(
            "INSERT INTO model_definitions (
                 id, schema_version, provider_id, canonical_model_id,
                 descriptor_json, descriptor_hash, source, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)
             ON CONFLICT(id) DO UPDATE SET
                 descriptor_json = excluded.descriptor_json,
                 descriptor_hash = excluded.descriptor_hash,
                 source = excluded.source,
                 revision = CASE
                     WHEN model_definitions.descriptor_hash <> excluded.descriptor_hash
                     THEN model_definitions.revision + 1
                     ELSE model_definitions.revision
                 END,
                 updated_at = datetime('now')",
            params![
                &definition.id,
                definition.descriptor.schema_version,
                &definition.descriptor.provider_id,
                &definition.descriptor.id,
                descriptor_json,
                &definition.descriptor_hash,
                format!("{:?}", definition.descriptor.source).to_ascii_lowercase(),
            ],
        )?;
    }
    for target in &projection.model_targets {
        transaction.execute(
            "INSERT INTO model_targets (
                 id, connection_id, model_definition_id, upstream_model_id,
                 availability, revision, source_kind, source_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 model_definition_id = excluded.model_definition_id,
                 availability = excluded.availability,
                 revision = CASE
                     WHEN model_targets.model_definition_id IS NOT excluded.model_definition_id
                       OR model_targets.availability <> excluded.availability
                     THEN model_targets.revision + 1
                     ELSE model_targets.revision
                 END,
                 source_kind = excluded.source_kind,
                 source_id = excluded.source_id,
                 updated_at = datetime('now')",
            params![
                &target.id,
                &target.connection_id,
                &target.model_definition_id,
                &target.upstream_model_id,
                target.availability.as_str(),
                target.revision,
                target.source.kind.as_str(),
                target.source.id.as_deref().unwrap_or(""),
            ],
        )?;
    }
    for binding in &projection.capabilities {
        let route_json = serde_json::to_string(binding)?;
        let route_hash = blake3::hash(route_json.as_bytes()).to_hex().to_string();
        let fallback_target_ids = serde_json::to_string(
            &binding
                .fallbacks
                .iter()
                .map(|candidate| candidate.target.id.as_str())
                .collect::<Vec<_>>(),
        )?;
        transaction.execute(
            "INSERT INTO capability_bindings (
                 id, capability_id, scope_kind, scope_id, revision,
                 primary_target_id, fallback_target_ids_json, fallback_mode,
                 route_json, route_hash
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(capability_id, scope_kind, scope_id) DO UPDATE SET
                 id = excluded.id,
                 revision = excluded.revision,
                 primary_target_id = excluded.primary_target_id,
                 fallback_target_ids_json = excluded.fallback_target_ids_json,
                 fallback_mode = excluded.fallback_mode,
                 route_json = excluded.route_json,
                 route_hash = excluded.route_hash,
                 updated_at = datetime('now')",
            params![
                &binding.binding_id,
                &binding.capability_id,
                binding.source.kind.as_str(),
                binding.source.id.as_deref().unwrap_or(""),
                binding.binding_revision,
                binding
                    .primary
                    .as_ref()
                    .map(|candidate| &candidate.target.id),
                fallback_target_ids,
                binding.fallback_mode.as_str(),
                route_json,
                route_hash,
            ],
        )?;
    }
    Ok(())
}

fn persist_builtin_snapshot(
    transaction: &Transaction<'_>,
    definitions: &[ModelDefinitionRecord],
) -> Result<(), CoreError> {
    let snapshot_json = serde_json::to_string(definitions)?;
    let content_hash = blake3::hash(snapshot_json.as_bytes()).to_hex().to_string();
    let id = stable_id("catalog", &format!("builtin|{content_hash}"));
    transaction.execute(
        "INSERT OR IGNORE INTO model_catalog_snapshots (
             id, source_id, connection_id, connection_revision, schema_version,
             content_hash, model_count, validation_status, snapshot_json
         ) VALUES (?1, 'builtin', NULL, NULL, 2, ?2, ?3, 'valid', ?4)",
        params![id, content_hash, definitions.len(), snapshot_json],
    )?;
    Ok(())
}

fn upsert_activation(
    transaction: &Transaction<'_>,
    capability_id: &str,
    scope: &SettingsScopeV2,
    mode: RegistryReadMode,
    source_revision: u64,
    parity_status: &str,
    parity: &serde_json::Value,
) -> Result<(), CoreError> {
    transaction.execute(
        "INSERT INTO registry_activation_state (
             capability_id, scope_kind, scope_id, read_mode, registry_revision,
             parity_status, parity_json, activated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                   CASE WHEN ?4 = 'registry' THEN datetime('now') END)
         ON CONFLICT(capability_id, scope_kind, scope_id) DO UPDATE SET
             read_mode = CASE
                 WHEN excluded.read_mode = 'legacy' THEN 'legacy'
                 ELSE registry_activation_state.read_mode
             END,
             parity_status = excluded.parity_status,
             parity_json = excluded.parity_json,
             registry_revision = MAX(registry_activation_state.registry_revision, excluded.registry_revision),
             updated_at = datetime('now')",
        params![
            capability_id,
            scope.kind.as_str(),
            scope.id.as_deref().unwrap_or(""),
            mode.as_str(),
            source_revision,
            parity_status,
            serde_json::to_string(parity)?,
        ],
    )?;
    Ok(())
}

fn registry_runtime_supported(capability_id: &str) -> bool {
    capability_id == "text_generation"
}

fn legacy_shadow_parity(
    conn: &Connection,
    route: &super::types::ResolvedCapabilityRoute,
) -> Result<(&'static str, serde_json::Value), CoreError> {
    let Some(primary) = route.primary.as_ref() else {
        return Ok((
            "blocked",
            serde_json::json!({
                "status": "blocked",
                "reasonCodes": ["missing_primary_target"],
                "bindingId": route.binding_id,
                "bindingRevision": route.binding_revision,
            }),
        ));
    };
    if route.capability_id != "text_generation" || route.source.kind != SettingsScopeKindV2::Agent {
        return Ok((
            "pending",
            serde_json::json!({
                "status": "pending",
                "reasonCodes": ["runtime_not_declared_for_shadow_activation"],
                "bindingId": route.binding_id,
                "bindingRevision": route.binding_revision,
            }),
        ));
    }
    let Some(agent_id) = route.source.id.as_deref() else {
        return Ok((
            "blocked",
            serde_json::json!({
                "status": "blocked",
                "reasonCodes": ["missing_legacy_agent_identity"],
            }),
        ));
    };
    let legacy = conn
        .query_row(
            "SELECT provider, COALESCE(base_url, ''), model,
                    COALESCE(provider_endpoint_id, ''), api_key
             FROM agent_configs WHERE id = ?1",
            [agent_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((provider, base_url, model, endpoint_id, api_key_ciphertext)) = legacy else {
        return Ok((
            "blocked",
            serde_json::json!({
                "status": "blocked",
                "reasonCodes": ["legacy_agent_missing"],
                "bindingId": route.binding_id,
                "bindingRevision": route.binding_revision,
            }),
        ));
    };
    let provider_matches = normalize_provider(&provider)
        == normalize_provider(&primary.connection.adapter_provider_id);
    let endpoint_matches =
        endpoint_id.is_empty() || endpoint_id.eq_ignore_ascii_case(&primary.connection.endpoint_id);
    let base_url_matches = normalize_endpoint_url(Some(&base_url)) == primary.connection.base_url;
    let model_matches = model.eq_ignore_ascii_case(&primary.target.upstream_model_id);
    let credential_ref_matches = primary.connection.credential_ref.as_deref()
        == Some(format!("legacy-agent-config:{agent_id}").as_str());
    let credential_healthy = crate::crypto::decrypt_api_key(&api_key_ciphertext)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let fallback_contract_matches = route.fallbacks.is_empty();
    let eligible = primary.eligibility.eligible;
    let matched = provider_matches
        && endpoint_matches
        && base_url_matches
        && model_matches
        && credential_ref_matches
        && credential_healthy
        && fallback_contract_matches
        && eligible;
    let status = if matched { "matched" } else { "mismatched" };
    Ok((
        status,
        serde_json::json!({
            "status": status,
            "bindingId": route.binding_id,
            "bindingRevision": route.binding_revision,
            "primaryTargetId": primary.target.id,
            "sourceRevision": route.source_revision,
            "checks": {
                "provider": provider_matches,
                "endpoint": endpoint_matches,
                "baseUrl": base_url_matches,
                "credentialReference": credential_ref_matches,
                "credentialHealth": credential_healthy,
                "model": model_matches,
                "advancedDefaultsPreserved": true,
                "fallbackContract": fallback_contract_matches,
                "runtimeEligibility": eligible,
            },
        }),
    ))
}

fn read_profiles(conn: &Connection) -> Result<Vec<SettingsProfileV2>, CoreError> {
    let mut statement = conn.prepare(
        "SELECT document_json FROM settings_profiles_v2 ORDER BY scope_kind, scope_id, id",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut profiles = Vec::new();
    for row in rows {
        let profile: SettingsProfileV2 = serde_json::from_str(&row?)?;
        profile.validate()?;
        profiles.push(profile);
    }
    Ok(profiles)
}

fn read_activations(conn: &Connection) -> Result<Vec<RegistryActivationRecord>, CoreError> {
    let mut statement = conn.prepare(
        "SELECT capability_id, scope_kind, scope_id, read_mode, registry_revision,
                parity_status, parity_json, activated_at, rolled_back_at
         FROM registry_activation_state
         ORDER BY scope_kind, scope_id, capability_id",
    )?;
    let rows = statement.query_map([], activation_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::Database)
}

fn applicable_activations(
    conn: &Connection,
    scope: &RegistryScope,
) -> Result<Vec<RegistryActivationRecord>, CoreError> {
    Ok(read_activations(conn)?
        .into_iter()
        .filter(|activation| match activation.scope.kind {
            SettingsScopeKindV2::Application => true,
            SettingsScopeKindV2::Workspace => {
                scope.workspace_id.as_deref() == activation.scope.id.as_deref()
            }
            SettingsScopeKindV2::Agent => {
                scope.agent_id.as_deref() == activation.scope.id.as_deref()
            }
            SettingsScopeKindV2::Task => scope.task_id.as_deref() == activation.scope.id.as_deref(),
        })
        .collect())
}

fn read_activation(
    conn: &Connection,
    capability_id: &str,
    scope: &SettingsScopeV2,
) -> Result<Option<RegistryActivationRecord>, CoreError> {
    conn.query_row(
        "SELECT capability_id, scope_kind, scope_id, read_mode, registry_revision,
                parity_status, parity_json, activated_at, rolled_back_at
         FROM registry_activation_state
         WHERE capability_id = ?1 AND scope_kind = ?2 AND scope_id = ?3",
        params![
            capability_id,
            scope.kind.as_str(),
            scope.id.as_deref().unwrap_or("")
        ],
        activation_from_row,
    )
    .optional()
    .map_err(CoreError::Database)
}

fn read_persisted_binding(
    conn: &Connection,
    capability_id: &str,
    scope: &SettingsScopeV2,
) -> Result<Option<super::types::ResolvedCapabilityRoute>, CoreError> {
    let route_json: Option<String> = conn
        .query_row(
            "SELECT route_json FROM capability_bindings
             WHERE capability_id = ?1 AND scope_kind = ?2 AND scope_id = ?3",
            params![
                capability_id,
                scope.kind.as_str(),
                scope.id.as_deref().unwrap_or("")
            ],
            |row| row.get(0),
        )
        .optional()?;
    route_json
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(CoreError::Serialization)
}

fn activation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RegistryActivationRecord> {
    let scope_kind: String = row.get(1)?;
    let scope_id: String = row.get(2)?;
    let read_mode: String = row.get(3)?;
    let parity_json: String = row.get(6)?;
    Ok(RegistryActivationRecord {
        capability_id: row.get(0)?,
        scope: SettingsScopeV2 {
            kind: parse_scope_kind(&scope_kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    format!("invalid settings scope {scope_kind}").into(),
                )
            })?,
            id: (!scope_id.is_empty()).then_some(scope_id),
        },
        read_mode: RegistryReadMode::from_str(&read_mode).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("invalid registry read mode {read_mode}").into(),
            )
        })?,
        registry_revision: row.get(4)?,
        parity_status: row.get(5)?,
        parity: serde_json::from_str(&parity_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        activated_at: row.get(7)?,
        rolled_back_at: row.get(8)?,
    })
}

fn credential_health(conn: &Connection) -> Result<HashMap<String, ConnectionHealth>, CoreError> {
    let mut health = HashMap::new();
    {
        let mut statement = conn.prepare("SELECT id, api_key FROM agent_configs ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, ciphertext) = row?;
            let state = match crate::crypto::decrypt_api_key(&ciphertext) {
                Ok(value) if !value.trim().is_empty() => ConnectionHealth::Configured,
                Ok(_) => ConnectionHealth::Missing,
                Err(_) => ConnectionHealth::Invalid,
            };
            health.insert(format!("legacy-agent-config:{id}"), state);
        }
    }
    let app_config_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'app_config')",
        [],
        |row| row.get(0),
    )?;
    let app_config_json = if app_config_exists {
        conn.query_row(
            "SELECT value FROM app_config WHERE key = 'app_config'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    } else {
        None
    };
    if let Some(json) = app_config_json {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            for field in ["imageGeneration", "textToSpeech", "speechToText"] {
                let state = value
                    .get(field)
                    .and_then(|config| config.get("apiKey"))
                    .and_then(serde_json::Value::as_str)
                    .map(crate::crypto::decrypt_api_key)
                    .transpose()
                    .map(|value| {
                        if value
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                        {
                            ConnectionHealth::Configured
                        } else {
                            ConnectionHealth::Missing
                        }
                    })
                    .unwrap_or(ConnectionHealth::Invalid);
                health.insert(format!("legacy-app-config:{field}"), state);
            }
        }
    }
    Ok(health)
}

fn resolve_connection_credential(
    conn: &Connection,
    connection: &ConnectionRecord,
) -> Result<Option<String>, CoreError> {
    let Some(reference) = connection.credential_ref.as_deref() else {
        return Ok(None);
    };
    let (namespace, identifier) = reference.split_once(':').ok_or_else(|| {
        CoreError::InvalidInput("Connection credential reference is not namespaced".to_string())
    })?;
    let (provider, base_url, ciphertext) = match namespace {
        "legacy-agent-config" => conn
            .query_row(
                "SELECT provider, base_url, api_key FROM agent_configs WHERE id = ?1",
                params![identifier],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Credential source {reference}"))
                }
                other => CoreError::Database(other),
            })?,
        "legacy-app-config" => {
            let json: String = conn.query_row(
                "SELECT value FROM app_config WHERE key = 'app_config'",
                [],
                |row| row.get(0),
            )?;
            let value: serde_json::Value = serde_json::from_str(&json)?;
            let config = value
                .get(identifier)
                .ok_or_else(|| CoreError::NotFound(format!("Credential source {reference}")))?;
            (
                config
                    .get("provider")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                config
                    .get("baseUrl")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                config
                    .get("apiKey")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        }
        _ => {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported credential reference namespace {namespace}"
            )))
        }
    };
    let normalized_source_url = normalize_endpoint_url(base_url.as_deref());
    if normalize_provider(&provider) != normalize_provider(&connection.provider_id)
        || normalized_source_url != connection.base_url
    {
        return Err(CoreError::InvalidInput(format!(
            "Credential source {reference} does not match connection endpoint identity"
        )));
    }
    let credential = crate::crypto::decrypt_api_key(&ciphertext)?;
    Ok((!credential.trim().is_empty()).then_some(credential))
}

fn terminal_scopes(profiles: &[SettingsProfileV2]) -> Vec<RegistryScope> {
    let mut scopes = vec![RegistryScope::default()];
    for profile in profiles {
        let mut scope = RegistryScope::default();
        match profile.scope.kind {
            SettingsScopeKindV2::Application => continue,
            SettingsScopeKindV2::Workspace => scope.workspace_id = profile.scope.id.clone(),
            SettingsScopeKindV2::Agent => scope.agent_id = profile.scope.id.clone(),
            SettingsScopeKindV2::Task => scope.task_id = profile.scope.id.clone(),
        }
        scopes.push(scope);
    }
    scopes
}

fn settings_v2_active(conn: &Connection) -> Result<bool, CoreError> {
    let version: u32 = conn.query_row(
        "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(version == 2)
}

fn empty_projection() -> Result<CapabilityRegistryProjection, CoreError> {
    let catalog = load_builtin_catalog().map_err(CoreError::InvalidInput)?;
    let definitions = catalog
        .models
        .into_iter()
        .map(|descriptor| {
            let descriptor_json = serde_json::to_vec(&descriptor)?;
            Ok(ModelDefinitionRecord {
                id: stable_id(
                    "model",
                    &format!(
                        "{}|{}",
                        normalize_provider(&descriptor.provider_id),
                        descriptor.id.trim().to_ascii_lowercase()
                    ),
                ),
                revision: 1,
                descriptor_hash: blake3::hash(&descriptor_json).to_hex().to_string(),
                descriptor,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(CapabilityRegistryProjection {
        schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
        settings_revisions: Vec::new(),
        connections: Vec::new(),
        model_definitions: definitions,
        model_targets: Vec::new(),
        capabilities: Vec::new(),
        activations: Vec::new(),
    })
}

fn connection_source_fingerprint(connection: &ConnectionRecord) -> String {
    stable_id(
        "source",
        &format!(
            "{}|{}|{}|{}|{}|{}",
            connection.provider_id,
            connection.adapter_provider_id,
            connection.endpoint_id,
            connection.base_url,
            connection.credential_ref.as_deref().unwrap_or(""),
            connection.source_revision
        ),
    )
}

fn normalize_provider(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "open_ai" => "openai".to_string(),
        "deep_seek" => "deepseek".to_string(),
        "lm_studio" => "lmstudio".to_string(),
        "qwen" | "dashscope" | "alibaba" => "alibaba_model_studio".to_string(),
        other => other.to_string(),
    }
}

fn scope_key(scope: &SettingsScopeV2) -> String {
    format!(
        "{}:{}",
        scope.kind.as_str(),
        scope.id.as_deref().unwrap_or("")
    )
}

fn parse_scope_kind(value: &str) -> Option<SettingsScopeKindV2> {
    match value {
        "application" => Some(SettingsScopeKindV2::Application),
        "workspace" => Some(SettingsScopeKindV2::Workspace),
        "agent" => Some(SettingsScopeKindV2::Agent),
        "task" => Some(SettingsScopeKindV2::Task),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::SaveAgentConfigInput;
    use crate::settings_schema_v2::{
        CapabilityBindingConstraintsV2, CapabilityBindingV2, CapabilityFallbackModeV2,
        ConnectionReferenceV2, ModelReferenceV2, SettingOverrideV2, SettingsOverridesV2,
        SETTINGS_SCHEMA_VERSION_V2,
    };

    fn agent(provider: &str, base_url: &str, model: &str, key: &str) -> SaveAgentConfigInput {
        SaveAgentConfigInput {
            id: None,
            name: format!("{provider}-{model}"),
            provider: provider.to_string(),
            api_key: key.to_string(),
            base_url: Some(base_url.to_string()),
            model: model.to_string(),
            provider_endpoint_id: None,
            model_id: None,
            temperature: None,
            max_tokens: None,
            context_window: None,
            is_default: true,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            delegation_limits_v2: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
        }
    }

    #[test]
    fn migrated_agent_resolves_through_registry_without_exposing_secret() {
        let db = Database::open_memory().unwrap();
        let saved = db
            .save_agent_config(&agent(
                "open_ai",
                "https://api.openai.com/v1",
                "gpt-4.1",
                "sk-registry-secret",
            ))
            .unwrap();
        let scope = RegistryScope {
            agent_id: Some(saved.id),
            ..RegistryScope::default()
        };
        let projection = db.capability_registry_projection(&scope).unwrap();
        let json = serde_json::to_string(&projection).unwrap();
        assert!(!json.contains("sk-registry-secret"));
        let persisted_registry: String = {
            let conn = db.conn();
            conn.query_row(
                "SELECT COALESCE(group_concat(value, '|'), '') FROM (
                     SELECT credential_ref AS value FROM provider_connections
                     UNION ALL SELECT descriptor_json FROM model_definitions
                     UNION ALL SELECT snapshot_json FROM model_catalog_snapshots
                     UNION ALL SELECT parity_json FROM registry_activation_state
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(!persisted_registry.contains("sk-registry-secret"));
        let runtime = db
            .resolve_runtime_capability(&scope, "text_generation")
            .unwrap()
            .expect("registry activation");
        assert_eq!(runtime.model_id, "gpt-4.1");
        assert_eq!(
            runtime.provider_config.api_key.as_deref(),
            Some("sk-registry-secret")
        );
        assert!(db.rollback_settings_schema_v2().unwrap());
        assert!(db
            .resolve_runtime_capability(&scope, "text_generation")
            .unwrap()
            .is_none());
    }

    #[test]
    fn shadow_mismatch_blocks_registry_activation() {
        let db = Database::open_memory().unwrap();
        let saved = db
            .save_agent_config(&agent(
                "open_ai",
                "https://api.openai.com/v1",
                "gpt-4.1",
                "sk-registry-secret",
            ))
            .unwrap();
        let mut profile = db
            .list_settings_profiles_v2()
            .unwrap()
            .into_iter()
            .find(|profile| {
                profile.scope.kind == SettingsScopeKindV2::Agent
                    && profile.scope.id.as_deref() == Some(saved.id.as_str())
            })
            .expect("migrated agent profile");
        let previous_revision = profile.revision;
        let Some(SettingOverrideV2::Set { value }) = profile.overrides.models.get_mut("text")
        else {
            panic!("migrated text model");
        };
        value.model_id = "gpt-4.1-mini".to_string();
        profile.revision += 1;
        {
            let mut conn = db.conn();
            let transaction = conn.transaction().unwrap();
            transaction
                .execute(
                    "UPDATE settings_profiles_v2
                     SET revision = ?2, document_json = ?3, updated_at = datetime('now')
                     WHERE id = ?1 AND revision = ?4",
                    params![
                        &profile.id,
                        profile.revision,
                        serde_json::to_string(&profile).unwrap(),
                        previous_revision,
                    ],
                )
                .unwrap();
            sync_registry_in_transaction(&transaction).unwrap();
            transaction.commit().unwrap();
        }

        let scope = RegistryScope {
            agent_id: Some(saved.id),
            ..RegistryScope::default()
        };
        let projection = db.capability_registry_projection(&scope).unwrap();
        let activation = projection
            .activations
            .iter()
            .find(|activation| {
                activation.capability_id == "text_generation"
                    && activation.scope.kind == SettingsScopeKindV2::Agent
            })
            .expect("text activation");
        assert_eq!(activation.parity_status, "mismatched");
        assert_eq!(activation.read_mode, RegistryReadMode::Legacy);
        assert!(db
            .resolve_runtime_capability(&scope, "text_generation")
            .unwrap()
            .is_none());
    }

    #[test]
    fn task_snapshot_is_immutable() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&crate::conversation::CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-4.1".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = crate::conversation::ConversationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: crate::llm::Role::User,
            content: "test".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            sort_order: 1,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, None)
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "test",
                Some("open_ai"),
                Some("gpt-4.1"),
            )
            .unwrap();
        let snapshot = RuntimeRegistrySnapshot {
            schema_version: 1,
            settings_revisions: Vec::new(),
            binding_id: "binding:a".to_string(),
            binding_revision: 1,
            capability_id: "text_generation".to_string(),
            target_id: "target:a".to_string(),
            target_revision: 1,
            connection_id: "connection:a".to_string(),
            connection_revision: 1,
            model_definition_id: None,
            descriptor_hash: None,
            fallback_index: 0,
            fallback_mode: CapabilityFallbackModeV2::Disabled,
            fallback_reason: None,
            adapter_provider_id: "open_ai".to_string(),
            provider_id: "openai".to_string(),
            endpoint_id: "text:openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            credential_ref: None,
            model_id: "gpt-4.1".to_string(),
        };
        db.pin_task_registry_snapshot(&run.id, &snapshot).unwrap();
        db.pin_task_registry_snapshot(&run.id, &snapshot).unwrap();
        let mut changed = snapshot.clone();
        changed.target_id = "target:b".to_string();
        assert!(matches!(
            db.pin_task_registry_snapshot(&run.id, &changed),
            Err(CoreError::Conflict(_))
        ));
        let resumed = db
            .resolve_or_pin_task_runtime_capability(
                &RegistryScope::default(),
                "text_generation",
                &run.id,
            )
            .unwrap()
            .expect("existing task pin");
        assert_eq!(resumed.model_id, "gpt-4.1");
        assert_eq!(resumed.snapshot, snapshot);
    }

    #[test]
    fn fallback_cannot_cross_credentials_on_the_same_provider_endpoint() {
        let db = Database::open_memory().unwrap();
        let first = db
            .save_agent_config(&agent(
                "open_ai",
                "https://api.openai.com/v1",
                "gpt-4.1",
                "",
            ))
            .unwrap();
        let mut second_input = agent(
            "open_ai",
            "https://api.openai.com/v1",
            "custom-text-model",
            "sk-second-account",
        );
        second_input.is_default = false;
        let second = db.save_agent_config(&second_input).unwrap();

        let mut overrides = SettingsOverridesV2::default();
        for (alias, saved) in [("primary", &first), ("fallback", &second)] {
            overrides.connections.insert(
                alias.to_string(),
                SettingOverrideV2::Set {
                    value: ConnectionReferenceV2 {
                        id: alias.to_string(),
                        provider_id: "open_ai".to_string(),
                        endpoint_id: None,
                        base_url: Some("https://api.openai.com/v1".to_string()),
                        credential_ref: Some(format!("legacy-agent-config:{}", saved.id)),
                    },
                },
            );
        }
        overrides.capabilities.insert(
            "text_generation".to_string(),
            SettingOverrideV2::Set {
                value: CapabilityBindingV2 {
                    primary: Some(ModelReferenceV2 {
                        connection_id: Some("primary".to_string()),
                        provider_id: "open_ai".to_string(),
                        endpoint_id: None,
                        model_id: "gpt-4.1".to_string(),
                    }),
                    fallbacks: vec![ModelReferenceV2 {
                        connection_id: Some("fallback".to_string()),
                        provider_id: "open_ai".to_string(),
                        endpoint_id: None,
                        model_id: "custom-text-model".to_string(),
                    }],
                    fallback_mode: CapabilityFallbackModeV2::Automatic,
                    constraints: CapabilityBindingConstraintsV2::default(),
                    options: BTreeMap::new(),
                },
            },
        );
        let profile = SettingsProfileV2 {
            schema_version: SETTINGS_SCHEMA_VERSION_V2,
            revision: 1,
            id: "workspace-cross-account-fallback".to_string(),
            name: "Cross-account fallback fixture".to_string(),
            scope: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Workspace,
                id: Some("workspace-a".to_string()),
            },
            preset: None,
            overrides,
            legacy_source: None,
            extensions: BTreeMap::new(),
        };
        db.save_settings_profile_v2(&profile, None).unwrap();

        {
            let conn = db.conn();
            conn.execute(
                "UPDATE registry_activation_state
                 SET read_mode = 'registry', parity_status = 'matched',
                     parity_json = json_object('bindingRevision', 1)
                 WHERE capability_id = 'text_generation'
                   AND scope_kind = 'workspace' AND scope_id = 'workspace-a'",
                [],
            )
            .unwrap();
        }

        let error = db
            .resolve_runtime_capability(
                &RegistryScope {
                    workspace_id: Some("workspace-a".to_string()),
                    ..RegistryScope::default()
                },
                "text_generation",
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("no policy-eligible fallback target"));
    }

    #[test]
    fn automatic_fallback_records_same_connection_selection_reason() {
        let db = Database::open_memory().unwrap();
        let saved = db
            .save_agent_config(&agent(
                "open_ai",
                "https://api.openai.com/v1",
                "gpt-4.1",
                "sk-one-account",
            ))
            .unwrap();
        let credential_ref = format!("legacy-agent-config:{}", saved.id);
        let mut overrides = SettingsOverridesV2::default();
        overrides.connections.insert(
            "primary".to_string(),
            SettingOverrideV2::Set {
                value: ConnectionReferenceV2 {
                    id: "primary".to_string(),
                    provider_id: "open_ai".to_string(),
                    endpoint_id: None,
                    base_url: Some("https://api.openai.com/v1".to_string()),
                    credential_ref: Some(credential_ref),
                },
            },
        );
        overrides.capabilities.insert(
            "text_generation".to_string(),
            SettingOverrideV2::Set {
                value: CapabilityBindingV2 {
                    primary: Some(ModelReferenceV2 {
                        connection_id: Some("primary".to_string()),
                        provider_id: "open_ai".to_string(),
                        endpoint_id: None,
                        model_id: "gpt-4.1".to_string(),
                    }),
                    fallbacks: vec![ModelReferenceV2 {
                        connection_id: Some("primary".to_string()),
                        provider_id: "open_ai".to_string(),
                        endpoint_id: None,
                        model_id: "gpt-4.1-mini".to_string(),
                    }],
                    fallback_mode: CapabilityFallbackModeV2::Automatic,
                    constraints: CapabilityBindingConstraintsV2::default(),
                    options: BTreeMap::new(),
                },
            },
        );
        let profile = SettingsProfileV2 {
            schema_version: SETTINGS_SCHEMA_VERSION_V2,
            revision: 1,
            id: "workspace-same-account-fallback".to_string(),
            name: "Same-account fallback fixture".to_string(),
            scope: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Workspace,
                id: Some("workspace-b".to_string()),
            },
            preset: None,
            overrides,
            legacy_source: None,
            extensions: BTreeMap::new(),
        };
        db.save_settings_profile_v2(&profile, None).unwrap();
        {
            let conn = db.conn();
            let route_json: String = conn
                .query_row(
                    "SELECT route_json FROM capability_bindings
                     WHERE capability_id = 'text_generation'
                       AND scope_kind = 'workspace' AND scope_id = 'workspace-b'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut route: super::super::types::ResolvedCapabilityRoute =
                serde_json::from_str(&route_json).unwrap();
            route.primary.as_mut().unwrap().eligibility.eligible = false;
            route
                .primary
                .as_mut()
                .unwrap()
                .eligibility
                .reason_codes
                .push("simulated_primary_outage".to_string());
            conn.execute(
                "UPDATE capability_bindings SET route_json = ?1
                 WHERE capability_id = 'text_generation'
                   AND scope_kind = 'workspace' AND scope_id = 'workspace-b'",
                [serde_json::to_string(&route).unwrap()],
            )
            .unwrap();
            conn.execute(
                "UPDATE registry_activation_state
                 SET read_mode = 'registry', parity_status = 'matched',
                     parity_json = json_object('bindingRevision', 1)
                 WHERE capability_id = 'text_generation'
                   AND scope_kind = 'workspace' AND scope_id = 'workspace-b'",
                [],
            )
            .unwrap();
        }

        let resolution = db
            .resolve_runtime_capability(
                &RegistryScope {
                    workspace_id: Some("workspace-b".to_string()),
                    ..RegistryScope::default()
                },
                "text_generation",
            )
            .unwrap()
            .expect("automatic fallback resolution");
        assert_eq!(resolution.model_id, "gpt-4.1-mini");
        assert_eq!(resolution.snapshot.fallback_index, 1);
        assert_eq!(
            resolution.snapshot.fallback_reason.as_deref(),
            Some("primary_ineligible_automatic_fallback")
        );
    }

    #[test]
    fn unsupported_runtime_capabilities_remain_on_legacy() {
        let db = Database::open_memory().unwrap();
        let mut input = agent(
            "open_ai",
            "https://api.openai.com/v1",
            "gpt-4.1",
            "sk-registry-secret",
        );
        input.image_generation_model = Some("dall-e-3".to_string());
        let saved = db.save_agent_config(&input).unwrap();
        let scope = RegistryScope {
            agent_id: Some(saved.id),
            ..RegistryScope::default()
        };
        let projection = db.capability_registry_projection(&scope).unwrap();
        let activation = projection
            .activations
            .iter()
            .find(|activation| activation.capability_id == "image_generation")
            .expect("image generation activation");

        assert_eq!(activation.read_mode, RegistryReadMode::Legacy);
        assert!(matches!(
            db.set_registry_read_mode(
                "image_generation",
                &activation.scope,
                RegistryReadMode::Registry,
                activation.registry_revision,
            ),
            Err(CoreError::InvalidInput(_))
        ));
    }
}
