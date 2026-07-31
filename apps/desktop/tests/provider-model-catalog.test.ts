import {
  bindProviderModelCatalogCredential,
  catalogMatchesProvider,
  isProviderModelCatalogStale,
  loadProviderModelCatalog,
  providerModelCatalogCacheKey,
  saveProviderModelCatalog,
  type ProviderModelCatalogSnapshot,
} from '../src/lib/providerModelCatalog';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  assert(Object.is(actual, expected), `${message}: expected ${String(expected)}, got ${String(actual)}`);
}

function assertDeepEqual(actual: unknown, expected: unknown, message: string): void {
  assert(JSON.stringify(actual) === JSON.stringify(expected), message);
}

function snapshot(): ProviderModelCatalogSnapshot {
  return {
    provider: 'alibaba_model_studio',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1/',
    refreshedAt: '2026-07-31T08:00:00Z',
    liveDiscoverySucceeded: true,
    models: [
      {
        id: 'account-model',
        name: 'account-model',
        recommended: false,
        source: 'discovered',
        status: 'active',
        regions: ['cn-beijing'],
        lastVerifiedAt: '2026-07-31T08:00:00Z',
        modalities: ['text'],
        supportsTools: false,
        supportsStructuredOutput: false,
        reasoningEfforts: [],
      },
    ],
  };
}

function testCatalogCache(): void {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
  };
  const value = snapshot();
  const apiKey = 'account-one-secret';
  const boundValue = bindProviderModelCatalogCredential(value, apiKey);

  saveProviderModelCatalog(boundValue, apiKey, storage);

  assertEqual(
    providerModelCatalogCacheKey(value.provider, value.baseUrl, apiKey),
    providerModelCatalogCacheKey(value.provider, value.baseUrl?.replace(/\/$/, ''), apiKey),
    'cache key should normalize a trailing slash',
  );
  assertDeepEqual(
    loadProviderModelCatalog(value.provider, value.baseUrl?.replace(/\/$/, ''), apiKey, storage),
    boundValue,
    'catalog should restore the cached live snapshot',
  );
  assertEqual(
    loadProviderModelCatalog('open_ai', value.baseUrl, apiKey, storage),
    null,
    'catalog cache should be isolated by provider',
  );
  assertEqual(
    loadProviderModelCatalog(value.provider, value.baseUrl, 'account-two-secret', storage),
    null,
    'catalog cache should be isolated by credential',
  );
}

function testCatalogFreshness(): void {
  const value = snapshot();
  const boundValue = bindProviderModelCatalogCredential(value, 'account-secret');
  assertEqual(
    catalogMatchesProvider(boundValue, 'alibaba_model_studio', value.baseUrl, 'account-secret'),
    true,
    'matching provider and endpoint should be accepted',
  );
  assertEqual(
    catalogMatchesProvider(boundValue, 'open_ai', value.baseUrl, 'account-secret'),
    false,
    'different providers should not share snapshots',
  );
  assertEqual(
    isProviderModelCatalogStale(value, Date.parse('2026-08-01T07:59:59Z')),
    false,
    'snapshot should remain fresh through the TTL',
  );
  assertEqual(
    isProviderModelCatalogStale(value, Date.parse('2026-08-01T08:00:01Z')),
    true,
    'snapshot should become stale after the TTL',
  );
}

testCatalogCache();
testCatalogFreshness();
