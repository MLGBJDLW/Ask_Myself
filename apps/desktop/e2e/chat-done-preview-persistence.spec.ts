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
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {
      'conv-done-preview': {
        id: 'conv-done-preview',
        title: 'Done Preview Gap',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-done-preview': [],
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let refreshDelayActive = false;

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
      id: 'cfg-done-preview',
      name: 'Done Preview Config',
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
          const payload = [clone(conversations[id]), clone(messagesByConversation[id] ?? [])] as const;
          if (!refreshDelayActive) {
            return payload;
          }
          return await new Promise<typeof payload>((resolve) => {
            setTimeout(() => resolve(payload), 700);
          });
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
          const toolCallId = nextId('tool-fetch');
          const toolArgs = JSON.stringify({ path: 'notes/retries.md' });

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
          const assistantToolMessage: Message = {
            id: nextId('m-assistant-tools'),
            conversationId,
            role: 'assistant',
            content: '',
            toolCallId: null,
            toolCalls: [{ id: toolCallId, name: 'read_file', arguments: toolArgs }],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 1,
            thinking: 'Checking the retry note first.',
            imageAttachments: null,
          };
          const toolMessage: Message = {
            id: nextId('m-tool'),
            conversationId,
            role: 'tool',
            content: 'Retry note loaded successfully.',
            toolCallId: toolCallId,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 2,
            thinking: null,
            imageAttachments: null,
          };
          const finalAssistantMessage: Message = {
            id: nextId('m-assistant-final'),
            conversationId,
            role: 'assistant',
            content: 'Final answer: keep retries bounded and show the limit.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 3,
            thinking: 'Writing the final recommendation.',
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = [
            ...currentMessages,
            userMessage,
            assistantToolMessage,
            toolMessage,
            finalAssistantMessage,
          ];
          conversations[conversationId].updatedAt = new Date().toISOString();

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'thinking',
              content: 'Checking the retry note first.',
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallStart',
              callId: toolCallId,
              toolName: 'read_file',
              arguments: toolArgs,
            });
          }, 60);

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallResult',
              callId: toolCallId,
              toolName: 'read_file',
              content: toolMessage.content,
              isError: false,
              artifacts: null,
            });
          }, 100);

          setTimeout(() => {
            refreshDelayActive = true;
            emitEvent('agent:event', {
              conversationId,
              type: 'done',
              message: {
                role: 'assistant',
                parts: [{ type: 'text', text: finalAssistantMessage.content }],
                name: null,
                toolCalls: null,
                reasoningContent: null,
              },
              usageTotal: {
                promptTokens: 900,
                completionTokens: 200,
                totalTokens: 1100,
                thinkingTokens: 0,
              },
              lastPromptTokens: 900,
              finishReason: 'stop',
              cached: false,
            });
          }, 150);

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

test('keeps the live thinking and tool preview mounted until delayed persisted messages load', async ({ page }) => {
  await page.goto('/chat/conv-done-preview');

  await page.getByTestId('chat-input-textarea').fill('Summarize the retry guidance.');
  await page.getByTestId('chat-send').click();

  await page.waitForTimeout(120);
  await expect(page.getByText('Checking the retry note first.')).toBeVisible();
  await expect(page.getByText('read_file')).toBeVisible();

  await page.waitForTimeout(140);
  await expect(page.getByText('Checking the retry note first.')).toBeVisible({ timeout: 50 });
  await expect(page.getByText('read_file')).toBeVisible({ timeout: 50 });

  await page.waitForTimeout(200);
  await expect(page.getByText('Final answer: keep retries bounded and show the limit.')).toBeVisible({ timeout: 50 });

  await expect(page.getByText('Final answer: keep retries bounded and show the limit.')).toBeVisible();
});
