import { expect, test } from '@playwright/test';
import { RUN_EVENT_FIXTURE_INIT_SCRIPT } from './run-event-fixture';

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ content: RUN_EVENT_FIXTURE_INIT_SCRIPT });
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    if (!sessionStorage.getItem('__e2e_initialized__')) {
      localStorage.removeItem('chat-token-usage-v1');
      localStorage.removeItem('__e2e_usage_samples__');
      localStorage.removeItem('nexa.context-compaction.v1.conv-e2e');
      localStorage.removeItem('nexa.context-compaction.v1.conv-empty');
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
      artifacts?: unknown;
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

    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    type UsageSample = {
      promptTokens: number;
      completionTokens: number;
      totalTokens: number;
      thinkingTokens: number;
      cacheReadTokens: number;
      cacheMissTokens: number;
      cacheCreationTokens: number;
      lastPromptTokens: number;
      contextBreakdown?: unknown;
    };
    const usageSamples = (): Record<string, UsageSample[]> =>
      JSON.parse(localStorage.getItem('__e2e_usage_samples__') ?? '{}') as Record<string, UsageSample[]>;
    const recordUsageSample = (conversationId: string, sample: UsageSample) => {
      const all = usageSamples();
      all[conversationId] = [...(all[conversationId] ?? []), sample];
      localStorage.setItem('__e2e_usage_samples__', JSON.stringify(all));
    };
    const durableUsageSnapshot = (conversationId: string) => {
      const samples = usageSamples()[conversationId] ?? [];
      const latest = samples.at(-1);
      if (!latest) return null;
      const sum = (field: keyof UsageSample) => samples.reduce((total, sample) => {
        const value = sample[field];
        return total + (typeof value === 'number' ? value : 0);
      }, 0);
      return {
        source: 'provider',
        promptTokens: sum('promptTokens'),
        completionTokens: sum('completionTokens'),
        totalTokens: sum('totalTokens'),
        thinkingTokens: sum('thinkingTokens'),
        cacheReadTokens: sum('cacheReadTokens'),
        cacheMissTokens: sum('cacheMissTokens'),
        cacheCreationTokens: sum('cacheCreationTokens'),
        lastPromptTokens: latest.lastPromptTokens,
        contextCapacity: 1048576,
        contextAuthority: 'catalog',
        contextBreakdown: latest.contextBreakdown,
        providerRaw: null,
      };
    };

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
        case 'get_conversation_usage_snapshot_cmd':
          return durableUsageSnapshot(String(args.conversationId ?? ''));
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
        case 'start_context_compaction_cmd': {
          const request = (args.request ?? {}) as Record<string, unknown>;
          const conversationId = String(request.conversationId ?? '');
          const operationId = `ctx-${String(request.idempotencyKey ?? nextId('operation'))}`;
          localStorage.setItem(`__e2e_compaction_${operationId}`, JSON.stringify({
            conversationId,
            startedAt: Date.now(),
            state: 'running',
          }));
          return {
            operationId,
            conversationId,
            snapshotVersion: conversations[conversationId]?.updatedAt ?? nowIso,
            state: 'running',
            phase: 'queued',
          };
        }
        case 'observe_context_compaction_cmd': {
          const operationId = String(args.operationId ?? '');
          const raw = localStorage.getItem(`__e2e_compaction_${operationId}`);
          if (!raw) throw new Error(`Unknown compaction operation ${operationId}`);
          const operation = JSON.parse(raw) as {
            conversationId: string;
            startedAt: number;
            state: 'running' | 'cancelled';
          };
          const remaining = Math.max(0, 1200 - (Date.now() - operation.startedAt));
          if (operation.state === 'running' && remaining > 0) {
            await new Promise((resolve) => setTimeout(resolve, Math.min(remaining, 250)));
          }
          const completed = operation.state === 'running'
            && Date.now() - operation.startedAt >= 1200;
          const state = operation.state === 'cancelled'
            ? 'cancelled'
            : completed ? 'completed' : 'running';
          const before = messagesByConversation[operation.conversationId] ?? [];
          const result = {
            conversationId: operation.conversationId,
            checkpointId: completed ? operationId : null,
            messagesBefore: before.length,
            messagesAfter: before.length,
            tokensBefore: 74000,
            tokensAfter: 1300,
            evictedMessages: Math.max(0, before.length - 2),
            summaryKind: 'abstractive',
            fallbackReason: null,
          };
          const cursor = completed || state === 'cancelled' ? 3 : 2;
          return {
            record: {
              activityId: operationId,
              state,
              startedAt: new Date(operation.startedAt).toISOString(),
              updatedAt: new Date().toISOString(),
              completedAt: state === 'running' ? null : new Date().toISOString(),
              lastEventSeq: cursor,
            },
            cursor,
            events: state === 'running'
              ? [{
                  activityId: operationId,
                  seq: 2,
                  timestamp: new Date().toISOString(),
                  kind: 'progress',
                  payload: { phase: 'summarizing', progress: 0.45 },
                }]
              : [{
                  activityId: operationId,
                  seq: 3,
                  timestamp: new Date().toISOString(),
                  kind: state,
                  payload: {
                    state,
                    detail: state === 'completed'
                      ? { eventKind: 'operationCompleted', result }
                      : { eventKind: 'operationCancelled', reason: 'user_requested' },
                  },
                }],
            timedOut: state === 'running',
          };
        }
        case 'cancel_context_compaction_cmd': {
          const operationId = String(args.operationId ?? '');
          const raw = localStorage.getItem(`__e2e_compaction_${operationId}`);
          if (raw) {
            const operation = JSON.parse(raw) as Record<string, unknown>;
            localStorage.setItem(`__e2e_compaction_${operationId}`, JSON.stringify({
              ...operation,
              state: 'cancelled',
            }));
          }
          return null;
        }
        case 'compact_conversation_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const before = messagesByConversation[conversationId] ?? [];
          await new Promise((resolve) => setTimeout(resolve, 1200));

          if (before.length === 0) {
            return {
              conversationId,
              messagesBefore: 0,
              messagesAfter: 0,
              tokensBefore: 0,
              tokensAfter: 0,
              evictedMessages: 0,
            };
          }

          const tail = before.slice(-2).map((message, index) => ({
            ...message,
            tokenCount: index === 0 ? 180 : 220,
            sortOrder: index + 1,
          }));
          const compacted: Message[] = [
            {
              id: nextId('m-compact-summary'),
              conversationId,
              role: 'system',
              content: '## Earlier conversation context (summarized)\nThe previous turns were compacted for the E2E test.',
              toolCallId: null,
              toolCalls: [],
              tokenCount: 900,
              createdAt: new Date().toISOString(),
              sortOrder: 0,
              thinking: null,
              imageAttachments: null,
            },
            ...tail,
          ];
          messagesByConversation[conversationId] = compacted;

          return {
            conversationId,
            messagesBefore: before.length,
            messagesAfter: compacted.length,
            tokensBefore: 74000,
            tokensAfter: 1300,
            evictedMessages: Math.max(0, before.length + 1 - compacted.length),
          };
        }
        case 'agent_stop_cmd':
          return null;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const userText = String(args.message ?? '');
          const lowerCacheSample = /lower cache/i.test(userText);
          const responseDelay = /keep streaming/i.test(userText) ? 1200 : 60;
          const streamUsage = {
            promptTokens: 74000,
            completionTokens: 1400,
            totalTokens: 75400,
            thinkingTokens: 0,
            cacheReadTokens: lowerCacheSample ? 10000 : 30000,
            cacheMissTokens: 30000,
            cacheCreationTokens: lowerCacheSample ? 1000 : 2000,
            lastPromptTokens: 74000,
            contextBreakdown: {
              totalTokens: 74000,
              segments: [
                { kind: 'systemCore', tokens: 3000 },
                { kind: 'runtime', tokens: 1500 },
                { kind: 'userMemory', tokens: 2000 },
                { kind: 'availableSkills', tokens: 1000 },
                { kind: 'sourceScope', tokens: 500 },
                { kind: 'toolCalls', tokens: 400 },
              ],
            },
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
            artifacts: args.userArtifacts ?? null,
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
            artifacts: null,
          };
          messagesByConversation[conversationId] = [...currentMessages, userMessage, assistantMessage];
          recordUsageSample(conversationId, streamUsage);
          if (conversations[conversationId]) {
            conversations[conversationId].updatedAt = new Date().toISOString();
          }

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'usageUpdate',
              usageTotal: streamUsage,
              lastPromptTokens: streamUsage.lastPromptTokens,
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'done',
              message: assistantMessage,
              usageTotal: streamUsage,
              lastPromptTokens: streamUsage.lastPromptTokens,
              finishReason: 'stop',
              cached: false,
            });
          }, responseDelay);

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

