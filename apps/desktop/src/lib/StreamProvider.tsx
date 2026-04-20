import { useEffect, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { streamStore } from './streamStore';
import type { AgentFrontendEvent } from '../types';

/**
 * Global Tauri event listener for agent streaming events.
 * Mount once at app root — never tears down, so streams survive page navigation.
 */
export function StreamProvider({ children }: { children: ReactNode }) {
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    listen<AgentFrontendEvent>('agent:event', (event) => {
      const data = event.payload;
      if (!data?.conversationId) return;
      try {
        streamStore.dispatch(data.conversationId, data);
      } catch (err) {
        console.error('[StreamProvider] dispatch error:', err);
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return <>{children}</>;
}
