import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('active-project-id', 'project-legacy');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const activeConversation = {
      id: 'conv-active',
      title: 'Active conversation',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: 'Legacy copied project prompt',
      collectionContext: null,
      projectId: 'project-legacy',
      personaId: 'programmer',
      initialAutoTitlePending: false,
      archivedAt: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const archivedConversation = {
      ...activeConversation,
      id: 'conv-archived',
      title: 'Archived conversation',
      archivedAt: nowIso,
    };
    const archivedMessages = [
      {
        id: 'msg-archived-user',
        conversationId: 'conv-archived',
        role: 'user',
        content: 'Keep this archived question available.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 8,
        createdAt: nowIso,
        sortOrder: 0,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'msg-archived-assistant',
        conversationId: 'conv-archived',
        role: 'assistant',
        content: 'This archived answer remains readable.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 8,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
    ];
    let active = [activeConversation];
    let archived = [archivedConversation];
    const commands: string[] = [];
    const createConversationArgs: Array<Record<string, unknown>> = [];

    const defaultAgentConfig = {
      id: 'cfg-archive',
      name: 'Archive Config',
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

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      commands.push(cmd);
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
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_conversations_cmd':
          return clone(active);
        case 'create_conversation_cmd': {
          createConversationArgs.push(clone(args));
          if (localStorage.getItem('e2e-fail-next-conversation-create') === '1') {
            localStorage.removeItem('e2e-fail-next-conversation-create');
            throw new Error('Injected conversation create failure');
          }
          if (localStorage.getItem('e2e-delay-conversation-create') === '1') {
            await new Promise((resolve) => setTimeout(resolve, 300));
          }
          const conversation = {
            ...activeConversation,
            id: 'conv-new',
            title: '',
            systemPrompt: String(args.systemPrompt ?? ''),
            projectId: args.projectId == null ? null : String(args.projectId),
            personaId: args.personaId == null ? null : String(args.personaId),
          };
          active = [conversation, ...active];
          return clone(conversation);
        }
        case 'list_archived_conversations_cmd':
          return clone(archived);
        case 'archive_conversation_cmd': {
          const id = String(args.id ?? '');
          const conversation = active.find((item) => item.id === id);
          if (!conversation) return null;
          active = active.filter((item) => item.id !== id);
          const next = { ...conversation, archivedAt: new Date().toISOString() };
          archived = [next, ...archived];
          return clone(next);
        }
        case 'unarchive_conversation_cmd': {
          const id = String(args.id ?? '');
          const conversation = archived.find((item) => item.id === id);
          if (!conversation) return null;
          archived = archived.filter((item) => item.id !== id);
          const next = { ...conversation, archivedAt: null };
          active = [next, ...active];
          return clone(next);
        }
        case 'delete_conversation_cmd': {
          const id = String(args.id ?? '');
          active = active.filter((item) => item.id !== id);
          archived = archived.filter((item) => item.id !== id);
          return null;
        }
        case 'agent_chat_cmd':
          if (localStorage.getItem('e2e-fail-next-agent-launch') === '1') {
            localStorage.removeItem('e2e-fail-next-agent-launch');
            throw new Error('Injected agent launch failure');
          }
          if (localStorage.getItem('e2e-delay-next-agent-launch') === '1') {
            localStorage.removeItem('e2e-delay-next-agent-launch');
            await new Promise((resolve) => setTimeout(resolve, 300));
          }
          return null;
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          const conversation = [...active, ...archived].find((item) => item.id === id);
          return [
            clone(conversation),
            id === 'conv-archived' ? clone(archivedMessages) : [],
          ];
        }
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_checkpoints_cmd':
          return [];
        case 'list_personas_cmd':
          return [
            {
              id: 'default',
              name: 'Default',
              description: 'Balanced assistant',
              instructions: '',
              enabled: true,
              builtin: true,
              defaultSkillIds: [],
              createdAt: nowIso,
              updatedAt: nowIso,
            },
            {
              id: 'programmer',
              name: 'Programmer',
              description: 'Software engineering assistant',
              instructions: 'Act as a programmer.',
              enabled: true,
              builtin: true,
              defaultSkillIds: [],
              createdAt: nowIso,
              updatedAt: nowIso,
            },
          ];
        case 'list_projects_cmd':
          return [{
            id: 'project-legacy',
            name: 'Legacy project',
            description: '',
            icon: 'folder',
            color: '#3b82f6',
            systemPrompt: 'Current live project prompt',
            sourceScope: null,
            archived: false,
            createdAt: nowIso,
            updatedAt: nowIso,
          }];
        case 'get_index_stats':
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case 'get_privacy_config':
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case 'get_embedder_config_cmd':
          return {
            provider: 'tfidf',
            apiKey: '',
            apiBaseUrl: '',
            apiModel: '',
            localModel: '',
            modelPath: '',
            vectorDimensions: 384,
          };
        case 'get_ocr_config_cmd':
          return {
            enabled: false,
            minConfidence: 0.5,
            llmFallback: false,
            detectionLimit: 2048,
            useCls: false,
          };
        case 'check_ocr_models_cmd':
          return false;
        default:
          return null;
      }
    };

    (window as unknown as { __ARCHIVE_COMMANDS__: string[] }).__ARCHIVE_COMMANDS__ = commands;
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__ = createConversationArgs;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      metadata: { currentWindow: { label: 'main' } },
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

