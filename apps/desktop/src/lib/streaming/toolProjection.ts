import type {
  ArtifactPayload,
  ToolRunItem,
  ToolRunStatus,
} from '../../types/conversation';
import type { ToolCallEvent, TraceToolEvent } from './protocol';
import { isPendingStatus, resetActiveStreamBlocks, type StreamTerminalProjectionState } from './terminalProjection';

export const PROGRESS_NOTES_MAX = 10;

export interface StreamToolProjectionState extends StreamTerminalProjectionState {
  _roundSeq: number;
}

export function createToolCall(partial: {
  callId: string;
  toolName: string;
  arguments?: string;
  status?: ToolCallEvent['status'];
  argsStatus?: ToolCallEvent['argsStatus'];
  renderKind?: ToolCallEvent['renderKind'];
  capabilities?: ToolCallEvent['capabilities'];
  plugin?: ToolCallEvent['plugin'];
}): ToolCallEvent {
  const argumentsText = partial.arguments ?? '';
  return {
    callId: partial.callId,
    toolName: partial.toolName,
    plugin: partial.plugin,
    arguments: argumentsText,
    status: partial.status ?? 'starting',
    renderKind: partial.renderKind,
    capabilities: partial.capabilities,
    argsStatus: partial.argsStatus ?? 'ready',
    argsBytes: argumentsText.length,
    progressNotes: [],
  };
}

function finalizeToolCall(
  tc: ToolCallEvent,
  isError: boolean | undefined,
  content: string | undefined,
  artifacts: ArtifactPayload | undefined,
): ToolCallEvent {
  return {
    ...tc,
    status: isError ? 'error' : 'done',
    argsStatus: isError ? 'error' : 'done',
    content,
    isError,
    artifacts,
  };
}

function toolRunStatusToToolCallStatus(status: ToolRunStatus): ToolCallEvent['status'] {
  switch (status) {
    case 'preparing':
      return 'preparing';
    case 'approvalPending':
      return 'approvalPending';
    case 'running':
      return 'running';
    case 'completed':
      return 'done';
    case 'failed':
      return 'error';
    case 'declined':
      return 'declined';
    case 'cancelled':
      return 'cancelled';
    case 'timedOut':
      return 'timedOut';
    default:
      return 'running';
  }
}

function argsStatusForToolRun(run: ToolRunItem, status: ToolCallEvent['status']): ToolCallEvent['argsStatus'] {
  if (status === 'preparing') return run.arguments ? 'streaming' : 'pending';
  if (status === 'error' || status === 'timedOut') return 'error';
  if (status === 'done' || status === 'declined' || status === 'cancelled') return 'done';
  return run.arguments ? 'ready' : 'pending';
}

function appendProgressNote(notes: string[], note: string | undefined): string[] {
  const trimmed = (note ?? '').trim();
  if (!trimmed) return notes;
  const next = notes.length >= PROGRESS_NOTES_MAX
    ? [...notes.slice(-(PROGRESS_NOTES_MAX - 1)), trimmed]
    : [...notes, trimmed];
  return next;
}

function patchToolCallFromRun(prev: ToolCallEvent, run: ToolRunItem): ToolCallEvent {
  const status = toolRunStatusToToolCallStatus(run.status);
  const argumentsText = run.arguments ?? prev.arguments;
  return {
    ...prev,
    toolName: run.toolName || prev.toolName,
    arguments: argumentsText,
    status,
    renderKind: run.renderKind ?? prev.renderKind,
    capabilities: run.capabilities ?? prev.capabilities,
    plugin: run.plugin ?? prev.plugin,
    argsStatus: argsStatusForToolRun(run, status),
    argsBytes: Math.max(prev.argsBytes, argumentsText.length),
    progressNotes: appendProgressNote(prev.progressNotes, run.progressNote),
    content: run.content ?? prev.content,
    isError: run.isError ?? prev.isError,
    artifacts: run.artifacts ?? prev.artifacts,
    durationMs: run.durationMs ?? prev.durationMs,
  };
}

export function resolveToolCallResult(
  prev: ToolCallEvent[],
  resultCallId: string,
  resultIsError: boolean | undefined,
  resultContent: string | undefined,
  resultArtifacts: ArtifactPayload | undefined,
): { next: ToolCallEvent[]; matched: boolean } {
  let matched = false;
  const updated = prev.map(tc => {
    if (tc.callId === resultCallId) {
      matched = true;
      return finalizeToolCall(tc, resultIsError, resultContent, resultArtifacts);
    }
    return tc;
  });

  if (matched) return { next: updated, matched: true };

  let fallbackIndex = -1;
  for (let i = updated.length - 1; i >= 0; i -= 1) {
    if (isPendingStatus(updated[i].status)) {
      fallbackIndex = i;
      break;
    }
  }
  if (fallbackIndex >= 0) {
    const copy = [...updated];
    copy[fallbackIndex] = finalizeToolCall(
      copy[fallbackIndex], resultIsError, resultContent, resultArtifacts,
    );
    return { next: copy, matched: true };
  }

  return { next: updated, matched: false };
}

