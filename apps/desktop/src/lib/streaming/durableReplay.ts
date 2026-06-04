import type {
  AgentFrontendEvent,
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../../types/conversation';
import { applyStreamBlockDelta } from './blockProjection';
import { normalizeAgentEventType } from './eventTypes';
import {
  isDurableStreamEvent,
  replayItemFromRunEvent,
  replayItemFromTaskEvent,
  type ReplayStreamItem,
} from './legacyAdapter';
import { applyLiveStreamEvent } from './liveEventReducer';
import { applyDoneEvent } from './liveProjection';
import {
  appendStatusTraceEvent,
  applyStreamResetProjection,
  applyTerminalProjection,
} from './terminalProjection';
import {
  createDefaultState,
  taskRunIsActive,
  type InternalStreamState,
} from './state';
import { isTaskTimelineEvent } from './taskTimeline';
import { createToolCall, insertPendingToolCall, type ToolPreparingPayload } from './toolProjection';

export type DurableReplayProjectionState = InternalStreamState;

export function taskTimelineEventsFromReplaySource(events: AgentTaskRunEvent[]): AgentTaskRunEvent[] {
  return events
    .filter(event => isTaskTimelineEvent(event) || !isDurableStreamEvent(event))
    .slice(-50);
}

export function durableReplayItemsFromTaskEvents(events: AgentTaskRunEvent[]): ReplayStreamItem[] {
  return events
    .map(replayItemFromTaskEvent)
    .filter((item): item is ReplayStreamItem => Boolean(item))
    .sort((a, b) => a.eventSeq - b.eventSeq);
}

export function durableReplayItemsFromRunEvents(events: AgentRunEvent[]): ReplayStreamItem[] {
  return events
    .map(replayItemFromRunEvent)
    .filter((item): item is ReplayStreamItem => Boolean(item))
    .sort((a, b) => a.eventSeq - b.eventSeq);
}

function applyDurableReplayItemsToState(
  state: DurableReplayProjectionState,
  items: ReplayStreamItem[],
): void {
  for (const item of items) {
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
      applyStreamResetProjection(state, reason, { clearTools: true });
      continue;
    }

    if (item.eventType === 'terminal') {
      const isError = item.payload.kind === 'error';
      const status = typeof item.payload.status === 'string' ? item.payload.status : null;
      const message = typeof item.payload.message === 'string'
        ? item.payload.message
        : (isError ? 'Request failed' : 'Task completed');
      if (!isError) {
        const rawDone = {
          conversationId: '',
          type: 'done',
          eventSeq: item.eventSeq,
          status,
          message,
          usageTotal: item.payload.usageTotal,
          lastPromptTokens: item.payload.lastPromptTokens,
          contextBreakdown: item.payload.contextBreakdown,
          cached: item.payload.cached,
          finishReason: item.payload.finishReason,
        } as AgentFrontendEvent & Record<string, unknown>;
        applyDoneEvent(state, rawDone, rawDone);
        continue;
      }
      applyTerminalProjection(state, {
        toolStatus: status === 'cancelled'
          ? 'cancelled'
          : status === 'timed_out'
            ? 'timedOut'
            : isError ? 'error' : 'done',
        message,
        toolFallbackMessage: status === 'cancelled'
          ? 'Cancelled'
          : status === 'timed_out'
            ? 'Timed out'
            : undefined,
        traceTone: isError ? 'error' : 'success',
        errorMessage: status === 'cancelled' ? null : (isError ? message : undefined),
      });
      continue;
    }

    if (item.eventType === 'frontend' && item.frontendEvent) {
      const eventType = normalizeAgentEventType(item.frontendEvent.type);
      if (!eventType) continue;
      const raw = item.frontendEvent as typeof item.frontendEvent & Record<string, unknown>;
      applyLiveStreamEvent(state, eventType, item.frontendEvent, raw, {
        scheduleToolPreparing: payload => applyToolPreparingReplay(state, payload),
      });
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
}

export function applyDurableReplayToState(
  state: DurableReplayProjectionState,
  events: AgentTaskRunEvent[],
): void {
  applyDurableReplayItemsToState(state, durableReplayItemsFromTaskEvents(events));
}

export function applyDurableRunEventsToState(
  state: DurableReplayProjectionState,
  events: AgentRunEvent[],
): void {
  applyDurableReplayItemsToState(state, durableReplayItemsFromRunEvents(events));
}

export function projectTaskEventsToStreamState(
  taskRun: AgentTaskRun,
  taskEvents: AgentTaskRunEvent[],
): DurableReplayProjectionState {
  const state = createDefaultState();
  state.isStreaming = taskRunIsActive(taskRun);
  state.taskRun = taskRun;
  state.taskEvents = taskTimelineEventsFromReplaySource(taskEvents);
  applyDurableReplayToState(state, taskEvents);
  return state;
}

export function projectRunEventsToStreamState(
  taskRun: AgentTaskRun,
  runEvents: AgentRunEvent[],
  taskEvents: AgentTaskRunEvent[] = [],
): DurableReplayProjectionState {
  const state = createDefaultState();
  state.isStreaming = taskRunIsActive(taskRun);
  state.taskRun = taskRun;
  state.taskEvents = taskTimelineEventsFromReplaySource(taskEvents);
  applyDurableRunEventsToState(state, runEvents);
  return state;
}

export function projectHistoricalEventsToStreamState(
  taskRun: AgentTaskRun,
  taskEvents: AgentTaskRunEvent[],
  runEvents: AgentRunEvent[],
): DurableReplayProjectionState {
  if (runEvents.length > 0) {
    return projectRunEventsToStreamState(taskRun, runEvents, taskEvents);
  }

  return projectTaskEventsToStreamState(taskRun, taskEvents);
}

function applyToolPreparingReplay(
  state: DurableReplayProjectionState,
  payload: ToolPreparingPayload,
): void {
  if (state.toolCalls.some(tc => tc.callId === payload.callId)) {
    return;
  }

  const roundThinking = state.thinkingText.trim() ? state.thinkingText : '';
  if (roundThinking) state.thinkingText = '';
  state.isThinking = false;

  const preparingCall = createToolCall({
    callId: payload.callId,
    toolName: payload.toolName,
    status: 'preparing',
    argsStatus: 'pending',
  });
  preparingCall.argsBytes = Math.max(0, payload.argsBytes);
  insertPendingToolCall(state, preparingCall, roundThinking);
}
