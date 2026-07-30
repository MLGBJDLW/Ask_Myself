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
  const tauriConfig = source('src-tauri/tauri.conf.json');
  assert(host.includes('WebviewBuilder::new'), 'desktop host must create a native child WebView');
  assert(host.includes('WebviewUrl::External'), 'remote pages must load as top-level WebView documents');
  assert(host.includes('.data_directory('), 'browser profiles need an isolated data directory');
  assert(host.includes('.data_store_identifier('), 'macOS browser profiles need an isolated website data store');
  assert(tauriConfig.includes('"minimumSystemVersion": "14.0"'), 'macOS must support isolated data stores and policy proxies');
  assert(!host.includes('.enable_clipboard_access()'), 'remote pages must not receive unconditional JavaScript clipboard access');
  assert(host.includes('.initialization_script_for_all_frames('), 'trusted-input takeover must cover embedded frames');
  assert(host.includes('.proxy_url('), 'all browser subresources must pass through the network policy proxy');
  assert(host.includes('--proxy-bypass-list=<-loopback>'), 'Windows must not bypass the policy proxy for loopback targets');
  assert(!host.includes('USER_TAKEOVER_TITLE_SIGNAL'), 'page-controlled titles must never authenticate user takeover');
  assert(!dock.includes('<iframe'), 'BrowserDock must not regress to iframe embedding');
});

test('Browser Workspace exposes shared sessions, control leases, and observation-scoped artifacts', () => {
  const runtime = source('../../crates/core/src/browser_runtime/runtime.rs');
  const browser = source('src-tauri/src/browser/state.rs');
  const agentTool = source('src-tauri/src/browser/agent_tool.rs');
  const commands = source('src-tauri/src/browser/commands.rs');
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
  assert(browser.includes('network_proxy: Arc<BrowserNetworkProxy>'), 'all tabs in a session must share one stable policy proxy');
  assert(browser.includes('revalidate_agent_action'), 'Agent actions must recheck the lease after asynchronous validation');
  assert(browser.includes('revalidate_agent_lease'), 'Agent observations must not cross a user takeover');
  assert(browser.includes('tab.webview.navigate(url.clone())'), 'navigation dispatch must remain atomic with its checked lease');
  assert(browser.includes('reload_as_agent'), 'Agent reload must validate and dispatch under one control lease');
  assert(browser.includes('activate_tab_as_agent'), 'Agent tab activation must validate its exact owner');
  assert(browser.includes('close_session_as_agent'), 'Agent session closure must validate its exact owner');
  assert(browser.includes('conversation_creation_lock'), 'session creation must serialize per conversation');
  assert(browser.includes('initializing'), 'partially initialized browser sessions must stay undiscoverable');
  const existingSessionReuse = browser.slice(
    browser.indexOf('if let Some(conversation_id) = conversation_id.as_deref()'),
    browser.indexOf('let session_id = format!'),
  );
  assert(existingSessionReuse.includes('if open_initial_url_on_reuse'), 'session reuse must distinguish explicit URLs from fallback initialization');
  assert(existingSessionReuse.includes('self.open_tab'), 'an explicit URL must still open after a concurrent session wins creation');
  assert(agentTool.includes('tokio::time::timeout(remaining'), 'wait_for must enforce its deadline around observation');
  const activateCommand = commands.slice(
    commands.indexOf('pub fn browser_activate_tab_cmd'),
    commands.indexOf('pub fn browser_set_bounds_cmd'),
  );
  assert(activateCommand.includes('BrowserControlOwner::User'), 'user tab activation must revoke Agent control');
  assert(dock.includes('ResizeObserver'), 'native child WebView must follow the dock content bounds');
  assert(dock.includes('beginBrowserElementPick'), 'dock must support point-out element mode');
  assert(dock.includes('beginBrowserRegionPick'), 'dock must support coordinate-region fallback');
  assert(dock.includes('openInitialUrlOnReuse: Boolean(url)'), 'only explicit Open in Browser URLs may open a tab when creation reuses a session');
  assert(dock.includes('event.preventDefault()'), 'the mounted dock must acknowledge Open in Browser delivery');
  assert(dock.includes('session?.conversationId === conversationId'), 'session reuse must be scoped to the active conversation');
});

test('Reader Preview remains a distinct safe reading mode with Browser as the primary remote action', () => {
  const preview = source('src/features/preview/FilePreviewProvider.tsx');
  assert(preview.includes('OPEN_BROWSER_WORKSPACE_EVENT'), 'remote previews must offer the first-class Browser Workspace');
  assert(preview.includes("t('preview.safeReadingMode')"), 'the sanitized srcDoc surface must be named Safe Reading Mode');
  assert(preview.includes('cancelable: true'), 'Open in Browser delivery must be observable by the preview');
  assert(preview.includes('if (handled) setWebPreview(null)'), 'the safe preview must remain open when no BrowserDock handles the URL');
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
