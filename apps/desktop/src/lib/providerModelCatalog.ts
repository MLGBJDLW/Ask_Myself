import type { ProviderModelPreset, ReasoningEffortLevel } from './providerTypes';
import { credentialFingerprint } from './credentialFingerprint';

export interface ProviderModelCatalogEntry extends ProviderModelPreset {
  source: 'official' | 'discovered' | 'curated';
  status: 'active' | 'preview' | 'legacy' | 'deprecated' | 'removed';
  recommended: boolean;
  regions: string[];
  lastVerifiedAt: string | null;
  modalities: string[];
  supportsTools: boolean | null;
  supportsStructuredOutput: boolean | null;
  reasoningEfforts: ReasoningEffortLevel[];
}

export interface ProviderModelCatalogSnapshot {
  provider: string;
  baseUrl: string | null;
  models: ProviderModelCatalogEntry[];
  refreshedAt: string;
  liveDiscoverySucceeded: boolean;
  credentialFingerprint?: string;
}

interface StoredProviderModelCatalog {
  version: 2;
  snapshot: ProviderModelCatalogSnapshot;
}

export const PROVIDER_MODEL_CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
const CACHE_PREFIX = 'nexa-provider-model-catalog-v1:';

function normalizeBaseUrl(value: string | null | undefined): string {
  return (value ?? '').trim().replace(/\/+$/, '').toLowerCase();
}

export function providerModelCatalogCacheKey(
  provider: string,
  baseUrl: string | null | undefined,
  apiKey: string,
): string {
  return `${CACHE_PREFIX}${encodeURIComponent(`${provider.trim().toLowerCase()}::${normalizeBaseUrl(baseUrl)}::${credentialFingerprint(apiKey)}`)}`;
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
    && normalizeBaseUrl(snapshot.baseUrl) === normalizeBaseUrl(baseUrl)
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
    const snapshot = stored.version === 2 ? stored.snapshot : null;
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
      JSON.stringify({ version: 2, snapshot: boundSnapshot } satisfies StoredProviderModelCatalog),
    );
  } catch {
    // A fresh in-memory catalog remains usable even when browser storage is
    // unavailable or full.
  }
}
