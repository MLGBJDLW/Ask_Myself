import {
  projectRunEventsToStreamState,
  taskTimelineEventsFromReplaySource,
} from '../src/lib/streaming/durableReplay';
import {
  extractPersistedTraceItems,
  isPersistedReasoningOnlyAssistant,
} from '../src/lib/streaming/persistedTrace';
import {
  isPendingToolCallStatus,
  normalizePersistedToolCallStatus,
} from '../src/lib/streaming/toolStatus';
import {
  formatToolArgumentsForDisplay,
  getStableFileChangeTarget,
  getToolBriefTarget,
  getToolInputPresentation,
  getToolTitleTarget,
} from '../src/lib/streaming/toolCardPresentation';
import {
  buildCollapsedLiveTrace,
  buildCurrentTimelineSections,
  buildLiveTraceTimeline,
  formatTurnDuration,
  normalizeThinking,
  persistedTraceItemToTimelineSections,
  projectLiveConversationTimeline,
  shouldRenderTraceToolCall,
  skillNamesFromTraceItems,
  skillRefsFromTraceItems,
  traceEventsAfterStreamRounds,
  turnLifecycleTimelineSections,
  visibleTraceEventsForTimeline,
} from '../src/lib/streaming/timelineViewModel';
import { formatElapsedDuration, resolveElapsedDurationMs } from '../src/lib/useElapsedTime';
import { armStreamWatchdog, clearStreamWatchdog } from '../src/lib/streaming/watchdog';
import { streamStore } from '../src/lib/streamStore';
import { ConversationFrameBatcher } from '../src/lib/streaming/frameBatcher';
import { parseAgentFrontendEvent } from '../src/lib/streaming/runEventWire';
import { applyStreamBlockDelta } from '../src/lib/streaming/blockProjection';
import { createDefaultState } from '../src/lib/streaming/state';
import { upsertBoundedConversationCache } from '../src/lib/boundedConversationCache';
import { buildAgentChatRequest } from '../src/lib/agentChatRequest';
import {
  invalidateTaskCheckpointLoadState,
  resumableCheckpointForTask,
} from '../src/lib/taskResume';
import { agentTurnStateSuspendsStream } from '../src/lib/streaming/runEventLifecycle';
import {
  isTaskTimelineEvent,
  taskTimelinePayloadFromTaskEvent,
} from '../src/lib/streaming/taskTimeline';
import {
  taskCenterHistoryFromEvents,
  taskCenterHistoryFromRunEvents,
} from '../src/lib/streaming/taskCenterHistory';
import type {
  AgentFrontendEvent,
  ConversationMessage,
  ConversationTurn,
  AgentRunEvent,
  AgentRunEventKind,
  AgentRunPhase,
  AgentTaskRun,
  AgentTaskRunEvent,
  ApprovalRequest,
  ToolRunItem,
} from '../src/types/conversation';
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
} from '../src/lib/streaming/protocol';
import type { TaskResumeCheckpoint, WorkflowAutomationSchedulerEvent } from '../src/types/workflows';

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

test('checkpoint resume request uses a deterministic idempotency key and camelCase checkpoint id', () => {
  const request = buildAgentChatRequest({
    conversationId: 'conversation-1',
    message: 'Continue from the checkpoint',
    resumeCheckpointId: ' checkpoint-7 ',
  }, () => 'random-id-must-not-be-used');

  assertEqual(request.idempotencyKey, 'task-resume:checkpoint-7', 'resume idempotency key');
  assertEqual(request.resumeCheckpointId, 'checkpoint-7', 'checkpoint id should be normalized');
  assert(
    Object.prototype.hasOwnProperty.call(request, 'resumeCheckpointId'),
    'request should expose resumeCheckpointId in camelCase',
  );
});

test('checkpoint resume and interaction continuation cannot share one agent request', () => {
  let rejected = false;
  try {
    buildAgentChatRequest({
      conversationId: 'conversation-1',
      message: 'Ambiguous continuation',
      resumeCheckpointId: 'checkpoint-7',
      userArtifacts: {
        kind: 'questionResponse',
        version: 2,
        interactionId: 'interaction-9',
      },
    });
  } catch {
    rejected = true;
  }
  assert(rejected, 'ambiguous continuation must be rejected');
});

test('Task Center only resumes a paused run through its own latest checkpoint', () => {
  const checkpoint: TaskResumeCheckpoint = {
    id: 'checkpoint-7',
    runId: 'run-1',
    reason: 'Paused by user',
    status: 'paused',
    phase: 'paused',
    state: null,
    resumePrompt: 'Continue',
    createdAt: '2026-08-10T00:00:00.000Z',
  };

  assertEqual(
    resumableCheckpointForTask('run-1', 'paused', [checkpoint]),
    checkpoint,
    'paused run should expose its matching latest checkpoint',
  );
  assertEqual(
    resumableCheckpointForTask('run-1', 'completed', [checkpoint]),
    null,
    'terminal run with an old checkpoint should use Retry instead of Resume',
  );
  assertEqual(
    resumableCheckpointForTask('run-2', 'paused', [checkpoint]),
    null,
    'checkpoint from another run must not enable Resume',
  );
});

test('pausing invalidates an already-loaded empty checkpoint cache so the paused run reloads it', () => {
  const cache = new Map<string, {
    loaded: Set<string>;
    resumeCheckpoints?: TaskResumeCheckpoint[];
    untouched: string;
  }>([[
    'run-1',
    {
      loaded: new Set(['history', 'checkpoint']),
      resumeCheckpoints: [],
      untouched: 'preserved',
    },
  ]]);
  const autoLoadedRuns = new Set(['run-1']);

  invalidateTaskCheckpointLoadState(cache, autoLoadedRuns, 'run-1');

  const invalidated = cache.get('run-1');
  assert(invalidated, 'the remaining task detail cache should be preserved');
  assert(!invalidated.loaded.has('checkpoint'), 'checkpoint panel must become loadable again');
  assertEqual(invalidated.loaded.has('history'), true, 'unrelated loaded panels stay cached');
  assertEqual(invalidated.resumeCheckpoints, undefined, 'the cached empty checkpoint list is removed');
  assertEqual(invalidated.untouched, 'preserved', 'unrelated cached details stay intact');
  assertEqual(autoLoadedRuns.has('run-1'), false, 'paused-run autoload may run again');
});

test('paused launch handles are resumable stream suspensions', () => {
  assertEqual(agentTurnStateSuspendsStream('paused'), true, 'paused handle suspends transport');
  assertEqual(
    agentTurnStateSuspendsStream('awaitingUserInput'),
    true,
    'awaiting-input handle also suspends transport',
  );
  assertEqual(agentTurnStateSuspendsStream('running'), false, 'running handle remains live');

  const conversationId = 'conversation-paused-launch-handle';
  streamStore.startStream(conversationId);
  const handle = {
    sessionId: conversationId,
    runId: 'run-paused-handle',
    turnId: 'turn-paused-handle',
    state: 'paused' as const,
  };
  streamStore.bindTurnHandle(conversationId, handle);
  if (agentTurnStateSuspendsStream(handle.state)) {
    streamStore.markResumableSuspension(conversationId);
  }

  const suspended = streamStore.getStream(conversationId);
  assert(suspended, 'paused launch state should remain addressable');
  assertEqual(suspended.isStreaming, false, 'paused launch handle settles live streaming');
  assertEqual(suspended.isThinking, false, 'paused launch handle settles thinking');
  assertEqual(suspended.turnHandle?.runId, handle.runId, 'paused launch retains its resumable run');
  streamStore.clearStream(conversationId);
});

