import { adaptFrontendRunEvent } from '../src/lib/streaming/legacyAdapter';
import {
  durableReplayItemsFromTaskEvents,
  taskTimelineEventsFromReplaySource,
} from '../src/lib/streaming/durableReplay';
import { armStreamWatchdog, clearStreamWatchdog } from '../src/lib/streaming/watchdog';
import { streamStore } from '../src/lib/streamStore';
import type {
  AgentFrontendEvent,
  AgentRunEvent,
  AgentRunEventKind,
  AgentRunPhase,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../src/types/conversation';

type TestFn = () => void | Promise<void>;

const tests: Array<{ name: string; fn: TestFn }> = [];

function test(name: string, fn: TestFn): void {
  tests.push({ name, fn });
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function runEvent(input: {
  eventSeq: number;
  kind: AgentRunEventKind;
  payload?: AgentRunEvent['payload'];
  phase?: AgentRunPhase;
  label?: string;
  status?: string | null;
}): AgentRunEvent {
  return {
    version: 2,
    runId: 'run-1',
    turnId: 'turn-1',
    eventSeq: input.eventSeq,
    kind: input.kind,
    phase: input.phase ?? 'responding',
    label: input.label ?? input.kind,
    status: input.status ?? 'running',
    payload: input.payload ?? null,
  };
}

function frontendEvent(runEvent: AgentRunEvent): AgentFrontendEvent {
  return {
    conversationId: 'conversation-1',
    type: 'status',
    runEvent,
  };
}

function taskEvent(input: {
  id: string;
  eventType: string;
  eventSeq?: number;
  payload?: AgentTaskRunEvent['payload'];
  label?: string;
  status?: string | null;
}): AgentTaskRunEvent {
  return {
    id: input.id,
    runId: 'run-1',
    eventType: input.eventType,
    label: input.label ?? input.eventType,
    status: input.status ?? null,
    payload: input.payload ?? null,
    createdAt: `2026-01-01T00:00:0${input.eventSeq ?? 0}.000Z`,
  };
}

function taskRun(status: string): AgentTaskRun {
  return {
    id: 'run-1',
    conversationId: 'conversation-1',
    turnId: 'turn-1',
    userMessageId: 'message-1',
    status,
    phase: status === 'running' ? 'responding' : 'done',
    title: 'Streaming contract test',
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:01.000Z',
  };
}

test('adapts canonical outputDelta into the legacy frontend stream shape', () => {
  const event = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 7,
    kind: 'outputDelta',
    payload: {
      blockId: 'block-answer',
      channel: 'answer',
      offset: 12,
      delta: 'hello',
    },
  })));

  assertEqual(event.type, 'streamBlockDelta', 'event type');
  assertEqual(event.eventSeq, 7, 'eventSeq');
  assertEqual(event.blockId, 'block-answer', 'block id');
  assertEqual(event.channel, 'answer', 'channel');
  assertEqual(event.offset, 12, 'offset');
  assertEqual(event.delta, 'hello', 'delta');
});

test('adapts recoveryAttempt into a muted status update', () => {
  const event = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 8,
    kind: 'recoveryAttempt',
    label: 'Retrying stream',
    status: 'recovering',
    payload: { reason: 'Retrying stream' },
  })));

  assertEqual(event.type, 'status', 'event type');
  assertEqual(event.eventSeq, 8, 'eventSeq');
  assertEqual(event.content, 'Retrying stream', 'content');
  assertEqual(event.tone, 'muted', 'tone');
});

test('builds durable replay items from canonical and legacy task events in eventSeq order', () => {
  const events = [
    taskEvent({
      id: 'legacy-stream',
      eventType: 'streamBlockDelta',
      payload: {
        eventSeq: 3,
        blockId: 'legacy-block',
        channel: 'answer',
        offset: 0,
        delta: ' legacy',
      },
    }),
    taskEvent({
      id: 'canonical-recovery',
      eventType: 'status',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'recoveryAttempt',
          label: 'Retrying stream',
          status: 'recovering',
          payload: { reason: 'Retrying stream' },
        }),
      },
    }),
    taskEvent({
      id: 'canonical-output',
      eventType: 'stream',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'outputDelta',
          payload: {
            blockId: 'canonical-block',
            channel: 'thinking',
            offset: 0,
            delta: 'thought',
          },
        }),
      },
    }),
  ];

  const replay = durableReplayItemsFromTaskEvents(events);

  assertEqual(replay.length, 3, 'replay item count');
  assertEqual(replay[0].eventSeq, 1, 'first eventSeq');
  assertEqual(replay[0].eventType, 'streamBlockDelta', 'first event type');
  assertEqual(replay[1].eventSeq, 2, 'second eventSeq');
  assertEqual(replay[1].eventType, 'status', 'second event type');
  assertEqual(replay[2].eventSeq, 3, 'third eventSeq');
});

