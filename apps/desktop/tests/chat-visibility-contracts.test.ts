import { projectChatStreamingVisibility } from '../src/lib/streaming/chatVisibility';
import type { StreamRoundEvent, TraceEvent } from '../src/lib/streaming/protocol';

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
