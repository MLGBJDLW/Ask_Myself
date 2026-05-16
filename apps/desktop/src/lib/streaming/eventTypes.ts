import type { AgentFrontendEvent } from '../../types';

export type AgentEventType = NonNullable<AgentFrontendEvent['type']>;

export function normalizeAgentEventType(value: unknown): AgentEventType | null {
  if (typeof value !== 'string') return null;
  const raw = value.trim();
  if (!raw) return null;

  const lowered = raw
    .replace(/[_\s-]+([a-zA-Z0-9])/g, (_m, ch: string) => ch.toUpperCase())
    .replace(/^([A-Z])/, (_m, ch: string) => ch.toLowerCase());

  switch (lowered) {
    case 'thinking': return 'thinking';
    case 'textDelta': return 'textDelta';
    case 'streamBlockDelta': return 'streamBlockDelta';
    case 'streamReset': return 'streamReset';
    case 'toolCallPreparing': return 'toolCallPreparing';
    case 'toolCallStart': return 'toolCallStart';
    case 'toolCallArgsDelta': return 'toolCallArgsDelta';
    case 'toolCallProgress': return 'toolCallProgress';
    case 'toolCallResult': return 'toolCallResult';
    case 'toolRunStarted': return 'toolRunStarted';
    case 'toolRunUpdated': return 'toolRunUpdated';
    case 'toolRunCompleted': return 'toolRunCompleted';
    case 'status': return 'status';
    case 'done': return 'done';
    case 'error': return 'error';
    case 'autoCompacted': return 'autoCompacted';
    case 'usageUpdate': return 'usageUpdate';
    case 'approvalRequested': return 'approvalRequested';
    case 'approvalResolved': return 'approvalResolved';
    case 'taskRunUpdated': return 'taskRunUpdated';
    case 'taskRunEvent': return 'taskRunEvent';
    default: return null;
  }
}

export function isTaskLifecycleEventType(eventType: AgentEventType): boolean {
  return eventType === 'taskRunUpdated' || eventType === 'taskRunEvent';
}

export function isTerminalEventType(eventType: AgentEventType): boolean {
  return eventType === 'done' || eventType === 'error';
}
