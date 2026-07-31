import type { ProviderModelPreset, ReasoningEffortLevel } from './providerTypes';

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
}

interface StoredProviderModelCatalog {
  version: 1;
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
): string {
  return `${CACHE_PREFIX}${encodeURIComponent(`${provider.trim().toLowerCase()}::${normalizeBaseUrl(baseUrl)}`)}`;
}

export function catalogMatchesProvider(
  snapshot: ProviderModelCatalogSnapshot,
  provider: string,
  baseUrl: string | null | undefined,
): boolean {
  return snapshot.provider.trim().toLowerCase() === provider.trim().toLowerCase()
    && normalizeBaseUrl(snapshot.baseUrl) === normalizeBaseUrl(baseUrl);
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
  storage: Pick<Storage, 'getItem'> = localStorage,
): ProviderModelCatalogSnapshot | null {
  try {
    const raw = storage.getItem(providerModelCatalogCacheKey(provider, baseUrl));
    if (!raw) return null;
    const stored = JSON.parse(raw) as Partial<StoredProviderModelCatalog>;
    const snapshot = stored.version === 1 ? stored.snapshot : null;
    if (!snapshot || !Array.isArray(snapshot.models) || !catalogMatchesProvider(snapshot, provider, baseUrl)) {
      return null;
    }
    return snapshot;
  } catch {
    return null;
  }
}

export function saveProviderModelCatalog(
  snapshot: ProviderModelCatalogSnapshot,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  try {
    storage.setItem(
      providerModelCatalogCacheKey(snapshot.provider, snapshot.baseUrl),
      JSON.stringify({ version: 1, snapshot } satisfies StoredProviderModelCatalog),
    );
  } catch {
    // A fresh in-memory catalog remains usable even when browser storage is
    // unavailable or full.
  }
}
