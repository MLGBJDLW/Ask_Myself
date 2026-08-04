import { useEffect, useMemo, useState } from 'react';
import type { TurnTiming } from './streaming/protocol';

export function formatElapsedDuration(elapsedMs: number): string {
  const seconds = Math.max(0, Math.floor(elapsedMs / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m${String(seconds % 60).padStart(2, '0')}s`;
}

export function useElapsedTime(
  timing: TurnTiming | null | undefined,
  live: boolean,
  minimumVisibleMs = 0,
): string | null {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!timing || !live || timing.finishedAtEpochMs != null) return undefined;
    let interval: number | null = null;

    const stop = () => {
      if (interval != null) window.clearInterval(interval);
      interval = null;
    };
    const start = () => {
      stop();
      setNow(Date.now());
      if (document.visibilityState === 'visible') {
        interval = window.setInterval(() => setNow(Date.now()), 1000);
      }
    };
    const handleVisibility = () => {
      if (document.visibilityState === 'visible') start();
      else stop();
    };

    start();
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      stop();
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, [live, timing]);

  return useMemo(() => {
    if (!timing) return null;
    const end = timing.finishedAtEpochMs ?? now;
    const elapsedMs = Math.max(0, end - timing.startedAtEpochMs);
    if (elapsedMs < minimumVisibleMs) return null;
    return formatElapsedDuration(elapsedMs);
  }, [minimumVisibleMs, now, timing]);
}

export function formatTimingLatency(startedAt: number, reachedAt?: number | null): string | null {
  if (reachedAt == null || reachedAt < startedAt) return null;
  return formatElapsedDuration(reachedAt - startedAt);
}
