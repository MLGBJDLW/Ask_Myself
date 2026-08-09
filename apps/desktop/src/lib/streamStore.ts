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
  ConversationMessage,
  ConversationTurn,
} from '../types/conversation';
import { projectRunEventsToStreamState } from './streaming/durableReplay';
import {
  enqueueStreamRunEvent,
  takeNextStreamRunEvent,
} from './streaming/ordering';
import { applyAgentRunEvent } from './streaming/runEventReducer';
import {
  getRecoveryRunEvents,
  getRecoveryTaskEvents,
  getRecoveryTaskRuns,
  getRecoveryConversation,
  getRecoveryConversationTurns,
} from './streaming/recoveryApi';
import { applyDoneEvent } from './streaming/liveProjection';
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
import { ConversationFrameBatcher } from './streaming/frameBatcher';
export type { ContextUsageBreakdown, StreamRoundEvent, StreamState, ToolCallEvent, TraceEvent, UsageTotal } from './streaming/protocol';

const TOOL_PREPARING_DELAY_MS = 150;
const MAX_RETAINED_STREAMS = 32;
const WATCHDOG_RECOVERY_QUERY_TIMEOUT_MS = 10_000;
const MAX_RUN_EVENT_GAP_RECOVERY_ATTEMPTS = 3;

async function withWatchdogRecoveryTimeout<T>(query: Promise<T>, label: string): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  try {
    return await Promise.race([
      query,
      new Promise<T>((_resolve, reject) => {
        timeoutId = setTimeout(() => {
          reject(new Error(`${label} exceeded ${WATCHDOG_RECOVERY_QUERY_TIMEOUT_MS}ms`));
        }, WATCHDOG_RECOVERY_QUERY_TIMEOUT_MS);
      }),
    ]);
  } finally {
    if (timeoutId !== null) clearTimeout(timeoutId);
  }
}

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

