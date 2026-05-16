import type { AgentFrontendEvent } from '../../types';
import type { AgentRunEvent, AgentTaskRunEvent } from '../../types/conversation';

type PayloadRecord = Record<string, unknown>;
type LegacyAgentEventType = NonNullable<AgentFrontendEvent['type']>;

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

function fallbackTypeForRunEvent(kind: AgentRunEvent['kind']): LegacyAgentEventType | null {
  switch (kind) {
    case 'outputDelta':
      return 'streamBlockDelta';
    case 'streamReset':
      return 'streamReset';
    case 'thinking':
      return 'thinking';
    case 'status':
    case 'recoveryAttempt':
      return 'status';
    case 'toolPreparing':
      return 'toolCallPreparing';
    case 'toolStarted':
      return 'toolRunStarted';
    case 'toolProgress':
      return 'toolRunUpdated';
    case 'toolCompleted':
      return 'toolRunCompleted';
    case 'approvalRequested':
      return 'approvalRequested';
    case 'approvalResolved':
      return 'approvalResolved';
    case 'usageUpdated':
      return 'usageUpdate';
    case 'autoCompacted':
      return 'autoCompacted';
    case 'done':
      return 'done';
    case 'error':
      return 'error';
    default:
      return null;
  }
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

  const fallbackType = fallbackTypeForRunEvent(runEvent.kind);
  if (!fallbackType) return base;

  return {
    ...base,
    type: fallbackType,
    content: fallbackType === 'status' ? runEvent.label : event.content,
    message: fallbackType === 'error' || fallbackType === 'done'
      ? (payload.message as AgentFrontendEvent['message'] | undefined) ?? runEvent.label
      : event.message,
  };
}

export interface ReplayStreamItem {
  event: AgentTaskRunEvent;
  eventType: 'streamBlockDelta' | 'streamReset' | 'status' | 'terminal';
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
    if (runEvent.kind === 'done' || runEvent.kind === 'error') {
      return {
        event,
        eventType: 'terminal',
        payload: {
          eventSeq: runEvent.eventSeq,
          kind: runEvent.kind,
          status: runEvent.status,
          message: stringValue(runPayload.message) ?? runEvent.label,
          finishReason: stringValue(runPayload.finishReason),
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
