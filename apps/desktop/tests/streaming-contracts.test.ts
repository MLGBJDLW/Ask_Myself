import { adaptFrontendRunEvent } from '../src/lib/streaming/legacyAdapter';
import {
  durableReplayItemsFromTaskEvents,
  taskTimelineEventsFromReplaySource,
} from '../src/lib/streaming/durableReplay';
import { extractPersistedTraceItems } from '../src/lib/streaming/persistedTrace';
import {
  isPendingToolCallStatus,
  normalizePersistedToolCallStatus,
} from '../src/lib/streaming/toolStatus';
import { armStreamWatchdog, clearStreamWatchdog } from '../src/lib/streaming/watchdog';
import { streamStore } from '../src/lib/streamStore';
import {
  isTaskTimelineEvent,
  taskTimelinePayloadFromTaskEvent,
} from '../src/lib/streaming/taskTimeline';
import type {
  AgentFrontendEvent,
  AgentRunEvent,
  AgentRunEventKind,
  AgentRunPhase,
  AgentTaskRun,
  AgentTaskRunEvent,
  ToolRunItem,
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

function toolRun(input: {
  callId: string;
  status: ToolRunItem['status'];
  content?: string;
  progressNote?: string;
}): ToolRunItem {
  return {
    callId: input.callId,
    toolName: 'search_knowledge_base',
    plugin: {
      id: 'knowledge',
      name: 'Knowledge',
      capability: 'search',
      description: 'Search local knowledge',
    },
    status: input.status,
    arguments: '{"query":"nexa"}',
    renderKind: 'search',
    capabilities: {
      inputStreaming: 'none',
      renderKind: 'search',
      readOnly: true,
      destructive: false,
      concurrencySafe: true,
      interruptBehavior: 'block',
      resourceKeys: ['source:notes'],
    },
    content: input.content,
    isError: input.status === 'failed',
    progressNote: input.progressNote,
    durationMs: input.status === 'completed' ? 42 : undefined,
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

test('preserves legacy textDelta payloads carried by outputDelta run events', () => {
  const event = adaptFrontendRunEvent({
    conversationId: 'conversation-1',
    runEvent: runEvent({
      eventSeq: 7,
      kind: 'outputDelta',
      payload: {
        type: 'textDelta',
        delta: 'legacy text',
      },
    }),
  } as AgentFrontendEvent);

  assertEqual(event.type, 'textDelta', 'event type');
  assertEqual(event.eventSeq, 7, 'eventSeq');
  assertEqual(event.delta, 'legacy text', 'delta');
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

test('adapts canonical terminal events without a legacy envelope type', () => {
  const event = adaptFrontendRunEvent({
    conversationId: 'conversation-1',
    runEvent: runEvent({
      eventSeq: 9,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution timed out.',
      status: 'failed',
      payload: { message: 'Agent execution timed out.' },
    }),
  } as AgentFrontendEvent);

  assertEqual(event.type, 'error', 'event type');
  assertEqual(event.eventSeq, 9, 'eventSeq');
  assertEqual(event.message, 'Agent execution timed out.', 'message');
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

test('keeps lifecycle status run events out of durable stream replay', () => {
  const events = [
    taskEvent({
      id: 'queued',
      eventType: 'status',
      label: 'Task queued',
      status: 'queued',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'status',
          phase: 'routing',
          label: 'Task queued',
          status: 'queued',
          payload: {},
        }),
      },
    }),
    taskEvent({
      id: 'output',
      eventType: 'stream',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'outputDelta',
          payload: { blockId: 'b', channel: 'answer', offset: 0, delta: 'x' },
        }),
      },
    }),
  ];

  const replay = durableReplayItemsFromTaskEvents(events);
  const timeline = taskTimelineEventsFromReplaySource(events);

  assertEqual(replay.length, 1, 'lifecycle status should not replay as stream');
  assertEqual(replay[0].event.eventType, 'stream', 'only stream event replays');
  assertEqual(timeline.length, 1, 'lifecycle status remains in task timeline');
  assertEqual(timeline[0].id, 'queued', 'queued status remains visible as task event');
});

test('keeps typed task timeline events out of durable stream replay', () => {
  const events = [
    taskEvent({
      id: 'subtask',
      eventType: 'subtask',
      label: 'Collect evidence',
      status: 'completed',
      payload: {
        taskTimeline: {
          version: 1,
          kind: 'subtask',
          label: 'Collect evidence',
          status: 'completed',
          payload: { subtaskRunId: 'subtask-1' },
        },
      },
    }),
    taskEvent({
      id: 'verification',
      eventType: 'verification',
      label: 'Evidence audit completed',
      status: 'passed',
      payload: {
        taskTimeline: {
          version: 1,
          kind: 'verification',
          label: 'Evidence audit completed',
          status: 'passed',
          payload: { kind: 'verification', overallStatus: 'passed' },
        },
      },
    }),
  ];

  const replay = durableReplayItemsFromTaskEvents(events);
  const timeline = taskTimelineEventsFromReplaySource(events);
  const subtaskTimeline = taskTimelinePayloadFromTaskEvent(events[0]);

  assertEqual(replay.length, 0, 'timeline events should not replay as stream output');
  assertEqual(timeline.length, 2, 'timeline events remain visible');
  assert(isTaskTimelineEvent(events[0]), 'subtask event should expose timeline payload');
  assert(subtaskTimeline, 'subtask timeline payload should parse');
  assertEqual(subtaskTimeline.kind, 'subtask', 'timeline kind');
  assert(
    subtaskTimeline.payload && typeof subtaskTimeline.payload === 'object' && !Array.isArray(subtaskTimeline.payload),
    'timeline payload should be a record',
  );
  assertEqual(
    (subtaskTimeline.payload as Record<string, unknown>).subtaskRunId,
    'subtask-1',
    'timeline payload',
  );
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

test('durable replay restores canonical tool run events through live projection', () => {
  const conversationId = 'conversation-tool-replay';

  streamStore.restoreFromTaskEvents(conversationId, taskRun('completed'), [
    taskEvent({
      id: 'output',
      eventType: 'stream',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'outputDelta',
          payload: { blockId: 'answer-block', channel: 'answer', offset: 0, delta: 'Checking' },
        }),
      },
    }),
    taskEvent({
      id: 'tool-start',
      eventType: 'tool',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'toolStarted',
          phase: 'tooling',
          label: 'search_knowledge_base',
          status: 'running',
          payload: { type: 'toolRunStarted', run: toolRun({ callId: 'call-1', status: 'running' }) },
        }),
      },
    }),
    taskEvent({
      id: 'tool-done',
      eventType: 'tool',
      payload: {
        agentRun: runEvent({
          eventSeq: 3,
          kind: 'toolCompleted',
          phase: 'tooling',
          label: 'search_knowledge_base',
          status: 'completed',
          payload: {
            type: 'toolRunCompleted',
            run: toolRun({ callId: 'call-1', status: 'completed', content: 'Found 2 notes' }),
          },
        }),
      },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'tool replay should create stream state');
  assertEqual(restored.toolCalls.length, 1, 'tool call count');
  assertEqual(restored.toolCalls[0].status, 'done', 'tool status');
  assertEqual(restored.toolCalls[0].content, 'Found 2 notes', 'tool content');
  assert(
    restored.traceEvents.some(event => event.kind === 'tool' && event.toolCall.callId === 'call-1'),
    'tool replay should restore trace tool event',
  );

  streamStore.clearStream(conversationId);
});

