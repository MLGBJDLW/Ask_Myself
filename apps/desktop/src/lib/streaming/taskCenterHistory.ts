import type { AgentRunEvent, AgentTaskRunEvent } from '../../types/conversation';
import type { WorkflowAutomationSchedulerEvent } from '../../types/workflows';
import { isTaskTimelineEvent, taskTimelinePayloadFromTaskEvent } from './taskTimeline';

export interface TaskCenterHistoryItem {
  id: string;
  runId: string;
  eventType: string;
  label: string;
  status?: string | null;
  createdAt: string;
  eventSeq?: number;
  source: 'agentRun' | 'taskEvent' | 'schedulerEvent';
}

const HIDDEN_RUN_EVENT_KINDS = new Set<AgentRunEvent['kind']>([
  'outputDelta',
  'outputSnapshot',
  'thinking',
  'usageUpdated',
]);

export interface TaskCenterHistoryOptions {
  includeDeveloper?: boolean;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function taskEventSeq(event: AgentTaskRunEvent): number | undefined {
  const payload = asRecord(event.payload);
  const raw = payload?.eventSeq;
  const value = typeof raw === 'number' ? raw : Number.parseInt(String(raw ?? ''), 10);
  return Number.isFinite(value) && value > 0 ? value : undefined;
}

function stableTime(value: string): number | null {
  if (!value) return null;
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : null;
}

function compareHistory(a: TaskCenterHistoryItem, b: TaskCenterHistoryItem): number {
  const aTime = stableTime(a.createdAt);
  const bTime = stableTime(b.createdAt);
  if (aTime !== null && bTime !== null && aTime !== bTime) {
    return aTime - bTime;
  }
  if (a.eventSeq !== undefined && b.eventSeq !== undefined && a.eventSeq !== b.eventSeq) {
    return a.eventSeq - b.eventSeq;
  }
  return a.id.localeCompare(b.id);
}

function statusForRunEvent(event: AgentRunEvent): string | null {
  if (event.status) return event.status;
  if (event.kind === 'done') return 'completed';
  if (event.kind === 'error') return 'failed';
  return null;
}

function visibilityIsIncluded(
  visibility: 'user' | 'developer' | 'internal' | undefined,
  includeDeveloper: boolean,
): boolean {
  if (visibility === 'internal') return false;
  return visibility !== 'developer' || includeDeveloper;
}

function itemFromRunEvent(
  event: AgentRunEvent,
  includeDeveloper: boolean,
): TaskCenterHistoryItem | null {
  if (HIDDEN_RUN_EVENT_KINDS.has(event.kind)) return null;
  if (!visibilityIsIncluded(event.visibility, includeDeveloper)) return null;
  return {
    id: `${event.runId}:${event.eventSeq}`,
    runId: event.runId,
    eventType: event.kind,
    label: event.label,
    status: statusForRunEvent(event),
    createdAt: event.createdAt ?? '',
    eventSeq: event.eventSeq,
    source: 'agentRun',
  };
}

function itemFromTaskEvent(
  event: AgentTaskRunEvent,
  includeDeveloper: boolean,
): TaskCenterHistoryItem | null {
  const timeline = taskTimelinePayloadFromTaskEvent(event);
  const visibility = timeline?.visibility
    ?? (timeline?.kind === 'verification' ? 'developer' : 'user');
  if (!visibilityIsIncluded(visibility, includeDeveloper)) return null;
  return {
    id: event.id,
    runId: event.runId,
    eventType: event.eventType,
    label: event.label,
    status: event.status ?? null,
    createdAt: event.createdAt,
    eventSeq: taskEventSeq(event),
    source: 'taskEvent',
  };
}

function itemFromSchedulerEvent(event: WorkflowAutomationSchedulerEvent): TaskCenterHistoryItem {
  return {
    id: event.id,
    runId: event.runId ?? event.automationId ?? event.id,
    eventType: event.eventType,
    label: event.summary || `Scheduler ${event.eventType.replace(/_/g, ' ')}`,
    status: event.status ?? null,
    createdAt: event.createdAt,
    source: 'schedulerEvent',
  };
}

export function taskCenterHistoryFromEvents(
  taskEvents: AgentTaskRunEvent[],
  runEvents: AgentRunEvent[],
  schedulerEvents: WorkflowAutomationSchedulerEvent[] = [],
  options: TaskCenterHistoryOptions = {},
): TaskCenterHistoryItem[] {
  return taskCenterHistoryFromRunEvents(runEvents, taskEvents, schedulerEvents, options);
}

export function taskCenterHistoryFromRunEvents(
  runEvents: AgentRunEvent[],
  taskEvents: AgentTaskRunEvent[] = [],
  schedulerEvents: WorkflowAutomationSchedulerEvent[] = [],
  options: TaskCenterHistoryOptions = {},
): TaskCenterHistoryItem[] {
  const includeDeveloper = options.includeDeveloper === true;
  const canonicalItems = runEvents
    .map((event) => itemFromRunEvent(event, includeDeveloper))
    .filter((item): item is TaskCenterHistoryItem => Boolean(item));
  const timelineItems = taskEvents
    .filter(isTaskTimelineEvent)
    .map((event) => itemFromTaskEvent(event, includeDeveloper))
    .filter((item): item is TaskCenterHistoryItem => Boolean(item));
  const schedulerItems = schedulerEvents.map(itemFromSchedulerEvent);
  return [...canonicalItems, ...timelineItems, ...schedulerItems].sort(compareHistory).slice(-50);
}
