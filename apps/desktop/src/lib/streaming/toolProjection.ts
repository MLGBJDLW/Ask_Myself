import type { AgentFrontendEvent } from '../../types';
import type {
  ArtifactPayload,
  ToolRunItem,
} from '../../types/conversation';
import type { ToolCallEvent, TraceToolEvent } from './protocol';
import { clearToolPreparingTimer, type InternalStreamState } from './state';
import {
  resetActiveStreamBlocks,
  syncTraceToolEvents,
  type StreamTerminalProjectionState,
} from './terminalProjection';
import {
  argsStatusForToolRun,
  isPendingToolCallStatus,
  toolRunStatusToToolCallStatus,
} from './toolStatus';

export type StreamToolProjectionState = InternalStreamState;

type RawFrontendEvent = AgentFrontendEvent & Record<string, unknown>;

export interface ToolPreparingPayload {
  callId: string;
  toolName: string;
  argsBytes: number;
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
    if (isPendingToolCallStatus(updated[i].status)) {
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
    if (!isPendingToolCallStatus(nextCall.status)) {
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
    if (!isPendingToolCallStatus(latest.status)) {
      state._activeRoundAcceptingStarts = false;
    }
  }
}

export function extractToolPreparingPayload(
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): ToolPreparingPayload | null {
  const callId = (
    (typeof event.callId === 'string' && event.callId)
    || (typeof raw.call_id === 'string' ? raw.call_id : '')
  ).trim();
  if (!callId) return null;
  const toolNameRaw = (typeof event.toolName === 'string' && event.toolName)
    || (typeof raw.tool_name === 'string' ? raw.tool_name : '');
  const toolName = toolNameRaw.trim() ? toolNameRaw : 'unknown_tool';
  const argsBytesRaw = event.argsBytes ?? raw.args_bytes ?? raw.argsBytes ?? 0;
  const argsBytes = typeof argsBytesRaw === 'number'
    ? argsBytesRaw
    : Number.parseInt(String(argsBytesRaw), 10);
  return {
    callId,
    toolName,
    argsBytes: Number.isFinite(argsBytes) ? argsBytes : 0,
  };
}

export function extractToolRunPayload(
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): ToolRunItem | null {
  const runRaw = event.run ?? raw.run;
  if (!runRaw || typeof runRaw !== 'object') return null;
  const run = runRaw as ToolRunItem;
  const callId = (run.callId || '').trim();
  if (!callId) return null;
  return run;
}

export function toolPreparingPayloadFromRun(run: ToolRunItem): ToolPreparingPayload | null {
  const callId = (run.callId || '').trim();
  if (!callId || run.status !== 'preparing') return null;
  return {
    callId,
    toolName: (run.toolName || '').trim() || 'unknown_tool',
    argsBytes: typeof run.arguments === 'string' ? run.arguments.length : 0,
  };
}

function patchStartedToolCall(
  prev: ToolCallEvent,
  toolName: string,
  argumentsText: string,
): ToolCallEvent {
  const mergedArgs = argumentsText || prev.arguments;
  return {
    ...prev,
    toolName,
    arguments: mergedArgs,
    argsBytes: Math.max(prev.argsBytes, mergedArgs.length),
    status: prev.status === 'preparing' ? 'starting' : prev.status,
    argsStatus: isPendingToolCallStatus(prev.status) ? 'ready' : prev.argsStatus,
  };
}

export function applyToolCallStartEvent(
  state: StreamToolProjectionState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  try {
    state.isThinking = false;

    const roundThinking = state.thinkingText.trim() ? state.thinkingText : '';
    if (roundThinking) state.thinkingText = '';

    const incomingCallId = (
      (typeof event.callId === 'string' && event.callId)
      || (typeof raw.call_id === 'string' ? raw.call_id : '')
    ).trim();
    const callId = incomingCallId || `tool-call-${Date.now()}-${state._toolCallSeq++}`;
    if (incomingCallId) clearToolPreparingTimer(state, incomingCallId);
    const toolNameRaw = (typeof event.toolName === 'string' && event.toolName)
      || (typeof raw.tool_name === 'string' ? raw.tool_name : '');
    const toolName = toolNameRaw.trim() ? toolNameRaw : 'unknown_tool';
    const argsRaw = event.arguments ?? raw.arguments;
    const argumentsText = typeof argsRaw === 'string'
      ? argsRaw
      : (argsRaw == null ? '' : String(argsRaw));

    const nextCall: ToolCallEvent = createToolCall({
      callId,
      toolName,
      arguments: argumentsText,
      status: 'starting',
      argsStatus: 'ready',
    });

    if (state.streamText.trim().length > 0) {
      const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
      state._activeRoundId = roundId;
      state._activeRoundAcceptingStarts = true;
      state.streamRounds = [...state.streamRounds, {
        id: roundId,
        thinking: roundThinking || undefined,
        reply: state.streamText,
        toolCalls: [nextCall],
      }];
      state.streamText = '';
    } else if (state._activeRoundId && state._activeRoundAcceptingStarts) {
      const mergeRoundId = state._activeRoundId;
      const targetRound = state.streamRounds.find(round => round.id === mergeRoundId);
      if (targetRound) {
        state.streamRounds = state.streamRounds.map(round => {
          if (round.id !== mergeRoundId) return round;
          const existingIdx = round.toolCalls.findIndex(tc => tc.callId === nextCall.callId);
          const mergedThinking = roundThinking ? ((round.thinking || '') + roundThinking) : round.thinking;
          if (existingIdx >= 0) {
            const nextToolCalls = [...round.toolCalls];
            nextToolCalls[existingIdx] = patchStartedToolCall(
              nextToolCalls[existingIdx],
              toolName,
              argumentsText,
            );
            return { ...round, thinking: mergedThinking, toolCalls: nextToolCalls };
          }
          return { ...round, thinking: mergedThinking, toolCalls: [...round.toolCalls, nextCall] };
        });
      } else {
        console.error('[streamStore] merge target round not found, creating new round');
        const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
        state._activeRoundId = roundId;
        state._activeRoundAcceptingStarts = true;
        state.streamRounds = [...state.streamRounds, {
          id: roundId,
          thinking: roundThinking || undefined,
          reply: '',
          toolCalls: [nextCall],
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
        toolCalls: [nextCall],
      }];
    }

    const existing = state.toolCalls.findIndex(tc => tc.callId === callId);
    if (existing >= 0) {
      state.toolCalls = [...state.toolCalls];
      state.toolCalls[existing] = patchStartedToolCall(state.toolCalls[existing], toolName, argumentsText);
      upsertToolTraceEvent(state, state.toolCalls[existing]);
    } else {
      state.toolCalls = [...state.toolCalls, nextCall];
      upsertToolTraceEvent(state, nextCall);
    }
    resetActiveStreamBlocks(state);
  } catch (err) {
    console.error('[streamStore] toolCallStart error, creating fallback round:', err);
    const fallbackCallId = `tool-call-${Date.now()}-${state._toolCallSeq++}`;
    const fallbackCall: ToolCallEvent = createToolCall({
      callId: fallbackCallId,
      toolName: 'unknown_tool',
      status: 'starting',
      argsStatus: 'ready',
    });
    const roundId = `stream-round-${Date.now()}-${state._roundSeq++}`;
    state._activeRoundId = roundId;
    state._activeRoundAcceptingStarts = false;
    state.streamRounds = [...state.streamRounds, {
      id: roundId, reply: '', toolCalls: [fallbackCall],
    }];
    state.toolCalls = [...state.toolCalls, fallbackCall];
    upsertToolTraceEvent(state, fallbackCall);
    state.isThinking = false;
    state.thinkingText = '';
    resetActiveStreamBlocks(state);
  }
}

export function applyToolCallArgsDeltaEvent(
  state: StreamToolProjectionState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  try {
    const callId = (
      (typeof event.callId === 'string' && event.callId)
      || (typeof raw.call_id === 'string' ? raw.call_id : '')
    ).trim();
    if (!callId) return;
    const deltaRaw = event.argumentsDelta
      ?? (raw.arguments_delta as string | undefined)
      ?? (raw.argumentsDelta as string | undefined)
      ?? '';
    const delta = typeof deltaRaw === 'string' ? deltaRaw : String(deltaRaw ?? '');
    if (!delta) return;
    const patchCall = (tc: ToolCallEvent): ToolCallEvent => {
      const nextArgs = tc.arguments + delta;
      return {
        ...tc,
        arguments: nextArgs,
        argsBytes: nextArgs.length,
        argsStatus: isPendingToolCallStatus(tc.status) ? 'streaming' : tc.argsStatus,
      };
    };

    let foundInFlat = false;
    state.toolCalls = state.toolCalls.map(tc => {
      if (tc.callId !== callId) return tc;
      foundInFlat = true;
      return patchCall(tc);
    });

    if (!foundInFlat) return;

    state.streamRounds = state.streamRounds.map(round => {
      const idx = round.toolCalls.findIndex(tc => tc.callId === callId);
      if (idx < 0) return round;
      const nextCalls = [...round.toolCalls];
      nextCalls[idx] = patchCall(nextCalls[idx]);
      return { ...round, toolCalls: nextCalls };
    });
    const latest = state.toolCalls.find(tc => tc.callId === callId);
    if (latest) upsertToolTraceEvent(state, latest);
  } catch (err) {
    console.error('[streamStore] toolCallArgsDelta error:', err);
  }
}

export function applyToolCallProgressEvent(
  state: StreamToolProjectionState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  try {
    const callId = (
      (typeof event.callId === 'string' && event.callId)
      || (typeof raw.call_id === 'string' ? raw.call_id : '')
    ).trim();
    if (!callId) return;
    const noteRaw = event.note
      ?? (raw.note as string | undefined)
      ?? '';
    const note = typeof noteRaw === 'string' ? noteRaw.trim() : '';
    if (!note) return;

    const patchCall = (tc: ToolCallEvent): ToolCallEvent => {
      const nextStatus: ToolCallEvent['status'] =
        isPendingToolCallStatus(tc.status)
          ? 'running'
          : tc.status;
      const nextArgsStatus: ToolCallEvent['argsStatus'] =
        tc.argsStatus === 'streaming' || tc.argsStatus === 'pending'
          ? 'ready'
          : tc.argsStatus;
      return { ...tc, status: nextStatus, argsStatus: nextArgsStatus };
    };

    let matched = false;
    state.toolCalls = state.toolCalls.map(tc => {
      if (tc.callId !== callId) return tc;
      matched = true;
      return patchCall(tc);
    });
    if (!matched) return;

    state.streamRounds = state.streamRounds.map(round => {
      const idx = round.toolCalls.findIndex(tc => tc.callId === callId);
      if (idx < 0) return round;
      const nextCalls = [...round.toolCalls];
      nextCalls[idx] = patchCall(nextCalls[idx]);
      return { ...round, toolCalls: nextCalls };
    });
    const latest = state.toolCalls.find(tc => tc.callId === callId);
    if (latest) upsertToolTraceEvent(state, latest);
  } catch (err) {
    console.error('[streamStore] toolCallProgress error:', err);
  }
}

export function applyToolCallResultEvent(
  state: StreamToolProjectionState,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
): void {
  try {
    const resultCallId = (typeof event.callId === 'string' && event.callId)
      || (typeof raw.call_id === 'string' ? raw.call_id : '') || '';
    if (resultCallId) clearToolPreparingTimer(state, resultCallId);
    const resultIsError = (typeof event.isError === 'boolean' ? event.isError : undefined)
      ?? (typeof raw.is_error === 'boolean' ? raw.is_error : undefined);
    const resultContent = (typeof event.content === 'string' ? event.content : undefined)
      ?? (typeof raw.content === 'string' ? raw.content : undefined);
    const resultArtifacts = (event.artifacts && typeof event.artifacts === 'object')
      ? event.artifacts
      : ((raw.artifacts && typeof raw.artifacts === 'object') ? raw.artifacts as ArtifactPayload : undefined);

    const { next: nextToolCalls } = resolveToolCallResult(
      state.toolCalls,
      resultCallId,
      resultIsError,
      resultContent,
      resultArtifacts,
    );
    state.toolCalls = nextToolCalls;
    syncTraceToolEvents(state);

    const roundsCopy = [...state.streamRounds];
    for (let i = roundsCopy.length - 1; i >= 0; i -= 1) {
      const round = roundsCopy[i];
      const resolved = resolveToolCallResult(
        round.toolCalls,
        resultCallId,
        resultIsError,
        resultContent,
        resultArtifacts,
      );
      if (resolved.matched) {
        roundsCopy[i] = { ...round, toolCalls: resolved.next };
        state.streamRounds = roundsCopy;
        break;
      }
    }
    state._activeRoundAcceptingStarts = false;
  } catch (err) {
    console.error('[streamStore] toolCallResult error:', err);
  }
}
