import type { MediaJobState, MediaOperation } from './mediaGeneration';

export type VideoInputRole =
  | 'first_frame'
  | 'last_frame'
  | 'input_video'
  | 'reference_image'
  | 'reference_video'
  | 'reference_audio';

export interface VideoInputAsset {
  role: VideoInputRole;
  uri: string;
  mediaType: string;
  metadataVerified: boolean;
  byteLength?: number | null;
  contentHashSha256?: string | null;
  localAssetId?: string | null;
  width?: number | null;
  height?: number | null;
  durationMs?: number | null;
  frameRate?: number | null;
  videoCodec?: string | null;
}

export interface VideoDurationOption {
  resolution: string;
  minDurationSeconds: number | null;
  maxDurationSeconds: number | null;
  durationsSeconds: number[];
}

export interface VideoOperationCapability {
  operation: MediaOperation;
  durationOptions: VideoDurationOption[];
  aspectRatios: string[];
  inputRoles: VideoInputRole[];
  supportsAudio: boolean;
  supportsSeed: boolean;
}

export interface VideoModelManifest {
  providerId: string;
  modelId: string;
  displayName: string;
  apiVersion: string | null;
  releaseStatus: 'ga' | 'preview' | 'announced' | 'contract_pending' | 'deprecated' | 'unverified';
  selectable: boolean;
  operationCapabilities: VideoOperationCapability[];
  supportsNegativePrompt: boolean;
  supportsWebhook: boolean;
  supportsCancellation: boolean;
  cancellationScope: string;
  cancellationMayDeleteTerminalRecord: boolean;
  regions: string[];
  moderationPolicy: string;
  pricing: {
    currency: string | null;
    kind: string;
    creditsPerSecond: number | null;
    microsPerSecond: number | null;
    minimumAmountMicros: number | null;
    freeReferenceImages: number | null;
    additionalReferenceImageMicros: number | null;
    tiers: Array<{
      resolution: string;
      durationSeconds: number | null;
      amountMicros: number | null;
      microsPerSecond: number | null;
    }>;
    inputVideoTiers: Array<{
      resolution: string;
      durationSeconds: number | null;
      amountMicros: number | null;
      microsPerSecond: number | null;
    }>;
    note: string;
  };
  outputUrlTtl: string;
  watermarkPolicy: string;
  provenancePolicy: string;
  lastVerifiedAt: string;
}

export interface VideoProviderPreset {
  id: string;
  name: string;
  providerId: string;
  apiStyle: string;
  baseUrl: string;
  requiresApiKey: boolean;
  apiVersion: string | null;
  description: string;
  dataRegions: string[];
  retentionPolicy: string;
  models: VideoModelManifest[];
}

