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

    const conversations: Record<string, Conversation> = {
      'conv-persisted-order': {
        id: 'conv-persisted-order',
        title: 'Persisted Order',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const toolCallId = 'tool-order-1';

    const messagesByConversation: Record<string, Message[]> = {
      'conv-persisted-order': [
        {
          id: 'm-user-1',
          conversationId: 'conv-persisted-order',
          role: 'user',
          content: 'Walk through the persisted order.',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-assistant-tools-1',
          conversationId: 'conv-persisted-order',
          role: 'assistant',
          content: '',
          toolCallId: null,
          toolCalls: [{ id: toolCallId, name: 'search_knowledge_base', arguments: '{"query":"order"}' }],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 1,
          thinking: 'phase one thinking',
          imageAttachments: null,
        },
        {
          id: 'm-tool-1',
          conversationId: 'conv-persisted-order',
          role: 'tool',
          content: 'Found the order note.',
          toolCallId,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 2,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-assistant-final',
          conversationId: 'conv-persisted-order',
          role: 'assistant',
          content: 'final-reply-segment',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 3,
          thinking: 'phase one thinking\nphase two thinking',
          imageAttachments: null,
        },
      ],
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const defaultAgentConfig = {
      id: 'cfg-persisted-order',
      name: 'Persisted Order Config',
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
        case 'plugin:event|listen':
          return listenerSeq++;
        case 'plugin:event|unlisten':
          return null;
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [clone(conversations[id]), clone(messagesByConversation[id] ?? [])];
        }
        case 'list_sources':
          return [];
        case 'get_conversation_sources_cmd':
          return [];
        case 'set_conversation_sources_cmd':
          return null;
        case 'update_conversation_system_prompt_cmd':
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
        case 'list_user_memories_cmd':
          return [];
        case 'list_skills_cmd':
          return [];
        case 'list_mcp_servers_cmd':
          return [];
        case 'clear_answer_cache':
          return 0;
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
      unregisterListener: () => {},
    };
  });
});

test('renders persisted multi-step traces in chronological order', async ({ page }) => {
  await page.goto('/chat/conv-persisted-order');

  const chatLogText = await page.getByLabel('Chat messages').textContent();
  expect(chatLogText).toBeTruthy();

  const text = chatLogText ?? '';
  expect(text.indexOf('phase one thinking')).toBeGreaterThanOrEqual(0);
  expect(text.indexOf('search_knowledge_base')).toBeGreaterThan(text.indexOf('phase one thinking'));
  expect(text.indexOf('phase two thinking')).toBeGreaterThan(text.indexOf('search_knowledge_base'));
  expect(text.indexOf('final-reply-segment')).toBeGreaterThan(text.indexOf('phase two thinking'));

  await expect(
    page.locator('button[aria-expanded="true"]').filter({ hasText: 'Thinking completed' }),
  ).toHaveCount(1);
  await expect(page.getByText('Conclusion')).toHaveCount(1);
});
