import { adaptFrontendRunEvent } from '../src/lib/streaming/legacyAdapter';
import {
  durableReplayItemsFromRunEvents,
  durableReplayItemsFromTaskEvents,
  projectHistoricalEventsToStreamState,
  projectRunEventsToStreamState,
  taskTimelineEventsFromReplaySource,
} from '../src/lib/streaming/durableReplay';
import { extractPersistedTraceItems } from '../src/lib/streaming/persistedTrace';
import {
  isPendingToolCallStatus,
  normalizePersistedToolCallStatus,
} from '../src/lib/streaming/toolStatus';
import {
  getStableFileChangeTarget,
  getToolBriefTarget,
  getToolTitleTarget,
} from '../src/lib/streaming/toolCardPresentation';
import {
  buildCurrentTimelineSections,
  buildLiveTraceTimeline,
  shouldHideTraceStatus,
  shouldRenderTraceToolCall,
  skillNamesFromTraceItems,
  skillRefsFromTraceItems,
  traceEventsAfterStreamRounds,
  turnLifecycleTimelineSections,
  visibleTraceEventsForTimeline,
} from '../src/lib/streaming/timelineViewModel';
import { armStreamWatchdog, clearStreamWatchdog } from '../src/lib/streaming/watchdog';
import { streamStore } from '../src/lib/streamStore';
import {
  isTaskTimelineEvent,
  taskTimelinePayloadFromTaskEvent,
} from '../src/lib/streaming/taskTimeline';
import {
  legacyTaskCenterHistoryFromTaskEvents,
  taskCenterHistoryFromEvents,
  taskCenterHistoryFromRunEvents,
} from '../src/lib/streaming/taskCenterHistory';
import type {
  AgentFrontendEvent,
  ConversationTurn,
  AgentRunEvent,
  AgentRunEventKind,
  AgentRunPhase,
  AgentTaskRun,
  AgentTaskRunEvent,
  ToolRunItem,
} from '../src/types/conversation';
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
} from '../src/lib/streaming/protocol';
import type { WorkflowAutomationSchedulerEvent } from '../src/types/workflows';

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
    createdAt: `2026-01-01T00:00:${String(input.eventSeq).padStart(2, '0')}.000Z`,
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

function schedulerEvent(input: {
  id: string;
  eventType: string;
  status?: string | null;
  summary?: string;
  createdAt?: string;
}): WorkflowAutomationSchedulerEvent {
  return {
    id: input.id,
    automationId: 'automation-1',
    runId: 'workflow-run-1',
    eventType: input.eventType,
    status: input.status ?? null,
    summary: input.summary ?? input.eventType,
    payload: { queueId: 'workflow_due:automation-1' },
    createdAt: input.createdAt ?? '2026-01-01T00:00:04.500Z',
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

function approvalRequest(id = 'approval-1'): NonNullable<AgentFrontendEvent['request']> {
  return {
    id,
    toolName: 'write_file',
    permissionKey: 'file:write',
    targetKind: 'file',
    targetValue: 'README.md',
    argumentsPreview: '{}',
    riskLevel: 'high',
    reason: 'Writes to workspace',
  };
}

function traceToolCall(input: {
  callId: string;
  toolName?: string;
  status?: ToolCallEvent['status'];
}): ToolCallEvent {
  return {
    callId: input.callId,
    toolName: input.toolName ?? 'run_shell',
    arguments: '{}',
    status: input.status ?? 'done',
    argsStatus: 'done',
    argsBytes: 2,
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

test('adapts canonical approval events without a legacy envelope type', () => {
  const request = approvalRequest();
  const requested = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 9,
    kind: 'approvalRequested',
    phase: 'approval',
    label: 'write_file',
    status: 'pending',
    payload: { request },
  })));
  const resolved = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 10,
    kind: 'approvalResolved',
    phase: 'approval',
    label: 'Approval resolved',
    status: 'denied',
    payload: { requestId: request.id, decision: 'deny' },
  })));

  assertEqual(requested.type, 'approvalRequested', 'requested event type');
  assertEqual(requested.request?.id, request.id, 'requested approval id');
  assertEqual(resolved.type, 'approvalResolved', 'resolved event type');
  assertEqual(resolved.requestId, request.id, 'resolved approval id');
  assertEqual(resolved.decision, 'deny', 'resolved decision');
});

test('adapts canonical tool run events without a legacy envelope type', () => {
  const startedRun = toolRun({ callId: 'call-approval', status: 'approvalPending' });
  const declinedRun = toolRun({
    callId: 'call-approval',
    status: 'declined',
    content: 'Denied by user',
  });
  const started = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 11,
    kind: 'toolStarted',
    phase: 'tooling',
    label: 'search_knowledge_base',
    status: 'approval_pending',
    payload: { run: startedRun },
  })));
  const completed = adaptFrontendRunEvent(frontendEvent(runEvent({
    eventSeq: 12,
    kind: 'toolCompleted',
    phase: 'tooling',
    label: 'search_knowledge_base',
    status: 'declined',
    payload: { run: declinedRun },
  })));

  assertEqual(started.type, 'toolRunStarted', 'started event type');
  assertEqual(started.run?.callId, 'call-approval', 'started tool run call id');
  assertEqual(started.run?.status, 'approvalPending', 'started tool run status');
  assertEqual(completed.type, 'toolRunCompleted', 'completed event type');
  assertEqual(completed.run?.callId, 'call-approval', 'completed tool run call id');
  assertEqual(completed.run?.status, 'declined', 'completed tool run status');
  assertEqual(completed.run?.content, 'Denied by user', 'completed tool run content');
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

