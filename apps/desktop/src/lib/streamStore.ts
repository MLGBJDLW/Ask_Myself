/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent, ApprovalRequest } from '../types';
import type { ArtifactPayload } from '../types/conversation';

/* ── Exported types ─────────────────────────────────────────────── */

export interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'starting' | 'running' | 'done' | 'error';
  /** Assembly progress of `arguments` during mid-stream streaming. */
  argsStatus: 'streaming' | 'ready' | 'done' | 'error';
  /** Number of characters received for `arguments` so far. */
  argsBytes: number;
  /** Latest up to 10 heartbeat notes accumulated during tool execution. */
  progressNotes: string[];
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
}

const PROGRESS_NOTES_MAX = 10;

function createToolCall(partial: {
  callId: string;
  toolName: string;
  arguments?: string;
  status?: ToolCallEvent['status'];
  argsStatus?: ToolCallEvent['argsStatus'];
}): ToolCallEvent {
  const argumentsText = partial.arguments ?? '';
  return {
    callId: partial.callId,
    toolName: partial.toolName,
    arguments: argumentsText,
    status: partial.status ?? 'starting',
    argsStatus: partial.argsStatus ?? 'streaming',
    argsBytes: argumentsText.length,
    progressNotes: [],
  };
}

export interface StreamRoundEvent {
  id: string;
  thinking?: string;
  reply: string;
  toolCalls: ToolCallEvent[];
}

export interface TraceThinkingEvent {
  id: string;
  kind: 'thinking';
  text: string;
}

export interface TraceReplyEvent {
  id: string;
  kind: 'reply';
  text: string;
}

export interface TraceToolEvent {
  id: string;
  kind: 'tool';
  toolCall: ToolCallEvent;
}

export interface TraceStatusEvent {
  id: string;
  kind: 'status';
  text: string;
  tone?: 'muted' | 'success' | 'error';
}

export type TraceEvent = TraceThinkingEvent | TraceReplyEvent | TraceToolEvent | TraceStatusEvent;

export interface UsageTotal {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  thinkingTokens?: number;
  lastPromptTokens?: number;
}

export interface StreamState {
  isStreaming: boolean;
  streamText: string;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  error: string | null;
  lastUsage: UsageTotal | null;
  lastCached: boolean;
  finishReason: string | null;
  contextOverflow: boolean;
  rateLimited: boolean;
  autoCompacted: { summary: string } | null;
  /** High-risk tool calls awaiting GUI approval. FIFO queue. */
  pendingApprovals: ApprovalRequest[];
}

/* ── Internal types ─────────────────────────────────────────────── */

interface InternalStreamState extends StreamState {
  _toolCallSeq: number;
  _roundSeq: number;
  _traceSeq: number;
  _activeRoundId: string | null;
  _activeRoundAcceptingStarts: boolean;
  _timeoutId: ReturnType<typeof setTimeout> | null;
}

/* ── Constants ──────────────────────────────────────────────────── */

