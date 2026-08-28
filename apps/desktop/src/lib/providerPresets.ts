import providerPresets from "../../../../shared/provider-presets.json";
import type { ProviderStreamingConfig } from '../types/conversation';
import type {
  ProviderCapabilities,
  ProviderModelPreset,
  ReasoningCapability,
} from './providerTypes';
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  normalizeModelEndpointUrl,
  type LegacyCatalogModel,
} from './modelCatalog';

export type {
  ModelCatalogSource,
  ModelLifecycleStatus,
  ProviderCapabilities,
  ProviderModelPreset,
  ReasoningCapability,
  ReasoningEffortLevel,
  ThinkingBudgetCapability,
} from './providerTypes';

export interface ProviderPreset {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  models: ProviderModelPreset[];
  requiresApiKey: boolean;
  icon: string;
  description: string;
  capabilities?: ProviderCapabilities;
  streaming?: ProviderStreamingConfig;
}

type RawProviderPreset = Omit<ProviderPreset, 'models'> & { models: LegacyCatalogModel[] };

export const PROVIDER_PRESETS: ProviderPreset[] = (providerPresets as RawProviderPreset[]).map((preset) => ({
  ...preset,
  models: attachModelDescriptors(preset.models, {
    surface: 'text',
    providerId: canonicalModelProviderId(preset.id, preset.provider),
    endpointId: modelEndpointId('text', preset.id),
    region: inferModelCatalogRegion(preset.baseUrl),
    apiStyle: preset.provider === 'anthropic'
      ? 'anthropic_messages'
      : preset.provider === 'google'
        ? 'gemini_generate_content'
        : preset.provider === 'ollama'
          ? 'ollama_chat'
          : 'openai_chat',
  }) as ProviderModelPreset[],
}));

function normalizePresetBaseUrl(baseUrl: string | null | undefined): string {
  return normalizeModelEndpointUrl(baseUrl);
}

function providerKeyForPresetLookup(provider: string, normalizedBaseUrl: string): string {
  const isLegacyQwenPayg = provider === "qwen"
    && PROVIDER_PRESETS.some(
      (preset) => preset.provider === 'alibaba_model_studio'
        && normalizePresetBaseUrl(preset.baseUrl) === normalizedBaseUrl,
    );
  return isLegacyQwenPayg ? "alibaba_model_studio" : provider;
}

function isAlibabaBeijingWorkspaceEndpoint(
  provider: string,
  normalizedBaseUrl: string,
): boolean {
  if (provider !== 'alibaba_model_studio' || !normalizedBaseUrl) return false;
  try {
    const url = new URL(normalizedBaseUrl);
    if (
      url.protocol !== 'https:' ||
      url.port ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      url.pathname.replace(/\/+$/, '') !== '/compatible-mode/v1'
    ) {
      return false;
    }
    const suffix = '.cn-beijing.maas.aliyuncs.com';
    const host = url.hostname.toLowerCase();
    if (!host.endsWith(suffix)) return false;
    const workspaceId = host.slice(0, -suffix.length);
    return workspaceId.length > 0 &&
      !workspaceId.includes('.') &&
      workspaceId !== 'trial' &&
      workspaceId !== 'token-plan';
  } catch {
    return false;
  }
}

export function findProviderPreset(input: {
  provider: string;
  baseUrl?: string | null;
}): ProviderPreset | null {
  const provider = input.provider.trim();
  const normalizedBaseUrl = normalizePresetBaseUrl(input.baseUrl);
  const lookupProvider = providerKeyForPresetLookup(provider, normalizedBaseUrl);

  if (normalizedBaseUrl) {
    const exactMatch = PROVIDER_PRESETS.find(
      (preset) =>
        preset.provider === lookupProvider &&
        normalizePresetBaseUrl(preset.baseUrl) === normalizedBaseUrl,
    );
    if (exactMatch) {
      return exactMatch;
    }
    // Alibaba recommends workspace-dedicated PAYG hosts in production. They
    // share the Beijing model contract while credentials and cache identities
    // remain bound to the exact workspace URL elsewhere.
    if (isAlibabaBeijingWorkspaceEndpoint(lookupProvider, normalizedBaseUrl)) {
      return PROVIDER_PRESETS.find((preset) => preset.id === 'alibaba-model-studio') ?? null;
    }
    // A configured endpoint is part of the capability identity. Familiar
    // provider labels or hosts must not make an edited endpoint inherit an
    // official model/reasoning contract.
    return null;
  }

  const providerMatches = PROVIDER_PRESETS.filter(
    (preset) => preset.provider === lookupProvider,
  );
  const defaultMatch = providerMatches.find(
    (preset) => preset.id === lookupProvider || preset.id.replace(/-/g, '_') === lookupProvider,
  );
  if (defaultMatch) {
    return defaultMatch;
  }
  if (providerMatches.length === 1) {
    return providerMatches[0];
  }

  return null;
}

function normalizeModelId(model: string | null | undefined): string {
  return (model ?? "").trim().toLowerCase();
}

export function findProviderModelPreset(input: {
  provider: string;
  baseUrl?: string | null;
  model?: string | null;
}): ProviderModelPreset | null {
  const preset = findProviderPreset(input);
  const model = normalizeModelId(input.model);
  if (!preset || !model) {
    return null;
  }
  return (
    preset.models.find((candidate) => normalizeModelId(candidate.id) === model) ??
    null
  );
}

function hasCapabilities(capabilities: ProviderCapabilities): boolean {
  return Object.keys(capabilities).length > 0;
}

function mergeCapabilities(
  providerCapabilities: ProviderCapabilities | undefined,
  modelCapabilities: ProviderCapabilities | undefined,
): ProviderCapabilities | null {
  const merged: ProviderCapabilities = {};

  if (providerCapabilities) {
    Object.assign(merged, providerCapabilities);
  }
  if (modelCapabilities) {
    Object.assign(merged, modelCapabilities);
  }

  return hasCapabilities(merged) ? merged : null;
}

export function getProviderModelCapabilities(input: {
  provider: string;
  baseUrl?: string | null;
  model?: string | null;
}): ProviderCapabilities | null {
  const preset = findProviderPreset(input);
  if (!preset) {
    return null;
  }

  return mergeCapabilities(
    preset.capabilities,
    findProviderModelPreset(input)?.capabilities,
  );
}

export function getReasoningCapability(input: {
  provider: string;
  baseUrl?: string | null;
  model?: string | null;
}): ReasoningCapability | null {
  return getProviderModelCapabilities(input)?.reasoning ?? null;
}