test('builds durable replay items directly from canonical run events in eventSeq order', () => {
  const replay = durableReplayItemsFromRunEvents([
    runEvent({
      eventSeq: 3,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: { message: 'done', usageTotal: { totalTokens: 4 } },
    }),
    runEvent({
      eventSeq: 1,
      kind: 'status',
      phase: 'routing',
      label: 'Task queued',
      status: 'queued',
      payload: { content: 'Task queued' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'outputDelta',
      payload: {
        blockId: 'canonical-block',
        channel: 'answer',
        offset: 0,
        delta: 'hello',
      },
    }),
  ]);

  assertEqual(replay.length, 2, 'lifecycle status should not replay');
  assertEqual(replay[0].eventSeq, 2, 'first replayed eventSeq');
  assertEqual(replay[0].eventType, 'streamBlockDelta', 'first replay type');
  assertEqual(replay[1].eventSeq, 3, 'terminal eventSeq');
  assertEqual(replay[1].eventType, 'terminal', 'terminal replay type');
});

test('historical stream projection prefers canonical run events over legacy task event replay', () => {
  const projected = projectHistoricalEventsToStreamState(
    taskRun('completed'),
    [
      taskEvent({
        id: 'legacy-output',
        eventType: 'stream',
        eventSeq: 1,
        payload: {
          agentRun: runEvent({
            eventSeq: 1,
            kind: 'outputDelta',
            payload: {
              blockId: 'legacy-block',
              channel: 'answer',
              offset: 0,
              delta: 'legacy',
            },
          }),
        },
      }),
      taskEvent({
        id: 'timeline',
        eventType: 'status',
        eventSeq: 2,
        label: 'Timeline status',
        status: 'running',
      }),
    ],
    [
      runEvent({
        eventSeq: 1,
        kind: 'outputDelta',
        payload: {
          blockId: 'canonical-block',
          channel: 'answer',
          offset: 0,
          delta: 'canonical',
        },
      }),
      runEvent({
        eventSeq: 2,
        kind: 'done',
        phase: 'done',
        label: 'Final answer produced',
        status: 'completed',
        payload: { message: 'done', usageTotal: { totalTokens: 4 } },
      }),
    ],
  );

  assertEqual(projected.streamRounds[0]?.reply, 'canonical', 'canonical output should win');
  assertEqual(projected.taskEvents.length, 1, 'timeline task event remains available');
  assertEqual(projected.taskEvents[0].id, 'timeline', 'timeline task event id');
});

test('historical projection marks stale active task runs as interrupted', () => {
  const projected = projectHistoricalEventsToStreamState(
    taskRun('running'),
    [],
    [
      runEvent({
        eventSeq: 1,
        kind: 'thinking',
        payload: { type: 'thinking', content: 'Working before app close.' },
      }),
    ],
  );

  assertEqual(projected.isStreaming, false, 'stale active historical run is not live');
  assertEqual(projected.taskRun?.status, 'cancelled', 'stale task run status');
  assertEqual(projected.error, null, 'interrupted stale run should not toast as a failure');
  assert(
    projected.traceEvents.some(event =>
      event.kind === 'status' &&
      event.text === 'Previous run interrupted when the app closed.'),
    'interrupted status trace should be visible in restored history',
  );
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

test('task center history consumes canonical run events and hides stream-only noise', () => {
  const history = taskCenterHistoryFromRunEvents(
    [
      runEvent({
        eventSeq: 1,
        kind: 'status',
        phase: 'routing',
        label: 'Task queued',
        status: 'queued',
        payload: { content: 'Task queued' },
      }),
      runEvent({
        eventSeq: 2,
        kind: 'outputDelta',
        payload: { blockId: 'b', channel: 'answer', offset: 0, delta: 'hidden' },
      }),
      runEvent({
        eventSeq: 3,
        kind: 'toolCompleted',
        phase: 'tooling',
        label: 'search_knowledge_base',
        status: 'completed',
        payload: { toolName: 'search_knowledge_base' },
      }),
      runEvent({
        eventSeq: 4,
        kind: 'usageUpdated',
        phase: 'accounting',
        label: 'Usage updated',
        status: null,
        payload: { usageTotal: { totalTokens: 4 }, lastPromptTokens: 2 },
      }),
      runEvent({
        eventSeq: 6,
        kind: 'done',
        phase: 'done',
        label: 'Final answer produced',
        status: 'completed',
        payload: { message: 'done', usageTotal: { totalTokens: 4 } },
      }),
    ],
    [
      taskEvent({
        id: 'legacy-status',
        eventType: 'status',
        eventSeq: 1,
        label: 'Legacy queued',
        status: 'queued',
      }),
      taskEvent({
        id: 'subtask',
        eventType: 'subtask',
        eventSeq: 5,
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
    ],
  );

  assertEqual(history.length, 4, 'visible history count');
  assertEqual(history[0].source, 'agentRun', 'canonical source is preferred');
  assertEqual(history[0].label, 'Task queued', 'canonical label');
  assertEqual(history[1].label, 'search_knowledge_base', 'tool history is visible');
  assertEqual(history[2].source, 'taskEvent', 'task timeline is preserved');
  assertEqual(history[3].eventType, 'done', 'terminal history is visible');
});

test('task center history surfaces scheduler events beside run and timeline events', () => {
  const history = taskCenterHistoryFromRunEvents(
    [
      runEvent({
        eventSeq: 1,
        kind: 'status',
        phase: 'routing',
        label: 'Task queued',
        status: 'queued',
        payload: { content: 'Task queued' },
      }),
      runEvent({
        eventSeq: 4,
        kind: 'done',
        phase: 'done',
        label: 'Final answer produced',
        status: 'completed',
        payload: { message: 'done' },
      }),
    ],
    [
      taskEvent({
        id: 'subtask',
        eventType: 'subtask',
        eventSeq: 3,
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
    ],
    [
      schedulerEvent({
        id: 'scheduler-claimed',
        eventType: 'claimed',
        status: 'queued',
        summary: 'Scheduler claimed due workflow',
        createdAt: '2026-01-01T00:00:02.500Z',
      }),
    ],
  );

  assertEqual(history.length, 4, 'visible history count');
  assertEqual(history[0].source, 'agentRun', 'canonical source');
  assertEqual(history[1].source, 'schedulerEvent', 'scheduler source');
  assertEqual(history[1].eventType, 'claimed', 'scheduler event type');
  assertEqual(history[1].label, 'Scheduler claimed due workflow', 'scheduler label');
  assertEqual(history[2].source, 'taskEvent', 'timeline source');
  assertEqual(history[3].eventType, 'done', 'terminal history');
});

test('task center history falls back to legacy task events when canonical events are absent', () => {
  const legacyEvents = [
    taskEvent({
      id: 'stream',
      eventType: 'stream',
      eventSeq: 1,
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
      eventSeq: 2,
      label: 'Retrying',
      payload: {
        agentRun: runEvent({
          eventSeq: 2,
          kind: 'recoveryAttempt',
          label: 'Retrying',
          payload: { reason: 'Retrying' },
        }),
      },
    }),
    taskEvent({
      id: 'tool',
      eventType: 'tool',
      eventSeq: 3,
      label: 'read_files',
      status: 'completed',
      payload: { toolName: 'read_files' },
    }),
  ];
  const directLegacy = legacyTaskCenterHistoryFromTaskEvents(legacyEvents);
  const history = taskCenterHistoryFromEvents(
    [
      ...legacyEvents,
    ],
    [],
    [
      schedulerEvent({
        id: 'scheduler-launch-failed',
        eventType: 'launch_failed',
        status: 'failed',
        summary: 'Scheduler failed to launch due workflow',
        createdAt: '2026-01-01T00:00:04.000Z',
      }),
    ],
  );

  assertEqual(history.length, 3, 'legacy visible history count');
  assertEqual(history[0].id, 'recovery', 'legacy recovery remains visible');
  assertEqual(history[1].id, 'tool', 'legacy tool event remains visible');
  assertEqual(history[2].source, 'schedulerEvent', 'scheduler event remains visible');
  assertEqual(directLegacy.length, 2, 'legacy adapter keeps task-only history');
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

test('durable replay restores direct canonical run events through live projection', () => {
  const conversationId = 'conversation-run-event-replay';

  streamStore.restoreFromRunEvents(conversationId, taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'answer-block', channel: 'answer', offset: 0, delta: 'Checking' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'running',
      payload: { type: 'toolRunStarted', run: toolRun({ callId: 'call-1', status: 'running' }) },
    }),
    runEvent({
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
    runEvent({
      eventSeq: 4,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: { message: 'done', usageTotal: { promptTokens: 1, completionTokens: 1, totalTokens: 2 } },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'run event replay should create stream state');
  assertEqual(restored.isStreaming, false, 'completed run should not remain streaming');
  assertEqual(restored.toolCalls.length, 1, 'tool call count');
  assertEqual(restored.toolCalls[0].status, 'done', 'tool status');
  assertEqual(restored.toolCalls[0].content, 'Found 2 notes', 'tool content');
  assertEqual(restored.streamRounds.length, 1, 'final answer round count');
  assertEqual(restored.streamRounds[0].reply, 'Checking', 'streamed reply');

  streamStore.clearStream(conversationId);
});

test('canonical run event projection matches live stream dispatch for render state', () => {
  const conversationId = 'conversation-live-replay-equivalence';
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'answer-block', channel: 'answer', offset: 0, delta: 'Checking' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'running',
      payload: { type: 'toolRunStarted', run: toolRun({ callId: 'call-1', status: 'running' }) },
    }),
    runEvent({
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
    runEvent({
      eventSeq: 4,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: {
        message: 'done',
        usageTotal: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
      },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('completed'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');
  assertEqual(live.streamRounds.length, projected.streamRounds.length, 'round count equivalence');
  assertEqual(live.streamRounds[0].reply, projected.streamRounds[0].reply, 'reply equivalence');
  assertEqual(live.toolCalls.length, projected.toolCalls.length, 'tool count equivalence');
  assertEqual(live.toolCalls[0].status, projected.toolCalls[0].status, 'tool status equivalence');
  assertEqual(live.toolCalls[0].content, projected.toolCalls[0].content, 'tool content equivalence');
  assertEqual(live.lastUsage?.totalTokens, projected.lastUsage?.totalTokens, 'usage equivalence');

  streamStore.clearStream(conversationId);
});

test('canonical auto-compaction projection matches live stream dispatch summary', () => {
  const conversationId = 'conversation-auto-compaction-equivalence';
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'autoCompacted',
      phase: 'compacting',
      label: 'Context compacted',
      payload: { summary: 'Earlier turns were summarized.' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: { message: 'done' },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('completed'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.autoCompacted?.summary, 'Earlier turns were summarized.', 'projected auto compact summary');
  assertEqual(live.autoCompacted?.summary, projected.autoCompacted?.summary, 'auto compact summary equivalence');
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');

  streamStore.clearStream(conversationId);
});

test('canonical approval and recovery projection matches live stream dispatch state', () => {
  const conversationId = 'conversation-approval-recovery-equivalence';
  const request = approvalRequest();
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'recoveryAttempt',
      phase: 'responding',
      label: 'Retrying stream',
      status: 'recovering',
      payload: { reason: 'Retrying stream' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'approvalRequested',
      phase: 'approval',
      label: 'write_file',
      status: 'pending',
      payload: { request },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('waiting_approval'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.pendingApprovals.length, 1, 'projected pending approval count');
  assertEqual(live.pendingApprovals.length, projected.pendingApprovals.length, 'pending approval count equivalence');
  assertEqual(live.pendingApprovals[0].id, projected.pendingApprovals[0].id, 'pending approval id equivalence');
  assertEqual(live.pendingApprovals[0].toolName, projected.pendingApprovals[0].toolName, 'pending approval tool equivalence');
  assert(
    projected.traceEvents.some(event => event.kind === 'status' && event.text === 'Retrying stream'),
    'projected recovery status should be visible',
  );
  assert(
    live.traceEvents.some(event => event.kind === 'status' && event.text === 'Retrying stream'),
    'live recovery status should be visible',
  );
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');

  streamStore.clearStream(conversationId);
});

test('canonical stream reset projection matches live stream dispatch recovered state', () => {
  const conversationId = 'conversation-stream-reset-equivalence';
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'answer-before-reset', channel: 'answer', offset: 0, delta: 'Checking' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'running',
      payload: { run: toolRun({ callId: 'call-reset', status: 'running' }) },
    }),
    runEvent({
      eventSeq: 3,
      kind: 'streamReset',
      phase: 'responding',
      label: 'Stream interrupted; retrying without streaming.',
      status: 'running',
      payload: { reason: 'Stream interrupted; retrying without streaming.' },
    }),
    runEvent({
      eventSeq: 4,
      kind: 'outputDelta',
      payload: { blockId: 'answer-after-reset', channel: 'answer', offset: 0, delta: 'Recovered' },
    }),
    runEvent({
      eventSeq: 5,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: { message: 'done' },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('completed'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.toolCalls.length, 0, 'projected stream reset clears active stale tools');
  assertEqual(live.toolCalls.length, projected.toolCalls.length, 'tool reset equivalence');
  assertEqual(live.streamRounds.length, projected.streamRounds.length, 'recovered round count equivalence');
  assertEqual(live.streamRounds[0].reply, projected.streamRounds[0].reply, 'pre-reset reply preservation equivalence');
  assertEqual(live.streamRounds[0].reply, 'Checking', 'stream reset should preserve pre-reset reply history');
  assertEqual(live.streamRounds[0].toolCalls[0].status, 'cancelled', 'pre-reset pending tool should be marked cancelled');
  assert(
    live.traceEvents.some(event => event.kind === 'reply' && event.text === 'Checking'),
    'stream reset should preserve pre-reset reply trace',
  );
  assert(
    live.traceEvents.some(event => event.kind === 'reply' && event.text === 'Recovered'),
    'stream reset should include recovered reply trace',
  );
  assert(
    projected.traceEvents.some(event =>
      event.kind === 'status' && event.text === 'Stream interrupted; retrying without streaming.'),
    'projected stream reset status should be visible',
  );
  assert(
    live.traceEvents.some(event =>
      event.kind === 'status' && event.text === 'Stream interrupted; retrying without streaming.'),
    'live stream reset status should be visible',
  );
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');

  streamStore.clearStream(conversationId);
});

test('canonical cancellation projection matches live stream dispatch terminal state', () => {
  const conversationId = 'conversation-cancellation-equivalence';
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'answer-block', channel: 'answer', offset: 0, delta: 'Working' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution cancelled.',
      status: 'cancelled',
      payload: { message: 'Agent execution cancelled.' },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('cancelled'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.isStreaming, false, 'projected cancellation stops streaming');
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');
  assertEqual(live.streamText, projected.streamText, 'partial output equivalence');
  assertEqual(live.error, projected.error, 'cancelled terminal error equivalence');
  assertEqual(live.error, null, 'cancelled terminal should not surface failed state');
  assert(
    projected.traceEvents.some(event => event.kind === 'status' && event.text === 'Agent execution cancelled.'),
    'projected cancellation status should be visible',
  );
  assert(
    live.traceEvents.some(event => event.kind === 'status' && event.text === 'Agent execution cancelled.'),
    'live cancellation status should be visible',
  );

  streamStore.clearStream(conversationId);
});

test('canonical tool execution cancellation projection matches live stream dispatch state', () => {
  const conversationId = 'conversation-tool-cancellation-equivalence';
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'answer-block', channel: 'answer', offset: 0, delta: 'Working' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'running',
      payload: { run: toolRun({ callId: 'call-cancel', status: 'running' }) },
    }),
    runEvent({
      eventSeq: 3,
      kind: 'done',
      phase: 'done',
      label: 'Request cancelled by user.',
      status: 'cancelled',
      payload: { finishReason: 'cancelled' },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('cancelled'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.toolCalls.length, 1, 'projected tool call count');
  assertEqual(live.toolCalls.length, projected.toolCalls.length, 'tool call count equivalence');
  assertEqual(live.toolCalls[0].status, projected.toolCalls[0].status, 'cancelled tool status equivalence');
  assertEqual(live.toolCalls[0].status, 'cancelled', 'cancelled terminal should cancel running tool');
  assertEqual(live.toolCalls[0].content, projected.toolCalls[0].content, 'cancelled tool content equivalence');
  assertEqual(live.toolCalls[0].content, 'Cancelled', 'cancelled tool should receive fallback content');
  assertEqual(live.streamRounds.length, projected.streamRounds.length, 'round count equivalence');
  assertEqual(live.streamRounds[0].toolCalls[0].status, projected.streamRounds[0].toolCalls[0].status, 'round tool status equivalence');
  assertEqual(live.error, projected.error, 'cancelled terminal error equivalence');
  assertEqual(live.error, null, 'cancelled tool execution should not surface failed state');
  assertEqual(live.finishReason, projected.finishReason, 'finish reason equivalence');
  assertEqual(live.finishReason, 'cancelled', 'finish reason should be preserved');
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');

  streamStore.clearStream(conversationId);
});

test('canonical approval denial projection matches live stream dispatch declined tool state', () => {
  const conversationId = 'conversation-approval-denial-equivalence';
  const request = approvalRequest();
  const runEvents = [
    runEvent({
      eventSeq: 1,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'approval_pending',
      payload: { run: toolRun({ callId: 'call-approval', status: 'approvalPending' }) },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'approvalRequested',
      phase: 'approval',
      label: 'search_knowledge_base',
      status: 'pending',
      payload: { request },
    }),
    runEvent({
      eventSeq: 3,
      kind: 'approvalResolved',
      phase: 'approval',
      label: 'Approval resolved',
      status: 'denied',
      payload: { requestId: request.id, decision: 'deny' },
    }),
    runEvent({
      eventSeq: 4,
      kind: 'toolCompleted',
      phase: 'tooling',
      label: 'search_knowledge_base',
      status: 'declined',
      payload: {
        run: toolRun({
          callId: 'call-approval',
          status: 'declined',
          content: 'Denied by user',
        }),
      },
    }),
    runEvent({
      eventSeq: 5,
      kind: 'error',
      phase: 'done',
      label: 'Tool approval denied.',
      status: 'failed',
      payload: { message: 'Tool approval denied.' },
    }),
  ];

  const projected = projectRunEventsToStreamState(taskRun('failed'), runEvents);
  streamStore.startStream(conversationId);
  for (const event of runEvents) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }

  const live = streamStore.getStream(conversationId);
  assert(live, 'live dispatch should create stream state');
  assertEqual(projected.pendingApprovals.length, 0, 'projected pending approval cleared');
  assertEqual(live.pendingApprovals.length, projected.pendingApprovals.length, 'pending approval count equivalence');
  assertEqual(projected.toolCalls.length, 1, 'projected tool call count');
  assertEqual(live.toolCalls.length, projected.toolCalls.length, 'tool call count equivalence');
  assertEqual(live.toolCalls[0].status, projected.toolCalls[0].status, 'declined tool status equivalence');
  assertEqual(live.toolCalls[0].status, 'declined', 'denied approval should decline tool run');
  assertEqual(live.toolCalls[0].content, projected.toolCalls[0].content, 'declined tool content equivalence');
  assertEqual(live.error, projected.error, 'terminal error equivalence');
  assertEqual(live.isStreaming, projected.isStreaming, 'streaming state equivalence');

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

test('timeline view model ignores skill index selections for loaded skill summaries', () => {
  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [
      {
        kind: 'skillSelection',
        skills: [
          {
            id: 'builtin-fiction-writing',
            name: 'fiction-writing',
            displayName: 'Fiction Writing',
          },
        ],
      },
    ],
  });

  assert(items, 'persisted skill selection trace should parse');
  const names = skillNamesFromTraceItems(items);
  assertEqual(names.length, 0, 'indexed skill names are not counted as loaded');
  const skillRefs = skillRefsFromTraceItems(items);
  assertEqual(skillRefs.length, 0, 'indexed skill refs are not counted as loaded');

  const turn: ConversationTurn = {
    id: 'turn-selected',
    conversationId: 'conversation-1',
    userMessageId: 'user-selected',
    assistantMessageId: 'assistant-selected',
    status: 'success',
    trace: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:01.000Z',
    finishedAt: '2026-01-01T00:00:01.000Z',
  };

  const sections = turnLifecycleTimelineSections({
    turn,
    routeKind: 'DirectResponse',
    traceItems: items,
  });

  const skillSection = sections.find((section) => section.id === 'turn-skills-turn-selected');
  assert(skillSection, 'turn lifecycle should include loaded skill summary');
  if (skillSection.kind !== 'status') {
    throw new Error(`skill summary should be a status section, got ${skillSection.kind}`);
  }
  assertEqual(skillSection.text, 'Skills: none', 'indexed-only skill summary text');
  assertEqual(skillSection.tone, 'muted', 'indexed-only skill summary tone');
});

test('timeline view model counts auto-loaded skill selections', () => {
  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [
      {
        kind: 'skillSelection',
        skills: [
          {
            id: 'builtin-fiction-writing',
            name: 'fiction-writing',
            displayName: 'Fiction Writing',
            activated: true,
          },
        ],
      },
    ],
  });

  assert(items, 'persisted auto-loaded skill trace should parse');
  const names = skillNamesFromTraceItems(items);
  assertEqual(names.length, 1, 'auto-loaded skill names are counted');
  assertEqual(names[0], 'Fiction Writing', 'auto-loaded skill display name');
  const skillRefs = skillRefsFromTraceItems(items);
  assertEqual(skillRefs.length, 1, 'auto-loaded skill refs are counted');
  assertEqual(skillRefs[0].activated, true, 'auto-loaded skill ref is marked loaded');

  const turn: ConversationTurn = {
    id: 'turn-auto-loaded',
    conversationId: 'conversation-1',
    userMessageId: 'user-auto-loaded',
    assistantMessageId: 'assistant-auto-loaded',
    status: 'success',
    trace: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:01.000Z',
    finishedAt: '2026-01-01T00:00:01.000Z',
  };

  const sections = turnLifecycleTimelineSections({
    turn,
    routeKind: 'DirectResponse',
    traceItems: items,
  });

  const skillSection = sections.find((section) => section.id === 'turn-skills-turn-auto-loaded');
  assert(skillSection, 'turn lifecycle should include auto-loaded skill summary');
  if (skillSection.kind !== 'status') {
    throw new Error(`skill summary should be a status section, got ${skillSection.kind}`);
  }
  assertEqual(skillSection.text, 'Skills: Fiction Writing', 'auto-loaded skill summary text');
  assertEqual(skillSection.tone, 'success', 'auto-loaded skill summary tone');
});

test('timeline view model dedupes loaded skills while ignoring index selections', () => {
  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [
      {
        kind: 'skillSelection',
        skills: [
          {
            id: 'builtin-frontend-design',
            name: 'frontend-design',
            displayName: 'Frontend Design',
          },
        ],
      },
      {
        kind: 'tool',
        toolCall: {
          callId: 'skill-call-1',
          toolName: 'manage_skill',
          arguments: '{"action":"activate_skill","skill_id":"frontend-design"}',
          status: 'done',
          artifacts: {
            kind: 'skillActivation',
            skill: {
              id: 'builtin-frontend-design',
              name: 'frontend-design',
              interface: {
                displayName: 'Frontend Design',
              },
            },
          },
        },
      },
      {
        kind: 'tool',
        toolCall: {
          callId: 'skill-call-2',
          toolName: 'manage_skill',
          arguments: '{"action":"view_skill","skill_id":"frontend-design"}',
          status: 'done',
          artifacts: {
            kind: 'skill',
            skill: {
              id: 'builtin-frontend-design',
              name: 'frontend-design',
            },
          },
        },
      },
    ],
  });

  assert(items, 'persisted skill activation trace should parse');
  const names = skillNamesFromTraceItems(items);
  assertEqual(names.length, 1, 'skill activation names are deduped');
  assertEqual(names[0], 'Frontend Design', 'display name is preferred');
  const skillRefs = skillRefsFromTraceItems(items);
  assertEqual(skillRefs.length, 1, 'skill activation refs are deduped');
  assertEqual(skillRefs[0].label, 'Frontend Design', 'skill activation ref label');
  assertEqual(skillRefs[0].activated, true, 'loaded skill ref is marked loaded');

  const turn: ConversationTurn = {
    id: 'turn-1',
    conversationId: 'conversation-1',
    userMessageId: 'user-1',
    assistantMessageId: 'assistant-1',
    status: 'success',
    trace: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:01.000Z',
    finishedAt: '2026-01-01T00:00:01.000Z',
  };

  const sections = turnLifecycleTimelineSections({
    turn,
    routeKind: 'DirectResponse',
    traceItems: items,
  });

  const skillSection = sections.find((section) => section.id === 'turn-skills-turn-1');
  assert(skillSection, 'turn lifecycle should include skill summary');
  if (skillSection.kind !== 'status') {
    throw new Error(`skill summary should be a status section, got ${skillSection.kind}`);
  }
  assertEqual(skillSection.text, 'Skills: Frontend Design', 'skill summary text');
  assertEqual(skillSection.tone, 'success', 'activated skill summary tone');
});

test('timeline view model reports when a traced turn activated no skills', () => {
  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [
      { kind: 'status', text: 'Running shell command', tone: 'muted' },
    ],
  });
  assert(items, 'persisted status trace should parse');

  const turn: ConversationTurn = {
    id: 'turn-2',
    conversationId: 'conversation-1',
    userMessageId: 'user-2',
    assistantMessageId: 'assistant-2',
    status: 'success',
    trace: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:01.000Z',
    finishedAt: '2026-01-01T00:00:01.000Z',
  };

  const sections = turnLifecycleTimelineSections({
    turn,
    routeKind: 'DirectResponse',
    traceItems: items,
  });

  const skillSection = sections.find((section) => section.id === 'turn-skills-turn-2');
  assert(skillSection, 'turn lifecycle should include no-skill summary');
  if (skillSection.kind !== 'status') {
    throw new Error(`no-skill summary should be a status section, got ${skillSection.kind}`);
  }
  assertEqual(skillSection.text, 'Skills: none', 'no-skill summary text');
  assertEqual(skillSection.tone, 'muted', 'no-skill summary tone');
});

test('uses one tool status reducer for run, trace, and card-facing statuses', () => {
  assertEqual(normalizePersistedToolCallStatus('completed'), 'done', 'completed status');
  assertEqual(normalizePersistedToolCallStatus('timed_out'), 'timedOut', 'timed out status');
  assert(isPendingToolCallStatus('preparing'), 'preparing is pending');
  assert(!isPendingToolCallStatus('done'), 'done is not pending');
});

test('file change tool cards use stable diff paths instead of partial streaming json as title target', () => {
  const partialArgs = '{"path":"notes/live.md","content":"first\\nsecond';

  assert(
    getToolBriefTarget(partialArgs)?.startsWith('{"path"'),
    'generic tool target still falls back to partial args for non-file tools',
  );
  assertEqual(
    getStableFileChangeTarget({ path: 'notes/live.md' }, { paths: ['notes/live.md'] }),
    'notes/live.md',
    'file change target should come from diff artifact path',
  );
});

test('command tool cards do not stream partial arguments into the title target', () => {
  const partialArgs = '{"command":"npm run test -- --watch';

  assertEqual(
    getToolTitleTarget({
      toolName: 'run_shell',
      renderKind: 'commandExecution',
      args: partialArgs,
      argsStatus: 'streaming',
    }),
    null,
    'partial command arguments should not appear in the card title',
  );
  assertEqual(
    getToolTitleTarget({
      toolName: 'run_shell',
      renderKind: 'commandExecution',
      args: '{"command":"npm run test -- --watch"}',
      argsStatus: 'ready',
    }),
    'npm run test -- --watch',
    'complete command arguments can become a stable title target',
  );
  assertEqual(
    getToolTitleTarget({
      toolName: 'run_shell',
      renderKind: 'commandExecution',
      args: '{"program":"npm","args":["run","test"]}',
      argsStatus: 'ready',
    }),
    'npm run test',
    'program and argv command arguments should be summarized together',
  );
});

test('timeline view model hides low-signal statuses and internal successful tools', () => {
  assert(shouldHideTraceStatus('Route selected: DirectResponse'), 'direct response route status should hide');
  assert(shouldHideTraceStatus('Task queued'), 'queued status should hide');
  assert(!shouldHideTraceStatus('Running shell command'), 'useful status should remain visible');
  assert(!shouldRenderTraceToolCall('tool_search', undefined, 'done', false), 'successful tool_search should hide');
  assert(shouldRenderTraceToolCall('tool_search', undefined, 'error', true), 'failed tool_search should remain visible');
  assert(!shouldRenderTraceToolCall('update_plan', 'plan', 'done', false), 'board-only plan tool should hide from trace');
});

test('timeline view model interleaves thinking, tools, and streamed replies', () => {
  const events: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'Checking files' },
    { id: 'tool-1', kind: 'tool', toolCall: traceToolCall({ callId: 'call-1' }) },
    { id: 'reply-1', kind: 'reply', text: 'Partial' },
    { id: 'reply-2', kind: 'reply', text: ' answer' },
  ];

  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    isStreaming: true,
    currentTraceActive: false,
    streamText: 'Fresh streamed answer',
    displayedText: 'Fresh streamed answer',
  });

  assertEqual(timeline.length, 3, 'live timeline item count');
  assertEqual(timeline[0].kind, 'thinking', 'first item kind');
  assert(timeline[0].kind === 'thinking', 'first item should be thinking');
  assertEqual(timeline[0].sections.length, 2, 'thinking section count');
  assertEqual(timeline[0].sections[1].kind, 'tool', 'tool section should be grouped before reply');
  assertEqual(timeline[1].kind, 'reply', 'second item kind');
  assert(timeline[1].kind === 'reply', 'second item should be reply');
  assertEqual(timeline[1].content, 'Partial answer', 'prior reply should remain visible while new text streams');
  assertEqual(timeline[1].isStreaming, false, 'prior reply should not be marked streaming');
  assertEqual(timeline[2].kind, 'reply', 'third item kind');
  assert(timeline[2].kind === 'reply', 'third item should be current streaming reply');
  assertEqual(timeline[2].content, 'Fresh streamed answer', 'streaming reply should append after existing reply');
  assertEqual(timeline[2].isStreaming, true, 'current reply remains streaming');
});

test('timeline view model typewriter text only replaces the active reply event', () => {
  const events: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'Checking files' },
    { id: 'reply-1', kind: 'reply', text: 'Fresh streamed answer' },
  ];

  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    isStreaming: true,
    currentTraceActive: false,
    streamText: 'Fresh streamed answer',
    displayedText: 'Fresh',
  });

  assertEqual(timeline.length, 2, 'active reply timeline item count');
  assertEqual(timeline[1].kind, 'reply', 'second item kind');
  assert(timeline[1].kind === 'reply', 'second item should be reply');
  assertEqual(timeline[1].content, 'Fresh', 'active reply should use typewriter text');
  assertEqual(timeline[1].isStreaming, true, 'active reply remains streaming');
});

test('timeline view model merges adjacent thinking trace events into one section', () => {
  const events: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'First thought' },
    { id: 'thinking-2', kind: 'thinking', text: 'Second thought' },
    { id: 'reply-1', kind: 'reply', text: 'Answer' },
  ];

  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    isStreaming: true,
    currentTraceActive: false,
    streamText: 'Answer',
    displayedText: 'Answer',
  });

  assertEqual(timeline.length, 2, 'merged thinking timeline item count');
  assert(timeline[0].kind === 'thinking', 'first item should be thinking');
  assertEqual(timeline[0].sections.length, 1, 'adjacent thinking events should render as one section');
  assert(timeline[0].sections[0].kind === 'thinking', 'merged section should be thinking');
  assertEqual(
    timeline[0].sections[0].text,
    'First thought\nSecond thought',
    'adjacent thinking text should be joined without visual section splitting',
  );
});