test('legacy missing-reasoning sentinel is never rendered as thinking', () => {
  assertEqual(
    normalizeThinking('[reasoning content unavailable in local history]'),
    '',
    'legacy sentinel should be hidden',
  );
  assertEqual(
    normalizeThinking('  captured reasoning  '),
    'captured reasoning',
    'real reasoning remains visible',
  );
});

function runEvent(input: {
  eventSeq: number;
  kind: AgentRunEventKind;
  payload?: AgentRunEvent['payload'];
  phase?: AgentRunPhase;
  label?: string;
  status?: string | null;
  visibility?: AgentRunEvent['visibility'];
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
    visibility: input.visibility ?? 'user',
    persistence: 'durable',
    displayKind: input.kind === 'done' ? 'completion' : input.kind === 'error' ? 'error' : 'status',
    importance: input.kind === 'error' ? 'high' : 'normal',
    payload: input.payload ?? {},
    createdAt: `2026-01-01T00:00:${String(input.eventSeq).padStart(2, '0')}.000Z`,
  };
}

function frontendEvent(runEvent: AgentRunEvent): AgentFrontendEvent {
  return {
    conversationId: 'conversation-1',
    runEvent,
  };
}

test('runtime wire schema accepts only the canonical Run Event envelope', () => {
  const canonical = frontendEvent(runEvent({ eventSeq: 1, kind: 'status' }));
  assert(parseAgentFrontendEvent(canonical), 'canonical envelope should parse');
  assert(
    parseAgentFrontendEvent(frontendEvent(runEvent({
      eventSeq: 2,
      kind: 'status',
      phase: 'paused',
      status: 'paused',
    }))),
    'paused status should remain valid in the protocol-v2 envelope',
  );
  assertEqual(
    parseAgentFrontendEvent({ ...canonical, type: 'status' }),
    null,
    'legacy top-level fields must be rejected',
  );
  assertEqual(
    parseAgentFrontendEvent({
      conversationId: 'conversation-1',
      runEvent: { ...canonical.runEvent, eventSeq: 0 },
    }),
    null,
    'non-positive event sequences must be rejected',
  );
  assertEqual(
    parseAgentFrontendEvent({
      conversationId: 'conversation-1',
      runEvent: { ...canonical.runEvent, legacyType: 'status' },
    }),
    null,
    'unknown Run Event fields must be rejected',
  );
  const { persistence: _removedPersistence, ...withoutPersistence } = canonical.runEvent;
  assertEqual(
    parseAgentFrontendEvent({ conversationId: 'conversation-1', runEvent: withoutPersistence }),
    null,
    'required presentation and persistence metadata must be present',
  );
});

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
    owner: {
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

function approvalRequest(id = 'approval-1'): ApprovalRequest {
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

test('historical stream projection uses canonical run events and typed task timeline events', () => {
  const projected = projectRunEventsToStreamState(
    taskRun('completed'),
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
        eventType: 'subtask',
        eventSeq: 2,
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

  assertEqual(projected.streamRounds[0]?.reply, 'canonical', 'canonical output should win');
  assertEqual(projected.taskEvents.length, 1, 'timeline task event remains available');
  assertEqual(projected.taskEvents[0].id, 'timeline', 'timeline task event id');
});

test('historical projection marks stale active task runs as interrupted', () => {
  const projected = projectRunEventsToStreamState(
    taskRun('running'),
    [
      runEvent({
        eventSeq: 1,
        kind: 'thinking',
        payload: { type: 'thinking', content: 'Working before app close.' },
      }),
    ],
    [],
    { interruptActive: true },
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

test('keeps only typed task timeline events beside canonical Run Events', () => {
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

  const timeline = taskTimelineEventsFromReplaySource(events);
  const subtaskTimeline = taskTimelinePayloadFromTaskEvent(events[0]);

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
        kind: 'status',
        phase: 'accounting',
        label: 'Resume checkpoint saved after tool round 6.',
        status: 'completed',
        visibility: 'developer',
      }),
      runEvent({
        eventSeq: 7,
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
      taskEvent({
        id: 'verification',
        eventType: 'verification',
        eventSeq: 6,
        label: 'Evidence audit completed',
        status: 'passed',
        payload: {
          taskTimeline: {
            version: 1,
            kind: 'verification',
            visibility: 'developer',
            label: 'Evidence audit completed',
            status: 'passed',
            payload: { overallStatus: 'passed' },
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

  const developerHistory = taskCenterHistoryFromRunEvents(
    [
      runEvent({
        eventSeq: 1,
        kind: 'status',
        label: 'Resume checkpoint saved after tool round 6.',
        visibility: 'developer',
      }),
    ],
    [
      taskEvent({
        id: 'verification-developer',
        eventType: 'verification',
        payload: {
          taskTimeline: {
            version: 1,
            kind: 'verification',
            visibility: 'developer',
            label: 'Evidence audit completed',
            status: 'passed',
            payload: {},
          },
        },
      }),
    ],
    [],
    { includeDeveloper: true },
  );
  assertEqual(developerHistory.length, 2, 'developer history includes diagnostics');
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

test('task center never treats untyped task rows as a Run Event fallback', () => {
  const history = taskCenterHistoryFromEvents(
    [
      taskEvent({
        id: 'old-wrapper',
        eventType: 'status',
        label: 'Old wrapper',
        payload: { eventSeq: 1 },
      }),
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

  assertEqual(history.length, 1, 'only the scheduler event remains visible');
  assertEqual(history[0].source, 'schedulerEvent', 'scheduler event remains visible');
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

test('durable legacy tool payloads are normalized into the canonical tool-card lifecycle', () => {
  const projected = projectRunEventsToStreamState(taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'run_shell',
      status: 'running',
      payload: {
        type: 'toolCallStart',
        callId: 'legacy-call-1',
        toolName: 'run_shell',
        arguments: '{"command":"pwd"}',
      },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolCompleted',
      phase: 'tooling',
      label: 'run_shell',
      status: 'completed',
      payload: {
        type: 'toolCallResult',
        callId: 'legacy-call-1',
        toolName: 'run_shell',
        content: '/workspace',
        isError: false,
      },
    }),
  ]);

  assertEqual(projected.toolCalls.length, 1, 'legacy durable calls must not disappear at replay');
  assertEqual(projected.toolCalls[0].callId, 'legacy-call-1', 'stable call id');
  assertEqual(projected.toolCalls[0].toolName, 'run_shell', 'tool name');
  assertEqual(projected.toolCalls[0].status, 'done', 'terminal tool status');
  assertEqual(projected.toolCalls[0].content, '/workspace', 'tool result content');
});

test('durable legacy tool progress preserves the started card identity', () => {
  const projected = projectRunEventsToStreamState(taskRun('running'), [
    runEvent({
      eventSeq: 1,
      kind: 'toolStarted',
      phase: 'tooling',
      label: 'read_file',
      status: 'running',
      payload: {
        type: 'toolCallStart',
        callId: 'legacy-progress-1',
        toolName: 'read_file',
        arguments: '{"path":"README.md"}',
      },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'toolProgress',
      phase: 'tooling',
      label: 'reading',
      status: 'running',
      payload: {
        type: 'toolCallProgress',
        callId: 'legacy-progress-1',
        note: 'reading',
      },
    }),
  ]);

  assertEqual(projected.toolCalls.length, 1, 'legacy progress updates the existing card');
  assertEqual(projected.toolCalls[0].toolName, 'read_file', 'progress note must not replace tool name');
  assertEqual(projected.toolCalls[0].progressNote, 'reading', 'progress note remains visible');
});

test('authoritative durable replay accepts legacy event sequence gaps without weakening live ordering', () => {
  const conversationId = 'conversation-legacy-run-event-gaps';

  streamStore.restoreFromRunEvents(conversationId, taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'outputDelta',
      payload: { blockId: 'legacy-answer', channel: 'answer', offset: 0, delta: 'Legacy ' },
    }),
    runEvent({
      eventSeq: 4,
      kind: 'outputDelta',
      payload: { blockId: 'legacy-answer', channel: 'answer', offset: 7, delta: 'reply' },
    }),
    runEvent({
      eventSeq: 7,
      kind: 'done',
      phase: 'done',
      label: 'Final answer produced',
      status: 'completed',
      payload: { message: 'Legacy reply', messageTruncated: true },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'legacy durable replay should create stream state');
  assertEqual(restored.isStreaming, false, 'legacy replay should consume its terminal event');
  assertEqual(restored.streamRounds.length, 1, 'legacy replay should preserve the answer block');
  assertEqual(restored.streamRounds[0].reply, 'Legacy reply', 'legacy replay should join gapped blocks');

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

test('durable Run Event replay restores usage and approval state', () => {
  const conversationId = 'conversation-usage-approval-replay';

  streamStore.restoreFromRunEvents(conversationId, taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'usageUpdated',
      phase: 'accounting',
      label: 'Token usage updated',
      payload: {
        usageTotal: { promptTokens: 10, completionTokens: 4, totalTokens: 14 },
        lastPromptTokens: 10,
      },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'approvalRequested',
      phase: 'approval',
      label: 'write_file',
      status: 'pending',
      payload: {
        request: approvalRequest(),
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
        tool_call: {
          callId: 'call-1',
          toolName: 'run_shell',
          arguments: '{"program":"cargo"}',
          status: 'failed',
          renderKind: 'commandExecution',
          isError: false,
          providerExecuted: true,
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
  assertEqual(
    items[0].toolCall.providerExecuted,
    true,
    'legacy snake_case trace carries provider execution ownership',
  );
  assert(items[1].kind === 'tool', 'second persisted item should be a tool');
  assertEqual(items[1].toolCall.status, 'approvalPending', 'approval status normalizes');
  assert(isPendingToolCallStatus(items[1].toolCall.status), 'approval trace is pending');
});

test('identifies legacy reasoning promoted into a persisted reply without hiding real answers', () => {
  const message: ConversationMessage = {
    id: 'assistant-reasoning-only',
    conversationId: 'conversation-1',
    role: 'assistant',
    content: 'raw internal reasoning',
    toolCallId: null,
    toolCalls: [],
    artifacts: null,
    tokenCount: 3,
    createdAt: '2026-01-01T00:00:01.000Z',
    sortOrder: 1,
    thinking: 'raw internal reasoning',
  };
  const reasoningOnlyTrace = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: [{ kind: 'thinking', text: 'raw internal reasoning' }],
  });

  assert(
    isPersistedReasoningOnlyAssistant(message, reasoningOnlyTrace),
    'matching content/thinking without a reply trace is legacy reasoning pollution',
  );
  assert(
    !isPersistedReasoningOnlyAssistant(
      { ...message, content: 'final answer' },
      reasoningOnlyTrace,
    ),
    'a distinct final answer remains visible',
  );
  assert(
    !isPersistedReasoningOnlyAssistant(message, [
      { kind: 'thinking', text: 'raw internal reasoning' },
      { kind: 'reply', text: 'raw internal reasoning' },
    ]),
    'an explicit reply trace is authoritative',
  );
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
    includeDeveloper: true,
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
    includeDeveloper: true,
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
    includeDeveloper: true,
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

  assertEqual(
    sections.some((section) => section.id === 'turn-skills-turn-2'),
    false,
    'ordinary mode should hide no-skill diagnostics',
  );

  const developerSections = turnLifecycleTimelineSections({
    turn,
    routeKind: 'DirectResponse',
    traceItems: items,
    includeDeveloper: true,
  });

  const skillSection = developerSections.find((section) => section.id === 'turn-skills-turn-2');
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

  assertEqual(getToolBriefTarget(partialArgs), null, 'partial JSON is never used as a title target');
  assertEqual(
    getStableFileChangeTarget({ path: 'notes/live.md' }, { paths: ['notes/live.md'] }),
    'notes/live.md',
    'file change target should come from diff artifact path',
  );
});

test('generic tool input policy hides partial JSON and redacts completed secrets', () => {
  assertEqual(
    getToolInputPresentation({
      toolName: 'fetch_url',
      renderKind: 'generic',
      argsStatus: 'streaming',
      status: 'preparing',
    }),
    'hidden',
    'generic partial input policy',
  );
  assertEqual(
    getToolInputPresentation({
      toolName: 'edit_file',
      renderKind: 'fileChange',
      argsStatus: 'streaming',
      status: 'preparing',
    }),
    'live_diff',
    'file edits retain semantic live diff rendering',
  );
  const formatted = formatToolArgumentsForDisplay(
    JSON.stringify({
      url: 'https://example.com',
      authorization: 'Bearer secret',
      headers: { 'x-client': 'should-stay-hidden-with-the-header-map' },
      nested: { apiKey: 'sk-secret', sessionToken: 'session-secret', query: 'status' },
    }),
    { redacted: '[REDACTED]', invalid: '[INVALID]' },
  );
  assert(!formatted.includes('Bearer secret'), 'authorization value should be redacted');
  assert(!formatted.includes('sk-secret'), 'nested API key should be redacted');
  assert(!formatted.includes('session-secret'), 'token suffix should be redacted');
  assert(!formatted.includes('should-stay-hidden'), 'header maps should be redacted');
  assert(formatted.includes('[REDACTED]'), 'redaction marker should remain auditable');
  assertEqual(
    formatToolArgumentsForDisplay('{invalid', { redacted: '[REDACTED]', invalid: '[INVALID]' }),
    '[INVALID]',
    'invalid argument copy is supplied by the translated presentation layer',
  );
});

test('structured connection recovery updates state without becoming reasoning', () => {
  const projected = projectRunEventsToStreamState(taskRun('running'), [
    runEvent({
      eventSeq: 1,
      kind: 'recoveryAttempt',
      label: 'Reconnecting to provider',
      status: 'reconnecting',
      payload: {
        type: 'connectionState',
        state: {
          state: 'reconnecting',
          providerId: 'openai',
          modelId: 'gpt-test',
          errorCategory: 'network',
          attempt: 1,
          maxAttempts: 3,
          nextRetryAt: '2026-01-01T00:00:05Z',
          recoverable: true,
          queuedUserInputs: 0,
          turnPreserved: true,
        },
      },
    }),
  ]);

  assertEqual(projected.connectionState?.state, 'reconnecting', 'connection state');
  assertEqual(projected.connectionState?.attempt, 1, 'connection retry attempt');
  assertEqual(projected.thinkingText, '', 'retry status must not enter reasoning');
});

test('terminal completion clears a stale reconnecting state', () => {
  const projected = projectRunEventsToStreamState(taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'recoveryAttempt',
      label: 'Reconnecting to provider',
      status: 'reconnecting',
      payload: {
        type: 'connectionState',
        state: {
          state: 'reconnecting',
          providerId: 'openai',
          modelId: 'gpt-test',
          errorCategory: 'network',
          attempt: 1,
          maxAttempts: 3,
          recoverable: true,
          queuedUserInputs: 0,
          turnPreserved: true,
        },
      },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'done',
      status: 'completed',
      payload: { type: 'done', message: { role: 'assistant', content: 'Recovered answer' } },
    }),
  ]);

  assertEqual(projected.connectionState, null, 'terminal state must not retain reconnecting');
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

test('timeline view model uses event visibility instead of localized status labels', () => {
  const visible = visibleTraceEventsForTimeline([
    { id: 'internal', kind: 'status', text: '任意内部文案', visibility: 'internal' },
    { id: 'developer', kind: 'status', text: 'Task queued', visibility: 'developer' },
    { id: 'user', kind: 'status', text: 'Task queued', visibility: 'user' },
    { id: 'legacy', kind: 'status', text: '未知旧版状态' },
  ]);
  assertEqual(visible.length, 2, 'only user and legacy user-visible statuses should remain');
  assertEqual(visible[0]?.id, 'user', 'labels must not override explicit user visibility');
  assertEqual(visible[1]?.id, 'legacy', 'legacy events default to user visibility');
  const developerVisible = visibleTraceEventsForTimeline([
    { id: 'internal', kind: 'status', text: 'internal', visibility: 'internal' },
    { id: 'developer', kind: 'status', text: 'route', visibility: 'developer' },
  ], true);
  assertEqual(developerVisible.length, 1, 'developer mode reveals developer telemetry only');
  assertEqual(developerVisible[0]?.id, 'developer', 'internal telemetry remains hidden');
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

test('canonical live timeline projection owns visibility, rounds, and collapse state', () => {
  const projection = projectLiveConversationTimeline({
    traceEvents: [
      { id: 'internal', kind: 'status', text: 'Task queued', visibility: 'internal' },
      { id: 'thinking', kind: 'thinking', text: 'Inspecting the runtime' },
      { id: 'reply', kind: 'reply', text: 'Ready' },
    ],
    streamRounds: [],
    isStreaming: false,
    isThinking: false,
    thinkingText: '',
    toolCalls: [],
    streamText: 'Ready',
    displayedText: 'Ready',
  });

  assertEqual(projection.visibleTraceEvents.length, 2, 'internal events are filtered once');
  assertEqual(projection.liveTraceTimeline.length, 2, 'timeline is projected once');
  assert(projection.collapsedLiveTrace !== null, 'completed trace receives one collapse result');
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

test('timeline view model keeps the terminal reply visible during persistence refresh', () => {
  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: [{ id: 'thinking-1', kind: 'thinking', text: 'Checking files' }],
    isStreaming: false,
    currentTraceActive: false,
    streamText: 'Final answer',
    displayedText: 'Final answer',
  });

  assertEqual(timeline.length, 2, 'terminal preview timeline item count');
  assert(timeline[1].kind === 'reply', 'terminal preview should be a separate reply');
  assertEqual(timeline[1].content, 'Final answer', 'terminal reply should remain visible');
  assertEqual(timeline[1].isStreaming, false, 'terminal reply should not remain streaming');
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

test('live trace only folds into a summary after the turn stops streaming', () => {
  const events: TraceEvent[] = [
    { id: 'thinking-1', kind: 'thinking', text: 'Reading the failing test' },
    { id: 'tool-1', kind: 'tool', toolCall: traceToolCall({ callId: 'call-1' }) },
    { id: 'reply-1', kind: 'reply', text: 'Found the cause, checking one more file.' },
  ];
  const visible = visibleTraceEventsForTimeline(events);

  // Mid-turn: the model emitted an intermediate reply between tool rounds, so
  // the trace is momentarily idle but the run has not finished.
  const midTurnTimeline = buildLiveTraceTimeline({
    visibleTraceEvents: visible,
    isStreaming: true,
    currentTraceActive: false,
    streamText: '',
    displayedText: '',
  });
  assert(midTurnTimeline.length >= 2, 'mid-turn timeline should have history and a reply');
  assertEqual(
    buildCollapsedLiveTrace({
      timeline: midTurnTimeline,
      isStreaming: true,
      currentTraceActive: false,
    }),
    null,
    'an intermediate reply must not collapse the whole live trace',
  );

  // Turn finished: the trace folds into one summary plus the closing reply.
  const finishedTimeline = buildLiveTraceTimeline({
    visibleTraceEvents: visible,
    isStreaming: false,
    currentTraceActive: false,
    streamText: '',
    displayedText: '',
  });
  const collapsed = buildCollapsedLiveTrace({
    timeline: finishedTimeline,
    isStreaming: false,
    currentTraceActive: false,
  });
  assert(collapsed !== null, 'a finished turn should fold its trace');
  assertEqual(collapsed.finalItem.kind, 'reply', 'the closing reply stays outside the fold');
  assert(collapsed.historySections.length > 0, 'folded trace keeps the turn history');
});

test('live trace never folds while the current trace is still active', () => {
  const events: TraceEvent[] = [
    { id: 'reply-1', kind: 'reply', text: 'Interim answer' },
    { id: 'thinking-1', kind: 'thinking', text: 'Now verifying' },
  ];
  const timeline = buildLiveTraceTimeline({
    visibleTraceEvents: visibleTraceEventsForTimeline(events),
    isStreaming: true,
    currentTraceActive: true,
    streamText: '',
    displayedText: '',
  });

  assertEqual(
    buildCollapsedLiveTrace({ timeline, isStreaming: true, currentTraceActive: true }),
    null,
    'an active trace must stay expanded',
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
    {
      id: 'steering-status',
      kind: 'status',
      text: 'focus on edge cases instead',
      tone: 'muted',
      displayKind: 'steering',
    },
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
  const section = timeline[0].sections[0];
  assertEqual(section.id, 'steering-status', 'steering status remains visible');
  assert(section.kind === 'steering', 'steering status gets a dedicated section');
  assertEqual(section.text, 'focus on edge cases instead', 'steering text is preserved');
  assertEqual(round.reply, 'Partial answer before steering.', 'completed round reply is preserved separately');
});

test('persisted trace replay omits completed steering controls', () => {
  const sections = persistedTraceItemToTimelineSections({
    item: {
      kind: 'status',
      text: 'focus on edge cases instead',
      tone: 'muted',
      displayKind: 'steering',
    },
    id: 'persisted-steering-status',
    trace: true,
  });

  assertEqual(sections.length, 0, 'completed steering is not replayed from persisted trace');
});

test('runtime diagnostics replay only in developer mode', () => {
  for (const [index, text] of [
    'Resume checkpoint saved after tool round 3.',
    'The model requested the same tool call batch 3 times without visible progress.',
    'Evidence audit: passed.',
  ].entries()) {
    const item = {
      kind: 'status' as const,
      text,
      tone: 'muted' as const,
      visibility: 'developer' as const,
    };
    assertEqual(
      persistedTraceItemToTimelineSections({
        item,
        id: `diagnostic-${index}`,
        trace: true,
      }).length,
      0,
      `ordinary mode should hide ${text}`,
    );
    assertEqual(
      persistedTraceItemToTimelineSections({
        item,
        id: `diagnostic-${index}`,
        trace: true,
        includeDeveloper: true,
      }).length,
      1,
      `developer mode should show ${text}`,
    );
  }
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

test('restoreFromRunEvents projects terminal error replay into stream state', () => {
  const conversationId = 'conversation-terminal-restore';

  streamStore.restoreFromRunEvents(conversationId, taskRun('failed'), [
    runEvent({
      eventSeq: 1,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution timed out.',
      status: 'failed',
      payload: { message: 'Agent execution timed out.' },
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

test('restoreFromRunEvents preserves cancelled terminal replay status', () => {
  const conversationId = 'conversation-cancelled-terminal-restore';

  streamStore.restoreFromRunEvents(conversationId, taskRun('cancelled'), [
    runEvent({
      eventSeq: 1,
      kind: 'error',
      phase: 'done',
      label: 'Agent execution cancelled.',
      status: 'cancelled',
      payload: { message: 'Agent execution cancelled.' },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelled stream state should exist');
  assertEqual(restored.isStreaming, false, 'cancelled terminal replay stops streaming');
  assertEqual(restored.error, null, 'cancelled terminal replay does not surface as an error');

  streamStore.clearStream(conversationId);
});

test('restoreFromRunEvents closes a stale cancelling task after app restart', () => {
  const conversationId = 'conversation-cancelling-restore';

  streamStore.restoreFromRunEvents(conversationId, taskRun('cancelling'), []);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'cancelling stream state should exist');
  assertEqual(restored.isStreaming, false, 'stale cancelling task run should be closed');
  assertEqual(restored.taskRun?.status, 'cancelled', 'stale task run status');

  streamStore.clearStream(conversationId);
});

test('awaiting-user-input status settles live streaming without a terminal error', () => {
  const conversationId = 'conversation-awaiting-user-input';
  streamStore.startStream(conversationId);

  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'thinking',
    phase: 'responding',
    label: 'Thinking',
    payload: { content: 'Need a decision.' },
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'status',
    phase: 'awaiting_user_input',
    label: 'Waiting for your input',
    status: 'awaiting_user_input',
    payload: { content: 'Waiting for your input' },
  })));

  const awaiting = streamStore.getStream(conversationId);
  assert(awaiting, 'awaiting stream state should exist');
  assertEqual(awaiting.isStreaming, false, 'awaiting input should settle streaming');
  assertEqual(awaiting.isThinking, false, 'awaiting input should settle thinking');
  assertEqual(awaiting.error, null, 'awaiting input should not surface an error');
  assert(awaiting.turnTiming?.finishedAtEpochMs, 'awaiting input should freeze turn timing');

  streamStore.clearStream(conversationId);
});

test('paused status suspends a bound run and a later running status resumes it', () => {
  const conversationId = 'conversation-paused-resume';
  streamStore.startStream(conversationId);
  streamStore.bindTurnHandle(conversationId, {
    sessionId: conversationId,
    runId: 'run-1',
    turnId: 'turn-1',
    state: 'running',
  });

  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'status',
    phase: 'paused',
    status: 'paused',
    label: 'Run paused',
  })));

  const paused = streamStore.getStream(conversationId);
  assert(paused, 'paused stream state should exist');
  assertEqual(paused.isStreaming, false, 'paused status should suspend live streaming');
  assertEqual(paused.isThinking, false, 'paused status should settle thinking');
  assertEqual(paused.error, null, 'paused status should not surface an error');
  assert(paused.turnTiming?.finishedAtEpochMs, 'paused status should freeze turn timing');
  assertEqual(paused.turnHandle?.runId, 'run-1', 'paused status retains the bound run identity');

  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'status',
    phase: 'responding',
    status: 'running',
    label: 'Run resumed',
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 3,
    kind: 'outputDelta',
    payload: {
      blockId: 'resumed-answer',
      channel: 'answer',
      offset: 0,
      delta: 'Continued on the same run',
    },
  })));

  const resumed = streamStore.getStream(conversationId);
  assert(resumed, 'resumed stream state should exist');
  assertEqual(resumed.isStreaming, true, 'running status should resume the suspended stream');
  assertEqual(resumed.turnHandle?.runId, 'run-1', 'resume keeps the original run projection');
  assertEqual(resumed.streamText, 'Continued on the same run', 'resumed events are projected');
  streamStore.clearStream(conversationId);
});

test('legacy Done paused suspends without closing the same-run continuation', () => {
  const conversationId = 'conversation-legacy-done-paused';
  const events: AgentRunEvent[] = [
    runEvent({
      eventSeq: 1,
      kind: 'done',
      phase: 'done',
      status: 'paused',
      label: 'Run paused',
      payload: { finishReason: 'paused' },
    }),
    runEvent({
      eventSeq: 2,
      kind: 'status',
      phase: 'routing',
      status: 'queued',
      label: 'Run queued to resume',
    }),
    runEvent({
      eventSeq: 3,
      kind: 'outputDelta',
      phase: 'responding',
      label: 'Continued output',
      payload: {
        blockId: 'continued-answer',
        channel: 'answer',
        offset: 0,
        delta: 'Continued after pause',
      },
    }),
    runEvent({
      eventSeq: 4,
      kind: 'done',
      phase: 'done',
      status: 'completed',
      label: 'Run completed',
      payload: {
        message: { role: 'assistant', parts: [{ type: 'text', text: 'Continued after pause' }] },
        usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      },
    }),
  ];

  streamStore.startStream(conversationId);
  streamStore.bindTurnHandle(conversationId, {
    sessionId: conversationId,
    runId: 'run-1',
    turnId: 'turn-1',
    state: 'running',
  });
  streamStore.dispatch(conversationId, frontendEvent(events[0]));

  const suspended = streamStore.getStream(conversationId);
  assert(suspended, 'legacy paused state should be retained');
  assertEqual(suspended.isStreaming, false, 'legacy paused Done suspends live transport');
  assertEqual(suspended.streamRounds.length, 0, 'legacy paused Done does not finalize a reply round');
  assertEqual(suspended.turnHandle?.runId, 'run-1', 'legacy pause keeps the same run identity');

  for (const event of events.slice(1)) {
    streamStore.dispatch(conversationId, frontendEvent(event));
  }
  const live = streamStore.getStream(conversationId);
  assert(live, 'continued live state should exist');
  assertEqual(live.isStreaming, false, 'the later completed Done closes live transport');
  assertEqual(
    live.streamRounds[live.streamRounds.length - 1]?.reply,
    'Continued after pause',
    'live projection consumes output after the legacy pause',
  );

  const durable = projectRunEventsToStreamState(taskRun('completed'), events);
  assertEqual(durable.isStreaming, false, 'the completed durable projection is settled');
  assertEqual(
    durable.streamRounds[durable.streamRounds.length - 1]?.reply,
    'Continued after pause',
    'durable replay continues past the legacy paused Done',
  );
  assertEqual(
    durable._lastEventSeq,
    4,
    'durable replay consumes the true terminal event after the suspension',
  );

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

test('turn timing stores lifecycle timestamps without a global elapsed counter', () => {
  const conversationId = 'conversation-turn-timing';
  streamStore.startStream(conversationId);
  const started = streamStore.getStream(conversationId)?.turnTiming;
  assert(started, 'startStream should establish timing facts');
  assert(started.startedAtMonotonicMs != null, 'live timing keeps a monotonic start anchor');
  assertEqual(started.firstEventAtEpochMs, null, 'first event is initially unknown');
  assertEqual(started.finishedAtEpochMs, null, 'finish is initially unknown');

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: runEvent({ eventSeq: 1, kind: 'status', label: 'Planning' }),
  } as AgentFrontendEvent);
  const afterEvent = streamStore.getStream(conversationId)?.turnTiming;
  assert(afterEvent?.firstEventAtEpochMs, 'first accepted event records TTFE timestamp');

  streamStore.stopStream(conversationId);
  const finished = streamStore.getStream(conversationId)?.turnTiming;
  assert(finished?.finishedAtEpochMs, 'terminal projection records a fixed finish timestamp');
  assert(finished?.finishedAtMonotonicMs != null, 'same-page completion freezes a monotonic finish anchor');
  assert(!('elapsedMs' in finished), 'stream timing does not store a ticking elapsed value');
  streamStore.clearStream(conversationId);
});

test('live elapsed timing ignores wall-clock jumps', () => {
  const elapsed = resolveElapsedDurationMs({
    startedAtEpochMs: 100_000,
    startedAtMonotonicMs: 1_000,
    firstEventAtEpochMs: null,
    firstVisibleOutputAtEpochMs: null,
    finishedAtEpochMs: null,
    finishedAtMonotonicMs: null,
  }, true, {
    epochMs: 10_000,
    monotonicMs: 5_250,
  });

  assertEqual(elapsed, 4_250, 'monotonic live duration survives a backward wall-clock jump');
});

test('turn duration formatting uses locale-neutral clock output', () => {
  const turn: ConversationTurn = {
    id: 'turn-duration',
    conversationId: 'conversation-duration',
    userMessageId: 'user-duration',
    assistantMessageId: 'assistant-duration',
    status: 'success',
    trace: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:01:08.000Z',
    finishedAt: '2026-01-01T00:01:08.000Z',
  };
  assertEqual(formatTurnDuration(turn), '1:08', 'completed turn wall duration');
  assertEqual(formatElapsedDuration(8_900), '0:08', 'live elapsed duration');
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

test('done message supplies the final reply when a thinking-only round is active', () => {
  const conversationId = 'conversation-thinking-then-done-message';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'thinking',
    payload: { content: 'Preparing the final answer' },
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'done',
    phase: 'done',
    status: 'completed',
    payload: {
      message: {
        role: 'assistant',
        parts: [{ type: 'text', text: 'Final answer' }],
      },
      usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    },
  })));

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'done event should retain terminal preview state');
  assertEqual(restored.isStreaming, false, 'done event stops streaming');
  assertEqual(restored.streamRounds.length, 1, 'thinking and final reply share one round');
  assertEqual(restored.streamRounds[0].reply, 'Final answer', 'done message becomes final reply');

  streamStore.clearStream(conversationId);
});

test('live ordering buffers out-of-order events and drains them without losing output', () => {
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
  streamStore.dispatch(conversationId, event(3, 2, 'C'));
  streamStore.dispatch(conversationId, event(2, 1, 'B'));

  const state = streamStore.getStream(conversationId);
  assert(state, 'ordered stream state should exist');
  assertEqual(state.streamText, 'ABC', 'the missing event is applied before the buffered successor');

  streamStore.clearStream(conversationId);
});

test('a settled retained projection is replaced when a background run starts at sequence one', () => {
  const conversationId = 'conversation-background-run-replacement';
  const eventForRun = (runId: string, event: AgentRunEvent): AgentFrontendEvent => ({
    conversationId,
    runEvent: { ...event, runId, turnId: `${runId}-turn` },
  });

  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, eventForRun('old-run', runEvent({
    eventSeq: 1,
    kind: 'done',
    phase: 'done',
    status: 'completed',
    payload: {
      message: { role: 'assistant', parts: [{ type: 'text', text: 'Old answer' }] },
      usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    },
  })));
  streamStore.dispatch(conversationId, eventForRun('new-run', runEvent({
    eventSeq: 1,
    kind: 'status',
    phase: 'routing',
    status: 'queued',
    payload: {},
  })));
  streamStore.dispatch(conversationId, eventForRun('new-run', runEvent({
    eventSeq: 2,
    kind: 'outputDelta',
    payload: {
      blockId: 'new-answer',
      channel: 'answer',
      offset: 0,
      delta: 'New answer',
    },
  })));
  streamStore.dispatch(conversationId, eventForRun('old-run', runEvent({
    eventSeq: 2,
    kind: 'outputDelta',
    payload: {
      blockId: 'late-old-answer',
      channel: 'answer',
      offset: 0,
      delta: 'Late old suffix',
    },
  })));

  const state = streamStore.getStream(conversationId);
  assert(state, 'replacement run state should exist');
  assertEqual(state.streamRounds.length, 0, 'old terminal projection is discarded');
  assertEqual(
    state.streamText,
    'New answer',
    'new run starts from its own sequence and rejects late events from the retired run',
  );
  streamStore.clearStream(conversationId);
});

test('binding a new launch rejects a stopped run terminal that arrived during the handshake', () => {
  const conversationId = 'conversation-stop-relaunch-race';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'done',
        phase: 'done',
        status: 'cancelled',
        payload: { finishReason: 'cancelled' },
      }),
      runId: 'stopped-run',
      turnId: 'stopped-turn',
    },
  });
  streamStore.bindTurnHandle(conversationId, {
    sessionId: conversationId,
    runId: 'new-run',
    turnId: 'new-turn',
    state: 'starting',
  });

  const rebound = streamStore.getStream(conversationId);
  assert(rebound, 'rebound launch state should exist');
  assertEqual(rebound.isStreaming, true, 'the authoritative launch reopens a clean live state');
  assertEqual(rebound.turnHandle?.runId, 'new-run', 'the launch handle owns the replacement state');
  assertEqual(rebound.streamRounds.length, 0, 'the stopped run terminal projection is discarded');

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'outputDelta',
        payload: {
          blockId: 'new-run-answer',
          channel: 'answer',
          offset: 0,
          delta: 'Fresh answer',
        },
      }),
      runId: 'new-run',
      turnId: 'new-turn',
    },
  });

  const current = streamStore.getStream(conversationId);
  assert(current, 'new launch state should remain available');
  assertEqual(current.streamText, 'Fresh answer', 'new run events project after authoritative binding');
  streamStore.clearStream(conversationId);
});

test('a bound launch rejects stopped run events that arrive after the handshake', () => {
  const conversationId = 'conversation-stop-relaunch-after-bind';
  streamStore.startStream(conversationId);
  streamStore.bindTurnHandle(conversationId, {
    sessionId: conversationId,
    runId: 'new-run',
    turnId: 'new-turn',
    state: 'starting',
  });

  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'status',
        phase: 'responding',
        status: 'cancelling',
        label: 'Stop requested',
      }),
      runId: 'stopped-run',
      turnId: 'stopped-turn',
    },
  });
  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'outputDelta',
        payload: {
          blockId: 'new-run-answer',
          channel: 'answer',
          offset: 0,
          delta: 'Fresh answer',
        },
      }),
      runId: 'new-run',
      turnId: 'new-turn',
    },
  });

  const current = streamStore.getStream(conversationId);
  assert(current, 'bound launch state should remain available');
  assertEqual(current.streamText, 'Fresh answer', 'the retired run cannot claim ordering');
  assert(
    !current.traceEvents.some(event => event.kind === 'status' && event.text === 'Stop requested'),
    'the retired run event never enters the live projection',
  );
  streamStore.clearStream(conversationId);
});

