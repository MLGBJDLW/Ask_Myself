import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: number }).__ASK_STREAM_TIMEOUT_MS__ = 150;
    history.replaceState(
      { usr: { initialMessage: 'Why does the stream die during long thinking?' }, key: 'e2e-keepalive', idx: 0 },
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
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {};
    const messagesByConversation: Record<string, Message[]> = {};

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const emitEvent = (eventName: string, payload: Record<string, unknown>) => {
      for (const [listenerId, listener] of listeners.entries()) {
        if (listener.event !== eventName) continue;
        const callback = callbackMap.get(listener.handlerId);
        if (callback) {
          callback({
            event: eventName,
            id: listenerId,
            payload,
          });
        }
      }
    };

    const defaultAgentConfig = {
      id: 'cfg-keepalive',
      name: 'Keepalive Config',
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
        case 'plugin:event|unlisten': {
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        }
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'create_conversation_cmd': {
          const id = 'conv-keepalive';
          const conversation: Conversation = {
            id,
            title: 'Keepalive Test',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
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
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const userText = String(args.message ?? '');
          const userMessage: Message = {
            id: nextId('m-user'),
            conversationId,
            role: 'user',
            content: userText,
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: 0,
            thinking: null,
            imageAttachments: null,
          };
          const assistantMessage: Message = {
            id: nextId('m-assistant'),
            conversationId,
            role: 'assistant',
            content: 'Final answer: keep the stream alive until the real result arrives.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: 1,
            thinking: 'Reasoning through the timeout path.',
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = [userMessage];
          conversations[conversationId].updatedAt = new Date().toISOString();

          queueMicrotask(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Reasoning through the timeout path.',
            });
          });

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: '',
            });
          }, 80);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: '',
            });
          }, 160);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: '',
            });
          }, 240);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'textDelta',
              delta: assistantMessage.content,
            });
          }, 300);

          setTimeout(() => {
            messagesByConversation[conversationId] = [userMessage, assistantMessage];
            conversations[conversationId].updatedAt = new Date().toISOString();
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: assistantMessage,
              usageTotal: {
                promptTokens: 120,
                completionTokens: 45,
                totalTokens: 165,
                thinkingTokens: 12,
              },
              lastPromptTokens: 120,
              finishReason: 'stop',
              cached: false,
            });
          }, 330);

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

test('keeps a live stream active when keepalive events arrive during a long silent phase', async ({ page }) => {
  await page.goto('/chat');

  await expect(page.getByText('Reasoning through the timeout path.').first()).toBeVisible();
  await page.waitForTimeout(220);
  await expect(page.getByText('Connection lost')).toHaveCount(0);
  await expect(page.getByText('Final answer: keep the stream alive until the real result arrives.')).toBeVisible();
});
