import sttProviderPresets from '../../../../shared/stt-provider-presets.json';
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  selectImplicitDefault,
  type LegacyCatalogModel,
  type ModelDescriptor,
} from './modelCatalog';

export interface SttCatalogItem {
  id: string;
  name: string;
  recommended?: boolean;
  descriptor: ModelDescriptor;
}

export interface SttProviderPreset {
  id: string;
  name: string;
  provider: string;
  apiStyle: string;
  requiresApiKey: boolean;
  local?: boolean;
  baseUrl: string;
  sherpaModelFamily?: string;
  description: string;
  models: SttCatalogItem[];
}

type RawSttProviderPreset = Omit<SttProviderPreset, 'models'> & { models: LegacyCatalogModel[] };

export const STT_PROVIDER_PRESETS: SttProviderPreset[] =
  (sttProviderPresets as RawSttProviderPreset[]).map((preset) => ({
    ...preset,
    models: attachModelDescriptors(preset.models, {
      surface: 'speech_to_text',
      providerId: canonicalModelProviderId(preset.id, preset.provider),
      endpointId: modelEndpointId('speech_to_text', preset.id),
      region: inferModelCatalogRegion(preset.baseUrl),
      apiStyle: preset.apiStyle,
    }) as SttCatalogItem[],
  }));

export function defaultSttItem(items: SttCatalogItem[]): SttCatalogItem | null {
  return selectImplicitDefault(items);
}

/** Resolve the catalog entry that backs a saved speech-to-text configuration. */
export function findSttProviderPreset(config: {
  provider: string;
  apiStyle: string;
  sherpaModelFamily?: string | null;
} | null | undefined): SttProviderPreset | null {
  if (!config) return null;
  const sherpaFamily = config.apiStyle === 'sherpa_onnx'
    ? config.sherpaModelFamily ?? 'sense_voice'
    : null;
  return STT_PROVIDER_PRESETS.find((preset) =>
    preset.provider === config.provider
    && preset.apiStyle === config.apiStyle
    && (preset.sherpaModelFamily ?? null) === sherpaFamily,
  ) ?? null;
}