test('a settled bound launch rejects a retired run terminal', () => {
  const conversationId = 'conversation-stop-relaunch-after-settle';
  streamStore.startStream(conversationId);
  streamStore.bindTurnHandle(conversationId, {
    sessionId: conversationId,
    runId: 'new-run',
    turnId: 'new-turn',
    state: 'starting',
  });
  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'done',
        phase: 'done',
        status: 'completed',
        payload: {
          message: { role: 'assistant', parts: [{ type: 'text', text: 'Fresh answer' }] },
          usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
        },
      }),
      runId: 'new-run',
      turnId: 'new-turn',
    },
  });
  streamStore.dispatch(conversationId, {
    conversationId,
    runEvent: {
      ...runEvent({
        eventSeq: 1,
        kind: 'error',
        phase: 'done',
        status: 'cancelled',
        payload: { message: 'Retired run cancelled' },
      }),
      runId: 'stopped-run',
      turnId: 'stopped-turn',
    },
  });

  const current = streamStore.getStream(conversationId);
  assert(current, 'settled bound launch should remain available');
  assertEqual(current.streamRounds[0]?.reply, 'Fresh answer', 'the current answer remains authoritative');
  assert(
    !current.traceEvents.some(event =>
      event.kind === 'status' && event.text === 'Retired run cancelled'),
    'the retired terminal never replaces the settled projection',
  );
  streamStore.clearStream(conversationId);
});

