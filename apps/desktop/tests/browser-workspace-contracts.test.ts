// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

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

test('remote browser webviews do not inherit the main application capability', () => {
  const capability = JSON.parse(source('src-tauri/capabilities/default.json')) as {
    windows?: string[];
    webviews?: string[];
  };
  assert(!capability.windows?.includes('main'), 'window-wide capability would grant every child webview IPC access');
  assert(capability.webviews?.length === 1, 'only one privileged webview should be declared');
  assert(capability.webviews?.[0] === 'main', 'only the main application webview should be privileged');
});

test('Browser Workspace uses a native top-level child webview instead of an iframe', () => {
  const host = source('src-tauri/src/browser/webview_host.rs');
  const dock = source('src/features/browser/BrowserDock.tsx');
  assert(host.includes('WebviewBuilder::new'), 'desktop host must create a native child WebView');
  assert(host.includes('WebviewUrl::External'), 'remote pages must load as top-level WebView documents');
  assert(host.includes('.data_directory('), 'browser profiles need an isolated data directory');
  assert(!host.includes('.enable_clipboard_access()'), 'remote pages must not receive unconditional JavaScript clipboard access');
  assert(host.includes('.initialization_script_for_all_frames('), 'trusted-input takeover must cover embedded frames');
  assert(!host.includes('USER_TAKEOVER_TITLE_SIGNAL'), 'page-controlled titles must never authenticate user takeover');
  assert(!dock.includes('<iframe'), 'BrowserDock must not regress to iframe embedding');
});

test('Browser Workspace exposes shared sessions, control leases, and observation-scoped artifacts', () => {
  const runtime = source('../../crates/core/src/browser_runtime/runtime.rs');
  const browser = source('src-tauri/src/browser/state.rs');
  const agentTool = source('src-tauri/src/browser/agent_tool.rs');
  const dock = source('src/features/browser/BrowserDock.tsx');
  assert(runtime.includes('trait BrowserRuntime'), 'core must expose an engine-neutral BrowserRuntime contract');
  assert(browser.includes('BrowserControlOwner'), 'runtime must model Agent/User control ownership');
  assert(browser.includes('observations'), 'runtime must retain observation identity for stale checks');
  assert(agentTool.includes('observe'), 'Agent adapter must observe the same native runtime');
  assert(agentTool.includes('requires_confirmation'), 'Agent adapter must implement risk-tiered approval');
  assert(browser.includes('open_popup'), 'popup tabs must preserve the initiating native tab policy');
  assert(dock.includes('openBrowserPopup'), 'popup events must use the policy-preserving host command');
  assert(browser.includes('record_user_takeover'), 'trusted direct page input must revoke the Agent control lease');
  assert(browser.includes('approved_agent_urls'), 'Agent redirects must fail closed unless their resolved URL was preapproved');
  assert(dock.includes('ResizeObserver'), 'native child WebView must follow the dock content bounds');
  assert(dock.includes('beginBrowserElementPick'), 'dock must support point-out element mode');
  assert(dock.includes('beginBrowserRegionPick'), 'dock must support coordinate-region fallback');
  assert(dock.includes('session?.conversationId === conversationId'), 'session reuse must be scoped to the active conversation');
});

test('Reader Preview remains a distinct safe reading mode with Browser as the primary remote action', () => {
  const preview = source('src/features/preview/FilePreviewProvider.tsx');
  assert(preview.includes('OPEN_BROWSER_WORKSPACE_EVENT'), 'remote previews must offer the first-class Browser Workspace');
  assert(preview.includes("t('preview.safeReadingMode')"), 'the sanitized srcDoc surface must be named Safe Reading Mode');
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
