import {
  DurableRunReconciler,
  taskRunCanAcceptStop,
  taskRunIsActive,
  taskRunIsSuspended,
  type DurableRunReconciliationPort,
} from '../src/lib/streaming/runReconciliation';
import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
  Conversation,
  ConversationMessage,
  ConversationTurn,
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

function taskRun(status: string, overrides: Partial<AgentTaskRun> = {}): AgentTaskRun {
  return {
    id: 'run-1',
    conversationId: 'conversation-1',
    turnId: 'turn-1',
    userMessageId: 'user-1',
    status,
    phase: status === 'paused' ? 'paused' : 'responding',
    title: 'Run',
    createdAt: '2026-08-11T00:00:00.000Z',
    updatedAt: '2026-08-11T00:01:00.000Z',
    ...overrides,
  };
}

function runEvent(eventSeq: number): AgentRunEvent {
  return {
    version: 2,
    runId: 'run-1',
    turnId: 'turn-1',
    eventSeq,
    kind: 'outputDelta',
    phase: 'responding',
    label: `event-${eventSeq}`,
    payload: { delta: String(eventSeq) },
  };
}

function taskEvent(index: number): AgentTaskRunEvent {
  return {
    id: `task-event-${index}`,
    runId: 'run-1',
    eventType: 'status',
    label: `task-${index}`,
    createdAt: `2026-08-11T00:00:${String(index).padStart(2, '0')}.000Z`,
  };
}

function message(
  id: string,
  role: ConversationMessage['role'],
  content: string,
  sortOrder: number,
): ConversationMessage {
  return {
    id,
    conversationId: 'conversation-1',
    role,
    content,
    toolCallId: null,
    toolCalls: [],
    artifacts: null,
    tokenCount: 0,
    createdAt: `2026-08-11T00:00:0${sortOrder}.000Z`,
    sortOrder,
    thinking: null,
  };
}

function createPort(overrides: Partial<DurableRunReconciliationPort> = {}) {
  const calls = {
    taskRuns: 0,
    runEvents: 0,
    taskEvents: 0,
    conversations: 0,
    turns: 0,
  };
  const conversation: Conversation = {
    id: 'conversation-1',
    title: 'Conversation',
    provider: 'test',
    model: 'test',
    systemPrompt: '',
    createdAt: '2026-08-11T00:00:00.000Z',
    updatedAt: '2026-08-11T00:00:00.000Z',
  };
  const port: DurableRunReconciliationPort = {
    async listTaskRuns() {
      calls.taskRuns += 1;
      return [taskRun('running')];
    },
    async listRunEvents() {
      calls.runEvents += 1;
      return [runEvent(2), runEvent(1)];
    },
    async listTaskEvents() {
      calls.taskEvents += 1;
      return [taskEvent(1)];
    },
    async loadConversation() {
      calls.conversations += 1;
      return [conversation, [message('user-1', 'user', 'Question', 1)]];
    },
    async listTurns() {
      calls.turns += 1;
      return [];
    },
    ...overrides,
  };
  return { calls, port };
}

test('hydration selects the newest resumable run and returns canonical event order', async () => {
  const older = taskRun('running', {
    id: 'run-old',
    turnId: 'turn-old',
    updatedAt: '2026-08-11T00:00:30.000Z',
  });
  const newest = taskRun('queued', {
    id: 'run-1',
    updatedAt: '2026-08-11T00:02:00.000Z',
  });
  const { port, calls } = createPort();
  const reconciler = new DurableRunReconciler(port);

  const outcome = await reconciler.reconcile({
    reason: 'hydration',
    conversationId: 'conversation-1',
    taskRuns: [older, taskRun('completed'), newest],
  });

  assert(outcome.kind === 'active', 'hydration should recover the newest resumable run');
  assertEqual(outcome.snapshot.taskRun.id, 'run-1', 'selected run');
  assertEqual(outcome.snapshot.runEvents[0].eventSeq, 1, 'events are sorted before projection');
  assertEqual(outcome.snapshot.runEvents[1].eventSeq, 2, 'successor follows missing event');
  assertEqual(calls.taskRuns, 0, 'provided hydration runs avoid a duplicate backend query');
});

