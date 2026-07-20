import embeddingProviderPresets from "../../../../shared/embedding-provider-presets.json";

export interface EmbeddingModelPreset {
  id: string;
  name: string;
  dimensions: number;
  supportsDimensionOverride: boolean;
  recommended?: boolean;
}

export interface EmbeddingProviderPreset {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  description: string;
  models: EmbeddingModelPreset[];
}

export const EMBEDDING_PROVIDER_PRESETS = embeddingProviderPresets as EmbeddingProviderPreset[];

export function defaultEmbeddingModel(preset: EmbeddingProviderPreset): EmbeddingModelPreset | null {
  return preset.models.find((model) => model.recommended) ?? preset.models[0] ?? null;
}

export function findEmbeddingProviderPreset(baseUrl: string): EmbeddingProviderPreset | null {
  const normalized = baseUrl.trim().replace(/\/+$/, "").toLowerCase();
  return EMBEDDING_PROVIDER_PRESETS.find(
    (preset) => preset.baseUrl.replace(/\/+$/, "").toLowerCase() === normalized,
  ) ?? null;
}
