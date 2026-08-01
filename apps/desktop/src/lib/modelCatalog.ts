// Public wire types derived from shared/model-catalog.schema.json. Keep changes
// schema-first; scripts/model-catalog-audit.mjs verifies enum drift in CI.
export type ModelCatalogSource = 'official' | 'discovered' | 'curated';
export type ModelLifecycle = 'active' | 'preview' | 'gated' | 'legacy' | 'deprecated' | 'removed';
export type ModelAccess = 'public' | 'account_enablement' | 'application' | 'private_preview';
export type ProductReadiness = 'known' | 'discoverable' | 'callable' | 'product_ready';
export type ModelModality = 'text' | 'image' | 'audio' | 'video' | 'file' | 'embedding';
export type ModelCatalogSurface = 'text' | 'image' | 'embedding' | 'speech_to_text' | 'text_to_speech';

export interface ThinkingBudgetCapability {
  enabled: boolean;
  defaultTokens?: number;
  minTokens?: number;
  maxTokens?: number;
  step?: number;
  allowZero?: boolean;
}

export interface ReasoningCapability {
  effortLevels?: string[];
  defaultEffort?: string;
  thinkingBudget?: ThinkingBudgetCapability;
}

export interface ModelCapabilities {
  reasoning?: ReasoningCapability | null;
  vision?: boolean;
  audioInput?: boolean;
  audioOutput?: boolean;
  videoInput?: boolean;
  videoOutput?: boolean;
  toolCalling?: boolean;
  parallelToolCalling?: boolean;
  structuredOutput?: boolean;
  imageGeneration?: boolean;
  imageEditing?: boolean;
  multiReferenceEditing?: boolean;
  realtime?: boolean;
  promptCache?: boolean;
  asyncJobs?: boolean;
  batch?: boolean;
  dimensionOverride?: boolean;
}

export interface ModelLimits {
  contextTokens?: number | null;
  maxOutputTokens?: number | null;
  maxImages?: number | null;
  maxInputBytes?: number | null;
  maxVideoSeconds?: number | null;
  maxAudioSeconds?: number | null;
  embeddingDimensions?: number | null;
  supportedSizes?: string[];
  outputFormats?: string[];
}

export interface ModelDescriptor {
  schemaVersion: 2;
  id: string;
  aliases: string[];
  displayName: string;
  providerId: string;
  family: string;
  version: string | null;
  lifecycle: ModelLifecycle;
  access: ModelAccess;
  regions: string[];
  endpointIds: string[];
  endpointKinds: string[];
  inputModalities: ModelModality[];
  outputModalities: ModelModality[];
  capabilities: ModelCapabilities;
  limits: ModelLimits;
  pricingRef: string | null;
  releaseDate: string | null;
  deprecationDate: string | null;
  replacementModelId: string | null;
  source: ModelCatalogSource;
  lastVerifiedAt: string | null;
  productReadiness: ProductReadiness;
  availableToCredential: boolean | null;
  recommended: boolean;
}

export interface LegacyCatalogModel {
  id: string;
  name: string;
  recommended?: boolean;
  aliases?: string[];
  family?: string;
  version?: string | null;
  source?: ModelCatalogSource;
  status?: ModelLifecycle;
  access?: ModelAccess;
  regions?: string[];
  inputModalities?: ModelModality[];
  outputModalities?: ModelModality[];
  modalities?: ModelModality[];
  capabilities?: {
    reasoning?: ReasoningCapability | null;
    vision?: boolean | null;
    audioInput?: boolean;
    audioOutput?: boolean;
    videoInput?: boolean;
    videoOutput?: boolean;
    toolCalling?: boolean;
    parallelToolCalling?: boolean;
    structuredOutput?: boolean;
    imageGeneration?: boolean;
    imageEditing?: boolean;
    multiReferenceEditing?: boolean;
    realtime?: boolean;
    promptCache?: boolean;
    asyncJobs?: boolean;
    batch?: boolean;
  };
  supportsTools?: boolean | null;
  supportsStructuredOutput?: boolean | null;
  supportsDimensionOverride?: boolean;
  dimensions?: number;
  contextTokens?: number;
  maxOutputTokens?: number;
  maxImages?: number;
  maxInputBytes?: number;
  maxVideoSeconds?: number;
  maxAudioSeconds?: number;
  pricingRef?: string | null;
  releaseDate?: string | null;
  deprecationDate?: string | null;
  replacementModelId?: string | null;
  lastVerifiedAt?: string | null;
  productReadiness?: ProductReadiness;
  availableToCredential?: boolean | null;
  descriptor?: ModelDescriptor;
}

export interface ModelProjectionContext {
  surface: ModelCatalogSurface;
  providerId: string;
  endpointId: string;
  region?: string;
  apiStyle?: string;
  supportedSizes?: string[];
  outputFormats?: string[];
}