test('conversation actions offer archive and delete with reversible archive', async ({ page }) => {
  await page.goto('/chat/conv-active');

  await page.getByTestId('conversation-item-conv-active').hover();
  await page.getByTestId('conversation-actions-trigger-conv-active').click();
  const actions = page.getByTestId('conversation-actions-conv-active');
  await expect(actions.getByRole('button', { name: 'Archive' })).toBeVisible();
  await expect(actions.getByRole('button', { name: 'Delete' })).toBeVisible();

  await actions.getByRole('button', { name: 'Archive' }).click();
  await expect(page.getByTestId('conversation-item-conv-active')).toBeHidden();
  await page.getByRole('button', { name: 'Undo' }).click();
  await expect(page.getByTestId('conversation-item-conv-active')).toBeVisible();
});

test('archive feedback remains an overlay and never participates in the app layout', async ({ page }) => {
  await page.goto('/chat/conv-active');

  const appMain = page.locator('main');
  const before = await appMain.boundingBox();
  if (!before) throw new Error('app main layout is not measurable');

  const removedSonnerStyles = await page.evaluate(() => {
    const styles = Array.from(document.querySelectorAll('style'));
    const sonnerStyles = styles.filter((style) => style.textContent?.includes('[data-sonner-toaster]'));
    sonnerStyles.forEach((style) => style.remove());
    return sonnerStyles.length;
  });
  expect(removedSonnerStyles).toBeGreaterThan(0);

  await page.getByTestId('conversation-item-conv-active').hover();
  await page.getByTestId('conversation-actions-trigger-conv-active').click();
  await page.getByTestId('conversation-actions-conv-active').getByRole('button', { name: 'Archive' }).click();
  await expect(page.getByRole('button', { name: 'Undo' })).toBeVisible();

  const notificationLayout = await page.locator('[data-sonner-toaster]').evaluate((toaster) => {
    const rect = toaster.getBoundingClientRect();
    const style = getComputedStyle(toaster);
    return {
      position: style.position,
      right: window.innerWidth - rect.right,
      bottom: window.innerHeight - rect.bottom,
      portaledToBody: toaster.closest('section')?.parentElement === document.body,
    };
  });
  const after = await appMain.boundingBox();
  if (!after) throw new Error('app main layout disappeared after archive feedback');

  expect(notificationLayout).toMatchObject({ position: 'fixed', portaledToBody: true });
  expect(notificationLayout.right).toBeGreaterThanOrEqual(0);
  expect(notificationLayout.bottom).toBeGreaterThanOrEqual(0);
  expect(after).toEqual(before);
});

