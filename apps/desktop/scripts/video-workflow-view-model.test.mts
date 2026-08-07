import assert from 'node:assert/strict';
import test from 'node:test';

import {
  defaultDuration,
  mayCancelVariant,
  mayRetryVariant,
  queueBucket,
  selectableVideoModels,
  variantStatusLabel,
} from '../src/lib/videoWorkflowViewModel.ts';
import type {
  VideoOperationCapability,
  VideoProviderConnectionRecord,
  VideoProviderPreset,
  VideoWorkflowVariantRecord,
} from '../src/types/videoWorkflow.ts';

const capability: VideoOperationCapability = {
  operation: 'text_to_video',
  durationOptions: [{ resolution: '768P', minDurationSeconds: 4, maxDurationSeconds: 15, durationsSeconds: [] }],
  aspectRatios: ['16:9'],
  inputRoles: [],
  supportsAudio: true,
  supportsSeed: false,
};

const preset = {
  id: 'minimax-video',
  name: 'MiniMax Video',
  providerId: 'minimax',
  models: [
    { providerId: 'minimax', modelId: 'MiniMax-H3', displayName: 'MiniMax H3', apiVersion: 'v2', releaseStatus: 'ga', selectable: true, operationCapabilities: [capability] },
    { providerId: 'minimax', modelId: 'future', displayName: 'Future', apiVersion: null, releaseStatus: 'preview', selectable: true, operationCapabilities: [capability] },
  ],
} as VideoProviderPreset;

const connection = {
  id: 'connection-1',
  providerId: 'minimax',
  displayName: 'Production',
} as VideoProviderConnectionRecord;

function variant(state: VideoWorkflowVariantRecord['job']['state'], error: Record<string, unknown> | null = null): VideoWorkflowVariantRecord {
  return {
    id: 'variant-1',
    jobId: 'job-1',
    job: {
      state,
      retryCount: 0,
      maxAttempts: 3,
      error,
      retryClassification: error ? 'rate_limited' : null,
      nextEligibleAt: null,
      cancellationRequestedAt: null,
    },
  } as VideoWorkflowVariantRecord;
}

test('only GA models with an exact configured provider connection are selectable', () => {
  const selected = selectableVideoModels([preset], [connection]);
  assert.deepEqual(selected.map(({ model }) => model.modelId), ['MiniMax-H3']);
  assert.deepEqual(selectableVideoModels([preset], []), []);
});

test('capability defaults use the declared minimum when no discrete duration exists', () => {
  assert.deepEqual(defaultDuration(capability), {
    durationSeconds: 4,
    resolution: '768P',
    aspectRatio: '16:9',
  });
});

test('queue actions expose two-phase cancellation and classified retry only', () => {
  const retryable = variant('submitting', { code: 'rate_limited' });
  assert.equal(queueBucket(retryable), 'active');
  assert.equal(mayRetryVariant(retryable), true);
  assert.equal(mayCancelVariant(retryable), true);
  assert.equal(mayRetryVariant(variant('failed', { code: 'fatal' })), false);
  assert.equal(mayCancelVariant(variant('completed')), false);
});

test('retry timing and cancellation intent are projected without hiding attention state', () => {
  const waiting = variant('submitting', { code: 'rate_limited' });
  waiting.job.nextEligibleAt = new Date(Date.now() + 60_000).toISOString();
  assert.equal(mayRetryVariant(waiting), false);

  const unknown = variant('provider_unknown', { code: 'status_lookup_failed' });
  unknown.job.currentProviderTaskId = 'provider-task-1';
  assert.equal(mayRetryVariant(unknown), true);
  assert.equal(mayCancelVariant(unknown), true);

  unknown.job.cancellationRequestedAt = new Date().toISOString();
  assert.equal(variantStatusLabel(unknown), 'cancel attention');
  assert.equal(mayCancelVariant(unknown), false);
});
