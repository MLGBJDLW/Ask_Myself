export type SettingsScopeKindV2 = 'application' | 'workspace' | 'agent' | 'task';

export interface SettingsScopeV2 {
  kind: SettingsScopeKindV2;
  id?: string | null;
}

export interface PresetSelectionV2 {
  id: string;
  version: number;
  contentHash: string;
}

export type SettingOverrideV2<T> =
  | { mode: 'set'; value: T }
  | { mode: 'clear' };

export interface ConnectionReferenceV2 {
  id: string;
  providerId: string;
  endpointId?: string | null;
  baseUrl?: string | null;
  credentialRef?: string | null;
}

export interface ModelReferenceV2 {
  providerId: string;
  endpointId?: string | null;
  modelId: string;
}

export interface CapabilityBindingV2 {
  primary?: ModelReferenceV2 | null;
  fallbacks?: ModelReferenceV2[];
  options?: Record<string, unknown>;
}

export type PermissionLevelV2 = 'allow' | 'require_approval' | 'deny';

export interface PolicyRuleV2 {
  id: string;
  effect: PermissionLevelV2;
}

export interface SettingsOverridesV2 {
  connections: Record<string, SettingOverrideV2<ConnectionReferenceV2>>;
  models: Record<string, SettingOverrideV2<ModelReferenceV2>>;
  capabilities: Record<string, SettingOverrideV2<CapabilityBindingV2>>;
  permissions: Record<string, PolicyRuleV2>;
  advanced: Record<string, SettingOverrideV2<unknown>>;
}

export interface LegacySettingsSourceV2 {
  kind: string;
  id: string;
  migrationKey: string;
  sourceFingerprint: string;
  credentialRef?: string | null;
}

export interface SettingsProfileV2 {
  schemaVersion: 2;
  revision: number;
  id: string;
  name: string;
  scope: SettingsScopeV2;
  preset?: PresetSelectionV2 | null;
  overrides: SettingsOverridesV2;
  legacySource?: LegacySettingsSourceV2 | null;
  extensions?: Record<string, unknown>;
}

export interface SettingsSchemaStateV2 {
  activeVersion: 1 | 2;
  migrationId?: string | null;
  activatedAt?: string | null;
}

export interface SettingsMigrationReportV2 {
  migrated: number;
  unchanged: number;
  skippedRolledBack: number;
  removedOrphans: number;
}