test('durable hydration replaces an unbound blank state created by a future event', () => {
  const conversationId = 'conversation-unbound-gap-hydration';
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'status',
    phase: 'responding',
    status: 'running',
    label: 'Future event',
  })));
  const blank = streamStore.getStream(conversationId);
  assert(blank, 'future event should create a recoverable stream state');
  assertEqual(blank.turnHandle, null, 'unsolicited event has no launch handle');
  assertEqual(blank.traceEvents.length, 0, 'future event remains buffered without a prefix');

  streamStore.restoreFromRunEvents(conversationId, taskRun('completed'), [
    runEvent({
      eventSeq: 1,
      kind: 'done',
      phase: 'done',
      status: 'completed',
      payload: {
        message: { role: 'assistant', parts: [{ type: 'text', text: 'Recovered answer' }] },
        usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      },
    }),
  ]);

  const restored = streamStore.getStream(conversationId);
  assert(restored, 'durable hydration should replace the blank unbound state');
  assertEqual(restored.isStreaming, false, 'durable terminal state settles the stream');
  assertEqual(restored.streamRounds[0]?.reply, 'Recovered answer', 'durable answer is visible');
  streamStore.clearStream(conversationId);
});

test('locally stopped streams consume suppressed events before the terminal', () => {
  const conversationId = 'conversation-stop-terminal-ordering';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'status',
    phase: 'responding',
    status: 'running',
    label: 'Agent running',
  })));
  streamStore.stopStream(conversationId);

  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'status',
    phase: 'responding',
    status: 'cancelling',
    label: 'Stop requested',
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 3,
    kind: 'error',
    phase: 'done',
    status: 'cancelled',
    payload: { message: 'Backend confirmed cancellation' },
  })));

  const state = streamStore.getStream(conversationId);
  assert(state, 'cancelled stream state should exist');
  assertEqual(state.isStreaming, false, 'authoritative cancellation remains terminal');
  assertEqual(state.error, null, 'cancelled terminal does not surface as a failure');
  assert(
    state.traceEvents.some(event =>
      event.kind === 'status' && event.text === 'Backend confirmed cancellation'),
    'terminal event is applied after the visually suppressed stop status',
  );
  assert(
    !state.traceEvents.some(event => event.kind === 'status' && event.text === 'Stop requested'),
    'late nonterminal status is consumed without changing the settled projection',
  );

  streamStore.clearStream(conversationId);
});

