export interface UndoActionGate {
  confirm(): Promise<void>;
  undo(): Promise<void>;
}

/**
 * Sonner may report an auto-close through both onAutoClose and onDismiss.
 * Settle the optimistic action exactly once so a destructive IPC cannot run
 * twice and a late callback cannot undo an already committed operation.
 */
export function createUndoActionGate(
  onConfirm: () => void | Promise<void>,
  onUndo?: () => void | Promise<void>,
): UndoActionGate {
  let settled = false;

  const settle = (action?: () => void | Promise<void>) => {
    if (settled) return Promise.resolve();
    settled = true;
    return Promise.resolve().then(action);
  };

  return {
    confirm: () => settle(onConfirm),
    undo: () => settle(onUndo),
  };
}