test('context usage ring persists after reloading the same conversation', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  const contextTrigger = page.getByTestId('chat-context-trigger');
  const contextDetails = page.getByTestId('chat-context-details');

  await expect(contextTrigger).not.toHaveAttribute('aria-label', /7% context used/);
  await page.getByTestId('chat-input-textarea').fill('Please summarize this thread.');
  await page.getByTestId('chat-send').click();

  await expect(contextTrigger).toHaveAttribute('aria-label', /7% context used/);
  await contextTrigger.hover();
  await expect(contextDetails).toBeVisible();
  await expect(contextTrigger).toHaveAttribute('aria-expanded', 'true');
  const triggerBox = await contextTrigger.boundingBox();
  const detailsBox = await contextDetails.boundingBox();
  expect(triggerBox).not.toBeNull();
  expect(detailsBox).not.toBeNull();
  await page.mouse.move(triggerBox!.x + triggerBox!.width / 2, triggerBox!.y + 1);
  await page.mouse.move(
    detailsBox!.x + detailsBox!.width / 2,
    detailsBox!.y + detailsBox!.height - 2,
    { steps: 12 },
  );
  await expect(contextDetails).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(contextDetails).toBeHidden();
  await expect(contextTrigger).toHaveAttribute('aria-expanded', 'false');

  await page.mouse.move(0, 0);
  await contextTrigger.hover();
  await expect(contextDetails).toBeVisible();
  await page.mouse.move(0, 0);
  await expect(contextDetails).toBeHidden();

  await contextTrigger.focus();
  await expect(contextDetails).toBeVisible();
  await expect(contextDetails.getByText('7% context used')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(contextDetails).toBeHidden();

  await page.reload();

  await expect(contextTrigger).toHaveAttribute('aria-label', /7% context used/);
  await contextTrigger.click();
  await expect(page.getByTestId('chat-context-details')).toContainText('Verified endpoint catalog');
});

