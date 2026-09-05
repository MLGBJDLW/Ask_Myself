import { useEffect, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { streamStore } from './streamStore';
import { parseAgentFrontendEvent } from './streaming/runEventWire';
import type { AgentHeartbeatEvent, AgentTaskSnapshotEvent } from '../types/conversation';

/**
 * Global Tauri event listener for agent streaming events.
 * Mount once at app root — never tears down, so streams survive page navigation.
 */
export function StreamProvider({ children }: { children: ReactNode }) {
  useEffect(() => {
    let unlisten: Array<() => void> = [];
    let cancelled = false;

    Promise.all([
      listen<unknown>('agent://run-event', (event) => {
        const data = parseAgentFrontendEvent(event.payload);
        if (!data) {
          console.error('[StreamProvider] rejected invalid Run Event payload');
          return;
        }
        try {
          streamStore.dispatch(data.conversationId, data);
        } catch (err) {
          console.error('[StreamProvider] dispatch error:', err);
        }
      }),
      listen<AgentTaskSnapshotEvent>('agent://task-snapshot', (event) => {
        if (event.payload?.type !== 'taskRunUpdated') return;
        streamStore.applyTaskSnapshot(event.payload);
      }),
      listen<AgentHeartbeatEvent>('agent://heartbeat', (event) => {
        const heartbeat = event.payload;
        if (
          !heartbeat
          || typeof heartbeat.conversationId !== 'string'
          || typeof heartbeat.runId !== 'string'
          || typeof heartbeat.turnId !== 'string'
        ) return;
        streamStore.recordHeartbeat(heartbeat.conversationId, heartbeat.runId, heartbeat.durableHighWater);
      }),
    ]).then((callbacks) => {
      if (cancelled) {
        callbacks.forEach(callback => callback());
      } else {
        unlisten = callbacks;
      }
    });

    return () => {
      cancelled = true;
      unlisten.forEach(callback => callback());
    };
  }, []);

  return <>{children}</>;
}
