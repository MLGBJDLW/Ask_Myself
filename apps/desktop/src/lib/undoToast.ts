import { toast } from 'sonner';

interface UndoToastOptions {
  message: string;
  undoLabel: string;
  onConfirm: () => void | Promise<void>;
  duration?: number;
}

export function undoableAction({ message, undoLabel, onConfirm, duration = 5000 }: UndoToastOptions) {
  let cancelled = false;

  const toastId = toast(message, {
    duration,
    action: {
      label: undoLabel,
      onClick: () => { cancelled = true; },
    },
    onDismiss: () => {
      if (!cancelled) void Promise.resolve(onConfirm());
    },
    onAutoClose: () => {
      if (!cancelled) void Promise.resolve(onConfirm());
    },
  });

  return toastId;
}
