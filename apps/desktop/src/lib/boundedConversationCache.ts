export interface BoundedCachePolicy<T> {
  maxEntries: number;
  maxBytes: number;
  estimateBytes: (value: T) => number;
  recency: Map<string, number>;
  protectedKeys?: Iterable<string>;
  tick: number;
}

export function estimateJsonBytes(value: unknown): number {
  try {
    return new TextEncoder().encode(JSON.stringify(value)).length;
  } catch {
    return 0;
  }
}

/** Immutable LRU upsert bounded by both conversation count and approximate bytes. */
export function upsertBoundedConversationCache<T>(
  current: Record<string, T>,
  conversationId: string,
  value: T,
  policy: BoundedCachePolicy<T>,
): Record<string, T> {
  const next = { ...current, [conversationId]: value };
  policy.recency.set(conversationId, policy.tick);
  const protectedKeys = new Set(policy.protectedKeys ?? []);
  protectedKeys.add(conversationId);

  const sizes = new Map(
    Object.entries(next).map(([key, entry]) => [key, policy.estimateBytes(entry)]),
  );
  let totalBytes = [...sizes.values()].reduce((sum, size) => sum + size, 0);
  const candidates = Object.keys(next)
    .filter(key => !protectedKeys.has(key))
    .sort((a, b) => (policy.recency.get(a) ?? 0) - (policy.recency.get(b) ?? 0));

  while (
    candidates.length > 0
    && (Object.keys(next).length > policy.maxEntries || totalBytes > policy.maxBytes)
  ) {
    const evicted = candidates.shift();
    if (!evicted) break;
    totalBytes -= sizes.get(evicted) ?? 0;
    delete next[evicted];
    policy.recency.delete(evicted);
  }

  return next;
}
