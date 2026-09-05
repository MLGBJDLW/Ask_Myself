import { expect, test } from '@playwright/test';
import { RUN_EVENT_FIXTURE_INIT_SCRIPT } from './run-event-fixture';

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ content: RUN_EVENT_FIXTURE_INIT_SCRIPT });
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
    let refreshReleased = false;
    let releaseRefresh: (() => void) | null = null;

    (window as unknown as { __releaseDonePreviewRefresh?: () => void })
      .__releaseDonePreviewRefresh = () => {
        refreshReleased = true;
        const release = releaseRefresh;
        releaseRefresh = null;
        release?.();
      };

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
        case 'get_wizard_state_cmd':
          return { completed: true, completedAt: nowIso };
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'list_projects_cmd':
          return [];
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          const payload = [clone(conversations[id]), clone(messagesByConversation[id] ?? [])] as const;
          if (!refreshDelayActive || refreshReleased) {
            return payload;
          }
          return await new Promise<typeof payload>((resolve) => {
            const release = () => resolve(payload);
            releaseRefresh = release;
            if (refreshReleased) {
              releaseRefresh = null;
              release();
            }
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
          return 0;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const currentMessages = messagesByConversation[conversationId] ?? [];
          const userText = String(args.message ?? '');
          const toolCallId = nextId('tool-fetch');
          const toolArgs = JSON.stringify({ path: 'notes/retries.md' });
          const createToolCallId = nextId('tool-create');
          const createToolArgs = JSON.stringify({
            path: 'notes/retry-summary.md',
            content: [
              'Keep retries bounded.',
              'Show the retry limit.',
              'Log retry exhaustion.',
            ].join('\n'),
          });
          const editToolCallId = nextId('tool-edit');
          const editToolArgs = JSON.stringify({
            path: 'notes/retries.md',
            old_str: 'Retry forever.',
            new_str: [
              'Keep retries bounded.',
              'Show the retry limit.',
            ].join('\n'),
          });

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
            toolCalls: [
              { id: toolCallId, name: 'read_file', arguments: toolArgs },
              { id: createToolCallId, name: 'create_file', arguments: createToolArgs },
              { id: editToolCallId, name: 'edit_file', arguments: editToolArgs },
            ],
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
          const createToolMessage: Message = {
            id: nextId('m-tool-create'),
            conversationId,
            role: 'tool',
            content: 'Created notes/retry-summary.md',
            toolCallId: createToolCallId,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 3,
            thinking: null,
            imageAttachments: null,
          };
          const editToolMessage: Message = {
            id: nextId('m-tool-edit'),
            conversationId,
            role: 'tool',
            content: 'Edited notes/retries.md',
            toolCallId: editToolCallId,
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
            content: 'Final answer: keep retries bounded and show the limit.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 5,
            thinking: 'Writing the final recommendation.',
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = [
            ...currentMessages,
            userMessage,
          ];
          conversations[conversationId].updatedAt = new Date().toISOString();

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'thinking',
              content: 'Checking the retry note first.',
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallStart',
              callId: toolCallId,
              toolName: 'read_file',
              arguments: toolArgs,
            });
          }, 60);

          setTimeout(() => {
            emitEvent('agent://run-event', {
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
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallStart',
              callId: createToolCallId,
              toolName: 'create_file',
              arguments: createToolArgs,
            });
          }, 70);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallResult',
              callId: createToolCallId,
              toolName: 'create_file',
              content: createToolMessage.content,
              isError: false,
              artifacts: null,
            });
          }, 2_600);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallStart',
              callId: editToolCallId,
              toolName: 'edit_file',
              arguments: editToolArgs,
            });
          }, 75);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallResult',
              callId: editToolCallId,
              toolName: 'edit_file',
              content: editToolMessage.content,
              isError: false,
              artifacts: null,
            });
          }, 2_600);

          setTimeout(() => {
            refreshDelayActive = true;
            messagesByConversation[conversationId] = [
              ...currentMessages,
              userMessage,
              assistantToolMessage,
              toolMessage,
              createToolMessage,
              editToolMessage,
              finalAssistantMessage,
            ];
            emitEvent('agent://run-event', {
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
          }, 3_000);

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

test('keeps live thinking and file edit badges mounted until persisted messages load', async ({ page }) => {
  await page.goto('/chat/conv-done-preview');

  await page.getByTestId('chat-input-textarea').fill('Summarize the retry guidance.');
  await page.getByTestId('chat-send').click();

  await page.waitForTimeout(120);
  const chatLog = page.getByLabel('Chat messages');
  await expect(page.getByText('Checking the retry note first.')).toBeVisible();
  const thinkingTrace = chatLog.locator('.thinking-trace').first();
  await expect(thinkingTrace).toHaveAttribute('data-trace-active', 'true');
  await expect(thinkingTrace.locator('.thinking-trace-node')).toBeVisible();
  await expect(chatLog.getByRole('button', { name: /Read file/i })).toHaveCount(1);
  const liveCreateFile = chatLog.getByRole('button', { name: /Create File.*retry-summary\.md/i });
  await expect(liveCreateFile).toHaveCount(1);
  await expect(liveCreateFile).toHaveAttribute('data-testid', 'tool-call-card');
  await expect(liveCreateFile).toHaveAttribute('data-tool-state', 'running');
  await expect(liveCreateFile).toHaveAttribute('data-tool-tone', 'edit');
  await expect(liveCreateFile).toHaveAttribute('aria-busy', 'true');
  await expect(liveCreateFile).toBeDisabled();
  await expect(liveCreateFile).not.toHaveAttribute('aria-expanded', /.+/);
  await expect(liveCreateFile).not.toContainText(/Running tool/i);
  await expect(liveCreateFile.getByTestId('tool-card-status')).toHaveCount(0);
  await expect(liveCreateFile.locator('.animate-spin')).toHaveCount(0);
  await expect.poll(() => liveCreateFile.evaluate((element) =>
    getComputedStyle(element, '::after').animationName,
  )).toBe('chat-tool-card-border-flow');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect.poll(() => liveCreateFile.evaluate((element) =>
    getComputedStyle(element, '::after').animationName,
  )).toBe('none');
  await expect.poll(() => liveCreateFile.evaluate((element) =>
    getComputedStyle(element).borderTopColor,
  )).not.toBe('rgba(0, 0, 0, 0)');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(liveCreateFile).not.toContainText('content');
  await expect(liveCreateFile).not.toContainText(/\d+\s+B/);
  await expect(liveCreateFile.getByTestId('tool-card-header-additions')).toHaveAttribute('data-value', '+3');
  await expect(liveCreateFile.getByTestId('tool-card-header-deletions')).toHaveAttribute('data-value', '-0');
  const liveEditFile = chatLog.getByRole('button', { name: /Edit file.*retries\.md/i });
  await expect(liveEditFile).toHaveCount(1);
  await expect(liveEditFile).toHaveAttribute('data-tool-state', 'running');
  await expect(liveEditFile).toBeDisabled();
  await expect(liveEditFile).not.toHaveAttribute('aria-expanded', /.+/);
  await expect(liveEditFile.getByTestId('tool-card-header-additions')).toHaveAttribute('data-value', '+2');
  await expect(liveEditFile.getByTestId('tool-card-header-deletions')).toHaveAttribute('data-value', '-1');

  await page.waitForTimeout(140);
  await expect(page.getByText('Checking the retry note first.')).toBeVisible({ timeout: 50 });

  await expect(page.getByText('Final answer: keep retries bounded and show the limit.')).toBeVisible({ timeout: 4_000 });
  await page.locator('button').filter({ hasText: /Thinking completed|Thought for/ }).first().click();
  await expect(chatLog.getByRole('button', { name: /Read file/i })).toHaveCount(1);
  const settledLiveCreateFile = chatLog.getByRole('button', { name: /Create File.*\+3.*-0/i });
  await expect(settledLiveCreateFile).toHaveCount(1);
  await expect(settledLiveCreateFile).toHaveAttribute('data-tool-state', 'done');
  await expect(settledLiveCreateFile).toHaveAttribute('aria-busy', 'false');
  await expect(settledLiveCreateFile.getByTestId('tool-card-status')).toHaveCount(1);
  await expect(settledLiveCreateFile.locator('.animate-spin')).toHaveCount(0);
  await expect(settledLiveCreateFile).not.toContainText('content');

  await page.evaluate(() => {
    (window as unknown as { __releaseDonePreviewRefresh?: () => void })
      .__releaseDonePreviewRefresh?.();
  });
  await expect(settledLiveCreateFile).toHaveCount(0);
  await page.locator('button').filter({ hasText: /Thinking completed|Thought for/ }).first().click();

  const persistedCreateFile = chatLog.getByRole('button', { name: /Create File.*\+3.*-0/i });
  await expect(persistedCreateFile).toHaveCount(1);
  await expect(persistedCreateFile).toHaveAttribute('data-tool-state', 'done');
  await expect(persistedCreateFile).toHaveAttribute('aria-busy', 'false');
  await expect(persistedCreateFile.getByTestId('tool-card-status')).toHaveCount(1);
  await expect(persistedCreateFile.locator('.animate-spin')).toHaveCount(0);
  await expect(persistedCreateFile).not.toContainText('content');
});
