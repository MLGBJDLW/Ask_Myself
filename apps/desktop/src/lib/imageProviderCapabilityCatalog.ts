import type { CapabilityPackageView } from '../types/conversation';
import {
  hydrateImageProviderPreset,
  isRuntimeImageProviderPreset,
} from './imageProviderCatalogHydration.ts';
import type { ImageProviderPreset } from './imageProviderPresets';

export function extractImageProviderPresets(
  capabilityPackage: CapabilityPackageView | null,
  fallbackPresets: readonly ImageProviderPreset[],
): ImageProviderPreset[] {
  const catalog = capabilityPackage?.providerCatalogs?.find((item) => item.id === 'imageProviders');
  const presets = (catalog?.items ?? [])
    .filter(isRuntimeImageProviderPreset)
    .map(hydrateImageProviderPreset);
  return presets.length > 0 ? presets : [...fallbackPresets];
}
