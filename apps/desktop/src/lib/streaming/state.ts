import type { AgentRunEvent } from '../../types/conversation';
import type { StreamState } from './protocol';
import type { StreamTimeoutHandle } from './watchdog';

export interface InternalStreamState extends StreamState {
  _toolCallSeq: number;
  _roundSeq: number;
  _traceSeq: number;
  _orderedRunId: string | null;
  _lastEventSeq: number;
  _pendingRunEvents: Map<number, AgentRunEvent>;
  _activeAnswerBlockId: string | null;
  _activeAnswerOffset: number;
  _activeThinkingBlockId: string | null;
  _activeThinkingOffset: number;
  _pendingAnswerBlockDeltas: Map<string, Map<number, string>>;
  _pendingThinkingBlockDeltas: Map<string, Map<number, string>>;
  _activeRoundId: string | null;
  _activeRoundAcceptingStarts: boolean;
  _timeoutId: StreamTimeoutHandle | null;
  _watchdogGeneration: number;
  _watchdogRecoveryAttempt: number;
  _watchdogMissingRunConfirmations: number;
  _toolPreparingTimers: Record<string, ReturnType<typeof setTimeout>>;
  _launchStartedAt: number | null;
  _frontendPaintScheduled: boolean;
  _frontendPaintReported: boolean;
}

export function createDefaultState(): InternalStreamState {
  return {
    turnHandle: null,
    isStreaming: false,
    streamText: '',
    streamRounds: [],
    traceEvents: [],
    thinkingText: '',
    isThinking: false,
    toolCalls: [],
    error: null,
    lastUsage: null,
    lastCached: false,
    finishReason: null,
    contextOverflow: false,
    rateLimited: false,
    connectionState: null,
    autoCompacted: null,
    pendingApprovals: [],
    taskRun: null,
    taskEvents: [],
    turnTiming: null,
    _toolCallSeq: 0,
    _roundSeq: 0,
    _traceSeq: 0,
    _orderedRunId: null,
    _lastEventSeq: 0,
    _pendingRunEvents: new Map(),
    _activeAnswerBlockId: null,
    _activeAnswerOffset: 0,
    _activeThinkingBlockId: null,
    _activeThinkingOffset: 0,
    _pendingAnswerBlockDeltas: new Map(),
    _pendingThinkingBlockDeltas: new Map(),
    _activeRoundId: null,
    _activeRoundAcceptingStarts: false,
    _timeoutId: null,
    _watchdogGeneration: 0,
    _watchdogRecoveryAttempt: 0,
    _watchdogMissingRunConfirmations: 0,
    _toolPreparingTimers: {},
    _launchStartedAt: null,
    _frontendPaintScheduled: false,
    _frontendPaintReported: false,
  };
}

export function clearToolPreparingTimer(state: InternalStreamState, callId: string): void {
  const timer = state._toolPreparingTimers[callId];
  if (!timer) return;
  clearTimeout(timer);
  delete state._toolPreparingTimers[callId];
}

export function clearToolPreparingTimers(state: InternalStreamState): void {
  Object.values(state._toolPreparingTimers).forEach(timer => clearTimeout(timer));
  state._toolPreparingTimers = {};
}
