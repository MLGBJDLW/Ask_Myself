import {
  findVisibleSubagentRuns,
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
      result: 'Verified result',
      toolEvents: [],
    },
  }),
];
const projection = projectSubagentLifecycle(events);
assert(projection.status === 'done', 'completed lifecycle should be terminal');
assert(projection.streamedResult === 'Finding one. Finding two.', 'output deltas should stay ordered');
assert(projection.thinking.join('') === 'Checking source. ', 'thinking deltas should remain separate');
assert(projection.artifact?.result === 'Verified result', 'terminal artifact should hydrate');

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