test('durable replay restores canonical usage and approval events through live projection', () => {
  const conversationId = 'conversation-usage-approval-replay';

  streamStore.restoreFromTaskEvents(conversationId, taskRun('waiting_approval'), [
    taskEvent({
      id: 'usage',
      eventType: 'stream',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'usageUpdated',
          phase: 'accounting',
          label: 'Token usage updated',
          payload: {
            type: 'usageUpdate',
            usageTotal: { promptTokens: 10, completionTokens: 4, totalTokens: 14 },
            lastPromptTokens: 10,
          },
        }),
      },
    }),
    taskEvent({
      id: 'approval',
      eventType: 'approval',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'approvalRequested',
          phase: 'approval',
          label: 'write_file',
          status: 'pending',
          payload: {
            type: 'approvalRequested',
            request: {
              id: 'approval-1',
              toolName: 'write_file',
              permissionKey: 'file:write',
              targetKind: 'file',
              targetValue: 'README.md',
              argumentsPreview: '{}',
              riskLevel: 'high',
              reason: 'Writes to workspace',
            },
          },
        }),
      },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'usage/approval replay should create stream state');
  assert(restored.lastUsage, 'usage should be restored');
  assertEqual(restored.lastUsage.totalTokens, 14, 'usage total');
  assertEqual(restored.pendingApprovals.length, 1, 'approval count');
  assertEqual(restored.pendingApprovals[0].id, 'approval-1', 'approval id');

  streamStore.clearStream(conversationId);
});

test('normalizes persisted trace tool calls through the shared tool status projection', () => {
  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [
      {
        kind: 'tool',
        toolCall: {
          callId: 'call-1',
          toolName: 'run_shell',
          arguments: '{"program":"cargo"}',
          status: 'failed',
          renderKind: 'commandExecution',
          isError: false,
        },
      },
      {
        kind: 'tool',
        toolCall: {
          callId: 'call-2',
          toolName: 'edit_file',
          arguments: '',
          status: 'approval_pending',
        },
      },
    ],
  });

  assert(items, 'persisted trace items should parse');
  assertEqual(items.length, 2, 'persisted trace item count');
  assert(items[0].kind === 'tool', 'first persisted item should be a tool');
  assertEqual(items[0].toolCall.status, 'error', 'failed trace status normalizes to error');
  assertEqual(items[0].toolCall.argsStatus, 'error', 'failed trace args status');
  assert(items[1].kind === 'tool', 'second persisted item should be a tool');
  assertEqual(items[1].toolCall.status, 'approvalPending', 'approval status normalizes');
  assert(isPendingToolCallStatus(items[1].toolCall.status), 'approval trace is pending');
});

