/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import type { ArtifactPayload } from '../types/conversation';

/* ── Exported types ─────────────────────────────────────────────── */

export interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
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

export type TraceEvent = TraceThinkingEvent | TraceToolEvent | TraceStatusEvent;

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
    case 'toolCallResult': return 'toolCallResult';
    case 'status': return 'status';
    case 'done': return 'done';
    case 'error': return 'error';
    case 'autoCompacted': return 'autoCompacted';
    case 'usageUpdate': return 'usageUpdate';
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
    content,
    isError,
    artifacts,
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
    if (updated[i].status === 'running') {
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
    tc.status === 'running'
      ? { ...tc, status, content: tc.content || fallbackContent, isError: status === 'error' }
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
        s.streamText += delta;
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

        const nextCall: ToolCallEvent = {
          callId, toolName, arguments: argumentsText, status: 'running',
        };

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
                nextToolCalls[existingIdx] = {
                  ...nextToolCalls[existingIdx],
                  toolName, arguments: argumentsText, status: 'running',
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
          const lastRound = s.streamRounds.length > 0 ? s.streamRounds[s.streamRounds.length - 1] : null;
          if (lastRound && !lastRound.reply.trim()) {
            s._activeRoundId = lastRound.id;
            s._activeRoundAcceptingStarts = true;
            s.streamRounds = s.streamRounds.map(round =>
              round.id === lastRound.id
                ? {
                    ...round,
                    thinking: roundThinking ? ((round.thinking || '') + roundThinking) : round.thinking,
                    toolCalls: [...round.toolCalls, nextCall],
                  }
                : round,
            );
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
        }

        // Update flat toolCalls list
        const existing = s.toolCalls.findIndex(tc => tc.callId === callId);
        if (existing >= 0) {
          s.toolCalls = [...s.toolCalls];
          s.toolCalls[existing] = { ...s.toolCalls[existing], toolName, arguments: argumentsText, status: 'running' };
          upsertToolTraceEvent(s, s.toolCalls[existing]);
        } else {
          s.toolCalls = [...s.toolCalls, nextCall];
          upsertToolTraceEvent(s, nextCall);
        }
        } catch (err) {
          console.error('[streamStore] toolCallStart error, creating fallback round:', err);
          // Fallback: create a simple new round with the tool call
          const fallbackCallId = `tool-call-${Date.now()}-${s._toolCallSeq++}`;
          const fallbackCall: ToolCallEvent = {
            callId: fallbackCallId, toolName: 'unknown_tool', arguments: '', status: 'running',
          };
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
          if (doneText) s.streamText = doneText;
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