export type CatalogModel<T extends LegacyCatalogModel = LegacyCatalogModel> = T & {
  descriptor: ModelDescriptor;
};

export function attachModelDescriptors<T extends LegacyCatalogModel>(
  models: readonly T[],
  context: ModelProjectionContext,
): CatalogModel<T>[] {
  return models.map((model) => ({
    ...model,
    descriptor: model.descriptor ?? projectModelDescriptor(model, context),
  }));
}

export function projectModelDescriptor(
  model: LegacyCatalogModel,
  context: ModelProjectionContext,
): ModelDescriptor {
  const lifecycle = inferLifecycle(model);
  const access = model.access ?? (lifecycle === 'preview'
    ? 'application'
    : lifecycle === 'gated'
      ? 'account_enablement'
      : 'public');
  const source = model.source ?? 'curated';
  const recommended = model.recommended ?? false;
  const productReadiness = model.productReadiness
    ?? (source === 'discovered'
      ? 'discoverable'
      : 'known');
  const realtime = context.apiStyle?.toLowerCase().includes('realtime') ?? false;
  const rawCapabilities = model.capabilities ?? {};

  return {
    schemaVersion: 2,
    id: model.id,
    aliases: model.aliases ?? [],
    displayName: model.name,
    providerId: context.providerId,
    family: model.family ?? model.id,
    version: model.version ?? null,
    lifecycle,
    access,
    regions: model.regions?.length ? model.regions : context.region ? [context.region] : [],
    endpointIds: [context.endpointId],
    endpointKinds: [context.surface],
    inputModalities: model.inputModalities?.length
      ? model.inputModalities
      : defaultInputModalities(context.surface, model.modalities),
    outputModalities: model.outputModalities?.length
      ? model.outputModalities
      : defaultOutputModalities(context.surface),
    capabilities: {
      reasoning: rawCapabilities.reasoning ?? undefined,
      vision: rawCapabilities.vision ?? undefined,
      audioInput: context.surface === 'speech_to_text' || rawCapabilities.audioInput || undefined,
      audioOutput: context.surface === 'text_to_speech' || rawCapabilities.audioOutput || undefined,
      videoInput: rawCapabilities.videoInput || undefined,
      videoOutput: rawCapabilities.videoOutput || undefined,
      toolCalling: model.supportsTools ?? rawCapabilities.toolCalling ?? undefined,
      parallelToolCalling: rawCapabilities.parallelToolCalling || undefined,
      structuredOutput: model.supportsStructuredOutput ?? rawCapabilities.structuredOutput ?? undefined,
      imageGeneration: context.surface === 'image' || rawCapabilities.imageGeneration || undefined,
      imageEditing: rawCapabilities.imageEditing || undefined,
      multiReferenceEditing: rawCapabilities.multiReferenceEditing || undefined,
      realtime: realtime || rawCapabilities.realtime || undefined,
      promptCache: rawCapabilities.promptCache || undefined,
      asyncJobs: rawCapabilities.asyncJobs || undefined,
      batch: rawCapabilities.batch || undefined,
      dimensionOverride: model.supportsDimensionOverride || undefined,
    },
    limits: {
      contextTokens: model.contextTokens ?? null,
      maxOutputTokens: model.maxOutputTokens ?? null,
      maxImages: model.maxImages ?? null,
      maxInputBytes: model.maxInputBytes ?? null,
      maxVideoSeconds: model.maxVideoSeconds ?? null,
      maxAudioSeconds: model.maxAudioSeconds ?? null,
      embeddingDimensions: model.dimensions ?? null,
      supportedSizes: context.supportedSizes ?? [],
      outputFormats: context.outputFormats ?? [],
    },
    pricingRef: model.pricingRef ?? null,
    releaseDate: model.releaseDate ?? null,
    deprecationDate: model.deprecationDate ?? null,
    replacementModelId: model.replacementModelId ?? null,
    source,
    lastVerifiedAt: model.lastVerifiedAt ?? null,
    productReadiness,
    availableToCredential: model.availableToCredential ?? null,
    recommended,
  };
}

export function isImplicitDefaultEligible(model: ModelDescriptor): boolean {
  return model.lifecycle === 'active'
    && model.access === 'public'
    && model.productReadiness === 'product_ready'
    && model.availableToCredential !== false;
}

