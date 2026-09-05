/**
 * Global streaming store — persists stream state across page navigation.
 * Events are dispatched here by StreamProvider and read by useAgentStream.
 */

import type { AgentFrontendEvent } from '../types';
import { recordAgentFrontendPaint } from './frontendPaintTelemetry';
import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskSnapshotEvent,
  AgentTaskRunEvent,
  AgentTurnHandle,
} from '../types/conversation';
import { projectRunEventsToStreamStateAsync, applyBufferedRunEventsToState } from './streaming/durableReplay';
import {
  enqueueStreamRunEvent,
  parseStreamEventSeq,
  takeAuthoritativeRunEventSuffix,
  takeNextStreamRunEvent,
} from './streaming/ordering';
import { applyAgentRunEvent } from './streaming/runEventReducer';
import { durableRunReconciler } from './streaming/runReconciliationRuntime';
import type { DurableRunReconciliationOutcome } from './streaming/runReconciliation';
import { applyDoneEvent } from './streaming/liveProjection';
import {
  clearToolPreparingTimers,
  createDefaultState,
  capStreamCollections,
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
import {
  classifyAgentRunEventLifecycle,
  suspendAgentRunProjection,
} from './streaming/runEventLifecycle';
export type { ContextUsageBreakdown, StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const TOOL_PREPARING_DELAY_MS = 150;
const MAX_RETAINED_STREAMS = 32;

/* ── Store implementation ───────────────────────────────────────── */

type StoreListener = (conversationId: string) => void;

function stateHasVisiblePreview(state: InternalStreamState | undefined): boolean {
  return Boolean(state && (
    state.traceEvents.length > 0 ||
    state.streamText.length > 0 ||
    state.streamRounds.length > 0 ||
    state.thinkingText.length > 0 ||
    state.toolCalls.length > 0
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
  private _gapRecoveries = new Map<string, string>();
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
    if (
      existing?.isStreaming
      && (existing.turnHandle !== null || stateHasVisiblePreview(existing))
    ) {
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
  async restoreFromRunEvents(
    conversationId: string,
    taskRun: AgentTaskRun,
    runEvents: AgentRunEvent[],
    taskEvents: AgentTaskRunEvent[] = [],
    isCurrent: () => boolean = () => true,
  ): Promise<void> {
    const original = this._streams[conversationId];
    const originalSequence = original?._lastEventSeq;
    const ownsRestore = () => isCurrent() && this._streams[conversationId] === original
      && original?._lastEventSeq === originalSequence;
    const projected = await projectRunEventsToStreamStateAsync(taskRun, runEvents, taskEvents, ownsRestore);
    if (!projected || !ownsRestore()) return;
    // Events buffered while the historical prefix loaded remain authoritative.
    if (original && !runEvents.some(event => classifyAgentRunEventLifecycle(event) === 'terminal')) {
      applyBufferedRunEventsToState(projected, [...original._pendingRunEvents.values()]);
    }
    capStreamCollections(projected);
    this.restoreProjectedState(conversationId, () => projected);
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
    let state = this._streams[conversationId];
    if (!state) return;
    const runWasClaimedByAnotherLaunch = state._orderedRunId !== null
      && state._orderedRunId !== handle.runId;
    if (runWasClaimedByAnotherLaunch) {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      const replacement = createDefaultState();
      replacement.isStreaming = true;
      replacement._launchStartedAt = state._launchStartedAt;
      replacement.turnTiming = state.turnTiming ? {
        ...state.turnTiming,
        firstEventAtEpochMs: null,
        firstVisibleOutputAtEpochMs: null,
        finishedAtEpochMs: null,
        finishedAtMonotonicMs: null,
      } : null;
      this._streams[conversationId] = replacement;
      state = replacement;
    }
    state.turnHandle = handle;
    if (state.isStreaming) {
      this.resetTimeout(conversationId);
      if (runWasClaimedByAnotherLaunch || state._pendingRunEvents.size > 0) {
        this.recoverMissingRunEvents(conversationId, state, handle.runId);
      }
    }
    this.notify(conversationId);
    this.scheduleFrontendFirstPaint(conversationId, state);
  }

  applyTaskSnapshot(event: AgentTaskSnapshotEvent): void {
    const state = this._streams[event.conversationId];
    if (!state) return;
    state.taskRun = event.taskRun;
    this.touch(event.conversationId);
    this.scheduleNotify(event.conversationId);
  }

  recordHeartbeat(conversationId: string, runId: string, durableHighWater?: number | null): void {
    const state = this._streams[conversationId];
    if (!state?.isStreaming || state.turnHandle?.runId !== runId) return;
    // A live executor does not prove its output reached this webview. Keep the
    // recovery deadline intact, including while a reconciliation is in flight.
    if (typeof durableHighWater === 'number'
      && Number.isSafeInteger(durableHighWater)
      && durableHighWater > state._lastEventSeq) {
      this.recoverMissingRunEvents(conversationId, state, runId);
    }
  }

  /** Settle the live transport while the durable turn waits for a response. */
  markResumableSuspension(conversationId: string): void {
    const state = this._streams[conversationId];
    if (!state) return;
    suspendAgentRunProjection(state);
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

  private resetTimeout(conversationId: string, preserveRecoveryAttempt = false): void {
    const s = this._streams[conversationId];
    if (!s) return;
    s._watchdogGeneration += 1;
    if (!preserveRecoveryAttempt) {
      s._watchdogRecoveryAttempt = 0;
      s._watchdogMissingRunConfirmations = 0;
    }
    const generation = s._watchdogGeneration;
    armStreamWatchdog(s, () => {
      void this.recoverSuspectedStall(conversationId, generation);
    });
  }

  private recoverMissingRunEvents(
    conversationId: string,
    state: InternalStreamState,
    runIdHint?: string,
  ): void {
    const runId = state.turnHandle?.runId ?? runIdHint;
    if (!runId || this._gapRecoveries.get(conversationId) === runId) return;
    this.recoverRunEventGap(conversationId, runId);
  }

  private recoverRunEventGap(conversationId: string, runId: string): void {
    this._gapRecoveries.set(conversationId, runId);
    const isCurrent = () => {
      const state = this._streams[conversationId];
      if (!state) return false;
      const boundRunId = state.turnHandle?.runId;
      const pendingRunMatches = [...state._pendingRunEvents.values()]
        .some(event => event.runId === runId);
      return !(
        (boundRunId && boundRunId !== runId)
        || (!boundRunId && !pendingRunMatches)
      );
    };
    void durableRunReconciler.recoverGap({
      runId,
      afterEventSeq: this._streams[conversationId]?._lastEventSeq,
      isCurrent,
      pendingHighWater: () => {
        const state = this._streams[conversationId];
        if (!state) return null;
        return this.pendingRunEventHighWater(state, runId);
      },
      accept: (runEvents, page) => this.applyAuthoritativeRunEventSuffix(
        conversationId,
        runId,
        runEvents,
        {
          includeLivePending: page.complete,
          authoritativeThroughEventSeq: page.authoritativeThroughEventSeq,
        },
      ),
    }).then(outcome => {
      if (outcome.kind !== 'exhausted' || !isCurrent()) return;
      const state = this._streams[conversationId];
      if (!state) return;
      state._pendingRunEvents.clear();
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      const message = 'The response stream could not recover a missing event. Reload this conversation.';
      applyTerminalProjection(state, {
        toolStatus: 'error',
        message,
        toolFallbackMessage: 'Interrupted',
        traceTone: 'error',
        errorMessage: message,
      });
      this.finishTurnTiming(state);
      this.touch(conversationId);
      this.evictCompletedStreams(conversationId);
      this.notifyImmediately(conversationId);
    }).finally(() => {
      if (this._gapRecoveries.get(conversationId) === runId) {
        this._gapRecoveries.delete(conversationId);
      }
    });
  }

  private applyAuthoritativeRunEventSuffix(
    conversationId: string,
    runId: string,
    runEvents: AgentRunEvent[],
    options: {
      includeLivePending: boolean;
      authoritativeThroughEventSeq: number;
    },
  ): boolean {
    const state = this._streams[conversationId];
    if (!state || (state.turnHandle?.runId && state.turnHandle.runId !== runId)) return false;

    const ordered = takeAuthoritativeRunEventSuffix(state, runId, runEvents, options);
    for (const runEvent of ordered) {
      if (this.applyOrderedRunEvent(conversationId, state, runEvent)) {
        state._pendingRunEvents.clear();
        break;
      }
    }
    return [...state._pendingRunEvents.values()].some(event => event.runId === runId);
  }

  private pendingRunEventHighWater(
    state: InternalStreamState,
    runId: string,
  ): number | null {
    let highWater: number | null = null;
    for (const event of state._pendingRunEvents.values()) {
      if (event.runId !== runId) continue;
      const eventSeq = parseStreamEventSeq(event.eventSeq);
      if (eventSeq === null) continue;
      highWater = Math.max(highWater ?? 0, eventSeq);
    }
    return highWater;
  }

  private watchdogStateIsCurrent(
    conversationId: string,
    generation: number,
    runId?: string,
  ): InternalStreamState | null {
    const state = this._streams[conversationId];
    if (
      !state
      || !state.isStreaming
      || state._watchdogGeneration !== generation
      || (runId && state.turnHandle?.runId !== runId)
    ) return null;
    return state;
  }

  private setWatchdogConnectionState(
    state: InternalStreamState,
    connectionState: 'degraded' | 'reconnecting' | 'recovered' | 'failed',
  ): void {
    state.connectionState = {
      state: connectionState,
      providerId: state.taskRun?.provider ?? 'unknown',
      modelId: state.taskRun?.model ?? 'unknown',
      errorCategory: connectionState === 'recovered' ? null : 'timeout',
      attempt: state._watchdogRecoveryAttempt,
      maxAttempts: 0,
      nextRetryAt: null,
      recoverable: connectionState !== 'failed',
      queuedUserInputs: 0,
      turnPreserved: true,
    };
  }

  private rearmWatchdogRecovery(
    conversationId: string,
    state: InternalStreamState,
    message: string,
  ): void {
    this.setWatchdogConnectionState(state, 'reconnecting');
    appendStatusTraceEvent(
      state,
      message,
      'muted',
      state._watchdogRecoveryAttempt === 1 ? 'user' : 'internal',
      'recovery',
    );
    this.touch(conversationId);
    capStreamCollections(state);
    this.notifyImmediately(conversationId);
    this.resetTimeout(conversationId, true);
  }

  private settleConfirmedTaskRun(
    conversationId: string,
    state: InternalStreamState,
    outcome: Extract<
      DurableRunReconciliationOutcome,
      { kind: 'suspended' | 'completed' | 'terminal' }
    >,
  ): void {
    if (outcome.kind === 'suspended') {
      suspendAgentRunProjection(state);
    } else if (outcome.kind === 'completed') {
      clearToolPreparingTimers(state);
      applyDoneEvent(state, {
        status: 'completed',
        message: outcome.finalMessage,
      });
    } else if (outcome.status === 'cancelled') {
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: 'cancelled',
        message: 'Backend confirmed this turn was cancelled.',
        traceTone: 'success',
        errorMessage: null,
      });
    } else {
      const message = outcome.snapshot.taskRun.errorMessage?.trim()
        || (outcome.status === 'timed_out'
          ? 'Backend confirmed this turn timed out.'
          : 'Backend confirmed this turn failed.');
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: outcome.status === 'timed_out' ? 'timedOut' : 'error',
        message,
        traceTone: 'error',
        errorMessage: message,
      });
    }

    this.finishTurnTiming(state);
    this.touch(conversationId);
    this.evictCompletedStreams(conversationId);
    this.notifyImmediately(conversationId);
  }

  private async recoverSuspectedStall(
    conversationId: string,
    generation: number,
  ): Promise<void> {
    let state = this.watchdogStateIsCurrent(conversationId, generation);
    if (!state) return;

    state._watchdogRecoveryAttempt += 1;
    const expectedRunId = state.turnHandle?.runId;
    const expectedTurnId = state.turnHandle?.turnId;
    const authoritativeLiveThrough = expectedRunId
      ? this.pendingRunEventHighWater(state, expectedRunId)
      : null;
    this.setWatchdogConnectionState(state, 'degraded');
    appendStatusTraceEvent(
      state,
      'No live events received; checking durable backend state.',
      'muted',
      state._watchdogRecoveryAttempt === 1 ? 'user' : 'internal',
      'recovery',
    );
    this.notifyImmediately(conversationId);

    try {
      const outcome = await durableRunReconciler.reconcile({
        reason: 'watchdog',
        conversationId,
        expectedRunId,
        expectedTurnId,
        missingRunConfirmations: state._watchdogMissingRunConfirmations,
        afterEventSeq: state._lastEventSeq,
        isCurrent: () => Boolean(
          this.watchdogStateIsCurrent(conversationId, generation, expectedRunId),
        ),
      });
      if (outcome.kind === 'stale') return;
      state = this.watchdogStateIsCurrent(conversationId, generation, expectedRunId);
      if (!state) return;

      if (outcome.kind === 'unavailable') {
        state._watchdogMissingRunConfirmations = 0;
        this.rearmWatchdogRecovery(
          conversationId,
          state,
          `Backend recovery query failed; retrying (${outcome.error}).`,
        );
        return;
      }

      if (outcome.kind === 'missing' || outcome.kind === 'idle') {
        state._watchdogMissingRunConfirmations = outcome.kind === 'missing'
          ? outcome.confirmations
          : 0;
        if (outcome.kind === 'missing' && outcome.exhausted) {
          const message = 'Connection lost: the backend no longer has this turn.';
          this.setWatchdogConnectionState(state, 'failed');
          clearToolPreparingTimers(state);
          applyTerminalProjection(state, {
            toolStatus: 'error',
            message,
            traceTone: 'error',
            errorMessage: message,
          });
          this.finishTurnTiming(state);
          this.touch(conversationId);
          this.evictCompletedStreams(conversationId);
          this.notifyImmediately(conversationId);
          return;
        }
        this.rearmWatchdogRecovery(
          conversationId,
          state,
          expectedRunId
            ? `The backend has not exposed this turn yet; recovery will retry (${state._watchdogMissingRunConfirmations}/3 confirmations).`
            : 'The backend has not exposed this turn yet; recovery will retry.',
        );
        return;
      }
      state._watchdogMissingRunConfirmations = 0;

      state.taskRun = outcome.snapshot.taskRun;
      state.taskEvents = outcome.snapshot.taskEvents;
      this.applyAuthoritativeRunEventSuffix(
        conversationId,
        outcome.snapshot.taskRun.id,
        outcome.snapshot.runEvents,
        {
          includeLivePending: true,
          authoritativeThroughEventSeq: Math.max(
            authoritativeLiveThrough ?? 0,
            outcome.snapshot.runEvents[outcome.snapshot.runEvents.length - 1]?.eventSeq
              ?? state._lastEventSeq,
          ),
        },
      );

      state = this._streams[conversationId];
      if (!state || !state.isStreaming) return;
      if (outcome.kind === 'active') {
        this.setWatchdogConnectionState(state, 'recovered');
        appendStatusTraceEvent(
          state,
          'Durable backend state is active; live recovery remains armed.',
          'success',
          'user',
          'recovery',
        );
        this.touch(conversationId);
        capStreamCollections(state);
        this.notifyImmediately(conversationId);
        this.resetTimeout(conversationId, true);
        return;
      }
      if (
        outcome.kind === 'suspended'
        || outcome.kind === 'completed'
        || outcome.kind === 'terminal'
      ) {
        this.settleConfirmedTaskRun(conversationId, state, outcome);
        return;
      }

      this.rearmWatchdogRecovery(
        conversationId,
        state,
        outcome.reason === 'finalMessage'
          ? 'Backend completed this turn, but the final assistant message is not durable yet; recovery will retry.'
          : `Backend status '${outcome.snapshot.taskRun.status}' is not terminal; recovery will retry.`,
      );
    } catch (error) {
      state = this.watchdogStateIsCurrent(conversationId, generation, expectedRunId);
      if (!state) return;
      state._watchdogMissingRunConfirmations = 0;
      this.rearmWatchdogRecovery(
        conversationId,
        state,
        `Backend recovery query failed; retrying (${error instanceof Error ? error.message : String(error)}).`,
      );
    }
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

  /** Process one versioned Run Event envelope. */
  dispatch(conversationId: string, event: AgentFrontendEvent): void {
    const runEvent = event.runEvent;
    let state = this._streams[conversationId];
    if (!state) {
      state = createDefaultState();
      state.isStreaming = true;
      this._streams[conversationId] = state;
    }
    if (
      state.turnHandle?.runId
      && state.turnHandle.runId !== runEvent.runId
    ) {
      // Once the launch handshake binds a run, that identity remains
      // authoritative for the lifetime of the retained projection. A late
      // event from the retired run must not claim ordering either before the
      // new run's first event or after the new run settles.
      return;
    }

    let enqueue = enqueueStreamRunEvent(state, runEvent);
    if (enqueue.runChanged) {
      // A live run owns its route identity. A different run may replace only
      // a settled retained projection (for example, a background workflow
      // that has no local startStream handshake).
      if (state.isStreaming) return;
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      state = createDefaultState();
      state.isStreaming = true;
      this._streams[conversationId] = state;
      enqueue = enqueueStreamRunEvent(state, runEvent);
    }
    if (!enqueue.accepted) return;
    if (!enqueue.ready) {
      this.recoverMissingRunEvents(conversationId, state, runEvent.runId);
      return;
    }

    let orderedEvent: AgentRunEvent | null;
    while ((orderedEvent = takeNextStreamRunEvent(state)) !== null) {
      if (this.applyOrderedRunEvent(conversationId, state, orderedEvent)) {
        state._pendingRunEvents.clear();
        break;
      }
    }
  }

  /** Returns true when the applied event is terminal. */
  private applyOrderedRunEvent(
    conversationId: string,
    state: InternalStreamState,
    runEvent: AgentRunEvent,
  ): boolean {
    const lifecycle = classifyAgentRunEventLifecycle(runEvent);
    const isTerminalEvent = lifecycle === 'terminal';
    const isResumableSuspension = lifecycle === 'suspension';
    const reopensSuspendedStream = lifecycle === 'resume';
    if (!state.isStreaming && !isTerminalEvent && !reopensSuspendedStream) return false;

    this.markFirstEventTiming(state);
    if (reopensSuspendedStream) {
      state.isStreaming = true;
      state.isThinking = false;
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
    if (isResumableSuspension) {
      suspendAgentRunProjection(state);
    }
    if (isTerminalEvent || isResumableSuspension) this.finishTurnTiming(state);
    this.touch(conversationId);
    capStreamCollections(state);
    if (isTerminalEvent) this.evictCompletedStreams(conversationId);
    if (
      isTerminalEvent
      || isResumableSuspension
      || runEvent.kind === 'approvalRequested'
      || runEvent.kind === 'approvalResolved'
    ) {
      this.notifyImmediately(conversationId);
    } else {
      this.scheduleNotify(conversationId);
    }
    this.scheduleFrontendFirstPaint(conversationId, state);
    return isTerminalEvent;
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
