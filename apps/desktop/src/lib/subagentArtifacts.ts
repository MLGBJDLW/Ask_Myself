import type { ArtifactPayload, ConversationMessage } from '../types/conversation';
import type { ToolCallEvent } from './useAgentStream';

export interface SubagentUsage {
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  thinkingTokens?: number;
}

export interface SubagentToolEvent {
  phase: 'start' | 'result';
  callId: string;
  toolName: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
}

export interface SubagentEvidenceHandoff {
  chunkId: string;
  path: string;
  title: string;
  excerpt: string;
}

export interface SubagentAppliedSkill {
  id: string;
  name: string;
}

export interface SubagentBudgetSnapshot {
  maxParallel: number;
  maxCallsPerTurn: number;
  callsStarted: number;
  remainingCalls: number;
  tokenBudget: number;
  tokensSpent: number;
  remainingTokens: number;
}

export interface SubagentArtifact {
  kind: 'subagent_result';
  task: string;
  role?: string | null;
  expectedOutput?: string | null;
  acceptanceCriteria?: string[] | null;
  evidenceChunkIds?: string[] | null;
  evidenceHandoff?: SubagentEvidenceHandoff[] | null;
  requestedSourceScope?: string[] | null;
  effectiveSourceScope?: string[] | null;
  requestedAllowedTools?: string[] | null;
  allowedSkills?: SubagentAppliedSkill[] | null;
  parallelGroup?: string | null;
  deliverableStyle?: string | null;
  returnSections?: string[] | null;
  result: string;
  finishReason?: string | null;
  usageTotal?: SubagentUsage | null;
  toolEvents: SubagentToolEvent[];
  thinking?: string[] | null;
  sourceScopeApplied?: boolean;
  allowedTools?: string[] | null;
}

export interface SubagentBatchArtifact {
  kind: 'subagent_batch_result';
  batchGoal?: string | null;
  parallelGroup?: string | null;
  requestedMaxParallel?: number | null;
  effectiveMaxParallel?: number | null;
  completedRuns?: number;
  failedRuns?: number;
  budgetBefore?: SubagentBudgetSnapshot | null;
  budgetAfter?: SubagentBudgetSnapshot | null;
  runs: SubagentRun[];
}

export interface SubagentJudgementArtifact {
  kind: 'subagent_judgement';
  task?: string | null;
  rubric?: string[] | null;
  decisionMode: string;
  expectedOutput?: string | null;
  parallelGroup?: string | null;
  winnerIds: string[];
  confidence?: string | null;
  summary: string;
  rationale?: string | null;
  rawResponse: string;
  candidates: Array<{
    id: string;
    label?: string | null;
    result: string;
    evidenceSummary?: string | null;
    concerns?: string[] | null;
  }>;
  usageTotal?: SubagentUsage | null;
  budget?: SubagentBudgetSnapshot | null;
}

export interface PendingSubagentArgs {
  task: string;
  role?: string | null;
  context?: string | null;
  expectedOutput?: string | null;
  maxIterations?: number | null;
  acceptanceCriteria?: string[] | null;
  evidenceChunkIds?: string[] | null;
  sourceIds?: string[] | null;
  allowedTools?: string[] | null;
  parallelGroup?: string | null;
  deliverableStyle?: string | null;
  returnSections?: string[] | null;
}

