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
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();

    const conversation: Conversation = {
      id: 'conv-turn-trace',
      title: 'Turn Trace',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const messages: Message[] = [
      {
        id: 'm-user-turn',
        conversationId: 'conv-turn-trace',
        role: 'user',
        content: 'Why did the retry guard fail?',
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
        id: 'm-assistant-turn',
        conversationId: 'conv-turn-trace',
        role: 'assistant',
        content: 'The retry guard was bypassed because the timeout branch did not return early.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
    ];

    const turns = [
      {
        id: 'turn-1',
        conversationId: 'conv-turn-trace',
        userMessageId: 'm-user-turn',
        assistantMessageId: 'm-assistant-turn',
        status: 'success',
        routeKind: 'KnowledgeRetrieval',
        trace: {
          kind: 'turnTrace',
          routeKind: 'KnowledgeRetrieval',
          items: [
            { kind: 'thinking', text: 'Checking the retry path through the saved evidence first.' },
            {
              kind: 'tool',
              toolCall: {
                callId: 'tool-turn-1',
                toolName: 'search_knowledge_base',
                arguments: '{"query":"retry guard"}',
                status: 'done',
                content: 'Found 2 retry notes.',
                isError: false,
                artifacts: null,
              },
            },
          ],
        },
        createdAt: nowIso,
        updatedAt: nowIso,
        finishedAt: nowIso,
      },
    ];

    const defaultAgentConfig = {
      id: 'cfg-turn-trace',
      name: 'Turn Trace Config',
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
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), clone(messages)];
        case 'get_conversation_turns_cmd':
          return clone(turns);
        case 'list_sources':
          return [];
        case 'get_conversation_sources_cmd':
          return [];
        case 'set_conversation_sources_cmd':
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

test('renders persisted turn traces from conversation_turns data', async ({ page }) => {
  await page.goto('/chat/conv-turn-trace');

  await expect(page.getByText('Route: Knowledge Retrieval')).toBeVisible();
  await expect(page.getByText('Status: Success')).toBeVisible();
  await expect(page.getByText('Checking the retry path through the saved evidence first.')).toBeVisible();
  await expect(page.getByText('search_knowledge_base')).toBeVisible();
  await expect(page.getByText('The retry guard was bypassed because the timeout branch did not return early.')).toBeVisible();
});
