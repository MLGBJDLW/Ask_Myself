use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ProviderConfig;
use crate::model_catalog::{load_builtin_catalog, normalize_endpoint_url};
use crate::provider_registry::provider_type_for_parts;
use crate::settings_schema_v2::{SettingsProfileV2, SettingsScopeKindV2, SettingsScopeV2};

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
            let parity = serde_json::json!({
                "status": "matched",
                "primaryTargetId": capability.primary.as_ref().map(|value| &value.target.id),
                "fallbackTargetIds": capability.fallbacks.iter().map(|value| &value.target.id).collect::<Vec<_>>(),
                "sourceRevision": capability.source_revision,
            });
            let read_mode = if registry_runtime_supported(&capability.capability_id) {
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
                "matched",
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
        let projection = self.capability_registry_projection(scope)?;
        let Some(route) = projection
            .capabilities
            .iter()
            .find(|route| route.capability_id == capability_id)
        else {
            return Ok(None);
        };
        let activation = projection.activations.iter().find(|activation| {
            activation.capability_id == capability_id && activation.scope == route.source
        });
        if activation.is_none_or(|activation| activation.read_mode != RegistryReadMode::Registry) {
            return Ok(None);
        }
        let candidates = route
            .primary
            .iter()
            .chain(route.fallbacks.iter())
            .collect::<Vec<_>>();
        let primary_boundary = candidates
            .first()
            .map(|candidate| candidate.connection.id.as_str());
        let (fallback_index, selected) = candidates
            .iter()
            .enumerate()
            .find(|(index, candidate)| {
                candidate.eligibility.eligible
                    && (*index == 0
                        || primary_boundary
                            .is_some_and(|connection_id| connection_id == candidate.connection.id))
            })
            .ok_or_else(|| {
                let reasons = candidates
                    .iter()
                    .flat_map(|candidate| candidate.eligibility.reason_codes.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                CoreError::InvalidInput(format!(
                    "Capability {capability_id} has no eligible same-boundary target: {reasons}"
                ))
            })?;
        let credential = {
            let conn = self.conn();
            resolve_connection_credential(&conn, &selected.connection)?
        };
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
            settings_revisions: projection.settings_revisions,
            capability_id: capability_id.to_string(),
            target_id: selected.target.id.clone(),
            target_revision: selected.target.revision,
            connection_id: selected.connection.id.clone(),
            connection_revision: selected.connection.revision,
            model_definition_id: selected.target.model_definition_id.clone(),
            descriptor_hash: selected
                .definition
                .as_ref()
                .map(|definition| definition.descriptor_hash.clone()),
            fallback_index,
        };
        Ok(Some(RuntimeCapabilityResolution {
            provider_id: selected.connection.adapter_provider_id.clone(),
            endpoint_id: selected.connection.endpoint_id.clone(),
            provider_config,
            model_id: selected.target.upstream_model_id.clone(),
            snapshot,
        }))
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
        CapabilityBindingV2, ConnectionReferenceV2, ModelReferenceV2, SettingOverrideV2,
        SettingsOverridesV2, SETTINGS_SCHEMA_VERSION_V2,
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
            capability_id: "text_generation".to_string(),
            target_id: "target:a".to_string(),
            target_revision: 1,
            connection_id: "connection:a".to_string(),
            connection_revision: 1,
            model_definition_id: None,
            descriptor_hash: None,
            fallback_index: 0,
        };
        db.pin_task_registry_snapshot(&run.id, &snapshot).unwrap();
        db.pin_task_registry_snapshot(&run.id, &snapshot).unwrap();
        let mut changed = snapshot.clone();
        changed.target_id = "target:b".to_string();
        assert!(matches!(
            db.pin_task_registry_snapshot(&run.id, &changed),
            Err(CoreError::Conflict(_))
        ));
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

        let error = db
            .resolve_runtime_capability(
                &RegistryScope {
                    workspace_id: Some("workspace-a".to_string()),
                    ..RegistryScope::default()
                },
                "text_generation",
            )
            .unwrap_err();
        assert!(error.to_string().contains("same-boundary target"));
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
