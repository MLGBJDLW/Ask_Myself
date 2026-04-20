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
      collectionContext?: {
        title: string;
        description?: string | null;
        queryText?: string | null;
        sourceIds: string[];
      } | null;
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

    const playbook = {
      id: 'pb-context',
      title: 'Retry Collection',
      description: 'Collection about retry handling.',
      queryText: 'retry timeout guard',
      citations: [
        {
          id: 'cit-context',
          playbookId: 'pb-context',
          chunkId: 'chunk-context-1',
          annotation: 'Timeout guard is critical',
          order: 0,
        },
      ],
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const conversations: Record<string, Conversation> = {};
    const messagesByConversation: Record<string, Message[]> = {};
    const conversationSources: Record<string, string[]> = {};

    const defaultAgentConfig = {
      id: 'cfg-playbook-context',
      name: 'Playbook Context Config',
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
          const id = 'conv-playbook-context';
          const conversation: Conversation = {
            id,
            title: '',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
            collectionContext: (args.collectionContext as Conversation['collectionContext']) ?? null,
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
        case 'list_sources':
          return [];
        case 'get_conversation_sources_cmd':
          return conversationSources[String(args.conversationId ?? '')] ?? [];
        case 'set_conversation_sources_cmd':
          conversationSources[String(args.conversationId ?? '')] = Array.isArray(args.sourceIds)
            ? (args.sourceIds as unknown[]).filter((value): value is string => typeof value === 'string')
            : [];
          return null;
        case 'update_conversation_collection_context_cmd': {
          const id = String(args.id ?? '');
          conversations[id].collectionContext = (args.collectionContext as Conversation['collectionContext']) ?? null;
          return null;
        }
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
        case 'list_playbooks':
          return [clone(playbook)];
        case 'get_playbook':
          return clone(playbook);
        case 'get_evidence_card':
          return {
            chunkId: 'chunk-context-1',
            documentId: 'doc-context-1',
            sourceId: 'source-retries',
            sourceName: 'Knowledge Base',
            documentPath: 'D:/notes/retries.md',
            documentTitle: 'Retries Guide',
            content: 'Keep the timeout guard and surface retry limits.',
            headingPath: ['Retries'],
            score: 0.95,
            highlights: [],
            snippet: 'Keep the timeout guard and surface retry limits.',
          };
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const current = messagesByConversation[conversationId] ?? [];
          const hasCollectionContext = conversations[conversationId]?.collectionContext?.title === 'Retry Collection';
          const hasSourceScope = (conversationSources[conversationId] ?? []).includes('source-retries');
          const assistantContent = hasCollectionContext
            ? (hasSourceScope ? 'collection-context-on' : 'collection-scope-missing')
            : 'collection-context-off';

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
        case 'open_file_in_default_app':
        case 'show_in_file_explorer':
          return null;
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

test('starts chat from a collection with collection-aware system prompt context', async ({ page }) => {
  await page.goto('/playbooks');

  await page.getByRole('button', { name: /Retry Collection/ }).click();
  await page.locator('button').filter({ hasText: 'Ask AI' }).click();

  await expect(page.getByText('collection-context-on')).toBeVisible();
});
