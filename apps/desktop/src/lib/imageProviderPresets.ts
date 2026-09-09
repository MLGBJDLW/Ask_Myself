import imageProviderPresets from "../../../../shared/image-provider-presets.json";
import {
  normalizeModelEndpointUrl,
  selectImplicitDefault,
  type ModelDescriptor,
} from './modelCatalog';
import {
  hydrateImageProviderPreset,
  type RuntimeImageProviderPreset,
} from './imageProviderCatalogHydration';

export type ImageApiStyle =
  | "openai_images"
  | "xai_images"
  | "gemini_generate_content"
  | "dashscope_multimodal";

export interface ImageModelPreset {
  id: string;
  name: string;
  recommended?: boolean;
  qualityOptions?: string[];
  sizeOptions?: ImageSizeOption[];
  descriptor: ModelDescriptor;
}

export interface ImageSizeOption {
  value: string;
  label: string;
}

export interface ImageProviderPreset {
  id: string;
  name: string;
  provider: string;
  apiStyle: ImageApiStyle;
  baseUrl: string;
  requiresApiKey: boolean;
  description: string;
  models: ImageModelPreset[];
  sizeOptions: ImageSizeOption[];
  qualityOptions: string[];
  outputFormats: string[];
}

export const IMAGE_PROVIDER_PRESETS: ImageProviderPreset[] =
  (imageProviderPresets as RuntimeImageProviderPreset[]).map(hydrateImageProviderPreset);

function normalize(value: string | null | undefined): string {
  return normalizeModelEndpointUrl(value);
}

export function findImageProviderPreset(input: {
  provider?: string | null;
  apiStyle?: string | null;
  baseUrl?: string | null;
}, presets = IMAGE_PROVIDER_PRESETS): ImageProviderPreset | null {
  const provider = (input.provider ?? "").trim();
  const apiStyle = (input.apiStyle ?? "").trim();
  const baseUrl = normalize(input.baseUrl);

  if (baseUrl) {
    const exact = presets.find(
      (preset) =>
        preset.provider === provider &&
        (!apiStyle || preset.apiStyle === apiStyle) &&
        normalize(preset.baseUrl) === baseUrl,
    );
    if (exact) return exact;
  }

  const matches = presets.filter(
    (preset) =>
      preset.provider === provider &&
      (!apiStyle || preset.apiStyle === apiStyle),
  );
  return matches.length === 1 ? matches[0] : null;
}

export function getDefaultImageModel(preset: ImageProviderPreset | null): string {
  return selectImplicitDefault(preset?.models ?? [])?.id ?? "";
}

export function getImageQualityOptions(preset: ImageProviderPreset, model: string): string[] {
  return preset.models.find(candidate => candidate.id === model)?.qualityOptions ?? preset.qualityOptions;
}

export function getImageSizeOptions(preset: ImageProviderPreset, model: string): ImageSizeOption[] {
  return preset.models.find(candidate => candidate.id === model)?.sizeOptions ?? preset.sizeOptions;
}
