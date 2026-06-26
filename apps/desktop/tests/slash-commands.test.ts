import {
  buildSlashCommandOptions,
  getMatchingSlashCommands,
  getSlashCommandTrigger,
  resolveSlashCommandMessage,
} from '../src/lib/slashCommands';
import {
  buildGoalContinuationLlmContext,
  getActiveGoalContext,
  mergeGoalContextArtifact,
} from '../src/lib/goalContext';
import type { ConversationMessage } from '../src/types/conversation';
import type { Skill } from '../src/types/extensions';

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

function skill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: 'builtin-frontend-design',
    name: 'frontend-design',
    description: 'Create polished production-grade frontend interfaces.',
    content: 'Use this skill when building UI.',
    enabled: true,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    builtin: true,
    interface: {
      displayName: 'Frontend Design',
      shortDescription: 'Design and implement refined UI.',
      defaultPrompt: 'Use frontend-design for this UI task.\n\nTask:\n{{input}}',
    },
    dependencies: { tools: [] },
    policy: { allowImplicitInvocation: true },
    sourcePath: null,
    resources: [],
    ...overrides,
  };
}

function message(
  id: string,
  role: ConversationMessage['role'],
  content: string,
  artifacts: ConversationMessage['artifacts'] = null,
): ConversationMessage {
  return {
    id,
    conversationId: 'conversation-1',
    role,
    content,
    toolCallId: null,
    toolCalls: [],
    artifacts,
    tokenCount: 0,
    createdAt: `2026-06-26T00:00:0${id.length}.000Z`,
    sortOrder: id.length,
    thinking: null,
  };
}

test('detects slash command trigger without matching urls', () => {
  assert(getSlashCommandTrigger('/pla', 4)?.query === 'pla', 'detects leading slash query');
  assert(getSlashCommandTrigger('please /pla', 11)?.query === 'pla', 'detects whitespace-prefixed slash query');
  assertEqual(getSlashCommandTrigger('https://example.com/a', 21), null, 'does not detect URL slashes');
});

test('builds direct skill slash commands', () => {
  const options = buildSlashCommandOptions([skill()], []);
  const matches = getMatchingSlashCommands(options, 'front');
  const frontend = matches.find((option) => option.skillId === 'builtin-frontend-design');
  assert(frontend, 'frontend skill command should be returned');
  assertEqual(frontend.name, 'frontend-design', 'skill command uses readable slash slug');
});

test('resolves skill command into a pinned skill and clean prompt', () => {
  const options = buildSlashCommandOptions([skill()], []);
  const resolved = resolveSlashCommandMessage('/frontend-design build a dense dashboard', options);
  assert(resolved, 'skill slash command should resolve');
  assertEqual(resolved.skillIds[0], 'builtin-frontend-design', 'skill id is pinned');
  assert(
    resolved.message.includes('Use frontend-design for this UI task.'),
    'default skill prompt should be expanded',
  );
  assert(resolved.message.includes('build a dense dashboard'), 'user task should be preserved');
});

test('resolves workflow command templates', () => {
  const options = buildSlashCommandOptions([], [
    {
      id: 'research_verify',
      label: 'Research + Verify',
      description: 'Evidence workflow.',
      promptTemplate: 'Run research.\n\nGoal:\n{{input}}',
    },
  ]);
  const resolved = resolveSlashCommandMessage('/workflow:research_verify model routing', options);
  assert(resolved, 'workflow slash command should resolve');
  assertEqual(resolved.skillIds.length, 0, 'workflow command does not pin skills');
  assert(resolved.message.includes('spawn_subagent_batch'), 'workflow prompt requires batch tool');
  assert(resolved.message.includes('workflow_template: research_verify'), 'workflow prompt includes template id');
  assert(resolved.message.includes('batch_goal:'), 'workflow prompt includes batch goal key');
  assert(resolved.message.includes('Run research.\n\nGoal:\nmodel routing'), 'workflow template is expanded');
});

test('resolves compact as a local command', () => {
  const options = buildSlashCommandOptions([], []);
  const resolved = resolveSlashCommandMessage('/compact', options);
  assert(resolved, 'compact should resolve');
  assertEqual(resolved.localAction, 'compact', 'compact is local action');
  assertEqual(resolved.message, '', 'compact has no message body');
});

