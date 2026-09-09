import assert from 'node:assert/strict';
import test from 'node:test';

import { extractImageProviderPresets } from '../src/lib/imageProviderCapabilityCatalog.ts';
import type { CapabilityPackageView } from '../src/types/conversation.ts';

const rawRuntimePreset = {
  id: 'openai',
  name: 'OpenAI Images',
  provider: 'open_ai',
  apiStyle: 'openai_images',
  baseUrl: 'https://api.openai.com/v1',
  requiresApiKey: true,
  description: 'Runtime capability catalog fixture',
  models: [
    {
      id: 'gpt-image-2',
      name: 'GPT Image 2',
      recommended: true,
    },
  ],
  sizeOptions: [{ value: '1024x1024', label: '1024 x 1024' }],
  qualityOptions: ['high'],
  outputFormats: ['png'],
};

test('runtime image provider models are hydrated before the catalog picker reads modalities', () => {
  const capabilityPackage = {
    providerCatalogs: [
      {
        id: 'imageProviders',
        label: 'Image providers',
        itemKind: 'imageProviderPreset',
        items: [rawRuntimePreset],
      },
    ],
  } as unknown as CapabilityPackageView;

  const presets = extractImageProviderPresets(capabilityPackage, []);
  const descriptor = presets[0].models[0].descriptor;

  assert.deepEqual(descriptor.inputModalities, ['text']);
  assert.deepEqual(descriptor.outputModalities, ['image']);
  assert.equal(descriptor.providerId, 'openai');
  assert.deepEqual(descriptor.endpointIds, ['image:openai']);
  assert.deepEqual(descriptor.regions, ['global']);
  assert.deepEqual(descriptor.limits.supportedSizes, ['1024x1024']);
  assert.deepEqual(descriptor.limits.outputFormats, ['png']);
});

test('malformed runtime descriptors are reprojected instead of reaching the picker', () => {
  const capabilityPackage = {
    providerCatalogs: [
      {
        id: 'imageProviders',
        label: 'Image providers',
        itemKind: 'imageProviderPreset',
        items: [
          {
            ...rawRuntimePreset,
            models: [{ ...rawRuntimePreset.models[0], descriptor: {} }],
          },
        ],
      },
    ],
  } as unknown as CapabilityPackageView;

  const presets = extractImageProviderPresets(capabilityPackage, []);
  const descriptor = presets[0].models[0].descriptor;

  assert.deepEqual(descriptor.inputModalities, ['text']);
  assert.deepEqual(descriptor.outputModalities, ['image']);
});

test('native image catalog preserves per-model options and the xAI protocol', () => {
  const presets = extractImageProviderPresets({ providerCatalogs: [{ id: 'imageProviders', items: [{
    ...rawRuntimePreset, id: 'xai', apiStyle: 'xai_images', baseUrl: 'https://api.x.ai/v1',
    models: [{ id: 'grok-imagine-image-2.0', name: 'Grok Imagine Image 2.0', qualityOptions: ['auto', 'low', 'medium'], sizeOptions: [{ value: '16:9|2k', label: 'Landscape 2K' }] }],
  }] }] } as unknown as CapabilityPackageView, []);
  assert.equal(presets[0].apiStyle, 'xai_images');
  assert.deepEqual(presets[0].models[0].qualityOptions, ['auto', 'low', 'medium']);
  assert.deepEqual(presets[0].models[0].sizeOptions, [{ value: '16:9|2k', label: 'Landscape 2K' }]);
  assert.deepEqual(presets[0].models[0].descriptor.limits.supportedSizes, ['16:9|2k']);
  assert.deepEqual(presets[0].models[0].descriptor.outputModalities, ['image']);
});
