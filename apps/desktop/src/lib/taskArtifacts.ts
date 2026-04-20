import type { ConversationMessage } from '../types/conversation';
import type { ToolCallEvent } from './useAgentStream';

export type PlanStepStatus = 'pending' | 'in_progress' | 'completed';
export type VerificationStatus = 'pending' | 'passed' | 'failed' | 'skipped';
export type VerificationOverallStatus = 'pending' | 'passed' | 'failed' | 'partial';

export interface PlanStepArtifact {
  id?: string | null;
  title: string;
  status: PlanStepStatus;
  notes?: string | null;
}

export interface PlanArtifact {
  kind: 'plan';
  title?: string | null;
  explanation?: string | null;
  steps: PlanStepArtifact[];
  counts?: {
    total?: number;
    completed?: number;
    inProgress?: number;
    pending?: number;
  } | null;
  updatedAt?: string | null;
}

export interface VerificationCheckArtifact {
  name: string;
  status: VerificationStatus;
  details?: string | null;
}

export interface VerificationArtifact {
  kind: 'verification';
  summary?: string | null;
  overallStatus?: VerificationOverallStatus | null;
  checks: VerificationCheckArtifact[];
  counts?: {
    total?: number;
    passed?: number;
    failed?: number;
    pending?: number;
    skipped?: number;
  } | null;
  updatedAt?: string | null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function normalizePlanArtifact(value: unknown): PlanArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== 'plan' || !Array.isArray(record.steps)) return null;

  const steps: PlanStepArtifact[] = record.steps
    .map((step): PlanStepArtifact | null => {
      const item = asRecord(step);
      if (!item || typeof item.title !== 'string' || typeof item.status !== 'string') return null;
      const status = item.status as PlanStepStatus;
      if (!['pending', 'in_progress', 'completed'].includes(status)) return null;
      return {
        id: typeof item.id === 'string' ? item.id : null,
        title: item.title.trim(),
        status,
        notes: typeof item.notes === 'string' ? item.notes : null,
      };
    })
    .filter((step): step is PlanStepArtifact => Boolean(step && step.title));

  if (steps.length === 0) return null;

  const counts = asRecord(record.counts);
  return {
    kind: 'plan',
    title: typeof record.title === 'string' ? record.title : null,
    explanation: typeof record.explanation === 'string' ? record.explanation : null,
    steps,
    counts: counts
      ? {
          total: typeof counts.total === 'number' ? counts.total : undefined,
          completed: typeof counts.completed === 'number' ? counts.completed : undefined,
          inProgress: typeof counts.inProgress === 'number' ? counts.inProgress : undefined,
          pending: typeof counts.pending === 'number' ? counts.pending : undefined,
        }
      : null,
    updatedAt: typeof record.updatedAt === 'string' ? record.updatedAt : null,
  };
}

function normalizeVerificationArtifact(value: unknown): VerificationArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== 'verification' || !Array.isArray(record.checks)) return null;

  const checks: VerificationCheckArtifact[] = record.checks
    .map((check): VerificationCheckArtifact | null => {
      const item = asRecord(check);
      if (!item || typeof item.name !== 'string' || typeof item.status !== 'string') return null;
      const status = item.status as VerificationStatus;
      if (!['pending', 'passed', 'failed', 'skipped'].includes(status)) return null;
      return {
        name: item.name.trim(),
        status,
        details: typeof item.details === 'string' ? item.details : null,
      };
    })
    .filter((check): check is VerificationCheckArtifact => Boolean(check && check.name));

  if (checks.length === 0) return null;

  const overall = record.overallStatus;
  const counts = asRecord(record.counts);
  return {
    kind: 'verification',
    summary: typeof record.summary === 'string' ? record.summary : null,
    overallStatus:
      typeof overall === 'string' &&
      ['pending', 'passed', 'failed', 'partial'].includes(overall)
        ? (overall as VerificationOverallStatus)
        : null,
    checks,
    counts: counts
      ? {
          total: typeof counts.total === 'number' ? counts.total : undefined,
          passed: typeof counts.passed === 'number' ? counts.passed : undefined,
          failed: typeof counts.failed === 'number' ? counts.failed : undefined,
          pending: typeof counts.pending === 'number' ? counts.pending : undefined,
          skipped: typeof counts.skipped === 'number' ? counts.skipped : undefined,
        }
      : null,
    updatedAt: typeof record.updatedAt === 'string' ? record.updatedAt : null,
  };
}

export function extractPlanArtifact(value: unknown): PlanArtifact | null {
  return normalizePlanArtifact(value);
}

export function extractVerificationArtifact(value: unknown): VerificationArtifact | null {
  return normalizeVerificationArtifact(value);
}

export function findLatestPlanArtifact(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
): PlanArtifact | null {
  for (let i = toolCalls.length - 1; i >= 0; i -= 1) {
    const artifact = extractPlanArtifact(toolCalls[i].artifacts);
    if (artifact) return artifact;
  }
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const artifact = extractPlanArtifact(messages[i].artifacts);
    if (artifact) return artifact;
  }
  return null;
}

export function findLatestVerificationArtifact(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
): VerificationArtifact | null {
  for (let i = toolCalls.length - 1; i >= 0; i -= 1) {
    const artifact = extractVerificationArtifact(toolCalls[i].artifacts);
    if (artifact) return artifact;
  }
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const artifact = extractVerificationArtifact(messages[i].artifacts);
    if (artifact) return artifact;
  }
  return null;
}
