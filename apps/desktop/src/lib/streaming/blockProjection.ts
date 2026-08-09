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

function pendingBlocks(
  state: StreamTerminalProjectionState,
  channel: 'answer' | 'thinking',
): Map<string, Map<number, string>> {
  return channel === 'answer'
    ? state._pendingAnswerBlockDeltas
    : state._pendingThinkingBlockDeltas;
}

function bufferBlockDelta(
  state: StreamTerminalProjectionState,
  channel: 'answer' | 'thinking',
  blockId: string,
  offset: number,
  delta: string,
): void {
  const blocks = pendingBlocks(state, channel);
  const pending = blocks.get(blockId) ?? new Map<number, string>();
  if (!pending.has(offset)) pending.set(offset, delta);
  blocks.set(blockId, pending);
}

function takeBufferedBlockDelta(
  state: StreamTerminalProjectionState,
  channel: 'answer' | 'thinking',
  blockId: string,
  offset: number,
): string | null {
  const blocks = pendingBlocks(state, channel);
  const pending = blocks.get(blockId);
  const delta = pending?.get(offset);
  if (delta === undefined) return null;
  pending?.delete(offset);
  if (pending?.size === 0) blocks.delete(blockId);
  return delta;
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

  if (channel === 'answer') {
    state.isThinking = false;
    if (state._activeRoundId) {
      state._activeRoundId = null;
      state._activeRoundAcceptingStarts = false;
    }
    if (state._activeAnswerBlockId !== blockId) {
      if (normalizedOffset !== 0) {
        bufferBlockDelta(state, channel, blockId, normalizedOffset, delta);
        return;
      }
      state._activeAnswerBlockId = blockId;
      state._activeAnswerOffset = 0;
      state.streamText = '';
    }
    if (normalizedOffset < state._activeAnswerOffset) return;
    if (normalizedOffset > state._activeAnswerOffset) {
      bufferBlockDelta(state, channel, blockId, normalizedOffset, delta);
      return;
    }
    state.thinkingText = '';
    let nextDelta: string | null = delta;
    while (nextDelta !== null) {
      const nextOffset = state._activeAnswerOffset;
      state.streamText += nextDelta;
      state._activeAnswerOffset = nextOffset + utf8ByteLength(nextDelta);
      upsertReplyBlockTraceEvent(state, blockId, nextOffset, nextDelta);
      nextDelta = takeBufferedBlockDelta(
        state,
        channel,
        blockId,
        state._activeAnswerOffset,
      );
    }
    return;
  }

  state.isThinking = true;
  if (state._activeThinkingBlockId !== blockId) {
    if (normalizedOffset !== 0) {
      bufferBlockDelta(state, channel, blockId, normalizedOffset, delta);
      return;
    }
    state._activeThinkingBlockId = blockId;
    state._activeThinkingOffset = 0;
    state.thinkingText = '';
  }
  if (normalizedOffset < state._activeThinkingOffset) return;
  if (normalizedOffset > state._activeThinkingOffset) {
    bufferBlockDelta(state, channel, blockId, normalizedOffset, delta);
    return;
  }
  let nextDelta: string | null = delta;
  while (nextDelta !== null) {
    const nextOffset = state._activeThinkingOffset;
    state.thinkingText += nextDelta;
    state._activeThinkingOffset = nextOffset + utf8ByteLength(nextDelta);
    upsertThinkingBlockTraceEvent(state, blockId, nextOffset, nextDelta);
    nextDelta = takeBufferedBlockDelta(
      state,
      channel,
      blockId,
      state._activeThinkingOffset,
    );
  }
}
