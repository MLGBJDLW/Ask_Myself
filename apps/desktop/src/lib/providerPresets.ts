import providerPresets from "../../../../shared/provider-presets.json";

export type ReasoningEffortLevel =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "max"
  | "xhigh";

export interface ThinkingBudgetCapability {
  enabled: boolean;
  defaultTokens?: number;
  minTokens?: number;
  maxTokens?: number;
  step?: number;
}

export interface ReasoningCapability {
  effortLevels?: ReasoningEffortLevel[];
  defaultEffort?: ReasoningEffortLevel;
  thinkingBudget?: ThinkingBudgetCapability;
}

export interface ProviderCapabilities {
  reasoning?: ReasoningCapability | null;
  vision?: boolean | null;
}

export interface ProviderModelPreset {
  id: string;
  name: string;
  tagKey?: string;
  recommended?: boolean;
  capabilities?: ProviderCapabilities;
}

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
}

export const PROVIDER_PRESETS: ProviderPreset[] =
  providerPresets as ProviderPreset[];

function normalizePresetBaseUrl(baseUrl: string | null | undefined): string {
  return (baseUrl ?? "").trim().replace(/\/+$/, "").toLowerCase();
}

export function findProviderPreset(input: {
  provider: string;
  baseUrl?: string | null;
}): ProviderPreset | null {
  const provider = input.provider.trim();
  const normalizedBaseUrl = normalizePresetBaseUrl(input.baseUrl);

  if (normalizedBaseUrl) {
    const exactMatch = PROVIDER_PRESETS.find(
      (preset) =>
        preset.provider === provider &&
        normalizePresetBaseUrl(preset.baseUrl) === normalizedBaseUrl,
    );
    if (exactMatch) {
      return exactMatch;
    }
  }

  const providerMatches = PROVIDER_PRESETS.filter(
    (preset) => preset.provider === provider,
  );
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
