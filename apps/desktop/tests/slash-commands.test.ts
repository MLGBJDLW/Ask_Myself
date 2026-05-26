import {
  buildSlashCommandOptions,
  getMatchingSlashCommands,
  getSlashCommandTrigger,
  resolveSlashCommandMessage,
} from '../src/lib/slashCommands';
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

async function main(): Promise<void> {
  for (const { name, fn } of tests) {
    await fn();
    console.log(`ok - ${name}`);
  }
}

void main();
