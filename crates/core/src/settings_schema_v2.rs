//! Versioned, inheritable settings documents and lossless legacy migration.
//!
//! The V2 document is deliberately a shadow of the current `agent_configs`
//! runtime source. Migration never deletes or rewrites legacy rows, and V2
//! stores only a reference to credentials rather than secret material. This
//! lets later registry work adopt the schema incrementally and makes rollback
//! a metadata operation instead of a lossy reverse transformation.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{db::Database, error::CoreError};

pub const SETTINGS_SCHEMA_VERSION_V2: u32 = 2;
pub const LEGACY_SETTINGS_MIGRATION_KEY: &str = "settings_v1_to_v2";
const LEGACY_AGENT_CONFIG_MANAGER: &str = "legacy_agent_config_v1";
const LEGACY_APP_CONFIG_MANAGER: &str = "legacy_app_config_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsScopeKindV2 {
    Application,
    Workspace,
    Agent,
    Task,
}

impl SettingsScopeKindV2 {
    fn rank(self) -> u8 {
        match self {
            Self::Application => 0,
            Self::Workspace => 1,
            Self::Agent => 2,
            Self::Task => 3,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Workspace => "workspace",
            Self::Agent => "agent",
            Self::Task => "task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsScopeV2 {
    pub kind: SettingsScopeKindV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSelectionV2 {
    pub id: String,
    pub version: u32,
    pub content_hash: String,
}

/// A stored override has only two states. Inheritance is represented by the
/// absence of a key, so "reset to inherit" removes that key. `Clear` is an
/// explicit provider-default/null value and therefore still has provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SettingOverrideV2<T> {
    Set { value: T },
    Clear,
}

impl<T> SettingOverrideV2<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Set { value } => Some(value),
            Self::Clear => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReferenceV2 {
    pub id: String,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReferenceV2 {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityBindingV2 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<ModelReferenceV2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ModelReferenceV2>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevelV2 {
    Allow,
    RequireApproval,
    Deny,
}

impl PermissionLevelV2 {
    fn severity(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::RequireApproval => 1,
            Self::Deny => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRuleV2 {
    pub id: String,
    pub effect: PermissionLevelV2,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsOverridesV2 {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: BTreeMap<String, SettingOverrideV2<ConnectionReferenceV2>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, SettingOverrideV2<ModelReferenceV2>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capabilities: BTreeMap<String, SettingOverrideV2<CapabilityBindingV2>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub permissions: BTreeMap<String, PolicyRuleV2>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub advanced: BTreeMap<String, SettingOverrideV2<Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsSourceV2 {
    pub kind: String,
    pub id: String,
    pub migration_key: String,
    pub source_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsProfileV2 {
    pub schema_version: u32,
    pub revision: u64,
    pub id: String,
    pub name: String,
    pub scope: SettingsScopeV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PresetSelectionV2>,
    #[serde(default)]
    pub overrides: SettingsOverridesV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_source: Option<LegacySettingsSourceV2>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

impl SettingsProfileV2 {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION_V2 {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported settings schema version {}; expected {}",
                self.schema_version, SETTINGS_SCHEMA_VERSION_V2
            )));
        }
        if self.revision == 0 {
            return Err(CoreError::InvalidInput(
                "Settings profile revision must be positive".to_string(),
            ));
        }
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "Settings profile id and name must not be empty".to_string(),
            ));
        }
        match self.scope.kind {
            SettingsScopeKindV2::Application if self.scope.id.is_some() => {
                return Err(CoreError::InvalidInput(
                    "Application settings scope must not have an id".to_string(),
                ));
            }
            SettingsScopeKindV2::Application => {}
            _ if self
                .scope
                .id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty()) =>
            {
                return Err(CoreError::InvalidInput(format!(
                    "{} settings scope requires a non-empty id",
                    self.scope.kind.as_str()
                )));
            }
            _ => {}
        }

        validate_keys("connection", &self.overrides.connections)?;
        validate_keys("model", &self.overrides.models)?;
        validate_keys("capability", &self.overrides.capabilities)?;
        validate_keys("permission", &self.overrides.permissions)?;
        validate_keys("advanced", &self.overrides.advanced)?;
        validate_keys("extension", &self.extensions)?;
        if self.extensions.len() > 64 || serde_json::to_vec(&self.extensions)?.len() > 64 * 1024 {
            return Err(CoreError::InvalidInput(
                "Settings extensions exceed the compatibility limit".to_string(),
            ));
        }
        validate_secret_free_value(
            &serde_json::json!({
                "overrides": &self.overrides,
                "extensions": &self.extensions,
            }),
            "settingsProfile",
        )?;

        for rule in self.overrides.permissions.values() {
            if rule.id.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "Permission rules require a stable id".to_string(),
                ));
            }
        }

        for value in self.overrides.connections.values() {
            if let SettingOverrideV2::Set { value } = value {
                if value.id.trim().is_empty() || value.provider_id.trim().is_empty() {
                    return Err(CoreError::InvalidInput(
                        "Connection references require id and providerId".to_string(),
                    ));
                }
                if let Some(reference) = &value.credential_ref {
                    validate_credential_reference(reference)?;
                }
            }
        }
        if let Some(reference) = self
            .legacy_source
            .as_ref()
            .and_then(|source| source.credential_ref.as_deref())
        {
            validate_credential_reference(reference)?;
        }
        for value in self.overrides.models.values() {
            if let SettingOverrideV2::Set { value } = value {
                validate_model_reference(value)?;
            }
        }
        for value in self.overrides.capabilities.values() {
            if let SettingOverrideV2::Set { value } = value {
                if let Some(primary) = &value.primary {
                    validate_model_reference(primary)?;
                }
                for fallback in &value.fallbacks {
                    validate_model_reference(fallback)?;
                }
            }
        }
        Ok(())
    }
}

fn validate_keys<T>(kind: &str, values: &BTreeMap<String, T>) -> Result<(), CoreError> {
    if values.keys().any(|key| key.trim().is_empty()) {
        return Err(CoreError::InvalidInput(format!(
            "Settings {kind} override keys must not be empty"
        )));
    }
    Ok(())
}

fn validate_model_reference(value: &ModelReferenceV2) -> Result<(), CoreError> {
    if value.provider_id.trim().is_empty() || value.model_id.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "Model references require providerId and modelId".to_string(),
        ));
    }
    Ok(())
}

