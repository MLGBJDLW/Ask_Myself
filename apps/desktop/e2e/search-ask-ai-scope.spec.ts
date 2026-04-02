import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('ask-myself-locale', 'en');

    type Conversation = {
      id: string;
      title: string;
      provider: string;
      model: string;
      systemPrompt: string;
      collectionContext?: null;
      createdAt: string;
      updatedAt: string;
    };

    type Source = {
      id: string;
      kind: string;
      rootPath: string;
      includeGlobs: string[];
      excludeGlobs: string[];
      watchEnabled: boolean;
      createdAt: string;
      updatedAt: string;
    };

    type Message = {
      id: string;
      conversationId: string;
      role: 'system' | 'user' | 'assistant' | 'tool';
      content: string;
      toolCallId: string | null;
      toolCalls: Array<{ id: string; name: string; arguments: string }>;
      artifacts: Record<string, unknown> | null;
      tokenCount: number;
      createdAt: string;
      sortOrder: number;
      thinking: string | null;
      imageAttachments: null;
    };

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();

    const emitEvent = (eventName: string, payload: Record<string, unknown>) => {
      for (const [listenerId, listener] of listeners.entries()) {
        if (listener.event !== eventName) continue;
        const callback = callbackMap.get(listener.handlerId);
        if (callback) {
          callback({ event: eventName, id: listenerId, payload });
        }
      }
    };

    const sources: Source[] = [
      {
        id: 'source-retries',
        kind: 'local_folder',
        rootPath: 'D:/notes/retries',
        includeGlobs: ['**/*.md'],
        excludeGlobs: [],
        watchEnabled: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      {
        id: 'source-random',
        kind: 'local_folder',
        rootPath: 'D:/notes/random',
        includeGlobs: ['**/*.md'],
        excludeGlobs: [],
        watchEnabled: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];

    const conversations: Record<string, Conversation> = {};
    const messagesByConversation: Record<string, Message[]> = {};
    const conversationSources: Record<string, string[]> = {};

    const defaultAgentConfig = {
      id: 'cfg-search-scope',
      name: 'Search Scope Config',
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
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'create_conversation_cmd': {
          const id = 'conv-search-scope';
          const conversation: Conversation = {
            id,
            title: '',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
            collectionContext: null,
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          };
          conversations[id] = conversation;
          messagesByConversation[id] = [];
          return clone(conversation);
        }
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [clone(conversations[id]), clone(messagesByConversation[id] ?? [])];
        }
        case 'get_conversation_turns_cmd':
          return [];
        case 'list_sources':
          return clone(sources);
        case 'get_conversation_sources_cmd':
          return conversationSources[String(args.conversationId ?? '')] ?? [];
        case 'set_conversation_sources_cmd':
          conversationSources[String(args.conversationId ?? '')] = Array.isArray(args.sourceIds)
            ? (args.sourceIds as unknown[]).filter((value): value is string => typeof value === 'string')
            : [];
          return null;
        case 'update_conversation_system_prompt_cmd':
          return null;
        case 'update_conversation_collection_context_cmd':
          return null;
        case 'list_checkpoints_cmd':
          return [];
        case 'compact_conversation_cmd':
          return null;
        case 'agent_stop_cmd':
          return null;
        case 'save_agent_config_cmd':
          return clone(defaultAgentConfig);
        case 'get_index_stats':
          return { totalDocuments: 2, totalChunks: 8, ftsRows: 8 };
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
        case 'list_user_memories_cmd':
          return [];
        case 'list_skills_cmd':
          return [];
        case 'list_mcp_servers_cmd':
          return [];
        case 'clear_answer_cache':
          return 0;
        case 'get_recent_queries':
          return [];
        case 'search':
        case 'hybrid_search':
          return {
            hits: [],
            total: 0,
            searchMode: 'fts',
          };
        case 'search_conversations_cmd':
          return [];
        case 'list_playbooks':
          return [];
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const current = messagesByConversation[conversationId] ?? [];
          const hasRetryScope = (conversationSources[conversationId] ?? []).includes('source-retries');
          const assistantContent = hasRetryScope ? 'scope-on' : 'scope-off';

          const userMessage: Message = {
            id: nextId('m-user'),
            conversationId,
            role: 'user',
            content: String(args.message ?? ''),
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: current.length,
            thinking: null,
            imageAttachments: null,
          };
          const assistantMessage: Message = {
            id: nextId('m-assistant'),
            conversationId,
            role: 'assistant',
            content: assistantContent,
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: current.length + 1,
            thinking: null,
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = [...current, userMessage, assistantMessage];

          queueMicrotask(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'textDelta',
              delta: assistantContent,
            });
          });

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: assistantMessage,
              usageTotal: {
                promptTokens: 120,
                completionTokens: 24,
                totalTokens: 144,
                thinkingTokens: 0,
              },
              lastPromptTokens: 120,
              finishReason: 'stop',
              cached: false,
            });
          }, 40);

          return null;
        }
        default:
          return null;
      }
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbackMap.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => {
        callbackMap.delete(id);
      },
      convertFileSrc: (filePath: string) => filePath,
    };

    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('carries active search source filters into chat scope when asking AI', async ({ page }) => {
  await page.goto('/');

  await page.locator('button').filter({ hasText: 'Filters' }).click();
  await page.getByRole('button', { name: 'retries' }).click();
  await page.getByPlaceholder('Search by keyword...').fill('Why did the retry guard fail?');
  await page.locator('button').filter({ hasText: 'Ask AI' }).click();

  await expect(page.getByText('scope-on')).toBeVisible();
});

test('recall mode can hand vague clues into chat with the active source scope', async ({ page }) => {
  await page.goto('/');

  await page.locator('button').filter({ hasText: 'Filters' }).click();
  await page.getByRole('button', { name: 'retries' }).click();
  await page.getByText('Recall with vague clues').waitFor();
  await page.getByLabel('What do you remember?').fill('Something about retry guards and timeout limits.');
  await page.locator('button').filter({ hasText: 'Recall with AI' }).click();

  await expect(page.getByText('scope-on')).toBeVisible();
});
