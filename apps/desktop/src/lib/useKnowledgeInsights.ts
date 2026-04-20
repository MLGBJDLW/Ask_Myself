import { useEffect, useRef, useState } from 'react';
import { getKnowledgeGaps, suggestExplorations } from './api';
import type { KnowledgeGap } from './api';

const STORAGE_KEY = 'last-insights-at';
const INTERVAL_MS = 12 * 60 * 60 * 1000; // 12 hours

function isDue(): boolean {
  const last = localStorage.getItem(STORAGE_KEY);
  if (!last) return true;
  return Date.now() - Number(last) >= INTERVAL_MS;
}

export interface KnowledgeInsights {
  gaps: KnowledgeGap[];
  explorations: string[];
  lastUpdated: Date | null;
}

/**
 * Periodically fetches knowledge gaps and exploration suggestions (every 12 h).
 * Exposes the latest results via returned state for optional UI consumption.
 *
 * Non-blocking: errors are silently swallowed.
 */
export function useKnowledgeInsights(): KnowledgeInsights {
  const [gaps, setGaps] = useState<KnowledgeGap[]>([]);
  const [explorations, setExplorations] = useState<string[]>([]);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const runningRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    const fetchInsights = async () => {
      if (cancelled || runningRef.current || !isDue()) return;
      runningRef.current = true;
      try {
        const [gapData, explorationData] = await Promise.all([
          getKnowledgeGaps(),
          suggestExplorations(),
        ]);
        if (cancelled) return;
        setGaps(gapData);
        setExplorations(explorationData);
        const now = new Date();
        setLastUpdated(now);
        localStorage.setItem(STORAGE_KEY, String(now.getTime()));
      } catch {
        console.debug('[knowledge-insights] fetch failed');
      } finally {
        runningRef.current = false;
      }
    };

    void fetchInsights();

    // Re-check hourly whether 12 h have elapsed
    const timer = setInterval(() => void fetchInsights(), 60 * 60 * 1000);

    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  return { gaps, explorations, lastUpdated };
}
