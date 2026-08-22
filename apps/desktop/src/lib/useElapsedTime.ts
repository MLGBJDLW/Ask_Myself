import { useEffect, useMemo, useState } from 'react';
import type { TurnTiming } from './streaming/protocol';

export interface ClockSample {
  epochMs: number;
  monotonicMs: number;
}

function readClockSample(): ClockSample {
  return {
    epochMs: Date.now(),
    monotonicMs: globalThis.performance?.now() ?? Date.now(),
  };
}

export function resolveElapsedDurationMs(
  timing: TurnTiming,
  live: boolean,
  now: ClockSample,
): number {
  const monotonicEnd = timing.finishedAtMonotonicMs
    ?? (live && timing.finishedAtEpochMs == null ? now.monotonicMs : null);
  return timing.startedAtMonotonicMs != null && monotonicEnd != null
    ? Math.max(0, monotonicEnd - timing.startedAtMonotonicMs)
    : Math.max(0, (timing.finishedAtEpochMs ?? now.epochMs) - timing.startedAtEpochMs);
}

export function formatElapsedDuration(elapsedMs: number): string {
  const seconds = Math.max(0, Math.floor(elapsedMs / 1000));
  const secondsPart = String(seconds % 60).padStart(2, '0');
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}:${secondsPart}`;
  return `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, '0')}:${secondsPart}`;
}

export function formatThinkingDuration(elapsedMs: number): string {
  const totalSeconds = Math.max(0, Math.floor(elapsedMs / 1000));
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const totalHours = Math.floor(totalMinutes / 60);
  const hours = totalHours % 24;
  const days = Math.floor(totalHours / 24);
  if (days > 0) {
    return `${days}d ${String(hours).padStart(2, '0')}h ${String(minutes).padStart(2, '0')}m`;
  }
  if (totalHours > 0) {
    return `${totalHours}h ${String(minutes).padStart(2, '0')}m ${String(seconds).padStart(2, '0')}s`;
  }
  if (totalMinutes > 0) {
    return `${totalMinutes}m ${String(seconds).padStart(2, '0')}s`;
  }
  return `${seconds}s`;
}

export function useElapsedTime(
  timing: TurnTiming | null | undefined,
  live: boolean,
  minimumVisibleMs = 0,
  formatter: (elapsedMs: number) => string = formatElapsedDuration,
): string | null {
  const [now, setNow] = useState(readClockSample);

  useEffect(() => {
    if (!timing || !live || timing.finishedAtEpochMs != null) return undefined;
    let interval: number | null = null;

    const stop = () => {
      if (interval != null) window.clearInterval(interval);
      interval = null;
    };
    const start = () => {
      stop();
      setNow(readClockSample());
      if (document.visibilityState === 'visible') {
        interval = window.setInterval(() => setNow(readClockSample()), 1000);
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
    const elapsedMs = resolveElapsedDurationMs(timing, live, now);
    if (elapsedMs < minimumVisibleMs) return null;
    return formatter(elapsedMs);
  }, [formatter, live, minimumVisibleMs, now, timing]);
}

export function formatTimingLatency(startedAt: number, reachedAt?: number | null): string | null {
  if (reachedAt == null || reachedAt < startedAt) return null;
  return formatElapsedDuration(reachedAt - startedAt);
}
