import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const conversation = {
      id: 'conv-browser-workspace',
      title: 'Shared Browser Workspace',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const agentConfig = {
      id: 'cfg-browser-workspace',
      name: 'Browser Agent',
      provider: 'open_ai',
      apiKey: '',
      baseUrl: null,
      model: 'gpt-4.1',
      temperature: 0.3,
      maxTokens: 4096,
      contextWindow: 1047576,
      isDefault: true,
      reasoningEnabled: null,
      thinkingBudget: null,
      reasoningEffort: null,
      maxIterations: null,
      summarizationModel: null,
      summarizationProvider: null,
      subagentAllowedTools: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    type BrowserTab = {
      id: string;
      sessionId: string;
      url: string;
      title: string;
      active: boolean;
      loading: boolean;
      status: string;
    };
    type BrowserSession = {
      id: string;
      conversationId: string;
      profileId: string;
      activeTabId: string;
      tabs: BrowserTab[];
      controlOwner: { type: 'user' | 'none' };
    };

    let session: BrowserSession | null = null;
    let nextTab = 1;
    let pickReady = false;
    const browserDiagnostics = {
      creates: [] as Array<Record<string, unknown>>,
      navigations: [] as Array<Record<string, unknown>>,
      bounds: [] as Array<Record<string, unknown>>,
      picks: [] as string[],
      popups: [] as Array<Record<string, unknown>>,
      controls: [] as string[],
    };
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const emitBrowserEvent = (payload: Record<string, unknown>) => {
      for (const [listenerId, listener] of listeners.entries()) {
        if (listener.event !== 'browser:event') continue;
        callbackMap.get(listener.handlerId)?.({
          event: 'browser:event',
          id: listenerId,
          payload,
        });
      }
    };

    const normalizeTabs = () => {
      if (!session) return;
      for (const tab of session.tabs) tab.active = tab.id === session.activeTabId;
    };
    const newTab = (url: string): BrowserTab => ({
      id: `tab-${nextTab++}`,
      sessionId: 'browser-session-1',
      url,
      title: url.includes('example.com') ? 'Example Domain' : 'Google',
      active: true,
      loading: false,
      status: 'idle',
    });

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'plugin:event|listen': {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(args.event ?? ''),
            handlerId: Number(args.handler ?? 0),
          });
          return listenerId;
        }
        case 'plugin:event|unlisten':
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        case 'list_agent_configs_cmd':
          return [clone(agentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), []];
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'get_agent_run_events_cmd':
        case 'get_agent_task_run_events_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_projects_cmd':
        case 'list_personas_cmd':
        case 'terminal_list_sessions_cmd':
          return [];
        case 'browser_active_session_cmd':
          return clone(session);
        case 'browser_list_sessions_cmd':
          return session ? [clone(session)] : [];
        case 'browser_create_session_cmd': {
          browserDiagnostics.creates.push(clone(args));
          const input = (args.input ?? {}) as Record<string, unknown>;
          const tab = newTab(String(input.url ?? 'https://www.google.com'));
          session = {
            id: 'browser-session-1',
            conversationId: String(input.conversationId ?? ''),
            profileId: 'temporary-browser-session-1',
            activeTabId: tab.id,
            tabs: [tab],
            controlOwner: { type: 'user' },
          };
          return clone(session);
        }
        case 'browser_set_bounds_cmd':
          browserDiagnostics.bounds.push(clone(args));
          return null;
        case 'browser_navigate_cmd': {
          if (!session) return null;
          browserDiagnostics.navigations.push(clone(args));
          const tab = session.tabs.find((candidate) => candidate.id === args.tabId);
          if (tab) {
            tab.url = String(args.url ?? '');
            tab.title = 'Example Domain';
          }
          return clone(tab);
        }
        case 'browser_open_tab_cmd': {
          if (!session) return null;
          const tab = newTab(String(args.url ?? 'https://www.google.com'));
          session.activeTabId = tab.id;
          session.tabs.push(tab);
          normalizeTabs();
          return clone(tab);
        }
        case 'browser_open_popup_cmd': {
          if (!session) return null;
          browserDiagnostics.popups.push(clone(args));
          const tab = newTab(String(args.url ?? 'https://www.google.com'));
          session.activeTabId = tab.id;
          session.tabs.push(tab);
          normalizeTabs();
          return clone(tab);
        }
        case 'browser_activate_tab_cmd':
          if (!session) return null;
          session.activeTabId = String(args.tabId ?? '');
          normalizeTabs();
          return clone(session);
        case 'browser_begin_element_pick_cmd':
          browserDiagnostics.picks.push('element');
          pickReady = true;
          return null;
        case 'browser_begin_region_pick_cmd':
          browserDiagnostics.picks.push('region');
          pickReady = true;
          return null;
        case 'browser_take_pick_cmd':
          if (!pickReady) return null;
          pickReady = false;
          return {
            kind: 'element',
            url: 'https://example.com/',
            title: 'Example Domain',
            ref: 'obs-1:el-1',
            tag: 'a',
            role: 'link',
            name: 'More information',
            href: 'https://iana.org/domains/example',
            inputType: null,
            bounds: { x: 20, y: 30, width: 140, height: 24 },
            locatorFingerprint: { tag: 'a', textHash: 'example' },
            userEpoch: 0,
          };
        case 'browser_selected_text_cmd':
          return 'This domain is for use in illustrative examples.';
        case 'browser_acquire_control_cmd':
          if (session) {
            const owner = String(args.owner ?? 'none') as 'user' | 'none';
            browserDiagnostics.controls.push(owner);
            session.controlOwner = { type: owner };
          }
          return clone(session);
        case 'browser_go_back_cmd':
        case 'browser_go_forward_cmd':
        case 'browser_reload_cmd':
        case 'browser_stop_cmd':
          return null;
        default:
          return null;
      }
    };

    (window as unknown as { __browserDiagnostics__: unknown }).__browserDiagnostics__ = browserDiagnostics;
    (window as unknown as { __emitBrowserEvent__: unknown }).__emitBrowserEvent__ = emitBrowserEvent;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbackMap.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbackMap.delete(id),
      convertFileSrc: (filePath: string) => filePath,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => listeners.delete(eventId),
    };
  });
});

