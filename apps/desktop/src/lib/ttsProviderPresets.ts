import ttsProviderPresets from "../../../../shared/tts-provider-presets.json";
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  selectImplicitDefault,
  type LegacyCatalogModel,
  type ModelDescriptor,
} from './modelCatalog';

export interface TtsCatalogItem {
  id: string;
  name: string;
  recommended?: boolean;
  modelIds?: string[];
  languages?: string[];
  gender?: string | null;
  description?: string | null;
  previewUrl?: string | null;
  descriptor?: ModelDescriptor;
}

export interface TtsProviderPreset {
  id: string;
  name: string;
  provider: string;
  apiStyle: string;
  requiresApiKey: boolean;
  local?: boolean;
  baseUrl: string;
  description: string;
  models: TtsCatalogItem[];
  voices: TtsCatalogItem[];
  outputFormats: string[];
}

type RawTtsProviderPreset = Omit<TtsProviderPreset, 'models'> & { models: LegacyCatalogModel[] };

export const TTS_PROVIDER_PRESETS: TtsProviderPreset[] =
  (ttsProviderPresets as RawTtsProviderPreset[]).map((preset) => ({
    ...preset,
    models: attachModelDescriptors(preset.models, {
      surface: 'text_to_speech',
      providerId: canonicalModelProviderId(preset.id, preset.provider),
      endpointId: modelEndpointId('text_to_speech', preset.id),
      region: inferModelCatalogRegion(preset.baseUrl),
      apiStyle: preset.apiStyle,
      outputFormats: preset.outputFormats,
    }) as TtsCatalogItem[],
  }));

export function defaultTtsItem(items: TtsCatalogItem[]): TtsCatalogItem | null {
  const models = items.filter(
    (item): item is TtsCatalogItem & { descriptor: ModelDescriptor } => Boolean(item.descriptor),
  );
  if (models.length === items.length) {
    return selectImplicitDefault(models);
  }
  // Voice rows are not model descriptors and keep their existing preference order.
  return items.find((item) => item.recommended) ?? items[0] ?? null;
}

/** Resolve the catalog entry that backs a saved text-to-speech configuration. */
export function findTtsProviderPreset(config: {
  provider: string;
  apiStyle: string;
} | null | undefined): TtsProviderPreset | null {
  if (!config) return null;
  return TTS_PROVIDER_PRESETS.find(
    (preset) => preset.provider === config.provider && preset.apiStyle === config.apiStyle,
  ) ?? null;
}