test('resolves goal into a goal-oriented prompt', () => {
  const options = buildSlashCommandOptions([], []);
  const resolved = resolveSlashCommandMessage('/goal ship offline sync', options);
  assert(resolved, 'goal should resolve');
  assertEqual(resolved.skillIds.length, 0, 'goal does not pin skills');
  assertEqual(resolved.displayMessage, 'ship offline sync', 'goal display hides slash syntax');
  assertEqual(resolved.artifact.kind, 'goal', 'goal uses dedicated artifact kind');
  assertEqual(resolved.artifact.objective, 'ship offline sync', 'goal artifact records objective');
  assert(resolved.message.includes('success criteria'), 'goal prompt asks for success criteria');
  assert(resolved.message.includes('future work'), 'goal prompt makes the objective persistent');
  assert(resolved.message.includes('ship offline sync'), 'goal preserves user objective');
});

test('keeps the latest goal active across assistant replies until replaced or completed', () => {
  const active = getActiveGoalContext([
    message('g1', 'user', 'ship offline sync', {
      kind: 'goal',
      objective: 'ship offline sync',
      status: 'active',
    }),
    message('a1', 'assistant', 'Initial plan complete.'),
    message('u2', 'user', 'continue implementation'),
  ]);

  assert(active, 'active goal should survive a completed assistant turn');
  assertEqual(active.objective, 'ship offline sync', 'active goal objective is preserved');
  assertEqual(active.sourceMessageId, 'g1', 'active goal points to the original goal message');

  const replaced = getActiveGoalContext([
    message('g1', 'user', 'ship offline sync', {
      kind: 'goal',
      objective: 'ship offline sync',
      status: 'active',
    }),
    message('g2', 'user', 'fix release publishing', {
      kind: 'goal',
      objective: 'fix release publishing',
      status: 'active',
    }),
  ]);

  assert(replaced, 'replacement goal should become active');
  assertEqual(replaced.objective, 'fix release publishing', 'latest goal wins');

  const completed = getActiveGoalContext([
    message('g1', 'user', 'ship offline sync', {
      kind: 'goal',
      objective: 'ship offline sync',
      status: 'active',
    }),
    message('g2', 'user', 'mark done', {
      kind: 'goal',
      objective: 'ship offline sync',
      status: 'complete',
    }),
  ]);

  assertEqual(completed, null, 'terminal goal artifact clears active context');
});

test('builds hidden continuation context for normal messages under an active goal', () => {
  const goal = {
    objective: 'ship offline sync',
    status: 'active' as const,
    sourceMessageId: 'g1',
    createdAt: '2026-06-26T00:00:00.000Z',
  };

  const llmContext = buildGoalContinuationLlmContext(goal, 'continue with tests');
  assert(llmContext.includes('Active conversation goal:'), 'context labels the active goal');
  assert(llmContext.includes('ship offline sync'), 'context includes the active goal objective');
  assert(llmContext.includes('continue with tests'), 'context includes the new user message');

  const artifact = mergeGoalContextArtifact({ kind: 'executionMode', mode: 'plan' }, goal, llmContext);
  assert(!Array.isArray(artifact), 'merged artifact stays object-shaped');
  assertEqual(artifact.kind, 'executionMode', 'existing artifact kind is preserved');
  assertEqual(artifact.llmContextContent, llmContext, 'hidden context is attached at the top level');
});

test('resolves ask into a question card prompt', () => {
  const options = buildSlashCommandOptions([], []);
  const resolved = resolveSlashCommandMessage('/ask clarify deployment', options);
  assert(resolved, 'ask should resolve');
  assert(resolved.message.includes('question-card set'), 'ask prompt requests question cards');
  assert(resolved.message.includes('clarify deployment'), 'ask preserves request');
});

test('resolves plan as execution mode without prompt expansion', () => {
  const options = buildSlashCommandOptions([], []);
  const resolved = resolveSlashCommandMessage('/plan build the reporting dashboard', options);
  assert(resolved, 'plan should resolve');
  assertEqual(resolved.executionMode, 'plan', 'plan enters plan execution mode');
  assertEqual(resolved.message, 'build the reporting dashboard', 'plan preserves the user goal');
  assertEqual(resolved.skillIds.length, 0, 'plan does not pin skills');
  assertEqual(resolved.artifact.executionMode, 'plan', 'plan artifact records execution mode');
});

async function main(): Promise<void> {
  for (const { name, fn } of tests) {
    await fn();
    console.log(`ok - ${name}`);
  }
}

void main();
