import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { compileAfterScan } from './api';

const DEBOUNCE_MS = 5_000;

/**
 * Listens for Tauri `file-changed` events and triggers a debounced
 * knowledge-graph compilation so entities stay up-to-date automatically.
 *
 * Non-blocking: errors are logged but never surface to the UI.
 */
export function useAutoCompile(): void {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<{ sourceId: string; filesAdded: number; filesUpdated: number }>(
      'file-changed',
      () => {
        if (cancelled) return;

        // Reset the debounce timer on every event
        if (timerRef.current !== null) {
          clearTimeout(timerRef.current);
        }

        timerRef.current = setTimeout(() => {
          if (cancelled) return;
          timerRef.current = null;
          compileAfterScan().catch((err) => {
            // Expected to fail when no AI provider is configured — swallow silently
            console.debug('[auto-compile] skipped or failed:', err);
          });
        }, DEBOUNCE_MS);
      },
    ).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, []);
}
