/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import { recordAgentFrontendPaint } from './frontendPaintTelemetry';
import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
  AgentTurnHandle,
} from '../types/conversation';
import { projectRunEventsToStreamState } from './streaming/durableReplay';
import {
  isTaskLifecycleEventType,
  isTerminalEventType,
  normalizeAgentEventType,
} from './streaming/eventTypes';
import { applyLiveStreamEvent } from './streaming/liveEventReducer';
import { applyStreamEventOrdering } from './streaming/ordering';
import { applyAgentRunEvent } from './streaming/runEventReducer';
import {
  clearToolPreparingTimers,
  createDefaultState,
  type InternalStreamState,
} from './streaming/state';
import {
  appendStatusTraceEvent,
  applyTerminalProjection,
  resetActiveStreamBlocks,
} from './streaming/terminalProjection';
import {
  createToolCall,
  insertPendingToolCall,
} from './streaming/toolProjection';
import type {
  StreamState,
} from './streaming/protocol';
import { armStreamWatchdog, clearStreamWatchdog } from './streaming/watchdog';
import { ConversationFrameBatcher } from './streaming/frameBatcher';
export type { ContextUsageBreakdown, StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const TOOL_PREPARING_DELAY_MS = 150;
const MAX_RETAINED_STREAMS = 32;

/* ── Store implementation ───────────────────────────────────────── */

type StoreListener = (conversationId: string) => void;

function stateHasVisiblePreview(state: InternalStreamState | undefined): boolean {
  return Boolean(state && (
    state.isStreaming ||
    state.traceEvents.length > 0 ||
    state.streamText.length > 0 ||
    state.streamRounds.length > 0
  ));
}

function stateHasVisibleGeneratedContent(state: InternalStreamState): boolean {
  return Boolean(
    state.streamText
    || state.thinkingText
    || state.toolCalls.length > 0
    || state.streamRounds.some(round => Boolean(round.reply || round.thinking)),
  );
}

function nextAnimationFrame(callback: () => void): void {
  if (typeof globalThis.requestAnimationFrame === 'function') {
    globalThis.requestAnimationFrame(callback);
    return;
  }
  globalThis.setTimeout(callback, 0);
}

class StreamStoreImpl {
  private _streams: Record<string, InternalStreamState> = {};
  private _recency = new Map<string, number>();
  private _recencyTick = 0;
  private _listeners = new Set<StoreListener>();
  private _notifications = new ConversationFrameBatcher(
    conversationId => this.notify(conversationId),
  );

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
    this._notifications.schedule(conversationId);
  }

  private notifyImmediately(conversationId: string): void {
    this._notifications.flushNow(conversationId);
  }

  private capLiveCollections(state: InternalStreamState): void {
    if (state.traceEvents.length > 512) {
      state.traceEvents = state.traceEvents.slice(-512);
    }
    if (state.streamRounds.length > 128) {
      state.streamRounds = state.streamRounds.slice(-128);
    }
    if (state.taskEvents.length > 256) {
      state.taskEvents = state.taskEvents.slice(-256);
    }
  }

  private touch(conversationId: string): void {
    this._recencyTick += 1;
    this._recency.set(conversationId, this._recencyTick);
  }

  private evictCompletedStreams(protectedConversationId?: string): void {
    while (Object.keys(this._streams).length > MAX_RETAINED_STREAMS) {
      const candidate = Object.entries(this._streams)
        .filter(([id, state]) => id !== protectedConversationId && !state.isStreaming)
        .sort(
          ([left], [right]) => (this._recency.get(left) ?? 0) - (this._recency.get(right) ?? 0),
        )[0];
      if (!candidate) return;

      const [conversationId, state] = candidate;
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      delete this._streams[conversationId];
      this._recency.delete(conversationId);
      this.notify(conversationId);
    }
  }

  getStream(id: string): StreamState | undefined {
    const s = this._streams[id];
    if (!s) return undefined;
    this.touch(id);
    return {
      turnHandle: s.turnHandle,
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
      connectionState: s.connectionState,
      autoCompacted: s.autoCompacted,
      pendingApprovals: s.pendingApprovals,
      taskRun: s.taskRun,
      taskEvents: s.taskEvents,
      turnTiming: s.turnTiming,
    };
  }

  /** Find the conversation ID of any currently active stream. */
  getActiveStreamId(): string | null {
    for (const [id, state] of Object.entries(this._streams)) {
      if (state.isStreaming) return id;
    }
    return null;
  }

  /** Return every conversation that currently owns a live stream. */
  getRunningConversationIds(): string[] {
    return Object.entries(this._streams)
      .filter(([, state]) => state.isStreaming)
      .map(([id]) => id);
  }

  private restoreProjectedState(
    conversationId: string,
    stateFactory: () => InternalStreamState,
  ): void {
    const existing = this._streams[conversationId];
    if (existing?.isStreaming && stateHasVisiblePreview(existing)) {
      return;
    }
    if (existing) clearStreamWatchdog(existing);
    if (existing) clearToolPreparingTimers(existing);

    const state = stateFactory();
    this._streams[conversationId] = state;
    this.touch(conversationId);
    if (state.isStreaming) {
      this.resetTimeout(conversationId);
    }
    this.evictCompletedStreams(conversationId);
    this.notify(conversationId);
  }

  /** Rebuild the visible stream preview directly from canonical durable Run Events. */
  restoreFromRunEvents(
    conversationId: string,
    taskRun: AgentTaskRun,
    runEvents: AgentRunEvent[],
    taskEvents: AgentTaskRunEvent[] = [],
  ): void {
    this.restoreProjectedState(conversationId, () =>
      projectRunEventsToStreamState(taskRun, runEvents, taskEvents, { interruptActive: true }),
    );
  }

  /** Initialize (or reset) stream state for a conversation. */
  startStream(conversationId: string, launchStartedAt?: number): void {
    const existing = this._streams[conversationId];
    if (existing) {
      clearStreamWatchdog(existing);
      clearToolPreparingTimers(existing);
    }

    const state = createDefaultState();
    state.isStreaming = true;
    const startedAtMonotonicMs = launchStartedAt ?? globalThis.performance?.now() ?? Date.now();
    state._launchStartedAt = startedAtMonotonicMs;
    state.turnTiming = {
      startedAtEpochMs: Date.now(),
      startedAtMonotonicMs,
      firstEventAtEpochMs: null,
      firstVisibleOutputAtEpochMs: null,
      finishedAtEpochMs: null,
      finishedAtMonotonicMs: null,
    };
    this._streams[conversationId] = state;
    this.touch(conversationId);
    this.evictCompletedStreams(conversationId);
    this.resetTimeout(conversationId);
    this.notify(conversationId);
  }

  /** Bind the authoritative runtime identity returned by the launch handshake. */
  bindTurnHandle(conversationId: string, handle: AgentTurnHandle): void {
    const state = this._streams[conversationId];
    if (!state) return;
    state.turnHandle = handle;
    this.notify(conversationId);
    this.scheduleFrontendFirstPaint(conversationId, state);
  }

  /** Settle the live transport while the durable turn waits for a response. */
  markAwaitingUserInput(conversationId: string): void {
    const state = this._streams[conversationId];
    if (!state) return;
    clearStreamWatchdog(state);
    clearToolPreparingTimers(state);
    state.isStreaming = false;
    state.isThinking = false;
    this.finishTurnTiming(state);
    this.touch(conversationId);
    this.notifyImmediately(conversationId);
  }

  /** Remove stream state entirely. */
  clearStream(conversationId: string): void {
    const existing = this._streams[conversationId];
    if (!existing) return;
    clearStreamWatchdog(existing);
    clearToolPreparingTimers(existing);
    delete this._streams[conversationId];
    this._recency.delete(conversationId);
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
      toolStatus: 'cancelled',
      message: 'Stopped by user',
      traceTone: 'error',
      errorMessage: null,
    });
    this.finishTurnTiming(s);
    this.touch(conversationId);
    this.evictCompletedStreams(conversationId);
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
    this.finishTurnTiming(s);
    this.touch(conversationId);
    this.evictCompletedStreams(conversationId);
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
      this.finishTurnTiming(state);
      this.touch(conversationId);
      this.evictCompletedStreams(conversationId);
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

  private scheduleFrontendFirstPaint(
    conversationId: string,
    state: InternalStreamState,
  ): void {
    if (
      state._frontendPaintScheduled
      || state._frontendPaintReported
      || state._launchStartedAt === null
      || !state.turnHandle
      || !stateHasVisibleGeneratedContent(state)
    ) return;

    state._frontendPaintScheduled = true;
    nextAnimationFrame(() => {
      nextAnimationFrame(() => {
        const current = this._streams[conversationId];
        if (!current || current._frontendPaintReported || !current.turnHandle) return;
        current._frontendPaintScheduled = false;
        current._frontendPaintReported = true;
        if (current.turnTiming && current.turnTiming.firstVisibleOutputAtEpochMs == null) {
          current.turnTiming = {
            ...current.turnTiming,
            firstVisibleOutputAtEpochMs: Date.now(),
          };
          this.scheduleNotify(conversationId);
        }
        const elapsedMs = (globalThis.performance?.now() ?? Date.now())
          - (current._launchStartedAt ?? 0);
        void recordAgentFrontendPaint(
          conversationId,
          current.turnHandle.runId,
          current.turnHandle.turnId,
          elapsedMs,
        ).catch(() => {
          // Paint telemetry must never affect the live conversation.
        });
      });
    });
  }

  /** Process an incoming agent event. */
  dispatch(conversationId: string, event: AgentFrontendEvent): void {
    if (event.runEvent) {
      const runEvent = event.runEvent;
      const isTerminalEvent = runEvent.kind === 'done' || runEvent.kind === 'error';
      const isAwaitingUserInput = runEvent.kind === 'status'
        && runEvent.phase === 'awaiting_user_input';
      let state = this._streams[conversationId];
      if (!state) {
        state = createDefaultState();
        state.isStreaming = !isTerminalEvent;
        this._streams[conversationId] = state;
      }
      if (!state.isStreaming && !isTerminalEvent) return;

      this.markFirstEventTiming(state);

      const ordering = applyStreamEventOrdering(state, runEvent.eventSeq);
      if (!ordering.accepted) return;
      if (ordering.gapDetected) {
        appendStatusTraceEvent(
          state,
          'Stream event gap detected; replay may be required.',
          'muted',
          'internal',
        );
      }
      if (state.isStreaming) this.resetTimeout(conversationId);

      applyAgentRunEvent(state, runEvent, {
        scheduleToolPreparing: payload => {
          this.scheduleToolPreparing(
            conversationId,
            payload.callId,
            payload.toolName,
            payload.argsBytes,
          );
        },
      });
      if (isAwaitingUserInput) {
        clearStreamWatchdog(state);
        clearToolPreparingTimers(state);
        state.isStreaming = false;
        state.isThinking = false;
      }
      if (isTerminalEvent || isAwaitingUserInput) this.finishTurnTiming(state);
      this.touch(conversationId);
      this.capLiveCollections(state);
      if (isTerminalEvent) this.evictCompletedStreams(conversationId);
      if (
        isTerminalEvent
        || isAwaitingUserInput
        || runEvent.kind === 'approvalRequested'
        || runEvent.kind === 'approvalResolved'
      ) {
        this.notifyImmediately(conversationId);
      } else {
        this.scheduleNotify(conversationId);
      }
      this.scheduleFrontendFirstPaint(conversationId, state);
      return;
    }

    const raw = event as AgentFrontendEvent & Record<string, unknown>;
    const eventType = normalizeAgentEventType(raw.type);
    if (!eventType) return;
    const isTaskLifecycleEvent = isTaskLifecycleEventType(eventType);
    const isTerminalEvent = isTerminalEventType(eventType);

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

    this.markFirstEventTiming(s);

    const ordering = applyStreamEventOrdering(s, event.eventSeq ?? raw.eventSeq);
    if (!ordering.accepted) return;
    if (ordering.gapDetected) {
      appendStatusTraceEvent(
        s,
        'Stream event gap detected; replay may be required.',
        'muted',
        'internal',
      );
    }

    // Reset inactivity timeout on every event, including empty keepalive
    // `thinking` events emitted while the backend is still working.
    if (s.isStreaming) {
      this.resetTimeout(conversationId);
    }

    applyLiveStreamEvent(s, eventType, event, raw, {
      scheduleToolPreparing: payload => {
        this.scheduleToolPreparing(
          conversationId,
          payload.callId,
          payload.toolName,
          payload.argsBytes,
        );
      },
    });
    if (isTerminalEvent) this.finishTurnTiming(s);
    this.touch(conversationId);
    this.capLiveCollections(s);
    if (isTerminalEvent) this.evictCompletedStreams(conversationId);
    if (
      isTerminalEvent
      || eventType === 'approvalRequested'
      || eventType === 'approvalResolved'
    ) {
      this.notifyImmediately(conversationId);
    } else {
      this.scheduleNotify(conversationId);
    }
    this.scheduleFrontendFirstPaint(conversationId, s);
  }

  private markFirstEventTiming(state: InternalStreamState): void {
    if (!state.turnTiming) {
      state.turnTiming = {
        startedAtEpochMs: Date.now(),
        startedAtMonotonicMs: globalThis.performance?.now() ?? Date.now(),
        firstEventAtEpochMs: Date.now(),
        firstVisibleOutputAtEpochMs: null,
        finishedAtEpochMs: null,
        finishedAtMonotonicMs: null,
      };
      return;
    }
    if (state.turnTiming.firstEventAtEpochMs == null) {
      state.turnTiming = { ...state.turnTiming, firstEventAtEpochMs: Date.now() };
    }
  }

  private finishTurnTiming(state: InternalStreamState): void {
    if (!state.turnTiming || state.turnTiming.finishedAtEpochMs != null) return;
    state.turnTiming = {
      ...state.turnTiming,
      finishedAtEpochMs: Date.now(),
      finishedAtMonotonicMs: globalThis.performance?.now() ?? Date.now(),
    };
  }
}

export const streamStore = new StreamStoreImpl();
