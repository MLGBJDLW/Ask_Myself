import type {
  ArtifactPayload,
  ImageAttachment,
  VisionTurnOverride,
} from '../types/conversation';

export type AgentExecutionMode = 'normal' | 'plan';
export type AgentPowerMode = 'standard' | 'nexus';
export type AgentCollaborationMode = 'direct' | 'mixtureOfAgents';
export type MoaPresetId = 'fastReview' | 'deepResearch' | 'crossModelCodeReview' | 'custom';
export type OrchestrationProfile = 'balanced' | 'deep' | 'codeUltra' | 'researchUltra' | 'custom';

export interface CustomOrchestrationOptions {
  maxIterations?: number | null;
  maxParallel?: number | null;
  maxCallsPerTurn?: number | null;
  delegatedTokenBudget?: number | null;
  verificationReservePercent?: number | null;
  retryLimit?: number | null;
  minEvidenceSources?: number | null;
}

export interface AgentChatRequestInput {
  conversationId: string;
  message: string;
  attachments?: ImageAttachment[];
  agentConfigId?: string | null;
  personaId?: string | null;
  skillIds?: string[];
  executionMode?: AgentExecutionMode | null;
  powerMode?: AgentPowerMode | null;
  collaborationMode?: AgentCollaborationMode | null;
  moaPreset?: MoaPresetId | null;
  orchestrationProfile?: OrchestrationProfile | null;
  customOrchestration?: CustomOrchestrationOptions | null;
  visionTurnOverride?: VisionTurnOverride | null;
  userArtifacts?: ArtifactPayload | null;
  taskOrchestratorRunId?: string | null;
  resumeCheckpointId?: string | null;
}

export interface AgentChatRequest {
  version: 1;
  idempotencyKey: string;
  conversationId: string;
  message: string;
  attachments: ImageAttachment[];
  agentConfigId: string | null;
  personaId: string | null;
  skillIds: string[];
  executionMode: AgentExecutionMode;
  powerMode: AgentPowerMode;
  collaborationMode: AgentCollaborationMode;
  moaPreset: MoaPresetId;
  orchestrationProfile: OrchestrationProfile;
  customOrchestration: CustomOrchestrationOptions | null;
  visionTurnOverride: VisionTurnOverride | null;
  userArtifacts: ArtifactPayload | null;
  taskOrchestratorRunId: string | null;
  resumeCheckpointId: string | null;
}

function interactionIdFromArtifacts(userArtifacts?: ArtifactPayload | null): string {
  return userArtifacts
    && !Array.isArray(userArtifacts)
    && userArtifacts.kind === 'questionResponse'
    && userArtifacts.version === 2
    && typeof userArtifacts.interactionId === 'string'
    ? userArtifacts.interactionId.trim()
    : '';
}

export function buildAgentChatRequest(
  input: AgentChatRequestInput,
  createId: () => string = () => crypto.randomUUID(),
): AgentChatRequest {
  const interactionId = interactionIdFromArtifacts(input.userArtifacts);
  const resumeCheckpointId = input.resumeCheckpointId?.trim() || '';
  if (interactionId && resumeCheckpointId) {
    throw new Error('Interaction continuation and checkpoint resume are mutually exclusive');
  }

  return {
    version: 1,
    idempotencyKey: resumeCheckpointId
      ? `task-resume:${resumeCheckpointId}`
      : interactionId
        ? `interaction-response:${interactionId}`
        : createId(),
    conversationId: input.conversationId,
    message: input.message,
    attachments: input.attachments ?? [],
    agentConfigId: input.agentConfigId ?? null,
    personaId: input.personaId ?? null,
    skillIds: input.skillIds ?? [],
    executionMode: input.executionMode ?? 'normal',
    powerMode: input.powerMode ?? 'standard',
    collaborationMode: input.collaborationMode ?? 'direct',
    moaPreset: input.moaPreset ?? 'fastReview',
    orchestrationProfile: input.orchestrationProfile ?? 'balanced',
    customOrchestration: input.customOrchestration ?? null,
    visionTurnOverride: input.visionTurnOverride ?? null,
    userArtifacts: input.userArtifacts ?? null,
    taskOrchestratorRunId: input.taskOrchestratorRunId ?? null,
    resumeCheckpointId: resumeCheckpointId || null,
  };
}
