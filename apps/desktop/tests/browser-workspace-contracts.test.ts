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
  const runtimeModule = source('../../crates/core/src/browser_runtime/mod.rs');
  const browserModule = source('src-tauri/src/browser/mod.rs');
  const browser = source('src-tauri/src/browser/state.rs');
  const agentTool = source('src-tauri/src/browser/agent_tool.rs');
  const commands = source('src-tauri/src/browser/commands.rs');
  const dock = source('src/features/browser/BrowserDock.tsx');
  const api = source('src/lib/api.ts');
  assert(!runtimeModule.includes('mod runtime;'), 'a single pass-through BrowserRuntime trait must not masquerade as a multi-engine seam');
  assert(!runtimeModule.includes('mod events;'), 'unproduced BrowserRuntime events must not remain as a second lifecycle vocabulary');
  assert(!browserModule.includes('runtime_adapter'), 'BrowserState callers must not pay for a one-implementation forwarding adapter');
  assert(browser.includes('BrowserControlOwner'), 'runtime must model Agent/User control ownership');
  assert(browser.includes('observations'), 'runtime must retain observation identity for stale checks');
  assert(agentTool.includes('observe'), 'Agent adapter must observe the same native runtime');
  assert(agentTool.includes('requires_confirmation'), 'Agent adapter must implement risk-tiered approval');
  assert(browser.includes('open_popup'), 'popup tabs must preserve the initiating native tab policy');
  assert(dock.includes('openBrowserPopup'), 'popup events must use the policy-preserving host command');
  assert(browser.includes('record_user_takeover'), 'trusted direct page input must revoke the Agent control lease');
  assert(browser.includes('approved_agent_urls'), 'Agent redirects must fail closed unless their resolved URL was preapproved');
  assert(browser.includes('network_proxy: Arc<BrowserNetworkProxy>'), 'all tabs in a session must share one stable policy proxy');
  assert(browser.includes('dispatch_agent_action'), 'Agent action validation and WebView dispatch must share one runtime lock');
  assert(browser.includes('revalidate_agent_lease'), 'Agent observations must not cross a user takeover');
  assert(browser.includes('prepare_agent_network_access(session_id, tab_id, &snapshot_url)'), 'observations must revalidate the captured URL and refresh any conversation-scoped local-service permit');
  assert(browser.includes('managed_loopback_permits(conversation_id)'), 'local browser access must originate from a live service owned by the same conversation');
  assert(browser.includes('dispatched_url != snapshot_url'), 'observations must reject navigation during snapshot validation');
  assert(
    browser.includes('dispatch_browser_navigation(commit_tracker, || {')
      && browser.includes('.navigate(url.clone())'),
    'navigation dispatch and its commit tracker must remain atomic with the checked lease',
  );
  assert(
    browser.includes('Agent browser navigation requires durable commit tracking before dispatch'),
    'Agent navigation must fail before dispatch when durable commit tracking is absent',
  );
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
  assert(dock.includes('current?.conversationId === conversationId'), 'session reuse must be scoped to the active conversation');
  assert(dock.includes('sessionScopeOwnsCurrent'), 'async session results must retain their conversation and session ownership guard');
  assert(dock.includes('{onSendArtifactToAgent && ('), 'agent artifact controls require a live artifact recipient');
  assert(dock.includes('!onSendArtifactToAgent) return'), 'artifact capture must fail closed without a recipient');
  assert(api.includes('visibilityRevision: number'), 'session snapshots must expose the authoritative visibility revision');
  assert(api.includes('visibilityRequested: boolean'), 'missed visibility events need a durable snapshot flag');
  assert(api.includes('visibilityRequestRevision?: number | null'), 'visibility acknowledgements must retain their minimum revision');
  assert(dock.includes('recordMinimumVisibilityRevision'), 'frontend visibility writes must advance beyond backend revision fences');
  assert(dock.includes('sessionPromisesRef = useRef(new Map'), 'pending session creation must be scoped by conversation');
  assert(dock.includes('MAX_BROWSER_TABS_PER_SESSION = 16'), 'frontend popup admission must match the authoritative backend session cap');
});

test('Agent Browser Workspace sessions become visible and every observation carries visual proof', () => {
  const types = source('../../crates/core/src/browser_runtime/types.rs');
  const host = source('src-tauri/src/browser/webview_host.rs');
  const browser = source('src-tauri/src/browser/state.rs');
  const agentTool = source('src-tauri/src/browser/agent_tool.rs');
  const streamBridge = source('src-tauri/src/agent_stream_bridge.rs');
  const dock = source('src/features/browser/BrowserDock.tsx');
  const toolCard = source('src/components/chat/ToolCallCard.tsx');
  assert(types.includes('pub screenshot: Option<BrowserScreenshot>'), 'browser observations need a typed screenshot channel');
  assert(host.includes('Page.captureScreenshot'), 'native observations must capture the exact shared WebView page');
  assert(browser.includes('wait_until_workspace_visible'), 'Agent observation must wait for real dock bounds instead of inspecting a hidden 1x1 view');
  assert(browser.includes('"requestVisible": actor == NavigationActor::Agent'), 'Agent-created sessions must ask the owning UI to reveal the shared browser');
  assert(browser.includes('"conversationId": conversation_id'), 'visibility events must be scoped to the active conversation');
  assert(agentTool.includes('browser_screenshot_attachment'), 'browser observations must return image proof through the tool attachment channel');
  assert(streamBridge.includes('AgentRunEventPersistence::Ephemeral'), 'raw screenshot proof must never enter durable Run Event storage');
  assert(streamBridge.includes('is_current_turn_visual_evidence'), 'visual evidence needs an explicit live-event boundary');
  assert(toolCard.includes('extractToolVisualEvidence'), 'tool cards must validate visual evidence before rendering it');
  assert(toolCard.includes('data-testid="tool-visual-evidence"'), 'users must see the captured browser or desktop screenshot in the tool card');
  assert(toolCard.includes('MAX_TOOL_VISUAL_EVIDENCE_BASE64_BYTES'), 'the frontend must bound current-turn screenshot payloads');
  assert(dock.includes("event.payload.kind === 'sessionCreated'"), 'the Browser Dock must react to Agent-created sessions');
  assert(dock.includes('requestVisible'), 'the Browser Dock must distinguish Agent visibility requests from background refreshes');
  assert(dock.includes('onOpenChange(true)'), 'Agent visibility requests must open the shared Browser Dock');
});

