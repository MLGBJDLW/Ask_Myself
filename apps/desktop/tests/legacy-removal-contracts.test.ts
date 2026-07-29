declare const require: (id: string) => unknown;
declare const process: { cwd(): string };

const { readFileSync } = require('fs') as {
  readFileSync(path: string, encoding: string): string;
};
const { join } = require('path') as {
  join(...paths: string[]): string;
};

type TestFn = () => void;

const tests: Array<{ name: string; fn: TestFn }> = [];

function test(name: string, fn: TestFn): void {
  tests.push({ name, fn });
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function source(relativePath: string): string {
  return readFileSync(join(process.cwd(), relativePath), 'utf8');
}

test('desktop IPC sends the versioned agent request through one canonical argument', () => {
  const api = source('src/lib/api.ts');
  assert(!api.includes('...request,'), 'agentChat must not mirror request fields at the command root');
});

test('stream types are imported from their authoritative protocol instead of a hook shim', () => {
  const hook = source('src/lib/useAgentStream.ts');
  assert(
    !hook.includes('Re-export types from streamStore for backward compatibility'),
    'useAgentStream must not retain the compatibility type forwarding layer',
  );
  assert(
    !hook.includes("import('./streamStore').UsageTotal"),
    'useAgentStream must consume UsageTotal from the canonical protocol',
  );
});

test('desktop agent sessions use the structured approval callback exclusively', () => {
  const session = source('src-tauri/src/desktop_agent_session.rs');
  assert(
    !session.includes('build_desktop_confirmation_callback'),
    'desktop must not construct the shadowed confirmation callback',
  );
  assert(
    !session.includes('.with_confirmation_callback('),
    'desktop must not wire the legacy confirmation callback',
  );
});

test('Whisper model metadata exposes only the active safetensors layout', () => {
  const video = source('../../crates/core/src/video.rs');
  assert(!video.includes('pub fn filename('), 'legacy GGML filename helper must stay removed');
});

test('skill catalog deduplicates exact IDs without hiding user prefixes', () => {
  const api = source('src/lib/api.ts');
  assert(
    !api.includes('isLegacyBuiltinSkillRow'),
    'skill catalog must not classify user rows by builtin flag or ID prefix',
  );
});

for (const { name, fn } of tests) {
  try {
    fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.error(`not ok - ${name}`);
    throw error;
  }
}
