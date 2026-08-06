//! Versioned, inheritable settings documents and lossless legacy migration.
//!
//! The V2 document is deliberately a shadow of the current `agent_configs`
//! runtime source. Migration never deletes or rewrites legacy rows, and V2
//! stores only a reference to credentials rather than secret material. This
//! lets later registry work adopt the schema incrementally and makes rollback
//! a metadata operation instead of a lossy reverse transformation.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
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
            }
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
    pub issuer: String,
    pub created_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_ms: Option<i64>,
    #[serde(default)]
    pub one_shot: bool,
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
            && grant.created_at_epoch_ms <= now_epoch_ms
            && grant
                .expires_at_epoch_ms
                .is_none_or(|expires_at| expires_at >= now_epoch_ms)
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
    let mut previous_rank = None;
    let mut resolved = ResolvedSettingsV2::default();
    let presets = builtin_settings_presets_v2();
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
        if let Some(selection) = &profile.preset {
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
            apply_profile_overrides(
                &mut resolved,
                &definition.patch,
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
    matches!(
        key.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .as_str(),
        "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "bearertoken"
            | "clientsecret"
            | "password"
            | "privatekey"
    )
}

fn is_endpoint_field(key: &str) -> bool {
    matches!(key, "baseUrl" | "base_url" | "endpoint" | "url")
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
    let mut report = SettingsMigrationReportV2::default();
    let migration_run_id = Uuid::new_v4().to_string();

    for source in sources {
        let snapshot = &source.config;
        let fingerprint = source.source_hash()?;
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
                   AND source_id = ?2 AND source_fingerprint = ?3",
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

        let round_trip = profile.extensions.get("legacyV1").ok_or_else(|| {
            CoreError::Internal("Migrated settings profile lost legacyV1 extension".to_string())
        })?;
        let expected_projection =
            sanitize_legacy_value(serde_json::to_value(snapshot)?, "legacy-agent-config");
        if round_trip != &expected_projection {
            return Err(CoreError::Internal(format!(
                "Settings V1 -> V2 -> V1 verification failed for {}",
                snapshot.id
            )));
        }

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
             ) VALUES (?1, ?2, ?3, 'agent_config', ?4, ?5, ?6, 'applied', ?7, ?8, ?9, 1)
             ON CONFLICT(migration_key, source_kind, source_id, source_fingerprint)
             DO UPDATE SET
                 migration_run_id = excluded.migration_run_id,
                 target_profile_id = excluded.target_profile_id,
                 status = 'applied',
                 source_snapshot_ciphertext = excluded.source_snapshot_ciphertext,
                 source_hash = excluded.source_hash,
                 target_hash = excluded.target_hash,
                 round_trip_verified = 1,
                 applied_at = datetime('now'),
                 rolled_back_at = NULL",
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
        let fingerprint = source.source_hash()?;
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
                   AND source_id = ?2 AND source_fingerprint = ?3",
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
            let sanitized = sanitize_app_config_value(serde_json::from_str(&source.value)?);
            if profile.extensions.get("legacyV1") != Some(&sanitized) {
                return Err(CoreError::Internal(
                    "Settings app_config V1 -> V2 -> V1 verification failed".to_string(),
                ));
            }
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
            transaction.execute(
                "INSERT INTO settings_schema_migration_journal (
                     id, migration_run_id, migration_key, source_kind, source_id,
                     source_fingerprint, target_profile_id, status,
                     source_snapshot_ciphertext, source_hash, target_hash,
                     round_trip_verified
                 ) VALUES (?1, ?2, ?3, 'app_config', ?4, ?5, ?6, 'applied', ?7, ?5, ?8, 1)
                 ON CONFLICT(migration_key, source_kind, source_id, source_fingerprint)
                 DO UPDATE SET
                     migration_run_id = excluded.migration_run_id,
                     target_profile_id = excluded.target_profile_id,
                     status = 'applied',
                     source_snapshot_ciphertext = excluded.source_snapshot_ciphertext,
                     source_hash = excluded.source_hash,
                     target_hash = excluded.target_hash,
                     round_trip_verified = 1,
                     applied_at = datetime('now'),
                     rolled_back_at = NULL",
                params![
                    Uuid::new_v4().to_string(),
                    &migration_run_id,
                    LEGACY_SETTINGS_MIGRATION_KEY,
                    &source.key,
                    &fingerprint,
                    &profile.id,
                    &source_snapshot_ciphertext,
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

    transaction.commit()?;
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

pub(crate) fn sync_legacy_agent_config(
    conn: &mut Connection,
    source_id: &str,
) -> Result<SettingsMigrationReportV2, CoreError> {
    let active_version: u32 = conn.query_row(
        "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| row.get(0),
    )?;
    if active_version != SETTINGS_SCHEMA_VERSION_V2 {
        return Ok(SettingsMigrationReportV2::default());
    }
    sync_legacy_agent_configs(conn, Some(source_id), false)
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
    }
    Ok(())
}

pub(crate) fn sync_legacy_app_config(
    conn: &mut Connection,
) -> Result<SettingsMigrationReportV2, CoreError> {
    let active_version: u32 = conn.query_row(
        "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
        [],
        |row| row.get(0),
    )?;
    if active_version != SETTINGS_SCHEMA_VERSION_V2 {
        return Ok(SettingsMigrationReportV2::default());
    }
    sync_legacy_agent_configs(conn, None, false)
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
        let active_version: u32 = transaction.query_row(
            "SELECT active_version FROM settings_schema_state WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )?;
        if active_version != SETTINGS_SCHEMA_VERSION_V2 {
            return Ok(false);
        }
        let snapshots = {
            let mut statement = transaction.prepare(
                "SELECT id, source_kind, source_snapshot_ciphertext, source_hash
                 FROM settings_schema_migration_journal
                 WHERE migration_key = ?1 AND source_kind = 'agent_config'
                   AND status = 'applied'
                 ORDER BY source_id",
            )?;
            let rows = statement.query_map(params![LEGACY_SETTINGS_MIGRATION_KEY], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (_journal_id, source_kind, ciphertext, expected_hash) in &snapshots {
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
             WHERE migration_key = ?1 AND source_kind = 'agent_config'
               AND status = 'applied'",
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
        let scope_id = profile.scope.id.as_deref().unwrap_or("");
        let conn = self.conn();
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
        assert_eq!(shell.matched_rules.len(), 2);
        assert_eq!(
            shell.matched_rules[1].source.kind,
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
            issuer: "user".to_string(),
            created_at_epoch_ms: 10,
            expires_at_epoch_ms: Some(30),
            one_shot: true,
        };
        let decision = resolve_permission_v2(Some(&approval), "shell", "task-1", Some(&grant), 20);
        assert_eq!(decision.effect, PermissionLevelV2::Allow);
        assert_eq!(decision.satisfied_by_grant_id.as_deref(), Some("grant-1"));

        let denied = ResolvedPolicyV2 {
            effect: PermissionLevelV2::Deny,
            matched_rules: Vec::new(),
        };
        let decision = resolve_permission_v2(Some(&denied), "shell", "task-1", Some(&grant), 20);
        assert_eq!(decision.effect, PermissionLevelV2::Deny);
        assert_eq!(decision.satisfied_by_grant_id, None);
        assert_eq!(
            resolve_permission_v2(None, "unknown", "task-1", Some(&grant), 20).effect,
            PermissionLevelV2::Deny
        );
    }

    #[test]
    fn legacy_projection_redacts_unknown_secrets_and_credential_urls() {
        let sanitized = sanitize_app_config_value(serde_json::json!({
            "futureProvider": {
                "apiKey": "future-secret",
                "endpoint": "https://user:password@example.test/v1",
                "nested": [{ "access_token": "token-secret" }]
            }
        }));
        let json = serde_json::to_string(&sanitized).unwrap();
        assert!(!json.contains("future-secret"));
        assert!(!json.contains("password@example"));
        assert!(!json.contains("token-secret"));
        assert!(json.contains("credentialRef"));
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
}
