import {
  findVisibleSubagentRuns,
  extractSubagentBatchArtifact,
  projectSubagentLifecycle,
  projectSubagentLifecycleRuns,
} from '../src/lib/subagentArtifacts';
import type { ActivityEvent } from '../src/types/conversation';
import type { ToolCallEvent } from '../src/lib/streaming/protocol';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function event(
  seq: number,
  subagentEvent: string,
  detail: Record<string, unknown>,
  agentId = 'agent-1',
): ActivityEvent {
  return {
    activityId: agentId,
    seq,
    timestamp: `2026-08-09T00:00:0${seq}Z`,
    kind: subagentEvent === 'completed' ? 'completed' : 'progress',
    payload: {
      subagentEvent,
      agentId,
      detail,
      ...(subagentEvent === 'outputDelta' ? { data: detail.delta } : {}),
    },
  };
}

const events = [
  event(1, 'spawned', { task: 'Research primary sources' }),
  event(2, 'thinkingDelta', { delta: 'Checking source. ' }),
  event(3, 'outputDelta', { delta: 'Finding one. ' }),
  event(4, 'outputDelta', { delta: 'Finding two.' }),
  event(5, 'completed', {
    status: 'completed',
    result: {
      id: 'agent-1',
      status: 'done',
      task: 'Research primary sources',
      modelPolicy: 'independentReviewer',
      effectiveModel: 'reviewer-model',
      modelRouteFallback: true,
      result: 'Verified result',
      toolEvents: [],
      preflight: {
        schemaVersion: 1,
        completedStages: ['history', 'provider', 'policy', 'budget', 'timeout'],
        providerId: 'OpenRouter',
        effectiveModel: 'reviewer-model',
        contextMessageCount: 4,
        droppedInvalidContextMessages: 0,
        reservedTokens: 12000,
        remainingTokenBudget: 48000,
        remainingCallBudget: 2,
        runDeadlineMs: 60000,
      },
    },
  }),
];
const projection = projectSubagentLifecycle(events);
assert(projection.status === 'done', 'completed lifecycle should be terminal');
assert(projection.streamedResult === 'Finding one. Finding two.', 'output deltas should stay ordered');
assert(projection.thinking.join('') === 'Checking source. ', 'thinking deltas should remain separate');
assert(projection.artifact?.result === 'Verified result', 'terminal artifact should hydrate');
assert(projection.artifact?.effectiveModel === 'reviewer-model', 'effective model should survive lifecycle projection');
assert(projection.artifact?.modelPolicy === 'independentReviewer', 'requested model route should survive lifecycle projection');
assert(projection.artifact?.modelRouteFallback === true, 'model route fallback should survive lifecycle projection');
assert(projection.artifact?.preflight?.providerId === 'OpenRouter', 'runtime provider should survive lifecycle projection');
assert(projection.artifact?.preflight?.remainingTokenBudget === 48000, 'preflight token budget should survive lifecycle projection');
assert(projection.artifact?.preflight?.remainingCallBudget === 2, 'preflight call budget should survive lifecycle projection');
assert(projection.artifact?.preflight?.runDeadlineMs === 60000, 'preflight deadline should survive lifecycle projection');

const cancelled = projectSubagentLifecycle([
  event(1, 'spawned', { task: 'Cancelled task' }, 'agent-cancelled'),
  event(2, 'cancelled', { status: 'cancelled' }, 'agent-cancelled'),
]);
assert(cancelled.status === 'cancelled', 'cooperative cancellation must remain distinct from failure');
assert(cancelled.errorMessage === null, 'cooperative cancellation must not manufacture an error');

const toolCall: ToolCallEvent = {
  callId: 'parent-call-1',
  toolName: 'spawn_subagent',
  arguments: JSON.stringify({ task: 'Research primary sources' }),
  status: 'done',
  argsStatus: 'done',
  argsBytes: 0,
  content: '',
  isError: false,
  artifacts: {
    kind: 'subagent_result',
    id: 'agent-1',
    status: 'running',
    task: 'Research primary sources',
    result: '',
    toolEvents: [],
  },
  activityEvents: events,
};
const [run] = findVisibleSubagentRuns([], [toolCall]);
assert(run.id === 'agent-1', 'visible run should expose lifecycle handle, not parent call id');
assert(run.status === 'done', 'live lifecycle should override the initial running artifact');
assert(run.result === 'Verified result', 'terminal result should replace spawn acknowledgement');
assert(run.effectiveModel === 'reviewer-model', 'visible run should expose the effective model');
assert(run.preflight?.providerId === 'OpenRouter', 'visible run should expose the runtime provider');

