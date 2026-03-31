import { useRef, useEffect, useState } from 'react';

/**
 * Gradually reveals `sourceText` with a typewriter effect, advancing by
 * word-boundaries so markdown tokens are never split mid-tag.
 *
 * While `isActive` is true the hook ticks forward; once streaming ends
 * (`isActive` → false) the full text is revealed instantly.
 */
export function useTypewriter(
  sourceText: string,
  isActive: boolean,
  options?: { charsPerTick?: number; intervalMs?: number; instantThreshold?: number },
): string {
  const charsPerTick = options?.charsPerTick ?? 5;
  const intervalMs = options?.intervalMs ?? 30;
  const instantThreshold = options?.instantThreshold ?? 160;

  const [displayed, setDisplayed] = useState('');
  const revealIdx = useRef(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const latestSourceRef = useRef(sourceText);
  const hasSource = sourceText.length > 0;

  // Keep latest source text available to the interval callback without
  // recreating the timer on every streamed chunk.
  useEffect(() => {
    latestSourceRef.current = sourceText;

    if (isActive && sourceText.length > 0 && sourceText.length <= instantThreshold) {
      revealIdx.current = sourceText.length;
      setDisplayed(sourceText);
      return;
    }

    // Source may reset mid-stream (e.g., between tool-call phases).
    if (isActive && sourceText.length === 0) {
      revealIdx.current = 0;
      setDisplayed('');
    }
  }, [instantThreshold, isActive, sourceText]);

  // When streaming finishes, flush everything immediately.
  useEffect(() => {
    if (!isActive) {
      if (timerRef.current) {
        clearInterval(timerRef.current);
        timerRef.current = null;
      }
      const finalText = latestSourceRef.current;
      revealIdx.current = finalText.length;
      setDisplayed(finalText);
    }
  }, [isActive]);

  // Tick-based reveal while active.
  useEffect(() => {
    if (!isActive) return;
    // If there's nothing to show yet, reset.
    if (!hasSource) {
      revealIdx.current = 0;
      setDisplayed('');
      return;
    }

    if (latestSourceRef.current.length <= instantThreshold) {
      revealIdx.current = latestSourceRef.current.length;
      setDisplayed(latestSourceRef.current);
      return;
    }

    timerRef.current = setInterval(() => {
      const src = latestSourceRef.current;
      setDisplayed((prev) => {
        let idx = revealIdx.current;

        // Source replaced with shorter content while streaming.
        if (idx > src.length) {
          idx = src.length;
          revealIdx.current = idx;
          return src;
        }

        if (idx >= src.length) return prev;

        const gap = src.length - idx;
        // Dynamic acceleration when gap is large.
        const step = gap > 100 ? Math.max(charsPerTick, Math.floor(gap / 8)) : charsPerTick;
        let target = Math.min(idx + step, src.length);

        // Snap forward to the next word boundary to avoid splitting markdown.
        while (target < src.length && !/\s/.test(src[target])) {
          target++;
        }

        revealIdx.current = target;
        return src.slice(0, target);
      });
    }, intervalMs);

    return () => {
      if (timerRef.current) {
        clearInterval(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [isActive, hasSource, charsPerTick, instantThreshold, intervalMs]);

  // Cleanup on unmount.
  useEffect(() => {
    return () => {
      if (timerRef.current) {
        clearInterval(timerRef.current);
        timerRef.current = null;
      }
    };
  }, []);

  return displayed;
}
