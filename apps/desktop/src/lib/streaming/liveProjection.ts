import type {
  ApprovalRequest,
  ContextUsageBreakdown,
  ProviderConnectionState,
} from '../../types/conversation';
import type { UsageTotal } from './protocol';
import type { InternalStreamState } from './state';
import {
  appendStatusTraceEvent,
  applyTerminalProjection,
  markRoundsToolCallsFinished,
  markToolCallsFinished,
  resetActiveStreamBlocks,
  syncTraceToolEvents,
} from './terminalProjection';

const CONNECTION_STATES = new Set([
  'degraded',
  'reconnecting',
  'recovered',
  'offline',
  'failed',
]);

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

export function applyConnectionStateEvent(
  state: InternalStreamState,
  candidate: unknown,
  label?: string,
): void {
  const value = asRecord(candidate);
  if (typeof value.state !== 'string' || !CONNECTION_STATES.has(value.state)) return;
  if (typeof value.providerId !== 'string' || typeof value.modelId !== 'string') return;

  const connectionState: ProviderConnectionState = {
    state: value.state as ProviderConnectionState['state'],
    providerId: value.providerId,
    modelId: value.modelId,
    errorCategory: typeof value.errorCategory === 'string'
      ? value.errorCategory as ProviderConnectionState['errorCategory']
      : null,
    attempt: typeof value.attempt === 'number' ? value.attempt : 0,
    maxAttempts: typeof value.maxAttempts === 'number' ? value.maxAttempts : 0,
    nextRetryAt: typeof value.nextRetryAt === 'string' ? value.nextRetryAt : null,
    recoverable: value.recoverable === true,
    queuedUserInputs: typeof value.queuedUserInputs === 'number' ? value.queuedUserInputs : 0,
    turnPreserved: value.turnPreserved !== false,
  };
  state.connectionState = connectionState;

  if (label) {
    appendStatusTraceEvent(
      state,
      label,
      connectionState.state === 'failed' || connectionState.state === 'offline'
        ? 'error'
        : connectionState.state === 'recovered' ? 'success' : 'muted',
      'user',
      'recovery',
    );
  }
}

function extractMessageText(message: unknown): string | null {
  const record = asRecord(message);
  if (typeof record.content === 'string' && record.content.trim().length > 0) {
    return record.content;
  }
  if (!Array.isArray(record.parts)) return null;
  const text = record.parts
    .map(part => {
      const item = asRecord(part);
      return typeof item.text === 'string' ? item.text : '';
    })
    .join('');
  return text.trim().length > 0 ? text : null;
}

export function applyUsageUpdateEvent(
  state: InternalStreamState,
  usageRaw: unknown,
  lastPromptRaw?: unknown,
  contextBreakdownRaw?: unknown,
): void {
  const usage = asRecord(usageRaw);
  if (
    typeof usage.promptTokens !== 'number'
    || typeof usage.completionTokens !== 'number'
    || typeof usage.totalTokens !== 'number'
  ) return;
  const lastPrompt = typeof lastPromptRaw === 'number' ? lastPromptRaw : undefined;
  const contextBreakdown = (
    contextBreakdownRaw
    ?? usage.contextBreakdown
  ) as ContextUsageBreakdown | undefined;
  state.lastUsage = {
    ...usage as unknown as UsageTotal,
    lastPromptTokens: lastPrompt ?? (usage.lastPromptTokens as number | undefined),
    contextBreakdown,
  };
}

export function applyStatusEvent(
  state: InternalStreamState,
  text: string,
  tone: 'muted' | 'success' | 'error',
  visibility: 'user' | 'developer' | 'internal' = 'user',
  displayKind: Parameters<typeof appendStatusTraceEvent>[4] = 'status',
): void {
  appendStatusTraceEvent(state, text, tone, visibility, displayKind);
}

function replaceTerminalReplyTrace(state: InternalStreamState, finalReply: string): void {
  if (!finalReply.trim()) return;
  let index = -1;
  for (let candidate = state.traceEvents.length - 1; candidate >= 0; candidate -= 1) {
    if (state.traceEvents[candidate].kind === 'reply') {
      index = candidate;
      break;
    }
  }
  if (index < 0) {
    state.traceEvents = [...state.traceEvents, {
      id: `trace-reply-${Date.now()}-${state._traceSeq++}`,
      kind: 'reply',
      text: finalReply,
    }];
    return;
  }
  const existing = state.traceEvents[index];
  if (existing.kind !== 'reply' || existing.text === finalReply) return;
  const next = [...state.traceEvents];
  next[index] = { ...existing, text: finalReply };
  state.traceEvents = next;
}

