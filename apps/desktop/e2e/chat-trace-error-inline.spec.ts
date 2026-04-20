import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: number }).__ASK_STREAM_TIMEOUT_MS__ = 120;
    history.replaceState(
      { usr: { initialMessage: 'Why did the connection fail?' }, key: 'e2e-inline-error', idx: 0 },
      '',
      '/chat',
    );

    type Conversation = {
      id: string;
      title: string;
      provider: string;
      model: string;
      systemPrompt: string;
      createdAt: string;
      updatedAt: string;
    };

    const nowIso = new Date().toISOString();
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

    const defaultAgentConfig = {
      id: 'cfg-inline-error',
      name: 'Inline Error Config',
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

    const conversations: Record<string, Conversation> = {};

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
          return [JSON.parse(JSON.stringify(defaultAgentConfig))];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(item => JSON.parse(JSON.stringify(item)));
        case 'create_conversation_cmd': {
          const id = 'conv-inline-error';
          const conversation: Conversation = {
            id,
            title: 'Inline Error',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          };
          conversations[id] = conversation;
          return JSON.parse(JSON.stringify(conversation));
        }
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [JSON.parse(JSON.stringify(conversations[id])), []];
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
          return JSON.parse(JSON.stringify(defaultAgentConfig));
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
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          queueMicrotask(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Investigating the failing connection path.',
            });
          });
          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallStart',
              callId: 'tool-inline-error',
              toolName: 'search_knowledge_base',
              arguments: JSON.stringify({ query: 'connection lost' }),
            });
          }, 30);
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

test('surfaces timeout state without falling back to a separate error bubble', async ({ page }) => {
  await page.goto('/chat');

  await expect(page.getByText('Investigating the failing connection path.').first()).toBeVisible();
  await expect(page.getByText('search_knowledge_base')).toBeVisible();
  await expect(page.getByText('Connection lost', { exact: true })).toBeVisible();
  await expect(page.getByText('An error occurred')).toHaveCount(0);
});
