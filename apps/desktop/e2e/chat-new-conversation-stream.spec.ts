import { expect, test } from '@playwright/test';
import { RUN_EVENT_FIXTURE_INIT_SCRIPT } from './run-event-fixture';

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ content: RUN_EVENT_FIXTURE_INIT_SCRIPT });
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    history.replaceState(
      {
        usr: {
          initialMessage: 'What should we change for retries?',
          projectId: 'project-live',
        },
        key: 'e2e-initial-message',
        idx: 0,
      },
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
    const createConversationArgs: Array<Record<string, unknown>> = [];
    const frontendPaintCalls: Array<Record<string, unknown>> = [];

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const emitEvent = (eventName: string, payload: Record<string, unknown>) => {
      const convert = (window as unknown as {
        __toRunEventFixture?: (
          name: string,
          value: Record<string, unknown>,
        ) => { eventName: string; payload: Record<string, unknown> };
      }).__toRunEventFixture;
      const converted = convert?.(eventName, payload);
      if (converted) {
        eventName = converted.eventName;
        payload = converted.payload;
      }
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
      id: 'cfg-new-conversation-stream',
      name: 'New Conversation Stream Config',
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
          createConversationArgs.push(clone(args));
          const id = 'conv-created-live';
          const conversation: Conversation = {
            id,
            title: '',
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
        case 'record_agent_frontend_paint_cmd':
          frontendPaintCalls.push(clone(args));
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
          return 0;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const runId = 'run-created-live';
          const turnId = 'turn-created-live';
          const userText = String(args.message ?? '');
          const toolCallId = nextId('tool-search');
          const toolArgs = JSON.stringify({ query: 'retry notes' });

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

          messagesByConversation[conversationId] = [userMessage];
          conversations[conversationId].updatedAt = new Date().toISOString();

          queueMicrotask(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'thinking',
              content: 'Planning the lookup first.',
            });
          });

          queueMicrotask(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'toolCallStart',
              callId: toolCallId,
              toolName: 'search_knowledge_base',
              arguments: toolArgs,
            });
          });

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'toolCallResult',
              callId: toolCallId,
              toolName: 'search_knowledge_base',
              content: 'Found 3 retry notes.',
              isError: false,
              artifacts: null,
            });
          }, 1_600);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'thinking',
              content: 'Drafting the answer now.',
            });
          }, 2_000);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'textDelta',
              delta: 'Final answer: keep the timeout guard',
            });
          }, 2_600);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'textDelta',
              delta: ' and surface retry limits.',
            });
          }, 4_500);

          setTimeout(() => {
            const assistantToolMessage: Message = {
              id: nextId('m-assistant-tools'),
              conversationId,
              role: 'assistant',
              content: '',
              toolCallId: null,
              toolCalls: [{ id: toolCallId, name: 'search_knowledge_base', arguments: toolArgs }],
              artifacts: null,
              tokenCount: 0,
              createdAt: new Date().toISOString(),
              sortOrder: 1,
              thinking: 'Planning the lookup first.',
              imageAttachments: null,
            };
            const toolMessage: Message = {
              id: nextId('m-tool'),
              conversationId,
              role: 'tool',
              content: 'Found 3 retry notes.',
              toolCallId: toolCallId,
              toolCalls: [],
              artifacts: null,
              tokenCount: 0,
              createdAt: new Date().toISOString(),
              sortOrder: 2,
              thinking: null,
              imageAttachments: null,
            };
            const finalAssistantMessage: Message = {
              id: nextId('m-assistant-final'),
              conversationId,
              role: 'assistant',
              content: 'Final answer: keep the timeout guard and surface retry limits.',
              toolCallId: null,
              toolCalls: [],
              artifacts: null,
              tokenCount: 0,
              createdAt: new Date().toISOString(),
              sortOrder: 3,
              thinking: 'Drafting the answer now.',
              imageAttachments: null,
            };

            messagesByConversation[conversationId] = [
              userMessage,
              assistantToolMessage,
              toolMessage,
              finalAssistantMessage,
            ];
            conversations[conversationId].updatedAt = new Date().toISOString();

            emitEvent('agent://run-event', {
              conversationId,
              runId,
              turnId,
              type: 'done',
              message: finalAssistantMessage,
              usageTotal: {
                promptTokens: 800,
                completionTokens: 200,
                totalTokens: 1000,
                thinkingTokens: 0,
              },
              lastPromptTokens: 800,
              finishReason: 'stop',
              cached: false,
            });
          }, 5_500);

          return {
            sessionId: conversationId,
            runId: 'run-created-live',
            turnId: 'turn-created-live',
            state: 'starting',
          };
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
    (window as unknown as { __frontendPaintCalls__: Array<Record<string, unknown>> })
      .__frontendPaintCalls__ = frontendPaintCalls;
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__ = createConversationArgs;

    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('keeps the first live thinking and tool call visible when a new conversation is auto-created', async ({ page }) => {
  await page.goto('/chat');

  const chatLog = page.getByLabel('Chat messages');
  await expect(page.getByText('Planning the lookup first.').first()).toBeVisible();
  await expect(chatLog.getByTestId('tool-call-card')).toBeVisible();
  await expect(chatLog.getByText('Final answer: keep the timeout guard', { exact: true })).toBeVisible();
  await expect(chatLog.getByText('Final answer: keep the timeout guard and surface retry limits.', { exact: true })).not.toBeVisible();
  await expect(page.getByText('Final answer: keep the timeout guard and surface retry limits.')).toBeVisible();

  await chatLog.getByRole('button', { name: /Thinking completed/ }).click();
  await expect(chatLog.getByText('Planning the lookup first.')).toBeVisible();
  await expect(chatLog.getByTestId('tool-call-card')).toBeVisible();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __frontendPaintCalls__: Array<Record<string, unknown>> })
      .__frontendPaintCalls__[0],
  )).toMatchObject({
    conversationId: 'conv-created-live',
    runId: 'run-created-live',
    turnId: 'turn-created-live',
  });
  await expect.poll(() => page.evaluate(() => Number(
    (window as unknown as { __frontendPaintCalls__: Array<Record<string, unknown>> })
      .__frontendPaintCalls__[0]?.elapsedMs ?? 0,
  ))).toBeGreaterThan(0);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __CREATE_CONVERSATION_ARGS__: Array<Record<string, unknown>> })
      .__CREATE_CONVERSATION_ARGS__[0],
  )).toMatchObject({
    provider: 'open_ai',
    model: 'gpt-4.1',
    projectId: 'project-live',
  });
});
