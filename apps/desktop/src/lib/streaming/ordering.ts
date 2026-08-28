import type { AgentRunEvent } from '../../types/conversation';

export interface StreamEventOrderingState {
  _orderedRunId: string | null;
  _lastEventSeq: number;
  _pendingRunEvents: Map<number, AgentRunEvent>;
}

export interface StreamEventEnqueueResult {
  accepted: boolean;
  ready: boolean;
  runChanged: boolean;
  missingRange: { from: number; to: number } | null;
}

export function parseStreamEventSeq(value: unknown): number | null {
  const eventSeq = typeof value === 'number'
    ? value
    : Number.parseInt(String(value ?? ''), 10);
  return Number.isFinite(eventSeq) && eventSeq > 0 ? eventSeq : null;
}

export function enqueueStreamRunEvent(
  state: StreamEventOrderingState,
  event: AgentRunEvent,
): StreamEventEnqueueResult {
  const eventSeq = parseStreamEventSeq(event.eventSeq);
  if (eventSeq === null) {
    return { accepted: false, ready: false, runChanged: false, missingRange: null };
  }

  if (state._orderedRunId !== null && state._orderedRunId !== event.runId) {
    return { accepted: false, ready: false, runChanged: true, missingRange: null };
  }
  state._orderedRunId = event.runId;

  if (eventSeq <= state._lastEventSeq || state._pendingRunEvents.has(eventSeq)) {
    return { accepted: false, ready: false, runChanged: false, missingRange: null };
  }

  state._pendingRunEvents.set(eventSeq, event);
  const expectedSeq = state._lastEventSeq + 1;
  const ready = eventSeq === expectedSeq;

  return {
    accepted: true,
    ready,
    runChanged: false,
    missingRange: ready ? null : { from: expectedSeq, to: eventSeq - 1 },
  };
}

/**
 * Align an empty ordering buffer to an authoritative durable ledger entry.
 *
 * Historical ledgers may contain intentional sequence gaps because ephemeral
 * liveness events were not persisted. The database result is already the
 * complete ordered source for replay, so those gaps must not strand later
 * durable events. Live delivery never calls this helper and therefore keeps
 * strict missing-event recovery.
 */
export function alignAuthoritativeReplayCursor(
  state: StreamEventOrderingState,
  event: AgentRunEvent,
): boolean {
  const eventSeq = parseStreamEventSeq(event.eventSeq);
  if (eventSeq === null || state._pendingRunEvents.size > 0) return false;
  if (state._orderedRunId !== null && state._orderedRunId !== event.runId) return false;
  if (eventSeq <= state._lastEventSeq) return false;

  state._orderedRunId = event.runId;
  if (eventSeq > state._lastEventSeq + 1) {
    state._lastEventSeq = eventSeq - 1;
  }
  return true;
}

/**
 * Merge a database-authoritative suffix with live events that arrived while
 * the query was in flight, then advance across sequence numbers that SQLite
 * confirms were intentionally not retained (for example ephemeral previews).
 */
export function takeAuthoritativeRunEventSuffix(
  state: StreamEventOrderingState,
  runId: string,
  durableEvents: AgentRunEvent[],
  options: {
    includeLivePending: boolean;
    authoritativeThroughEventSeq: number;
  },
): AgentRunEvent[] {
  if (state._orderedRunId !== null && state._orderedRunId !== runId) return [];

  const bySequence = new Map<number, AgentRunEvent>();
  const deferredLiveEvents: AgentRunEvent[] = [];
  for (const event of state._pendingRunEvents.values()) {
    const eventSeq = parseStreamEventSeq(event.eventSeq);
    if (
      event.runId === runId
      && eventSeq !== null
      && options.includeLivePending
      && eventSeq <= options.authoritativeThroughEventSeq
    ) {
      bySequence.set(eventSeq, event);
    } else {
      deferredLiveEvents.push(event);
    }
  }
  for (const event of durableEvents) {
    const eventSeq = parseStreamEventSeq(event.eventSeq);
    if (event.runId === runId && eventSeq !== null) bySequence.set(eventSeq, event);
  }

  state._pendingRunEvents.clear();
  const ready: AgentRunEvent[] = [];
  const ordered = [...bySequence.entries()]
    .sort(([left], [right]) => left - right)
    .map(([, event]) => event);
  for (const event of ordered) {
    const eventSeq = parseStreamEventSeq(event.eventSeq);
    if (eventSeq === null || eventSeq <= state._lastEventSeq) continue;
    if (
      eventSeq > state._lastEventSeq + 1
      && (
        eventSeq > options.authoritativeThroughEventSeq
        || !alignAuthoritativeReplayCursor(state, event)
      )
    ) {
      enqueueStreamRunEvent(state, event);
      continue;
    }
    const enqueue = enqueueStreamRunEvent(state, event);
    if (!enqueue.accepted || enqueue.runChanged) continue;
    let next: AgentRunEvent | null;
    while ((next = takeNextStreamRunEvent(state)) !== null) ready.push(next);
  }

  for (const event of deferredLiveEvents.sort((left, right) => (
    (parseStreamEventSeq(left.eventSeq) ?? Number.MAX_SAFE_INTEGER)
    - (parseStreamEventSeq(right.eventSeq) ?? Number.MAX_SAFE_INTEGER)
  ))) {
    const enqueue = enqueueStreamRunEvent(state, event);
    if (!enqueue.accepted || enqueue.runChanged) continue;
    let next: AgentRunEvent | null;
    while ((next = takeNextStreamRunEvent(state)) !== null) ready.push(next);
  }
  return ready;
}

export function takeNextStreamRunEvent(
  state: StreamEventOrderingState,
): AgentRunEvent | null {
  const expectedSeq = state._lastEventSeq + 1;
  const event = state._pendingRunEvents.get(expectedSeq);
  if (!event) return null;
  state._pendingRunEvents.delete(expectedSeq);
  state._lastEventSeq = expectedSeq;
  return event;
}
