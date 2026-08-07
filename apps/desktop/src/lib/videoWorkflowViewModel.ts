import type {
  VideoModelManifest,
  VideoOperationCapability,
  VideoProviderConnectionRecord,
  VideoProviderPreset,
  VideoWorkflowVariantRecord,
} from '../types/videoWorkflow';

export function selectableVideoModels(
  presets: VideoProviderPreset[],
  connections: VideoProviderConnectionRecord[],
): Array<{ preset: VideoProviderPreset; model: VideoModelManifest; connection: VideoProviderConnectionRecord }> {
  const result: Array<{
    preset: VideoProviderPreset;
    model: VideoModelManifest;
    connection: VideoProviderConnectionRecord;
  }> = [];
  for (const preset of presets) {
    for (const connection of connections.filter((candidate) => candidate.providerId === preset.providerId)) {
      for (const model of preset.models) {
        if (model.selectable && model.releaseStatus === 'ga') {
          result.push({ preset, model, connection });
        }
      }
    }
  }
  return result;
}

export function operationCapability(
  model: VideoModelManifest,
  operation: string,
): VideoOperationCapability | null {
  return model.operationCapabilities.find((candidate) => candidate.operation === operation) ?? null;
}

export function defaultDuration(capability: VideoOperationCapability): {
  durationSeconds: number;
  resolution: string;
  aspectRatio: string;
} {
  const option = capability.durationOptions[0];
  const durationSeconds = option.durationsSeconds[0]
    ?? option.minDurationSeconds
    ?? option.maxDurationSeconds
    ?? 4;
  return {
    durationSeconds,
    resolution: option.resolution,
    aspectRatio: capability.aspectRatios[0] ?? '16:9',
  };
}

export function queueBucket(variant: VideoWorkflowVariantRecord): 'active' | 'complete' | 'attention' | 'draft' {
  if (variant.job.cancellationRequestedAt != null) {
    return variant.job.error ? 'attention' : 'active';
  }
  switch (variant.job.state) {
    case 'draft':
      return 'draft';
    case 'completed':
      return 'complete';
    case 'failed':
    case 'cancelled':
    case 'expired':
    case 'provider_unknown':
      return 'attention';
    default:
      return 'active';
  }
}

export function formatMicros(amount: number | null, currency: string | null = 'USD'): string {
  if (amount == null) return '—';
  const value = amount / 1_000_000;
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: currency ?? 'USD',
    minimumFractionDigits: value < 1 ? 2 : 0,
    maximumFractionDigits: 2,
  }).format(value);
}

export function mayRetryVariant(variant: VideoWorkflowVariantRecord): boolean {
  if (variant.job.cancellationRequestedAt != null) return false;
  if (variant.job.nextEligibleAt && Date.parse(variant.job.nextEligibleAt) > Date.now()) return false;
  if (variant.job.state === 'provider_unknown') return variant.job.currentProviderTaskId != null;
  if (variant.job.state === 'post_processing') return variant.job.error != null;
  return variant.job.state === 'submitting'
    && variant.job.retryCount < variant.job.maxAttempts
    && variant.job.error != null
    && variant.job.retryClassification != null;
}

export function mayCancelVariant(variant: VideoWorkflowVariantRecord): boolean {
  return !['completed', 'failed', 'cancelled', 'expired'].includes(variant.job.state)
    && (variant.job.state !== 'provider_unknown' || variant.job.currentProviderTaskId != null)
    && variant.job.cancellationRequestedAt == null;
}

export function variantStatusLabel(variant: VideoWorkflowVariantRecord): string {
  if (variant.job.cancellationRequestedAt != null) {
    return variant.job.error ? 'cancel attention' : 'cancelling';
  }
  return variant.job.state.replace(/_/g, ' ');
}