test('block projection buffers future UTF-8 byte offsets for answer and thinking', () => {
  const state = createDefaultState();
  const prefix = '你🙂';
  const prefixBytes = new TextEncoder().encode(prefix).length;

  applyStreamBlockDelta(state, 'answer', 'answer-cjk', prefixBytes, '好');
  assertEqual(state.streamText, '', 'future answer fragment waits for its prefix');
  applyStreamBlockDelta(state, 'answer', 'answer-cjk', 0, prefix);
  assertEqual(state.streamText, '你🙂好', 'answer fragments drain by UTF-8 byte offset');

  applyStreamBlockDelta(state, 'thinking', 'thinking-cjk', prefixBytes, '完');
  assertEqual(state.thinkingText, '', 'future thinking fragment waits for its prefix');
  applyStreamBlockDelta(state, 'thinking', 'thinking-cjk', 0, prefix);
  assertEqual(state.thinkingText, '你🙂完', 'thinking fragments drain by UTF-8 byte offset');
});

test('Done message replaces an incomplete streamed answer as the terminal authority', () => {
  const conversationId = 'conversation-authoritative-done';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'outputDelta',
    payload: {
      blockId: 'partial-answer',
      channel: 'answer',
      offset: 0,
      delta: 'Partial',
    },
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'done',
    phase: 'done',
    status: 'completed',
    payload: {
      message: { role: 'assistant', parts: [{ type: 'text', text: 'Complete answer' }] },
      usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    },
  })));

  const state = streamStore.getStream(conversationId);
  assert(state, 'terminal state should exist');
  assertEqual(
    state.streamRounds[state.streamRounds.length - 1]?.reply,
    'Complete answer',
    'Done replaces partial output',
  );
  const finalReplyTrace = [...state.traceEvents].reverse().find(event => event.kind === 'reply');
  assertEqual(finalReplyTrace?.kind === 'reply' ? finalReplyTrace.text : '', 'Complete answer', 'trace uses Done authority');
  streamStore.clearStream(conversationId);
});

