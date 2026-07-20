import ttsProviderPresets from "../../../../shared/tts-provider-presets.json";

export interface TtsCatalogItem {
  id: string;
  name: string;
  recommended?: boolean;
}

export interface TtsProviderPreset {
  id: string;
  name: string;
  provider: string;
  apiStyle: string;
  baseUrl: string;
  description: string;
  models: TtsCatalogItem[];
  voices: TtsCatalogItem[];
  outputFormats: string[];
}

export const TTS_PROVIDER_PRESETS = ttsProviderPresets as TtsProviderPreset[];

export function defaultTtsItem(items: TtsCatalogItem[]): TtsCatalogItem | null {
  return items.find((item) => item.recommended) ?? items[0] ?? null;
}
