import { useEffect, useState } from 'react';
import { getConversationFileChanges, type TurnFileChangeSummary } from './api';

const inFlight = new Map<string, Promise<TurnFileChangeSummary[]>>();
function query(conversationId: string) {
  const pending = inFlight.get(conversationId);
  if (pending) return pending;
  const request = getConversationFileChanges(conversationId);
  inFlight.set(conversationId, request);
  const retire = () => { if (inFlight.get(conversationId) === request) inFlight.delete(conversationId); };
  void request.then(retire, retire);
  return request;
}

export function useConversationFileChanges(conversationId: string | null | undefined, active: boolean, completedTools: string) {
  const [state, setState] = useState<{ conversationId: string; summaries: Map<string, TurnFileChangeSummary> } | null>(null);
  useEffect(() => {
    if (!conversationId) return;
    let disposed = false;
    let pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const values = await query(conversationId);
        if (!disposed && Array.isArray(values)) setState(previous => {
          const old = previous?.conversationId === conversationId ? previous.summaries : new Map<string, TurnFileChangeSummary>();
          const next = new Map(values.map(summary => [summary.turnId, summary]));
          if (old.size === next.size && values.every(value => old.get(value.turnId)?.revision === value.revision)) return previous;
          return { conversationId, summaries: next };
        });
      } catch (error) { console.warn('Could not refresh recorded file changes', error); }
      finally { pending = false; }
    };
    void refresh();
    // Child workers can commit files while their parent tool is still running.
    // Only small summaries are queried; details stay in the native database.
    const timer = active ? window.setInterval(() => void refresh(), 2000) : null;
    return () => { disposed = true; if (timer !== null) window.clearInterval(timer); };
  }, [conversationId, active, completedTools]);
  return state && state.conversationId === conversationId ? state.summaries : new Map<string, TurnFileChangeSummary>();
}