test('last-tab cleanup failure exposes authoritative retry state instead of a false empty workspace', () => {
  const dock = source('src/features/browser/BrowserDock.tsx');
  const api = source('src/lib/api.ts');
  const closeTab = dock.slice(
    dock.indexOf('const closeTab = useCallback'),
    dock.indexOf('const retryCloseSession = useCallback'),
  );

  assert(api.includes('cleanupPending: boolean'), 'session snapshots must expose typed cleanup state');
  assert(
    closeTab.includes('await api.closeBrowserSession(session.id)')
      && closeTab.includes('const retained = await api.activeBrowserSession(conversationId)')
      && closeTab.includes('commitSession(verificationScope, retained)'),
    'a failed final close must read and commit the retained CleanupPending session',
  );
  assert(
    dock.includes("session?.cleanupPending ? t('browser.cleanupPending') : t('browser.empty')")
      && dock.includes('data-testid="browser-retry-close-session"'),
    'only authoritative CleanupPending sessions should render the retry state',
  );
  assert(
    dock.includes('disabled={busy || session?.cleanupPending}'),
    'the new-tab control must stay disabled while terminal cleanup is pending',
  );
});

test('browser close schemas expose their explicit terminal targets', () => {
  const nativeTool = source('src-tauri/src/browser/agent_tool.rs');
  const sharedTool = source('../../crates/core/src/tools/browser_session_tool.rs');

  assert(
    nativeTool.includes('browser_session_action_schema_variants')
      && nativeTool.includes('&session_optional_actions')
      && nativeTool.includes('!matches!(*action, "close_tab" | "close_session")'),
    'the native Browser Workspace schema must reuse the terminal target contract',
  );
  assert(
    sharedTool.includes('"properties": { "action": { "enum": session_required_actions } }')
      && sharedTool.includes('"required": ["sessionId"]'),
    'close_session must advertise its explicit sessionId requirement',
  );
  assert(
    /"close_tab"[\s\S]*"required": \["sessionId", "tabId"\]/.test(sharedTool),
    'close_tab must advertise its explicit sessionId and tabId requirements',
  );
});

test('HTTP links have one in-app destination and the duplicate web preview is removed', () => {
  const preview = source('src/features/preview/FilePreviewProvider.tsx');
  const previewApi = source('src/lib/api.ts');
  const previewCommands = source('src-tauri/src/commands/preview.rs');
  const router = source('src/features/browser/openNexaBrowser.ts');
  const markdown = source('src/components/chat/markdownComponents.tsx');
  const app = source('src/App.tsx');
  const chatPage = source('src/pages/ChatPage.tsx');
  const searchEvidence = source('src/components/EvidenceCard.tsx');
  assert(router.includes('OPEN_BROWSER_WORKSPACE_EVENT'), 'links must use the first-class Browser Workspace');
  assert(router.includes('cancelable: true'), 'Open in Browser delivery must be observable by the preview');
  assert(router.includes('dispatchEvent'), 'HTTP(S) opening must have one Browser Workspace routing owner');
  assert(preview.includes('openNexaBrowser(trimmed, title)'), 'file and document links must route to Nexa Browser');
  assert(!preview.includes('webPreview'), 'the duplicate web-preview state and iframe must be deleted');
  assert(!previewApi.includes('probeWebPreview'), 'the retired web-preview API must be deleted');
  assert(!previewCommands.includes('probe_web_preview'), 'the retired native web fetcher must be deleted');
  assert(!preview.includes('target="_blank"'), 'document hyperlinks must not bypass Nexa Browser');
  assert(markdown.includes('openWebLink'), 'chat hyperlinks must use the shared Nexa Browser route');
  assert(app.includes('<GlobalBrowserDock />'), 'non-chat pages need a mounted Nexa Browser destination');
  assert(
    chatPage.includes('onSendArtifactToAgent={isArchivedConversation ? undefined : handleBrowserArtifact}'),
    'archived chats must not expose artifact actions that cannot reach the read-only composer',
  );
  assert(app.includes('data-testid="app-workspace"'), 'global browser and routed content need one horizontal workspace');
  assert(app.includes('className="flex h-full min-h-0 min-w-0 overflow-hidden"'), 'global browser workspace must dock horizontally');
  assert(preview.includes('dirtyRef.current && !window.confirm(labels.discardPrompt)'), 'dirty previews must confirm before routing a web link');
  assert(preview.includes('setOpen(false)'), 'an acknowledged web link must close the covering file preview');
  assert(searchEvidence.includes('openWebLink(card.documentPath'), 'search result URLs must use Nexa Browser');
  assert(!searchEvidence.includes('openExternal(card.documentPath'), 'search result URLs must not bypass Nexa Browser');
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
