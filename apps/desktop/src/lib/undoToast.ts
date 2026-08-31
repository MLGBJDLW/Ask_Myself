import { toast } from 'sonner';
import { createUndoActionGate } from './undoActionGate';

interface UndoToastOptions {
  message: string;
  undoLabel: string;
  onConfirm: () => void | Promise<void>;
  onUndo?: () => void | Promise<void>;
  duration?: number;
}

export function undoableAction({ message, undoLabel, onConfirm, onUndo, duration = 5000 }: UndoToastOptions) {
  const gate = createUndoActionGate(onConfirm, onUndo);

  const toastId = toast(message, {
    duration,
    action: {
      label: undoLabel,
      onClick: () => {
        void gate.undo();
      },
    },
    onDismiss: () => {
      void gate.confirm();
    },
    onAutoClose: () => {
      void gate.confirm();
    },
  });

  return toastId;
}