export interface VideoProviderConnectionRecord {
  id: string;
  providerId: string;
  displayName: string;
  officialBaseUrl: string;
  credentialScope: string;
  dataRegion: string | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export interface SaveVideoProviderConnectionRequest {
  id?: string | null;
  expectedRevision?: number | null;
  providerId: string;
  displayName: string;
  apiKey: string;
  dataRegion?: string | null;
}

export interface VideoWorkflowRecord {
  id: string;
  projectId: string | null;
  title: string;
  brief: Record<string, unknown>;
  aspectRatio: string;
  targetDurationMs: number;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export interface VideoWorkflowShotRecord {
  id: string;
  workflowId: string;
  ordinal: number;
  title: string;
  prompt: string;
  operation: MediaOperation;
  connectionId: string | null;
  providerId: string | null;
  modelId: string | null;
  apiVersion: string | null;
  durationSeconds: number;
  resolution: string;
  aspectRatio: string;
  inputAssets: VideoInputAsset[];
  seed: number | null;
  generateAudio: boolean | null;
  dataRegion: string | null;
  retentionPolicy: string;
  watermarkPolicy: string;
  provenancePolicy: string;
  allowCrossProviderFallback: boolean;
  selectedVariantId: string | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export interface VideoVariantJobRecord {
  id: string;
  state: MediaJobState;
  revision: number;
  providerId: string;
  providerSource: string;
  modelId: string;
  currentAttemptId: string | null;
  currentProviderTaskId: string | null;
  retryCount: number;
  maxAttempts: number;
  estimatedCostMicros: number | null;
  finalCostMicros: number | null;
  currency: string | null;
  cancellationRequestedAt: string | null;
  error: Record<string, unknown> | null;
  retryClassification: string | null;
  nextEligibleAt: string | null;
  outputAssetId: string | null;
  outputMediaType: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface VideoWorkflowVariantRecord {
  id: string;
  workflowId: string;
  shotId: string;
  ordinal: number;
  jobId: string;
  label: string;
  createdAt: string;
  job: VideoVariantJobRecord;
}

export interface VideoWorkflowShotSnapshot {
  shot: VideoWorkflowShotRecord;
  variants: VideoWorkflowVariantRecord[];
}

export interface VideoQueueSummary {
  draft: number;
  active: number;
  completed: number;
  failed: number;
  cancelled: number;
  providerUnknown: number;
  estimatedCostMicros: number;
  finalCostMicros: number;
}

export interface VideoWorkflowSnapshot {
  workflow: VideoWorkflowRecord;
  shots: VideoWorkflowShotSnapshot[];
  queue: VideoQueueSummary;
  dag: VideoWorkflowDag;
}

export type VideoWorkflowDagNodeKind = 'prompt' | 'reference_asset' | 'generate_video' | 'select_variant';

export interface VideoWorkflowDagNode {
  id: string;
  kind: VideoWorkflowDagNodeKind;
  shotId: string;
  dependsOn: string[];
  variantIds: string[];
  selectedVariantId: string | null;
}

export interface VideoWorkflowDag {
  workflowId: string;
  revision: number;
  nodes: VideoWorkflowDagNode[];
}

export interface CreateVideoWorkflowRequest {
  projectId?: string | null;
  title: string;
  brief: Record<string, unknown>;
  aspectRatio: string;
  targetDurationMs: number;
}

export interface UpdateVideoWorkflowRequest extends CreateVideoWorkflowRequest {
  workflowId: string;
  expectedRevision: number;
}

export interface VideoShotInput {
  title: string;
  prompt: string;
  operation: MediaOperation;
  connectionId?: string | null;
  providerId?: string | null;
  modelId?: string | null;
  apiVersion?: string | null;
  durationSeconds: number;
  resolution: string;
  aspectRatio: string;
  inputAssets: VideoInputAsset[];
  seed?: number | null;
  generateAudio?: boolean | null;
  allowCrossProviderFallback: boolean;
}

export interface AddVideoWorkflowShotRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shot: VideoShotInput;
}

export interface UpdateVideoWorkflowShotRequest extends AddVideoWorkflowShotRequest {
  shotId: string;
  expectedShotRevision: number;
}

export interface ReorderVideoWorkflowShotsRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  orderedShotIds: string[];
}

export interface ReorderVideoWorkflowVariantsRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shotId: string;
  expectedShotRevision: number;
  orderedVariantIds: string[];
}

export interface DeleteVideoWorkflowShotRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shotId: string;
  expectedShotRevision: number;
}

export interface QueueVideoShotVariantsRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shotId: string;
  expectedShotRevision: number;
  idempotencyKey: string;
  count: number;
  expectedConnectionRevision: number;
}

export interface PreviewVideoShotQueueRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shotId: string;
  expectedShotRevision: number;
  count: number;
}

export interface VideoQueueDisclosure {
  workflowId: string;
  shotId: string;
  shotRevision: number;
  providerId: string;
  modelId: string;
  apiVersion: string | null;
  officialBaseUrl: string;
  connectionId: string;
  connectionRevision: number;
  connectionName: string;
  credentialScope: string;
  dataRegion: string | null;
  retentionPolicy: string;
  deletionPolicy: string;
  orderedInputs: Array<{
    ordinal: number;
    role: string;
    uri: string;
    mediaType: string;
    byteLength: number | null;
    contentHashSha256: string;
  }>;
  count: number;
  estimatedCostMicrosPerVariant: number | null;
  estimatedCostMicrosTotal: number | null;
  currency: string | null;
  crossProviderFallbackAuthorized: boolean;
}

export interface VerifiedVideoReferenceImage {
  uri: string;
  mediaType: string;
  byteLength: number;
  contentHashSha256: string;
  width: number;
  height: number;
}

export interface RetryVideoVariantRequest {
  jobId: string;
  expectedJobRevision: number;
}

export interface CancelVideoVariantRequest {
  jobId: string;
  expectedJobRevision: number;
  reason: string;
  allowTerminalRecordDeletion: boolean;
}

export interface SelectVideoWorkflowVariantRequest {
  workflowId: string;
  expectedWorkflowRevision: number;
  shotId: string;
  expectedShotRevision: number;
  variantId: string;
}
