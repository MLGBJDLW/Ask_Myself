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
    const secondConversation = {
      ...conversation,
      id: 'conv-browser-workspace-b',
      title: 'Shared Browser Workspace B',
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
      controlOwner: { type: 'user' | 'none' | 'agent'; callId?: string };
      workspaceVisible: boolean;
      visibilityRevision: number;
      visibilityRequested: boolean;
      visibilityRequestRevision: number | null;
    };

    const sessions = new Map<string, BrowserSession>();
    let nextTab = 1;
    let nextSession = 1;
    let pickReady = false;
    let deferNextPick = false;
    let deferredPickResolver: (() => void) | null = null;
    let deferNextCreate = false;
    let deferredCreateResolver: (() => void) | null = null;
    let deferNextControl = false;
    let deferredControlResolver: (() => void) | null = null;
    let deferNextSelectedText = false;
    let deferredSelectedTextResolver: (() => void) | null = null;
    let deferFirstActiveSession = new URL(window.location.href).searchParams.has('deferBrowserActiveSession');
    let deferredActiveSessionRejecter: (() => void) | null = null;
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

    const sessionById = (sessionId: unknown) => sessions.get(String(sessionId ?? '')) ?? null;
    const normalizeTabs = (session: BrowserSession) => {
      for (const tab of session.tabs) tab.active = tab.id === session.activeTabId;
    };
    const newTab = (sessionId: string, url: string): BrowserTab => ({
      id: `tab-${nextTab++}`,
      sessionId,
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
          return [clone(conversation), clone(secondConversation)];
        case 'get_conversation_cmd':
          return [clone(args.id === secondConversation.id ? secondConversation : conversation), []];
        case 'get_agent_run_event_page_cmd':
          return {
            events: [],
            durableHighWater: Number(args.durableHighWater ?? args.afterEventSeq ?? 0),
            nextAfterEventSeq: null,
            hasMore: false,
          };
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
        case 'get_recent_queries':
          return [];
        case 'browser_active_session_cmd':
          if (deferFirstActiveSession) {
            deferFirstActiveSession = false;
            return new Promise((_resolve, reject) => {
              deferredActiveSessionRejecter = () => {
                deferredActiveSessionRejecter = null;
                reject(new Error('deferred active session failure'));
              };
            });
          }
          return clone([...sessions.values()].find(candidate => candidate.conversationId === args.conversationId) ?? null);
        case 'browser_list_sessions_cmd':
          return clone([...sessions.values()]);
        case 'browser_create_session_cmd': {
          browserDiagnostics.creates.push(clone(args));
          const input = (args.input ?? {}) as Record<string, unknown>;
          const sessionId = `browser-session-${nextSession++}`;
          const tab = newTab(sessionId, String(input.url ?? 'https://www.google.com'));
          const session: BrowserSession = {
            id: sessionId,
            conversationId: String(input.conversationId ?? ''),
            profileId: `temporary-${sessionId}`,
            activeTabId: tab.id,
            tabs: [tab],
            controlOwner: { type: 'user' },
            workspaceVisible: Boolean(input.bounds),
            visibilityRevision: 0,
            visibilityRequested: false,
            visibilityRequestRevision: null,
          };
          if (deferNextCreate) {
            deferNextCreate = false;
            return new Promise((resolve) => {
              deferredCreateResolver = () => {
                sessions.set(session.id, session);
                deferredCreateResolver = null;
                resolve(clone(session));
              };
            });
          }
          sessions.set(session.id, session);
          return clone(session);
        }
        case 'browser_set_bounds_cmd':
          browserDiagnostics.bounds.push(clone(args));
          {
            const session = sessionById(args.sessionId);
            if (session) {
              const incomingRevision = Number(args.visibilityRevision);
              if (incomingRevision <= session.visibilityRevision) {
                throw new Error(`stale visibility revision ${incomingRevision}`);
              }
              session.workspaceVisible = Boolean(args.visible);
              session.visibilityRevision = incomingRevision;
              if (
                session.visibilityRequestRevision === null
                || Number(args.visibilityRevision) >= session.visibilityRequestRevision
              ) {
                session.visibilityRequested = false;
                session.visibilityRequestRevision = null;
              }
            }
          }
          return null;
        case 'browser_navigate_cmd': {
          const session = sessionById(args.sessionId);
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
          const session = sessionById(args.sessionId);
          if (!session) return null;
          const tab = newTab(session.id, String(args.url ?? 'https://www.google.com'));
          session.activeTabId = tab.id;
          session.tabs.push(tab);
          normalizeTabs(session);
          return clone(tab);
        }
        case 'browser_open_popup_cmd': {
          const session = sessionById(args.sessionId);
          if (!session) return null;
          browserDiagnostics.popups.push(clone(args));
          const tab = newTab(session.id, String(args.url ?? 'https://www.google.com'));
          session.activeTabId = tab.id;
          session.tabs.push(tab);
          normalizeTabs(session);
          return clone(tab);
        }
        case 'browser_activate_tab_cmd': {
          const session = sessionById(args.sessionId);
          if (!session) return null;
          session.activeTabId = String(args.tabId ?? '');
          normalizeTabs(session);
          return clone(session);
        }
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
          const artifact = {
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
          if (deferNextPick) {
            deferNextPick = false;
            return new Promise((resolve) => {
              deferredPickResolver = () => {
                pickReady = false;
                deferredPickResolver = null;
                resolve(clone(artifact));
              };
            });
          }
          pickReady = false;
          return artifact;
        case 'browser_selected_text_cmd': {
          const selectedText = 'This domain is for use in illustrative examples.';
          if (deferNextSelectedText) {
            deferNextSelectedText = false;
            return new Promise((resolve) => {
              deferredSelectedTextResolver = () => {
                deferredSelectedTextResolver = null;
                resolve(selectedText);
              };
            });
          }
          return selectedText;
        }
        case 'browser_acquire_control_cmd': {
          const session = sessionById(args.sessionId);
          if (session) {
            const owner = String(args.owner ?? 'none') as 'user' | 'none';
            browserDiagnostics.controls.push(owner);
            session.controlOwner = { type: owner };
          }
          const result = clone(session);
          if (deferNextControl) {
            deferNextControl = false;
            return new Promise((resolve) => {
              deferredControlResolver = () => {
                deferredControlResolver = null;
                resolve(result);
              };
            });
          }
          return result;
        }
        case 'browser_close_tab_cmd': {
          const session = sessionById(args.sessionId);
          if (!session) return null;
          session.tabs = session.tabs.filter(tab => tab.id !== args.tabId);
          if (session.activeTabId === args.tabId) {
            session.activeTabId = session.tabs[0]?.id ?? '';
          }
          normalizeTabs(session);
          return clone(session);
        }
        case 'browser_close_session_cmd':
          sessions.delete(String(args.sessionId ?? ''));
          return null;
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
    (window as unknown as { __deferNextBrowserPick__: () => void }).__deferNextBrowserPick__ = () => {
      deferNextPick = true;
    };
    (window as unknown as { __deferredBrowserPickPending__: () => boolean }).__deferredBrowserPickPending__ = () => (
      deferredPickResolver !== null
    );
    (window as unknown as { __resolveDeferredBrowserPick__: () => void }).__resolveDeferredBrowserPick__ = () => {
      deferredPickResolver?.();
    };
    (window as unknown as { __deferNextBrowserCreate__: () => void }).__deferNextBrowserCreate__ = () => {
      deferNextCreate = true;
    };
    (window as unknown as { __deferredBrowserCreatePending__: () => boolean }).__deferredBrowserCreatePending__ = () => (
      deferredCreateResolver !== null
    );
    (window as unknown as { __resolveDeferredBrowserCreate__: () => void }).__resolveDeferredBrowserCreate__ = () => {
      deferredCreateResolver?.();
    };
    (window as unknown as { __deferNextBrowserControl__: () => void }).__deferNextBrowserControl__ = () => {
      deferNextControl = true;
    };
    (window as unknown as { __deferredBrowserControlPending__: () => boolean }).__deferredBrowserControlPending__ = () => (
      deferredControlResolver !== null
    );
    (window as unknown as { __resolveDeferredBrowserControl__: () => void }).__resolveDeferredBrowserControl__ = () => {
      deferredControlResolver?.();
    };
    (window as unknown as { __deferNextBrowserSelectedText__: () => void }).__deferNextBrowserSelectedText__ = () => {
      deferNextSelectedText = true;
    };
    (window as unknown as { __deferredBrowserSelectedTextPending__: () => boolean }).__deferredBrowserSelectedTextPending__ = () => (
      deferredSelectedTextResolver !== null
    );
    (window as unknown as { __resolveDeferredBrowserSelectedText__: () => void }).__resolveDeferredBrowserSelectedText__ = () => {
      deferredSelectedTextResolver?.();
    };
    (window as unknown as { __deferredBrowserActiveSessionPending__: () => boolean }).__deferredBrowserActiveSessionPending__ = () => (
      deferredActiveSessionRejecter !== null
    );
    (window as unknown as { __rejectDeferredBrowserActiveSession__: () => void }).__rejectDeferredBrowserActiveSession__ = () => {
      deferredActiveSessionRejecter?.();
    };
    (window as unknown as {
      __seedBrowserVisibilityRequest__: (conversationId: string, minimumRevision: number) => string;
    }).__seedBrowserVisibilityRequest__ = (conversationId, minimumRevision) => {
      const sessionId = `browser-session-${nextSession++}`;
      const tab = newTab(sessionId, 'https://example.com/visibility-request');
      const seeded: BrowserSession = {
        id: sessionId,
        conversationId,
        profileId: `temporary-${sessionId}`,
        activeTabId: tab.id,
        tabs: [tab],
        controlOwner: { type: 'agent', callId: 'call-visibility' },
        workspaceVisible: false,
        visibilityRevision: minimumRevision - 1,
        visibilityRequested: true,
        visibilityRequestRevision: minimumRevision,
      };
      sessions.set(sessionId, seeded);
      return sessionId;
    };
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
  const visibilityRevisions = diagnostics.bounds.map((entry) => Number(entry.visibilityRevision));
  expect(visibilityRevisions.every((revision) => Number.isSafeInteger(revision) && revision > 0)).toBe(true);
  expect(visibilityRevisions.every((revision, index) => index === 0 || revision > visibilityRevisions[index - 1])).toBe(true);
  expect(diagnostics.picks).toEqual(['element']);
  expect(diagnostics.popups).toContainEqual({
    sessionId: 'browser-session-1',
    sourceTabId: 'tab-1',
    url: 'https://example.com/popup',
    bounds: expect.any(Object),
  });
  expect(diagnostics.controls).toContain('none');
});

test('ignores popup requests while the owning Browser Workspace is hidden', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'newWindowRequested',
      payload: {
        sessionId: 'browser-session-1',
        tabId: 'tab-1',
        url: 'https://example.com/hidden-popup',
      },
    });
  });
  await page.waitForTimeout(100);

  const popupCount = await page.evaluate(() => (
    window as unknown as {
      __browserDiagnostics__: { popups: Array<Record<string, unknown>> };
    }
  ).__browserDiagnostics__.popups.length);
  expect(popupCount).toBe(0);
});

