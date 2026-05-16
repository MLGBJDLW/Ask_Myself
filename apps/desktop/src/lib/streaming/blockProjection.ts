import type { TraceReplyEvent, TraceThinkingEvent } from './protocol';
import type { StreamTerminalProjectionState } from './terminalProjection';

export function appendThinkingTraceEvent(state: StreamTerminalProjectionState, delta: string): void {
  if (!delta) return;
  const last = state.traceEvents[state.traceEvents.length - 1];
  if (last?.kind === 'thinking') {
    state.traceEvents = [
      ...state.traceEvents.slice(0, -1),
      { ...last, text: last.text + delta },
    ];
    return;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-thinking-${Date.now()}-${state._traceSeq++}`,
    kind: 'thinking',
    text: delta,
  }];
}

export function appendReplyTraceEvent(state: StreamTerminalProjectionState, delta: string): void {
  if (!delta) return;
  const last = state.traceEvents[state.traceEvents.length - 1];
  if (last?.kind === 'reply') {
    state.traceEvents = [
      ...state.traceEvents.slice(0, -1),
      { ...last, text: last.text + delta },
    ];
    return;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-reply-${Date.now()}-${state._traceSeq++}`,
    kind: 'reply',
    text: delta,
  }];
}

function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

function upsertThinkingBlockTraceEvent(
  state: StreamTerminalProjectionState,
  blockId: string,
  offset: number,
  delta: string,
): boolean {
  if (!delta) return false;
  const deltaBytes = utf8ByteLength(delta);
  const idx = state.traceEvents.findIndex(
    event => event.kind === 'thinking' && event.blockId === blockId,
  );
  if (idx >= 0) {
    const prev = state.traceEvents[idx] as TraceThinkingEvent;
    const nextOffset = prev.nextOffset ?? 0;
    if (offset < nextOffset) return false;
    const next = [...state.traceEvents];
    next[idx] = {
      ...prev,
      text: prev.text + delta,
      nextOffset: offset + deltaBytes,
    };
    state.traceEvents = next;
    return true;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-thinking-${Date.now()}-${state._traceSeq++}`,
    kind: 'thinking',
    text: delta,
    blockId,
    nextOffset: offset + deltaBytes,
  }];
  return true;
}

function upsertReplyBlockTraceEvent(
  state: StreamTerminalProjectionState,
  blockId: string,
  offset: number,
  delta: string,
): boolean {
  if (!delta) return false;
  const deltaBytes = utf8ByteLength(delta);
  const idx = state.traceEvents.findIndex(
    event => event.kind === 'reply' && event.blockId === blockId,
  );
  if (idx >= 0) {
    const prev = state.traceEvents[idx] as TraceReplyEvent;
    const nextOffset = prev.nextOffset ?? 0;
    if (offset < nextOffset) return false;
    const next = [...state.traceEvents];
    next[idx] = {
      ...prev,
      text: prev.text + delta,
      nextOffset: offset + deltaBytes,
    };
    state.traceEvents = next;
    return true;
  }

  state.traceEvents = [...state.traceEvents, {
    id: `trace-reply-${Date.now()}-${state._traceSeq++}`,
    kind: 'reply',
    text: delta,
    blockId,
    nextOffset: offset + deltaBytes,
  }];
  return true;
}

export function applyStreamBlockDelta(
  state: StreamTerminalProjectionState,
  channel: 'answer' | 'thinking',
  blockId: string,
  offset: number,
  delta: string,
): void {
  if (!blockId || !delta) return;
  const normalizedOffset = Number.isFinite(offset) && offset >= 0 ? offset : 0;
  const deltaBytes = utf8ByteLength(delta);

  if (channel === 'answer') {
    state.isThinking = false;
    if (state._activeRoundId) {
      state._activeRoundId = null;
      state._activeRoundAcceptingStarts = false;
    }
    if (state._activeAnswerBlockId !== blockId) {
      state._activeAnswerBlockId = blockId;
      state._activeAnswerOffset = 0;
      state.streamText = '';
    }
    if (normalizedOffset < state._activeAnswerOffset) return;
    state.thinkingText = '';
    state.streamText += delta;
    state._activeAnswerOffset = normalizedOffset + deltaBytes;
    upsertReplyBlockTraceEvent(state, blockId, normalizedOffset, delta);
    return;
  }

  state.isThinking = true;
  if (state._activeThinkingBlockId !== blockId) {
    state._activeThinkingBlockId = blockId;
    state._activeThinkingOffset = 0;
    state.thinkingText = '';
  }
  if (normalizedOffset < state._activeThinkingOffset) return;
  state.thinkingText += delta;
  state._activeThinkingOffset = normalizedOffset + deltaBytes;
  upsertThinkingBlockTraceEvent(state, blockId, normalizedOffset, delta);
}