test('a truncated Done preview preserves the complete ordered streamed answer', () => {
  const conversationId = 'conversation-truncated-done';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'outputDelta',
    payload: {
      blockId: 'complete-answer',
      channel: 'answer',
      offset: 0,
      delta: 'Complete streamed answer',
    },
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'done',
    phase: 'done',
    status: 'completed',
    payload: {
      message: { role: 'assistant', parts: [{ type: 'text', text: 'Complete stre\n[truncated]' }] },
      messageTruncated: true,
      usageTotal: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    },
  })));

  const state = streamStore.getStream(conversationId);
  assert(state, 'terminal state should exist');
  assertEqual(
    state.streamRounds[state.streamRounds.length - 1]?.reply,
    'Complete streamed answer',
    'ordered stream wins only when Done declares message truncation',
  );
  const finalReplyTrace = [...state.traceEvents].reverse().find(event => event.kind === 'reply');
  assertEqual(
    finalReplyTrace?.kind === 'reply' ? finalReplyTrace.text : '',
    'Complete streamed answer',
    'trace retains the complete streamed answer',
  );
  streamStore.clearStream(conversationId);
});

test('a newer launch status reopens a stream settled by awaiting user input', () => {
  const conversationId = 'conversation-fast-interaction-resume';
  streamStore.startStream(conversationId);
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 1,
    kind: 'status',
    phase: 'awaiting_user_input',
    status: 'awaiting_user_input',
    label: 'Waiting for your response',
  })));
  assertEqual(
    streamStore.getStream(conversationId)?.isStreaming,
    false,
    'awaiting status settles the suspended stream',
  );

  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 2,
    kind: 'status',
    phase: 'routing',
    status: 'running',
    label: 'Agent started',
  })));
  streamStore.dispatch(conversationId, frontendEvent(runEvent({
    eventSeq: 3,
    kind: 'outputDelta',
    payload: {
      blockId: 'resumed-answer',
      channel: 'answer',
      offset: 0,
      delta: 'Continued',
    },
  })));

  const resumed = streamStore.getStream(conversationId);
  assert(resumed, 'resumed stream state should exist');
  assertEqual(resumed.isStreaming, true, 'new running status reopens the continuation');
  assertEqual(resumed.streamText, 'Continued', 'resumed nonterminal events are accepted');
  streamStore.clearStream(conversationId);
});