test('bounds popup admission for a visible Browser Workspace', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();

  await page.evaluate(() => {
    const emit = (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__;
    for (let index = 0; index < 30; index += 1) {
      emit({
        kind: 'newWindowRequested',
        payload: {
          sessionId: 'browser-session-1',
          tabId: 'tab-1',
          url: `https://example.com/popup-${index}`,
        },
      });
    }
  });

  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __browserDiagnostics__: { popups: Array<Record<string, unknown>> };
    }
  ).__browserDiagnostics__.popups.length)).toBe(15);
});

test('keeps the native surface visible across same-session snapshot refreshes', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __browserDiagnostics__: { bounds: Array<Record<string, unknown>> };
    }
  ).__browserDiagnostics__.bounds.some(entry => entry.visible === true))).toBe(true);
  const hiddenBefore = await page.evaluate(() => (
    window as unknown as {
      __browserDiagnostics__: { bounds: Array<Record<string, unknown>> };
    }
  ).__browserDiagnostics__.bounds.filter(entry => entry.visible === false).length);

  await page.evaluate(() => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'tabUpdated',
      payload: { sessionId: 'browser-session-1', tabId: 'tab-1' },
    });
  });
  await page.waitForTimeout(100);

  const hiddenAfter = await page.evaluate(() => (
    window as unknown as {
      __browserDiagnostics__: { bounds: Array<Record<string, unknown>> };
    }
  ).__browserDiagnostics__.bounds.filter(entry => entry.visible === false).length);
  expect(hiddenAfter).toBe(hiddenBefore);
});

