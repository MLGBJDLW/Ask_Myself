import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const activeConversation = {
      id: 'conv-active',
      title: 'Active conversation',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      personaId: null,
      titleIsAuto: false,
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
        case 'list_projects_cmd':
        case 'list_personas_cmd':
        case 'list_checkpoints_cmd':
          return [];
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

  await banner.getByRole('button', { name: 'Restore' }).click();
  await expect(banner).toBeHidden();
  await expect(page.getByTestId('conversation-item-conv-archived')).toBeVisible();
  await expect(page.getByPlaceholder('Type a message...')).toBeVisible();
});
