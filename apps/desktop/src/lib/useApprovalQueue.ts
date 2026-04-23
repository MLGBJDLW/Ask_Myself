import { useCallback } from 'react';
import { useAgentStream } from './useAgentStream';
import type { ApprovalRequest, ApprovalDecisionValue } from '../types';

/**
 * Manage the pending-approval FIFO queue for a conversation.
 *
 * The queue lives in `streamStore` (populated by `approvalRequested`
 * events). This hook surfaces the head plus a no-op `onResolved` — the
 * backend emits `approvalResolved`, which removes the request from the
 * queue via the normal event dispatch path.
 */
export function useApprovalQueue(conversationId: string) {
  const { pendingApprovals } = useAgentStream(conversationId);
  const current: ApprovalRequest | null = pendingApprovals[0] ?? null;

  const onResolved = useCallback(
    (_request: ApprovalRequest, _decision: ApprovalDecisionValue) => {
      // Resolution is driven by backend `approvalResolved` events.
    },
    [],
  );

  return { current, queue: pendingApprovals, onResolved };
}

export default useApprovalQueue;