test('drops a deferred page pick when the active conversation changes', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __deferNextBrowserPick__: () => void }).__deferNextBrowserPick__();
  });
  await page.getByRole('button', { name: 'Point out' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __deferredBrowserPickPending__: () => boolean }
  ).__deferredBrowserPickPending__())).toBe(true);

  await page.getByText('Shared Browser Workspace B', { exact: true }).click();
  await expect(page).toHaveURL(/\/chat\/conv-browser-workspace-b$/);
  const composer = page.getByTestId('chat-input-textarea');
  await expect(composer).toHaveValue('');

  await page.evaluate(() => {
    (window as unknown as { __resolveDeferredBrowserPick__: () => void }).__resolveDeferredBrowserPick__();
  });
  await page.waitForTimeout(400);
  await expect(composer).toHaveValue('');
});

test('drops deferred selected text when the active conversation changes', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTestId('browser-dock')).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as {
      __deferNextBrowserSelectedText__: () => void;
    }).__deferNextBrowserSelectedText__();
  });
  await page.getByRole('button', { name: 'Send text' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __deferredBrowserSelectedTextPending__: () => boolean;
    }
  ).__deferredBrowserSelectedTextPending__())).toBe(true);

  await page.getByText('Shared Browser Workspace B', { exact: true }).click();
  await expect(page).toHaveURL(/\/chat\/conv-browser-workspace-b$/);
  const composer = page.getByTestId('chat-input-textarea');
  await expect(composer).toHaveValue('');

  await page.evaluate(() => {
    (window as unknown as {
      __resolveDeferredBrowserSelectedText__: () => void;
    }).__resolveDeferredBrowserSelectedText__();
  });
  await page.waitForTimeout(100);
  await expect(composer).toHaveValue('');
});

