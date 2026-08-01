import embeddingProviderPresets from "../../../../shared/embedding-provider-presets.json";
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  normalizeModelEndpointUrl,
  selectImplicitDefault,
  type LegacyCatalogModel,
  type ModelDescriptor,
} from './modelCatalog';

export interface EmbeddingModelPreset {
  id: string;
  name: string;
  dimensions: number;
  supportsDimensionOverride: boolean;
  recommended?: boolean;
  descriptor: ModelDescriptor;
}

export interface EmbeddingProviderPreset {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  description: string;
  models: EmbeddingModelPreset[];
}

type RawEmbeddingProviderPreset = Omit<EmbeddingProviderPreset, 'models'> & {
  models: LegacyCatalogModel[];
};

export const EMBEDDING_PROVIDER_PRESETS: EmbeddingProviderPreset[] =
  (embeddingProviderPresets as RawEmbeddingProviderPreset[]).map((preset) => ({
    ...preset,
    models: attachModelDescriptors(preset.models, {
      surface: 'embedding',
      providerId: canonicalModelProviderId(preset.id, preset.provider),
      endpointId: modelEndpointId('embedding', preset.id),
      region: inferModelCatalogRegion(preset.baseUrl),
      apiStyle: 'openai_embeddings',
    }) as EmbeddingModelPreset[],
  }));

export function defaultEmbeddingModel(preset: EmbeddingProviderPreset): EmbeddingModelPreset | null {
  return selectImplicitDefault(preset.models);
}

export function findEmbeddingProviderPreset(baseUrl: string): EmbeddingProviderPreset | null {
  const normalized = normalizeModelEndpointUrl(baseUrl);
  return EMBEDDING_PROVIDER_PRESETS.find(
    (preset) => normalizeModelEndpointUrl(preset.baseUrl) === normalized,
  ) ?? null;
}
