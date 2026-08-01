import {
  attachModelDescriptors,
  catalogEndpointIdForSelection,
  endpointIdForSavedSelection,
  isImplicitDefaultEligible,
  modelDescriptorFacts,
  normalizeModelEndpointUrl,
  resolveExplicitModelSelection,
  selectImplicitDefault,
  type ModelDescriptor,
} from '../src/lib/modelCatalog';
import { providerModelCatalogCacheKey } from '../src/lib/providerModelCatalog';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  assert(Object.is(actual, expected), `${message}: expected ${String(expected)}, got ${String(actual)}`);
}

function descriptor(overrides: Partial<ModelDescriptor>): ModelDescriptor {
  return {
    schemaVersion: 2,
    id: 'model',
    aliases: [],
    displayName: 'Model',
    providerId: 'provider',
    family: 'model',
    version: null,
    lifecycle: 'active',
    access: 'public',
    regions: ['global'],
    endpointIds: ['text:provider'],
    endpointKinds: ['text'],
    inputModalities: ['text'],
    outputModalities: ['text'],
    capabilities: {},
    limits: {},
    pricingRef: null,
    releaseDate: null,
    deprecationDate: null,
    replacementModelId: null,
    source: 'official',
    lastVerifiedAt: '2026-08-01',
    productReadiness: 'product_ready',
    availableToCredential: true,
    recommended: false,
    ...overrides,
  };
}

function testImplicitDefaultsRespectLifecycleAccessAndReadiness(): void {
  const models = [
    { id: 'preview', descriptor: descriptor({ id: 'preview', lifecycle: 'preview', recommended: true }) },
    { id: 'gated', descriptor: descriptor({ id: 'gated', lifecycle: 'gated', recommended: true }) },
    { id: 'known', descriptor: descriptor({ id: 'known', productReadiness: 'known', recommended: true }) },
    { id: 'ready', descriptor: descriptor({ id: 'ready' }) },
  ];

  assertEqual(selectImplicitDefault(models)?.id, 'ready', 'only the safe product-ready model should default');
  assertEqual(isImplicitDefaultEligible(models[0].descriptor), false, 'preview must be explicit');
}

function testLegacyImagePresetProjectsCanonicalMetadata(): void {
  const [model] = attachModelDescriptors(
    [{
      id: 'qwen-image-3.0-pro',
      name: 'Qwen Image 3.0 Pro (Limited Preview)',
      recommended: false,
      source: 'official',
      status: 'preview',
      access: 'application',
      productReadiness: 'known',
    }],
    {
      surface: 'image',
      providerId: 'alibaba_model_studio',
      endpointId: 'image:qwen-dashscope-cn',
      region: 'cn-beijing',
      apiStyle: 'dashscope_multimodal',
      outputFormats: ['png'],
      supportedSizes: ['2048x2048'],
    },
  );

  assertEqual(model.descriptor.lifecycle, 'preview', 'lifecycle should survive projection');
  assertEqual(model.descriptor.access, 'application', 'access should survive projection');
  assertEqual(model.descriptor.outputModalities[0], 'image', 'image models should output image');
  assertEqual(model.descriptor.capabilities.imageGeneration, true, 'surface capability should be explicit');
  assertEqual(model.descriptor.limits.outputFormats?.[0], 'png', 'provider limits should project');
}

function testRecommendationDoesNotFabricateProductReadiness(): void {
  const [model] = attachModelDescriptors(
    [{ id: 'unverified', name: 'Unverified', recommended: true }],
    { surface: 'text', providerId: 'provider', endpointId: 'text:provider' },
  );

  assertEqual(model.descriptor.productReadiness, 'known', 'recommendation is not readiness evidence');
  assertEqual(isImplicitDefaultEligible(model.descriptor), false, 'unverified models must not default');
}

