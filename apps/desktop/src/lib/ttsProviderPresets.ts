import ttsProviderPresets from "../../../../shared/tts-provider-presets.json";

export interface TtsCatalogItem {
  id: string;
  name: string;
  recommended?: boolean;
  modelIds?: string[];
  languages?: string[];
  gender?: string | null;
  description?: string | null;
  previewUrl?: string | null;
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

export const TTS_PROVIDER_PRESETS = ttsProviderPresets as TtsProviderPreset[];

export function defaultTtsItem(items: TtsCatalogItem[]): TtsCatalogItem | null {
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