function finalAssistantMessageForTaskRun(
  taskRun: AgentTaskRun,
  turns: ConversationTurn[],
  messages: ConversationMessage[],
): ConversationMessage | null {
  const turn = turns.find(candidate => candidate.id === taskRun.turnId);
  if (turn?.assistantMessageId) {
    const mapped = messages.find(message => message.id === turn.assistantMessageId);
    if (mapped?.role === 'assistant' && mapped.content.trim()) return mapped;
  }

  const taskUserIndex = messages.findIndex(message => message.id === taskRun.userMessageId);
  if (taskUserIndex < 0) return null;
  for (let index = taskUserIndex + 1; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role === 'user') break;
    if (message.role === 'assistant' && message.content.trim()) return message;
  }
  return null;
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
  private _gapRecoveries = new Set<string>();
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
  restoreFromRunEvents(
    conversationId: string,
    taskRun: AgentTaskRun,
    runEvents: AgentRunEvent[],
    taskEvents: AgentTaskRunEvent[] = [],
  ): void {
    this.restoreProjectedState(conversationId, () =>
      projectRunEventsToStreamState(taskRun, runEvents, taskEvents, {
        interruptActive: taskRun.status === 'cancelling',
      }),
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
    if (state.isStreaming) {
      this.resetTimeout(conversationId);
      if (state._pendingRunEvents.size > 0) this.recoverMissingRunEvents(conversationId, state);
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

  recordHeartbeat(conversationId: string, runId: string): void {
    const state = this._streams[conversationId];
    if (!state?.isStreaming || state.turnHandle?.runId !== runId) return;
    this.resetTimeout(conversationId);
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
    if (!runId || this._gapRecoveries.has(conversationId)) return;
    this.recoverRunEventGap(conversationId, runId);
  }

  private recoverRunEventGap(conversationId: string, runId: string): void {
    this._gapRecoveries.add(conversationId);
    void withWatchdogRecoveryTimeout(
      getRecoveryRunEvents(runId),
      'Settled run-event gap recovery query',
    ).then(runEvents => {
      const state = this._streams[conversationId];
      if (!state) return;
      const boundRunId = state.turnHandle?.runId;
      const pendingRunMatches = [...state._pendingRunEvents.values()]
        .some(event => event.runId === runId);
      if ((boundRunId && boundRunId !== runId) || (!boundRunId && !pendingRunMatches)) return;
      for (const runEvent of [...runEvents].sort((left, right) => left.eventSeq - right.eventSeq)) {
        this.dispatch(conversationId, { conversationId, runEvent } as AgentFrontendEvent);
      }
      this.retryRunEventGapIfNeeded(conversationId, runId);
    }).catch(() => {
      this.retryRunEventGapIfNeeded(conversationId, runId);
    }).finally(() => {
      this._gapRecoveries.delete(conversationId);
    });
  }

  private retryRunEventGapIfNeeded(conversationId: string, runId: string): void {
    const state = this._streams[conversationId];
    if (!state) return;
    const pendingRunMatches = [...state._pendingRunEvents.values()]
      .some(event => event.runId === runId);
    if (!pendingRunMatches) {
      state._runEventGapRecoveryAttempt = 0;
      return;
    }

    state._runEventGapRecoveryAttempt += 1;
    if (state._runEventGapRecoveryAttempt > MAX_RUN_EVENT_GAP_RECOVERY_ATTEMPTS) {
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
      return;
    }

    const delayMs = Math.min(250 * (2 ** (state._runEventGapRecoveryAttempt - 1)), 2_000);
    armStreamWatchdog(state, () => {
      const current = this._streams[conversationId];
      if (!current) return;
      this.recoverMissingRunEvents(conversationId, current, runId);
    }, delayMs);
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
    this.capLiveCollections(state);
    this.notifyImmediately(conversationId);
    this.resetTimeout(conversationId, true);
  }

  private settleConfirmedTaskRun(
    conversationId: string,
    state: InternalStreamState,
    taskRun: AgentTaskRun,
    finalMessage: ConversationMessage | null = null,
  ): boolean {
    const status = taskRun.status.toLowerCase();
    if (taskRun.phase === 'awaiting_user_input' || status === 'paused') {
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: 'cancelled',
        message: 'Backend confirmed this turn is paused for input.',
        traceTone: 'success',
        errorMessage: null,
      });
    } else if (status === 'completed') {
      if (!finalMessage) return false;
      clearToolPreparingTimers(state);
      applyDoneEvent(state, {
        status: 'completed',
        message: finalMessage,
      });
    } else if (status === 'cancelled') {
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: 'cancelled',
        message: 'Backend confirmed this turn was cancelled.',
        traceTone: 'success',
        errorMessage: null,
      });
    } else if (status === 'failed' || status === 'timed_out') {
      const message = taskRun.errorMessage?.trim()
        || (status === 'timed_out'
          ? 'Backend confirmed this turn timed out.'
          : 'Backend confirmed this turn failed.');
      clearToolPreparingTimers(state);
      applyTerminalProjection(state, {
        toolStatus: status === 'timed_out' ? 'timedOut' : 'error',
        message,
        traceTone: 'error',
        errorMessage: message,
      });
    } else {
      return false;
    }

    this.finishTurnTiming(state);
    this.touch(conversationId);
    this.evictCompletedStreams(conversationId);
    this.notifyImmediately(conversationId);
    return true;
  }

  private async recoverSuspectedStall(
    conversationId: string,
    generation: number,
  ): Promise<void> {
    let state = this.watchdogStateIsCurrent(conversationId, generation);
    if (!state) return;

    state._watchdogRecoveryAttempt += 1;
    const expectedRunId = state.turnHandle?.runId;
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
      const taskRuns = await withWatchdogRecoveryTimeout(
        getRecoveryTaskRuns(conversationId),
        'Task-run recovery query',
      );
      state = this.watchdogStateIsCurrent(conversationId, generation, expectedRunId);
      if (!state) return;

      const candidates = [...taskRuns].sort((left, right) =>
        Date.parse(right.updatedAt) - Date.parse(left.updatedAt));
      const taskRun = expectedRunId
        ? candidates.find(run => run.id === expectedRunId || run.turnId === state?.turnHandle?.turnId)
        : candidates.find(taskRunIsActive) ?? candidates[0];
      if (!taskRun) {
        if (expectedRunId) state._watchdogMissingRunConfirmations += 1;
        else state._watchdogMissingRunConfirmations = 0;
        if (expectedRunId && state._watchdogMissingRunConfirmations >= 3) {
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

      const [runEvents, taskEvents] = await withWatchdogRecoveryTimeout(
        Promise.all([
          getRecoveryRunEvents(taskRun.id),
          getRecoveryTaskEvents(taskRun.id),
        ]),
        'Durable-event recovery query',
      );
      state = this.watchdogStateIsCurrent(conversationId, generation, expectedRunId);
      if (!state) return;

      state.taskRun = taskRun;
      state.taskEvents = taskEvents.slice(-256);
      const missingRunEvents = [...runEvents]
        .sort((left, right) => left.eventSeq - right.eventSeq);
      for (const runEvent of missingRunEvents) {
        this.dispatch(conversationId, { conversationId, runEvent } as AgentFrontendEvent);
      }

      state = this._streams[conversationId];
      if (!state || !state.isStreaming) return;
      if (taskRunIsActive(taskRun)) {
        this.setWatchdogConnectionState(state, 'recovered');
        appendStatusTraceEvent(
          state,
          'Durable backend state is active; live recovery remains armed.',
          'success',
          'user',
          'recovery',
        );
        this.touch(conversationId);
        this.capLiveCollections(state);
        this.notifyImmediately(conversationId);
        this.resetTimeout(conversationId, true);
        return;
      }
      let finalMessage: ConversationMessage | null = null;
      if (taskRun.status.toLowerCase() === 'completed') {
        const [conversationResult, turns] = await withWatchdogRecoveryTimeout(
          Promise.all([
            getRecoveryConversation(conversationId),
            getRecoveryConversationTurns(conversationId),
          ]),
          'Final-answer recovery query',
        );
        state = this.watchdogStateIsCurrent(conversationId, generation, expectedRunId);
        if (!state) return;
        finalMessage = finalAssistantMessageForTaskRun(taskRun, turns, conversationResult[1]);
      }
      if (this.settleConfirmedTaskRun(conversationId, state, taskRun, finalMessage)) return;

      this.rearmWatchdogRecovery(
        conversationId,
        state,
        taskRun.status.toLowerCase() === 'completed'
          ? 'Backend completed this turn, but the final assistant message is not durable yet; recovery will retry.'
          : `Backend status '${taskRun.status}' is not terminal; recovery will retry.`,
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

    const enqueue = enqueueStreamRunEvent(state, runEvent);
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
    if (state._pendingRunEvents.size === 0) {
      state._runEventGapRecoveryAttempt = 0;
    }
  }

  /** Returns true when the applied event is terminal. */
  private applyOrderedRunEvent(
    conversationId: string,
    state: InternalStreamState,
    runEvent: AgentRunEvent,
  ): boolean {
    const isTerminalEvent = runEvent.kind === 'done' || runEvent.kind === 'error';
    const isAwaitingUserInput = runEvent.kind === 'status'
      && runEvent.phase === 'awaiting_user_input';
    const reopensAwaitingStream = runEvent.kind === 'status'
      && runEvent.phase !== 'awaiting_user_input'
      && ['queued', 'running', 'recovering'].includes(runEvent.status ?? '');
    if (!state.isStreaming && !isTerminalEvent && !reopensAwaitingStream) return false;

    this.markFirstEventTiming(state);
    if (reopensAwaitingStream) {
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
