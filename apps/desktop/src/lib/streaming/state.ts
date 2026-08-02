import type { AgentTaskRun } from '../../types/conversation';
import type { StreamState } from './protocol';
import type { StreamTimeoutHandle } from './watchdog';

export interface InternalStreamState extends StreamState {
  _toolCallSeq: number;
  _roundSeq: number;
  _traceSeq: number;
  _lastEventSeq: number;
  _eventSeqGapRecorded: boolean;
  _activeAnswerBlockId: string | null;
  _activeAnswerOffset: number;
  _activeThinkingBlockId: string | null;
  _activeThinkingOffset: number;
  _activeRoundId: string | null;
  _activeRoundAcceptingStarts: boolean;
  _timeoutId: StreamTimeoutHandle | null;
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
    autoCompacted: null,
    pendingApprovals: [],
    taskRun: null,
    taskEvents: [],
    _toolCallSeq: 0,
    _roundSeq: 0,
    _traceSeq: 0,
    _lastEventSeq: 0,
    _eventSeqGapRecorded: false,
    _activeAnswerBlockId: null,
    _activeAnswerOffset: 0,
    _activeThinkingBlockId: null,
    _activeThinkingOffset: 0,
    _activeRoundId: null,
    _activeRoundAcceptingStarts: false,
    _timeoutId: null,
    _toolPreparingTimers: {},
    _launchStartedAt: null,
    _frontendPaintScheduled: false,
    _frontendPaintReported: false,
  };
}

export function taskRunIsActive(taskRun: AgentTaskRun): boolean {
  return ['queued', 'running', 'waiting_approval', 'cancelling'].includes(taskRun.status);
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
