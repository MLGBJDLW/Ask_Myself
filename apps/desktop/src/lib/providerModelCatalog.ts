import type { ProviderModelPreset, ReasoningEffortLevel } from './providerTypes';
import { credentialFingerprint } from './credentialFingerprint';
import {
  attachModelDescriptors,
  inferModelCatalogRegion,
  modelEndpointId,
  normalizeModelEndpointUrl,
  type ModelDescriptor,
  type ModelModality,
} from './modelCatalog';

export interface ProviderModelCatalogEntry extends Omit<ProviderModelPreset, 'descriptor'> {
  source: 'official' | 'discovered' | 'curated';
  status: 'active' | 'preview' | 'gated' | 'legacy' | 'deprecated' | 'removed';
  recommended: boolean;
  regions: string[];
  lastVerifiedAt: string | null;
  modalities: ModelModality[];
  supportsTools: boolean | null;
  supportsStructuredOutput: boolean | null;
  reasoningEfforts: ReasoningEffortLevel[];
}

export interface ProviderModelCatalogSnapshot {
  schemaVersion?: number;
  provider: string;
  baseUrl: string | null;
  endpointId?: string;
  models: ProviderModelCatalogEntry[];
  descriptors?: ModelDescriptor[];
  tombstones?: ModelDescriptor[];
  refreshedAt: string;
  liveDiscoverySucceeded: boolean;
  capabilityProbeSucceeded?: boolean;
  credentialFingerprint?: string;
}

interface StoredProviderModelCatalog {
  version: 2 | 3;
  snapshot: ProviderModelCatalogSnapshot;
}

export const PROVIDER_MODEL_CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
const CACHE_PREFIX = 'nexa-provider-model-catalog-v1:';

export function providerModelCatalogCacheKey(
  provider: string,
  baseUrl: string | null | undefined,
  apiKey: string,
): string {
  return `${CACHE_PREFIX}${encodeURIComponent(`${provider.trim().toLowerCase()}::${normalizeModelEndpointUrl(baseUrl)}::${credentialFingerprint(apiKey)}`)}`;
}

export function bindProviderModelCatalogCredential(
  snapshot: ProviderModelCatalogSnapshot,
  apiKey: string,
): ProviderModelCatalogSnapshot {
  return { ...snapshot, credentialFingerprint: credentialFingerprint(apiKey) };
}

export function catalogMatchesProvider(
  snapshot: ProviderModelCatalogSnapshot,
  provider: string,
  baseUrl: string | null | undefined,
  apiKey: string,
): boolean {
  return snapshot.provider.trim().toLowerCase() === provider.trim().toLowerCase()
    && normalizeModelEndpointUrl(snapshot.baseUrl) === normalizeModelEndpointUrl(baseUrl)
    && snapshot.credentialFingerprint === credentialFingerprint(apiKey);
}

export function isProviderModelCatalogStale(
  snapshot: ProviderModelCatalogSnapshot,
  now = Date.now(),
): boolean {
  const refreshedAt = Date.parse(snapshot.refreshedAt);
  return !Number.isFinite(refreshedAt) || now - refreshedAt > PROVIDER_MODEL_CATALOG_TTL_MS;
}

export function loadProviderModelCatalog(
  provider: string,
  baseUrl: string | null | undefined,
  apiKey: string,
  storage: Pick<Storage, 'getItem'> = localStorage,
): ProviderModelCatalogSnapshot | null {
  try {
    const raw = storage.getItem(providerModelCatalogCacheKey(provider, baseUrl, apiKey));
    if (!raw) return null;
    const stored = JSON.parse(raw) as Partial<StoredProviderModelCatalog>;
    const snapshot = stored.version === 2 || stored.version === 3 ? stored.snapshot : null;
    if (!snapshot || !Array.isArray(snapshot.models) || !catalogMatchesProvider(snapshot, provider, baseUrl, apiKey)) {
      return null;
    }
    return snapshot;
  } catch {
    return null;
  }
}

export function saveProviderModelCatalog(
  snapshot: ProviderModelCatalogSnapshot,
  apiKey: string,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  try {
    const boundSnapshot = bindProviderModelCatalogCredential(snapshot, apiKey);
    storage.setItem(
      providerModelCatalogCacheKey(snapshot.provider, snapshot.baseUrl, apiKey),
      JSON.stringify({ version: 3, snapshot: boundSnapshot } satisfies StoredProviderModelCatalog),
    );
  } catch {
    // A fresh in-memory catalog remains usable even when browser storage is
    // unavailable or full.
  }
}

export function catalogModelsForSnapshot(
  snapshot: ProviderModelCatalogSnapshot,
): ProviderModelPreset[] {
  if (snapshot.descriptors?.length) {
    return snapshot.descriptors.map((descriptor) => {
      const legacy = snapshot.models.find(
        (candidate) => candidate.id.trim().toLowerCase() === descriptor.id.trim().toLowerCase(),
      );
      return {
        ...legacy,
        id: descriptor.id,
        name: legacy?.name ?? descriptor.displayName,
        recommended: descriptor.recommended,
        source: descriptor.source,
        status: descriptor.lifecycle,
        regions: descriptor.regions,
        lastVerifiedAt: descriptor.lastVerifiedAt,
        modalities: [...new Set([...descriptor.inputModalities, ...descriptor.outputModalities])],
        supportsTools: descriptor.capabilities.toolCalling ?? null,
        supportsStructuredOutput: descriptor.capabilities.structuredOutput ?? null,
        reasoningEfforts: (descriptor.capabilities.reasoning?.effortLevels ?? []) as ReasoningEffortLevel[],
        descriptor,
      };
    });
  }

  const endpointId = snapshot.endpointId ?? modelEndpointId('text', snapshot.provider);
  return attachModelDescriptors(snapshot.models, {
    surface: 'text',
    providerId: snapshot.provider,
    endpointId,
    region: inferModelCatalogRegion(snapshot.baseUrl),
    apiStyle: 'openai_chat',
  }) as ProviderModelPreset[];
}
