/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  ApprovalRequest,
  ArtifactPayload,
  ToolRunItem,
  ToolRunStatus,
} from '../types/conversation';
import {
  adaptFrontendRunEvent,
  isDurableStreamEvent,
  replayItemFromTaskEvent,
} from './streaming/legacyAdapter';
import type {
  StreamRoundEvent,
  StreamState,
  ToolCallEvent,
  TraceReplyEvent,
  TraceStatusEvent,
  TraceThinkingEvent,
  TraceToolEvent,
  UsageTotal,
} from './streaming/protocol';
export type { StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const PROGRESS_NOTES_MAX = 10;
const TOOL_PREPARING_DELAY_MS = 150;

function createToolCall(partial: {
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

/* ── Internal types ─────────────────────────────────────────────── */

interface InternalStreamState extends StreamState {
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
  _timeoutId: ReturnType<typeof setTimeout> | null;
  _toolPreparingTimers: Record<string, ReturnType<typeof setTimeout>>;
}

/* ── Constants ──────────────────────────────────────────────────── */

function resolveStreamTimeoutMs(): number {
  if (typeof window === 'undefined') return 120_000;
  const override = (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: unknown }).__ASK_STREAM_TIMEOUT_MS__;
  return typeof override === 'number' && Number.isFinite(override) && override > 0
    ? override
    : 120_000;
}

const STREAM_TIMEOUT_MS = resolveStreamTimeoutMs();

/* ── Helper functions ───────────────────────────────────────────── */

type AgentEventType = AgentFrontendEvent['type'];

function normalizeAgentEventType(value: unknown): AgentEventType | null {
  if (typeof value !== 'string') return null;
  const raw = value.trim();
  if (!raw) return null;

  const lowered = raw
    .replace(/[_\s-]+([a-zA-Z0-9])/g, (_m, ch: string) => ch.toUpperCase())
    .replace(/^([A-Z])/, (_m, ch: string) => ch.toLowerCase());

  switch (lowered) {
    case 'thinking': return 'thinking';
    case 'textDelta': return 'textDelta';
    case 'streamBlockDelta': return 'streamBlockDelta';
    case 'streamReset': return 'streamReset';
    case 'toolCallPreparing': return 'toolCallPreparing';
    case 'toolCallStart': return 'toolCallStart';
    case 'toolCallArgsDelta': return 'toolCallArgsDelta';
    case 'toolCallProgress': return 'toolCallProgress';
    case 'toolCallResult': return 'toolCallResult';
    case 'toolRunStarted': return 'toolRunStarted';
    case 'toolRunUpdated': return 'toolRunUpdated';
    case 'toolRunCompleted': return 'toolRunCompleted';
    case 'status': return 'status';
    case 'done': return 'done';
    case 'error': return 'error';
    case 'autoCompacted': return 'autoCompacted';
    case 'usageUpdate': return 'usageUpdate';
    case 'approvalRequested': return 'approvalRequested';
    case 'approvalResolved': return 'approvalResolved';
    case 'taskRunUpdated': return 'taskRunUpdated';
    case 'taskRunEvent': return 'taskRunEvent';
    default: return null;
  }
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

function isPendingStatus(status: ToolCallEvent['status']): boolean {
  return status === 'running'
    || status === 'starting'
    || status === 'preparing'
    || status === 'approvalPending';
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

function resolveToolCallResult(
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

function createDefaultState(): InternalStreamState {
  return {
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
  };
}

function markToolCallsFinished(
  toolCalls: ToolCallEvent[],
  status: 'done' | 'error',
  fallbackContent: string,
): ToolCallEvent[] {
  return toolCalls.map(tc =>
    isPendingStatus(tc.status)
      ? {
          ...tc,
          status,
          argsStatus: status === 'error' ? 'error' : 'done',
          content: tc.content || fallbackContent,
          isError: status === 'error',
        }
      : tc,
  );
}

function markRoundsToolCallsFinished(
  rounds: StreamRoundEvent[],
  status: 'done' | 'error',
  fallbackContent: string,
): StreamRoundEvent[] {
  return rounds.map(round => ({
    ...round,
    toolCalls: markToolCallsFinished(round.toolCalls, status, fallbackContent),
  }));
}

function appendStatusTraceEvent(
  state: InternalStreamState,
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

function appendThinkingTraceEvent(state: InternalStreamState, delta: string): void {
  if (!delta) return;
  const last = state.traceEvents[state.traceEvents.length - 1];
  if (last?.kind === 'thinking') {
    state.traceEvents = [
      ...state.traceEvents.slice(0, -1),
      { ...last, text: last.text + delta },
    ];
    return;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-thinking-${Date.now()}-${state._traceSeq++}`,
    kind: 'thinking',
    text: delta,
  }];
}

function appendReplyTraceEvent(state: InternalStreamState, delta: string): void {
  if (!delta) return;
  const last = state.traceEvents[state.traceEvents.length - 1];
  if (last?.kind === 'reply') {
    state.traceEvents = [
      ...state.traceEvents.slice(0, -1),
      { ...last, text: last.text + delta },
    ];
    return;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-reply-${Date.now()}-${state._traceSeq++}`,
    kind: 'reply',
    text: delta,
  }];
}

function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

function upsertThinkingBlockTraceEvent(
  state: InternalStreamState,
  blockId: string,
  offset: number,
  delta: string,
): boolean {
  if (!delta) return false;
  const deltaBytes = utf8ByteLength(delta);
  const idx = state.traceEvents.findIndex(
    event => event.kind === 'thinking' && event.blockId === blockId,
  );
  if (idx >= 0) {
    const prev = state.traceEvents[idx] as TraceThinkingEvent;
    const nextOffset = prev.nextOffset ?? 0;
    if (offset < nextOffset) return false;
    const next = [...state.traceEvents];
    next[idx] = {
      ...prev,
      text: prev.text + delta,
      nextOffset: offset + deltaBytes,
    };
    state.traceEvents = next;
    return true;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-thinking-${Date.now()}-${state._traceSeq++}`,
    kind: 'thinking',
    text: delta,
    blockId,
    nextOffset: offset + deltaBytes,
  }];
  return true;
}

function upsertReplyBlockTraceEvent(
  state: InternalStreamState,
  blockId: string,
  offset: number,
  delta: string,
): boolean {
  if (!delta) return false;
  const deltaBytes = utf8ByteLength(delta);
  const idx = state.traceEvents.findIndex(
    event => event.kind === 'reply' && event.blockId === blockId,
  );
  if (idx >= 0) {
    const prev = state.traceEvents[idx] as TraceReplyEvent;
    const nextOffset = prev.nextOffset ?? 0;
    if (offset < nextOffset) return false;
    const next = [...state.traceEvents];
    next[idx] = {
      ...prev,
      text: prev.text + delta,
      nextOffset: offset + deltaBytes,
    };
    state.traceEvents = next;
    return true;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-reply-${Date.now()}-${state._traceSeq++}`,
    kind: 'reply',
    text: delta,
    blockId,
    nextOffset: offset + deltaBytes,
  }];
  return true;
}

function upsertToolTraceEvent(state: InternalStreamState, toolCall: ToolCallEvent): void {
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

function syncTraceToolEvents(state: InternalStreamState): void {
  state.traceEvents = state.traceEvents.map(event => {
    if (event.kind !== 'tool') return event;
    const latest = state.toolCalls.find(tc => tc.callId === event.toolCall.callId);
    return latest ? { ...event, toolCall: latest } : event;
  });
}

function clearToolPreparingTimer(state: InternalStreamState, callId: string): void {
  const timer = state._toolPreparingTimers[callId];
  if (!timer) return;
  clearTimeout(timer);
  delete state._toolPreparingTimers[callId];
}

function clearToolPreparingTimers(state: InternalStreamState): void {
  Object.values(state._toolPreparingTimers).forEach(timer => clearTimeout(timer));
  state._toolPreparingTimers = {};
}

function resetActiveStreamBlocks(state: InternalStreamState): void {
  state._activeAnswerBlockId = null;
  state._activeAnswerOffset = 0;
  state._activeThinkingBlockId = null;
  state._activeThinkingOffset = 0;
}

function applyStreamBlockDelta(
  state: InternalStreamState,
  channel: 'answer' | 'thinking',
  blockId: string,
  offset: number,
  delta: string,
): void {
  if (!blockId || !delta) return;
  const normalizedOffset = Number.isFinite(offset) && offset >= 0 ? offset : 0;
  const deltaBytes = utf8ByteLength(delta);

  if (channel === 'answer') {
    state.isThinking = false;
    if (state._activeRoundId) {
      state._activeRoundId = null;
      state._activeRoundAcceptingStarts = false;
    }
    if (state._activeAnswerBlockId !== blockId) {
      state._activeAnswerBlockId = blockId;
      state._activeAnswerOffset = 0;
      state.streamText = '';
    }
    if (normalizedOffset < state._activeAnswerOffset) return;
    state.thinkingText = '';
    state.streamText += delta;
    state._activeAnswerOffset = normalizedOffset + deltaBytes;
    upsertReplyBlockTraceEvent(state, blockId, normalizedOffset, delta);
    return;
  }

  state.isThinking = true;
  if (state._activeThinkingBlockId !== blockId) {
    state._activeThinkingBlockId = blockId;
    state._activeThinkingOffset = 0;
    state.thinkingText = '';
  }
  if (normalizedOffset < state._activeThinkingOffset) return;
  state.thinkingText += delta;
  state._activeThinkingOffset = normalizedOffset + deltaBytes;
  upsertThinkingBlockTraceEvent(state, blockId, normalizedOffset, delta);
}

function taskRunIsActive(taskRun: AgentTaskRun): boolean {
  return ['queued', 'running', 'waiting_approval'].includes(taskRun.status);
}

function insertPendingToolCall(
  state: InternalStreamState,
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

function applyToolRunEvent(state: InternalStreamState, run: ToolRunItem): void {
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

/* ── Store implementation ───────────────────────────────────────── */

type StoreListener = (conversationId: string) => void;

class StreamStoreImpl {
  private _streams: Record<string, InternalStreamState> = {};
  private _listeners = new Set<StoreListener>();
  private _pendingNotify = new Set<string>();
  private _notifyScheduled = false;

  subscribe = (listener: StoreListener): (() => void) => {
    this._listeners.add(listener);
    return () => { this._listeners.delete(listener); };
  };

  private notify(conversationId: string): void {
    for (const listener of this._listeners) {
      listener(conversationId);
    }
  }

  private scheduleNotify(conversationId: string): void {
    this._pendingNotify.add(conversationId);
    if (!this._notifyScheduled) {
      this._notifyScheduled = true;
      queueMicrotask(() => {
        this._notifyScheduled = false;
        const pending = new Set(this._pendingNotify);
        this._pendingNotify.clear();
        for (const id of pending) {
          for (const listener of this._listeners) {
            listener(id);
          }
        }
      });
    }
  }

  getStream(id: string): StreamState | undefined {
    const s = this._streams[id];
    if (!s) return undefined;
    return {
      isStreaming: s.isStreaming,
      streamText: s.streamText,
      streamRounds: s.streamRounds,
      traceEvents: s.traceEvents,
      thinkingText: s.thinkingText,
      isThinking: s.isThinking,
      toolCalls: s.toolCalls,
      error: s.error,
      lastUsage: s.lastUsage,
      lastCached: s.lastCached,
      finishReason: s.finishReason,
      contextOverflow: s.contextOverflow,
      rateLimited: s.rateLimited,
      autoCompacted: s.autoCompacted,
      pendingApprovals: s.pendingApprovals,
      taskRun: s.taskRun,
      taskEvents: s.taskEvents,
    };
  }

  /** Find the conversation ID of any currently active stream. */
  getActiveStreamId(): string | null {
    for (const [id, state] of Object.entries(this._streams)) {
      if (state.isStreaming) return id;
    }
    return null;
  }

  /** Rebuild the visible stream preview from durable typed stream events. */
  restoreFromTaskEvents(
    conversationId: string,
    taskRun: AgentTaskRun,
    taskEvents: AgentTaskRunEvent[],
  ): void {
    const existing = this._streams[conversationId];
    if (existing?.isStreaming && (
      existing.traceEvents.length > 0 ||
      existing.streamText.length > 0 ||
      existing.streamRounds.length > 0
    )) {
      return;
    }
    if (existing?._timeoutId) clearTimeout(existing._timeoutId);
    if (existing) clearToolPreparingTimers(existing);

    const state = createDefaultState();
    state.isStreaming = taskRunIsActive(taskRun);
    state.taskRun = taskRun;
    state.taskEvents = taskEvents
      .filter(event => !isDurableStreamEvent(event))
      .slice(-50);

    const replayEvents = taskEvents
      .map(replayItemFromTaskEvent)
      .filter((item): item is NonNullable<ReturnType<typeof replayItemFromTaskEvent>> => Boolean(item))
      .sort((a, b) => a.eventSeq - b.eventSeq);

    for (const item of replayEvents) {
      if (item.eventSeq <= state._lastEventSeq) continue;
      state._lastEventSeq = item.eventSeq;
      if (item.eventType === 'status') {
        const reason = typeof item.payload.reason === 'string'
          ? item.payload.reason
          : 'Stream recovery update.';
        appendStatusTraceEvent(state, reason, 'muted');
        continue;
      }
      if (item.eventType === 'streamReset') {
        const reason = typeof item.payload.reason === 'string'
          ? item.payload.reason
          : 'Stream restarted.';
        state.streamText = '';
        state.streamRounds = [];
        state.thinkingText = '';
        state.isThinking = false;
        state.traceEvents = state.traceEvents.filter(trace => trace.kind === 'status');
        resetActiveStreamBlocks(state);
        appendStatusTraceEvent(state, reason, 'muted');
        continue;
      }
      if (item.eventType !== 'streamBlockDelta') continue;
      const channel = item.payload.channel === 'answer' || item.payload.channel === 'thinking'
        ? item.payload.channel
        : null;
      const blockId = typeof item.payload.blockId === 'string' ? item.payload.blockId : '';
      const delta = typeof item.payload.delta === 'string' ? item.payload.delta : '';
      const offsetRaw = item.payload.offset;
      const offset = typeof offsetRaw === 'number'
        ? offsetRaw
        : Number.parseInt(String(offsetRaw ?? '0'), 10);
      if (channel && blockId && delta) {
        applyStreamBlockDelta(
          state,
          channel,
          blockId,
          Number.isFinite(offset) ? offset : 0,
          delta,
        );
      }
    }

    this._streams[conversationId] = state;
    if (state.isStreaming) {
      this.resetTimeout(conversationId);
    }
    this.notify(conversationId);
  }

  /** Initialize (or reset) stream state for a conversation. */
  startStream(conversationId: string): void {
    const existing = this._streams[conversationId];
    if (existing) {
      if (existing._timeoutId) clearTimeout(existing._timeoutId);
      clearToolPreparingTimers(existing);
    }

    const state = createDefaultState();
    state.isStreaming = true;
    this._streams[conversationId] = state;
    this.resetTimeout(conversationId);
    this.notify(conversationId);
  }

  /** Remove stream state entirely. */
  clearStream(conversationId: string): void {
    const existing = this._streams[conversationId];
    if (!existing) return;
    if (existing._timeoutId) clearTimeout(existing._timeoutId);
    clearToolPreparingTimers(existing);
    delete this._streams[conversationId];
    this.notify(conversationId);
  }

  /** Clear preview/display data but keep metadata. */
  clearPreview(conversationId: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    s.streamText = '';
    s.streamRounds = [];
    s.traceEvents = [];
    s.thinkingText = '';
    s.isThinking = false;
    s.toolCalls = [];
    clearToolPreparingTimers(s);
    s._activeRoundId = null;
    s._activeRoundAcceptingStarts = false;
    resetActiveStreamBlocks(s);
    this.notify(conversationId);
  }

  /** Mark stream as stopped by user. */
  stopStream(conversationId: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    if (s._timeoutId) clearTimeout(s._timeoutId);
    clearToolPreparingTimers(s);
    s.isThinking = false;
    s.thinkingText = '';
    s.toolCalls = markToolCallsFinished(s.toolCalls, 'error', 'Stopped by user');
    s.streamRounds = markRoundsToolCallsFinished(s.streamRounds, 'error', 'Stopped by user');
    syncTraceToolEvents(s);
    appendStatusTraceEvent(s, 'Stopped by user', 'error');
    s.isStreaming = false;
    s._activeRoundId = null;
    s._activeRoundAcceptingStarts = false;
    resetActiveStreamBlocks(s);
    s._timeoutId = null;
    this.notify(conversationId);
  }

  /** Handle a send() failure (api.agentChat threw). */
  sendError(conversationId: string, errorMessage: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    if (s._timeoutId) clearTimeout(s._timeoutId);
    clearToolPreparingTimers(s);
    s.isThinking = false;
    s.thinkingText = '';
    s.toolCalls = markToolCallsFinished(s.toolCalls, 'error', 'Request failed');
    s.streamRounds = markRoundsToolCallsFinished(s.streamRounds, 'error', 'Request failed');
    syncTraceToolEvents(s);
    appendStatusTraceEvent(s, errorMessage || 'Request failed', 'error');
    s.error = errorMessage;
    s.isStreaming = false;
    s._activeRoundId = null;
    s._activeRoundAcceptingStarts = false;
    resetActiveStreamBlocks(s);
    s._timeoutId = null;
    this.notify(conversationId);
  }

  private resetTimeout(conversationId: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    if (s._timeoutId) clearTimeout(s._timeoutId);
    s._timeoutId = setTimeout(() => {
      const state = this._streams[conversationId];
      if (!state) return;
      clearToolPreparingTimers(state);
      state.toolCalls = markToolCallsFinished(state.toolCalls, 'error', 'Connection lost');
      state.streamRounds = markRoundsToolCallsFinished(state.streamRounds, 'error', 'Connection lost');
      syncTraceToolEvents(state);
      appendStatusTraceEvent(state, 'Connection lost', 'error');
      state.thinkingText = '';
      state.isThinking = false;
      state.error = 'Connection lost';
      state.isStreaming = false;
      state._activeRoundId = null;
      state._activeRoundAcceptingStarts = false;
      resetActiveStreamBlocks(state);
      state._timeoutId = null;
      this.notify(conversationId);
    }, STREAM_TIMEOUT_MS);
  }

  private scheduleToolPreparing(
    conversationId: string,
    callId: string,
    toolName: string,
    argsBytes: number,
  ): void {
    const s = this._streams[conversationId];
    if (!s || s.toolCalls.some(tc => tc.callId === callId) || s._toolPreparingTimers[callId]) {
      return;
    }

    s._toolPreparingTimers[callId] = setTimeout(() => {
      const state = this._streams[conversationId];
      if (!state) return;
      delete state._toolPreparingTimers[callId];
      if (!state.isStreaming || state.toolCalls.some(tc => tc.callId === callId)) return;

      const roundThinking = state.thinkingText.trim() ? state.thinkingText : '';
      if (roundThinking) state.thinkingText = '';
      state.isThinking = false;

      const preparingCall = createToolCall({
        callId,
        toolName,
        status: 'preparing',
        argsStatus: 'pending',
      });
      preparingCall.argsBytes = Math.max(0, argsBytes);
      insertPendingToolCall(state, preparingCall, roundThinking);
      this.scheduleNotify(conversationId);
    }, TOOL_PREPARING_DELAY_MS);
  }

  /** Process an incoming agent event. */
  dispatch(conversationId: string, event: AgentFrontendEvent): void {
    event = adaptFrontendRunEvent(event);
    const raw = event as AgentFrontendEvent & Record<string, unknown>;
    const eventType = normalizeAgentEventType(raw.type);
    if (!eventType) return;
    const isTaskLifecycleEvent = eventType === 'taskRunUpdated' || eventType === 'taskRunEvent';

    let s = this._streams[conversationId];
    if (!s) {
      if (!event.runEvent && !isTaskLifecycleEvent && eventType !== 'done' && eventType !== 'error') {
        return;
      }
      s = createDefaultState();
      s.isStreaming = eventType !== 'done' && eventType !== 'error';
      this._streams[conversationId] = s;
    }
    if (!s.isStreaming && !isTaskLifecycleEvent) return;

    const eventSeqRaw = event.eventSeq ?? raw.eventSeq;
    const eventSeq = typeof eventSeqRaw === 'number'
      ? eventSeqRaw
      : Number.parseInt(String(eventSeqRaw ?? ''), 10);
    if (Number.isFinite(eventSeq) && eventSeq > 0) {
      if (eventSeq <= s._lastEventSeq) return;
      if (s._lastEventSeq > 0 && eventSeq > s._lastEventSeq + 1 && !s._eventSeqGapRecorded) {
        s._eventSeqGapRecorded = true;
        appendStatusTraceEvent(s, 'Stream event gap detected; replay may be required.', 'muted');
      }
      s._lastEventSeq = eventSeq;
    }

    // Reset inactivity timeout on every event, including empty keepalive
    // `thinking` events emitted while the backend is still working.
    if (s.isStreaming) {
      this.resetTimeout(conversationId);
    }

    switch (eventType) {
      case 'streamBlockDelta': {
        const blockId = (typeof event.blockId === 'string' ? event.blockId : '')
          || (typeof raw.blockId === 'string' ? raw.blockId : '');
        const rawChannel = event.channel ?? raw.channel;
        const channel = rawChannel === 'answer' || rawChannel === 'thinking'
          ? rawChannel
          : null;
        const offsetRaw = event.offset ?? raw.offset;
        const offset = typeof offsetRaw === 'number'
          ? offsetRaw
          : Number.parseInt(String(offsetRaw ?? '0'), 10);
        const delta = typeof event.delta === 'string'
          ? event.delta
          : (typeof raw.delta === 'string' ? raw.delta : '');
        if (channel && blockId && delta) {
          applyStreamBlockDelta(
            s,
            channel,
            blockId,
            Number.isFinite(offset) ? offset : 0,
            delta,
          );
        }
        break;
      }

      case 'thinking': {
        try {
        const delta = typeof event.content === 'string'
          ? event.content
          : (typeof raw.content === 'string' ? raw.content : '');
        if (!delta) break;
        s.isThinking = true;
        s.thinkingText += delta;
        appendThinkingTraceEvent(s, delta);
        } catch (err) {
          console.error('[streamStore] thinking error:', err);
        }
        break;
      }

      case 'textDelta': {
        s.isThinking = false;
        if (s._activeRoundId) {
          s._activeRoundId = null;
          s._activeRoundAcceptingStarts = false;
        }
        const delta = typeof event.delta === 'string'
          ? event.delta
          : (typeof raw.delta === 'string' ? raw.delta : '');
        if (!delta) break;
        s.thinkingText = '';
        s.streamText += delta;
        appendReplyTraceEvent(s, delta);
        break;
      }

      case 'streamReset': {
        const reason = (typeof event.reason === 'string' ? event.reason : '')
          || (typeof raw.reason === 'string' ? raw.reason : '')
          || 'Stream interrupted; retrying without streaming.';
        s.streamText = '';
        s.streamRounds = [];
        s.thinkingText = '';
        s.isThinking = false;
        s.toolCalls = [];
        clearToolPreparingTimers(s);
        s.traceEvents = s.traceEvents.filter(trace => trace.kind === 'status');
        s.error = null;
        s._activeRoundId = null;
        s._activeRoundAcceptingStarts = false;
        resetActiveStreamBlocks(s);
        appendStatusTraceEvent(s, reason, 'muted');
        break;
      }

      case 'toolCallPreparing': {
        try {
          const callId = (
            (typeof event.callId === 'string' && event.callId)
            || (typeof raw.call_id === 'string' ? raw.call_id : '')
          ).trim();
          if (!callId) break;
          const toolNameRaw = (typeof event.toolName === 'string' && event.toolName)
            || (typeof raw.tool_name === 'string' ? raw.tool_name : '');
          const toolName = toolNameRaw.trim() ? toolNameRaw : 'unknown_tool';
          const argsBytesRaw = event.argsBytes ?? raw.args_bytes ?? raw.argsBytes ?? 0;
          const argsBytes = typeof argsBytesRaw === 'number'
            ? argsBytesRaw
            : Number.parseInt(String(argsBytesRaw), 10);
          this.scheduleToolPreparing(
            conversationId,
            callId,
            toolName,
            Number.isFinite(argsBytes) ? argsBytes : 0,
          );
        } catch (err) {
          console.error('[streamStore] toolCallPreparing error:', err);
        }
        break;
      }

      case 'toolRunStarted':
      case 'toolRunUpdated':
      case 'toolRunCompleted': {
        try {
          const runRaw = event.run ?? raw.run;
          if (!runRaw || typeof runRaw !== 'object') break;
          const run = runRaw as ToolRunItem;
          const callId = (run.callId || '').trim();
          if (!callId) break;

          if (run.status === 'preparing') {
            this.scheduleToolPreparing(
              conversationId,
              callId,
              (run.toolName || '').trim() || 'unknown_tool',
              typeof run.arguments === 'string' ? run.arguments.length : 0,
            );
            break;
          }

          clearToolPreparingTimer(s, callId);
          applyToolRunEvent(s, run);
        } catch (err) {
          console.error('[streamStore] toolRun event error:', err);
        }
        break;
      }

      case 'toolCallStart': {
        try {
        s.isThinking = false;

        // Capture and reset thinking segment
        const roundThinking = s.thinkingText.trim() ? s.thinkingText : '';
        if (roundThinking) s.thinkingText = '';

        const incomingCallId = (
          (typeof event.callId === 'string' && event.callId)
          || (typeof raw.call_id === 'string' ? raw.call_id : '')
        ).trim();
        const callId = incomingCallId || `tool-call-${Date.now()}-${s._toolCallSeq++}`;
        if (incomingCallId) clearToolPreparingTimer(s, incomingCallId);
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

        // If there's accumulated text, start a new round with it
        if (s.streamText.trim().length > 0) {
          const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
          s._activeRoundId = roundId;
          s._activeRoundAcceptingStarts = true;
          s.streamRounds = [...s.streamRounds, {
            id: roundId,
            thinking: roundThinking || undefined,
            reply: s.streamText,
            toolCalls: [nextCall],
          }];
          s.streamText = '';
        } else if (s._activeRoundId && s._activeRoundAcceptingStarts) {
          // Merge into existing active round — verify target exists
          const mergeRoundId = s._activeRoundId;
          const targetRound = s.streamRounds.find(r => r.id === mergeRoundId);
          if (targetRound) {
            s.streamRounds = s.streamRounds.map(round => {
              if (round.id !== mergeRoundId) return round;
              const existingIdx = round.toolCalls.findIndex(tc => tc.callId === nextCall.callId);
              const mergedThinking = roundThinking ? ((round.thinking || '') + roundThinking) : round.thinking;
              if (existingIdx >= 0) {
                const nextToolCalls = [...round.toolCalls];
                const prev = nextToolCalls[existingIdx];
                const mergedArgs = argumentsText || prev.arguments;
                nextToolCalls[existingIdx] = {
                  ...prev,
                  toolName,
                  arguments: mergedArgs,
                  argsBytes: Math.max(prev.argsBytes, mergedArgs.length),
                  status: prev.status === 'preparing' ? 'starting' : prev.status,
                  argsStatus: isPendingStatus(prev.status) ? 'ready' : prev.argsStatus,
                };
                return { ...round, thinking: mergedThinking, toolCalls: nextToolCalls };
              }
              return { ...round, thinking: mergedThinking, toolCalls: [...round.toolCalls, nextCall] };
            });
          } else {
            // Merge target missing — fall back to new round
            console.error('[streamStore] merge target round not found, creating new round');
            const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
            s._activeRoundId = roundId;
            s._activeRoundAcceptingStarts = true;
            s.streamRounds = [...s.streamRounds, {
              id: roundId,
              thinking: roundThinking || undefined,
              reply: '',
              toolCalls: [nextCall],
            }];
          }
        } else {
          const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
          s._activeRoundId = roundId;
          s._activeRoundAcceptingStarts = true;
          s.streamRounds = [...s.streamRounds, {
            id: roundId,
            thinking: roundThinking || undefined,
            reply: '',
            toolCalls: [nextCall],
          }];
        }

        // Update flat toolCalls list
        const existing = s.toolCalls.findIndex(tc => tc.callId === callId);
        if (existing >= 0) {
          s.toolCalls = [...s.toolCalls];
          const prev = s.toolCalls[existing];
          const mergedArgs = argumentsText || prev.arguments;
          s.toolCalls[existing] = {
            ...prev,
            toolName,
            arguments: mergedArgs,
            argsBytes: Math.max(prev.argsBytes, mergedArgs.length),
            status: prev.status === 'preparing' ? 'starting' : prev.status,
            argsStatus: isPendingStatus(prev.status) ? 'ready' : prev.argsStatus,
          };
          upsertToolTraceEvent(s, s.toolCalls[existing]);
        } else {
          s.toolCalls = [...s.toolCalls, nextCall];
          upsertToolTraceEvent(s, nextCall);
        }
        resetActiveStreamBlocks(s);
        } catch (err) {
          console.error('[streamStore] toolCallStart error, creating fallback round:', err);
          // Fallback: create a simple new round with the tool call
          const fallbackCallId = `tool-call-${Date.now()}-${s._toolCallSeq++}`;
          const fallbackCall: ToolCallEvent = createToolCall({
            callId: fallbackCallId,
            toolName: 'unknown_tool',
            status: 'starting',
            argsStatus: 'ready',
          });
          const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
          s._activeRoundId = roundId;
          s._activeRoundAcceptingStarts = false;
          s.streamRounds = [...s.streamRounds, {
            id: roundId, reply: '', toolCalls: [fallbackCall],
          }];
          s.toolCalls = [...s.toolCalls, fallbackCall];
          upsertToolTraceEvent(s, fallbackCall);
          s.isThinking = false;
          s.thinkingText = '';
          resetActiveStreamBlocks(s);
        }
        break;
      }

      case 'toolCallArgsDelta': {
        try {
          const callId = (
            (typeof event.callId === 'string' && event.callId)
            || (typeof raw.call_id === 'string' ? raw.call_id : '')
          ).trim();
          if (!callId) break;
          const deltaRaw = event.argumentsDelta
            ?? (raw.arguments_delta as string | undefined)
            ?? (raw.argumentsDelta as string | undefined)
            ?? '';
          const delta = typeof deltaRaw === 'string' ? deltaRaw : String(deltaRaw ?? '');
          if (!delta) break;
          const patchCall = (tc: ToolCallEvent): ToolCallEvent => {
            const nextArgs = tc.arguments + delta;
            return {
              ...tc,
              arguments: nextArgs,
              argsBytes: nextArgs.length,
              argsStatus: isPendingStatus(tc.status) ? 'streaming' : tc.argsStatus,
            };
          };

          let foundInFlat = false;
          s.toolCalls = s.toolCalls.map(tc => {
            if (tc.callId !== callId) return tc;
            foundInFlat = true;
            return patchCall(tc);
          });

          if (!foundInFlat) {
            // Legacy partial-argument deltas are only accepted after a stable
            // tool row exists. Avoid creating UI cards from incomplete JSON.
            break;
          } else {
            s.streamRounds = s.streamRounds.map(round => {
              const idx = round.toolCalls.findIndex(tc => tc.callId === callId);
              if (idx < 0) return round;
              const nextCalls = [...round.toolCalls];
              nextCalls[idx] = patchCall(nextCalls[idx]);
              return { ...round, toolCalls: nextCalls };
            });
            const latest = s.toolCalls.find(tc => tc.callId === callId);
            if (latest) upsertToolTraceEvent(s, latest);
          }
        } catch (err) {
          console.error('[streamStore] toolCallArgsDelta error:', err);
        }
        break;
      }

      case 'toolCallProgress': {
        try {
          const callId = (
            (typeof event.callId === 'string' && event.callId)
            || (typeof raw.call_id === 'string' ? raw.call_id : '')
          ).trim();
          if (!callId) break;
          const noteRaw = event.note
            ?? (raw.note as string | undefined)
            ?? '';
          const note = typeof noteRaw === 'string' ? noteRaw.trim() : '';
          if (!note) break;

          const patchCall = (tc: ToolCallEvent): ToolCallEvent => {
            const nextNotes = tc.progressNotes.length >= PROGRESS_NOTES_MAX
              ? [...tc.progressNotes.slice(-(PROGRESS_NOTES_MAX - 1)), note]
              : [...tc.progressNotes, note];
            const nextStatus: ToolCallEvent['status'] =
              tc.status === 'starting' || tc.status === 'preparing' || tc.status === 'approvalPending'
                ? 'running'
                : tc.status;
            const nextArgsStatus: ToolCallEvent['argsStatus'] =
              tc.argsStatus === 'streaming' || tc.argsStatus === 'pending'
                ? 'ready'
                : tc.argsStatus;
            return { ...tc, progressNotes: nextNotes, status: nextStatus, argsStatus: nextArgsStatus };
          };

          let matched = false;
          s.toolCalls = s.toolCalls.map(tc => {
            if (tc.callId !== callId) return tc;
            matched = true;
            return patchCall(tc);
          });
          if (!matched) break;

          s.streamRounds = s.streamRounds.map(round => {
            const idx = round.toolCalls.findIndex(tc => tc.callId === callId);
            if (idx < 0) return round;
            const nextCalls = [...round.toolCalls];
            nextCalls[idx] = patchCall(nextCalls[idx]);
            return { ...round, toolCalls: nextCalls };
          });
          const latest = s.toolCalls.find(tc => tc.callId === callId);
          if (latest) upsertToolTraceEvent(s, latest);
        } catch (err) {
          console.error('[streamStore] toolCallProgress error:', err);
        }
        break;
      }

      case 'toolCallResult': {
        try {
        const resultCallId = (typeof event.callId === 'string' && event.callId)
          || (typeof raw.call_id === 'string' ? raw.call_id : '') || '';
        if (resultCallId) clearToolPreparingTimer(s, resultCallId);
        const resultIsError = (typeof event.isError === 'boolean' ? event.isError : undefined)
          ?? (typeof raw.is_error === 'boolean' ? raw.is_error : undefined);
        const resultContent = (typeof event.content === 'string' ? event.content : undefined)
          ?? (typeof raw.content === 'string' ? raw.content : undefined);
        const resultArtifacts = (event.artifacts && typeof event.artifacts === 'object')
          ? event.artifacts
          : ((raw.artifacts && typeof raw.artifacts === 'object') ? raw.artifacts as ArtifactPayload : undefined);

        const { next: nextToolCalls } = resolveToolCallResult(
          s.toolCalls, resultCallId, resultIsError, resultContent, resultArtifacts,
        );
        s.toolCalls = nextToolCalls;
        syncTraceToolEvents(s);

        // Update rounds
        const roundsCopy = [...s.streamRounds];
        for (let i = roundsCopy.length - 1; i >= 0; i -= 1) {
          const round = roundsCopy[i];
          const resolved = resolveToolCallResult(
            round.toolCalls, resultCallId, resultIsError, resultContent, resultArtifacts,
          );
          if (resolved.matched) {
            roundsCopy[i] = { ...round, toolCalls: resolved.next };
            s.streamRounds = roundsCopy;
            break;
          }
        }
        s._activeRoundAcceptingStarts = false;
        } catch (err) {
          console.error('[streamStore] toolCallResult error:', err);
        }
        break;
      }

      case 'usageUpdate': {
        const uUsage = event.usageTotal ?? (raw.usage_total as UsageTotal | undefined);
        if (uUsage) {
          const uLpt = (raw.lastPromptTokens ?? raw.last_prompt_tokens) as number | undefined;
          s.lastUsage = { ...uUsage, lastPromptTokens: uLpt ?? uUsage.lastPromptTokens };
        }
        break;
      }

      case 'status': {
        const text = (typeof event.content === 'string' ? event.content : '')
          || (typeof raw.content === 'string' ? raw.content : '');
        const tone = event.tone === 'success' || event.tone === 'error'
          ? event.tone
          : (raw.tone === 'success' || raw.tone === 'error' ? raw.tone : 'muted');
        appendStatusTraceEvent(s, text, tone);
        break;
      }

      case 'done': {
        if (s._timeoutId) clearTimeout(s._timeoutId);
        clearToolPreparingTimers(s);

        // Capture final round
        const finalThinking = s.thinkingText;
        const finalReply = s.streamText;
        const hasFinalRound = finalThinking.trim() || finalReply.trim();
        if (hasFinalRound) {
          const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
          s.streamRounds = [...s.streamRounds, {
            id: roundId,
            thinking: finalThinking || undefined,
            reply: finalReply,
            toolCalls: [],
          }];
          s.streamText = '';
        }
        s.isThinking = false;
        s.thinkingText = '';

        if (!hasFinalRound) {
          const doneMessage = event.message ?? raw.message;
          const doneText = extractMessageText(doneMessage);
          if (doneText) {
            s.streamText = doneText;
            appendReplyTraceEvent(s, doneText);
          }
        }

        s.toolCalls = markToolCallsFinished(s.toolCalls, 'done', 'No output');
        s.streamRounds = markRoundsToolCallsFinished(s.streamRounds, 'done', 'No output');
        syncTraceToolEvents(s);

        const usage = event.usageTotal ?? (raw.usage_total as UsageTotal | undefined);
        if (usage) {
          const lastPrompt = (raw.lastPromptTokens ?? raw.last_prompt_tokens) as number | undefined;
          s.lastUsage = { ...usage, lastPromptTokens: lastPrompt ?? usage.lastPromptTokens };
        }
        s.lastCached = Boolean(raw.cached ?? false);
        const fr = raw.finishReason ?? raw.finish_reason ?? null;
        s.finishReason = typeof fr === 'string' ? fr : null;
        s.isStreaming = false;
        s._activeRoundId = null;
        s._activeRoundAcceptingStarts = false;
        resetActiveStreamBlocks(s);
        s._timeoutId = null;
        break;
      }

      case 'autoCompacted': {
        const summary = (typeof event.summary === 'string' ? event.summary : '')
          || (typeof raw.summary === 'string' ? raw.summary : '');
        s.autoCompacted = { summary };
        break;
      }

      case 'approvalRequested': {
        const req = (event.request ?? raw.request) as ApprovalRequest | undefined;
        if (req && typeof req.id === 'string' && typeof req.toolName === 'string') {
          if (!s.pendingApprovals.some(p => p.id === req.id)) {
            s.pendingApprovals = [...s.pendingApprovals, req];
          }
        }
        break;
      }

      case 'approvalResolved': {
        const requestId = (typeof event.requestId === 'string' ? event.requestId : undefined)
          ?? (typeof raw.requestId === 'string' ? raw.requestId : undefined);
        if (requestId) {
          s.pendingApprovals = s.pendingApprovals.filter(p => p.id !== requestId);
        }
        break;
      }

      case 'taskRunUpdated': {
        const taskRun = (event.taskRun ?? raw.taskRun) as AgentTaskRun | undefined;
        if (taskRun && typeof taskRun.id === 'string') {
          s.taskRun = taskRun;
        }
        break;
      }

      case 'taskRunEvent': {
        const taskEvent = (event.taskEvent ?? raw.taskEvent) as AgentTaskRunEvent | undefined;
        if (taskEvent && typeof taskEvent.id === 'string') {
          if (!s.taskEvents.some(existing => existing.id === taskEvent.id)) {
            s.taskEvents = [...s.taskEvents, taskEvent].slice(-50);
          }
        }
        break;
      }

      case 'error': {
        if (s._timeoutId) clearTimeout(s._timeoutId);
        clearToolPreparingTimers(s);
        s.isThinking = false;
        s.thinkingText = '';
        s.toolCalls = markToolCallsFinished(s.toolCalls, 'error', 'Interrupted');
        s.streamRounds = markRoundsToolCallsFinished(s.streamRounds, 'error', 'Interrupted');
        syncTraceToolEvents(s);

        const errMsg = (typeof event.message === 'string' ? event.message
          : (typeof raw.message === 'string' ? raw.message : 'Unknown error'));
        if (/context.*(window|overflow|exceeded)|ContextOverflow/i.test(errMsg)) {
          s.contextOverflow = true;
        }
        if (/rate.?limit/i.test(errMsg)) {
          s.rateLimited = true;
          s.error = 'Rate limited';
          appendStatusTraceEvent(s, 'Rate limited', 'error');
        } else {
          s.error = errMsg;
          appendStatusTraceEvent(s, errMsg, 'error');
        }
        s.isStreaming = false;
        s._activeRoundId = null;
        s._activeRoundAcceptingStarts = false;
        resetActiveStreamBlocks(s);
        s._timeoutId = null;
        break;
      }
    }

    this.scheduleNotify(conversationId);
  }
}

export const streamStore = new StreamStoreImpl();
