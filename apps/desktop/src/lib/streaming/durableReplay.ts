import type { AgentTaskRunEvent } from '../../types/conversation';
import {
  isDurableStreamEvent,
  replayItemFromTaskEvent,
  type ReplayStreamItem,
} from './legacyAdapter';

export function taskTimelineEventsFromReplaySource(events: AgentTaskRunEvent[]): AgentTaskRunEvent[] {
  return events
    .filter(event => !isDurableStreamEvent(event))
    .slice(-50);
}

export function durableReplayItemsFromTaskEvents(events: AgentTaskRunEvent[]): ReplayStreamItem[] {
  return events
    .map(replayItemFromTaskEvent)
    .filter((item): item is ReplayStreamItem => Boolean(item))
    .sort((a, b) => a.eventSeq - b.eventSeq);
}
