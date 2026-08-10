import type { AgentRunEvent, AgentTurnState } from '../../types/conversation';
import { clearToolPreparingTimers, type InternalStreamState } from './state';
import { clearStreamWatchdog } from './watchdog';

export type AgentRunEventLifecycle = 'progress' | 'resume' | 'suspension' | 'terminal';

/**
 * Keep lifecycle interpretation at the protocol seam so live delivery and
 * durable replay agree about which events actually close a run.
 */
export function classifyAgentRunEventLifecycle(
  event: AgentRunEvent,
): AgentRunEventLifecycle {
  const isSuspensionStatus = event.kind === 'status'
    && (
      event.phase === 'awaiting_user_input'
      || event.phase === 'paused'
      || event.status === 'awaiting_user_input'
      || event.status === 'paused'
    );
  const isLegacyPausedDone = event.kind === 'done' && event.status === 'paused';
  if (isSuspensionStatus || isLegacyPausedDone) return 'suspension';

  if (event.kind === 'done' || event.kind === 'error') return 'terminal';

  if (
    event.kind === 'status'
    && ['queued', 'running', 'recovering'].includes(event.status ?? '')
  ) {
    return 'resume';
  }

  return 'progress';
}

export function agentTurnStateSuspendsStream(state: AgentTurnState): boolean {
  return state === 'awaitingUserInput' || state === 'paused';
}

export function suspendAgentRunProjection(state: InternalStreamState): void {
  clearStreamWatchdog(state);
  clearToolPreparingTimers(state);
  state.isStreaming = false;
  state.isThinking = false;
}
