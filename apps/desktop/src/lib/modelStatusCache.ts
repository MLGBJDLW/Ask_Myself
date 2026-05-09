/**
 * Cache for model-probe IPC results.
 *
 * Rationale: probes like `check_ffmpeg` spawn subprocesses (100-500ms on
 * Windows cold-path). Re-running all probes on every app launch or Settings
 * mount causes visible lag. Cache the first result for a while; self-heals on
 * download/delete via `invalidate(kind)` and manual refresh buttons.
 */

export type ProbeKind = 'embed' | 'ocr' | 'whisper' | 'ffmpeg' | 'office';

interface Entry<T> {
  value: T;
  expiresAt: number;
}

const TTL_MS = 12 * 60 * 60 * 1000;
const STORAGE_KEY = 'nexa-model-status-cache-v1';

const cache = new Map<string, Entry<unknown>>();
const inFlight = new Map<string, Promise<unknown>>();
let storageLoaded = false;

function buildKey(kind: ProbeKind, key: string): string {
  return `${kind}:${key}`;
}

function loadStorage(): void {
  if (storageLoaded || typeof window === 'undefined') return;
  storageLoaded = true;

  try {
    const raw = window.localStorage?.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as Record<string, Entry<unknown>>;
    const now = Date.now();
    for (const [key, entry] of Object.entries(parsed)) {
      if (entry && typeof entry.expiresAt === 'number' && entry.expiresAt > now) {
        cache.set(key, entry);
      }
    }
  } catch {
    // Ignore malformed or unavailable storage; the in-memory cache still works.
  }
}

function saveStorage(): void {
  if (typeof window === 'undefined') return;
  try {
    const obj = Object.fromEntries(cache.entries());
    window.localStorage?.setItem(STORAGE_KEY, JSON.stringify(obj));
  } catch {
    // Ignore quota/storage failures.
  }
}

export async function getModelStatus<T>(
  kind: ProbeKind,
  key: string,
  fetcher: () => Promise<T>,
): Promise<T> {
  loadStorage();
  const cacheKey = buildKey(kind, key);
  const now = Date.now();
  const existing = cache.get(cacheKey) as Entry<T> | undefined;
  if (existing && existing.expiresAt > now) {
    return existing.value;
  }
  const pending = inFlight.get(cacheKey) as Promise<T> | undefined;
  if (pending) {
    return pending;
  }
  const promise = fetcher()
    .then((value) => {
      cache.set(cacheKey, { value, expiresAt: Date.now() + TTL_MS });
      saveStorage();
      return value;
    })
    .finally(() => {
      inFlight.delete(cacheKey);
    });
  inFlight.set(cacheKey, promise);
  return promise;
}

export function invalidate(kind: ProbeKind, key?: string): void {
  loadStorage();
  if (key !== undefined) {
    cache.delete(buildKey(kind, key));
    inFlight.delete(buildKey(kind, key));
    saveStorage();
    return;
  }
  const prefix = `${kind}:`;
  for (const k of cache.keys()) {
    if (k.startsWith(prefix)) cache.delete(k);
  }
  for (const k of inFlight.keys()) {
    if (k.startsWith(prefix)) inFlight.delete(k);
  }
  saveStorage();
}

export function invalidateAll(): void {
  loadStorage();
  cache.clear();
  inFlight.clear();
  saveStorage();
}