test('starts the new conversation browser without waiting for an older conversation create', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.evaluate(() => {
    (window as unknown as { __deferNextBrowserCreate__: () => void }).__deferNextBrowserCreate__();
  });
  await page.getByTestId('browser-workspace-toggle').click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __deferredBrowserCreatePending__: () => boolean }
  ).__deferredBrowserCreatePending__())).toBe(true);

  await page.getByText('Shared Browser Workspace B', { exact: true }).click();
  await expect(page).toHaveURL(/\/chat\/conv-browser-workspace-b$/);
  await expect(page.getByRole('textbox', { name: 'Browser address or search' }))
    .toHaveValue('https://www.google.com');

  await page.evaluate(() => {
    (window as unknown as { __resolveDeferredBrowserCreate__: () => void }).__resolveDeferredBrowserCreate__();
  });
});

test('keeps a newer session snapshot when an older refresh fails', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace?deferBrowserActiveSession=1');
  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __deferredBrowserActiveSessionPending__: () => boolean;
    }
  ).__deferredBrowserActiveSessionPending__())).toBe(true);

  await page.getByTestId('browser-workspace-toggle').click();
  const address = page.getByRole('textbox', { name: 'Browser address or search' });
  await expect(address).toHaveValue('https://www.google.com');

  await page.evaluate(() => {
    (window as unknown as {
      __rejectDeferredBrowserActiveSession__: () => void;
    }).__rejectDeferredBrowserActiveSession__();
  });
  await page.waitForTimeout(100);
  await expect(address).toHaveValue('https://www.google.com');
});

test('does not replace the active conversation session with a stale control result', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByRole('textbox', { name: 'Browser address or search' }))
    .toHaveValue('https://www.google.com');

  await page.evaluate(() => {
    (window as unknown as { __deferNextBrowserControl__: () => void }).__deferNextBrowserControl__();
  });
  await page.getByRole('button', { name: 'Hand back' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __deferredBrowserControlPending__: () => boolean }
  ).__deferredBrowserControlPending__())).toBe(true);

  await page.getByText('Shared Browser Workspace B', { exact: true }).click();
  await expect(page).toHaveURL(/\/chat\/conv-browser-workspace-b$/);
  const address = page.getByRole('textbox', { name: 'Browser address or search' });
  await expect(address).toHaveValue('https://www.google.com');

  await page.evaluate(() => {
    (window as unknown as { __resolveDeferredBrowserControl__: () => void }).__resolveDeferredBrowserControl__();
  });
  await page.waitForTimeout(100);
  await expect(address).toHaveValue('https://www.google.com');
  await expect(page.getByText('You are controlling this page')).toBeVisible();
});