export interface SubagentRun {
  id: string;
  status: 'running' | 'done' | 'error';
  task: string;
  role?: string | null;
  expectedOutput?: string | null;
  acceptanceCriteria?: string[] | null;
  evidenceChunkIds?: string[] | null;
  evidenceHandoff?: SubagentEvidenceHandoff[] | null;
  requestedSourceScope?: string[] | null;
  effectiveSourceScope?: string[] | null;
  requestedAllowedTools?: string[] | null;
  allowedSkills?: SubagentAppliedSkill[] | null;
  parallelGroup?: string | null;
  deliverableStyle?: string | null;
  returnSections?: string[] | null;
  result?: string;
  finishReason?: string | null;
  usageTotal?: SubagentUsage | null;
  toolEvents: SubagentToolEvent[];
  thinking?: string[] | null;
  sourceScopeApplied?: boolean;
  allowedTools?: string[] | null;
  argumentsText?: string;
  isError?: boolean;
  content?: string;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function asStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  const items = value
    .map(item => (typeof item === 'string' ? item.trim() : ''))
    .filter(Boolean);
  return items.length > 0 ? items : [];
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function parseAppliedSkills(value: unknown): SubagentAppliedSkill[] | null {
  if (!Array.isArray(value)) return null;
  const skills = value
    .map(item => {
      const row = asRecord(item);
      if (!row) return null;
      const id = typeof row.id === 'string' ? row.id : '';
      const name = typeof row.name === 'string' ? row.name : '';
      if (!id || !name) return null;
      return { id, name };
    })
    .filter((item): item is SubagentAppliedSkill => Boolean(item));
  return skills.length > 0 ? skills : [];
}

function parseBudgetSnapshot(value: unknown): SubagentBudgetSnapshot | null {
  const record = asRecord(value);
  if (!record) return null;
  const maxParallel = asNumber(record.maxParallel);
  const maxCallsPerTurn = asNumber(record.maxCallsPerTurn);
  const callsStarted = asNumber(record.callsStarted);
  const remainingCalls = asNumber(record.remainingCalls);
  const tokenBudget = asNumber(record.tokenBudget);
  const tokensSpent = asNumber(record.tokensSpent);
  const remainingTokens = asNumber(record.remainingTokens);
  if (
    maxParallel == null
    || maxCallsPerTurn == null
    || callsStarted == null
    || remainingCalls == null
    || tokenBudget == null
    || tokensSpent == null
    || remainingTokens == null
  ) {
    return null;
  }
  return {
    maxParallel,
    maxCallsPerTurn,
    callsStarted,
    remainingCalls,
    tokenBudget,
    tokensSpent,
    remainingTokens,
  };
}

export function parseSubagentArguments(raw?: string): PendingSubagentArgs | null {
  if (!raw) return null;
  try {
    const record = JSON.parse(raw) as Record<string, unknown>;
    const task = typeof record.task === 'string' ? record.task.trim() : '';
    if (!task) return null;
    return {
      task,
      role: typeof record.role === 'string' ? record.role.trim() : null,
      context: typeof record.context === 'string' ? record.context.trim() : null,
      expectedOutput: typeof record.expected_output === 'string'
        ? record.expected_output.trim()
        : null,
      maxIterations: typeof record.max_iterations === 'number' ? record.max_iterations : null,
      acceptanceCriteria: asStringArray(record.acceptance_criteria),
      evidenceChunkIds: asStringArray(record.evidence_chunk_ids),
      sourceIds: asStringArray(record.source_ids),
      allowedTools: asStringArray(record.allowed_tools),
      parallelGroup: typeof record.parallel_group === 'string' ? record.parallel_group.trim() : null,
      deliverableStyle: typeof record.deliverable_style === 'string' ? record.deliverable_style.trim() : null,
      returnSections: asStringArray(record.return_sections),
    };
  } catch {
    return null;
  }
}

export function extractSubagentArtifact(value: unknown): SubagentArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== 'subagent_result' || typeof record.task !== 'string') return null;

  const toolEventsRaw = Array.isArray(record.toolEvents) ? record.toolEvents : [];
  const toolEvents: SubagentToolEvent[] = toolEventsRaw
    .map((event): SubagentToolEvent | null => {
      const item = asRecord(event);
      if (!item) return null;
      const phase = item.phase;
      const callId = typeof item.callId === 'string' ? item.callId : '';
      const toolName = typeof item.toolName === 'string' ? item.toolName : '';
      if ((phase !== 'start' && phase !== 'result') || !callId || !toolName) return null;
      return {
        phase,
        callId,
        toolName,
        arguments: typeof item.arguments === 'string' ? item.arguments : undefined,
        content: typeof item.content === 'string' ? item.content : undefined,
        isError: typeof item.isError === 'boolean' ? item.isError : undefined,
        artifacts: item.artifacts as ArtifactPayload | undefined,
      };
    })
    .filter((event): event is SubagentToolEvent => Boolean(event));

  const usageRecord = asRecord(record.usageTotal);
  const thinking = asStringArray(record.thinking);
  const allowedTools = asStringArray(record.allowedTools);
  const allowedSkills = parseAppliedSkills(record.allowedSkills);
  const requestedAllowedTools = asStringArray(record.requestedAllowedTools);
  const acceptanceCriteria = asStringArray(record.acceptanceCriteria);
  const evidenceChunkIds = asStringArray(record.evidenceChunkIds);
  const requestedSourceScope = asStringArray(record.requestedSourceScope);
  const effectiveSourceScope = asStringArray(record.effectiveSourceScope);
  const returnSections = asStringArray(record.returnSections);
  const evidenceHandoffRaw = Array.isArray(record.evidenceHandoff) ? record.evidenceHandoff : [];
  const evidenceHandoff: SubagentEvidenceHandoff[] = evidenceHandoffRaw
    .map(item => {
      const row = asRecord(item);
      if (!row) return null;
      const chunkId = typeof row.chunkId === 'string' ? row.chunkId : '';
      const path = typeof row.path === 'string' ? row.path : '';
      const title = typeof row.title === 'string' ? row.title : '';
      const excerpt = typeof row.excerpt === 'string' ? row.excerpt : '';
      if (!chunkId || !path || !excerpt) return null;
      return { chunkId, path, title, excerpt };
    })
    .filter((item): item is SubagentEvidenceHandoff => Boolean(item));

  return {
    kind: 'subagent_result',
    task: record.task.trim(),
    role: typeof record.role === 'string' ? record.role : null,
    expectedOutput: typeof record.expectedOutput === 'string' ? record.expectedOutput : null,
    acceptanceCriteria,
    evidenceChunkIds,
    evidenceHandoff,
    requestedSourceScope,
    effectiveSourceScope,
    requestedAllowedTools,
    allowedSkills,
    parallelGroup: typeof record.parallelGroup === 'string' ? record.parallelGroup : null,
    deliverableStyle: typeof record.deliverableStyle === 'string' ? record.deliverableStyle : null,
    returnSections,
    result: typeof record.result === 'string' ? record.result : '',
    finishReason: typeof record.finishReason === 'string' ? record.finishReason : null,
    usageTotal: usageRecord
      ? {
          promptTokens: typeof usageRecord.promptTokens === 'number' ? usageRecord.promptTokens : undefined,
          completionTokens: typeof usageRecord.completionTokens === 'number' ? usageRecord.completionTokens : undefined,
          totalTokens: typeof usageRecord.totalTokens === 'number' ? usageRecord.totalTokens : undefined,
          thinkingTokens: typeof usageRecord.thinkingTokens === 'number' ? usageRecord.thinkingTokens : undefined,
        }
      : null,
    toolEvents,
    thinking,
    sourceScopeApplied: record.sourceScopeApplied === true,
    allowedTools,
  };
}

