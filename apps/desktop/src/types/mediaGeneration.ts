export type MediaOperation =
  | 'text_to_video'
  | 'image_to_video'
  | 'video_to_video'
  | 'extend'
  | 'edit'
  | 'first_last_frame'
  | 'motion_transfer'
  | 'lip_sync'
  | 'upscale'
  | 'audio_generation';

export type MediaJobState =
  | 'draft'
  | 'validating'
  | 'uploading_assets'
  | 'submitting'
  | 'queued'
  | 'running'
  | 'post_processing'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'expired'
  | 'provider_unknown';

export type MediaObservationMode = 'polling' | 'webhook' | 'hybrid';
export type MediaJobAttemptState =
  | 'created'
  | 'submitting'
  | 'accepted'
  | 'observing'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'expired'
  | 'provider_unknown';
export type MediaAssetStorageKind = 'managed_local' | 'provider_remote' | 'external';
export type MediaAssetLocalState = 'available' | 'deletion_requested' | 'deleted';
export type MediaAssetLocalRetentionPolicy = 'retain_until_deleted' | 'delete_after_expiry';
export type MediaRemoteDeletionStatus =
  | 'not_requested'
  | 'requested'
  | 'confirmed'
  | 'unsupported'
  | 'failed';
export type MediaAssetRelationType =
  | 'input'
  | 'output'
  | 'derived_from'
  | 'variant_of'
  | 'extends'
  | 'edits'
  | 'audio_track'
  | 'export';

export interface CreateMediaJobRequest {
  idempotencyKey: string;
  projectId?: string | null;
  conversationId?: string | null;
  providerId: string;
  providerSource: string;
  modelId: string;
  apiVersion?: string | null;
  operation: MediaOperation;
  inputAssetIds?: string[];
  rawParameters: Record<string, unknown>;
  normalizedParameters: Record<string, unknown>;
  providerExtras?: Record<string, unknown>;
  observationMode: MediaObservationMode;
  estimatedCostMicros?: number | null;
  currency?: string | null;
  dataRegion?: string | null;
  remoteRetentionExpiresAt?: string | null;
  allowCrossProviderFallback?: boolean;
  maxAttempts?: number;
}

export interface MediaJobRecord {
  id: string;
  idempotencyKey: string;
  projectId: string | null;
  conversationId: string | null;
  providerId: string;
  providerSource: string;
  modelId: string;
  apiVersion: string | null;
  operation: MediaOperation;
  inputAssetIds: string[];
  state: MediaJobState;
  revision: number;
  rawParameters: Record<string, unknown>;
  normalizedParameters: Record<string, unknown>;
  providerExtras: Record<string, unknown>;
  observationMode: MediaObservationMode;
  currentAttemptId: string | null;
  currentProviderTaskId: string | null;
  retryCount: number;
  maxAttempts: number;
  estimatedCostMicros: number | null;
  finalCostMicros: number | null;
  currency: string | null;
  dataRegion: string | null;
  remoteRetentionExpiresAt: string | null;
  cancellationRequestedAt: string | null;
  cancellationReason: string | null;
  allowCrossProviderFallback: boolean;
  watermarkPresent: boolean | null;
  provenance: Record<string, unknown>;
  lastProviderObservedAt: string | null;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  expiresAt: string | null;
}

export interface MediaJobAttemptRecord {
  id: string;
  jobId: string;
  attemptNumber: number;
  idempotencyKey: string;
  providerId: string;
  providerSource: string;
  modelId: string;
  apiVersion: string | null;
  dataRegion: string | null;
  remoteRetentionExpiresAt: string | null;
  crossProviderFallbackAuthorized: boolean;
  providerTaskId: string | null;
  state: MediaJobAttemptState;
  error: Record<string, unknown> | null;
  retryClassification: string | null;
  nextEligibleAt: string | null;
  cancellationRequestedAt: string | null;
  cancellationResult: Record<string, unknown> | null;
  remoteDeletionRequestedAt: string | null;
  remoteDeletionStatus: MediaRemoteDeletionStatus;
  remoteDeletionCompletedAt: string | null;
  remoteDeletionError: Record<string, unknown> | null;
  submittedAt: string | null;
  lastObservedAt: string | null;
  completedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface MediaAssetRecord {
  id: string;
  contentHashSha256: string;
  contentVerifiedAt: string;
  mediaType: string;
  byteLength: number;
  storageKind: MediaAssetStorageKind;
  storageKey: string;
  width: number | null;
  height: number | null;
  durationMs: number | null;
  localState: MediaAssetLocalState;
  localDeletionRequestedAt: string | null;
  localDeletionCompletedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface MediaAssetRelationRecord {
  id: string;
  jobId: string;
  attemptId: string;
  assetId: string;
  parentAssetId: string | null;
  relationType: MediaAssetRelationType;
  ordinal: number;
  localRetentionPolicy: MediaAssetLocalRetentionPolicy;
  localRetentionExpiresAt: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
}

export interface MediaProviderEventRecord {
  id: string;
  jobId: string;
  attemptId: string;
  sequence: number;
  providerId: string;
  eventSource: string;
  deduplicationKey: string;
  eventKind: string;
  payload: Record<string, unknown>;
  providerCreatedAt: string | null;
  observedAt: string;
}

export interface MediaJobSnapshot {
  job: MediaJobRecord;
  attempts: MediaJobAttemptRecord[];
  assets: MediaAssetRecord[];
  assetRelations: MediaAssetRelationRecord[];
  providerEventCount: number;
  providerEvents: MediaProviderEventRecord[];
}

export interface RequestMediaJobCancellation {
  jobId: string;
  expectedRevision: number;
  reason: string;
}

export interface RequestMediaJobRemoteDeletion {
  jobId: string;
  expectedRevision: number;
  attemptId: string;
}

export interface DeleteMediaAssetOccurrenceRequest {
  jobId: string;
  expectedRevision: number;
  relationId: string;
}

export interface RequestMediaAssetDeletion {
  assetId: string;
}
