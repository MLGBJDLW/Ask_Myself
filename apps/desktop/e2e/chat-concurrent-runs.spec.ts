import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    const nowIso = new Date().toISOString();
    const conversations = ['alpha', 'beta'].map((suffix) => ({
      id: `conv-${suffix}`,
      title: `${suffix[0].toUpperCase()}${suffix.slice(1)} chat`,
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      personaId: null,
      initialAutoTitlePending: false,
      archivedAt: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    }));
    const messages: Record<string, unknown[]> = {
      'conv-alpha': [],
      'conv-beta': [],
    };
    const stoppedConversationIds: string[] = [];
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const defaultAgentConfig = {
      id: 'cfg-concurrent',
      name: 'Concurrent Config',
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

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      if (cmd === 'agent_chat_cmd') args = (args.request as Record<string, unknown>) ?? {};
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
          return clone(conversations);
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [clone(conversations.find((item) => item.id === id)), clone(messages[id] ?? [])];
        }
        case 'agent_chat_cmd':
          return null;
        case 'agent_stop_cmd':
          stoppedConversationIds.push(String(args.conversationId ?? ''));
          return null;
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

    (window as unknown as { __STOPPED_CONVERSATIONS__: string[] }).__STOPPED_CONVERSATIONS__ = stoppedConversationIds;
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

test('two chats keep running across navigation and stop only the active run', async ({ page }) => {
  await page.goto('/chat/conv-alpha');
  await page.getByTestId('chat-input-textarea').fill('Run alpha in the background');
  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('conversation-running-conv-alpha')).toBeVisible();

  await page.getByTestId('conversation-item-conv-beta').click();
  await expect(page).toHaveURL(/\/chat\/conv-beta$/);
  await page.getByTestId('chat-input-textarea').fill('Run beta at the same time');
  await page.getByTestId('chat-send').click();

  await expect(page.getByTestId('conversation-running-conv-alpha')).toBeVisible();
  await expect(page.getByTestId('conversation-running-conv-beta')).toBeVisible();
  await expect(page.getByTestId('chat-stop')).toBeVisible();

  await page.getByTestId('conversation-item-conv-alpha').click();
  await expect(page).toHaveURL(/\/chat\/conv-alpha$/);
  await expect(page.getByTestId('chat-stop')).toBeVisible();
  await page.getByTestId('chat-stop').click();

  await expect(page.getByTestId('conversation-running-conv-alpha')).toHaveCount(0);
  await expect(page.getByTestId('conversation-running-conv-beta')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __STOPPED_CONVERSATIONS__: string[] }
  ).__STOPPED_CONVERSATIONS__)).toEqual(['conv-alpha']);
});
