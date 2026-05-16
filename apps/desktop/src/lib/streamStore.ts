/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../types/conversation';
import { applyDurableReplayToState, taskTimelineEventsFromReplaySource } from './streaming/durableReplay';
import {
  isTaskLifecycleEventType,
  isTerminalEventType,
  normalizeAgentEventType,
} from './streaming/eventTypes';
import { adaptFrontendRunEvent } from './streaming/legacyAdapter';
import { applyLiveStreamEvent } from './streaming/liveEventReducer';
import { applyStreamEventOrdering } from './streaming/ordering';
import {
  clearToolPreparingTimers,
  createDefaultState,
  taskRunIsActive,
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
export type { StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const TOOL_PREPARING_DELAY_MS = 150;

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

    this.scheduleNotify(conversationId);
  }
}

export const streamStore = new StreamStoreImpl();
