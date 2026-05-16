import type { AgentTaskRunEvent } from '../../types/conversation';
import { applyStreamBlockDelta } from './blockProjection';
import {
  isDurableStreamEvent,
  replayItemFromTaskEvent,
  type ReplayStreamItem,
} from './legacyAdapter';
import {
  appendStatusTraceEvent,
  applyStreamResetProjection,
  applyTerminalProjection,
  type StreamTerminalProjectionState,
} from './terminalProjection';
import { isTaskTimelineEvent } from './taskTimeline';

export interface DurableReplayProjectionState extends StreamTerminalProjectionState {
  _lastEventSeq: number;
}

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

export function applyDurableReplayToState(
  state: DurableReplayProjectionState,
  events: AgentTaskRunEvent[],
): void {
  for (const item of durableReplayItemsFromTaskEvents(events)) {
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
      applyStreamResetProjection(state, reason);
      continue;
    }

    if (item.eventType === 'terminal') {
      const isError = item.payload.kind === 'error';
      const message = typeof item.payload.message === 'string'
        ? item.payload.message
        : (isError ? 'Request failed' : 'Task completed');
      applyTerminalProjection(state, {
        toolStatus: isError ? 'error' : 'done',
        message,
        traceTone: isError ? 'error' : 'success',
        errorMessage: isError ? message : undefined,
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
