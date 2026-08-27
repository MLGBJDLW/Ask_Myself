import type { ActivityEvent, ArtifactPayload, ConversationMessage } from '../types/conversation';
import type { ToolCallEvent } from './streaming/protocol';

export interface SubagentUsage {
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  thinkingTokens?: number;
  toolPromptTokens?: number;
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

export interface SubagentContextSnapshot {
  id?: string;
  selectedMessageIds?: string[];
  tokenEstimate: number;
  contextCapacity: number | null;
  contextAuthority: 'user_override' | 'catalog' | 'model_profile' | 'provider_managed';
  handoffTokenBudget: number;
  droppedInvalidMessages: number;
}

export interface SubagentEffectiveModelBudgets {
  contextCapacity: number | null;
  parentHistoryHandoff: number;
  maxOutputPerStep: number | null;
  maxActualTokensPerWorker: number | null;
  contextAuthority: SubagentContextSnapshot['contextAuthority'];
  outputAuthority: 'user_override' | 'catalog_ceiling' | 'safe_default';
}

export interface SubagentPreflightReport {
  schemaVersion: number;
  completedStages: string[];
  providerId: string;
  effectiveModel: string;
  contextMessageCount: number;
  droppedInvalidContextMessages: number;
  reservedTokens: number;
  remainingTokenBudget: number;
  remainingCallBudget: number;
  runDeadlineMs: number;
}

export interface SubagentPreflightFailure {
  schemaVersion: number;
  stage: string;
  code: string;
  retryable: boolean;
  message: string;
}

export interface SubagentArtifact {
  kind: 'subagent_result';
  id?: string | null;
  status?: string | null;
  task: string;
  roleId?: string | null;
  roleName?: string | null;
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
  preflight?: SubagentPreflightReport | null;
  preflightFailure?: SubagentPreflightFailure | null;
  contextSnapshot?: SubagentContextSnapshot | null;
  effectiveModelBudgets?: SubagentEffectiveModelBudgets | null;
}

export interface SubagentBatchArtifact {
  kind: 'subagent_batch_result';
  lifecycleWorkers?: Array<{
    agentId: string;
    workerId?: string | null;
    task: string;
    roleId?: string | null;
    role?: string | null;
  }>;
  batchGoal?: string | null;
  workflowTemplate?: string | null;
  workflowTemplateLabel?: string | null;
  workflowTemplateDescription?: string | null;
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
  roleId?: string | null;
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
  status: 'running' | 'done' | 'error' | 'cancelled';
  task: string;
  roleId?: string | null;
  roleName?: string | null;
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
  preflight?: SubagentPreflightReport | null;
  preflightFailure?: SubagentPreflightFailure | null;
  contextSnapshot?: SubagentContextSnapshot | null;
  effectiveModelBudgets?: SubagentEffectiveModelBudgets | null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

export interface SubagentLifecycleProjection {
  status: 'running' | 'done' | 'error' | 'cancelled' | null;
  artifact: SubagentArtifact | null;
  streamedResult: string;
  thinking: string[];
  errorMessage: string | null;
}

function lifecycleEnvelope(event: ActivityEvent): Record<string, unknown> | null {
  const payload = asRecord(event.payload);
  if (!payload) return null;
  if (typeof payload.subagentEvent === 'string') return payload;
  const detail = asRecord(payload.detail);
  return detail && typeof detail.subagentEvent === 'string' ? detail : null;
}

export function projectSubagentLifecycle(
  events: ActivityEvent[] | undefined,
): SubagentLifecycleProjection {
  let status: SubagentLifecycleProjection['status'] = null;
  let artifact: SubagentArtifact | null = null;
  let streamedResult = '';
  const thinking: string[] = [];
  let errorMessage: string | null = null;

  for (const event of events ?? []) {
    const envelope = lifecycleEnvelope(event);
    if (!envelope) continue;
    const kind = typeof envelope.subagentEvent === 'string' ? envelope.subagentEvent : '';
    const detail = asRecord(envelope.detail);
    if (kind === 'spawned' || kind === 'queued' || kind === 'connected') status = 'running';
    if (kind === 'outputDelta') {
      const delta = typeof event.payload.data === 'string'
        ? event.payload.data
        : typeof detail?.delta === 'string' ? detail.delta : '';
      streamedResult += delta;
    }
    if (kind === 'thinkingDelta' && typeof detail?.delta === 'string') {
      thinking.push(detail.delta);
    }
    if (kind === 'completed') {
      status = 'done';
      const run = asRecord(detail?.result);
      artifact = run ? extractSubagentArtifact({ kind: 'subagent_result', ...run }) : artifact;
    }
    if (kind === 'failed') {
      status = 'error';
      errorMessage = typeof detail?.errorMessage === 'string' ? detail.errorMessage : errorMessage;
    }
    if (kind === 'cancelled') {
      status = 'cancelled';
      errorMessage = null;
    }
  }
  return { status, artifact, streamedResult, thinking, errorMessage };
}

/** Project each worker independently when a batch shares one parent tool call. */
export function projectSubagentLifecycleRuns(
  events: ActivityEvent[] | undefined,
): SubagentRun[] {
  const grouped = new Map<string, ActivityEvent[]>();
  for (const event of events ?? []) {
    const envelope = lifecycleEnvelope(event);
    const agentId = typeof envelope?.agentId === 'string' ? envelope.agentId : '';
    if (!agentId) continue;
    const group = grouped.get(agentId) ?? [];
    group.push(event);
    grouped.set(agentId, group);
  }

  const runs: SubagentRun[] = [];
  for (const [agentId, workerEvents] of grouped) {
    const projection = projectSubagentLifecycle(workerEvents);
    let task = '';
    let roleId: string | null = null;
    let role: string | null = null;
    for (const event of workerEvents) {
      const envelope = lifecycleEnvelope(event);
      if (envelope?.subagentEvent !== 'spawned') continue;
      const detail = asRecord(envelope.detail);
      task = typeof detail?.task === 'string' ? detail.task : task;
      roleId = typeof detail?.roleId === 'string' ? detail.roleId : roleId;
      role = typeof detail?.role === 'string' ? detail.role : role;
    }

    if (projection.artifact) {
      runs.push({
        ...buildRunFromArtifact(projection.artifact, agentId),
        status: projection.status ?? 'done',
        result: projection.artifact.result || projection.streamedResult || undefined,
        thinking: projection.artifact.thinking
          ?? (projection.thinking.length > 0 ? projection.thinking : null),
        isError: projection.status === 'error',
        content: projection.errorMessage ?? undefined,
      });
      continue;
    }
    if (!task) continue;
    runs.push({
      id: agentId,
      status: projection.status ?? 'running',
      task,
      roleId,
      role,
      result: projection.streamedResult || undefined,
      thinking: projection.thinking.length > 0 ? projection.thinking : null,
      toolEvents: [],
      sourceScopeApplied: false,
      isError: projection.status === 'error',
      content: projection.errorMessage ?? undefined,
    });
  }
  return runs;
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

function parseContextAuthority(
  value: unknown,
): SubagentContextSnapshot['contextAuthority'] | null {
  return value === 'user_override'
    || value === 'catalog'
    || value === 'model_profile'
    || value === 'provider_managed'
    ? value
    : null;
}

function parseContextSnapshot(value: unknown): SubagentContextSnapshot | null {
  const record = asRecord(value);
  if (!record) return null;
  const tokenEstimate = asNumber(record.tokenEstimate);
  const handoffTokenBudget = asNumber(record.handoffTokenBudget);
  const droppedInvalidMessages = asNumber(record.droppedInvalidMessages);
  const contextAuthority = parseContextAuthority(record.contextAuthority);
  if (
    tokenEstimate == null
    || handoffTokenBudget == null
    || droppedInvalidMessages == null
    || !contextAuthority
  ) return null;
  return {
    id: typeof record.id === 'string' ? record.id : undefined,
    selectedMessageIds: asStringArray(record.selectedMessageIds) ?? undefined,
    tokenEstimate,
    contextCapacity: asNumber(record.contextCapacity),
    contextAuthority,
    handoffTokenBudget,
    droppedInvalidMessages,
  };
}

function parseEffectiveModelBudgets(value: unknown): SubagentEffectiveModelBudgets | null {
  const record = asRecord(value);
  if (!record) return null;
  const contextAuthority = parseContextAuthority(record.contextAuthority);
  const parentHistoryHandoff = asNumber(record.parentHistoryHandoff);
  const outputAuthority = record.outputAuthority;
  if (
    !contextAuthority
    || parentHistoryHandoff == null
    || (outputAuthority !== 'user_override'
      && outputAuthority !== 'catalog_ceiling'
      && outputAuthority !== 'safe_default')
  ) return null;
  return {
    contextCapacity: asNumber(record.contextCapacity),
    parentHistoryHandoff,
    maxOutputPerStep: asNumber(record.maxOutputPerStep),
    maxActualTokensPerWorker: asNumber(record.maxActualTokensPerWorker),
    contextAuthority,
    outputAuthority,
  };
}

function parsePreflightReport(value: unknown): SubagentPreflightReport | null {
  const record = asRecord(value);
  if (!record) return null;
  const schemaVersion = asNumber(record.schemaVersion);
  const contextMessageCount = asNumber(record.contextMessageCount);
  const droppedInvalidContextMessages = asNumber(record.droppedInvalidContextMessages);
  const reservedTokens = asNumber(record.reservedTokens);
  const remainingTokenBudget = asNumber(record.remainingTokenBudget);
  const remainingCallBudget = asNumber(record.remainingCallBudget);
  const runDeadlineMs = asNumber(record.runDeadlineMs);
  const providerId = typeof record.providerId === 'string' ? record.providerId : '';
  const effectiveModel = typeof record.effectiveModel === 'string' ? record.effectiveModel : '';
  if (
    schemaVersion == null
    || contextMessageCount == null
    || droppedInvalidContextMessages == null
    || reservedTokens == null
    || remainingTokenBudget == null
    || remainingCallBudget == null
    || runDeadlineMs == null
    || !providerId
    || !effectiveModel
  ) return null;
  return {
    schemaVersion,
    completedStages: asStringArray(record.completedStages) ?? [],
    providerId,
    effectiveModel,
    contextMessageCount,
    droppedInvalidContextMessages,
    reservedTokens,
    remainingTokenBudget,
    remainingCallBudget,
    runDeadlineMs,
  };
}

function parsePreflightFailure(value: unknown): SubagentPreflightFailure | null {
  const record = asRecord(value);
  if (!record) return null;
  const schemaVersion = asNumber(record.schemaVersion);
  const stage = typeof record.stage === 'string' ? record.stage : '';
  const code = typeof record.code === 'string' ? record.code : '';
  const message = typeof record.message === 'string' ? record.message : '';
  if (schemaVersion == null || !stage || !code || !message) return null;
  return {
    schemaVersion,
    stage,
    code,
    retryable: record.retryable === true,
    message,
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
      roleId: typeof record.role_id === 'string' ? record.role_id.trim() : null,
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
  const preflight = parsePreflightReport(record.preflight);
  const preflightFailure = parsePreflightFailure(record.preflightFailure);
  const contextSnapshot = parseContextSnapshot(record.contextSnapshot);
  const effectiveModelBudgets = parseEffectiveModelBudgets(record.effectiveModelBudgets);

  return {
    kind: 'subagent_result',
    id: typeof record.id === 'string' ? record.id : null,
    status: typeof record.status === 'string' ? record.status : null,
    task: record.task.trim(),
    roleId: typeof record.roleId === 'string' ? record.roleId : null,
    roleName: typeof record.roleName === 'string' ? record.roleName : null,
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
          toolPromptTokens: typeof usageRecord.toolPromptTokens === 'number' ? usageRecord.toolPromptTokens : undefined,
        }
      : null,
    toolEvents,
    thinking,
    sourceScopeApplied: record.sourceScopeApplied === true,
    allowedTools,
    preflight,
    preflightFailure,
    contextSnapshot,
    effectiveModelBudgets,
  };
}

function buildRunFromArtifact(artifact: SubagentArtifact, id: string, content?: string): SubagentRun {
  return {
    id,
    status: artifact.status === 'running' || artifact.status === 'queued'
      ? 'running'
      : artifact.status === 'cancelled'
        ? 'cancelled'
        : artifact.status === 'error' || artifact.status === 'failed'
        ? 'error'
        : 'done',
    task: artifact.task,
    roleId: artifact.roleId ?? null,
    roleName: artifact.roleName ?? null,
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
    preflight: artifact.preflight ?? null,
    preflightFailure: artifact.preflightFailure ?? null,
    contextSnapshot: artifact.contextSnapshot ?? null,
    effectiveModelBudgets: artifact.effectiveModelBudgets ?? null,
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
        status: status === 'error'
          ? 'error'
          : status === 'cancelled'
            ? 'cancelled'
            : status === 'running' ? 'running' : 'done',
        isError: row.isError === true,
        content: typeof row.errorMessage === 'string' ? row.errorMessage : run.content,
      });
    });
  const lifecycleWorkers = Array.isArray(record.lifecycleWorkers)
    ? record.lifecycleWorkers.flatMap(item => {
        const row = asRecord(item);
        const agentId = typeof row?.agentId === 'string' ? row.agentId : '';
        const task = typeof row?.task === 'string' ? row.task : '';
        if (!agentId || !task) return [];
        return [{
          agentId,
          workerId: typeof row?.workerId === 'string' ? row.workerId : null,
          task,
          roleId: typeof row?.roleId === 'string' ? row.roleId : null,
          role: typeof row?.role === 'string' ? row.role : null,
        }];
      })
    : undefined;

