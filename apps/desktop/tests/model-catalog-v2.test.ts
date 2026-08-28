import {
  attachModelDescriptors,
  catalogEndpointIdForSelection,
  endpointIdForSavedSelection,
  inferModelCatalogRegion,
  isImplicitDefaultEligible,
  modelDescriptorFacts,
  normalizeModelEndpointUrl,
  resolveExplicitModelSelection,
  selectImplicitDefault,
  shouldUseCatalogModelSelect,
  type ModelDescriptor,
} from '../src/lib/modelCatalog';
import { providerModelCatalogCacheKey } from '../src/lib/providerModelCatalog';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

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

function testUncataloguedImageModelsRemainEditable(): void {
  const models = [{ id: 'curated-image-model' }];
  assertEqual(
    shouldUseCatalogModelSelect('', models),
    true,
    'an empty image model should require an explicit curated selection',
  );
  assertEqual(
    shouldUseCatalogModelSelect('curated-image-model', models),
    true,
    'a curated image model should use the metadata-rich select',
  );
  assertEqual(
    shouldUseCatalogModelSelect('account-custom-image-model', models),
    false,
    'an uncatalogued saved image model should retain the editable text input',
  );
}

function testGlm53AndDeepSeekCurrentModelsExposeOfficialCapabilities(): void {
  type RawPreset = {
    id: string;
    provider: string;
    baseUrl: string;
    models: Array<Parameters<typeof attachModelDescriptors>[0][number]>;
  };
  const presets = JSON.parse(readFileSync(
    join(process.cwd(), '..', '..', 'shared', 'provider-presets.json'),
    'utf8',
  )) as RawPreset[];
  const findPreset = (provider: string, baseUrl: string) => presets.find(
    preset => preset.provider === provider && normalizeModelEndpointUrl(preset.baseUrl) === normalizeModelEndpointUrl(baseUrl),
  );
  const zhipu = findPreset('zhipu', 'https://open.bigmodel.cn/api/paas/v4');
  assert(zhipu, 'the ordinary Zhipu Model API preset should exist');
  const models = attachModelDescriptors(zhipu.models, {
    surface: 'text',
    providerId: zhipu.id,
    endpointId: `text:${zhipu.id}`,
    apiStyle: 'openai_chat',
  });
  const model = models.find(candidate => candidate.id === 'glm-5.3');
  assert(model, 'the released GLM-5.3 model should be discoverable');
  assertEqual(model.descriptor.lifecycle, 'active', 'the ordinary GLM-5.3 Model API is live');
  assertEqual(model.descriptor.productReadiness, 'product_ready', 'GLM-5.3 is product ready');
  assertEqual(model.descriptor.availableToCredential, true, 'GLM-5.3 is available to credentials');
  assertEqual(model.descriptor.limits.contextTokens, 1_000_000, 'GLM-5.3 context should be official');
  assertEqual(model.descriptor.limits.maxOutputTokens, 131_072, 'GLM-5.3 output limit should be official');
  assertEqual(selectImplicitDefault(models)?.id, 'glm-5.3', 'GLM-5.3 should be the implicit Zhipu default');
  const rawReasoning = model.capabilities?.reasoning as {
    mode?: string;
    defaultEffort?: string;
  } | null | undefined;
  assertEqual(rawReasoning?.mode, 'always', 'GLM-5.3 reasoning cannot be disabled');
  assertEqual(rawReasoning?.defaultEffort, 'max', 'GLM-5.3 should default to max effort');

  const directFlash = models.find(candidate => candidate.id === 'glm-5.3-flash');
  assert(directFlash, 'the direct GLM-5.3-Flash model should be listed');
  assertEqual(directFlash.descriptor.limits.contextTokens, 1_000_000, 'direct Flash context');
  assertEqual(directFlash.descriptor.limits.maxOutputTokens, 131_072, 'direct Flash output');
  assert(directFlash.descriptor.inputModalities.includes('image'), 'direct Flash should accept images');
  assert(
    !directFlash.descriptor.inputModalities.includes('video'),
    'catalog claims must stop at Nexa current text/image wire support',
  );
  assertEqual(directFlash.descriptor.capabilities.vision, true, 'direct Flash vision capability');

  const zhipuInternational = findPreset('zhipu', 'https://api.z.ai/api/paas/v4');
  assert(zhipuInternational, 'the international Z.ai Model API preset should exist');
  assertEqual(zhipuInternational.id, 'zhipu-intl', 'international Z.ai endpoint identity');
  assertEqual(
    inferModelCatalogRegion(zhipuInternational.baseUrl),
    'international',
    'international Z.ai endpoint region',
  );
  assertEqual(
    inferModelCatalogRegion('https://api.z.ai.evil.example/api/paas/v4'),
    'global',
    'edited lookalike endpoints must not inherit the Z.ai region',
  );
  assert(zhipuInternational.id !== zhipu.id, 'China and international presets must stay distinct');
  const internationalModels = attachModelDescriptors(zhipuInternational.models, {
    surface: 'text',
    providerId: zhipuInternational.id,
    endpointId: `text:${zhipuInternational.id}`,
    apiStyle: 'openai_chat',
  });
  for (const [id, expectedVision] of [
    ['glm-5.3', false],
    ['glm-5.3-flash', true],
  ] as const) {
    const candidate = internationalModels.find(model => model.id === id);
    assert(candidate, `${id} should be listed by the international Z.ai Model API`);
    assertEqual(candidate.descriptor.regions.join(','), 'international', `${id} regional identity`);
    assertEqual(candidate.descriptor.limits.contextTokens, 1_000_000, `${id} international context`);
    assertEqual(candidate.descriptor.limits.maxOutputTokens, 131_072, `${id} international output`);
    assertEqual(candidate.descriptor.capabilities.vision, expectedVision, `${id} international vision`);
    assert(
      !candidate.descriptor.inputModalities.includes('video')
        && !candidate.descriptor.inputModalities.includes('file'),
      `${id} must stop at Nexa current wire modalities`,
    );
  }

  const openrouter = findPreset('openrouter', 'https://openrouter.ai/api/v1');
  assert(openrouter, 'the OpenRouter preset should exist');
  const openrouterModels = attachModelDescriptors(openrouter.models, {
    surface: 'text',
    providerId: openrouter.id,
    endpointId: `text:${openrouter.id}`,
    apiStyle: 'openai_chat',
  });
  for (const [id, expectedVision] of [
    ['z-ai/glm-5.3', false],
    ['z-ai/glm-5.3-flash', true],
  ] as const) {
    const candidate = openrouterModels.find(model => model.id === id);
    assert(candidate, `${id} should be listed by OpenRouter`);
    assertEqual(candidate.descriptor.limits.contextTokens, 1_048_576, `${id} safe route context`);
    assertEqual(candidate.descriptor.limits.maxOutputTokens, 131_072, `${id} output`);
    assertEqual(candidate.descriptor.capabilities.vision, expectedVision, `${id} vision`);
    const candidateReasoning = candidate.capabilities?.reasoning as {
      mode?: string;
      effortLevels?: string[];
    } | null | undefined;
    assertEqual(candidateReasoning?.mode, 'always', `${id} mandatory reasoning`);
    assertEqual(
      candidateReasoning?.effortLevels?.join(','),
      'low,high,max',
      `${id} native efforts`,
    );
  }

  const alibaba = findPreset(
    'alibaba_model_studio',
    'https://dashscope.aliyuncs.com/compatible-mode/v1',
  );
  assert(alibaba, 'the Alibaba Model Studio PAYG preset should exist');
  const alibabaModels = attachModelDescriptors(alibaba.models, {
    surface: 'text',
    providerId: alibaba.id,
    endpointId: `text:${alibaba.id}`,
    apiStyle: 'openai_chat',
  });
  const bailianGlm53 = alibabaModels.find(candidate => candidate.id === 'ZHIPU/GLM-5.3');
  assert(bailianGlm53, 'Alibaba PAYG should expose the exact Zhipu direct-supply ID');
  assertEqual(bailianGlm53.descriptor.access, 'account_enablement', 'Bailian model enablement');
  assertEqual(bailianGlm53.descriptor.regions[0], 'cn-beijing', 'Bailian model region');
  assertEqual(bailianGlm53.descriptor.limits.contextTokens, 1_048_576, 'Bailian context');
  assertEqual(bailianGlm53.descriptor.limits.maxOutputTokens, 131_072, 'Bailian output');
  assert(
    bailianGlm53.descriptor.capabilities.structuredOutput !== true,
    'conflicting Alibaba docs must not become a positive structured-output guarantee',
  );

  const deepseek = findPreset('deep_seek', 'https://api.deepseek.com');
  assert(deepseek, 'the official DeepSeek API preset should exist');
  const deepseekModels = attachModelDescriptors(deepseek.models, {
    surface: 'text',
    providerId: deepseek.id,
    endpointId: `text:${deepseek.id}`,
    apiStyle: 'openai_chat',
  });
  for (const id of [
    'deepseek-v4-pro',
    'deepseek-v4-flash',
    'deepseek-v4-flash-vision-exp',
  ]) {
    const candidate = deepseekModels.find(model => model.id === id);
    assert(candidate, `${id} should be present in the official DeepSeek catalog`);
    assertEqual(candidate.descriptor.limits.contextTokens, 1_000_000, `${id} context`);
    assertEqual(candidate.descriptor.limits.maxOutputTokens, 384_000, `${id} output`);
    assertEqual(candidate.descriptor.capabilities.toolCalling, true, `${id} tools`);
    assertEqual(candidate.descriptor.capabilities.structuredOutput, true, `${id} JSON`);
    assertEqual(
      candidate.descriptor.capabilities.nativeWebSearch?.dialect,
      'deepSeekResponses',
      `${id} Responses search`,
    );
  }
  const vision = deepseekModels.find(model => model.id === 'deepseek-v4-flash-vision-exp');
  assert(vision, 'DeepSeek vision model');
  assertEqual(vision.descriptor.capabilities.vision, true, 'DeepSeek vision capability');
  assert(vision.descriptor.inputModalities.includes('image'), 'DeepSeek vision input modality');

  for (const codingPlanUrl of [
    'https://open.bigmodel.cn/api/coding/paas/v4',
    'https://api.z.ai/api/coding/paas/v4',
  ]) {
    assert(
      !findPreset('zhipu', codingPlanUrl),
      'Coding Plan must not be exposed outside its officially supported clients',
    );
  }

  for (const route of [
    { provider: 'qwen', baseUrl: 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1' },
    { provider: 'qwen', baseUrl: 'https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1' },
    { provider: 'siliconflow', baseUrl: 'https://api.siliconflow.cn/v1' },
  ]) {
    assert(
      !findPreset(route.provider, route.baseUrl)?.models.some(model => model.id.toLowerCase().includes('glm-5.3')),
      `${route.provider} must not advertise GLM-5.3 before its own live catalog does`,
    );
  }
}

testImplicitDefaultsRespectLifecycleAccessAndReadiness();
testLegacyImagePresetProjectsCanonicalMetadata();
testRecommendationDoesNotFabricateProductReadiness();
testSettingsFactsExposeLifecycleAccessCapabilitiesAndAvailability();
testEndpointIdentityRequiresAnExactBaseUrlMatch();
testSavedExternalEndpointIdentityRequiresAnUnchangedScope();
testWizardRequiresAnExplicitNonEmptyModel();
testUncataloguedImageModelsRemainEditable();
testGlm53AndDeepSeekCurrentModelsExposeOfficialCapabilities();
