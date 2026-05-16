export interface StreamEventOrderingState {
  _lastEventSeq: number;
  _eventSeqGapRecorded: boolean;
}

export interface StreamEventOrderingDecision {
  accepted: boolean;
  eventSeq: number | null;
  gapDetected: boolean;
}

export function parseStreamEventSeq(value: unknown): number | null {
  const eventSeq = typeof value === 'number'
    ? value
    : Number.parseInt(String(value ?? ''), 10);
  return Number.isFinite(eventSeq) && eventSeq > 0 ? eventSeq : null;
}

export function applyStreamEventOrdering(
  state: StreamEventOrderingState,
  eventSeqRaw: unknown,
): StreamEventOrderingDecision {
  const eventSeq = parseStreamEventSeq(eventSeqRaw);
  if (eventSeq === null) {
    return { accepted: true, eventSeq: null, gapDetected: false };
  }

  if (eventSeq <= state._lastEventSeq) {
    return { accepted: false, eventSeq, gapDetected: false };
  }

  const gapDetected = state._lastEventSeq > 0
    && eventSeq > state._lastEventSeq + 1
    && !state._eventSeqGapRecorded;
  if (gapDetected) {
    state._eventSeqGapRecorded = true;
  }
  state._lastEventSeq = eventSeq;

  return { accepted: true, eventSeq, gapDetected };
}
