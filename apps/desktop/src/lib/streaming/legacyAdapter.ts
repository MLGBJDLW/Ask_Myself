import type { AgentFrontendEvent } from '../../types';
import type { AgentRunEvent, AgentTaskRunEvent } from '../../types/conversation';

type PayloadRecord = Record<string, unknown>;

function asRecord(value: unknown): PayloadRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as PayloadRecord
    : null;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

export function adaptFrontendRunEvent(event: AgentFrontendEvent): AgentFrontendEvent {
  const runEvent = event.runEvent;
  if (!runEvent) return event;

  const payload = asRecord(runEvent.payload) ?? {};
  const base: AgentFrontendEvent = {
    ...event,
    eventSeq: runEvent.eventSeq,
  };

  if (runEvent.kind === 'outputDelta') {
    return {
      ...base,
      type: 'streamBlockDelta',
      blockId: stringValue(payload.blockId) ?? event.blockId,
      channel: payload.channel === 'thinking' ? 'thinking' : 'answer',
      offset: numberValue(payload.offset) ?? event.offset,
      delta: stringValue(payload.delta) ?? event.delta,
    };
  }

  if (runEvent.kind === 'streamReset') {
    return {
      ...base,
      type: 'streamReset',
      reason: stringValue(payload.reason) ?? runEvent.label,
    };
  }

  if (runEvent.kind === 'recoveryAttempt') {
    return {
      ...base,
      type: 'status',
      content: runEvent.label,
      tone: 'muted',
    };
  }

  const legacyType = stringValue(payload.type);
  if (legacyType) {
    return {
      ...payload,
      ...base,
      type: legacyType as AgentFrontendEvent['type'],
      eventSeq: runEvent.eventSeq,
    };
  }

  return base;
}

export interface ReplayStreamItem {
  event: AgentTaskRunEvent;
  eventType: 'streamBlockDelta' | 'streamReset' | 'status';
  payload: PayloadRecord;
  eventSeq: number;
}

export function replayItemFromTaskEvent(event: AgentTaskRunEvent): ReplayStreamItem | null {
  const payload = asRecord(event.payload);
  if (!payload) return null;

  const agentRun = asRecord(payload.agentRun);
  if (agentRun) {
    const runEvent = agentRun as unknown as AgentRunEvent;
    const runPayload = asRecord(runEvent.payload) ?? {};
    if (runEvent.kind === 'outputDelta') {
      return {
        event,
        eventType: 'streamBlockDelta',
        payload: {
          eventSeq: runEvent.eventSeq,
          blockId: runPayload.blockId,
          channel: runPayload.channel,
          offset: runPayload.offset,
          delta: runPayload.delta,
        },
        eventSeq: runEvent.eventSeq,
      };
    }
    if (runEvent.kind === 'streamReset') {
      return {
        event,
        eventType: 'streamReset',
        payload: {
          eventSeq: runEvent.eventSeq,
          reason: stringValue(runPayload.reason) ?? runEvent.label,
        },
        eventSeq: runEvent.eventSeq,
      };
    }
    if (runEvent.kind === 'recoveryAttempt') {
      return {
        event,
        eventType: 'status',
        payload: {
          eventSeq: runEvent.eventSeq,
          reason: runEvent.label,
        },
        eventSeq: runEvent.eventSeq,
      };
    }
    return null;
  }

  const eventSeqRaw = payload.eventSeq;
  const eventSeq = typeof eventSeqRaw === 'number'
    ? eventSeqRaw
    : Number.parseInt(String(eventSeqRaw ?? ''), 10);
  if (!Number.isFinite(eventSeq) || eventSeq <= 0) return null;
  if (event.eventType !== 'streamBlockDelta' && event.eventType !== 'streamReset') return null;

  return {
    event,
    eventType: event.eventType,
    payload,
    eventSeq,
  };
}

export function isDurableStreamEvent(event: AgentTaskRunEvent): boolean {
  const item = replayItemFromTaskEvent(event);
  return item?.eventType === 'streamBlockDelta' || item?.eventType === 'streamReset';
}