test('opens a shared Browser Workspace and attaches pointed page context', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');

  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await expect(page.getByText('You are controlling this page')).toBeVisible();
  await expect(page.getByTestId('browser-native-surface')).toBeVisible();

  const address = page.getByRole('textbox', { name: 'Browser address or search' });
  await expect(address).toHaveValue('https://www.google.com');
  await address.fill('https://example.com');
  await address.press('Enter');
  await expect(address).toHaveValue('https://example.com');

  await page.evaluate(() => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'newWindowRequested',
      payload: {
        sessionId: 'browser-session-1',
        tabId: 'tab-1',
        url: 'https://example.com/popup',
      },
    });
  });
  await expect(page.getByText('Example Domain', { exact: true })).toHaveCount(2);

  await page.getByRole('button', { name: 'Point out' }).click();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue(/<browser_artifact>/);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue(/More information/);

  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);
  await page.keyboard.press('Control+Shift+B');
  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await page.getByRole('button', { name: 'Hand back' }).click();
  await expect(page.getByText('Shared session ready')).toBeVisible();

  await page.evaluate(() => {
    window.dispatchEvent(new CustomEvent('nexa:open-browser-workspace', {
      detail: { url: 'https://openai.com/unified-browser', title: 'Unified browser' },
      cancelable: true,
    }));
  });
  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await expect(address).toHaveValue('https://openai.com/unified-browser');

  const diagnostics = await page.evaluate(() => (window as unknown as {
    __browserDiagnostics__: {
      creates: Array<Record<string, unknown>>;
      navigations: Array<Record<string, unknown>>;
      bounds: Array<Record<string, unknown>>;
      picks: string[];
      popups: Array<Record<string, unknown>>;
      controls: string[];
    };
  }).__browserDiagnostics__);
  expect(diagnostics.creates).toHaveLength(1);
  expect(diagnostics.creates[0]).toMatchObject({
    input: { openInitialUrlOnReuse: false },
  });
  expect(diagnostics.navigations).toContainEqual({
    sessionId: 'browser-session-1',
    tabId: 'tab-1',
    url: 'https://example.com',
  });
  expect(diagnostics.bounds.some((entry) => entry.visible === true)).toBe(true);
  expect(diagnostics.picks).toEqual(['element']);
  expect(diagnostics.popups).toContainEqual({
    sessionId: 'browser-session-1',
    sourceTabId: 'tab-1',
    url: 'https://example.com/popup',
    bounds: expect.any(Object),
  });
  expect(diagnostics.controls).toContain('none');
});

test('docks the global Browser Workspace beside non-chat content', async ({ page }) => {
  await page.goto('/');

  const routedContent = page.locator('[data-app-area="home"]');
  await expect(routedContent).toBeVisible();

  await page.evaluate(() => {
    window.dispatchEvent(new CustomEvent('nexa:open-browser-workspace', {
      detail: { url: 'https://openai.com/non-chat', title: 'Non-chat browser' },
      cancelable: true,
    }));
  });

  const dock = page.getByTestId('browser-dock');
  await expect(dock).toBeVisible();
  await expect(routedContent).toBeInViewport();

  const bounds = await page.evaluate(() => {
    const content = document.querySelector<HTMLElement>('[data-app-area="home"]')?.getBoundingClientRect();
    const browser = document.querySelector<HTMLElement>('[data-testid="browser-dock"]')?.getBoundingClientRect();
    return content && browser
      ? {
          content: { top: content.top, right: content.right, width: content.width, height: content.height },
          browser: { top: browser.top, left: browser.left, width: browser.width, height: browser.height },
        }
      : null;
  });

  expect(bounds).not.toBeNull();
  expect(Math.abs((bounds?.content.top ?? 0) - (bounds?.browser.top ?? 0))).toBeLessThan(2);
  expect(bounds?.content.right ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual((bounds?.browser.left ?? 0) + 1);
  expect(bounds?.content.width ?? 0).toBeGreaterThan(0);
  expect(bounds?.content.height ?? 0).toBeGreaterThan(0);
});
