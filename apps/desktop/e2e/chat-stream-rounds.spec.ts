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
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {
      'conv-stream-rounds': {
        id: 'conv-stream-rounds',
        title: 'Streaming Rounds Demo',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-stream-rounds': [],
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

    const defaultAgentConfig = {
      id: 'cfg-stream-rounds',
      name: 'Stream Rounds Config',
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
          const currentMessages = messagesByConversation[conversationId] ?? [];
          const userText = String(args.message ?? '');
          const firstToolCallId = nextId('tool-search');
          const secondToolCallId = nextId('tool-compare');
          const firstToolArgs = JSON.stringify({ query: 'retry edge cases' });
          const secondToolArgs = JSON.stringify({ left: 'notes/a.md', right: 'notes/b.md' });

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
            sortOrder: currentMessages.length,
            thinking: null,
            imageAttachments: null,
          };
          const firstAssistantToolMessage: Message = {
            id: nextId('m-assistant-tools-1'),
            conversationId,
            role: 'assistant',
            content: '',
            toolCallId: null,
            toolCalls: [{ id: firstToolCallId, name: 'search_knowledge_base', arguments: firstToolArgs }],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 1,
            thinking: 'Need to search the knowledge base first.',
            imageAttachments: null,
          };
          const firstToolMessage: Message = {
            id: nextId('m-tool-1'),
            conversationId,
            role: 'tool',
            content: 'Found 2 notes about retry handling.',
            toolCallId: firstToolCallId,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 2,
            thinking: null,
            imageAttachments: null,
          };
          const secondAssistantToolMessage: Message = {
            id: nextId('m-assistant-tools-2'),
            conversationId,
            role: 'assistant',
            content: '',
            toolCallId: null,
            toolCalls: [{ id: secondToolCallId, name: 'compare_documents', arguments: secondToolArgs }],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 3,
            thinking: 'Now compare the two candidate files.',
            imageAttachments: null,
          };
          const secondToolMessage: Message = {
            id: nextId('m-tool-2'),
            conversationId,
            role: 'tool',
            content: 'The second file adds a timeout guard.',
            toolCallId: secondToolCallId,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 4,
            thinking: null,
            imageAttachments: null,
          };
          const finalAssistantMessage: Message = {
            id: nextId('m-assistant-final'),
            conversationId,
            role: 'assistant',
            content: 'Final answer: add the timeout guard from the second file.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 5,
            thinking: null,
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = [
            ...currentMessages,
            userMessage,
            firstAssistantToolMessage,
            firstToolMessage,
            secondAssistantToolMessage,
            secondToolMessage,
            finalAssistantMessage,
          ];

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Need to search the knowledge base first.',
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallStart',
              callId: firstToolCallId,
              toolName: 'search_knowledge_base',
              arguments: firstToolArgs,
            });
          }, 80);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallResult',
              callId: firstToolCallId,
              toolName: 'search_knowledge_base',
              content: firstToolMessage.content,
              isError: false,
              artifacts: null,
            });
          }, 150);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Now compare the two candidate files.',
            });
          }, 240);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallStart',
              callId: secondToolCallId,
              toolName: 'compare_documents',
              arguments: secondToolArgs,
            });
          }, 360);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallResult',
              callId: secondToolCallId,
              toolName: 'compare_documents',
              content: secondToolMessage.content,
              isError: false,
              artifacts: null,
            });
          }, 430);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'textDelta',
              delta: 'Final answer: add the timeout guard from the second file.',
            });
          }, 520);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: finalAssistantMessage,
              usageTotal: {
                promptTokens: 1200,
                completionTokens: 300,
                totalTokens: 1500,
                thinkingTokens: 0,
              },
              lastPromptTokens: 1200,
              finishReason: 'stop',
              cached: false,
            });
          }, 600);

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

test('preserves multiple thinking and tool rounds during a single streamed response', async ({ page }) => {
  await page.goto('/chat/conv-stream-rounds');

  await page.getByTestId('chat-input-textarea').fill('Walk through the retries problem.');
  await page.getByTestId('chat-send').click();

  await page.waitForTimeout(110);
  await expect(page.getByText('Need to search the knowledge base first.')).toBeVisible();
  await expect(page.getByText('search_knowledge_base')).toBeVisible();

  await page.waitForTimeout(170);
  await expect(page.getByText('Now compare the two candidate files.')).toBeVisible({ timeout: 50 });

  await page.waitForTimeout(120);
  await expect(page.getByText('compare_documents')).toBeVisible();
  await expect(page.getByText('Now compare the two candidate files.')).toBeVisible();

  await page.waitForTimeout(250);
  await expect(page.getByText('Final answer: add the timeout guard from the second file.')).toBeVisible();
  await expect(
    page.locator('button[aria-expanded="false"]').filter({ hasText: 'Thinking completed' }),
  ).toHaveCount(2);

  const chatLogText = await page.getByLabel('Chat messages').textContent();
  expect(chatLogText).toBeTruthy();

  const text = chatLogText ?? '';
  expect(text.indexOf('Need to search the knowledge base first.')).toBeGreaterThanOrEqual(0);
  expect(text.indexOf('search_knowledge_base')).toBeGreaterThan(text.indexOf('Need to search the knowledge base first.'));
  expect(text.indexOf('Now compare the two candidate files.')).toBeGreaterThan(text.indexOf('search_knowledge_base'));
  expect(text.indexOf('compare_documents')).toBeGreaterThan(text.indexOf('Now compare the two candidate files.'));
  expect(text.indexOf('Final answer: add the timeout guard from the second file.')).toBeGreaterThan(text.indexOf('compare_documents'));
});
