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
    const staleRunningMode = () => localStorage.getItem('e2e-steering-stale-running') === '1';
    const retryMode = () => localStorage.getItem('e2e-steering-retry') === '1';
    let retryMessagesSeeded = false;
    const diagnostics = {
      chatCalls: 0,
      stopCalls: 0,
      chatMessages: [] as string[],
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

    const appendUserMessage = (
      conversationId: string,
      content: string,
      artifacts: Record<string, unknown> | null = null,
    ) => {
      messages.push({
        id: nextId('m-user'),
        conversationId,
        role: 'user',
        content,
        toolCallId: null,
        toolCalls: [],
        artifacts,
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: messages.length,
        thinking: null,
        imageAttachments: null,
      });
    };

    const ensureRetryMessages = () => {
      if (!retryMode() || retryMessagesSeeded) return;
      retryMessagesSeeded = true;
      messages.push(
        {
          id: 'm-retry-user',
          conversationId: conversation.id,
          role: 'user',
          content: 'Persisted retry prompt',
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
          id: 'm-retry-assistant',
          conversationId: conversation.id,
          role: 'assistant',
          content: 'Persisted answer before retry.',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 1,
          thinking: null,
          imageAttachments: null,
        },
      );
    };

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      ensureRetryMessages();
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
        case 'get_agent_task_runs_cmd':
          return [
            {
              id: 'task-steering',
              conversationId: 'conv-steering',
              turnId: 'turn-steering',
              userMessageId: messages.find((message) => message.role === 'user')?.id ?? 'm-user-0',
              status: staleRunningMode() ? 'running' : 'completed',
              phase: staleRunningMode() ? 'responding' : 'done',
              title: 'Steering task run',
              routeKind: 'DirectResponse',
              summary: staleRunningMode() ? 'Task was active before app close' : 'Task completed',
              errorMessage: null,
              provider: 'open_ai',
              model: 'gpt-4.1',
              plan: null,
              artifacts: null,
              createdAt: nowIso,
              updatedAt: nowIso,
              startedAt: nowIso,
              finishedAt: nowIso,
            },
          ];
        case 'get_agent_task_run_events_cmd':
          return [];
        case 'get_agent_run_events_cmd':
          return staleRunningMode()
            ? [
                {
                  runId: 'task-steering',
                  turnId: 'turn-steering',
                  eventSeq: 1,
                  version: 1,
                  kind: 'thinking',
                  phase: 'responding',
                  label: 'Working on a previous request.',
                  status: 'running',
                  payload: {
                    type: 'thinking',
                    content: 'Working on a previous request.',
                  },
                  createdAt: nowIso,
                },
              ]
            : [];
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
          const message = String(args.message ?? '');
          diagnostics.chatMessages.push(message);
          appendUserMessage(conversationId, message);

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
          if (staleRunningMode()) {
            throw new Error('No running agent for this conversation.');
          }

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

test('does not send stale restored running chats as steering messages', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('e2e-steering-stale-running', '1');
  });
  await page.goto('/chat/conv-steering');

  await page.waitForTimeout(100);
  const textbox = page.getByTestId('chat-input-textarea');
  await textbox.fill('start a new turn after restart');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('Working on the first request.')).toBeVisible();

  const diagnostics = await page.evaluate(() => (window as unknown as {
    __STEERING_E2E__: {
      chatCalls: number;
      stopCalls: number;
      steerCalls: Array<{ conversationId: string; message: string }>;
    };
  }).__STEERING_E2E__);

  expect(diagnostics.chatCalls).toBe(1);
  expect(diagnostics.stopCalls).toBe(0);
  expect(diagnostics.steerCalls).toEqual([]);
});

test('retries a persisted user message after reopening a conversation', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('e2e-steering-retry', '1');
  });
  await page.goto('/chat/conv-steering');

  await expect(page.getByText('Persisted answer before retry.')).toBeVisible();
  await page.getByRole('button', { name: 'Retry' }).first().click();
  await expect(page.getByText('Working on the first request.')).toBeVisible();

  const diagnostics = await page.evaluate(() => (window as unknown as {
    __STEERING_E2E__: {
      chatCalls: number;
      chatMessages: string[];
      steerCalls: Array<{ conversationId: string; message: string }>;
    };
  }).__STEERING_E2E__);

  expect(diagnostics.chatCalls).toBe(1);
  expect(diagnostics.chatMessages).toEqual(['Persisted retry prompt']);
  expect(diagnostics.steerCalls).toEqual([]);
});

test('keeps short user message bubbles close to their text width', async ({ page }) => {
  await page.goto('/chat/conv-steering');

  const textbox = page.getByTestId('chat-input-textarea');
  await textbox.fill('继续');
  await page.getByTestId('chat-send').click();

  const bubble = page.getByLabel('User message').filter({ hasText: '继续' }).last();
  await expect(bubble).toBeVisible();

  const metrics = await bubble.evaluate((node) => {
    const rect = node.getBoundingClientRect();
    const text = node.querySelector('span.whitespace-pre-wrap');
    const textRect = text?.getBoundingClientRect();
    return {
      bubbleWidth: rect.width,
      textWidth: textRect?.width ?? 0,
    };
  });

  expect(metrics.bubbleWidth).toBeLessThan(metrics.textWidth + 80);
});

test('sends steering while an agent stream is running without stopping it', async ({ page }) => {
  await page.goto('/chat/conv-steering');

  const textbox = page.getByTestId('chat-input-textarea');
  await textbox.fill('start with a broad answer');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('Working on the first request.')).toBeVisible();

  await textbox.fill('focus on edge cases instead');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('Steering', { exact: true })).toBeVisible();
  await expect(page.getByText('focus on edge cases instead')).toBeVisible();
  await expect(page.getByText('Adjusted answer after steering.')).toBeVisible();
  await expect(page.getByTestId('task-board')).toHaveCount(0);
  await expect
    .poll(async () =>
      page.locator('body').evaluate((body) => {
        const text = body.innerText;
        return (
          text.indexOf('focus on edge cases instead') >= 0 &&
          text.indexOf('Adjusted answer after steering.') >= 0 &&
          text.indexOf('focus on edge cases instead') <
            text.indexOf('Adjusted answer after steering.')
        );
      }),
    )
    .toBe(true);

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
