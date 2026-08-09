import type { AgentRunEvent } from '../../types/conversation';

export interface StreamEventOrderingState {
  _lastEventSeq: number;
  _pendingRunEvents: Map<number, AgentRunEvent>;
}

export interface StreamEventEnqueueResult {
  accepted: boolean;
  ready: boolean;
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
    return { accepted: false, ready: false, missingRange: null };
  }

  if (eventSeq <= state._lastEventSeq || state._pendingRunEvents.has(eventSeq)) {
    return { accepted: false, ready: false, missingRange: null };
  }

  state._pendingRunEvents.set(eventSeq, event);
  const expectedSeq = state._lastEventSeq + 1;
  const ready = eventSeq === expectedSeq;

  return {
    accepted: true,
    ready,
    missingRange: ready ? null : { from: expectedSeq, to: eventSeq - 1 },
  };
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
