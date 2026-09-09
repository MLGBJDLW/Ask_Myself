import type {
  ImageApiStyle,
  ImageProviderPreset,
  ImageSizeOption,
} from './imageProviderPresets';
import {
  attachModelDescriptors,
  canonicalModelProviderId,
  inferModelCatalogRegion,
  modelEndpointId,
  type LegacyCatalogModel,
  type ModelDescriptor,
} from './modelCatalog.ts';

export type RuntimeImageProviderPreset = Omit<ImageProviderPreset, 'models'> & {
  models: (LegacyCatalogModel & { qualityOptions?: string[] })[];
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

function isImageApiStyle(value: unknown): value is ImageApiStyle {
  return value === 'openai_images'
    || value === 'xai_images'
    || value === 'gemini_generate_content'
    || value === 'dashscope_multimodal';
}

function isLegacyCatalogModel(value: unknown): value is LegacyCatalogModel {
  if (!isRecord(value)) return false;
  return typeof value.id === 'string' && typeof value.name === 'string';
}

function isImageSizeOption(value: unknown): value is ImageSizeOption {
  if (!isRecord(value)) return false;
  return typeof value.value === 'string' && typeof value.label === 'string';
}

function isPickerSafeModelDescriptor(value: unknown): value is ModelDescriptor {
  if (!isRecord(value)) return false;
  return (
    value.schemaVersion === 2 &&
    typeof value.id === 'string' &&
    isStringArray(value.aliases) &&
    typeof value.displayName === 'string' &&
    typeof value.providerId === 'string' &&
    typeof value.family === 'string' &&
    isNullableString(value.version) &&
    typeof value.lifecycle === 'string' &&
    typeof value.access === 'string' &&
    isStringArray(value.regions) &&
    isStringArray(value.endpointIds) &&
    isStringArray(value.endpointKinds) &&
    isStringArray(value.inputModalities) &&
    isStringArray(value.outputModalities) &&
    isRecord(value.capabilities) &&
    isRecord(value.limits) &&
    isNullableString(value.pricingRef) &&
    isNullableString(value.releaseDate) &&
    isNullableString(value.deprecationDate) &&
    isNullableString(value.replacementModelId) &&
    typeof value.source === 'string' &&
    isNullableString(value.lastVerifiedAt) &&
    typeof value.productReadiness === 'string' &&
    (value.availableToCredential === null || typeof value.availableToCredential === 'boolean') &&
    typeof value.recommended === 'boolean'
  );
}

export function isRuntimeImageProviderPreset(value: unknown): value is RuntimeImageProviderPreset {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    typeof value.provider === 'string' &&
    isImageApiStyle(value.apiStyle) &&
    typeof value.baseUrl === 'string' &&
    typeof value.requiresApiKey === 'boolean' &&
    typeof value.description === 'string' &&
    Array.isArray(value.models) && value.models.every(isLegacyCatalogModel) &&
    Array.isArray(value.sizeOptions) && value.sizeOptions.every(isImageSizeOption) &&
    isStringArray(value.qualityOptions) &&
    isStringArray(value.outputFormats)
  );
}

export function hydrateImageProviderPreset(
  preset: RuntimeImageProviderPreset,
): ImageProviderPreset {
  const models = preset.models.map((model) => ({
    ...model,
    qualityOptions: isStringArray(model.qualityOptions) ? model.qualityOptions : undefined,
    descriptor: isPickerSafeModelDescriptor(model.descriptor)
      ? model.descriptor
      : undefined,
  }));

  return {
    ...preset,
    models: attachModelDescriptors(models, {
      surface: 'image',
      providerId: canonicalModelProviderId(preset.id, preset.provider),
      endpointId: modelEndpointId('image', preset.id),
      region: inferModelCatalogRegion(preset.baseUrl),
      apiStyle: preset.apiStyle,
      supportedSizes: preset.sizeOptions.map((option) => option.value),
      outputFormats: preset.outputFormats,
    }),
  };
}