test('keeps non-stream task timeline events while filtering durable stream events', () => {
  const events = [
    taskEvent({
      id: 'stream',
      eventType: 'stream',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'outputDelta',
          payload: { blockId: 'b', channel: 'answer', offset: 0, delta: 'x' },
        }),
      },
    }),
    taskEvent({
      id: 'recovery',
      eventType: 'status',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'recoveryAttempt',
          label: 'Retrying',
          payload: { reason: 'Retrying' },
        }),
      },
    }),
    taskEvent({ id: 'tool', eventType: 'tool', payload: { toolName: 'read_files' } }),
  ];

  const timeline = taskTimelineEventsFromReplaySource(events);

  assertEqual(timeline.length, 2, 'timeline item count');
  assertEqual(timeline[0].id, 'recovery', 'keeps recovery status event');
  assertEqual(timeline[1].id, 'tool', 'keeps tool event');
});

test('projects canonical terminal errors as durable replay terminal items', () => {
  const replay = durableReplayItemsFromTaskEvents([
    taskEvent({
      id: 'terminal',
      eventType: 'error',
      payload: {
        agentRun: runEvent({
          eventSeq: 9,
          kind: 'error',
          phase: 'done',
          label: 'Agent execution timed out.',
          status: 'failed',
          payload: { type: 'error', message: 'Agent execution timed out.' },
        }),
      },
    }),
  ]);

  assertEqual(replay.length, 1, 'terminal replay item count');
  assertEqual(replay[0].eventType, 'terminal', 'terminal replay event type');
  assertEqual(replay[0].payload.kind, 'error', 'terminal kind');
  assertEqual(replay[0].payload.message, 'Agent execution timed out.', 'terminal message');
});

test('watchdog arms, fires, and clears timeout handles', async () => {
  const state = { _timeoutId: null };
  let fired = 0;

  armStreamWatchdog(state, () => {
    fired += 1;
  }, 1);

  assert(state._timeoutId !== null, 'watchdog should store timeout handle');
  await new Promise(resolve => setTimeout(resolve, 10));
  assertEqual(fired, 1, 'watchdog callback count');
  assertEqual(state._timeoutId, null, 'watchdog clears handle after firing');

  armStreamWatchdog(state, () => {
    fired += 1;
  }, 50);
  clearStreamWatchdog(state);
  await new Promise(resolve => setTimeout(resolve, 60));
  assertEqual(fired, 1, 'cleared watchdog should not fire');
  assertEqual(state._timeoutId, null, 'cleared watchdog handle');
});

test('restoreFromTaskEvents projects terminal error replay into stream state', () => {
  const conversationId = 'conversation-terminal-restore';

  streamStore.restoreFromTaskEvents(conversationId, taskRun('failed'), [
    taskEvent({
      id: 'terminal',
      eventType: 'error',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'error',
          phase: 'done',
          label: 'Agent execution timed out.',
          status: 'failed',
          payload: { type: 'error', message: 'Agent execution timed out.' },
        }),
      },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'restored stream state should exist');
  assertEqual(restored.isStreaming, false, 'terminal replay stops streaming');
  assertEqual(restored.error, 'Agent execution timed out.', 'terminal replay error');
  assert(
    restored.traceEvents.some(event => event.kind === 'status' && event.tone === 'error'),
    'terminal replay should add an error status trace',
  );

  streamStore.clearStream(conversationId);
});

async function main(): Promise<void> {
  for (const { name, fn } of tests) {
    try {
      await fn();
      console.log(`ok - ${name}`);
    } catch (error) {
      console.error(`not ok - ${name}`);
      throw error;
    }
  }
}

void main();