test('uses one tool status reducer for run, trace, and card-facing statuses', () => {
  assertEqual(normalizePersistedToolCallStatus('completed'), 'done', 'completed status');
  assertEqual(normalizePersistedToolCallStatus('timed_out'), 'timedOut', 'timed out status');
  assert(isPendingToolCallStatus('preparing'), 'preparing is pending');
  assert(!isPendingToolCallStatus('done'), 'done is not pending');
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

test('restoreFromTaskEvents preserves cancelled terminal replay status', () => {
  const conversationId = 'conversation-cancelled-terminal-restore';

  streamStore.restoreFromTaskEvents(conversationId, taskRun('cancelled'), [
    taskEvent({
      id: 'terminal-cancelled',
      eventType: 'error',
      payload: {
        agentRun: runEvent({
          eventSeq: 1,
          kind: 'error',
          phase: 'done',
          label: 'Agent execution cancelled.',
          status: 'cancelled',
          payload: { type: 'error', message: 'Agent execution cancelled.' },
        }),
      },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelled stream state should exist');
  assertEqual(restored.isStreaming, false, 'cancelled terminal replay stops streaming');
  assertEqual(restored.error, null, 'cancelled terminal replay does not surface as an error');

  streamStore.clearStream(conversationId);
});

test('restoreFromTaskEvents keeps cancelling task runs active until terminal event', () => {
  const conversationId = 'conversation-cancelling-restore';

  streamStore.restoreFromTaskEvents(conversationId, taskRun('cancelling'), []);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelling stream state should exist');
  assertEqual(restored.isStreaming, true, 'cancelling task run should remain active');

  streamStore.clearStream(conversationId);
});

test('dispatches canonical terminal errors without an active stream state', () => {
  const conversationId = 'conversation-no-state-terminal';

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: runEvent({
      eventSeq: 1,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution timed out.',
      status: 'failed',
      payload: { message: 'Agent execution timed out.' },
    }),
  } as AgentFrontendEvent);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'terminal event should create stream state');
  assertEqual(restored.isStreaming, false, 'terminal event stops streaming');
  assertEqual(restored.error, 'Agent execution timed out.', 'terminal event error');
  assert(
    restored.traceEvents.some(event => event.kind === 'status' && event.tone === 'error'),
    'terminal event should add an error status trace',
  );

  streamStore.clearStream(conversationId);
});

test('dispatches canonical cancelled terminal errors without surfacing failed state', () => {
  const conversationId = 'conversation-no-state-cancelled-terminal';

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: runEvent({
      eventSeq: 1,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution cancelled.',
      status: 'cancelled',
      payload: { message: 'Agent execution cancelled.' },
    }),
  } as AgentFrontendEvent);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelled terminal event should create stream state');
  assertEqual(restored.isStreaming, false, 'cancelled terminal event stops streaming');
  assertEqual(restored.error, null, 'cancelled terminal event should not set failed error');

  streamStore.clearStream(conversationId);
});

test('dispatches cancelled done terminal events without surfacing failed state', () => {
  const conversationId = 'conversation-no-state-cancelled-done-terminal';

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: runEvent({
      eventSeq: 1,
      kind: 'done',
      phase: 'done',
      label: 'Request cancelled by user.',
      status: 'cancelled',
      payload: { finishReason: 'cancelled' },
    }),
  } as AgentFrontendEvent);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelled done terminal event should create stream state');
  assertEqual(restored.isStreaming, false, 'cancelled done terminal event stops streaming');
  assertEqual(restored.error, null, 'cancelled done terminal event should not set failed error');

  streamStore.clearStream(conversationId);
});

test('live ordering ignores duplicate and late events while marking gaps', () => {
  const conversationId = 'conversation-live-ordering';
  const event = (eventSeq: number, offset: number, delta: string): AgentFrontendEvent => frontendEvent(runEvent({
    eventSeq,
    kind: 'outputDelta',
    payload: {
      blockId: 'answer-block',
      channel: 'answer',
      offset,
      delta,
    },
  }));

  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, event(1, 0, 'A'));
  streamStore.dispatch(conversationId, event(1, 1, 'duplicate'));
  streamStore.dispatch(conversationId, event(3, 1, 'C'));
  streamStore.dispatch(conversationId, event(2, 2, 'late'));

  const state = streamStore.getStream(conversationId);
  assert(state, 'ordered stream state should exist');
  assertEqual(state.streamText, 'AC', 'duplicate and late deltas are ignored');
  assert(
    state.traceEvents.some(trace =>
      trace.kind === 'status' && trace.text.includes('Stream event gap detected')),
    'gap should be marked in trace events',
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
