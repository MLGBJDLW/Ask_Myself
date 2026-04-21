/**
 * Module-level cache for model-probe IPC results.
 *
 * Rationale: probes like `check_ffmpeg` spawn subprocesses (100-500ms on
 * Windows cold-path). Settings re-mounts re-run all 4 probes, causing visible
 * lag. Cache first result for 5min; self-heals on download/delete via
 * `invalidate(kind)`.
 */

export type ProbeKind = 'embed' | 'ocr' | 'whisper' | 'ffmpeg';

interface Entry<T> {
  value: T;
  expiresAt: number;
}

const TTL_MS = 5 * 60 * 1000;

const cache = new Map<string, Entry<unknown>>();

function buildKey(kind: ProbeKind, key: string): string {
  return `${kind}:${key}`;
}

export async function getModelStatus<T>(
  kind: ProbeKind,
  key: string,
  fetcher: () => Promise<T>,
): Promise<T> {
  const cacheKey = buildKey(kind, key);
  const now = Date.now();
  const existing = cache.get(cacheKey) as Entry<T> | undefined;
  if (existing && existing.expiresAt > now) {
    return existing.value;
  }
  const value = await fetcher();
  cache.set(cacheKey, { value, expiresAt: now + TTL_MS });
  return value;
}

export function invalidate(kind: ProbeKind, key?: string): void {
  if (key !== undefined) {
    cache.delete(buildKey(kind, key));
    return;
  }
  const prefix = `${kind}:`;
  for (const k of cache.keys()) {
    if (k.startsWith(prefix)) cache.delete(k);
  }
}

export function invalidateAll(): void {
  cache.clear();
}
