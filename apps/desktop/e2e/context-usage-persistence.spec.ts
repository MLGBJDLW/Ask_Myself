import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    if (!sessionStorage.getItem('__e2e_initialized__')) {
      localStorage.removeItem('chat-token-usage-v1');
      sessionStorage.setItem('__e2e_initialized__', '1');
    }

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
      tokenCount: number;
      createdAt: string;
      sortOrder: number;
      thinking: string | null;
      imageAttachments: null;
    };

    const nowIso = new Date().toISOString();
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;

    const conversations: Record<string, Conversation> = {
      'conv-e2e': {
        id: 'conv-e2e',
        title: 'Persist Usage Conversation',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      'conv-empty': {
        id: 'conv-empty',
        title: 'No Usage Conversation',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-e2e': [
        {
          id: 'm-u-1',
          conversationId: 'conv-e2e',
          role: 'user',
          content: 'Hello',
          toolCallId: null,
          toolCalls: [],
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-a-1',
          conversationId: 'conv-e2e',
          role: 'assistant',
          content: 'Hi, how can I help?',
          toolCallId: null,
          toolCalls: [],
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 1,
          thinking: null,
          imageAttachments: null,
        },
      ],
      'conv-empty': [
        {
          id: 'm-u-2',
          conversationId: 'conv-empty',
          role: 'user',
          content: 'Fresh conversation',
          toolCallId: null,
          toolCalls: [],
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: null,
          imageAttachments: null,
        },
      ],
    };

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

    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const defaultAgentConfig = {
      id: 'cfg-e2e',
      name: 'E2E Config',
      provider: 'open_ai',
      apiKey: '',
      baseUrl: null,
      model: 'gpt-4.1',
      temperature: null,
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
          const eventId = Number(args.eventId ?? 0);
          listeners.delete(eventId);
          return null;
        }
        case 'list_agent_configs_cmd':
          return [defaultAgentConfig];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations)
            .sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1))
            .map(clone);
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          const conversation = conversations[id];
          const messages = messagesByConversation[id] ?? [];
          return [clone(conversation), clone(messages)];
        }
        case 'list_sources':
          return [];
        case 'get_conversation_sources_cmd':
          return [];
        case 'set_conversation_sources_cmd':
          return null;
        case 'update_conversation_system_prompt_cmd': {
          const id = String(args.id ?? '');
          const systemPrompt = String(args.systemPrompt ?? '');
          if (conversations[id]) {
            conversations[id].systemPrompt = systemPrompt;
            conversations[id].updatedAt = new Date().toISOString();
          }
          return null;
        }
        case 'list_checkpoints_cmd':
          return [];
        case 'compact_conversation_cmd':
          return null;
        case 'agent_stop_cmd':
          return null;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const userText = String(args.message ?? '');
          const streamUsage = {
            promptTokens: 74000,
            completionTokens: 1400,
            totalTokens: 75400,
            thinkingTokens: 0,
            lastPromptTokens: 74000,
          };

          const currentMessages = messagesByConversation[conversationId] ?? [];
          const userMessage: Message = {
            id: nextId('m-user'),
            conversationId,
            role: 'user',
            content: userText,
            toolCallId: null,
            toolCalls: [],
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length,
            thinking: null,
            imageAttachments: null,
          };
          const assistantMessage: Message = {
            id: nextId('m-assistant'),
            conversationId,
            role: 'assistant',
            content: 'Mock response for context usage persistence.',
            toolCallId: null,
            toolCalls: [],
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 1,
            thinking: null,
            imageAttachments: null,
          };
          messagesByConversation[conversationId] = [...currentMessages, userMessage, assistantMessage];
          if (conversations[conversationId]) {
            conversations[conversationId].updatedAt = new Date().toISOString();
          }

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'usageUpdate',
              usageTotal: streamUsage,
              lastPromptTokens: streamUsage.lastPromptTokens,
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: assistantMessage,
              usageTotal: streamUsage,
              lastPromptTokens: streamUsage.lastPromptTokens,
              finishReason: 'stop',
              cached: false,
            });
          }, 60);

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

test('context usage card persists after reloading the same conversation', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  const usageSummary = page.getByText('7% context used').first();

  await expect(usageSummary).toHaveCount(0);
  await page.getByTestId('chat-input-textarea').fill('Please summarize this thread.');
  await page.getByTestId('chat-send').click();

  await expect(usageSummary).toBeVisible();

  await page.reload();

  await expect(page.getByText('7% context used').first()).toBeVisible();
});

test('usage cache is scoped to conversation id and does not leak to another conversation', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  const usageSummary = page.getByText('7% context used').first();

  await page.getByTestId('chat-input-textarea').fill('Generate usage for this conversation.');
  await page.getByTestId('chat-send').click();
  await expect(usageSummary).toBeVisible();

  await page.goto('/chat/conv-empty');
  await expect(page.getByText(/context used/i)).toHaveCount(0);
});
