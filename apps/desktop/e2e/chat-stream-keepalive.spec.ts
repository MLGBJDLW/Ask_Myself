import { expect, test } from '@playwright/test';
import { RUN_EVENT_FIXTURE_INIT_SCRIPT } from './run-event-fixture';

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ content: RUN_EVENT_FIXTURE_INIT_SCRIPT });
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: number }).__ASK_STREAM_TIMEOUT_MS__ = new URL(location.href).searchParams.has('display') ? 60000 : 150;
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
      if (eventName === 'agent://run-event') {
        payload = { runId: 'run-keepalive', turnId: 'turn-keepalive', ...payload };
      }
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
          return 0;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const userText = String(args.message ?? '');
          const exerciseDurableRecovery = localStorage.getItem('e2e-watchdog-silent') === '1';
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

          (window as Window & { __emitPresentationBurst?: () => void }).__emitPresentationBurst = () => emitEvent('agent://run-event', {
            conversationId, type: 'textDelta', delta: 'Fluid example '.repeat(120),
          });
          const finishStream = () => {
            messagesByConversation[conversationId] = [userMessage, assistantMessage];
            conversations[conversationId].updatedAt = new Date().toISOString();
            emitEvent('agent://run-event', {
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
          };

          if (exerciseDurableRecovery) {
            (window as Window & { __finishSilentStream__?: () => void }).__finishSilentStream__ = () => {
              emitEvent('agent://run-event', {
                conversationId,
                type: 'textDelta',
                delta: assistantMessage.content,
              });
              finishStream();
            };
          } else {
            queueMicrotask(() => {
              emitEvent('agent://run-event', {
                conversationId,
                type: 'thinking',
                content: 'Reasoning through the timeout path.',
              });
            });

            for (const delay of [80, 160, 240]) {
              setTimeout(() => {
                emitEvent('agent://run-event', {
                  conversationId,
                  type: 'thinking',
                  content: '',
                });
              }, delay);
            }

            (window as Window & { __finishSilentStream__?: () => void }).__finishSilentStream__ = () => {
              emitEvent('agent://run-event', {
                conversationId,
                type: 'textDelta',
                delta: assistantMessage.content,
              });
              finishStream();
            };
          }

          return {
            sessionId: 'session-keepalive',
            runId: 'run-keepalive',
            turnId: 'turn-keepalive',
            state: 'running',
          };
        }
        case 'get_agent_task_runs_cmd': {
          const recoveryWindow = window as Window & { __E2E_WATCHDOG_QUERY_COUNT__?: number };
          recoveryWindow.__E2E_WATCHDOG_QUERY_COUNT__ = (
            recoveryWindow.__E2E_WATCHDOG_QUERY_COUNT__ ?? 0
          ) + 1;
          if (
            localStorage.getItem('e2e-watchdog-silent') === '1'
            && recoveryWindow.__E2E_WATCHDOG_QUERY_COUNT__ === 4
          ) return [];
          return [{
            id: 'run-keepalive',
            conversationId: 'conv-keepalive',
            turnId: 'turn-keepalive',
            userMessageId: 'm-user-keepalive',
            status: 'running',
            phase: 'responding',
            title: 'Durable watchdog recovery',
            provider: 'open_ai',
            model: 'gpt-4.1',
            createdAt: nowIso,
            updatedAt: new Date().toISOString(),
          }];
        }
        case 'get_agent_run_event_page_cmd':
          return {
            events: [],
            durableHighWater: Number(args.durableHighWater ?? args.afterEventSeq ?? 0),
            nextAfterEventSeq: null,
            hasMore: false,
          };
        case 'get_agent_run_events_cmd':
        case 'get_agent_task_run_events_cmd':
          return [];
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

test('keeps timing above the composer, animates the brain and smoothly reveals provider bursts', async ({ page }, testInfo) => {
  await page.addInitScript(() => localStorage.setItem('nexa-display-preferences', JSON.stringify({ streamingMode: 'smooth' })));
  await page.goto('/chat?display=1');
  const toggle = page.getByTestId('thinking-trace-toggle').first();
  await expect(toggle).toBeVisible();
  await expect(page.getByTestId('chat-turn-elapsed')).toBeVisible({ timeout: 8000 });
  await expect(toggle).not.toContainText(/\d+\s*(s|ms|m)\b/);
  const ink = page.getByTestId('thinking-brain').locator('.thinking-brain-ink path').first();
  await expect.poll(() => ink.evaluate(node => getComputedStyle(node).animationName)).toBe('thinking-brain-draw');
  await toggle.screenshot({ path: testInfo.outputPath('thinking-brain.png') });
  const lengths = await page.evaluate(async () => {
    const values: number[] = [];
    const observe = () => document.querySelectorAll('[data-markdown-source-chars]').forEach(node => values.push(Number(node.getAttribute('data-markdown-source-chars'))));
    const observer = new MutationObserver(observe);
    observer.observe(document.body, { subtree: true, attributes: true, childList: true, attributeFilter: ['data-markdown-source-chars'] });
    (window as Window & { __emitPresentationBurst?: () => void }).__emitPresentationBurst?.();
    await new Promise(resolve => setTimeout(resolve, 550));
    observer.disconnect();
    return [...new Set(values)];
  });
  expect(lengths.filter(value => value > 0 && value < 'Fluid example '.repeat(120).length).length).toBeGreaterThanOrEqual(2);
  expect(lengths).toContain('Fluid example '.repeat(120).length);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  // Reasoning can finish as output starts; the global rule remains static for any active icon.
  const animation = await page.evaluate(() => {
    const node = document.querySelector('.thinking-brain-ink path');
    return node ? getComputedStyle(node).animationName : 'none';
  });
  expect(animation).toBe('none');
  await page.evaluate(() => (window as Window & { __finishSilentStream__?: () => void }).__finishSilentStream__?.());
  await expect(page.getByText('Final answer: keep the stream alive until the real result arrives.')).toBeVisible();
  await expect(page.getByText(/Fluid example/)).toHaveCount(0);
});

test('keeps a live stream active when keepalive events arrive during a long silent phase', async ({ page }) => {
  await page.goto('/chat');

  await expect(page.getByText('Reasoning through the timeout path.').first()).toBeVisible();
  await page.waitForTimeout(220);
  await expect(page.getByText('Connection lost')).toHaveCount(0);
  await page.evaluate(() => (window as Window & { __finishSilentStream__?: () => void }).__finishSilentStream__?.());
  await expect(page.getByText('Final answer: keep the stream alive until the real result arrives.')).toBeVisible();
});

test('queries durable state and preserves an active backend turn after live silence', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('e2e-watchdog-silent', '1');
  });
  await page.goto('/chat');

  await expect.poll(() => page.evaluate(() => (
    (window as Window & { __E2E_WATCHDOG_QUERY_COUNT__?: number })
      .__E2E_WATCHDOG_QUERY_COUNT__ ?? 0
  ))).toBeGreaterThan(0);
  await expect(page.getByText('Durable backend state is active; live recovery remains armed.')).toHaveCount(0);
  await expect(page.getByText('No live events received; checking durable backend state.')).toHaveCount(0);
  await page.waitForTimeout(650);
  await expect.poll(() => page.evaluate(() => (
    (window as Window & { __E2E_WATCHDOG_QUERY_COUNT__?: number })
      .__E2E_WATCHDOG_QUERY_COUNT__ ?? 0
  ))).toBeGreaterThanOrEqual(4);
  await expect(page.getByText('Connection lost')).toHaveCount(0);
  await page.evaluate(() => (window as Window & { __finishSilentStream__?: () => void }).__finishSilentStream__?.());
  await expect(page.getByText('Final answer: keep the stream alive until the real result arrives.')).toBeVisible();
});
