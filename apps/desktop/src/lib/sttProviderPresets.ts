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
