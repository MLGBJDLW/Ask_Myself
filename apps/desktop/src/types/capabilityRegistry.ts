import type { ModelDescriptor } from '../lib/modelCatalog';
import type { SettingsScopeV2 } from './settingsSchemaV2';

export interface RegistryScope {
  workspaceId?: string;
  agentId?: string;
  taskId?: string;
}

export type ConnectionHealth = 'unknown' | 'configured' | 'missing' | 'invalid' | 'expired';
export type TargetAvailability = 'unknown' | 'unavailable' | 'discoverable' | 'callable' | 'product_ready';
export type RegistryReadMode = 'legacy' | 'registry';

export interface ConnectionRecord {
  schemaVersion: number;
  id: string;
  revision: number;
  adapterProviderId: string;
  providerId: string;
  endpointId: string;
  baseUrl: string;
  endpointFingerprint: string;
  credentialRef?: string;
  enabled: boolean;
  health: ConnectionHealth;
  source: SettingsScopeV2;
  sourceRevision: number;
}

export interface ModelDefinitionRecord {
  id: string;
  revision: number;
  descriptorHash: string;
  descriptor: ModelDescriptor;
}

export interface ModelTargetRecord {
  id: string;
  revision: number;
  connectionId: string;
  modelDefinitionId?: string;
  upstreamModelId: string;
  availability: TargetAvailability;
  source: SettingsScopeV2;
  sourceRevision: number;
}

export interface CapabilityEligibility {
  eligible: boolean;
  reasonCodes: string[];
}

export interface ResolvedCapabilityRouteTarget {
  target: ModelTargetRecord;
  connection: ConnectionRecord;
  definition?: ModelDefinitionRecord;
  eligibility: CapabilityEligibility;
}

export interface ResolvedCapabilityRoute {
  capabilityId: string;
  source: SettingsScopeV2;
  sourceRevision: number;
  primary?: ResolvedCapabilityRouteTarget;
  fallbacks: ResolvedCapabilityRouteTarget[];
}

export interface RegistryActivationRecord {
  capabilityId: string;
  scope: SettingsScopeV2;
  readMode: RegistryReadMode;
  registryRevision: number;
  parityStatus: string;
  parity: unknown;
  activatedAt?: string;
  rolledBackAt?: string;
}

export interface CapabilityRegistryProjection {
  schemaVersion: number;
  settingsRevisions: Array<{
    profileId: string;
    scope: SettingsScopeV2;
    revision: number;
  }>;
  connections: ConnectionRecord[];
  modelDefinitions: ModelDefinitionRecord[];
  modelTargets: ModelTargetRecord[];
  capabilities: ResolvedCapabilityRoute[];
  activations: RegistryActivationRecord[];
}
