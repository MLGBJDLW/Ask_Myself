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

export interface SubtaskRunArtifact {
  id: string;
  label: string;
  role?: string | null;
  status: string;
  phase?: string | null;
  task?: string | null;
  result?: string | null;
  errorMessage?: string | null;
  tokenBudget?: number | null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function asText(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function normalizePlanArtifact(value: unknown): PlanArtifact | null {
  const record = asRecord(value);
  if (!record || !Array.isArray(record.steps)) return null;
  const isClassicPlan = record.kind === 'plan';
  const isTypedTaskPlan = typeof record.routeKind === 'string' && typeof record.version === 'number';
  if (!isClassicPlan && !isTypedTaskPlan) return null;

  const steps: PlanStepArtifact[] = record.steps
    .map((step): PlanStepArtifact | null => {
      const item = asRecord(step);
      if (!item || typeof item.title !== 'string' || typeof item.status !== 'string') return null;
      const status = normalizePlanStepStatus(item.status);
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

function normalizePlanStepStatus(value: string): PlanStepStatus {
  if (value === 'inProgress') return 'in_progress';
  if (value === 'completed' || value === 'pending' || value === 'in_progress') {
    return value;
  }
  return 'pending';
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

function normalizeSubtaskRun(value: unknown): SubtaskRunArtifact | null {
  const record = asRecord(value);
  if (!record) return null;

  const input = asRecord(record.input);
  const output = asRecord(record.output);
  const outputRun = asRecord(output?.run);
  const outputJudgement = asRecord(output?.judgement);
  const directRun = record.kind === 'subagent_result' || record.status === 'done' ? record : null;
  const run = outputRun ?? outputJudgement ?? directRun;

  const id =
    asText(record.id) ??
    asText(input?.callLabel) ??
    asText(run?.id) ??
    asText(run?.task) ??
    asText(record.label);
  const label =
    asText(record.label) ??
    asText(input?.task) ??
    asText(run?.task) ??
    asText(run?.summary) ??
    id;
  if (!id || !label) return null;

  const rawStatus =
    asText(record.status) ??
    asText(run?.status) ??
    (record.isError === true || run?.isError === true ? 'failed' : null) ??
    'completed';
  const status = rawStatus === 'done' ? 'completed' : rawStatus === 'error' ? 'failed' : rawStatus;
  const role =
    asText(record.role) ??
    asText(input?.roleName) ??
    asText(input?.role) ??
    asText(run?.roleName) ??
    asText(run?.role);

  return {
    id,
    label,
    role,
    status,
    phase: asText(record.phase),
    task: asText(input?.task) ?? asText(run?.task),
    result: asText(run?.result) ?? asText(run?.summary),
    errorMessage: asText(record.errorMessage) ?? asText(run?.errorMessage) ?? asText(output?.error),
    tokenBudget: asNumber(record.tokenBudget) ?? asNumber(input?.reservedTokens),
  };
}

function normalizeSubtaskArtifacts(value: unknown): SubtaskRunArtifact[] | null {
  if (Array.isArray(value)) {
    const subtasks = value
      .map(normalizeSubtaskRun)
      .filter((subtask): subtask is SubtaskRunArtifact => Boolean(subtask));
    return subtasks.length ? subtasks : null;
  }

  const record = asRecord(value);
  if (!record) return null;

  if (Array.isArray(record.subtasks)) {
    return normalizeSubtaskArtifacts(record.subtasks);
  }
  if (Array.isArray(record.runs)) {
    return normalizeSubtaskArtifacts(record.runs);
  }
  const single = normalizeSubtaskRun(record);
  return single ? [single] : null;
}

function extractNestedArtifact<T>(
  value: unknown,
  normalize: (candidate: unknown) => T | null,
  depth = 0,
): T | null {
  const direct = normalize(value);
  if (direct) return direct;
  if (depth >= 6 || value == null) return null;

  if (Array.isArray(value)) {
    for (let i = value.length - 1; i >= 0; i -= 1) {
      const found = extractNestedArtifact(value[i], normalize, depth + 1);
      if (found) return found;
    }
    return null;
  }

  const record = asRecord(value);
  if (!record) return null;

  const preferredKeys = ['artifacts', 'toolCall', 'toolCalls', 'items'];
  const visited = new Set<string>();
  for (const key of preferredKeys) {
    if (!(key in record)) continue;
    visited.add(key);
    const found = extractNestedArtifact(record[key], normalize, depth + 1);
    if (found) return found;
  }

  for (const [key, child] of Object.entries(record)) {
    if (visited.has(key)) continue;
    if (child == null || ['string', 'number', 'boolean'].includes(typeof child)) continue;
    const found = extractNestedArtifact(child, normalize, depth + 1);
    if (found) return found;
  }

  return null;
}

export function extractPlanArtifact(value: unknown): PlanArtifact | null {
  return extractNestedArtifact(value, normalizePlanArtifact);
}

export function extractVerificationArtifact(value: unknown): VerificationArtifact | null {
  return extractNestedArtifact(value, normalizeVerificationArtifact);
}

export function extractSubtaskArtifacts(value: unknown): SubtaskRunArtifact[] {
  return extractNestedArtifact(value, normalizeSubtaskArtifacts) ?? [];
}

export function findLatestPlanArtifact(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
  taskPlan?: unknown,
): PlanArtifact | null {
  const taskRunPlan = extractPlanArtifact(taskPlan);
  if (taskRunPlan) return taskRunPlan;
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
  taskArtifacts?: unknown,
): VerificationArtifact | null {
  const taskRunVerification = extractVerificationArtifact(taskArtifacts);
  if (taskRunVerification) return taskRunVerification;
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

export function findLatestSubtaskArtifacts(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
  taskArtifacts?: unknown,
): SubtaskRunArtifact[] {
  const taskRunSubtasks = extractSubtaskArtifacts(taskArtifacts);
  if (taskRunSubtasks.length > 0) return taskRunSubtasks;
  for (let i = toolCalls.length - 1; i >= 0; i -= 1) {
    const artifact = extractSubtaskArtifacts(toolCalls[i].artifacts);
    if (artifact.length > 0) return artifact;
  }
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const artifact = extractSubtaskArtifacts(messages[i].artifacts);
    if (artifact.length > 0) return artifact;
  }
  return [];
}
