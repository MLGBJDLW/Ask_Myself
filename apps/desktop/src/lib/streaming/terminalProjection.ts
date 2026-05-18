import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
  TraceStatusEvent,
  TraceToolEvent,
} from './protocol';
import {
  isPendingToolCallStatus,
  type TerminalToolStatus,
} from './toolStatus';

export interface StreamTerminalProjectionState {
  isStreaming: boolean;
  streamText: string;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  error: string | null;
  _traceSeq: number;
  _activeAnswerBlockId: string | null;
  _activeAnswerOffset: number;
  _activeThinkingBlockId: string | null;
  _activeThinkingOffset: number;
  _activeRoundId: string | null;
  _activeRoundAcceptingStarts: boolean;
}

export function markToolCallsFinished(
  toolCalls: ToolCallEvent[],
  status: TerminalToolStatus,
  fallbackContent: string,
): ToolCallEvent[] {
  return toolCalls.map(tc =>
    isPendingToolCallStatus(tc.status)
      ? {
          ...tc,
          status,
          argsStatus: status === 'error' || status === 'timedOut' ? 'error' : 'done',
          content: tc.content || fallbackContent,
          isError: status === 'error' || status === 'timedOut',
        }
      : tc,
  );
}

export function markRoundsToolCallsFinished(
  rounds: StreamRoundEvent[],
  status: TerminalToolStatus,
  fallbackContent: string,
): StreamRoundEvent[] {
  return rounds.map(round => ({
    ...round,
    toolCalls: markToolCallsFinished(round.toolCalls, status, fallbackContent),
  }));
}

export function appendStatusTraceEvent(
  state: StreamTerminalProjectionState,
  text: string,
  tone: TraceStatusEvent['tone'] = 'muted',
): void {
  if (!text.trim()) return;
  state.traceEvents = [...state.traceEvents, {
    id: `trace-status-${Date.now()}-${state._traceSeq++}`,
    kind: 'status',
    text,
    tone,
  }];
}

export function syncTraceToolEvents(state: StreamTerminalProjectionState): void {
  state.traceEvents = state.traceEvents.map(event => {
    if (event.kind !== 'tool') return event;
    const latest = state.toolCalls.find(tc => tc.callId === event.toolCall.callId);
    return latest ? { ...event, toolCall: latest } as TraceToolEvent : event;
  });
}

export function resetActiveStreamBlocks(state: StreamTerminalProjectionState): void {
  state._activeAnswerBlockId = null;
  state._activeAnswerOffset = 0;
  state._activeThinkingBlockId = null;
  state._activeThinkingOffset = 0;
}

export function applyStreamResetProjection(
  state: StreamTerminalProjectionState,
  reason: string,
  options: { clearTools?: boolean } = {},
): void {
  state.streamText = '';
  state.streamRounds = [];
  state.thinkingText = '';
  state.isThinking = false;
  if (options.clearTools) state.toolCalls = [];
  state.traceEvents = state.traceEvents.filter(trace => trace.kind === 'status');
  state.error = null;
  state._activeRoundId = null;
  state._activeRoundAcceptingStarts = false;
  resetActiveStreamBlocks(state);
  appendStatusTraceEvent(state, reason, 'muted');
}

export function applyTerminalProjection(
  state: StreamTerminalProjectionState,
  input: {
    toolStatus: TerminalToolStatus;
    message: string;
    toolFallbackMessage?: string;
    traceTone: 'success' | 'error';
    errorMessage?: string | null;
  },
): void {
  state.isStreaming = false;
  state.isThinking = false;
  state.thinkingText = '';
  const toolFallbackMessage = input.toolFallbackMessage ?? input.message;
  state.toolCalls = markToolCallsFinished(state.toolCalls, input.toolStatus, toolFallbackMessage);
  state.streamRounds = markRoundsToolCallsFinished(state.streamRounds, input.toolStatus, toolFallbackMessage);
  syncTraceToolEvents(state);
  if (input.errorMessage !== undefined) state.error = input.errorMessage;
  appendStatusTraceEvent(state, input.message, input.traceTone);
  state._activeRoundId = null;
  state._activeRoundAcceptingStarts = false;
  resetActiveStreamBlocks(state);
}