function buildRunFromArtifact(artifact: SubagentArtifact, id: string, content?: string): SubagentRun {
  return {
    id,
    status: 'done',
    task: artifact.task,
    role: artifact.role ?? null,
    expectedOutput: artifact.expectedOutput ?? null,
    acceptanceCriteria: artifact.acceptanceCriteria ?? null,
    evidenceChunkIds: artifact.evidenceChunkIds ?? null,
    evidenceHandoff: artifact.evidenceHandoff ?? null,
    requestedSourceScope: artifact.requestedSourceScope ?? null,
    effectiveSourceScope: artifact.effectiveSourceScope ?? null,
    requestedAllowedTools: artifact.requestedAllowedTools ?? null,
    allowedSkills: artifact.allowedSkills ?? null,
    parallelGroup: artifact.parallelGroup ?? null,
    deliverableStyle: artifact.deliverableStyle ?? null,
    returnSections: artifact.returnSections ?? null,
    result: artifact.result,
    finishReason: artifact.finishReason ?? null,
    usageTotal: artifact.usageTotal ?? null,
    toolEvents: artifact.toolEvents,
    thinking: artifact.thinking ?? null,
    sourceScopeApplied: artifact.sourceScopeApplied ?? false,
    allowedTools: artifact.allowedTools ?? null,
    content,
  };
}

