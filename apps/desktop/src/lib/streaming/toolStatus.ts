import type {
  ToolRunItem,
  ToolRunStatus,
} from '../../types/conversation';
import type { ToolCallEvent } from './protocol';

export type TerminalToolStatus = 'done' | 'error' | 'cancelled' | 'timedOut';

export function isPendingToolCallStatus(status: ToolCallEvent['status']): boolean {
  return status === 'running'
    || status === 'starting'
    || status === 'preparing'
    || status === 'approvalPending';
}

export function isUnsuccessfulToolCallStatus(status?: string | null): boolean {
  return status === 'error'
    || status === 'failed'
    || status === 'declined'
    || status === 'cancelled'
    || status === 'timedOut'
    || status === 'timed_out';
}

export function toolRunStatusToToolCallStatus(status: ToolRunStatus): ToolCallEvent['status'] {
  switch (status) {
    case 'preparing':
      return 'preparing';
    case 'approvalPending':
      return 'approvalPending';
    case 'running':
      return 'running';
    case 'completed':
      return 'done';
    case 'failed':
      return 'error';
    case 'declined':
      return 'declined';
    case 'cancelled':
      return 'cancelled';
    case 'timedOut':
      return 'timedOut';
    default:
      return 'running';
  }
}

export function argsStatusForToolRun(
  run: Pick<ToolRunItem, 'arguments'>,
  status: ToolCallEvent['status'],
): ToolCallEvent['argsStatus'] {
  if (status === 'preparing') return run.arguments ? 'streaming' : 'pending';
  if (status === 'error' || status === 'timedOut') return 'error';
  if (status === 'done' || status === 'declined' || status === 'cancelled') return 'done';
  return run.arguments ? 'ready' : 'pending';
}

export function normalizePersistedToolCallStatus(
  status: unknown,
  isError?: boolean,
): ToolCallEvent['status'] {
  if (isError) return 'error';
  switch (status) {
    case 'preparing':
      return 'preparing';
    case 'starting':
      return 'starting';
    case 'approvalPending':
    case 'approval_pending':
      return 'approvalPending';
    case 'running':
      return 'running';
    case 'done':
    case 'completed':
      return 'done';
    case 'error':
    case 'failed':
      return 'error';
    case 'declined':
      return 'declined';
    case 'cancelled':
      return 'cancelled';
    case 'timedOut':
    case 'timed_out':
      return 'timedOut';
    default:
      return 'done';
  }
}

export function defaultArgsStatusForToolCall(
  status: ToolCallEvent['status'],
  argumentsText: string,
): ToolCallEvent['argsStatus'] {
  if (status === 'error' || status === 'timedOut') return 'error';
  if (status === 'done' || status === 'declined' || status === 'cancelled') return 'done';
  if (status === 'preparing' && !argumentsText) return 'pending';
  return argumentsText ? 'ready' : 'pending';
}
