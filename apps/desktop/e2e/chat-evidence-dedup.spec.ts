import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

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

    const conversation: Conversation = {
      id: 'conv-evidence-dedup',
      title: 'Evidence Dedup',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const messages: Message[] = [
      {
        id: 'm-user',
        conversationId: 'conv-evidence-dedup',
        role: 'user',
        content: 'What should we change?',
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
        id: 'm-assistant-tools',
        conversationId: 'conv-evidence-dedup',
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{ id: 'tool-1', name: 'search_knowledge_base', arguments: '{"query":"retry"}' }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: 'Searching retry notes',
        imageAttachments: null,
      },
      {
        id: 'm-tool-1',
        conversationId: 'conv-evidence-dedup',
        role: 'tool',
        content: 'Chunk A',
        toolCallId: 'tool-1',
        toolCalls: [],
        artifacts: {
          result1: {
            chunkId: 'chunk-a',
            documentPath: 'D:/notes/retries.md',
            documentTitle: 'Retries Guide',
            sourceName: 'Knowledge Base',
            content: 'Keep the timeout guard.',
            score: 0.95,
            headingPath: ['Retries'],
          },
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-2',
        conversationId: 'conv-evidence-dedup',
        role: 'tool',
        content: 'Chunk B',
        toolCallId: 'tool-1',
        toolCalls: [],
        artifacts: {
          result2: {
            chunkId: 'chunk-b',
            documentPath: 'D:/notes/retries.md',
            documentTitle: 'Retries Guide',
            sourceName: 'Knowledge Base',
            content: 'Show retry limits in the UI.',
            score: 0.93,
            headingPath: ['Retries'],
          },
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 3,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-final',
        conversationId: 'conv-evidence-dedup',
        role: 'assistant',
        content: 'Update the UI and keep the timeout guard [cite:chunk-a] [cite:chunk-b].',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 4,
        thinking: null,
        imageAttachments: null,
      },
    ];

    const defaultAgentConfig = {
      id: 'cfg-evidence-dedup',
      name: 'Evidence Dedup Config',
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

    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();

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
          return [];
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

test('groups multiple cited chunks from the same file into one evidence source chip', async ({ page }) => {
  await page.goto('/chat/conv-evidence-dedup');

  await expect(page.getByText('Retries Guide ×2')).toBeVisible();
  await expect(page.getByText('1 cited sources').first()).toBeVisible();
});