test('new chat stays an unpersisted draft until the first send', async ({ page }) => {
  await page.goto('/chat/conv-active');
  await page.getByTestId('chat-history-sidebar').getByRole('button', { name: 'New Chat' }).click();
  await expect(page).toHaveURL(/\/chat$/);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(0);
  await expect(page.getByTestId('conversation-item-conv-new')).toHaveCount(0);

  await page.reload();
  await expect(page.getByTestId('conversation-item-conv-new')).toHaveCount(0);
  expect(await page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(0);
});

test('new chat resets the previous conversation persona before first persistence', async ({ page }) => {
  await page.goto('/chat/conv-active');
  await expect(page.getByRole('button', { name: 'Personas' })).toHaveAttribute('title', /Programmer/);

  await page.getByTestId('chat-history-sidebar').getByRole('button', { name: 'New Chat' }).click();
  await expect(page).toHaveURL(/\/chat$/);
  await expect(page.getByRole('button', { name: 'Personas' })).toHaveAttribute('title', /Default/);
  await page.getByPlaceholder('Type a message...').fill('Hello from a fresh draft.');
  await page.getByPlaceholder('Type a message...').press('Enter');

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__[0],
  )).toMatchObject({ personaId: 'default' });
});

test('failed first persistence keeps the local draft available for retry', async ({ page }) => {
  await page.goto('/chat/conv-active');
  await page.getByTestId('chat-history-sidebar').getByRole('button', { name: 'New Chat' }).click();
  await page.evaluate(() => localStorage.setItem('e2e-fail-next-conversation-create', '1'));
  const input = page.getByPlaceholder('Type a message...');
  await input.fill('Keep this draft when persistence fails.');

  await input.press('Enter');

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(1);
  await expect(input).toHaveValue('Keep this draft when persistence fails.');
  await expect(page).toHaveURL(/\/chat$/);
  await expect(page.getByTestId('conversation-item-conv-new')).toHaveCount(0);
});

test('rejected first turn rolls back its empty conversation and keeps the draft', async ({ page }) => {
  await page.goto('/chat/conv-active');
  await page.getByTestId('chat-history-sidebar').getByRole('button', { name: 'New Chat' }).click();
  await page.evaluate(() => localStorage.setItem('e2e-fail-next-agent-launch', '1'));
  const input = page.getByPlaceholder('Type a message...');
  await input.fill('Keep this draft when the agent launch is rejected.');

  await input.press('Enter');

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(1);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __ARCHIVE_COMMANDS__: string[] })
      .__ARCHIVE_COMMANDS__.filter((command) => command === 'delete_conversation_cmd').length,
  )).toBe(1);
  await expect(input).toHaveValue('Keep this draft when the agent launch is rejected.');
  await expect(page).toHaveURL(/\/chat$/);
  await expect(page.getByTestId('conversation-item-conv-new')).toHaveCount(0);
});

test('a deferred first send never redirects after the user selects another conversation', async ({ page }) => {
  await page.goto('/chat/conv-active');
  const sidebar = page.getByTestId('chat-history-sidebar');
  await sidebar.getByRole('button', { name: 'New Chat' }).click();
  await page.evaluate(() => localStorage.setItem('e2e-delay-next-agent-launch', '1'));
  const input = page.getByPlaceholder('Type a message...');
  await input.fill('Start this conversation without stealing later navigation.');

  await input.press('Enter');
  await sidebar.getByTestId('conversation-item-conv-active').click();
  await expect(page).toHaveURL(/\/chat\/conv-active$/);

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(1);
  await page.waitForTimeout(400);
  await expect(page).toHaveURL(/\/chat\/conv-active$/);
  await expect(sidebar.getByTestId('conversation-item-conv-new')).toHaveCount(1);
});