test('hydration restores either durable event source when the other is temporarily unavailable', async () => {
  const { port } = createPort({
    async listTaskEvents() { throw new Error('task timeline unavailable'); },
  });
  const outcome = await new DurableRunReconciler(port).reconcile({
    reason: 'hydration',
    conversationId: 'conversation-1',
    taskRuns: [taskRun('running')],
  });

  assert(outcome.kind === 'active', 'optional task timeline failure must not hide live Run Events');
  assertEqual(outcome.snapshot.runEvents.length, 2, 'canonical Run Events remain recoverable');
  assertEqual(outcome.snapshot.taskEvents.length, 0, 'failed optional source becomes empty');
});

test('watchdog preserves expected run identity instead of adopting a newer run', async () => {
  const expected = taskRun('running', {
    id: 'run-expected',
    turnId: 'turn-expected',
    updatedAt: '2026-08-11T00:00:30.000Z',
  });
  const newer = taskRun('running', {
    id: 'run-newer',
    turnId: 'turn-newer',
    updatedAt: '2026-08-11T00:02:00.000Z',
  });
  const { port } = createPort({
    async listTaskRuns() { return [newer, expected]; },
    async listRunEvents(runId) { return [{ ...runEvent(1), runId }]; },
  });
  const reconciler = new DurableRunReconciler(port);

  const outcome = await reconciler.reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-expected',
    expectedTurnId: 'turn-expected',
    missingRunConfirmations: 0,
  });

  assert(outcome.kind === 'active', 'expected active run should be recovered');
  assertEqual(outcome.snapshot.taskRun.id, 'run-expected', 'newer unrelated run is ignored');
});

test('missing expected run becomes terminal only after three durable confirmations', async () => {
  const { port } = createPort({ async listTaskRuns() { return []; } });
  const reconciler = new DurableRunReconciler(port);

  const retry = await reconciler.reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-missing',
    expectedTurnId: 'turn-missing',
    missingRunConfirmations: 1,
  });
  assert(retry.kind === 'missing', 'missing expected run returns a typed retry');
  assertEqual(retry.confirmations, 2, 'confirmation count advances once per query');
  assertEqual(retry.exhausted, false, 'two confirmations are recoverable');

  const exhausted = await reconciler.reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-missing',
    expectedTurnId: 'turn-missing',
    missingRunConfirmations: 2,
  });
  assert(exhausted.kind === 'missing', 'third miss remains explicit');
  assertEqual(exhausted.confirmations, 3, 'third confirmation is recorded');
  assertEqual(exhausted.exhausted, true, 'third confirmation closes recovery');
});

test('completed reconciliation waits for the final message and joins it to the exact turn', async () => {
  const completed = taskRun('completed');
  const final = message('assistant-1', 'assistant', 'Final answer', 2);
  const { port } = createPort({
    async listTaskRuns() { return [completed]; },
    async loadConversation() {
      return [{} as Conversation, [message('user-1', 'user', 'Question', 1), final]];
    },
    async listTurns(): Promise<ConversationTurn[]> {
      return [{
        id: 'turn-1',
        conversationId: 'conversation-1',
        userMessageId: 'user-1',
        assistantMessageId: 'assistant-1',
        status: 'completed',
        createdAt: '2026-08-11T00:00:00.000Z',
        updatedAt: '2026-08-11T00:01:00.000Z',
      }];
    },
  });
  const reconciler = new DurableRunReconciler(port);

  const completedOutcome = await reconciler.reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-1',
    expectedTurnId: 'turn-1',
    missingRunConfirmations: 0,
  });
  assert(completedOutcome.kind === 'completed', 'durable final answer settles completion');
  assertEqual(completedOutcome.finalMessage.id, 'assistant-1', 'turn-linked message wins');

  const pendingPort = createPort({
    async listTaskRuns() { return [completed]; },
  }).port;
  const pending = await new DurableRunReconciler(pendingPort).reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-1',
    expectedTurnId: 'turn-1',
    missingRunConfirmations: 0,
  });
  assert(pending.kind === 'pending', 'completion without a durable answer must retry');
  assertEqual(pending.reason, 'finalMessage', 'pending reason is typed');
});

