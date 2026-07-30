import { useEffect, useRef } from 'react';
import { runKnowledgeHealthCheck } from './api';

const STORAGE_KEY = 'last-health-check-at';
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000; // 24 hours
const POLL_INTERVAL_MS = 60 * 60 * 1000;        // re-check every hour

function isCheckDue(): boolean {
  const last = localStorage.getItem(STORAGE_KEY);
  if (!last) return true;
  const elapsed = Date.now() - Number(last);
  return elapsed >= CHECK_INTERVAL_MS;
}

/**
 * Periodically runs the knowledge health check (every 24 h).
 * Results are stored in localStorage under `health-check-result` so a
 * future notification component can pick them up.
 *
 * Non-blocking: errors are silently swallowed.
 */
export function useAutoHealthCheck(enabled = true): void {
  const runningRef = useRef(false);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let timer: ReturnType<typeof setInterval> | null = null;
    let initialTimer: ReturnType<typeof setTimeout> | null = null;

    const maybeRun = async () => {
      if (cancelled || runningRef.current || !isCheckDue()) return;
      runningRef.current = true;
      try {
        const report = await runKnowledgeHealthCheck();
        if (cancelled) return;
        localStorage.setItem(STORAGE_KEY, String(Date.now()));
        localStorage.setItem('health-check-result', JSON.stringify(report));
      } catch {
        // non-blocking — swallow
        console.debug('[auto-health-check] skipped or failed');
      } finally {
        runningRef.current = false;
      }
    };

    // Let first paint and wizard routing finish before competing for the DB.
    initialTimer = setTimeout(() => void maybeRun(), 2_000);

    // Poll every hour to see if 24 h have elapsed
    timer = setInterval(() => void maybeRun(), POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      if (initialTimer !== null) clearTimeout(initialTimer);
      if (timer !== null) clearInterval(timer);
    };
  }, [enabled]);
}
