import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../../types/conversation';
import { applyAgentRunEvent } from './runEventReducer';
import {
  createDefaultState,
  capStreamCollections,
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
): boolean {
  const ordered = [...events].sort((a, b) => a.eventSeq - b.eventSeq);
  for (const event of ordered) {
    capStreamCollections(state);
    alignAuthoritativeReplayCursor(state, event);
    enqueueStreamRunEvent(state, event);
    if (drainReplayEvents(state)) return true;
  }
  return false;
}

function drainReplayEvents(state: DurableReplayProjectionState): boolean {
    let ready: AgentRunEvent | null;
    while ((ready = takeNextStreamRunEvent(state)) !== null) {
      applyAgentRunEvent(state, ready, {
        scheduleToolPreparing: payload => applyToolPreparingReplay(state, payload),
      });
      if (classifyAgentRunEventLifecycle(ready) === 'terminal') {
        state._pendingRunEvents.clear();
        return true;
      }
    }
  return false;
}

/** Buffered live events do not authorize skipping missing sequence numbers. */
export function applyBufferedRunEventsToState(state: DurableReplayProjectionState, events: AgentRunEvent[]): void {
  for (const event of events) enqueueStreamRunEvent(state, event);
  drainReplayEvents(state);
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

  finishReplayProjection(state, taskRun, options.interruptActive === true);

  return state;
}

function finishReplayProjection(state: DurableReplayProjectionState, taskRun: AgentTaskRun, interruptActive: boolean): void {
  if (interruptActive && taskRunIsActive(taskRun) && state.isStreaming) {
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

/** Bounded batches keep navigation and live delivery responsive during hydration. */
export async function projectRunEventsToStreamStateAsync(
  taskRun: AgentTaskRun,
  runEvents: AgentRunEvent[],
  taskEvents: AgentTaskRunEvent[],
  isCurrent: () => boolean,
): Promise<DurableReplayProjectionState | null> {
  const state = projectRunEventsToStreamState(taskRun, [], taskEvents);
  state.turnTiming = turnTimingFromTaskRun(taskRun, runEvents);
  const ordered = [...runEvents].sort((a, b) => a.eventSeq - b.eventSeq);
  for (let index = 0; index < ordered.length; index += 256) {
    if (!isCurrent()) return null;
    const terminal = applyDurableRunEventsToState(state, ordered.slice(index, index + 256));
    capStreamCollections(state);
    if (terminal) break;
    if (index + 256 < ordered.length) await new Promise<void>(resolve => {
      // A posted task yields without nested timer clamping on long histories.
      const channel = new MessageChannel();
      channel.port1.onmessage = () => {
        channel.port1.close();
        channel.port2.close();
        resolve();
      };
      channel.port2.postMessage(null);
    });
  }
  finishReplayProjection(state, taskRun, taskRun.status === 'cancelling');
  return isCurrent() ? state : null;
}
