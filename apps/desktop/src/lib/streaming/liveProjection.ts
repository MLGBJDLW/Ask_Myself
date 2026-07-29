import type { AgentFrontendEvent } from '../../types';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  ApprovalRequest,
  ContextUsageBreakdown,
} from '../../types/conversation';
import { appendReplyTraceEvent } from './blockProjection';
import type { UsageTotal } from './protocol';
import type { InternalStreamState } from './state';
import {
  appendStatusTraceEvent,
  applyStreamResetProjection,
  applyTerminalProjection,
  markRoundsToolCallsFinished,
  markToolCallsFinished,
  resetActiveStreamBlocks,
  syncTraceToolEvents,
} from './terminalProjection';

type RawFrontendEvent = AgentFrontendEvent & Record<string, unknown>;

function extractMessageText(message: unknown): string | null {
  if (!message || typeof message !== 'object') return null;
  const record = message as Record<string, unknown>;
  if (typeof record.content === 'string' && record.content.trim().length > 0) {
    return record.content;
  }
  if (!Array.isArray(record.parts)) return null;
  const text = record.parts
    .map(part => {
      if (!part || typeof part !== 'object') return '';
      const item = part as Record<string, unknown>;
      return typeof item.text === 'string' ? item.text : '';
    })
    .join('');
  return text.trim().length > 0 ? text : null;
}

function terminalRunStatus(event: AgentFrontendEvent, raw: RawFrontendEvent): string | null {
  const status = event.runEvent?.status ?? raw.status;
  return typeof status === 'string' ? status : null;
}

export function applyUsageUpdateEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const usage = event.usageTotal ?? (raw.usage_total as UsageTotal | undefined);
  if (!usage) return;
  const lastPrompt = (raw.lastPromptTokens ?? raw.last_prompt_tokens) as number | undefined;
  const contextBreakdown = (
    event.contextBreakdown
    ?? raw.contextBreakdown
    ?? raw.context_breakdown
    ?? usage.contextBreakdown
  ) as ContextUsageBreakdown | undefined;
  state.lastUsage = {
    ...usage,
    lastPromptTokens: lastPrompt ?? usage.lastPromptTokens,
    contextBreakdown,
  };
}

export function applyStatusEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const text = (typeof event.content === 'string' ? event.content : '')
    || (typeof raw.content === 'string' ? raw.content : '');
  const tone = event.tone === 'success' || event.tone === 'error'
    ? event.tone
    : (raw.tone === 'success' || raw.tone === 'error' ? raw.tone : 'muted');
  appendStatusTraceEvent(
    state,
    text,
    tone,
    event.runEvent?.visibility ?? 'user',
    event.runEvent?.displayKind ?? 'status',
  );
}

export function applyDoneEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const terminalStatus = terminalRunStatus(event, raw);
  const finalThinking = state.thinkingText;
  const doneMessage = event.message ?? raw.message;
  const doneText = extractMessageText(doneMessage);
  const finalReply = state.streamText.trim().length > 0
    ? state.streamText
    : (doneText ?? '');
  const hasFinalRound = finalThinking.trim() || finalReply.trim();
  if (hasFinalRound) {
    if (!state.streamText.trim() && finalReply.trim()) {
      appendReplyTraceEvent(state, finalReply);
    }
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

  const toolStatus = terminalStatus === 'cancelled'
    ? 'cancelled'
    : terminalStatus === 'timed_out'
      ? 'timedOut'
      : 'done';
  const toolFallback = terminalStatus === 'cancelled'
    ? 'Cancelled'
    : terminalStatus === 'timed_out'
      ? 'Timed out'
      : 'No output';
  state.toolCalls = markToolCallsFinished(state.toolCalls, toolStatus, toolFallback);
  state.streamRounds = markRoundsToolCallsFinished(state.streamRounds, toolStatus, toolFallback);
  syncTraceToolEvents(state);

  applyUsageUpdateEvent(state, event, raw);
  state.lastCached = Boolean(raw.cached ?? false);
  const finishReason = raw.finishReason ?? raw.finish_reason ?? null;
  state.finishReason = typeof finishReason === 'string' ? finishReason : null;
  state.isStreaming = false;
  if (terminalStatus === 'cancelled') state.error = null;
  state._activeRoundId = null;
  state._activeRoundAcceptingStarts = false;
  resetActiveStreamBlocks(state);
}

export function applyAutoCompactedEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const summary = (typeof event.summary === 'string' ? event.summary : '')
    || (typeof raw.summary === 'string' ? raw.summary : '');
  state.autoCompacted = { summary };
}

export function applyStreamResetEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const reason = (typeof event.reason === 'string' ? event.reason : '')
    || (typeof raw.reason === 'string' ? raw.reason : '')
    || 'Stream interrupted; retrying without streaming.';
  applyStreamResetProjection(state, reason, { clearTools: true });
}

export function applyApprovalRequestedEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const req = (event.request ?? raw.request) as ApprovalRequest | undefined;
  if (req && typeof req.id === 'string' && typeof req.toolName === 'string') {
    if (!state.pendingApprovals.some(p => p.id === req.id)) {
      state.pendingApprovals = [...state.pendingApprovals, req];
    }
  }
}

export function applyApprovalResolvedEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const requestId = (typeof event.requestId === 'string' ? event.requestId : undefined)
    ?? (typeof raw.requestId === 'string' ? raw.requestId : undefined);
  if (requestId) {
    state.pendingApprovals = state.pendingApprovals.filter(p => p.id !== requestId);
  }
}

export function applyTaskRunUpdatedEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const taskRun = (event.taskRun ?? raw.taskRun) as AgentTaskRun | undefined;
  if (taskRun && typeof taskRun.id === 'string') {
    state.taskRun = taskRun;
  }
}

export function applyTaskRunEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const taskEvent = (event.taskEvent ?? raw.taskEvent) as AgentTaskRunEvent | undefined;
  if (taskEvent && typeof taskEvent.id === 'string') {
    if (!state.taskEvents.some(existing => existing.id === taskEvent.id)) {
      state.taskEvents = [...state.taskEvents, taskEvent].slice(-50);
    }
  }
}

export function applyErrorEvent(
  state: InternalStreamState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  const errMsg = (typeof event.message === 'string' ? event.message
    : (typeof raw.message === 'string' ? raw.message : 'Unknown error'));
  const terminalStatus = terminalRunStatus(event, raw);
  if (terminalStatus === 'cancelled') {
    applyTerminalProjection(state, {
      toolStatus: 'cancelled',
      message: errMsg,
      toolFallbackMessage: 'Cancelled',
      traceTone: 'error',
      errorMessage: null,
    });
    return;
  }
  if (terminalStatus === 'timed_out') {
    applyTerminalProjection(state, {
      toolStatus: 'timedOut',
      message: errMsg,
      toolFallbackMessage: 'Timed out',
      traceTone: 'error',
      errorMessage: errMsg,
    });
    return;
  }
  if (/context.*(window|overflow|exceeded)|ContextOverflow/i.test(errMsg)) {
    state.contextOverflow = true;
  }
  if (/rate.?limit/i.test(errMsg)) {
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
    message: errMsg,
    toolFallbackMessage: 'Interrupted',
    traceTone: 'error',
    errorMessage: errMsg,
  });
}
