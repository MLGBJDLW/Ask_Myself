import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';

interface ResizablePanelOptions {
  storageKey: string;
  defaultSize: number;
  minSize: number;
  maxSize: number;
  direction?: 1 | -1;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, Math.round(value)));
}

function readStoredSize(storageKey: string, fallback: number, min: number, max: number) {
  try {
    const raw = localStorage.getItem(storageKey);
    const parsed = raw ? Number.parseInt(raw, 10) : NaN;
    return Number.isFinite(parsed) ? clamp(parsed, min, max) : fallback;
  } catch {
    return fallback;
  }
}

export function useResizablePanel({
  storageKey,
  defaultSize,
  minSize,
  maxSize,
  direction = 1,
}: ResizablePanelOptions) {
  const [size, setSizeState] = useState(() =>
    readStoredSize(storageKey, defaultSize, minSize, maxSize),
  );
  const [isResizing, setIsResizing] = useState(false);
  const sizeRef = useRef(size);

  useEffect(() => {
    sizeRef.current = size;
  }, [size]);

  const setSize = useCallback(
    (nextSize: number) => {
      const next = clamp(nextSize, minSize, maxSize);
      sizeRef.current = next;
      setSizeState(next);
      try {
        localStorage.setItem(storageKey, String(next));
      } catch {
        // Ignore storage failures; resizing should still work for the session.
      }
      return next;
    },
    [maxSize, minSize, storageKey],
  );

  const startResize = useCallback(
    (event: ReactPointerEvent<HTMLElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();

      const startX = event.clientX;
      const startSize = sizeRef.current;
      const previousCursor = document.body.style.cursor;
      const previousUserSelect = document.body.style.userSelect;

      setIsResizing(true);
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';

      const handleMove = (moveEvent: PointerEvent) => {
        setSize(startSize + (moveEvent.clientX - startX) * direction);
      };

      const cleanup = () => {
        window.removeEventListener('pointermove', handleMove);
        window.removeEventListener('pointerup', cleanup);
        window.removeEventListener('pointercancel', cleanup);
        window.removeEventListener('blur', cleanup);
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousUserSelect;
        setIsResizing(false);
      };

      window.addEventListener('pointermove', handleMove);
      window.addEventListener('pointerup', cleanup);
      window.addEventListener('pointercancel', cleanup);
      window.addEventListener('blur', cleanup);
    },
    [direction, setSize],
  );

  return { size, setSize, startResize, isResizing };
}