test('timeline view model merges adjacent thinking trace events into one section', () => {
  const events: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'First thought' },
    { id: 'thinking-2', kind: 'thinking', text: 'Second thought' },
    { id: 'reply-1', kind: 'reply', text: 'Answer' },
  ];

  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    isStreaming: true,
    currentTraceActive: false,
    streamText: 'Answer',
    displayedText: 'Answer',
  });

  assertEqual(timeline.length, 2, 'merged thinking timeline item count');
  assert(timeline[0].kind === 'thinking', 'first item should be thinking');
  assertEqual(timeline[0].sections.length, 1, 'adjacent thinking events should render as one section');
  assert(timeline[0].sections[0].kind === 'thinking', 'merged section should be thinking');
  assertEqual(
    timeline[0].sections[0].text,
    'First thought\nSecond thought',
    'adjacent thinking text should be joined without visual section splitting',
  );
});

test('timeline view model separates completed round trace from current trace', () => {
  const round: StreamRoundEvent = {
    id: 'round-1',
    thinking: 'Earlier thinking',
    reply: 'Earlier reply',
    toolCalls: [traceToolCall({ callId: 'round-call' })],
  };
  const events: TraceEvent[] = [
    { id: 'round-thinking', kind: 'thinking', text: 'Earlier thinking' },
    { id: 'round-tool', kind: 'tool', toolCall: traceToolCall({ callId: 'round-call' }) },
    { id: 'current-status', kind: 'status', text: 'Running shell command', tone: 'muted' },
    { id: 'current-thinking', kind: 'thinking', text: 'Reviewing output' },
  ];

  const sections = buildCurrentTimelineSections({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    streamRounds: [round],
  });

  assertEqual(sections.length, 2, 'current section count');
  assertEqual(sections[0].id, 'current-status', 'current status remains');
  assertEqual(sections[1].id, 'current-thinking', 'current thinking remains');
});