test('first persistence is single-flight when Enter is pressed repeatedly', async ({ page }) => {
  await page.goto('/chat/conv-active');
  await page.getByTestId('chat-history-sidebar').getByRole('button', { name: 'New Chat' }).click();
  await page.evaluate(() => localStorage.setItem('e2e-delay-conversation-create', '1'));
  const input = page.getByPlaceholder('Type a message...');
  await input.fill('Create exactly one conversation.');

  await input.press('Enter');
  await input.press('Enter');
  await page.waitForTimeout(500);

  expect(await page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__.length,
  )).toBe(1);
});

test('archived conversations can be restored from the sidebar manager', async ({ page }) => {
  await page.goto('/chat/conv-active');

  await expect(page.getByTestId('chat-archive-nav')).toBeVisible();
  await page.getByTestId('chat-archive-nav').click();

  const archivedItem = page.getByTestId('archived-conversation-conv-archived');
  await expect(archivedItem).toContainText('Archived conversation');
  await expect(archivedItem.getByRole('button', { name: 'Delete' })).toBeVisible();
  await archivedItem.click();
  await expect(page.getByTestId('archived-conversation-banner')).toContainText('read-only');
  await expect(page.getByText('Keep this archived question available.')).toBeVisible();
  await expect(page.getByText('This archived answer remains readable.')).toBeVisible();
  await expect(page.getByPlaceholder('Type a message...')).toBeHidden();

  await archivedItem.getByRole('button', { name: 'Unarchive' }).click();
  await expect(archivedItem).toBeHidden();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __ARCHIVE_COMMANDS__: string[] }).__ARCHIVE_COMMANDS__,
  )).toContain('unarchive_conversation_cmd');

  await expect(page.getByText('Archived conversation', { exact: true })).toBeVisible();
});

test('a direct archived conversation link opens read-only without joining the active list', async ({ page }) => {
  await page.goto('/chat/conv-archived');

  const banner = page.getByTestId('archived-conversation-banner');
  await expect(banner).toBeVisible();
  await expect(page.getByTestId('archived-conversation-conv-archived')).toHaveAttribute(
    'aria-current',
    'page',
  );
  await expect(page.getByTestId('conversation-item-conv-archived')).toHaveCount(0);
  await expect(page.getByPlaceholder('Type a message...')).toHaveCount(0);

  await page.keyboard.press('Control+Shift+B');
  const dock = page.getByTestId('browser-dock');
  await expect(dock).toBeVisible();
  await expect(dock.getByRole('button', { name: 'Point out' })).toHaveCount(0);
  await expect(dock.getByRole('button', { name: 'Coordinate region' })).toHaveCount(0);
  await expect(dock.getByRole('button', { name: 'Send text' })).toHaveCount(0);

  await banner.getByRole('button', { name: 'Restore' }).click();
  await expect(banner).toBeHidden();
  await expect(page.getByTestId('conversation-item-conv-archived')).toBeVisible();
  await expect(page.getByPlaceholder('Type a message...')).toBeVisible();
  await expect(dock.getByRole('button', { name: 'Point out' })).toBeVisible();
});

test('responsive sidebar collapse is temporary and restores the user preference', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 720 });
  await page.goto('/chat/conv-active');

  const sidebar = page.getByTestId('chat-history-sidebar');
  await expect(sidebar).toHaveAttribute('data-collapsed', 'false');

  await page.setViewportSize({ width: 700, height: 720 });
  await expect(sidebar).toHaveAttribute('data-collapsed', 'true');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('chat-sidebar-collapsed')))
    .toBeNull();

  await page.setViewportSize({ width: 1000, height: 720 });
  await expect(sidebar).toHaveAttribute('data-collapsed', 'false');
});

test('typing shortcuts do not toggle the conversation sidebar', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 720 });
  await page.goto('/chat/conv-active');

  const sidebar = page.getByTestId('chat-history-sidebar');
  const composer = page.getByPlaceholder('Type a message...');
  await composer.focus();
  await composer.press('Control+b');
  await expect(sidebar).toHaveAttribute('data-collapsed', 'false');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('chat-sidebar-collapsed')))
    .toBeNull();
});
