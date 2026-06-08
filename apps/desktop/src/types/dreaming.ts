export type DreamTriggerKind = 'manual' | 'idle' | 'after_scan' | 'after_turn' | 'schedule';
export type DreamRunStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
export type DreamArtifactStatus = 'pending' | 'applied' | 'rejected' | 'expired' | 'undone';

export interface StartDreamInput {
  triggerKind?: DreamTriggerKind;
  scopeJson?: unknown;
  maxArtifacts?: number;
}

export interface DreamRun {
  id: string;
  triggerKind: DreamTriggerKind | string;
  scopeJson: unknown;
  status: DreamRunStatus | string;
  phase: string | null;
  summary: string | null;
  statsJson: Record<string, unknown>;
  error: string | null;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

export interface DreamRunEvent {
  id: string;
  runId: string;
  eventType: string;
  status: string | null;
  summary: string | null;
  payloadJson: unknown;
  createdAt: string;
}

export interface DreamArtifact {
  id: string;
  runId: string;
  kind: string;
  status: DreamArtifactStatus | string;
  title: string;
  summary: string;
  payloadJson: unknown;
  evidenceJson: unknown;
  applicationJson: unknown;
  confidence: number;
  reviewRequired: boolean;
  createdAt: string;
  appliedAt: string | null;
  rejectedAt: string | null;
  undoneAt: string | null;
}

export interface UpdateDreamArtifactInput {
  title?: string;
  summary?: string;
  payloadJson?: unknown;
  evidenceJson?: unknown;
  confidence?: number;
}

export interface DreamArtifactFilters {
  status?: DreamArtifactStatus | string;
  kind?: string;
  limit?: number;
}