test('usage cache is scoped to conversation id and does not leak to another conversation', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  const contextTrigger = page.getByTestId('chat-context-trigger');

  await page.getByTestId('chat-input-textarea').fill('Generate usage for this conversation.');
  await page.getByTestId('chat-send').click();
  await expect(contextTrigger).toHaveAttribute('aria-label', /7% context used/);

  await page.goto('/chat/conv-empty');
  await expect(contextTrigger).not.toHaveAttribute('aria-label', /\d+% context used/);
});

test('context HUD groups detailed segments and averages cache across completed turns', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  const contextTrigger = page.getByTestId('chat-context-trigger');
  const contextDetails = page.getByTestId('chat-context-details');

  await page.getByTestId('chat-input-textarea').fill('Generate the first cache sample.');
  await page.getByTestId('chat-send').click();

  await expect(page.getByTestId('chat-run-cache-hit-summary')).toBeVisible();
  await expect(page.getByTestId('chat-run-cache-hit-summary')).toContainText('50.0%');
  await contextTrigger.hover();
  await expect(contextDetails).toBeVisible();
  await expect(page.getByTestId('chat-run-cache-hit')).toHaveText('50.0%');
  await expect(contextDetails.getByText('Prompts 4.5K')).toBeVisible();
  await expect(contextDetails.getByText('Memory 2.0K')).toBeVisible();
  await expect(contextDetails.getByText('Skills 1.0K')).toBeVisible();
  await expect(contextDetails.getByText('Sources 500')).toBeVisible();
  await expect(contextDetails.getByText('Tools 400')).toBeVisible();
  await expect(contextDetails.getByText(/Other/)).toHaveCount(0);

  await page.getByTestId('chat-input-textarea').fill('Generate a lower cache sample.');
  await page.getByTestId('chat-send').click();

  await expect(page.getByTestId('chat-run-cache-hit-summary')).toContainText('40.0%');
  await contextTrigger.hover();
  await expect(page.getByTestId('chat-run-cache-hit')).toHaveText('40.0%');
});