export function applyDoneEvent(
  state: InternalStreamState,
  input: {
    status: string | null;
    message: unknown;
    messageTruncated?: unknown;
    usageTotal?: unknown;
    lastPromptTokens?: unknown;
    contextBreakdown?: unknown;
    cached?: unknown;
    finishReason?: unknown;
  },
): void {
  const finalThinking = state.thinkingText;
  const doneText = extractMessageText(input.message);
  const finalReply = input.messageTruncated === true && state.streamText.trim()
    ? state.streamText
    : doneText ?? state.streamText;
  const hasFinalRound = finalThinking.trim() || finalReply.trim();
  if (hasFinalRound) {
    replaceTerminalReplyTrace(state, finalReply);
    const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
    state.streamRounds = [...state.streamRounds, {
      id: roundId,
      thinking: finalThinking || undefined,
      reply: finalReply,
      toolCalls: [],
    }];
    state.streamText = '';
  }
  state.isThinking = false;
  state.thinkingText = '';

  const toolStatus = input.status === 'cancelled'
    ? 'cancelled'
    : input.status === 'timed_out'
      ? 'timedOut'
      : 'done';
  const toolFallback = input.status === 'cancelled'
    ? 'Cancelled'
    : input.status === 'timed_out'
      ? 'Timed out'
      : 'No output';
  state.toolCalls = markToolCallsFinished(state.toolCalls, toolStatus, toolFallback);
  state.streamRounds = markRoundsToolCallsFinished(state.streamRounds, toolStatus, toolFallback);
  syncTraceToolEvents(state);

  applyUsageUpdateEvent(
    state,
    input.usageTotal,
    input.lastPromptTokens,
    input.contextBreakdown,
  );
  state.lastCached = input.cached === true;
  state.finishReason = typeof input.finishReason === 'string' ? input.finishReason : null;
  state.isStreaming = false;
  if (
    state.connectionState?.state === 'reconnecting'
    || state.connectionState?.state === 'degraded'
  ) {
    state.connectionState = null;
  }
  if (input.status === 'cancelled') state.error = null;
  state._activeRoundId = null;
  state._activeRoundAcceptingStarts = false;
  resetActiveStreamBlocks(state);
}

export function applyAutoCompactedEvent(state: InternalStreamState, summary: string): void {
  state.autoCompacted = { summary };
}

export function applyApprovalRequestedEvent(
  state: InternalStreamState,
  request: unknown,
): void {
  const candidate = asRecord(request) as unknown as ApprovalRequest;
  if (typeof candidate.id !== 'string' || typeof candidate.toolName !== 'string') return;
  if (!state.pendingApprovals.some(item => item.id === candidate.id)) {
    state.pendingApprovals = [...state.pendingApprovals, candidate];
  }
}

export function applyApprovalResolvedEvent(
  state: InternalStreamState,
  requestId: string | undefined,
): void {
  if (requestId) {
    state.pendingApprovals = state.pendingApprovals.filter(item => item.id !== requestId);
  }
}

export function applyErrorEvent(
  state: InternalStreamState,
  message: string,
  terminalStatus: string | null,
): void {
  if (terminalStatus === 'cancelled') {
    applyTerminalProjection(state, {
      toolStatus: 'cancelled',
      message,
      toolFallbackMessage: 'Cancelled',
      traceTone: 'error',
      errorMessage: null,
    });
    return;
  }
  if (terminalStatus === 'timed_out') {
    applyTerminalProjection(state, {
      toolStatus: 'timedOut',
      message,
      toolFallbackMessage: 'Timed out',
      traceTone: 'error',
      errorMessage: message,
    });
    return;
  }
  if (/context.*(window|overflow|exceeded)|ContextOverflow/i.test(message)) {
    state.contextOverflow = true;
  }
  if (/rate.?limit/i.test(message)) {
    state.rateLimited = true;
    applyTerminalProjection(state, {
      toolStatus: 'error',
      message: 'Rate limited',
      toolFallbackMessage: 'Interrupted',
      traceTone: 'error',
      errorMessage: 'Rate limited',
    });
    return;
  }

  applyTerminalProjection(state, {
    toolStatus: 'error',
    message,
    toolFallbackMessage: 'Interrupted',
    traceTone: 'error',
    errorMessage: message,
  });
}