export function extractSubagentBatchArtifact(value: unknown): SubagentBatchArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== 'subagent_batch_result') return null;
  const runsRaw = Array.isArray(record.runs) ? record.runs : [];
  const runs: SubagentRun[] = [];
  runsRaw.forEach((item, index) => {
      const row = asRecord(item);
      if (!row) return;
      const artifact = extractSubagentArtifact({ kind: 'subagent_result', ...row });
      if (!artifact) return;
      const status = typeof row.status === 'string' ? row.status : 'done';
      const run = buildRunFromArtifact(
        artifact,
        typeof row.id === 'string' ? row.id : `batch-run-${index}`,
        typeof row.result === 'string' ? row.result : undefined,
      );
      runs.push({
        ...run,
        status: status === 'error' ? 'error' : status === 'running' ? 'running' : 'done',
        isError: row.isError === true,
        content: typeof row.errorMessage === 'string' ? row.errorMessage : run.content,
      });
    });

  return {
    kind: 'subagent_batch_result',
    batchGoal: typeof record.batchGoal === 'string' ? record.batchGoal : null,
    parallelGroup: typeof record.parallelGroup === 'string' ? record.parallelGroup : null,
    requestedMaxParallel: asNumber(record.requestedMaxParallel),
    effectiveMaxParallel: asNumber(record.effectiveMaxParallel),
    completedRuns: asNumber(record.completedRuns) ?? undefined,
    failedRuns: asNumber(record.failedRuns) ?? undefined,
    budgetBefore: parseBudgetSnapshot(record.budgetBefore),
    budgetAfter: parseBudgetSnapshot(record.budgetAfter),
    runs,
  };
}

export function extractSubagentJudgementArtifact(value: unknown): SubagentJudgementArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== 'subagent_judgement') return null;
  const candidatesRaw = Array.isArray(record.candidates) ? record.candidates : [];
  const candidates = candidatesRaw
    .map(item => {
      const row = asRecord(item);
      if (!row || typeof row.id !== 'string' || typeof row.result !== 'string') return null;
      return {
        id: row.id,
        label: typeof row.label === 'string' ? row.label : null,
        result: row.result,
        evidenceSummary: typeof row.evidenceSummary === 'string' ? row.evidenceSummary : null,
        concerns: asStringArray(row.concerns),
      };
    })
    .filter((item): item is NonNullable<typeof item> => Boolean(item));

  const decisionMode = typeof record.decisionMode === 'string' ? record.decisionMode : '';
  const summary = typeof record.summary === 'string' ? record.summary : '';
  if (!decisionMode || !summary) return null;

  return {
    kind: 'subagent_judgement',
    task: typeof record.task === 'string' ? record.task : null,
    rubric: asStringArray(record.rubric),
    decisionMode,
    expectedOutput: typeof record.expectedOutput === 'string' ? record.expectedOutput : null,
    parallelGroup: typeof record.parallelGroup === 'string' ? record.parallelGroup : null,
    winnerIds: asStringArray(record.winnerIds) ?? [],
    confidence: typeof record.confidence === 'string' ? record.confidence : null,
    summary,
    rationale: typeof record.rationale === 'string' ? record.rationale : null,
    rawResponse: typeof record.rawResponse === 'string' ? record.rawResponse : summary,
    candidates,
    usageTotal: asRecord(record.usageTotal)
      ? {
          promptTokens: typeof (record.usageTotal as Record<string, unknown>).promptTokens === 'number' ? (record.usageTotal as Record<string, unknown>).promptTokens as number : undefined,
          completionTokens: typeof (record.usageTotal as Record<string, unknown>).completionTokens === 'number' ? (record.usageTotal as Record<string, unknown>).completionTokens as number : undefined,
          totalTokens: typeof (record.usageTotal as Record<string, unknown>).totalTokens === 'number' ? (record.usageTotal as Record<string, unknown>).totalTokens as number : undefined,
          thinkingTokens: typeof (record.usageTotal as Record<string, unknown>).thinkingTokens === 'number' ? (record.usageTotal as Record<string, unknown>).thinkingTokens as number : undefined,
        }
      : null,
    budget: parseBudgetSnapshot(record.budget),
  };
}