export function upsertToolTraceEvent(state: StreamTerminalProjectionState, toolCall: ToolCallEvent): void {
  const idx = state.traceEvents.findIndex(event =>
    event.kind === 'tool' && event.toolCall.callId === toolCall.callId);
  if (idx >= 0) {
    const next = [...state.traceEvents];
    next[idx] = { ...next[idx], toolCall } as TraceToolEvent;
    state.traceEvents = next;
    return;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-tool-${Date.now()}-${state._traceSeq++}`,
    kind: 'tool',
    toolCall,
  }];
}

export function insertPendingToolCall(
  state: StreamToolProjectionState,
  toolCall: ToolCallEvent,
  roundThinking: string,
): void {
  if (state.streamText.trim().length > 0) {
    const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
    state._activeRoundId = roundId;
    state._activeRoundAcceptingStarts = true;
    state.streamRounds = [...state.streamRounds, {
      id: roundId,
      thinking: roundThinking || undefined,
      reply: state.streamText,
      toolCalls: [toolCall],
    }];
    state.streamText = '';
  } else if (state._activeRoundId && state._activeRoundAcceptingStarts) {
    const mergeRoundId = state._activeRoundId;
    const targetRound = state.streamRounds.find(r => r.id === mergeRoundId);
    if (targetRound) {
      state.streamRounds = state.streamRounds.map(round =>
        round.id === mergeRoundId
          ? {
              ...round,
              thinking: roundThinking ? ((round.thinking || '') + roundThinking) : round.thinking,
              toolCalls: [...round.toolCalls, toolCall],
            }
          : round,
      );
    } else {
      const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
      state._activeRoundId = roundId;
      state._activeRoundAcceptingStarts = true;
      state.streamRounds = [...state.streamRounds, {
        id: roundId,
        thinking: roundThinking || undefined,
        reply: '',
        toolCalls: [toolCall],
      }];
    }
  } else {
    const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
    state._activeRoundId = roundId;
    state._activeRoundAcceptingStarts = true;
    state.streamRounds = [...state.streamRounds, {
      id: roundId,
      thinking: roundThinking || undefined,
      reply: '',
      toolCalls: [toolCall],
    }];
  }

  state.toolCalls = [...state.toolCalls, toolCall];
  upsertToolTraceEvent(state, toolCall);
}

export function applyToolRunEvent(state: StreamToolProjectionState, run: ToolRunItem): void {
  const callId = (run.callId || '').trim();
  if (!callId) return;
  const toolName = (run.toolName || '').trim() || 'unknown_tool';

  const existingIdx = state.toolCalls.findIndex(tc => tc.callId === callId);
  if (existingIdx < 0) {
    const roundThinking = state.thinkingText.trim() ? state.thinkingText : '';
    if (roundThinking) state.thinkingText = '';
    state.isThinking = false;

    const status = toolRunStatusToToolCallStatus(run.status);
    const base = createToolCall({
      callId,
      toolName,
      arguments: run.arguments ?? '',
      status,
      argsStatus: argsStatusForToolRun(run, status),
      renderKind: run.renderKind,
      capabilities: run.capabilities,
      plugin: run.plugin,
    });
    const nextCall = patchToolCallFromRun(base, run);
    insertPendingToolCall(state, nextCall, roundThinking);
    resetActiveStreamBlocks(state);
    if (!isPendingStatus(nextCall.status)) {
      state._activeRoundAcceptingStarts = false;
    }
    return;
  }

  state.toolCalls = state.toolCalls.map(tc => {
    if (tc.callId !== callId) return tc;
    return patchToolCallFromRun(tc, run);
  });

  state.streamRounds = state.streamRounds.map(round => {
    const idx = round.toolCalls.findIndex(tc => tc.callId === callId);
    if (idx < 0) return round;
    const nextCalls = [...round.toolCalls];
    nextCalls[idx] = patchToolCallFromRun(nextCalls[idx], run);
    return { ...round, toolCalls: nextCalls };
  });

  const latest = state.toolCalls.find(tc => tc.callId === callId);
  if (latest) {
    upsertToolTraceEvent(state, latest);
    if (!isPendingStatus(latest.status)) {
      state._activeRoundAcceptingStarts = false;
    }
  }
}
