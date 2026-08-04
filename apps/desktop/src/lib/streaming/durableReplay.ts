import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../../types/conversation';
import { applyAgentRunEvent } from './runEventReducer';
import {
  createDefaultState,
  taskRunIsActive,
  type InternalStreamState,
} from './state';
import { isTaskTimelineEvent } from './taskTimeline';
import { applyTerminalProjection } from './terminalProjection';
import { createToolCall, insertPendingToolCall, type ToolPreparingPayload } from './toolProjection';

export type DurableReplayProjectionState = InternalStreamState;

export function taskTimelineEventsFromReplaySource(events: AgentTaskRunEvent[]): AgentTaskRunEvent[] {
  return events.filter(isTaskTimelineEvent).slice(-50);
}

export function applyDurableRunEventsToState(
  state: DurableReplayProjectionState,
  events: AgentRunEvent[],
): void {
  const ordered = [...events].sort((a, b) => a.eventSeq - b.eventSeq);
  for (const event of ordered) {
    if (event.eventSeq <= state._lastEventSeq) continue;
    state._lastEventSeq = event.eventSeq;
    applyAgentRunEvent(state, event, {
      scheduleToolPreparing: payload => applyToolPreparingReplay(state, payload),
    });
  }
}

export function projectRunEventsToStreamState(
  taskRun: AgentTaskRun,
  runEvents: AgentRunEvent[],
  taskEvents: AgentTaskRunEvent[] = [],
  options: { interruptActive?: boolean } = {},
): DurableReplayProjectionState {
  const state = createDefaultState();
  state.isStreaming = taskRunIsActive(taskRun);
  state.taskRun = taskRun;
  const startedAt = Date.parse(taskRun.startedAt ?? taskRun.createdAt);
  const finishedAt = taskRun.finishedAt ? Date.parse(taskRun.finishedAt) : null;
  if (Number.isFinite(startedAt)) {
    state.turnTiming = {
      startedAtEpochMs: startedAt,
      firstEventAtEpochMs: null,
      firstVisibleOutputAtEpochMs: null,
      finishedAtEpochMs: finishedAt != null && Number.isFinite(finishedAt) ? finishedAt : null,
    };
  }
  state.taskEvents = taskTimelineEventsFromReplaySource(taskEvents);
  applyDurableRunEventsToState(state, runEvents);

  if (options.interruptActive && taskRunIsActive(taskRun) && state.isStreaming) {
    applyTerminalProjection(state, {
      toolStatus: 'cancelled',
      message: 'Previous run interrupted when the app closed.',
      toolFallbackMessage: 'Interrupted',
      traceTone: 'error',
      errorMessage: null,
    });
    state.taskRun = {
      ...taskRun,
      status: 'cancelled',
      phase: 'done',
      summary: taskRun.summary ?? 'Interrupted because the app was closed.',
      finishedAt: taskRun.finishedAt ?? taskRun.updatedAt,
    };
  }

  return state;
}

function applyToolPreparingReplay(
  state: DurableReplayProjectionState,
  payload: ToolPreparingPayload,
): void {
  if (state.toolCalls.some(toolCall => toolCall.callId === payload.callId)) return;

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