test('stream registry keeps concurrent conversations independently addressable', () => {
  const firstId = 'concurrent-stream-first';
  const secondId = 'concurrent-stream-second';

  streamStore.startStream(firstId);
  streamStore.startStream(secondId);
  assertEqual(
    streamStore.getRunningConversationIds().sort().join(','),
    [firstId, secondId].sort().join(','),
    'both running conversations are exposed to navigation surfaces',
  );

  streamStore.stopStream(firstId);
  assertEqual(
    streamStore.getRunningConversationIds().join(','),
    secondId,
    'stopping one conversation does not affect the other',
  );
  assert(streamStore.getStream(secondId)?.isStreaming, 'second conversation keeps running');

  streamStore.clearStream(firstId);
  streamStore.clearStream(secondId);
});

test('ordinary stream notifications coalesce by frame while urgent state flushes now', () => {
  const queuedFrames: Array<() => void> = [];
  const flushed: string[] = [];
  const batcher = new ConversationFrameBatcher(
    conversationId => flushed.push(conversationId),
    callback => queuedFrames.push(callback),
  );

  batcher.schedule('conversation-a');
  batcher.schedule('conversation-a');
  batcher.schedule('conversation-b');
  assertEqual(flushed.length, 0, 'ordinary notifications wait for a paint frame');
  assertEqual(queuedFrames.length, 1, 'one frame serves all pending conversations');

  batcher.flushNow('conversation-a');
  assertEqual(flushed.join(','), 'conversation-a', 'urgent state is delivered immediately');
  queuedFrames[0]();
  assertEqual(flushed.join(','), 'conversation-a,conversation-b', 'urgent state is not duplicated');
});

