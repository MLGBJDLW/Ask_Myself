/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  ArtifactPayload,
  ToolRunItem,
} from '../types/conversation';
import {
  appendReplyTraceEvent,
  appendThinkingTraceEvent,
  applyStreamBlockDelta,
} from './streaming/blockProjection';
import { applyDurableReplayToState, taskTimelineEventsFromReplaySource } from './streaming/durableReplay';
import { adaptFrontendRunEvent } from './streaming/legacyAdapter';
import {
  applyApprovalRequestedEvent,
  applyApprovalResolvedEvent,
  applyAutoCompactedEvent,
  applyDoneEvent,
  applyErrorEvent,
  applyStatusEvent,
  applyTaskRunEvent,
  applyTaskRunUpdatedEvent,
  applyUsageUpdateEvent,
} from './streaming/liveProjection';
import { applyStreamEventOrdering } from './streaming/ordering';
import {
  clearToolPreparingTimer,
  clearToolPreparingTimers,
  createDefaultState,
  taskRunIsActive,
  type InternalStreamState,
} from './streaming/state';
import {
  appendStatusTraceEvent,
  applyStreamResetProjection,
  applyTerminalProjection,
  isPendingStatus,
  resetActiveStreamBlocks,
  syncTraceToolEvents,
} from './streaming/terminalProjection';
import {
  applyToolRunEvent,
  createToolCall,
  insertPendingToolCall,
  PROGRESS_NOTES_MAX,
  resolveToolCallResult,
  upsertToolTraceEvent,
} from './streaming/toolProjection';
import type {
  StreamState,
  ToolCallEvent,
} from './streaming/protocol';
import { armStreamWatchdog, clearStreamWatchdog } from './streaming/watchdog';
export type { StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const TOOL_PREPARING_DELAY_MS = 150;

/* ── Helper functions ───────────────────────────────────────────── */

type AgentEventType = NonNullable<AgentFrontendEvent['type']>;

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
    if (existing) clearStreamWatchdog(existing);
    if (existing) clearToolPreparingTimers(existing);

    const state = createDefaultState();
    state.isStreaming = taskRunIsActive(taskRun);
    state.taskRun = taskRun;
    state.taskEvents = taskTimelineEventsFromReplaySource(taskEvents);

    applyDurableReplayToState(state, taskEvents);

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
      clearStreamWatchdog(existing);
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
    clearStreamWatchdog(existing);
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
    clearStreamWatchdog(s);
    clearToolPreparingTimers(s);
    applyTerminalProjection(s, {
      toolStatus: 'error',
      message: 'Stopped by user',
      traceTone: 'error',
    });
    this.notify(conversationId);
  }

  /** Handle a send() failure (api.agentChat threw). */
  sendError(conversationId: string, errorMessage: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    clearStreamWatchdog(s);
    clearToolPreparingTimers(s);
    applyTerminalProjection(s, {
      toolStatus: 'error',
      message: errorMessage || 'Request failed',
      toolFallbackMessage: 'Request failed',
      traceTone: 'error',
      errorMessage,
    });
    this.notify(conversationId);
  }

  private resetTimeout(conversationId: string): void {
    const s = this._streams[conversationId];
    if (!s) return;
    armStreamWatchdog(s, () => {
      const state = this._streams[conversationId];
      if (!state) return;
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: 'error',
        message: 'Connection lost',
        traceTone: 'error',
        errorMessage: 'Connection lost',
      });
      this.notify(conversationId);
    });
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
    const isTerminalEvent = eventType === 'done' || eventType === 'error';

    let s = this._streams[conversationId];
    if (!s) {
      if (!event.runEvent && !isTaskLifecycleEvent && !isTerminalEvent) {
        return;
      }
      s = createDefaultState();
      s.isStreaming = !isTerminalEvent;
      this._streams[conversationId] = s;
    }
    if (!s.isStreaming && !isTaskLifecycleEvent && !isTerminalEvent) return;

    const ordering = applyStreamEventOrdering(s, event.eventSeq ?? raw.eventSeq);
    if (!ordering.accepted) return;
    if (ordering.gapDetected) {
      appendStatusTraceEvent(s, 'Stream event gap detected; replay may be required.', 'muted');
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
        clearToolPreparingTimers(s);
        applyStreamResetProjection(s, reason, { clearTools: true });
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
        applyUsageUpdateEvent(s, event, raw);
        break;
      }

      case 'status': {
        applyStatusEvent(s, event, raw);
        break;
      }

      case 'done': {
        clearStreamWatchdog(s);
        clearToolPreparingTimers(s);
        applyDoneEvent(s, event, raw);
        break;
      }

      case 'autoCompacted': {
        applyAutoCompactedEvent(s, event, raw);
        break;
      }

      case 'approvalRequested': {
        applyApprovalRequestedEvent(s, event, raw);
        break;
      }

      case 'approvalResolved': {
        applyApprovalResolvedEvent(s, event, raw);
        break;
      }

      case 'taskRunUpdated': {
        applyTaskRunUpdatedEvent(s, event, raw);
        break;
      }

      case 'taskRunEvent': {
        applyTaskRunEvent(s, event, raw);
        break;
      }

      case 'error': {
        clearStreamWatchdog(s);
        clearToolPreparingTimers(s);
        applyErrorEvent(s, event, raw);
        break;
      }
    }

    this.scheduleNotify(conversationId);
  }
}

export const streamStore = new StreamStoreImpl();