  return {
    kind: 'subagent_batch_result',
    lifecycleWorkers,
    batchGoal: typeof record.batchGoal === 'string' ? record.batchGoal : null,
    workflowTemplate: typeof record.workflowTemplate === 'string' ? record.workflowTemplate : null,
    workflowTemplateLabel: typeof record.workflowTemplateLabel === 'string' ? record.workflowTemplateLabel : null,
    workflowTemplateDescription: typeof record.workflowTemplateDescription === 'string' ? record.workflowTemplateDescription : null,
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
          toolPromptTokens: typeof (record.usageTotal as Record<string, unknown>).toolPromptTokens === 'number' ? (record.usageTotal as Record<string, unknown>).toolPromptTokens as number : undefined,
        }
      : null,
    budget: parseBudgetSnapshot(record.budget),
  };
}

function buildRunFromToolCall(toolCall: ToolCallEvent): SubagentRun | null {
  if (toolCall.toolName !== 'spawn_subagent') return null;
  const initialArtifact = extractSubagentArtifact(toolCall.artifacts);
  const lifecycle = projectSubagentLifecycle(toolCall.activityEvents);
  const artifact = lifecycle.artifact ?? initialArtifact;
  const parsedArgs = parseSubagentArguments(toolCall.arguments);
  const task = artifact?.task ?? parsedArgs?.task ?? 'Delegated task';
  return {
    id: artifact?.id ?? toolCall.callId,
    status: lifecycle.status ?? (artifact?.status === 'running' || artifact?.status === 'queued'
      ? 'running'
      : toolCall.status === 'starting'
      || toolCall.status === 'preparing'
      || toolCall.status === 'approvalPending'
      || toolCall.status === 'running'
      ? 'running'
      : toolCall.status === 'cancelled'
        ? 'cancelled'
      : toolCall.status === 'done'
        ? 'done'
        : 'error'),
    task,
    roleId: artifact?.roleId ?? parsedArgs?.roleId ?? null,
    roleName: artifact?.roleName ?? null,
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
    result: artifact?.result || lifecycle.streamedResult || undefined,
    finishReason: artifact?.finishReason ?? null,
    usageTotal: artifact?.usageTotal ?? null,
    toolEvents: artifact?.toolEvents ?? [],
    thinking: artifact?.thinking ?? (lifecycle.thinking.length > 0 ? lifecycle.thinking : null),
    sourceScopeApplied: artifact?.sourceScopeApplied ?? false,
    allowedTools: artifact?.allowedTools ?? null,
    preflight: artifact?.preflight ?? null,
    preflightFailure: artifact?.preflightFailure ?? null,
    contextSnapshot: artifact?.contextSnapshot ?? null,
    effectiveModelBudgets: artifact?.effectiveModelBudgets ?? null,
    argumentsText: toolCall.arguments,
    isError: lifecycle.status === 'error' ? true : toolCall.isError,
    content: lifecycle.errorMessage ?? toolCall.content,
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
    const lifecycleRuns = projectSubagentLifecycleRuns(toolCall.activityEvents);
    if (lifecycleRuns.length > 0) return lifecycleRuns;
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