test('does not revive a closed session from a stale control result', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await page.getByTestId('browser-workspace-toggle').click();
  await expect(page.getByTitle('Google')).toHaveCount(1);

  await page.evaluate(() => {
    (window as unknown as { __deferNextBrowserControl__: () => void }).__deferNextBrowserControl__();
  });
  await page.getByRole('button', { name: 'Hand back' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __deferredBrowserControlPending__: () => boolean }
  ).__deferredBrowserControlPending__())).toBe(true);

  await page.getByRole('button', { name: 'Close tab' }).click();
  await expect(page.getByTitle('Google')).toHaveCount(0);
  await page.getByRole('button', { name: 'New tab' }).click();
  await expect(page.getByTitle('Google')).toHaveCount(2);

  await page.evaluate(() => {
    (window as unknown as { __resolveDeferredBrowserControl__: () => void }).__resolveDeferredBrowserControl__();
  });
  await page.waitForTimeout(100);
  await expect(page.getByTitle('Google')).toHaveCount(2);
});

test('docks the global Browser Workspace beside non-chat content', async ({ page }) => {
  await page.goto('/');

  const routedContent = page.getByTestId('app-workspace').locator(':scope > div').first();
  await expect(routedContent).toBeVisible();

  await page.evaluate(() => {
    window.dispatchEvent(new CustomEvent('nexa:open-browser-workspace', {
      detail: { url: 'https://openai.com/non-chat', title: 'Non-chat browser' },
      cancelable: true,
    }));
  });

  const dock = page.getByTestId('browser-dock');
  await expect(dock).toBeVisible();
  await expect(dock.getByRole('button', { name: 'Point out' })).toHaveCount(0);
  await expect(dock.getByRole('button', { name: 'Coordinate region' })).toHaveCount(0);
  await expect(dock.getByRole('button', { name: 'Send text' })).toHaveCount(0);
  await expect(routedContent).toBeInViewport();

  const bounds = await page.evaluate(() => {
    const content = document.querySelector<HTMLElement>('[data-testid="app-workspace"] > div')?.getBoundingClientRect();
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

test('reveals the shared Browser Workspace when the Agent creates or observes its session', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await expect(page.getByTestId('browser-workspace-toggle')).toBeVisible();
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'sessionCreated',
      payload: {
        sessionId: 'browser-session-1',
        conversationId: 'another-conversation',
        requestVisible: true,
      },
    });
  });
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'sessionCreated',
      payload: {
        sessionId: 'browser-session-1',
        conversationId: 'conv-browser-workspace',
        requestVisible: true,
      },
    });
  });

  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await expect(page.getByTestId('browser-native-surface')).toBeVisible();
  const diagnostics = await page.evaluate(() => (window as unknown as {
    __browserDiagnostics__: { bounds: Array<Record<string, unknown>> };
  }).__browserDiagnostics__);
  expect(diagnostics.bounds.some((entry) => entry.visible === true)).toBe(true);
});

test('recovers a missed visibility event from the owning session snapshot', async ({ page }) => {
  await page.goto('/chat/conv-browser-workspace');
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);

  const sessionId = await page.evaluate(() => (
    window as unknown as {
      __seedBrowserVisibilityRequest__: (conversationId: string, minimumRevision: number) => string;
    }
  ).__seedBrowserVisibilityRequest__('conv-browser-workspace', 42));
  await page.evaluate((currentSessionId) => {
    (window as unknown as {
      __emitBrowserEvent__: (payload: Record<string, unknown>) => void;
    }).__emitBrowserEvent__({
      kind: 'sessionCreated',
      payload: {
        sessionId: currentSessionId,
        conversationId: 'conv-browser-workspace',
        requestVisible: false,
      },
    });
  }, sessionId);

  await expect(page.getByTestId('browser-dock')).toBeVisible();
  await expect.poll(async () => page.evaluate((currentSessionId) => {
    const diagnostics = (window as unknown as {
      __browserDiagnostics__: { bounds: Array<Record<string, unknown>> };
    }).__browserDiagnostics__;
    return diagnostics.bounds.some(entry => (
      entry.sessionId === currentSessionId
      && entry.visible === true
      && Number(entry.visibilityRevision) > 42
    ));
  }, sessionId)).toBe(true);
});
