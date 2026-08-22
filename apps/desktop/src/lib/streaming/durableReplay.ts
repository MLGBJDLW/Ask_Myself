import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../../types/conversation';
import { applyAgentRunEvent } from './runEventReducer';
import {
  createDefaultState,
  type InternalStreamState,
} from './state';
import { taskRunIsActive, taskRunIsSuspended } from './runReconciliation';
import { isTaskTimelineEvent } from './taskTimeline';
import { applyTerminalProjection } from './terminalProjection';
import { createToolCall, insertPendingToolCall, type ToolPreparingPayload } from './toolProjection';
import {
  alignAuthoritativeReplayCursor,
  enqueueStreamRunEvent,
  takeNextStreamRunEvent,
} from './ordering';
import { classifyAgentRunEventLifecycle } from './runEventLifecycle';
import type { TurnTiming } from './protocol';

export type DurableReplayProjectionState = InternalStreamState;

function parsedEpochMs(value?: string | null): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function durableSuspensionEpochMs(
  taskRun: AgentTaskRun,
  runEvents: AgentRunEvent[],
): number | null {
  if (!taskRunIsSuspended(taskRun)) return null;

  let suspensionAt: number | null = null;
  for (const event of [...runEvents].sort((left, right) => left.eventSeq - right.eventSeq)) {
    const lifecycle = classifyAgentRunEventLifecycle(event);
    if (lifecycle === 'suspension') {
      suspensionAt = parsedEpochMs(event.createdAt) ?? suspensionAt;
    } else if (lifecycle === 'resume') {
      suspensionAt = null;
    }
  }
  return suspensionAt ?? parsedEpochMs(taskRun.updatedAt);
}

export function turnTimingFromTaskRun(
  taskRun: AgentTaskRun,
  runEvents: AgentRunEvent[] = [],
): TurnTiming | null {
  const startedAt = parsedEpochMs(taskRun.startedAt ?? taskRun.createdAt);
  if (startedAt == null) return null;

  const explicitFinishedAt = parsedEpochMs(taskRun.finishedAt);
  const suspensionAt = durableSuspensionEpochMs(taskRun, runEvents);
  const terminalFallback = !taskRunIsActive(taskRun) && !taskRunIsSuspended(taskRun)
    ? parsedEpochMs(taskRun.updatedAt)
    : null;
  return {
    startedAtEpochMs: startedAt,
    startedAtMonotonicMs: null,
    firstEventAtEpochMs: null,
    firstVisibleOutputAtEpochMs: null,
    finishedAtEpochMs: explicitFinishedAt ?? suspensionAt ?? terminalFallback,
    finishedAtMonotonicMs: null,
  };
}

export function taskTimelineEventsFromReplaySource(events: AgentTaskRunEvent[]): AgentTaskRunEvent[] {
  return events.filter(isTaskTimelineEvent).slice(-50);
}

export function applyDurableRunEventsToState(
  state: DurableReplayProjectionState,
  events: AgentRunEvent[],
): void {
  const ordered = [...events].sort((a, b) => a.eventSeq - b.eventSeq);
  for (const event of ordered) {
    alignAuthoritativeReplayCursor(state, event);
    enqueueStreamRunEvent(state, event);
    let ready: AgentRunEvent | null;
    while ((ready = takeNextStreamRunEvent(state)) !== null) {
      applyAgentRunEvent(state, ready, {
        scheduleToolPreparing: payload => applyToolPreparingReplay(state, payload),
      });
      if (classifyAgentRunEventLifecycle(ready) === 'terminal') {
        state._pendingRunEvents.clear();
        return;
      }
    }
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
  state.turnTiming = turnTimingFromTaskRun(taskRun, runEvents);
  state.taskEvents = taskTimelineEventsFromReplaySource(taskEvents);
  applyDurableRunEventsToState(state, runEvents);

  if (options.interruptActive === true && taskRunIsActive(taskRun) && state.isStreaming) {
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