function resolveStreamTimeoutMs(): number {
  if (typeof window === 'undefined') return 30_000;
  const override = (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: unknown }).__ASK_STREAM_TIMEOUT_MS__;
  return typeof override === 'number' && Number.isFinite(override) && override > 0
    ? override
    : 30_000;
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
    case 'toolCallStart': return 'toolCallStart';
    case 'toolCallArgsDelta': return 'toolCallArgsDelta';
    case 'toolCallProgress': return 'toolCallProgress';
    case 'toolCallResult': return 'toolCallResult';
    case 'status': return 'status';
    case 'done': return 'done';
    case 'error': return 'error';
    case 'autoCompacted': return 'autoCompacted';
    case 'usageUpdate': return 'usageUpdate';
    case 'approvalRequested': return 'approvalRequested';
    case 'approvalResolved': return 'approvalResolved';
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
  return status === 'running' || status === 'starting';
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
    _toolCallSeq: 0,
    _roundSeq: 0,
    _traceSeq: 0,
    _activeRoundId: null,
    _activeRoundAcceptingStarts: false,
    _timeoutId: null,
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
    };
  }

  /** Find the conversation ID of any currently active stream. */
  getActiveStreamId(): string | null {
    for (const [id, state] of Object.entries(this._streams)) {
      if (state.isStreaming) return id;
    }
    return null;
  }

  /** Initialize (or reset) stream state for a conversation. */
  startStream(conversationId: string): void {
    const existing = this._streams[conversationId];
    if (existing?._timeoutId) clearTimeout(existing._timeoutId);

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
    s._activeRoundId = null;
    s._activeRoundAcceptingStarts = false;
    this.notify(conversationId);
  }

  /** Mark stream as stopped by user. */
  stopStream(conversationId: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    if (s._timeoutId) clearTimeout(s._timeoutId);
    s.isThinking = false;
    s.thinkingText = '';
    s.toolCalls = markToolCallsFinished(s.toolCalls, 'error', 'Stopped by user');
    s.streamRounds = markRoundsToolCallsFinished(s.streamRounds, 'error', 'Stopped by user');
    syncTraceToolEvents(s);
    appendStatusTraceEvent(s, 'Stopped by user', 'error');
    s.isStreaming = false;
    s._activeRoundId = null;
    s._activeRoundAcceptingStarts = false;
    s._timeoutId = null;
    this.notify(conversationId);
  }

  /** Handle a send() failure (api.agentChat threw). */
  sendError(conversationId: string, errorMessage: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    if (s._timeoutId) clearTimeout(s._timeoutId);
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
      state._timeoutId = null;
      this.notify(conversationId);
    }, STREAM_TIMEOUT_MS);
  }

  /** Process an incoming agent event. */
  dispatch(conversationId: string, event: AgentFrontendEvent): void {
    const s = this._streams[conversationId];
    if (!s || !s.isStreaming) return;

    const raw = event as AgentFrontendEvent & Record<string, unknown>;
    const eventType = normalizeAgentEventType(raw.type);
    if (!eventType) return;

    // Reset inactivity timeout on every event, including empty keepalive
    // `thinking` events emitted while the backend is still working.
    this.resetTimeout(conversationId);

    switch (eventType) {
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
          argsStatus: 'streaming',
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
                  status: isPendingStatus(prev.status) ? prev.status : 'starting',
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
            status: isPendingStatus(prev.status) ? prev.status : 'starting',
          };
          upsertToolTraceEvent(s, s.toolCalls[existing]);
        } else {
          s.toolCalls = [...s.toolCalls, nextCall];
          upsertToolTraceEvent(s, nextCall);
        }
        } catch (err) {
          console.error('[streamStore] toolCallStart error, creating fallback round:', err);
          // Fallback: create a simple new round with the tool call
          const fallbackCallId = `tool-call-${Date.now()}-${s._toolCallSeq++}`;
          const fallbackCall: ToolCallEvent = createToolCall({
            callId: fallbackCallId,
            toolName: 'unknown_tool',
            status: 'starting',
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
          const toolNameRaw = (typeof event.toolName === 'string' && event.toolName)
            || (typeof raw.tool_name === 'string' ? raw.tool_name : '')
            || 'unknown_tool';

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
            // Defensive: args delta arrived before toolCallStart. Create a stub.
            const stub = createToolCall({
              callId,
              toolName: toolNameRaw,
              arguments: delta,
              status: 'starting',
              argsStatus: 'streaming',
            });
            s.toolCalls = [...s.toolCalls, stub];

            // Also place it on the active round (or a new one).
            const mergeRoundId = s._activeRoundId;
            const targetRound = mergeRoundId ? s.streamRounds.find(r => r.id === mergeRoundId) : null;
            if (targetRound && s._activeRoundAcceptingStarts) {
              s.streamRounds = s.streamRounds.map(round =>
                round.id === mergeRoundId
                  ? { ...round, toolCalls: [...round.toolCalls, stub] }
                  : round,
              );
            } else {
              const roundId = `stream-round-${Date.now()}-${s._roundSeq++}`;
              s._activeRoundId = roundId;
              s._activeRoundAcceptingStarts = true;
              s.streamRounds = [...s.streamRounds, {
                id: roundId, reply: '', toolCalls: [stub],
              }];
            }
            upsertToolTraceEvent(s, stub);
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
              tc.status === 'starting' ? 'running' : tc.status;
            const nextArgsStatus: ToolCallEvent['argsStatus'] =
              tc.argsStatus === 'streaming' ? 'ready' : tc.argsStatus;
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

      case 'error': {
        if (s._timeoutId) clearTimeout(s._timeoutId);
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
        s._timeoutId = null;
        break;
      }
    }

    this.scheduleNotify(conversationId);
  }
}

export const streamStore = new StreamStoreImpl();
