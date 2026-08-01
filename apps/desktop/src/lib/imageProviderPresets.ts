import imageProviderPresets from "../../../../shared/image-provider-presets.json";
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  selectImplicitDefault,
  type LegacyCatalogModel,
  type ModelDescriptor,
} from './modelCatalog';

export type ImageApiStyle =
  | "openai_images"
  | "gemini_generate_content"
  | "dashscope_multimodal";

export interface ImageModelPreset {
  id: string;
  name: string;
  recommended?: boolean;
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

type RawImageProviderPreset = Omit<ImageProviderPreset, 'models'> & {
  models: LegacyCatalogModel[];
};

export const IMAGE_PROVIDER_PRESETS: ImageProviderPreset[] =
  (imageProviderPresets as RawImageProviderPreset[]).map((preset) => ({
    ...preset,
    models: attachModelDescriptors(preset.models, {
      surface: 'image',
      providerId: canonicalModelProviderId(preset.id, preset.provider),
      endpointId: modelEndpointId('image', preset.id),
      region: inferModelCatalogRegion(preset.baseUrl),
      apiStyle: preset.apiStyle,
      supportedSizes: preset.sizeOptions.map((option) => option.value),
      outputFormats: preset.outputFormats,
    }) as ImageModelPreset[],
  }));

function normalize(value: string | null | undefined): string {
  return (value ?? "").trim().replace(/\/+$/, "").toLowerCase();
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
        preset.apiStyle === apiStyle &&
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