test('active goal owns the top-right task capsule and stays out of context details', async ({ page }) => {
  await page.goto('/chat/conv-e2e');

  await page.getByTestId('chat-input-textarea').fill('/goal Finish the performance release');
  await page.getByTestId('chat-send').click();

  const board = page.getByTestId('task-board');
  const panel = page.getByTestId('plan-progress-panel');
  const collapsed = page.getByTestId('task-board-collapsed');
  const trigger = page.getByTestId('chat-context-trigger');
  await expect(board).toBeVisible();
  await expect(panel).toHaveAttribute('data-goal-active', 'true');
  await expect(collapsed).toContainText('Goal');
  await expect(collapsed).toContainText('Finish the performance release');
  await expect(page.getByTestId('chat-context-goal-summary')).toHaveCount(0);

  await trigger.hover();
  await expect(page.getByTestId('chat-context-goal')).toHaveCount(0);
  await page.mouse.move(0, 0);

  const initialBox = await panel.boundingBox();
  if (!initialBox) throw new Error('Missing goal capsule geometry');
  const startX = initialBox.x + initialBox.width / 2;
  const startY = initialBox.y + initialBox.height / 2;

  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX - 180, startY - 80, { steps: 4 });

  await expect.poll(async () => (await panel.boundingBox())?.x ?? initialBox.x).toBeLessThan(initialBox.x - 100);
  await expect(page.getByTestId('chat-context-details')).toBeHidden();
  await page.mouse.up();

  await expect.poll(async () => {
    const box = await panel.boundingBox();
    return box ? Math.abs(box.x - initialBox.x) : Number.POSITIVE_INFINITY;
  }).toBeLessThan(1);
  await expect(panel).not.toHaveAttribute('data-dragging');
});

test('manual compact keeps canonical messages and updates only projected context usage', async ({ page }) => {
  await page.goto('/chat/conv-e2e');

  await page.getByTestId('chat-input-textarea').fill('Generate usage before compacting.');
  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('chat-context-trigger')).toHaveAttribute('aria-label', /7% context used/);

  await page.getByTestId('chat-compact').click();
  await expect(page.getByTestId('chat-compact-status').first()).toBeVisible();
  await expect(page.getByTestId('chat-input-textarea')).toBeEnabled();
  await expect(page.getByTestId('chat-send')).toBeDisabled();
  await page.getByTestId('chat-input-textarea').fill('Draft the next question while compacting.');
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('Draft the next question while compacting.');
  await expect(page.getByTestId('chat-send')).toBeDisabled();

  await expect(page.getByTestId('chat-input-textarea')).toBeEnabled();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('Draft the next question while compacting.');
  await expect(page.getByTestId('chat-compact-status').first()).toBeVisible();
  await expect(page.getByText('Compaction complete').first()).toBeVisible();
  await expect(page.getByTestId('chat-context-trigger')).not.toHaveAttribute('aria-label', /7% context used/);
  await expect(page.getByTestId('chat-context-trigger')).toHaveAttribute('aria-label', /0% context used/);
  await expect(page.getByText('Hello', { exact: true }).last()).toBeVisible();
});

test('manual compact resumes observation after a page reload', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  await page.getByTestId('chat-compact').click();
  await expect(page.getByTestId('chat-compact-status').first()).toContainText('Queued');

  await page.reload();

  await expect(page.getByTestId('chat-compact-status').first()).toBeVisible();
  await expect(page.getByText('Compaction complete').first()).toBeVisible();
  await expect(page.getByText('Hello', { exact: true }).last()).toBeVisible();
});

test('manual compact exposes cancellation and keeps the canonical transcript', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  await page.getByTestId('chat-compact').click();
  await page.getByTestId('chat-compact-cancel').click();

  await expect(page.getByTestId('chat-compact-status').first()).toContainText('user_requested');
  await expect(page.getByText('Hello', { exact: true }).last()).toBeVisible();
});

test('manual compact status and completion stay scoped to the target conversation', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  await page.getByTestId('chat-compact').click();
  await expect(page.getByTestId('chat-compact-status').first()).toBeVisible();

  await page.getByTestId('conversation-item-conv-empty').click();
  await expect(page.getByTestId('chat-compact-status')).toHaveCount(0);
  await expect(page.getByTestId('chat-input-textarea')).not.toHaveAttribute('placeholder', /compacting/i);

  await page.waitForTimeout(500);
  await expect(page.getByText('Compaction complete')).toHaveCount(0);

  await page.getByTestId('conversation-item-conv-e2e').click();
  await expect(page.getByText('Compaction complete').first()).toBeVisible();
});

test('manual compact is rejected while the target conversation is streaming', async ({ page }) => {
  await page.goto('/chat/conv-e2e');
  await page.getByTestId('chat-input-textarea').fill('Keep streaming while I inspect the controls.');
  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('chat-send')).toHaveAttribute('aria-label', 'Steering message');
  await expect(page.getByTestId('chat-compact')).toBeDisabled();

  await page.getByTestId('chat-input-textarea').fill('/compact');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText('Wait for the current response to finish before compacting.')).toBeVisible();
  await expect(page.getByTestId('chat-compact-status')).toHaveCount(0);
});