/** Compact, stable facts shared by every model-settings surface. */
export function modelDescriptorFacts(model: ModelDescriptor): string[] {
  const credential = model.availableToCredential === true
    ? 'available'
    : model.availableToCredential === false
      ? 'unavailable'
      : 'unknown';

  return [
    `lifecycle:${model.lifecycle}`,
    `readiness:${model.productReadiness}`,
    `access:${model.access}`,
    `region:${model.regions.length ? model.regions.join('+') : 'unknown'}`,
    `io:${model.inputModalities.join('+')}→${model.outputModalities.join('+')}`,
    `tools:${Boolean(model.capabilities.toolCalling)}`,
    `reasoning:${Boolean(model.capabilities.reasoning)}`,
    `realtime:${Boolean(model.capabilities.realtime)}`,
    `async:${Boolean(model.capabilities.asyncJobs)}`,
    `source:${model.source}`,
    `verified:${model.lastVerifiedAt ?? 'unknown'}`,
    `replacement:${model.replacementModelId ?? 'none'}`,
    `credential:${credential}`,
  ];
}

/** Single-line metadata for native select and datalist rows. */
export function modelDescriptorSummary(model: ModelDescriptor): string {
  return modelDescriptorFacts(model)
    .map((fact) => fact.replace(':', '=').replace(/_/g, ' '))
    .join(' · ');
}

export function selectImplicitDefault<T extends { descriptor: ModelDescriptor }>(
  models: readonly T[],
): T | null {
  return models.find((model) => model.descriptor.recommended && isImplicitDefaultEligible(model.descriptor))
    ?? models.find((model) => isImplicitDefaultEligible(model.descriptor))
    ?? null;
}

export function resolveExplicitModelSelection<T extends { id: string }>(
  models: readonly T[],
  modelId: string,
): T | null {
  const selected = modelId.trim();
  if (!selected) return null;
  return models.find((model) => model.id === selected) ?? null;
}

export function modelEndpointId(surface: ModelCatalogSurface, presetId: string): string {
  return `${surface}:${presetId.trim().toLowerCase()}`;
}

/** Normalize URL syntax while preserving case-sensitive path/query identity. */
export function normalizeModelEndpointUrl(value: string | null | undefined): string {
  const raw = (value ?? '').trim();
  if (!raw) return '';
  try {
    const url = new URL(raw);
    const path = url.pathname === '/' ? '/' : url.pathname.replace(/\/+$/, '');
    return `${url.protocol}//${url.host}${path}${url.search}${url.hash}`;
  } catch {
    return raw.replace(/\/+$/, '');
  }
}

export function catalogEndpointIdForSelection(
  descriptor: ModelDescriptor | null | undefined,
  catalogBaseUrl: string | null | undefined,
  configuredBaseUrl: string | null | undefined,
): string | null {
  if (!descriptor) return null;
  if (normalizeModelEndpointUrl(catalogBaseUrl) !== normalizeModelEndpointUrl(configuredBaseUrl)) return null;
  return descriptor.endpointIds[0] ?? null;
}

export function canonicalModelProviderId(presetId: string, adapterProvider: string): string {
  const id = presetId.trim().toLowerCase();
  if (id === 'openai' || id === 'openai-live') return 'openai';
  if (id.startsWith('google')) return 'google';
  if (id.startsWith('qwen') || id.startsWith('alibaba') || id.startsWith('dashscope')) {
    return 'alibaba_model_studio';
  }
  if (id.startsWith('custom')) return 'custom';
  if (id.startsWith('sherpa')) return 'sherpa_onnx';
  if (adapterProvider === 'deep_seek') return 'deepseek';
  if (adapterProvider === 'lm_studio') return 'lmstudio';
  if (adapterProvider === 'open_ai') return id.replace(/-/g, '_');
  return adapterProvider;
}

export function inferModelCatalogRegion(baseUrl: string | null | undefined): string {
  const value = (baseUrl ?? '').trim().toLowerCase();
  if (!value || value.includes('localhost') || value.includes('127.0.0.1')) return 'local';
  if (value.includes('dashscope-intl')) return 'ap-southeast-1';
  if (value.includes('dashscope') || value.includes('cn-beijing')) return 'cn-beijing';
  if (value.includes('eastus')) return 'eastus';
  return 'global';
}

function inferLifecycle(model: LegacyCatalogModel): ModelLifecycle {
  if (model.status) return model.status;
  const text = `${model.id} ${model.name}`.toLowerCase();
  return text.includes('preview') ? 'preview' : 'active';
}

function defaultInputModalities(
  surface: ModelCatalogSurface,
  explicit: ModelModality[] | undefined,
): ModelModality[] {
  if (surface === 'text' && explicit?.length) return explicit;
  if (surface === 'speech_to_text') return ['audio'];
  return ['text'];
}

function defaultOutputModalities(surface: ModelCatalogSurface): ModelModality[] {
  switch (surface) {
    case 'image':
      return ['image'];
    case 'embedding':
      return ['embedding'];
    case 'text_to_speech':
      return ['audio'];
    case 'text':
    case 'speech_to_text':
      return ['text'];
  }
}