test('suspensions and terminal task states are classified without duplicating projection logic', async () => {
  for (const [run, expectedKind] of [
    [taskRun('paused'), 'suspended'],
    [taskRun('running', { phase: 'awaiting_user_input' }), 'suspended'],
    [taskRun('cancelled'), 'terminal'],
    [taskRun('failed'), 'terminal'],
    [taskRun('timed_out'), 'terminal'],
  ] as const) {
    const { port } = createPort({ async listTaskRuns() { return [run]; } });
    const outcome = await new DurableRunReconciler(port).reconcile({
      reason: 'watchdog',
      conversationId: 'conversation-1',
      expectedRunId: 'run-1',
      expectedTurnId: 'turn-1',
      missingRunConfirmations: 0,
    });
    assertEqual(outcome.kind, expectedKind, `${run.status}/${run.phase} classification`);
  }
});

test('hydration restores paused and awaiting-input runs as suspensions', async () => {
  for (const run of [
    taskRun('paused'),
    taskRun('running', { phase: 'awaiting_user_input' }),
  ]) {
    const { port } = createPort({ async listTaskRuns() { return [run]; } });
    const outcome = await new DurableRunReconciler(port).reconcile({
      reason: 'hydration',
      conversationId: 'conversation-1',
    });
    assertEqual(outcome.kind, 'suspended', `${run.status}/${run.phase} hydration`);
  }
});

test('typed task lifecycle helpers preserve stop and pause semantics', () => {
  const running = taskRun('running');
  const awaiting = taskRun('running', { phase: 'awaiting_user_input' });
  const paused = taskRun('paused');

  assertEqual(taskRunIsActive(running), true, 'running task is active');
  assertEqual(taskRunCanAcceptStop(running), true, 'running task can stop');
  assertEqual(taskRunIsSuspended(awaiting), true, 'awaiting input is suspended');
  assertEqual(taskRunIsActive(awaiting), false, 'awaiting input cannot be paused again');
  assertEqual(taskRunCanAcceptStop(awaiting), true, 'awaiting input can still be stopped');
  assertEqual(taskRunCanAcceptStop(paused), false, 'paused task resumes instead of stopping');
});

test('stale generations are rejected between durable queries', async () => {
  let current = true;
  const { port, calls } = createPort({
    async listTaskRuns() {
      current = false;
      return [taskRun('running')];
    },
  });
  const outcome = await new DurableRunReconciler(port).reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-1',
    expectedTurnId: 'turn-1',
    missingRunConfirmations: 0,
    isCurrent: () => current,
  });
  assertEqual(outcome.kind, 'stale', 'stale result is discarded');
  assertEqual(calls.runEvents, 0, 'no later query runs after staleness is observed');
});

test('backend query failures return a typed unavailable outcome', async () => {
  const { port, calls } = createPort({
    async listTaskRuns() { throw new Error('database busy'); },
  });
  const outcome = await new DurableRunReconciler(port).reconcile({
    reason: 'watchdog',
    conversationId: 'conversation-1',
    expectedRunId: 'run-1',
    expectedTurnId: 'turn-1',
    missingRunConfirmations: 2,
  });

  assert(outcome.kind === 'unavailable', 'query failure should not escape the interface');
  assert(outcome.error.includes('database busy'), 'typed failure preserves the cause');
  assertEqual(calls.runEvents, 0, 'failed task lookup does not start dependent queries');
});

test('gap recovery owns canonical ordering, backoff, and exhaustion', async () => {
  let queries = 0;
  const delays: number[] = [];
  const { port } = createPort({
    async listRunEvents() {
      queries += 1;
      return [runEvent(3), runEvent(1), runEvent(2)];
    },
  });
  const reconciler = new DurableRunReconciler(port, {
    delay: async delayMs => { delays.push(delayMs); },
  });
  const observed: number[][] = [];
  const recovered = await reconciler.recoverGap({
    runId: 'run-1',
    isCurrent: () => true,
    accept(events) {
      observed.push(events.map(event => event.eventSeq));
      return observed.length < 3;
    },
  });
  assertEqual(recovered.kind, 'recovered', 'third replay closes the gap');
  assertEqual(queries, 3, 'reconciler owns repeated durable queries');
  assertEqual(observed[0].join(','), '1,2,3', 'each replay is canonical');
  assertEqual(delays.join(','), '250,500', 'retry backoff is deterministic');

  const exhausted = await reconciler.recoverGap({
    runId: 'run-1',
    isCurrent: () => true,
    accept: () => true,
  });
  assertEqual(exhausted.kind, 'exhausted', 'four unresolved queries exhaust recovery');
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