function buildRunFromToolCall(toolCall: ToolCallEvent): SubagentRun | null {
  if (toolCall.toolName !== 'spawn_subagent') return null;
  const artifact = extractSubagentArtifact(toolCall.artifacts);
  const parsedArgs = parseSubagentArguments(toolCall.arguments);
  const task = artifact?.task ?? parsedArgs?.task ?? 'Delegated task';
  return {
    id: toolCall.callId,
    status: toolCall.status,
    task,
    role: artifact?.role ?? parsedArgs?.role ?? null,
    expectedOutput: artifact?.expectedOutput ?? parsedArgs?.expectedOutput ?? null,
    acceptanceCriteria: artifact?.acceptanceCriteria ?? parsedArgs?.acceptanceCriteria ?? null,
    evidenceChunkIds: artifact?.evidenceChunkIds ?? parsedArgs?.evidenceChunkIds ?? null,
    evidenceHandoff: artifact?.evidenceHandoff ?? null,
    requestedSourceScope: artifact?.requestedSourceScope ?? parsedArgs?.sourceIds ?? null,
    effectiveSourceScope: artifact?.effectiveSourceScope ?? null,
    requestedAllowedTools: artifact?.requestedAllowedTools ?? parsedArgs?.allowedTools ?? null,
    allowedSkills: artifact?.allowedSkills ?? null,
    parallelGroup: artifact?.parallelGroup ?? parsedArgs?.parallelGroup ?? null,
    deliverableStyle: artifact?.deliverableStyle ?? parsedArgs?.deliverableStyle ?? null,
    returnSections: artifact?.returnSections ?? parsedArgs?.returnSections ?? null,
    result: artifact?.result ?? undefined,
    finishReason: artifact?.finishReason ?? null,
    usageTotal: artifact?.usageTotal ?? null,
    toolEvents: artifact?.toolEvents ?? [],
    thinking: artifact?.thinking ?? null,
    sourceScopeApplied: artifact?.sourceScopeApplied ?? false,
    allowedTools: artifact?.allowedTools ?? null,
    argumentsText: toolCall.arguments,
    isError: toolCall.isError,
    content: toolCall.content,
  };
}

function buildRunFromMessage(message: ConversationMessage): SubagentRun | null {
  const artifact = extractSubagentArtifact(message.artifacts);
  if (!artifact) return null;
  return buildRunFromArtifact(artifact, message.toolCallId ?? message.id, message.content);
}

export function findVisibleSubagentRuns(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
  limit = 4,
): SubagentRun[] {
  const liveRuns = toolCalls.flatMap(toolCall => {
    const direct = buildRunFromToolCall(toolCall);
    if (direct) return [direct];
    const batch = extractSubagentBatchArtifact(toolCall.artifacts);
    return batch?.runs ?? [];
  });

  if (liveRuns.length > 0) {
    return liveRuns.slice(-limit);
  }

  const historicalRuns: SubagentRun[] = [];
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const run = buildRunFromMessage(messages[i]);
    if (run) historicalRuns.push(run);
    const batch = extractSubagentBatchArtifact(messages[i].artifacts);
    if (batch) historicalRuns.push(...batch.runs.slice().reverse());
    if (historicalRuns.length >= limit) break;
  }

  return historicalRuns;
}