function testSettingsFactsExposeLifecycleAccessCapabilitiesAndAvailability(): void {
  const facts = modelDescriptorFacts(descriptor({
    lifecycle: 'deprecated',
    access: 'application',
    inputModalities: ['text', 'image'],
    outputModalities: ['image'],
    capabilities: { toolCalling: true, reasoning: { effortLevels: ['high'] }, asyncJobs: true },
    replacementModelId: 'model-v2',
    availableToCredential: false,
  }));

  for (const expected of [
    'lifecycle:deprecated',
    'readiness:product_ready',
    'access:application',
    'region:global',
    'io:text+image→image',
    'tools:true',
    'reasoning:true',
    'realtime:false',
    'async:true',
    'source:official',
    'verified:2026-08-01',
    'replacement:model-v2',
    'credential:unavailable',
  ]) {
    assert(facts.includes(expected), `settings facts should include ${expected}`);
  }
}

function testEndpointIdentityRequiresAnExactBaseUrlMatch(): void {
  const selected = descriptor({ endpointIds: ['text:openai'] });
  assertEqual(
    catalogEndpointIdForSelection(
      selected,
      'https://api.openai.com/v1',
      'https://api.openai.com/v1/',
    ),
    'text:openai',
    'normalized official URLs should retain the stable catalog endpoint',
  );
  assertEqual(
    catalogEndpointIdForSelection(
      selected,
      'https://api.openai.com/v1',
      'https://custom.example.test/v1',
    ),
    null,
    'a custom URL must not inherit the public OpenAI endpoint identity',
  );
  assert(
    providerModelCatalogCacheKey('open_ai', 'https://example.test/TenantA', 'key')
      !== providerModelCatalogCacheKey('open_ai', 'https://example.test/tenanta', 'key'),
    'case-sensitive paths must use isolated catalog cache keys',
  );
  assertEqual(
    normalizeModelEndpointUrl('HTTPS://EXAMPLE.TEST/TenantA/?workspace=A/'),
    'https://example.test/TenantA?workspace=A/',
    'normalization should lowercase the origin while preserving path/query case and suffixes',
  );
}

function testSavedExternalEndpointIdentityRequiresAnUnchangedScope(): void {
  const baseInput = {
    descriptor: null,
    catalogBaseUrl: null,
    configuredBaseUrl: 'https://tenant.example.test/TenantA/v1',
    persistedEndpointId: 'text:tenant-a',
    persistedProvider: 'open_ai',
    persistedBaseUrl: 'https://tenant.example.test/TenantA/v1/',
    currentProvider: 'open_ai',
  };
  assertEqual(
    endpointIdForSavedSelection(baseInput),
    'text:tenant-a',
    'an unchanged external endpoint identity should round-trip through the edit form',
  );
  assertEqual(
    endpointIdForSavedSelection({
      ...baseInput,
      configuredBaseUrl: 'https://tenant.example.test/TenantB/v1',
    }),
    null,
    'editing the endpoint URL must not retain a stale external identity',
  );
  assertEqual(
    endpointIdForSavedSelection({ ...baseInput, currentProvider: 'anthropic' }),
    null,
    'editing the provider must not retain a stale external identity',
  );
}

function testWizardRequiresAnExplicitNonEmptyModel(): void {
  const models = [{ id: 'known-model' }];
  assertEqual(resolveExplicitModelSelection(models, ''), null, 'an empty wizard model must not resolve');
  assertEqual(
    resolveExplicitModelSelection(models, 'known-model')?.id,
    'known-model',
    'an explicit wizard choice should resolve',
  );
}

testImplicitDefaultsRespectLifecycleAccessAndReadiness();
testLegacyImagePresetProjectsCanonicalMetadata();
testRecommendationDoesNotFabricateProductReadiness();
testSettingsFactsExposeLifecycleAccessCapabilitiesAndAvailability();
testEndpointIdentityRequiresAnExactBaseUrlMatch();
testSavedExternalEndpointIdentityRequiresAnUnchangedScope();
testWizardRequiresAnExplicitNonEmptyModel();
