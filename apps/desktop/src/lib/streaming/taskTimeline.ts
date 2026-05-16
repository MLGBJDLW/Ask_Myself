import type { AgentTaskRunEvent, TaskTimelineEvent } from '../../types/conversation';

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

export function taskTimelinePayloadFromTaskEvent(
  event: AgentTaskRunEvent,
): TaskTimelineEvent | null {
  const payload = asRecord(event.payload);
  const timeline = asRecord(payload?.taskTimeline);
  if (!timeline) return null;

  const kind = timeline.kind;
  if (kind !== 'subtask' && kind !== 'verification') return null;

  return timeline as unknown as TaskTimelineEvent;
}

export function isTaskTimelineEvent(event: AgentTaskRunEvent): boolean {
  return taskTimelinePayloadFromTaskEvent(event) !== null;
}
