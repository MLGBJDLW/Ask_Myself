declare const require: (id: string) => unknown;
declare const process: { cwd(): string };

const { existsSync, readFileSync } = require('fs') as {
  existsSync(path: string): boolean;
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

test('canonical Run Events have no task-event compatibility bridge', () => {
  const adapterPath = join(process.cwd(), 'src/lib/streaming/legacyAdapter.ts');
  assert(!existsSync(adapterPath), 'legacyAdapter.ts must stay deleted');

  const store = source('src/lib/streamStore.ts');
  for (const symbol of [
    'adaptFrontendRunEvent',
    'restoreFromTaskEvents',
    'restoreFromHistoricalEvents',
  ]) {
    assert(!store.includes(symbol), `streamStore must not expose ${symbol}`);
  }

  const taskCenter = source('src/lib/streaming/taskCenterHistory.ts');
  assert(
    !taskCenter.includes('legacyTaskCenterHistoryFromTaskEvents'),
    'Task Center must not replay AgentRunEvent wrappers from task events',
  );

  const agentRun = source('../../crates/core/src/agent_run.rs');
  assert(
    !agentRun.includes('task_event_payload'),
    'AgentRunEvent must not know how to wrap itself as an AgentTaskRunEvent',
  );

  const taskEvents = source('src-tauri/src/agent_task_events.rs');
  assert(
    !taskEvents.includes('record_agent_run_task_event'),
    'desktop runtime must not dual-write Run Events as task events',
  );

  const directDispatch = source('../../crates/core/src/agent/direct_dispatch_runner.rs');
  assert(
    !directDispatch.includes('AgentEvent::ToolCallStart'),
    'direct dispatch must expose one canonical tool lifecycle',
  );
});

test('Tool execution has one context-based entry point', () => {
  const tools = source('../../crates/core/src/tools/mod.rs');
  assert(!tools.includes('execute_with_context'), 'Tool API must not expose execute_with_context');
  assert(!tools.includes('execute_with_run_context'), 'Tool API must not expose a transitional run entry point');
  assert(
    /async fn execute\(\s*&self,\s*context: ToolExecutionContext<'_>,?\s*\)/m.test(tools),
    'Tool::execute must receive ToolExecutionContext',
  );

  for (const relativePath of [
    '../../crates/core/src/agent/direct_dispatch_runner.rs',
    '../../crates/core/src/agent/tool_dispatch.rs',
  ]) {
    const runtime = source(relativePath);
    assert(!runtime.includes('execute_with_context'), `${relativePath} must use ToolRegistry::execute`);
    assert(!runtime.includes('execute_with_run_context'), `${relativePath} must use ToolRegistry::execute`);
  }

  for (const relativePath of [
    '../../crates/core/src/tools/create_file_tool.rs',
    '../../crates/core/src/tools/edit_file_tool.rs',
    '../../crates/core/src/tools/multi_edit_tool.rs',
    '../../crates/core/src/tools/write_note_tool.rs',
  ]) {
    assert(!source(relativePath).includes('execute_impl('), `${relativePath} must implement the canonical entry point directly`);
  }
});

test('tool approvals use structured permission policies exclusively', () => {
  const approval = source('../../crates/core/src/approval.rs');
  const session = source('src-tauri/src/desktop_agent_session.rs');

  for (const symbol of [
    'get_tool_approval_policy',
    'save_tool_approval_policy',
    'delete_tool_approval_policy',
  ]) {
    assert(!approval.includes(symbol), `approval storage must not expose ${symbol}`);
    assert(!session.includes(symbol), `desktop runtime must not fall back through ${symbol}`);
  }
  assert(
    approval.includes('resolve_tool_permission_policy'),
    'approval storage must resolve exact and wildcard structured policies',
  );
  assert(
    !approval.includes('pub permission_key: Option<String>'),
    'persisted permission policies must not model legacy rows with optional keys',
  );
});

test('built-in capabilities are described directly as capability packages', () => {
  const packages = source('../../crates/core/src/plugins.rs');
  const api = source('src/lib/api.ts');
  const types = source('src/types/conversation.ts');

  for (const legacyName of [
    'PluginManifest',
    'BuiltinPlugin',
    'ToolPluginInfo',
    'builtin_plugin_manifests',
    'plugin_for_tool',
  ]) {
    assert(!packages.includes(legacyName), `capability package registry must not retain ${legacyName}`);
  }
  assert(!api.includes('listBuiltinPlugins'), 'desktop API must expose capability packages');
  assert(!types.includes('interface PluginManifest'), 'frontend must project CapabilityPackageView');
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
