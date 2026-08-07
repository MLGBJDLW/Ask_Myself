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

export interface VideoTimelineRecord {
  id: string;
  workflowId: string;
  schemaVersion: number;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export interface VideoTimelineClipRecord {
  id: string;
  timelineId: string;
  shotId: string;
  shotTitle: string;
  variantId: string;
  selectedVariantId: string | null;
  assetId: string;
  assetContentHash: string;
  mediaType: string;
  ordinal: number;
  sourceStartUs: number;
  sourceDurationUs: number;
  availableDurationUs: number;
  stale: boolean;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export type VideoTimelineExportState =
  | 'validating'
  | 'queued'
  | 'running'
  | 'verifying'
  | 'publishing'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'interrupted';

export type VideoTimelineExportStageKind = 'validate' | 'normalize' | 'concatenate' | 'verify' | 'publish';
export type VideoTimelineExportStageState = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';

export interface VideoTimelineOutputProfile {
  schemaVersion: 1;
  width: number;
  height: number;
  fit: 'contain';
  fpsNumerator: number;
  fpsDenominator: number;
  pixelFormat: 'yuv420p';
  videoCodec: 'h264';
  videoProfile: 'high';
  videoLevel: 52;
  videoTimeBaseNumerator: 1;
  videoTimeBaseDenominator: 90000;
  colorPrimaries: 'bt709';
  colorTransfer: 'bt709';
  colorSpace: 'bt709';
  colorRange: 'tv';
  videoPreset: 'medium' | 'fast';
  videoCrf: number;
  audioCodec: 'aac';
  audioSampleRate: 48000;
  audioChannelLayout: 'stereo';
}

export interface VideoTimelineExportClipSnapshot {
  ordinal: number;
  clipId: string;
  clipRevision: number;
  shotId: string;
  shotTitle: string;
  variantId: string;
  assetId: string;
  assetContentHash: string;
  sourceStartUs: number;
  sourceDurationUs: number;
}

export interface VideoTimelineExportStageRecord {
  ordinal: number;
  stageKind: VideoTimelineExportStageKind;
  clipOrdinal: number | null;
  state: VideoTimelineExportStageState;
  fingerprintSha256: string;
  attemptCount: number;
  progressBasisPoints: number;
  intermediateAssetId: string | null;
  error: Record<string, unknown> | null;
  startedAt: string | null;
  completedAt: string | null;
  updatedAt: string;
}

export interface VideoTimelineExportRecord {
  id: string;
  workflowId: string;
  timelineId: string;
  timelineRevision: number;
  state: VideoTimelineExportState;
  currentStage: VideoTimelineExportStageKind;
  outputProfile: VideoTimelineOutputProfile;
  ffmpegIdentity: Record<string, unknown> | null;
  clips: VideoTimelineExportClipSnapshot[];
  inputFingerprintSha256: string;
  destinationPath: string;
  progressBasisPoints: number;
  outputAssetId: string | null;
  cancellationRequestedAt: string | null;
  error: Record<string, unknown> | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  stages: VideoTimelineExportStageRecord[];
}

export interface VideoTimelineSnapshot {
  timeline: VideoTimelineRecord;
  clips: VideoTimelineClipRecord[];
  exports: VideoTimelineExportRecord[];
}

export interface AddVideoTimelineClipRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  shotId: string;
  variantId: string;
}

export interface RefreshVideoTimelineClipRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  clipId: string;
  expectedClipRevision: number;
}

export interface UpdateVideoTimelineClipRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  clipId: string;
  expectedClipRevision: number;
  sourceStartUs: number;
  sourceDurationUs: number;
}

export interface ReorderVideoTimelineClipsRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  orderedClipIds: string[];
}

export interface RemoveVideoTimelineClipRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  clipId: string;
  expectedClipRevision: number;
}

export interface CreateVideoTimelineExportRequest {
  workflowId: string;
  expectedTimelineRevision: number;
  idempotencyKey: string;
  destinationPath: string;
  outputProfile: VideoTimelineOutputProfile;
}

export interface CancelVideoTimelineExportRequest {
  exportId: string;
  expectedRevision: number;
}

export interface RetryVideoTimelineExportRequest {
  exportId: string;
  expectedRevision: number;
  destinationPath: string;
}
