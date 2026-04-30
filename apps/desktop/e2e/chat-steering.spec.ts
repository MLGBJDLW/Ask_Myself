import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

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
    const nextId = (prefix: string) => `${prefix}-${seq++}`;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversation = {
      id: 'conv-steering',
      title: 'Steering test',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const messages: Message[] = [];
    const diagnostics = {
      chatCalls: 0,
      stopCalls: 0,
      steerCalls: [] as Array<{ conversationId: string; message: string }>,
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
          callback({ event: eventName, id: listenerId, payload });
        }
      }
    };

    const defaultAgentConfig = {
      id: 'cfg-steering',
      name: 'Steering Config',
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

    const appendUserMessage = (conversationId: string, content: string) => {
      messages.push({
        id: nextId('m-user'),
        conversationId,
        role: 'user',
        content,
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: messages.length,
        thinking: null,
        imageAttachments: null,
      });
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
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), clone(messages)];
        case 'get_conversation_turns_cmd':
          return [];
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_projects_cmd':
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
        case 'clear_answer_cache':
          return 0;
        case 'update_conversation_system_prompt_cmd':
        case 'compact_conversation_cmd':
        case 'save_agent_config_cmd':
          return null;
        case 'agent_stop_cmd':
          diagnostics.stopCalls += 1;
          return null;
        case 'agent_chat_cmd': {
          diagnostics.chatCalls += 1;
          const conversationId = String(args.conversationId ?? '');
          appendUserMessage(conversationId, String(args.message ?? ''));

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Working on the first request.',
            });
          }, 25);
          return null;
        }
        case 'agent_steer_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const message = String(args.message ?? '');
          diagnostics.steerCalls.push({ conversationId, message });
          appendUserMessage(conversationId, message);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'status',
              content: 'Steering message received.',
              tone: 'muted',
            });
          }, 10);
          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'textDelta',
              delta: 'Adjusted answer after steering.',
            });
          }, 35);
          setTimeout(() => {
            const assistantMessage: Message = {
              id: nextId('m-assistant'),
              conversationId,
              role: 'assistant',
              content: 'Adjusted answer after steering.',
              toolCallId: null,
              toolCalls: [],
              artifacts: null,
              tokenCount: 0,
              createdAt: new Date().toISOString(),
              sortOrder: messages.length,
              thinking: null,
              imageAttachments: null,
            };
            messages.push(assistantMessage);
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: assistantMessage,
              usageTotal: {
                promptTokens: 100,
                completionTokens: 20,
                totalTokens: 120,
                thinkingTokens: 0,
              },
              lastPromptTokens: 100,
              finishReason: 'stop',
              cached: false,
            });
          }, 70);
          return null;
        }
        default:
          return null;
      }
    };

    (window as unknown as { __STEERING_E2E__: typeof diagnostics }).__STEERING_E2E__ = diagnostics;
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

test('sends steering while an agent stream is running without stopping it', async ({ page }) => {
  await page.goto('/chat/conv-steering');

  const textbox = page.getByTestId('chat-input-textarea');
  await textbox.fill('start with a broad answer');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('Working on the first request.')).toBeVisible();

  await textbox.fill('focus on edge cases instead');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('focus on edge cases instead')).toBeVisible();
  await expect(page.getByText('Adjusted answer after steering.')).toBeVisible();

  const diagnostics = await page.evaluate(() => (window as unknown as {
    __STEERING_E2E__: {
      chatCalls: number;
      stopCalls: number;
      steerCalls: Array<{ conversationId: string; message: string }>;
    };
  }).__STEERING_E2E__);

  expect(diagnostics.chatCalls).toBe(1);
  expect(diagnostics.stopCalls).toBe(0);
  expect(diagnostics.steerCalls).toEqual([
    { conversationId: 'conv-steering', message: 'focus on edge cases instead' },
  ]);
});