test('conversation cache evicts least-recently-used entries by count and bytes', () => {
  const recency = new Map<string, number>();
  let cache: Record<string, string> = {};
  const upsert = (id: string, value: string, tick: number, protectedKeys: string[] = []) => {
    cache = upsertBoundedConversationCache(cache, id, value, {
      maxEntries: 3,
      maxBytes: 9,
      estimateBytes: entry => entry.length,
      recency,
      protectedKeys,
      tick,
    });
  };

  upsert('a', 'aaa', 1);
  upsert('b', 'bbb', 2);
  upsert('c', 'ccc', 3);
  upsert('d', 'ddd', 4, ['a']);
  assert(Boolean(cache.a), 'protected conversation remains cached');
  assert(!cache.b, 'least-recently-used unprotected conversation is evicted');
  assertEqual(Object.keys(cache).length, 3, 'entry count remains bounded');

  upsert('oversized', '1234567890', 5);
  assert(Boolean(cache.oversized), 'new active entry remains available even above byte budget');
  assert(!cache.c && !cache.d, 'older entries are evicted to satisfy the byte budget');
});

test('completed stream state is bounded and recoverable from durable events', () => {
  const ids = Array.from({ length: 40 }, (_, index) => `bounded-stream-${index}`);
  ids.forEach(id => {
    streamStore.restoreFromRunEvents(id, taskRun('completed'), []);
  });

  assertEqual(
    streamStore.getStream(ids[0]),
    undefined,
    'the oldest completed stream is evicted from the in-memory preview cache',
  );
  assert(
    streamStore.getStream(ids[ids.length - 1]),
    'the newest completed stream remains available',
  );

  const restoredId = ids[0];
  streamStore.restoreFromRunEvents(restoredId, taskRun('completed'), []);
  assert(streamStore.getStream(restoredId), 'an evicted preview restores from durable events');
  ids.forEach(id => streamStore.clearStream(id));
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