fn validate_credential_reference(reference: &str) -> Result<(), CoreError> {
    let Some((namespace, identifier)) = reference.split_once(':') else {
        return Err(CoreError::InvalidInput(
            "credentialRef must be a namespaced reference, not secret material".to_string(),
        ));
    };
    if !matches!(namespace, "legacy-agent-config" | "legacy-app-config")
        || identifier.trim().is_empty()
        || identifier.len() > 256
        || identifier.chars().any(char::is_whitespace)
        || identifier.chars().any(char::is_control)
    {
        return Err(CoreError::InvalidInput(
            "credentialRef must identify an existing supported credential store".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSettingV2<T> {
    pub value: Option<T>,
    pub source: SettingsScopeV2,
    pub source_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_origin: Option<PresetSelectionV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedPolicyRuleV2 {
    pub key: String,
    pub rule: PolicyRuleV2,
    pub source: SettingsScopeV2,
    pub source_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_origin: Option<PresetSelectionV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPolicyV2 {
    pub effect: PermissionLevelV2,
    pub matched_rules: Vec<MatchedPolicyRuleV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPermissionGrantV2 {
    pub id: String,
    pub task_id: String,
    pub permission_key: String,
    pub resource_selector: String,
    pub issuer: String,
    pub created_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_ms: Option<i64>,
    pub scope: TaskPermissionGrantScopeV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at_epoch_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPermissionGrantScopeV2 {
    OneShot,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDecisionV2 {
    pub effect: PermissionLevelV2,
    pub matched_rules: Vec<MatchedPolicyRuleV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfied_by_grant_id: Option<String>,
}

/// Apply an exact task grant. Missing policy is denied, and a grant can satisfy
/// RequireApproval but can never relax a Deny ceiling.
pub fn resolve_permission_v2(
    policy: Option<&ResolvedPolicyV2>,
    permission_key: &str,
    resource_selector: &str,
    task_id: &str,
    grant: Option<&TaskPermissionGrantV2>,
    now_epoch_ms: i64,
) -> PermissionDecisionV2 {
    let Some(policy) = policy else {
        return PermissionDecisionV2 {
            effect: PermissionLevelV2::Deny,
            matched_rules: Vec::new(),
            satisfied_by_grant_id: None,
        };
    };
    let valid_grant = grant.filter(|grant| {
        !grant.id.trim().is_empty()
            && !grant.issuer.trim().is_empty()
            && grant.task_id == task_id
            && grant.permission_key == permission_key
            && grant.resource_selector == resource_selector
            && grant.created_at_epoch_ms <= now_epoch_ms
            && grant
                .expires_at_epoch_ms
                .is_none_or(|expires_at| expires_at >= now_epoch_ms)
            && (grant.scope != TaskPermissionGrantScopeV2::OneShot
                || grant.consumed_at_epoch_ms.is_none())
    });
    let satisfied_by_grant_id = if policy.effect == PermissionLevelV2::RequireApproval {
        valid_grant.map(|grant| grant.id.clone())
    } else {
        None
    };
    PermissionDecisionV2 {
        effect: if satisfied_by_grant_id.is_some() {
            PermissionLevelV2::Allow
        } else {
            policy.effect
        },
        matched_rules: policy.matched_rules.clone(),
        satisfied_by_grant_id,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSettingsV2 {
    pub revisions: Vec<SettingsRevisionV2>,
    pub connections: BTreeMap<String, ResolvedSettingV2<ConnectionReferenceV2>>,
    pub models: BTreeMap<String, ResolvedSettingV2<ModelReferenceV2>>,
    pub capabilities: BTreeMap<String, ResolvedSettingV2<CapabilityBindingV2>>,
    pub permissions: BTreeMap<String, ResolvedPolicyV2>,
    pub advanced: BTreeMap<String, ResolvedSettingV2<Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRevisionV2 {
    pub profile_id: String,
    pub scope: SettingsScopeV2,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PresetSelectionV2>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetDefinitionV2 {
    pub id: String,
    pub version: u32,
    pub patch: SettingsOverridesV2,
    pub content_hash: String,
}

impl PresetDefinitionV2 {
    pub fn selection(&self) -> PresetSelectionV2 {
        PresetSelectionV2 {
            id: self.id.clone(),
            version: self.version,
            content_hash: self.content_hash.clone(),
        }
    }
}

/// Versioned built-ins are sparse, credential-free patches. Updating a preset
/// requires a new version rather than mutating an existing selection.
pub fn builtin_settings_presets_v2() -> Vec<PresetDefinitionV2> {
    [
        (
            "chat_only",
            &[
                ("file_read", PermissionLevelV2::Deny),
                ("file_edit", PermissionLevelV2::Deny),
                ("shell", PermissionLevelV2::Deny),
                ("web", PermissionLevelV2::Deny),
                ("external_connectors", PermissionLevelV2::Deny),
                ("desktop_automation", PermissionLevelV2::Deny),
                ("destructive_actions", PermissionLevelV2::Deny),
                ("subagent_delegation", PermissionLevelV2::Deny),
            ][..],
        ),
        (
            "research",
            &[
                ("file_read", PermissionLevelV2::Allow),
                ("file_edit", PermissionLevelV2::Deny),
                ("shell", PermissionLevelV2::RequireApproval),
                ("web", PermissionLevelV2::Allow),
                ("external_connectors", PermissionLevelV2::RequireApproval),
                ("desktop_automation", PermissionLevelV2::Deny),
                ("destructive_actions", PermissionLevelV2::Deny),
                ("subagent_delegation", PermissionLevelV2::Allow),
            ][..],
        ),
        (
            "coding",
            &[
                ("file_read", PermissionLevelV2::Allow),
                ("file_edit", PermissionLevelV2::Allow),
                ("shell", PermissionLevelV2::RequireApproval),
                ("web", PermissionLevelV2::Allow),
                ("external_connectors", PermissionLevelV2::RequireApproval),
                ("desktop_automation", PermissionLevelV2::Deny),
                ("destructive_actions", PermissionLevelV2::RequireApproval),
                ("subagent_delegation", PermissionLevelV2::Allow),
            ][..],
        ),
        (
            "full_agent_safe",
            &[
                ("file_read", PermissionLevelV2::Allow),
                ("file_edit", PermissionLevelV2::Allow),
                ("shell", PermissionLevelV2::RequireApproval),
                ("web", PermissionLevelV2::Allow),
                ("external_connectors", PermissionLevelV2::RequireApproval),
                ("desktop_automation", PermissionLevelV2::RequireApproval),
                ("destructive_actions", PermissionLevelV2::RequireApproval),
                ("subagent_delegation", PermissionLevelV2::Allow),
            ][..],
        ),
        (
            "desktop_operator",
            &[
                ("file_read", PermissionLevelV2::Allow),
                ("file_edit", PermissionLevelV2::RequireApproval),
                ("shell", PermissionLevelV2::RequireApproval),
                ("web", PermissionLevelV2::Allow),
                ("external_connectors", PermissionLevelV2::RequireApproval),
                ("desktop_automation", PermissionLevelV2::RequireApproval),
                ("destructive_actions", PermissionLevelV2::RequireApproval),
                ("subagent_delegation", PermissionLevelV2::Deny),
            ][..],
        ),
    ]
    .into_iter()
    .map(|(id, rules)| {
        let mut patch = SettingsOverridesV2::default();
        for (key, effect) in rules {
            patch.permissions.insert(
                (*key).to_string(),
                PolicyRuleV2 {
                    id: format!("preset:{id}:v1:{key}"),
                    effect: *effect,
                },
            );
        }
        let bytes = serde_json::to_vec(&patch).expect("built-in settings preset serializes");
        let content_hash = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        PresetDefinitionV2 {
            id: id.to_string(),
            version: 1,
            patch,
            content_hash,
        }
    })
    .collect()
}

/// Resolve profiles in Application -> Workspace -> Agent -> Task order while
/// retaining the winning source for every ordinary value and composing
/// permissions through the fail-closed Deny > RequireApproval > Allow lattice.
pub fn resolve_settings_v2(
    profiles: &[SettingsProfileV2],
) -> Result<ResolvedSettingsV2, CoreError> {
    let has_policy = profiles
        .iter()
        .any(|profile| profile.preset.is_some() || !profile.overrides.permissions.is_empty());
    if has_policy
        && profiles
            .first()
            .is_none_or(|profile| profile.scope.kind != SettingsScopeKindV2::Application)
    {
        return Err(CoreError::InvalidInput(
            "Permission resolution requires an Application policy ceiling".to_string(),
        ));
    }
    let mut previous_rank = None;
    let mut resolved = ResolvedSettingsV2::default();
    let presets = builtin_settings_presets_v2();
    let mut application_policy_keys = BTreeSet::new();
    for profile in profiles {
        profile.validate()?;
        let rank = profile.scope.kind.rank();
        if previous_rank.is_some_and(|previous| rank <= previous) {
            return Err(CoreError::InvalidInput(
                "Settings profiles must be unique and ordered Application -> Workspace -> Agent -> Task"
                    .to_string(),
            ));
        }
        previous_rank = Some(rank);
        resolved.revisions.push(SettingsRevisionV2 {
            profile_id: profile.id.clone(),
            scope: profile.scope.clone(),
            revision: profile.revision,
            preset: profile.preset.clone(),
        });
        let preset_patch = if let Some(selection) = &profile.preset {
            let definition = presets.iter().find(|definition| {
                definition.id == selection.id
                    && definition.version == selection.version
                    && definition.content_hash == selection.content_hash
            });
            let Some(definition) = definition else {
                return Err(CoreError::InvalidInput(format!(
                    "Pinned settings preset {} v{} is missing or has a mismatched hash",
                    selection.id, selection.version
                )));
            };
            Some((&definition.patch, selection))
        } else {
            None
        };
        let policy_keys = preset_patch
            .iter()
            .flat_map(|(patch, _)| patch.permissions.keys())
            .chain(profile.overrides.permissions.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        if profile.scope.kind == SettingsScopeKindV2::Application {
            application_policy_keys.extend(policy_keys);
        } else if let Some(missing_key) = policy_keys
            .iter()
            .find(|key| !application_policy_keys.contains(*key))
        {
            return Err(CoreError::InvalidInput(format!(
                "Permission {missing_key} requires an Application policy baseline"
            )));
        }
        if let Some((patch, selection)) = preset_patch {
            apply_profile_overrides(
                &mut resolved,
                patch,
                &profile.scope,
                profile.revision,
                Some(selection),
            );
        }
        apply_profile_overrides(
            &mut resolved,
            &profile.overrides,
            &profile.scope,
            profile.revision,
            None,
        );
    }
    Ok(resolved)
}

fn apply_profile_overrides(
    resolved: &mut ResolvedSettingsV2,
    overrides: &SettingsOverridesV2,
    scope: &SettingsScopeV2,
    revision: u64,
    preset_origin: Option<&PresetSelectionV2>,
) {
    apply_namespace(
        &mut resolved.connections,
        &overrides.connections,
        scope,
        revision,
        preset_origin,
    );
    apply_namespace(
        &mut resolved.models,
        &overrides.models,
        scope,
        revision,
        preset_origin,
    );
    apply_namespace(
        &mut resolved.capabilities,
        &overrides.capabilities,
        scope,
        revision,
        preset_origin,
    );
    apply_namespace(
        &mut resolved.advanced,
        &overrides.advanced,
        scope,
        revision,
        preset_origin,
    );
    for (key, rule) in &overrides.permissions {
        let matched = MatchedPolicyRuleV2 {
            key: key.clone(),
            rule: rule.clone(),
            source: scope.clone(),
            source_revision: revision,
            preset_origin: preset_origin.cloned(),
        };
        let policy = resolved
            .permissions
            .entry(key.clone())
            .or_insert_with(|| ResolvedPolicyV2 {
                effect: PermissionLevelV2::Allow,
                matched_rules: Vec::new(),
            });
        if rule.effect.severity() > policy.effect.severity() {
            policy.effect = rule.effect;
        }
        policy.matched_rules.push(matched);
    }
}

fn apply_namespace<T: Clone>(
    target: &mut BTreeMap<String, ResolvedSettingV2<T>>,
    overrides: &BTreeMap<String, SettingOverrideV2<T>>,
    source: &SettingsScopeV2,
    source_revision: u64,
    preset_origin: Option<&PresetSelectionV2>,
) {
    for (key, value) in overrides {
        target.insert(
            key.clone(),
            ResolvedSettingV2 {
                value: value.clone().into_option(),
                source: source.clone(),
                source_revision,
                preset_origin: preset_origin.cloned(),
            },
        );
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsMigrationReportV2 {
    pub migrated: usize,
    pub unchanged: usize,
    pub skipped_rolled_back: usize,
    pub removed_orphans: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSchemaStateV2 {
    pub active_version: u32,
    pub migration_id: Option<String>,
    pub activated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCredentialReference {
    storage: String,
    source_id: String,
    configured: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyAgentConfigSnapshot {
    id: String,
    name: String,
    provider: String,
    base_url: Option<String>,
    model: String,
    provider_endpoint_id: Option<String>,
    model_id: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    context_window: Option<i64>,
    is_default: bool,
    reasoning_enabled: Option<bool>,
    thinking_budget: Option<i64>,
    reasoning_effort: Option<String>,
    max_iterations: Option<i64>,
    summarization_model: Option<String>,
    summarization_provider: Option<String>,
    image_generation_model: Option<String>,
    subagent_allowed_tools_json: Option<String>,
    subagent_allowed_skill_ids_json: Option<String>,
    subagent_max_parallel: Option<i64>,
    subagent_max_calls_per_turn: Option<i64>,
    subagent_token_budget: Option<i64>,
    delegation_limits_v2_json: Option<String>,
    tool_timeout_secs: Option<i64>,
    agent_timeout_secs: Option<i64>,
    credential: LegacyCredentialReference,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLegacyAgentConfigSnapshot {
    config: LegacyAgentConfigSnapshot,
    api_key_ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLegacyAppConfigSnapshot {
    key: String,
    value: String,
    updated_at: String,
}

impl RawLegacyAppConfigSnapshot {
    fn source_hash(&self) -> Result<String, CoreError> {
        sha256_json(self)
    }

    fn projection_fingerprint(&self) -> Result<String, CoreError> {
        sha256_json(&sanitize_app_config_value(serde_json::from_str(
            &self.value,
        )?))
    }

    fn profile(&self, fingerprint: &str, revision: u64) -> Result<SettingsProfileV2, CoreError> {
        let raw: Value = serde_json::from_str(&self.value)?;
        let sanitized = sanitize_app_config_value(raw);
        let mut overrides = SettingsOverridesV2::default();
        for (field, capability) in [
            ("imageGeneration", "image_generation"),
            ("textToSpeech", "text_to_speech"),
            ("speechToText", "speech_to_text"),
        ] {
            let Some(config) = sanitized.get(field).and_then(Value::as_object) else {
                continue;
            };
            let Some(provider_id) = config
                .get("provider")
                .and_then(Value::as_str)
                .and_then(|value| non_empty(Some(value)))
            else {
                continue;
            };
            let Some(model_id) = config
                .get("model")
                .and_then(Value::as_str)
                .and_then(|value| non_empty(Some(value)))
            else {
                continue;
            };
            let credential_ref = format!("legacy-app-config:{field}");
            overrides.connections.insert(
                capability.to_string(),
                SettingOverrideV2::Set {
                    value: ConnectionReferenceV2 {
                        id: credential_ref.clone(),
                        provider_id: provider_id.to_string(),
                        endpoint_id: None,
                        base_url: config
                            .get("baseUrl")
                            .and_then(Value::as_str)
                            .and_then(|value| non_empty(Some(value)))
                            .map(str::to_string),
                        credential_ref: Some(credential_ref),
                    },
                },
            );
            overrides.capabilities.insert(
                capability.to_string(),
                SettingOverrideV2::Set {
                    value: CapabilityBindingV2 {
                        primary: Some(ModelReferenceV2 {
                            provider_id: provider_id.to_string(),
                            endpoint_id: None,
                            model_id: model_id.to_string(),
                        }),
                        fallbacks: Vec::new(),
                        options: BTreeMap::new(),
                    },
                },
            );
        }
        let profile = SettingsProfileV2 {
            schema_version: SETTINGS_SCHEMA_VERSION_V2,
            revision,
            id: "settings-v2:application".to_string(),
            name: "Application defaults".to_string(),
            scope: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Application,
                id: None,
            },
            preset: None,
            overrides,
            legacy_source: Some(LegacySettingsSourceV2 {
                kind: "app_config".to_string(),
                id: self.key.clone(),
                migration_key: LEGACY_SETTINGS_MIGRATION_KEY.to_string(),
                source_fingerprint: fingerprint.to_string(),
                credential_ref: None,
            }),
            extensions: BTreeMap::from([("legacyV1".to_string(), sanitized)]),
        };
        profile.validate()?;
        Ok(profile)
    }
}

impl RawLegacyAgentConfigSnapshot {
    fn source_hash(&self) -> Result<String, CoreError> {
        sha256_json(self)
    }

    fn projection_fingerprint(&self) -> Result<String, CoreError> {
        let mut projection =
            sanitize_legacy_value(serde_json::to_value(&self.config)?, "legacy-agent-config");
        if let Value::Object(fields) = &mut projection {
            fields.remove("createdAt");
            fields.remove("updatedAt");
        }
        sha256_json(&projection)
    }
}

impl LegacyAgentConfigSnapshot {
    fn profile(&self, fingerprint: &str) -> SettingsProfileV2 {
        let endpoint_id = self.provider_endpoint_id.clone();
        let model_id = self
            .model_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.model.clone());
        let text_model = ModelReferenceV2 {
            provider_id: self.provider.clone(),
            endpoint_id: endpoint_id.clone(),
            model_id,
        };
        let profile_id = legacy_profile_id(&self.id);
        let credential_ref = format!("legacy-agent-config:{}", self.id);
        let mut overrides = SettingsOverridesV2::default();
        overrides.connections.insert(
            "default".to_string(),
            SettingOverrideV2::Set {
                value: ConnectionReferenceV2 {
                    id: credential_ref.clone(),
                    provider_id: self.provider.clone(),
                    endpoint_id: endpoint_id.clone(),
                    base_url: self
                        .base_url
                        .as_deref()
                        .filter(|value| !endpoint_contains_credentials(value))
                        .map(str::to_string),
                    credential_ref: Some(credential_ref.clone()),
                },
            },
        );
        overrides.models.insert(
            "text".to_string(),
            SettingOverrideV2::Set {
                value: text_model.clone(),
            },
        );

        if let Some(image_model) = non_empty(self.image_generation_model.as_deref()) {
            overrides.capabilities.insert(
                "image_generation".to_string(),
                SettingOverrideV2::Set {
                    value: CapabilityBindingV2 {
                        primary: Some(ModelReferenceV2 {
                            provider_id: self.provider.clone(),
                            endpoint_id: endpoint_id.clone(),
                            model_id: image_model.to_string(),
                        }),
                        fallbacks: Vec::new(),
                        options: BTreeMap::new(),
                    },
                },
            );
        }
        if let Some(summary_model) = non_empty(self.summarization_model.as_deref()) {
            overrides.capabilities.insert(
                "summarization".to_string(),
                SettingOverrideV2::Set {
                    value: CapabilityBindingV2 {
                        primary: Some(ModelReferenceV2 {
                            provider_id: non_empty(self.summarization_provider.as_deref())
                                .unwrap_or(&self.provider)
                                .to_string(),
                            endpoint_id: None,
                            model_id: summary_model.to_string(),
                        }),
                        fallbacks: Vec::new(),
                        options: BTreeMap::new(),
                    },
                },
            );
        }

        insert_legacy_value(&mut overrides.advanced, "temperature", self.temperature);
        insert_legacy_value(
            &mut overrides.advanced,
            "max_output_tokens",
            self.max_tokens,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "context_window_override",
            self.context_window,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "reasoning_enabled",
            self.reasoning_enabled,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "thinking_budget",
            self.thinking_budget,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "reasoning_effort",
            self.reasoning_effort.clone(),
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "max_iterations",
            self.max_iterations,
        );
        insert_legacy_json(
            &mut overrides.advanced,
            "subagent_allowed_tools",
            self.subagent_allowed_tools_json.as_deref(),
        );
        insert_legacy_json(
            &mut overrides.advanced,
            "subagent_allowed_skill_ids",
            self.subagent_allowed_skill_ids_json.as_deref(),
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "subagent_max_parallel",
            self.subagent_max_parallel,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "subagent_max_calls_per_turn",
            self.subagent_max_calls_per_turn,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "subagent_token_budget",
            self.subagent_token_budget,
        );
        insert_legacy_json(
            &mut overrides.advanced,
            "delegation_limits_v2",
            self.delegation_limits_v2_json.as_deref(),
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "tool_timeout_seconds",
            self.tool_timeout_secs,
        );
        insert_legacy_value(
            &mut overrides.advanced,
            "agent_timeout_seconds",
            self.agent_timeout_secs,
        );
        insert_legacy_value(&mut overrides.advanced, "is_default", Some(self.is_default));

        SettingsProfileV2 {
            schema_version: SETTINGS_SCHEMA_VERSION_V2,
            revision: 1,
            id: profile_id,
            name: self.name.clone(),
            scope: SettingsScopeV2 {
                kind: SettingsScopeKindV2::Agent,
                id: Some(self.id.clone()),
            },
            preset: None,
            overrides,
            legacy_source: Some(LegacySettingsSourceV2 {
                kind: "agent_config".to_string(),
                id: self.id.clone(),
                migration_key: LEGACY_SETTINGS_MIGRATION_KEY.to_string(),
                source_fingerprint: fingerprint.to_string(),
                credential_ref: Some(credential_ref),
            }),
            extensions: BTreeMap::from([(
                "legacyV1".to_string(),
                sanitize_legacy_value(
                    serde_json::to_value(self).unwrap_or(Value::Null),
                    "legacy-agent-config",
                ),
            )]),
        }
    }
}

fn sha256_json<T: Serialize>(value: &T) -> Result<String, CoreError> {
    let bytes = serde_json::to_vec(value)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn sanitize_app_config_value(value: Value) -> Value {
    sanitize_legacy_value(value, "legacy-app-config")
}

fn sanitize_legacy_value(mut value: Value, path: &str) -> Value {
    match &mut value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let child_path = format!("{path}:{key}");
                if is_secret_field(key) {
                    let configured = child.as_str().is_some_and(|value| !value.is_empty());
                    *child = serde_json::json!({
                        "credentialRef": child_path,
                        "configured": configured,
                    });
                } else if is_endpoint_field(key)
                    && child.as_str().is_some_and(endpoint_contains_credentials)
                {
                    *child = serde_json::json!({
                        "credentialRef": child_path,
                        "redacted": true,
                    });
                } else {
                    *child = sanitize_legacy_value(child.take(), &child_path);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                *child = sanitize_legacy_value(child.take(), &format!("{path}:{index}"));
            }
        }
        _ => {}
    }
    value
}

fn is_secret_field(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "token"
            | "secret"
            | "authorization"
            | "cookie"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "bearertoken"
            | "clientsecret"
            | "secretkey"
            | "sessiontoken"
            | "signingkey"
            | "encryptionkey"
            | "webhooksecret"
            | "credential"
            | "credentials"
            | "password"
            | "privatekey"
            | "sessionkey"
    ) || [
        "apikey",
        "token",
        "secret",
        "password",
        "privatekey",
        "secretkey",
        "sessionkey",
        "signingkey",
        "encryptionkey",
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
}

fn validate_secret_free_value(value: &Value, path: &str) -> Result<(), CoreError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                if is_secret_field(key) {
                    let reference_value = child.as_object().and_then(|reference| {
                        let valid_shape = reference.keys().all(|field| {
                            matches!(field.as_str(), "credentialRef" | "configured" | "redacted")
                        }) && reference
                            .get("configured")
                            .is_none_or(Value::is_boolean)
                            && reference.get("redacted").is_none_or(Value::is_boolean);
                        valid_shape
                            .then(|| reference.get("credentialRef").and_then(Value::as_str))
                            .flatten()
                    });
                    let Some(reference) = reference_value else {
                        return Err(CoreError::InvalidInput(format!(
                            "Settings V2 cannot persist secret field {child_path}; use credentialRef"
                        )));
                    };
                    validate_credential_reference(reference)?;
                }
                if is_endpoint_field(key)
                    && child.as_str().is_some_and(endpoint_contains_credentials)
                {
                    return Err(CoreError::InvalidInput(format!(
                        "Settings V2 cannot persist credentials in endpoint {child_path}"
                    )));
                }
                validate_secret_free_value(child, &child_path)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                validate_secret_free_value(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reject_native_secret_fields(value: &Value, path: &str) -> Result<(), CoreError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                if is_secret_field(key) {
                    return Err(CoreError::InvalidInput(format!(
                        "Native Settings V2 cannot persist secret-shaped field {child_path}"
                    )));
                }
                reject_native_secret_fields(child, &child_path)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_native_secret_fields(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_endpoint_field(key: &str) -> bool {
    matches!(
        key.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .as_str(),
        "baseurl" | "endpoint" | "endpointurl" | "apiurl" | "url"
    )
}

fn endpoint_contains_credentials(endpoint: &str) -> bool {
    let authority = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, remainder)| remainder)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.contains('@') {
        return true;
    }
    let Some((_, query_and_fragment)) = endpoint.split_once('?') else {
        return false;
    };
    query_and_fragment
        .split(['&', '#'])
        .filter_map(|pair| pair.split_once('=').map(|(key, _)| key))
        .any(is_secret_field)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn legacy_profile_id(source_id: &str) -> String {
    format!("settings-v2:agent:{source_id}")
}

fn insert_legacy_value<T: Serialize>(
    target: &mut BTreeMap<String, SettingOverrideV2<Value>>,
    key: &str,
    value: Option<T>,
) {
    let value = match value {
        Some(value) => SettingOverrideV2::Set {
            value: serde_json::to_value(value).unwrap_or(Value::Null),
        },
        None => SettingOverrideV2::Clear,
    };
    target.insert(key.to_string(), value);
}

fn insert_legacy_json(
    target: &mut BTreeMap<String, SettingOverrideV2<Value>>,
    key: &str,
    raw: Option<&str>,
) {
    let value = match raw {
        Some(raw) => SettingOverrideV2::Set {
            value: serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string())),
        },
        None => SettingOverrideV2::Clear,
    };
    target.insert(key.to_string(), value);
}

fn projection_error(source_id: &str, detail: &str) -> CoreError {
    CoreError::Internal(format!(
        "Settings V1 -> V2 -> V1 verification failed for {source_id}: {detail}"
    ))
}

fn verify_legacy_agent_profile(
    profile: &SettingsProfileV2,
    snapshot: &LegacyAgentConfigSnapshot,
) -> Result<(), CoreError> {
    if profile.name != snapshot.name
        || profile.scope.kind != SettingsScopeKindV2::Agent
        || profile.scope.id.as_deref() != Some(snapshot.id.as_str())
    {
        return Err(projection_error(&snapshot.id, "profile identity changed"));
    }
    let connection = match profile.overrides.connections.get("default") {
        Some(SettingOverrideV2::Set { value }) => value,
        _ => return Err(projection_error(&snapshot.id, "default connection missing")),
    };
    let expected_base_url = snapshot
        .base_url
        .as_deref()
        .filter(|value| !endpoint_contains_credentials(value));
    let expected_credential_ref = format!("legacy-agent-config:{}", snapshot.id);
    if connection.provider_id != snapshot.provider
        || connection.endpoint_id != snapshot.provider_endpoint_id
        || connection.base_url.as_deref() != expected_base_url
        || connection.credential_ref.as_deref() != Some(expected_credential_ref.as_str())
    {
        return Err(projection_error(&snapshot.id, "connection mapping changed"));
    }
    let text_model = match profile.overrides.models.get("text") {
        Some(SettingOverrideV2::Set { value }) => value,
        _ => return Err(projection_error(&snapshot.id, "text model missing")),
    };
    let expected_model_id = snapshot
        .model_id
        .as_deref()
        .and_then(|value| non_empty(Some(value)))
        .unwrap_or(&snapshot.model);
    if text_model.provider_id != snapshot.provider
        || text_model.endpoint_id != snapshot.provider_endpoint_id
        || text_model.model_id != expected_model_id
    {
        return Err(projection_error(&snapshot.id, "text model mapping changed"));
    }

    let mut expected_advanced = BTreeMap::new();
    insert_legacy_value(&mut expected_advanced, "temperature", snapshot.temperature);
    insert_legacy_value(
        &mut expected_advanced,
        "max_output_tokens",
        snapshot.max_tokens,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "context_window_override",
        snapshot.context_window,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "reasoning_enabled",
        snapshot.reasoning_enabled,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "thinking_budget",
        snapshot.thinking_budget,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "reasoning_effort",
        snapshot.reasoning_effort.clone(),
    );
    insert_legacy_value(
        &mut expected_advanced,
        "max_iterations",
        snapshot.max_iterations,
    );
    insert_legacy_json(
        &mut expected_advanced,
        "subagent_allowed_tools",
        snapshot.subagent_allowed_tools_json.as_deref(),
    );
    insert_legacy_json(
        &mut expected_advanced,
        "subagent_allowed_skill_ids",
        snapshot.subagent_allowed_skill_ids_json.as_deref(),
    );
    insert_legacy_value(
        &mut expected_advanced,
        "subagent_max_parallel",
        snapshot.subagent_max_parallel,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "subagent_max_calls_per_turn",
        snapshot.subagent_max_calls_per_turn,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "subagent_token_budget",
        snapshot.subagent_token_budget,
    );
    insert_legacy_json(
        &mut expected_advanced,
        "delegation_limits_v2",
        snapshot.delegation_limits_v2_json.as_deref(),
    );
    insert_legacy_value(
        &mut expected_advanced,
        "tool_timeout_seconds",
        snapshot.tool_timeout_secs,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "agent_timeout_seconds",
        snapshot.agent_timeout_secs,
    );
    insert_legacy_value(
        &mut expected_advanced,
        "is_default",
        Some(snapshot.is_default),
    );
    if profile.overrides.advanced != expected_advanced {
        return Err(projection_error(
            &snapshot.id,
            "advanced settings mapping changed",
        ));
    }

    verify_capability_projection(
        profile,
        "image_generation",
        snapshot.image_generation_model.as_deref(),
        &snapshot.provider,
        snapshot.provider_endpoint_id.as_deref(),
        &snapshot.id,
    )?;
    let summary_provider =
        non_empty(snapshot.summarization_provider.as_deref()).unwrap_or(&snapshot.provider);
    verify_capability_projection(
        profile,
        "summarization",
        snapshot.summarization_model.as_deref(),
        summary_provider,
        None,
        &snapshot.id,
    )?;

    let expected_extension =
        sanitize_legacy_value(serde_json::to_value(snapshot)?, "legacy-agent-config");
    if profile.extensions.get("legacyV1") != Some(&expected_extension) {
        return Err(projection_error(
            &snapshot.id,
            "legacy compatibility extension changed",
        ));
    }
    Ok(())
}

fn verify_capability_projection(
    profile: &SettingsProfileV2,
    capability: &str,
    legacy_model: Option<&str>,
    provider_id: &str,
    endpoint_id: Option<&str>,
    source_id: &str,
) -> Result<(), CoreError> {
    let expected_model = non_empty(legacy_model);
    let actual = profile.overrides.capabilities.get(capability);
    match (expected_model, actual) {
        (None, None) => Ok(()),
        (Some(expected_model), Some(SettingOverrideV2::Set { value }))
            if value.primary.as_ref().is_some_and(|primary| {
                primary.provider_id == provider_id
                    && primary.endpoint_id.as_deref() == endpoint_id
                    && primary.model_id == expected_model
            }) && value.fallbacks.is_empty() =>
        {
            Ok(())
        }
        _ => Err(projection_error(
            source_id,
            &format!("{capability} capability mapping changed"),
        )),
    }
}

fn verify_legacy_app_profile(
    profile: &SettingsProfileV2,
    source: &RawLegacyAppConfigSnapshot,
) -> Result<(), CoreError> {
    if profile.scope.kind != SettingsScopeKindV2::Application || profile.scope.id.is_some() {
        return Err(projection_error("app_config", "application scope changed"));
    }
    let sanitized = sanitize_app_config_value(serde_json::from_str(&source.value)?);
    if profile.extensions.get("legacyV1") != Some(&sanitized) {
        return Err(projection_error(
            "app_config",
            "legacy compatibility extension changed",
        ));
    }
    for (field, capability) in [
        ("imageGeneration", "image_generation"),
        ("textToSpeech", "text_to_speech"),
        ("speechToText", "speech_to_text"),
    ] {
        let config = sanitized.get(field).and_then(Value::as_object);
        let provider = config
            .and_then(|value| value.get("provider"))
            .and_then(Value::as_str)
            .and_then(|value| non_empty(Some(value)));
        let model = config
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str)
            .and_then(|value| non_empty(Some(value)));
        match (provider, model) {
            (Some(provider), Some(model)) => {
                let connection = match profile.overrides.connections.get(capability) {
                    Some(SettingOverrideV2::Set { value }) => value,
                    _ => {
                        return Err(projection_error(
                            "app_config",
                            &format!("{capability} connection missing"),
                        ));
                    }
                };
                if connection.provider_id != provider
                    || connection.base_url.as_deref()
                        != config
                            .and_then(|value| value.get("baseUrl"))
                            .and_then(Value::as_str)
                            .and_then(|value| non_empty(Some(value)))
                {
                    return Err(projection_error(
                        "app_config",
                        &format!("{capability} connection mapping changed"),
                    ));
                }
                verify_capability_projection(
                    profile,
                    capability,
                    Some(model),
                    provider,
                    None,
                    "app_config",
                )?;
            }
            _ if profile.overrides.connections.contains_key(capability)
                || profile.overrides.capabilities.contains_key(capability) =>
            {
                return Err(projection_error(
                    "app_config",
                    &format!("{capability} should be absent"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn read_legacy_agent_configs(
    conn: &Connection,
    source_id: Option<&str>,
) -> Result<Vec<RawLegacyAgentConfigSnapshot>, CoreError> {
    let sql = "SELECT id, name, provider, api_key, base_url, model, temperature,
                      max_tokens, context_window, is_default, reasoning_enabled,
                      thinking_budget, reasoning_effort, created_at, updated_at,
                      max_iterations, summarization_model, summarization_provider,
                      image_generation_model, subagent_allowed_tools_json,
                      subagent_allowed_skill_ids_json, subagent_max_parallel,
                      subagent_max_calls_per_turn, subagent_token_budget,
                      tool_timeout_secs, agent_timeout_secs, provider_endpoint_id,
                      model_id, delegation_limits_v2_json
               FROM agent_configs
               WHERE (?1 IS NULL OR id = ?1)
               ORDER BY id";
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![source_id], |row| {
        let api_key: String = row.get(3)?;
        let id: String = row.get(0)?;
        Ok(RawLegacyAgentConfigSnapshot {
            config: LegacyAgentConfigSnapshot {
                id: id.clone(),
                name: row.get(1)?,
                provider: row.get(2)?,
                base_url: row.get(4)?,
                model: row.get(5)?,
                temperature: row.get(6)?,
                max_tokens: row.get(7)?,
                context_window: row.get(8)?,
                is_default: row.get::<_, i32>(9)? != 0,
                reasoning_enabled: row.get(10)?,
                thinking_budget: row.get(11)?,
                reasoning_effort: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                max_iterations: row.get(15)?,
                summarization_model: row.get(16)?,
                summarization_provider: row.get(17)?,
                image_generation_model: row.get(18)?,
                subagent_allowed_tools_json: row.get(19)?,
                subagent_allowed_skill_ids_json: row.get(20)?,
                subagent_max_parallel: row.get(21)?,
                subagent_max_calls_per_turn: row.get(22)?,
                subagent_token_budget: row.get(23)?,
                tool_timeout_secs: row.get(24)?,
                agent_timeout_secs: row.get(25)?,
                provider_endpoint_id: row.get(26)?,
                model_id: row.get(27)?,
                delegation_limits_v2_json: row.get(28)?,
                credential: LegacyCredentialReference {
                    storage: "agent_configs.api_key".to_string(),
                    source_id: id,
                    configured: !api_key.is_empty(),
                },
            },
            api_key_ciphertext: api_key,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::Database)
}

fn read_legacy_app_config(
    conn: &Connection,
) -> Result<Option<RawLegacyAppConfigSnapshot>, CoreError> {
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'app_config'
         )",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(None);
    }
    conn.query_row(
        "SELECT key, value, updated_at FROM app_config WHERE key = 'app_config'",
        [],
        |row| {
            Ok(RawLegacyAppConfigSnapshot {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(CoreError::Database)
}

fn sync_legacy_agent_configs(
    conn: &mut Connection,
    source_id: Option<&str>,
    force_rolled_back: bool,
) -> Result<SettingsMigrationReportV2, CoreError> {
    let sources = read_legacy_agent_configs(conn, source_id)?;
    let app_source = if source_id.is_none() {
        read_legacy_app_config(conn)?
    } else {
        None
    };
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let report = sync_legacy_sources_in_transaction(
        &transaction,
        sources,
        app_source,
        force_rolled_back,
        source_id.is_none(),
    )?;
    transaction.commit()?;
    Ok(report)
}

fn sync_legacy_sources_in_transaction(
    transaction: &Transaction<'_>,
    sources: Vec<RawLegacyAgentConfigSnapshot>,
    app_source: Option<RawLegacyAppConfigSnapshot>,
    force_rolled_back: bool,
    remove_agent_orphans: bool,
) -> Result<SettingsMigrationReportV2, CoreError> {
    let mut report = SettingsMigrationReportV2::default();
    let migration_run_id = Uuid::new_v4().to_string();

    if remove_agent_orphans {
        transaction.execute(
            "UPDATE settings_schema_migration_journal
             SET status = 'superseded'
             WHERE migration_key = ?1 AND source_kind = 'agent_config'
               AND status = 'applied'
               AND NOT EXISTS (
                   SELECT 1 FROM agent_configs
                   WHERE agent_configs.id = settings_schema_migration_journal.source_id
               )",
            params![LEGACY_SETTINGS_MIGRATION_KEY],
        )?;
        report.removed_orphans = transaction.execute(
            "DELETE FROM settings_profiles_v2
             WHERE managed_by = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM agent_configs
                   WHERE agent_configs.id = settings_profiles_v2.legacy_source_id
               )",
            params![LEGACY_AGENT_CONFIG_MANAGER],
        )?;
    }

    for source in sources {
        let snapshot = &source.config;
        let fingerprint = source.projection_fingerprint()?;
        let current_revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM settings_profiles_v2 WHERE id = ?1",
                params![legacy_profile_id(&snapshot.id)],
                |row| row.get(0),
            )
            .optional()?;
        let mut profile = snapshot.profile(&fingerprint);
        profile.revision = current_revision.map_or(1, |revision| revision + 1);
        profile.validate()?;
        let existing_status: Option<String> = transaction
            .query_row(
                "SELECT status
                 FROM settings_schema_migration_journal
                 WHERE migration_key = ?1 AND source_kind = 'agent_config'
                   AND source_id = ?2 AND source_fingerprint = ?3
                 ORDER BY applied_at DESC, rowid DESC
                 LIMIT 1",
                params![LEGACY_SETTINGS_MIGRATION_KEY, &snapshot.id, &fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        let profile_exists: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM settings_profiles_v2
                 WHERE id = ?1 AND managed_by = ?2 AND source_fingerprint = ?3
             )",
            params![&profile.id, LEGACY_AGENT_CONFIG_MANAGER, &fingerprint],
            |row| row.get(0),
        )?;

        if existing_status.as_deref() == Some("rolled_back") && !force_rolled_back {
            report.skipped_rolled_back += 1;
            continue;
        }
        if existing_status.as_deref() == Some("applied") && profile_exists {
            report.unchanged += 1;
            continue;
        }

        verify_legacy_agent_profile(&profile, snapshot)?;

        transaction.execute(
            "UPDATE settings_schema_migration_journal
             SET status = 'superseded'
             WHERE migration_key = ?1 AND source_kind = 'agent_config'
               AND source_id = ?2 AND status = 'applied'",
            params![LEGACY_SETTINGS_MIGRATION_KEY, &snapshot.id],
        )?;

        let document_json = serde_json::to_string(&profile)?;
        let target_hash = sha256_json(&profile)?;
        transaction.execute(
            "INSERT INTO settings_profiles_v2 (
                 id, schema_version, revision, scope_kind, scope_id, name,
                 preset_id, preset_version, preset_hash, document_json,
                 managed_by, legacy_source_id, source_fingerprint
             ) VALUES (?1, ?2, ?3, 'agent', ?4, ?5, NULL, NULL, NULL, ?6, ?7, ?4, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 scope_kind = excluded.scope_kind,
                 scope_id = excluded.scope_id,
                 name = excluded.name,
                 preset_id = excluded.preset_id,
                 document_json = excluded.document_json,
                 managed_by = excluded.managed_by,
                 legacy_source_id = excluded.legacy_source_id,
                 source_fingerprint = excluded.source_fingerprint,
                 updated_at = datetime('now')",
            params![
                &profile.id,
                SETTINGS_SCHEMA_VERSION_V2,
                profile.revision,
                &snapshot.id,
                &profile.name,
                &document_json,
                LEGACY_AGENT_CONFIG_MANAGER,
                &fingerprint,
            ],
        )?;

        let source_snapshot_json = serde_json::to_string(&source)?;
        let source_snapshot_ciphertext = crate::crypto::encrypt_api_key(&source_snapshot_json)?;
        let source_hash = source.source_hash()?;
        transaction.execute(
            "INSERT INTO settings_schema_migration_journal (
                 id, migration_run_id, migration_key, source_kind, source_id,
                 source_fingerprint, target_profile_id, status,
                 source_snapshot_ciphertext, source_hash, target_hash,
                 round_trip_verified
             ) VALUES (?1, ?2, ?3, 'agent_config', ?4, ?5, ?6, 'applied', ?7, ?8, ?9, 1)",
            params![
                Uuid::new_v4().to_string(),
                &migration_run_id,
                LEGACY_SETTINGS_MIGRATION_KEY,
                &snapshot.id,
                &fingerprint,
                &profile.id,
                &source_snapshot_ciphertext,
                &source_hash,
                &target_hash,
            ],
        )?;
        report.migrated += 1;
    }

    if let Some(source) = app_source {
        let fingerprint = source.projection_fingerprint()?;
        let profile_id = "settings-v2:application";
        let current_revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM settings_profiles_v2 WHERE id = ?1",
                params![profile_id],
                |row| row.get(0),
            )
            .optional()?;
        let profile = source.profile(
            &fingerprint,
            current_revision.map_or(1, |revision| revision + 1),
        )?;
        let existing_status: Option<String> = transaction
            .query_row(
                "SELECT status
                 FROM settings_schema_migration_journal
                 WHERE migration_key = ?1 AND source_kind = 'app_config'
                   AND source_id = ?2 AND source_fingerprint = ?3
                 ORDER BY applied_at DESC, rowid DESC
                 LIMIT 1",
                params![LEGACY_SETTINGS_MIGRATION_KEY, &source.key, &fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        let profile_exists: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM settings_profiles_v2
                 WHERE id = ?1 AND managed_by = ?2 AND source_fingerprint = ?3
             )",
            params![profile_id, LEGACY_APP_CONFIG_MANAGER, &fingerprint],
            |row| row.get(0),
        )?;
        if existing_status.as_deref() == Some("rolled_back") && !force_rolled_back {
            report.skipped_rolled_back += 1;
        } else if existing_status.as_deref() == Some("applied") && profile_exists {
            report.unchanged += 1;
        } else {
            verify_legacy_app_profile(&profile, &source)?;
            transaction.execute(
                "UPDATE settings_schema_migration_journal
                 SET status = 'superseded'
                 WHERE migration_key = ?1 AND source_kind = 'app_config'
                   AND source_id = ?2 AND status = 'applied'",
                params![LEGACY_SETTINGS_MIGRATION_KEY, &source.key],
            )?;
            let document_json = serde_json::to_string(&profile)?;
            let target_hash = sha256_json(&profile)?;
            transaction.execute(
                "INSERT INTO settings_profiles_v2 (
                     id, schema_version, revision, scope_kind, scope_id, name,
                     preset_id, preset_version, preset_hash, document_json,
                     managed_by, legacy_source_id, source_fingerprint
                 ) VALUES (?1, ?2, ?3, 'application', '', ?4, NULL, NULL, NULL, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                     schema_version = excluded.schema_version,
                     revision = excluded.revision,
                     scope_kind = excluded.scope_kind,
                     scope_id = excluded.scope_id,
                     name = excluded.name,
                     preset_id = excluded.preset_id,
                     preset_version = excluded.preset_version,
                     preset_hash = excluded.preset_hash,
                     document_json = excluded.document_json,
                     managed_by = excluded.managed_by,
                     legacy_source_id = excluded.legacy_source_id,
                     source_fingerprint = excluded.source_fingerprint,
                     updated_at = datetime('now')",
                params![
                    &profile.id,
                    SETTINGS_SCHEMA_VERSION_V2,
                    profile.revision,
                    &profile.name,
                    &document_json,
                    LEGACY_APP_CONFIG_MANAGER,
                    &source.key,
                    &fingerprint,
                ],
            )?;
            let source_snapshot_json = serde_json::to_string(&source)?;
            let source_snapshot_ciphertext = crate::crypto::encrypt_api_key(&source_snapshot_json)?;
            let source_hash = source.source_hash()?;
            transaction.execute(
                "INSERT INTO settings_schema_migration_journal (
                     id, migration_run_id, migration_key, source_kind, source_id,
                     source_fingerprint, target_profile_id, status,
                     source_snapshot_ciphertext, source_hash, target_hash,
                     round_trip_verified
                 ) VALUES (?1, ?2, ?3, 'app_config', ?4, ?5, ?6, 'applied', ?7, ?8, ?9, 1)",
                params![
                    Uuid::new_v4().to_string(),
                    &migration_run_id,
                    LEGACY_SETTINGS_MIGRATION_KEY,
                    &source.key,
                    &fingerprint,
                    &profile.id,
                    &source_snapshot_ciphertext,
                    &source_hash,
                    &target_hash,
                ],
            )?;
            report.migrated += 1;
        }
    }

    let (active_version, active_migration_id): (u32, Option<String>) = transaction.query_row(
        "SELECT active_version, migration_id
         FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let activation_id = if report.migrated > 0 || active_version != SETTINGS_SCHEMA_VERSION_V2 {
        migration_run_id
    } else {
        active_migration_id.unwrap_or(migration_run_id)
    };
    transaction.execute(
        "UPDATE settings_schema_state
         SET active_version = 2, migration_id = ?1,
             activated_at = COALESCE(activated_at, datetime('now')),
             updated_at = datetime('now')
         WHERE singleton_id = 1",
        params![activation_id],
    )?;

    Ok(report)
}

pub(crate) fn migrate_legacy_agent_configs_on_open(
    conn: &mut Connection,
) -> Result<SettingsMigrationReportV2, CoreError> {
    let (active_version, migration_id): (u32, Option<String>) = conn.query_row(
        "SELECT active_version, migration_id
         FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if active_version == 1 && migration_id.is_some() {
        return Ok(SettingsMigrationReportV2::default());
    }
    sync_legacy_agent_configs(conn, None, false)
}

pub(crate) fn sync_legacy_agent_config_in_transaction(
    transaction: &Transaction<'_>,
    source_id: &str,
) -> Result<SettingsMigrationReportV2, CoreError> {
    if !settings_schema_v2_is_active(transaction)? {
        return Ok(SettingsMigrationReportV2::default());
    }
    let sources = read_legacy_agent_configs(transaction, Some(source_id))?;
    sync_legacy_sources_in_transaction(transaction, sources, None, false, false)
}

pub(crate) fn sync_all_legacy_agent_configs_in_transaction(
    transaction: &Transaction<'_>,
) -> Result<SettingsMigrationReportV2, CoreError> {
    if !settings_schema_v2_is_active(transaction)? {
        return Ok(SettingsMigrationReportV2::default());
    }
    let sources = read_legacy_agent_configs(transaction, None)?;
    sync_legacy_sources_in_transaction(transaction, sources, None, false, true)
}

pub(crate) fn sync_legacy_app_config_in_transaction(
    transaction: &Transaction<'_>,
) -> Result<SettingsMigrationReportV2, CoreError> {
    if !settings_schema_v2_is_active(transaction)? {
        return Ok(SettingsMigrationReportV2::default());
    }
    let app_source = read_legacy_app_config(transaction)?;
    sync_legacy_sources_in_transaction(transaction, Vec::new(), app_source, false, false)
}

fn settings_schema_v2_is_active(transaction: &Transaction<'_>) -> Result<bool, CoreError> {
    let active_version: u32 = transaction.query_row(
        "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(active_version == SETTINGS_SCHEMA_VERSION_V2)
}

pub(crate) fn remove_legacy_agent_config_projection(
    conn: &Connection,
    source_id: &str,
) -> Result<(), CoreError> {
    let active_version: u32 = conn.query_row(
        "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| row.get(0),
    )?;
    if active_version == SETTINGS_SCHEMA_VERSION_V2 {
        conn.execute(
            "DELETE FROM settings_profiles_v2
             WHERE managed_by = ?1 AND legacy_source_id = ?2",
            params![LEGACY_AGENT_CONFIG_MANAGER, source_id],
        )?;
        conn.execute(
            "UPDATE settings_schema_migration_journal
             SET status = 'superseded'
             WHERE migration_key = ?1 AND source_kind = 'agent_config'
               AND source_id = ?2 AND status = 'applied'",
            params![LEGACY_SETTINGS_MIGRATION_KEY, source_id],
        )?;
    }
    Ok(())
}

fn expected_rollback_sources(
    transaction: &Transaction<'_>,
) -> Result<BTreeSet<(String, String)>, CoreError> {
    let mut expected = BTreeSet::new();
    {
        let mut statement = transaction.prepare("SELECT id FROM agent_configs ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for source_id in rows {
            expected.insert(("agent_config".to_string(), source_id?));
        }
    }
    if let Some(source) = read_legacy_app_config(transaction)? {
        expected.insert(("app_config".to_string(), source.key));
    }
    Ok(expected)
}

fn validate_native_credential_references(
    conn: &Connection,
    profile: &SettingsProfileV2,
) -> Result<(), CoreError> {
    for value in profile.overrides.connections.values() {
        let SettingOverrideV2::Set { value } = value else {
            continue;
        };
        let Some(reference) = value.credential_ref.as_deref() else {
            continue;
        };
        let (namespace, identifier) = reference.split_once(':').ok_or_else(|| {
            CoreError::InvalidInput("credentialRef must be namespaced".to_string())
        })?;
        let exists = match namespace {
            "legacy-agent-config" => conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_configs WHERE id = ?1)",
                params![identifier],
                |row| row.get(0),
            )?,
            "legacy-app-config"
                if matches!(
                    identifier,
                    "imageGeneration" | "textToSpeech" | "speechToText"
                ) =>
            {
                let table_exists: bool = conn.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sqlite_master
                         WHERE type = 'table' AND name = 'app_config'
                     )",
                    [],
                    |row| row.get(0),
                )?;
                table_exists
                    && conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM app_config WHERE key = 'app_config')",
                        [],
                        |row| row.get(0),
                    )?
            }
            _ => false,
        };
        if !exists {
            return Err(CoreError::InvalidInput(format!(
                "credentialRef {reference} does not resolve to an existing credential store"
            )));
        }
    }
    Ok(())
}

impl Database {
    pub fn settings_schema_state_v2(&self) -> Result<SettingsSchemaStateV2, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT active_version, migration_id, activated_at
             FROM settings_schema_state WHERE singleton_id = 1",
            [],
            |row| {
                Ok(SettingsSchemaStateV2 {
                    active_version: row.get(0)?,
                    migration_id: row.get(1)?,
                    activated_at: row.get(2)?,
                })
            },
        )
        .map_err(CoreError::Database)
    }

    pub fn migrate_settings_schema_v2(&self) -> Result<SettingsMigrationReportV2, CoreError> {
        let mut conn = self.conn();
        sync_legacy_agent_configs(&mut conn, None, true)
    }

    /// Restore every exact encrypted V1 row captured by the active migration,
    /// then flip the active schema pointer last. V2 sidecars are preserved.
    pub fn rollback_settings_schema_v2(&self) -> Result<bool, CoreError> {
        let mut conn = self.conn();
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (active_version, migration_id): (u32, Option<String>) = transaction.query_row(
            "SELECT active_version, migration_id
             FROM settings_schema_state WHERE singleton_id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if active_version != SETTINGS_SCHEMA_VERSION_V2 {
            return Ok(false);
        }
        if migration_id
            .as_deref()
            .is_none_or(|migration_id| migration_id.trim().is_empty())
        {
            return Err(CoreError::InvalidInput(
                "Active Settings V2 state has no migration id".to_string(),
            ));
        }
        let snapshots = {
            let mut statement = transaction.prepare(
                "SELECT id, source_kind, source_id,
                        source_snapshot_ciphertext, source_hash
                 FROM settings_schema_migration_journal
                 WHERE migration_key = ?1 AND status = 'applied'
                 ORDER BY source_kind, source_id",
            )?;
            let rows = statement.query_map(params![LEGACY_SETTINGS_MIGRATION_KEY], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let actual_sources = snapshots
            .iter()
            .map(|(_, source_kind, source_id, _, _)| (source_kind.clone(), source_id.clone()))
            .collect::<BTreeSet<_>>();
        let expected_sources = expected_rollback_sources(&transaction)?;
        if snapshots.len() != actual_sources.len() || actual_sources != expected_sources {
            return Err(CoreError::InvalidInput(
                "Settings rollback snapshot set is incomplete or inconsistent".to_string(),
            ));
        }
        for (_journal_id, source_kind, _source_id, ciphertext, expected_hash) in &snapshots {
            let snapshot_json = crate::crypto::decrypt_api_key(ciphertext)?;
            match source_kind.as_str() {
                "agent_config" => {
                    let source: RawLegacyAgentConfigSnapshot =
                        serde_json::from_str(&snapshot_json)?;
                    if source.source_hash()? != *expected_hash {
                        return Err(CoreError::InvalidInput(format!(
                            "Settings rollback snapshot hash mismatch for {}",
                            source.config.id
                        )));
                    }
                    restore_legacy_agent_config(&transaction, &source)?;
                }
                "app_config" => {
                    let source: RawLegacyAppConfigSnapshot = serde_json::from_str(&snapshot_json)?;
                    if source.source_hash()? != *expected_hash {
                        return Err(CoreError::InvalidInput(
                            "Settings rollback snapshot hash mismatch for app_config".to_string(),
                        ));
                    }
                    restore_legacy_app_config(&transaction, &source)?;
                }
                other => {
                    return Err(CoreError::InvalidInput(format!(
                        "Unknown settings rollback source kind {other}"
                    )));
                }
            }
        }
        transaction.execute(
            "UPDATE settings_schema_migration_journal
              SET status = 'rolled_back', rolled_back_at = datetime('now')
             WHERE migration_key = ?1 AND status = 'applied'",
            params![LEGACY_SETTINGS_MIGRATION_KEY],
        )?;
        transaction.execute(
            "UPDATE settings_schema_state
             SET active_version = 1, updated_at = datetime('now')
             WHERE singleton_id = 1",
            [],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn list_settings_profiles_v2(&self) -> Result<Vec<SettingsProfileV2>, CoreError> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT document_json
             FROM settings_profiles_v2
             ORDER BY scope_kind, scope_id, name, id",
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

    /// Save a native V2 profile. Migration-managed profiles are read-only at
    /// this seam so compatibility metadata cannot be silently detached.
    pub fn save_settings_profile_v2(
        &self,
        profile: &SettingsProfileV2,
        expected_revision: Option<u64>,
    ) -> Result<SettingsProfileV2, CoreError> {
        profile.validate()?;
        if profile.legacy_source.is_some() {
            return Err(CoreError::InvalidInput(
                "Native V2 profiles must not provide legacySource".to_string(),
            ));
        }
        reject_native_secret_fields(
            &serde_json::json!({
                "overrides": &profile.overrides,
                "extensions": &profile.extensions,
            }),
            "settingsProfile",
        )?;
        let scope_id = profile.scope.id.as_deref().unwrap_or("");
        let conn = self.conn();
        validate_native_credential_references(&conn, profile)?;
        let scope_occupant: Option<String> = conn
            .query_row(
                "SELECT id FROM settings_profiles_v2
                 WHERE scope_kind = ?1 AND scope_id = ?2",
                params![profile.scope.kind.as_str(), scope_id],
                |row| row.get(0),
            )
            .optional()?;
        if scope_occupant
            .as_deref()
            .is_some_and(|occupant| occupant != profile.id)
        {
            return Err(CoreError::Conflict(format!(
                "Settings scope {}:{} is already owned by another profile",
                profile.scope.kind.as_str(),
                scope_id
            )));
        }
        let existing: Option<(u64, Option<String>)> = conn
            .query_row(
                "SELECT revision, managed_by FROM settings_profiles_v2 WHERE id = ?1",
                params![&profile.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if existing
            .as_ref()
            .and_then(|(_, manager)| manager.as_ref())
            .is_some()
        {
            return Err(CoreError::InvalidInput(
                "Migration-managed settings profiles cannot be overwritten".to_string(),
            ));
        }
        match (existing.as_ref(), expected_revision) {
            (None, Some(_)) => {
                return Err(CoreError::Conflict(format!(
                    "Settings profile {} does not exist",
                    profile.id
                )));
            }
            (Some(_), None) => {
                return Err(CoreError::Conflict(format!(
                    "Settings profile {} already exists",
                    profile.id
                )));
            }
            (Some((current, _)), Some(expected)) if *current != expected => {
                return Err(CoreError::Conflict(format!(
                    "Settings profile {} revision changed from {} to {}",
                    profile.id, expected, current
                )));
            }
            _ => {}
        }
        let mut saved = profile.clone();
        saved.revision = existing.map_or(1, |(revision, _)| revision + 1);
        let document_json = serde_json::to_string(&saved)?;
        let (preset_id, preset_version, preset_hash) = saved
            .preset
            .as_ref()
            .map(|preset| {
                (
                    Some(preset.id.as_str()),
                    Some(preset.version),
                    Some(preset.content_hash.as_str()),
                )
            })
            .unwrap_or((None, None, None));
        let affected = conn.execute(
            "INSERT INTO settings_profiles_v2 (
                 id, schema_version, revision, scope_kind, scope_id, name,
                 preset_id, preset_version, preset_hash, document_json,
                 managed_by, legacy_source_id, source_fingerprint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 scope_kind = excluded.scope_kind,
                 scope_id = excluded.scope_id,
                 name = excluded.name,
                 preset_id = excluded.preset_id,
                 preset_version = excluded.preset_version,
                 preset_hash = excluded.preset_hash,
                 document_json = excluded.document_json,
                 updated_at = datetime('now')
             WHERE settings_profiles_v2.revision = ?11
               AND settings_profiles_v2.managed_by IS NULL",
            params![
                &saved.id,
                SETTINGS_SCHEMA_VERSION_V2,
                saved.revision,
                saved.scope.kind.as_str(),
                scope_id,
                &saved.name,
                preset_id,
                preset_version,
                preset_hash,
                document_json,
                expected_revision.unwrap_or(0),
            ],
        )?;
        if affected != 1 {
            return Err(CoreError::Conflict(format!(
                "Settings profile {} was modified concurrently",
                saved.id
            )));
        }
        Ok(saved)
    }
}

fn restore_legacy_agent_config(
    transaction: &rusqlite::Transaction<'_>,
    source: &RawLegacyAgentConfigSnapshot,
) -> Result<(), CoreError> {
    let value = &source.config;
    transaction.execute(
        "INSERT INTO agent_configs (
             id, name, provider, api_key, base_url, model, temperature, max_tokens,
             context_window, is_default, reasoning_enabled, thinking_budget,
             reasoning_effort, created_at, updated_at, max_iterations,
             summarization_model, summarization_provider, image_generation_model,
             subagent_allowed_tools_json, subagent_allowed_skill_ids_json,
             subagent_max_parallel, subagent_max_calls_per_turn,
             subagent_token_budget, tool_timeout_secs, agent_timeout_secs,
             provider_endpoint_id, model_id, delegation_limits_v2_json
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
             ?28, ?29
         ) ON CONFLICT(id) DO UPDATE SET
             name = excluded.name, provider = excluded.provider,
             api_key = excluded.api_key, base_url = excluded.base_url,
             model = excluded.model, temperature = excluded.temperature,
             max_tokens = excluded.max_tokens, context_window = excluded.context_window,
             is_default = excluded.is_default,
             reasoning_enabled = excluded.reasoning_enabled,
             thinking_budget = excluded.thinking_budget,
             reasoning_effort = excluded.reasoning_effort,
             created_at = excluded.created_at, updated_at = excluded.updated_at,
             max_iterations = excluded.max_iterations,
             summarization_model = excluded.summarization_model,
             summarization_provider = excluded.summarization_provider,
             image_generation_model = excluded.image_generation_model,
             subagent_allowed_tools_json = excluded.subagent_allowed_tools_json,
             subagent_allowed_skill_ids_json = excluded.subagent_allowed_skill_ids_json,
             subagent_max_parallel = excluded.subagent_max_parallel,
             subagent_max_calls_per_turn = excluded.subagent_max_calls_per_turn,
             subagent_token_budget = excluded.subagent_token_budget,
             tool_timeout_secs = excluded.tool_timeout_secs,
             agent_timeout_secs = excluded.agent_timeout_secs,
             provider_endpoint_id = excluded.provider_endpoint_id,
             model_id = excluded.model_id,
             delegation_limits_v2_json = excluded.delegation_limits_v2_json",
        params![
            &value.id,
            &value.name,
            &value.provider,
            &source.api_key_ciphertext,
            &value.base_url,
            &value.model,
            value.temperature,
            value.max_tokens,
            value.context_window,
            value.is_default as i32,
            value.reasoning_enabled,
            value.thinking_budget,
            &value.reasoning_effort,
            &value.created_at,
            &value.updated_at,
            value.max_iterations,
            &value.summarization_model,
            &value.summarization_provider,
            &value.image_generation_model,
            &value.subagent_allowed_tools_json,
            &value.subagent_allowed_skill_ids_json,
            value.subagent_max_parallel,
            value.subagent_max_calls_per_turn,
            value.subagent_token_budget,
            value.tool_timeout_secs,
            value.agent_timeout_secs,
            &value.provider_endpoint_id,
            &value.model_id,
            &value.delegation_limits_v2_json,
        ],
    )?;
    Ok(())
}

fn restore_legacy_app_config(
    transaction: &rusqlite::Transaction<'_>,
    source: &RawLegacyAppConfigSnapshot,
) -> Result<(), CoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_config (
             key TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         )",
    )?;
    transaction.execute(
        "INSERT INTO app_config (key, value, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
        params![&source.key, &source.value, &source.updated_at],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::SaveAgentConfigInput;

    fn profile(id: &str, kind: SettingsScopeKindV2, scope_id: Option<&str>) -> SettingsProfileV2 {
        SettingsProfileV2 {
            schema_version: SETTINGS_SCHEMA_VERSION_V2,
            revision: 1,
            id: id.to_string(),
            name: id.to_string(),
            scope: SettingsScopeV2 {
                kind,
                id: scope_id.map(str::to_string),
            },
            preset: None,
            overrides: SettingsOverridesV2::default(),
            legacy_source: None,
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn resolves_four_layers_with_clear_and_provenance() {
        let mut application = profile("app", SettingsScopeKindV2::Application, None);
        application.overrides.advanced.insert(
            "temperature".to_string(),
            SettingOverrideV2::Set {
                value: Value::from(0.2),
            },
        );
        application.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "app-shell".to_string(),
                effect: PermissionLevelV2::Allow,
            },
        );
        let mut workspace = profile("workspace", SettingsScopeKindV2::Workspace, Some("w1"));
        workspace
            .overrides
            .advanced
            .insert("temperature".to_string(), SettingOverrideV2::Clear);
        let mut agent = profile("agent", SettingsScopeKindV2::Agent, Some("a1"));
        agent.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "agent-shell".to_string(),
                effect: PermissionLevelV2::RequireApproval,
            },
        );
        let mut task = profile("task", SettingsScopeKindV2::Task, Some("t1"));
        task.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "task-shell".to_string(),
                effect: PermissionLevelV2::Allow,
            },
        );

        let resolved = resolve_settings_v2(&[application, workspace, agent, task]).unwrap();
        let temperature = resolved.advanced.get("temperature").unwrap();
        assert_eq!(temperature.value, None);
        assert_eq!(temperature.source.kind, SettingsScopeKindV2::Workspace);
        assert_eq!(temperature.source_revision, 1);
        let shell = resolved.permissions.get("shell").unwrap();
        assert_eq!(shell.effect, PermissionLevelV2::RequireApproval);
        assert_eq!(shell.matched_rules.len(), 3);
        assert_eq!(
            shell.matched_rules[2].source.kind,
            SettingsScopeKindV2::Task
        );
    }

    #[test]
    fn rejects_out_of_order_or_duplicate_layers() {
        let task = profile("task", SettingsScopeKindV2::Task, Some("t1"));
        let agent = profile("agent", SettingsScopeKindV2::Agent, Some("a1"));
        assert!(resolve_settings_v2(&[task, agent]).is_err());
        let a1 = profile("a1", SettingsScopeKindV2::Agent, Some("a1"));
        let a2 = profile("a2", SettingsScopeKindV2::Agent, Some("a2"));
        assert!(resolve_settings_v2(&[a1, a2]).is_err());

        let mut task_only = profile("task-only", SettingsScopeKindV2::Task, Some("t1"));
        task_only.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "unsafe-without-parent".to_string(),
                effect: PermissionLevelV2::Allow,
            },
        );
        assert!(resolve_settings_v2(&[task_only]).is_err());

        let application = profile("app-empty-policy", SettingsScopeKindV2::Application, None);
        let mut task = profile("task-policy", SettingsScopeKindV2::Task, Some("t1"));
        task.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "unsafe-without-key-baseline".to_string(),
                effect: PermissionLevelV2::Allow,
            },
        );
        assert!(resolve_settings_v2(&[application, task]).is_err());
    }

    #[test]
    fn pinned_presets_are_immutable_and_user_policy_cannot_relax_a_deny() {
        let preset = builtin_settings_presets_v2()
            .into_iter()
            .find(|preset| preset.id == "chat_only")
            .unwrap();
        let mut application = profile("app", SettingsScopeKindV2::Application, None);
        application.preset = Some(preset.selection());
        let mut task = profile("task", SettingsScopeKindV2::Task, Some("task-1"));
        task.overrides.permissions.insert(
            "shell".to_string(),
            PolicyRuleV2 {
                id: "task-shell-allow".to_string(),
                effect: PermissionLevelV2::Allow,
            },
        );
        let resolved = resolve_settings_v2(&[application.clone(), task]).unwrap();
        let shell = resolved.permissions.get("shell").unwrap();
        assert_eq!(shell.effect, PermissionLevelV2::Deny);
        assert_eq!(shell.matched_rules.len(), 2);
        assert_eq!(shell.matched_rules[0].preset_origin, application.preset);

        application.preset.as_mut().unwrap().content_hash = "tampered".to_string();
        assert!(resolve_settings_v2(&[application]).is_err());
    }

    #[test]
    fn exact_task_grant_satisfies_approval_but_never_deny() {
        let approval = ResolvedPolicyV2 {
            effect: PermissionLevelV2::RequireApproval,
            matched_rules: Vec::new(),
        };
        let grant = TaskPermissionGrantV2 {
            id: "grant-1".to_string(),
            task_id: "task-1".to_string(),
            permission_key: "shell".to_string(),
            resource_selector: "workspace:**".to_string(),
            issuer: "user".to_string(),
            created_at_epoch_ms: 10,
            expires_at_epoch_ms: Some(30),
            scope: TaskPermissionGrantScopeV2::OneShot,
            consumed_at_epoch_ms: None,
        };
        let decision = resolve_permission_v2(
            Some(&approval),
            "shell",
            "workspace:**",
            "task-1",
            Some(&grant),
            20,
        );
        assert_eq!(decision.effect, PermissionLevelV2::Allow);
        assert_eq!(decision.satisfied_by_grant_id.as_deref(), Some("grant-1"));

        let denied = ResolvedPolicyV2 {
            effect: PermissionLevelV2::Deny,
            matched_rules: Vec::new(),
        };
        let decision = resolve_permission_v2(
            Some(&denied),
            "shell",
            "workspace:**",
            "task-1",
            Some(&grant),
            20,
        );
        assert_eq!(decision.effect, PermissionLevelV2::Deny);
        assert_eq!(decision.satisfied_by_grant_id, None);
        assert_eq!(
            resolve_permission_v2(None, "unknown", "workspace:**", "task-1", Some(&grant), 20,)
                .effect,
            PermissionLevelV2::Deny
        );

        let mut consumed = grant;
        consumed.consumed_at_epoch_ms = Some(19);
        assert_eq!(
            resolve_permission_v2(
                Some(&approval),
                "shell",
                "workspace:**",
                "task-1",
                Some(&consumed),
                20,
            )
            .effect,
            PermissionLevelV2::RequireApproval
        );
    }

    #[test]
    fn legacy_projection_redacts_unknown_secrets_and_credential_urls() {
        let sanitized = sanitize_app_config_value(serde_json::json!({
            "futureProvider": {
                "apiKey": "future-secret",
                "sessionToken": "session-secret",
                "secretKey": "key-secret",
                "oauthToken": "oauth-secret",
                "clientToken": "client-secret",
                "apiSecret": "api-secret",
                "xApiKey": "x-api-secret",
                "endpoint": "https://user:password@example.test/v1",
                "nested": [{ "access_token": "token-secret" }]
            }
        }));
        let json = serde_json::to_string(&sanitized).unwrap();
        assert!(!json.contains("future-secret"));
        assert!(!json.contains("session-secret"));
        assert!(!json.contains("key-secret"));
        assert!(!json.contains("oauth-secret"));
        assert!(!json.contains("client-secret"));
        assert!(!json.contains("api-secret"));
        assert!(!json.contains("x-api-secret"));
        assert!(!json.contains("password@example"));
        assert!(!json.contains("token-secret"));
        assert!(json.contains("credentialRef"));
    }

    #[test]
    fn migration_verification_rejects_dropped_structured_fields() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("secret")).unwrap();
        let agent_source = {
            let conn = db.conn();
            read_legacy_agent_configs(&conn, Some("legacy-agent"))
                .unwrap()
                .remove(0)
        };
        let mut agent_profile = agent_source
            .config
            .profile(&agent_source.projection_fingerprint().unwrap());
        agent_profile.overrides.models.remove("text");
        assert!(verify_legacy_agent_profile(&agent_profile, &agent_source.config).is_err());

        let mut config = crate::app_settings::AppConfig::default();
        config.image_generation.provider = "openai".to_string();
        config.image_generation.model = "image-test".to_string();
        db.save_app_config(&config).unwrap();
        let app_source = {
            let conn = db.conn();
            read_legacy_app_config(&conn).unwrap().unwrap()
        };
        let mut app_profile = app_source
            .profile(&app_source.projection_fingerprint().unwrap(), 1)
            .unwrap();
        app_profile
            .overrides
            .capabilities
            .remove("image_generation");
        assert!(verify_legacy_app_profile(&app_profile, &app_source).is_err());
    }

    fn legacy_input(api_key: &str) -> SaveAgentConfigInput {
        SaveAgentConfigInput {
            id: Some("legacy-agent".to_string()),
            name: "Legacy coding".to_string(),
            provider: "openai".to_string(),
            api_key: api_key.to_string(),
            base_url: Some("https://api.example.test/v1".to_string()),
            model: "gpt-test".to_string(),
            provider_endpoint_id: Some("openai.custom".to_string()),
            model_id: Some("gpt-test".to_string()),
            temperature: Some(0.4),
            max_tokens: Some(8192),
            context_window: None,
            is_default: true,
            reasoning_enabled: Some(true),
            thinking_budget: Some(2048),
            reasoning_effort: Some("high".to_string()),
            max_iterations: Some(24),
            summarization_model: Some("gpt-summary".to_string()),
            summarization_provider: Some("openai".to_string()),
            image_generation_model: Some("image-test".to_string()),
            subagent_allowed_tools: Some(vec!["read_file".to_string()]),
            subagent_allowed_skill_ids: Some(vec!["research".to_string()]),
            subagent_max_parallel: Some(3),
            subagent_max_calls_per_turn: Some(8),
            subagent_token_budget: Some(12_000),
            delegation_limits_v2: None,
            tool_timeout_secs: Some(60),
            agent_timeout_secs: Some(600),
        }
    }

    #[test]
    fn migration_is_lossless_secret_free_idempotent_and_reversible() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("sk-do-not-copy"))
            .unwrap();

        let profiles = db.list_settings_profiles_v2().unwrap();
        assert_eq!(profiles.len(), 1);
        let document = serde_json::to_string(&profiles[0]).unwrap();
        assert!(!document.contains("sk-do-not-copy"));
        assert!(document.contains("legacy-agent-config:legacy-agent"));
        assert_eq!(
            profiles[0]
                .overrides
                .advanced
                .get("context_window_override"),
            Some(&SettingOverrideV2::Clear)
        );

        let report = db.migrate_settings_schema_v2().unwrap();
        assert_eq!(report.migrated, 0);
        assert_eq!(report.unchanged, 1);

        let journal: String = db
            .conn()
            .query_row(
                "SELECT source_snapshot_ciphertext FROM settings_schema_migration_journal",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!journal.contains("sk-do-not-copy"));
        assert!(crate::crypto::is_encrypted(&journal));

        assert!(db.rollback_settings_schema_v2().unwrap());
        assert_eq!(db.list_settings_profiles_v2().unwrap().len(), 1);
        assert_eq!(
            db.get_agent_config("legacy-agent").unwrap().api_key,
            "sk-do-not-copy"
        );

        assert!(!db.rollback_settings_schema_v2().unwrap());
        let report = db.migrate_settings_schema_v2().unwrap();
        assert_eq!(report.migrated, 1);
        assert_eq!(db.list_settings_profiles_v2().unwrap().len(), 1);
    }

    #[test]
    fn native_profile_cannot_claim_legacy_ownership() {
        let db = Database::open_memory().unwrap();
        let mut native = profile("native", SettingsScopeKindV2::Workspace, Some("workspace"));
        native = db.save_settings_profile_v2(&native, None).unwrap();
        assert_eq!(
            db.list_settings_profiles_v2().unwrap(),
            vec![native.clone()]
        );

        native.legacy_source = Some(LegacySettingsSourceV2 {
            kind: "agent_config".to_string(),
            id: "source".to_string(),
            migration_key: LEGACY_SETTINGS_MIGRATION_KEY.to_string(),
            source_fingerprint: "fingerprint".to_string(),
            credential_ref: None,
        });
        assert!(db
            .save_settings_profile_v2(&native, Some(native.revision))
            .is_err());
    }

    #[test]
    fn native_profile_writes_use_compare_and_set_revisions() {
        let db = Database::open_memory().unwrap();
        let native = profile("native", SettingsScopeKindV2::Workspace, Some("workspace"));
        let saved = db.save_settings_profile_v2(&native, None).unwrap();
        assert_eq!(saved.revision, 1);

        let mut changed = saved.clone();
        changed.name = "Changed".to_string();
        let saved = db
            .save_settings_profile_v2(&changed, Some(saved.revision))
            .unwrap();
        assert_eq!(saved.revision, 2);
        let error = db.save_settings_profile_v2(&changed, Some(1)).unwrap_err();
        assert!(matches!(error, CoreError::Conflict(_)));

        let duplicate_scope = profile(
            "native-other-id",
            SettingsScopeKindV2::Workspace,
            Some("workspace"),
        );
        let error = db
            .save_settings_profile_v2(&duplicate_scope, None)
            .unwrap_err();
        assert!(matches!(error, CoreError::Conflict(_)));
    }

    #[test]
    fn native_profile_writes_reject_inline_secrets() {
        let db = Database::open_memory().unwrap();
        let mut inline_secret = profile(
            "native-secret",
            SettingsScopeKindV2::Workspace,
            Some("workspace"),
        );
        inline_secret.extensions.insert(
            "provider".to_string(),
            serde_json::json!({
                "authorization": {
                    "credentialRef": "credential:valid-looking",
                    "value": "Bearer must-not-persist"
                }
            }),
        );

        assert!(db.save_settings_profile_v2(&inline_secret, None).is_err());
        let mut raw_reference = profile(
            "native-raw-reference",
            SettingsScopeKindV2::Workspace,
            Some("workspace"),
        );
        raw_reference.overrides.connections.insert(
            "default".to_string(),
            SettingOverrideV2::Set {
                value: ConnectionReferenceV2 {
                    id: "connection-1".to_string(),
                    provider_id: "openai".to_string(),
                    endpoint_id: None,
                    base_url: None,
                    credential_ref: Some("sk-live-secret".to_string()),
                },
            },
        );
        assert!(db.save_settings_profile_v2(&raw_reference, None).is_err());
        let mut disguised_secret = profile(
            "native-disguised-secret",
            SettingsScopeKindV2::Workspace,
            Some("workspace"),
        );
        disguised_secret.extensions.insert(
            "provider".to_string(),
            serde_json::json!({
                "apiKey": {
                    "credentialRef": "legacy-agent-config:sk-live-secret"
                }
            }),
        );
        assert!(db
            .save_settings_profile_v2(&disguised_secret, None)
            .is_err());
        assert!(db.list_settings_profiles_v2().unwrap().is_empty());
    }

    #[test]
    fn native_credential_reference_must_resolve_to_existing_storage() {
        let db = Database::open_memory().unwrap();
        let mut native = profile(
            "native-reference",
            SettingsScopeKindV2::Workspace,
            Some("workspace"),
        );
        native.overrides.connections.insert(
            "default".to_string(),
            SettingOverrideV2::Set {
                value: ConnectionReferenceV2 {
                    id: "connection-1".to_string(),
                    provider_id: "openai".to_string(),
                    endpoint_id: None,
                    base_url: None,
                    credential_ref: Some("legacy-agent-config:missing".to_string()),
                },
            },
        );
        assert!(db.save_settings_profile_v2(&native, None).is_err());

        db.save_agent_config(&legacy_input("secret")).unwrap();
        let SettingOverrideV2::Set { value } =
            native.overrides.connections.get_mut("default").unwrap()
        else {
            unreachable!();
        };
        value.credential_ref = Some("legacy-agent-config:legacy-agent".to_string());
        assert!(db.save_settings_profile_v2(&native, None).is_ok());
    }

    #[test]
    fn semantic_idempotency_keeps_revision_and_original_rollback_snapshot() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("first-secret")).unwrap();
        let first = db.list_settings_profiles_v2().unwrap().remove(0);
        let original_snapshot: String = db
            .conn()
            .query_row(
                "SELECT source_snapshot_ciphertext
                 FROM settings_schema_migration_journal
                 WHERE source_kind = 'agent_config' AND status = 'applied'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        db.save_agent_config(&legacy_input("replacement-secret"))
            .unwrap();
        let second = db.list_settings_profiles_v2().unwrap().remove(0);
        assert_eq!(second.revision, first.revision);
        let preserved_snapshot: String = db
            .conn()
            .query_row(
                "SELECT source_snapshot_ciphertext
                 FROM settings_schema_migration_journal
                 WHERE source_kind = 'agent_config' AND status = 'applied'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_snapshot, original_snapshot);
        let journal_count: u32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM settings_schema_migration_journal
                 WHERE source_kind = 'agent_config' AND status = 'applied'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(journal_count, 1);

        assert!(db.rollback_settings_schema_v2().unwrap());
        assert_eq!(
            db.get_agent_config("legacy-agent").unwrap().api_key,
            "first-secret"
        );
    }

    #[test]
    fn missing_snapshot_blocks_rollback_without_flipping_active_version() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("secret")).unwrap();
        db.conn()
            .execute(
                "DELETE FROM settings_schema_migration_journal
                 WHERE source_kind = 'agent_config'",
                [],
            )
            .unwrap();

        assert!(db.rollback_settings_schema_v2().is_err());
        assert_eq!(db.settings_schema_state_v2().unwrap().active_version, 2);
        assert_eq!(
            db.get_agent_config("legacy-agent").unwrap().api_key,
            "secret"
        );
    }

    #[test]
    fn deleted_agent_is_not_resurrected_by_rollback_or_remigration() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("secret")).unwrap();
        db.delete_agent_config("legacy-agent").unwrap();
        assert!(db.rollback_settings_schema_v2().unwrap());
        assert!(db.get_agent_config("legacy-agent").is_err());

        db.save_agent_config(&legacy_input("replacement")).unwrap();
        db.migrate_settings_schema_v2().unwrap();
        assert!(db.rollback_settings_schema_v2().unwrap());
        db.delete_agent_config("legacy-agent").unwrap();
        assert_eq!(db.list_settings_profiles_v2().unwrap().len(), 1);
        let report = db.migrate_settings_schema_v2().unwrap();
        assert_eq!(report.removed_orphans, 1);
        assert!(db.list_settings_profiles_v2().unwrap().is_empty());
        assert!(db.get_agent_config("legacy-agent").is_err());
    }

    #[test]
    fn app_config_migration_redacts_secrets_and_rollback_preserves_sidecar() {
        let db = Database::open_memory().unwrap();
        let mut config = crate::app_settings::AppConfig::default();
        config.image_generation.api_key = "image-secret".to_string();
        config.text_to_speech.api_key = "speech-secret".to_string();
        config.speech_to_text.api_key = "transcription-secret".to_string();
        db.save_app_config(&config).unwrap();

        let application = db
            .list_settings_profiles_v2()
            .unwrap()
            .into_iter()
            .find(|profile| profile.scope.kind == SettingsScopeKindV2::Application)
            .unwrap();
        let document = serde_json::to_string(&application).unwrap();
        for secret in ["image-secret", "speech-secret", "transcription-secret"] {
            assert!(!document.contains(secret));
        }
        assert!(document.contains("legacy-app-config:imageGeneration"));
        assert_eq!(db.settings_schema_state_v2().unwrap().active_version, 2);

        let original_row: (String, String) = db
            .conn()
            .query_row(
                "SELECT value, updated_at FROM app_config WHERE key = 'app_config'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE app_config
                 SET value = '{}', updated_at = '2099-01-01 00:00:00'
                 WHERE key = 'app_config'",
                [],
            )
            .unwrap();

        assert!(db.rollback_settings_schema_v2().unwrap());
        assert_eq!(db.settings_schema_state_v2().unwrap().active_version, 1);
        assert!(db
            .list_settings_profiles_v2()
            .unwrap()
            .iter()
            .any(|profile| profile.scope.kind == SettingsScopeKindV2::Application));
        let restored = db.load_app_config().unwrap();
        assert_eq!(restored.image_generation.api_key, "image-secret");
        assert_eq!(restored.text_to_speech.api_key, "speech-secret");
        assert_eq!(restored.speech_to_text.api_key, "transcription-secret");
        let restored_row: (String, String) = db
            .conn()
            .query_row(
                "SELECT value, updated_at FROM app_config WHERE key = 'app_config'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(restored_row, original_row);
        let app_journal_status: String = db
            .conn()
            .query_row(
                "SELECT status FROM settings_schema_migration_journal
                 WHERE source_kind = 'app_config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(app_journal_status, "rolled_back");
    }

    #[test]
    fn corrupted_snapshot_blocks_rollback_without_flipping_active_version() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("secret")).unwrap();
        db.conn()
            .execute(
                "UPDATE settings_schema_migration_journal
                 SET source_snapshot_ciphertext = 'enc:v1:corrupt'
                 WHERE source_kind = 'agent_config'",
                [],
            )
            .unwrap();

        assert!(db.rollback_settings_schema_v2().is_err());
        assert_eq!(db.settings_schema_state_v2().unwrap().active_version, 2);
        assert_eq!(
            db.get_agent_config("legacy-agent").unwrap().api_key,
            "secret"
        );
    }

    #[test]
    fn failed_preflight_keeps_v1_active_without_partial_profiles() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::migrations::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO app_config (key, value) VALUES ('app_config', '{invalid');",
        )
        .unwrap();

        assert!(sync_legacy_agent_configs(&mut conn, None, false).is_err());
        let active_version: u32 = conn
            .query_row(
                "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let profile_count: u32 = conn
            .query_row("SELECT COUNT(*) FROM settings_profiles_v2", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(active_version, 1);
        assert_eq!(profile_count, 0);
    }

    #[test]
    fn legacy_write_and_v2_projection_roll_back_together_on_sync_failure() {
        let db = Database::open_memory().unwrap();
        db.save_agent_config(&legacy_input("secret")).unwrap();
        let original_profile = db.list_settings_profiles_v2().unwrap().remove(0);

        let mut oversized = legacy_input("replacement-secret");
        oversized.name = "Must not persist".to_string();
        oversized.subagent_allowed_tools = Some(
            (0..8_000)
                .map(|index| format!("tool-with-a-long-name-{index}"))
                .collect(),
        );
        assert!(db.save_agent_config(&oversized).is_err());

        let restored = db.get_agent_config("legacy-agent").unwrap();
        assert_eq!(restored.name, "Legacy coding");
        assert_eq!(restored.api_key, "secret");
        assert_eq!(
            db.list_settings_profiles_v2().unwrap(),
            vec![original_profile]
        );
        assert_eq!(db.settings_schema_state_v2().unwrap().active_version, 2);
    }
}
