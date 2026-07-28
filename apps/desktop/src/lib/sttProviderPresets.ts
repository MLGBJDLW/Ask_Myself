import sttProviderPresets from '../../../../shared/stt-provider-presets.json';

export interface SttCatalogItem {
  id: string;
  name: string;
  recommended?: boolean;
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

export const STT_PROVIDER_PRESETS = sttProviderPresets as SttProviderPreset[];

export function defaultSttItem(items: SttCatalogItem[]): SttCatalogItem | null {
  return items.find((item) => item.recommended) ?? items[0] ?? null;
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
