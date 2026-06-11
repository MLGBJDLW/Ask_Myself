import {
  projectChatMessageVisibility,
  projectChatStreamingVisibility,
} from '../src/lib/streaming/chatVisibility';
import type { StreamRoundEvent, TraceEvent } from '../src/lib/streaming/protocol';
import {
  buildLiveTraceTimeline,
  isCurrentTraceActive,
  traceEventsAfterStreamRounds,
  visibleTraceEventsForTimeline,
} from '../src/lib/streaming/timelineViewModel';

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

function toolCall(callId: string) {
  return {
    callId,
    toolName: 'search_knowledge_base',
    arguments: '{}',
    status: 'done' as const,
    argsStatus: 'done' as const,
    argsBytes: 2,
  };
}

function replyTextsFromTimeline(
  traceEvents: TraceEvent[],
  streamRounds: StreamRoundEvent[],
): string[] {
  const visibleTraceEvents = visibleTraceEventsForTimeline(traceEvents);
  const liveTraceEvents = traceEventsAfterStreamRounds(visibleTraceEvents, streamRounds);
  const currentTraceActive = isCurrentTraceActive({
    isStreaming: true,
    isThinking: true,
    thinkingText: 'Second thinking streaming',
    toolCalls: [],
    visibleTraceEvents,
  });
  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: liveTraceEvents,
    isStreaming: true,
    currentTraceActive,
    streamText: '',
    displayedText: '',
  });
  return timeline.flatMap((item) => (item.kind === 'reply' ? [item.content] : []));
}

test('streaming visibility prefers full trace when rounds and new live trace coexist', () => {
  const traceEvents: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'First thinking' },
    { id: 'reply-1', kind: 'reply', text: 'First reply' },
    { id: 'tool-1', kind: 'tool', toolCall: toolCall('call-1') },
    { id: 'thinking-2', kind: 'thinking', text: 'Second thinking streaming' },
  ];
  const streamRounds: StreamRoundEvent[] = [
    {
      id: 'round-1',
      thinking: 'First thinking',
      reply: 'First reply',
      toolCalls: [toolCall('call-1')],
    },
  ];

  const projected = projectChatStreamingVisibility({
    isStreaming: true,
    streamRounds,
    traceEvents,
  });

  assertEqual(projected.strategy, 'traceTimeline', 'projection strategy');
  assertEqual(projected.streamRounds.length, 0, 'rounds are suppressed to avoid double render');
  assertEqual(projected.traceEvents.length, traceEvents.length, 'full trace is preserved');
  assert(
    projected.traceEvents.some((event) => event.kind === 'reply' && event.text === 'First reply'),
    'prior streamed reply remains visible while later thinking streams',
  );
  assert(
    projected.traceEvents.some((event) => event.kind === 'thinking' && event.text === 'Second thinking streaming'),
    'current thinking remains visible',
  );
});

test('full live timeline keeps prior reply while later thinking streams', () => {
  const traceEvents: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'First thinking' },
    { id: 'reply-1', kind: 'reply', text: 'First reply' },
    { id: 'tool-1', kind: 'tool', toolCall: toolCall('call-1') },
    { id: 'thinking-2', kind: 'thinking', text: 'Second thinking streaming' },
  ];
  const streamRounds: StreamRoundEvent[] = [
    {
      id: 'round-1',
      thinking: 'First thinking',
      reply: 'First reply',
      toolCalls: [toolCall('call-1')],
    },
  ];

  const oldPathReplies = replyTextsFromTimeline(traceEvents, streamRounds);
  assert(
    !oldPathReplies.includes('First reply'),
    'pre-fix stream-round trimming drops the prior reply from live timeline',
  );

  const projected = projectChatStreamingVisibility({
    isStreaming: true,
    streamRounds,
    traceEvents,
  });
  const newPathReplies = replyTextsFromTimeline(projected.traceEvents, projected.streamRounds);

  assert(
    newPathReplies.includes('First reply'),
    'projected live timeline preserves prior reply while later thinking streams',
  );
});

test('streaming visibility leaves normal states unchanged', () => {
  const traceEvents: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'Thinking' },
  ];
  const streamRounds: StreamRoundEvent[] = [];

  const projected = projectChatStreamingVisibility({
    isStreaming: true,
    streamRounds,
    traceEvents,
  });

  assertEqual(projected.strategy, 'default', 'projection strategy');
  assert(projected.streamRounds === streamRounds, 'rounds reference is unchanged');
  assert(projected.traceEvents === traceEvents, 'trace reference is unchanged');
});

test('live optimistic steering is projected out of history while streaming', () => {
  const firstUser = message({
    id: 'user-1',
    role: 'user',
    content: 'Start the turn',
    createdAt: '2026-01-01T00:00:00.000Z',
  });
  const steering = message({
    id: 'temp-steer-1',
    role: 'user',
    content: 'Please redirect the investigation here.',
    createdAt: '2026-01-01T00:00:05.000Z',
    artifacts: { kind: 'steering', delivery: 'accepted' },
  });
  const projected = projectChatMessageVisibility({
    isStreaming: true,
    messages: [firstUser, steering],
  });

  assertEqual(projected.historyMessages.length, 1, 'history count');
  assertEqual(projected.historyMessages[0].id, 'user-1', 'history keeps first user');
  assertEqual(projected.liveSteeringMessages.length, 1, 'live steering count');
  assertEqual(projected.liveSteeringMessages[0].id, 'temp-steer-1', 'live steering id');
});

test('persisted steering stays in history after streaming completes', () => {
  const firstUser = message({
    id: 'user-1',
    role: 'user',
    content: 'Start the turn',
    createdAt: '2026-01-01T00:00:00.000Z',
  });
  const steering = message({
    id: 'steer-persisted-1',
    role: 'user',
    content: 'Please redirect the investigation here.',
    createdAt: '2026-01-01T00:00:05.000Z',
    artifacts: { kind: 'steering' },
  });
  const messages = [firstUser, steering];
  const projected = projectChatMessageVisibility({
    isStreaming: false,
    messages,
  });

  assert(projected.historyMessages === messages, 'completed projection keeps history reference');
  assertEqual(projected.historyMessages.length, 2, 'history count');
  assertEqual(projected.liveSteeringMessages.length, 0, 'live steering count');
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