test('timeline view model keeps completed rounds when steering adds a new status', () => {
  const round: StreamRoundEvent = {
    id: 'round-1',
    thinking: 'Already investigated retries',
    reply: 'Partial answer before steering.',
    toolCalls: [traceToolCall({ callId: 'round-call' })],
  };
  const events: TraceEvent[] = [
    { id: 'round-thinking', kind: 'thinking', text: 'Already investigated retries' },
    { id: 'round-tool', kind: 'tool', toolCall: traceToolCall({ callId: 'round-call' }) },
    { id: 'steering-status', kind: 'status', text: 'Steering message received.', tone: 'muted' },
  ];

  const currentEvents = traceEventsAfterStreamRounds(
    visibleTraceEventsForTimeline(events),
    [round],
  );
  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: currentEvents,
    isStreaming: true,
    currentTraceActive: true,
    streamText: '',
    displayedText: '',
  });

  assertEqual(currentEvents.length, 1, 'only post-round steering status remains live');
  assertEqual(timeline.length, 1, 'steering status renders as current trace');
  assertEqual(timeline[0].kind, 'thinking', 'status renders inside a trace block');
  assert(timeline[0].kind === 'thinking', 'timeline item should be trace sections');
  assertEqual(timeline[0].sections[0].id, 'steering-status', 'steering status remains visible');
  assertEqual(round.reply, 'Partial answer before steering.', 'completed round reply is preserved separately');
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