const runningToolCall: ToolCallEvent = {
  callId: 'parent-call-running',
  toolName: 'spawn_subagent',
  arguments: JSON.stringify({
    task: 'Continue background research',
    model_policy: 'fast',
  }),
  status: 'done',
  argsStatus: 'done',
  argsBytes: 0,
  content: 'Subagent spawned.',
  isError: false,
  artifacts: {
    kind: 'subagent_result',
    id: 'agent-running',
    status: 'running',
    task: 'Continue background research',
    result: '',
    toolEvents: [],
    lifecycleTools: {
      observe: 'observe_subagent',
      wait: 'wait_subagent',
      sendInput: 'send_subagent_input',
      cancel: 'cancel_subagent',
      close: 'close_subagent',
      retry: 'retry_subagent',
    },
  },
};
const [runningRun] = findVisibleSubagentRuns([], [runningToolCall]);
assert(runningRun.status === 'cancelled', 'a persisted running artifact without an active parent must fail closed');
assert(runningRun.runtimeState === 'interrupted', 'historical lifecycle handles must be labelled interrupted');
assert(runningRun.modelPolicy === 'fast', 'running artifact should retain the requested model route from tool arguments');
assert(runningRun.lifecycleTools?.sendInput === 'send_subagent_input', 'advertised steer control should project');
assert(runningRun.lifecycleTools?.cancel === 'cancel_subagent', 'advertised cancel control should project');
assert(
  !('retry' in (runningRun.lifecycleTools ?? {})),
  'unknown lifecycle controls must not be projected as supported UI capabilities',
);

const budgetedBatch = extractSubagentBatchArtifact({
  kind: 'subagent_batch_result',
  budgetAfter: {
    maxParallel: 3,
    maxCallsPerTurn: 6,
    callsStarted: 3,
    remainingCalls: 3,
    tokenBudget: 120000,
    tokensSpent: 48000,
    remainingTokens: 72000,
  },
  runs: [],
});
assert(budgetedBatch?.budgetAfter?.remainingTokens === 72000, 'authoritative post-batch token budget must project');
assert(budgetedBatch?.budgetAfter?.remainingCalls === 3, 'authoritative post-batch call budget must project');

const batchEvents = [
  event(1, 'spawned', { task: 'Worker one', roleId: 'researcher' }, 'agent-b1'),
  event(2, 'spawned', { task: 'Worker two', roleId: 'verifier' }, 'agent-b2'),
  event(3, 'spawned', { task: 'Worker three', roleId: 'critic' }, 'agent-b3'),
  event(4, 'outputDelta', { delta: 'one-progress' }, 'agent-b1'),
  event(5, 'outputDelta', { delta: 'two-progress' }, 'agent-b2'),
  event(6, 'thinkingDelta', { delta: 'three-thinking' }, 'agent-b3'),
];
const batchRuns = projectSubagentLifecycleRuns(batchEvents);
assert(batchRuns.length === 3, 'batch lifecycle should retain three independent workers');
assert(
  batchRuns.find(item => item.id === 'agent-b1')?.result === 'one-progress',
  'first worker output should not merge with another worker',
);
assert(
  batchRuns.find(item => item.id === 'agent-b2')?.result === 'two-progress',
  'second worker output should remain independently visible',
);
assert(
  batchRuns.find(item => item.id === 'agent-b3')?.thinking?.join('') === 'three-thinking',
  'third worker reasoning should remain independently visible',
);

const batchToolCall: ToolCallEvent = {
  callId: 'parent-batch-1',
  toolName: 'spawn_subagent_batch',
  arguments: JSON.stringify({ tasks: [{ task: 'one' }, { task: 'two' }, { task: 'three' }] }),
  status: 'running',
  argsStatus: 'done',
  argsBytes: 0,
  activityEvents: batchEvents,
};
assert(
  findVisibleSubagentRuns([], [batchToolCall]).length === 3,
  'a running batch tool call should project worker cards before its aggregate artifact exists',
);

console.log('ok - subagent lifecycle projects incremental and terminal events');
