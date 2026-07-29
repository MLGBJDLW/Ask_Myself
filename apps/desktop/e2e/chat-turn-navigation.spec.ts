import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

    const now = Date.now();
    const nowIso = new Date(now).toISOString();
    const conversation = {
      id: 'conv-turn-navigation',
      title: 'Turn navigation',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const messages = Array.from({ length: 4 }, (_, index) => {
      const number = index + 1;
      const userId = `m-user-${number}`;
      const assistantId = `m-assistant-${number}`;
      const questionArguments = JSON.stringify({
        questions: [{
          id: 'scope',
          header: 'Scope',
          question: 'Which scope should I use?',
          type: 'single_choice',
          options: [
            { label: 'App (Recommended)', description: 'Only this app.' },
            { label: 'Repository', description: 'The whole repository.' },
          ],
        }],
      });
      const rows = [
        {
          id: userId,
          conversationId: conversation.id,
          role: 'user',
          content: `Question ${number}: explain the navigation behavior for this turn.`,
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: new Date(now + index * 2_000).toISOString(),
          sortOrder: index * 2,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: assistantId,
          conversationId: conversation.id,
          role: 'assistant',
          content: Array.from(
            { length: 9 },
            (_, paragraph) => `Turn ${number}, paragraph ${paragraph + 1}. This content makes the conversation tall enough to exercise precise scrolling.`,
          ).join('\n\n'),
          toolCallId: null,
          toolCalls: index === 0
            ? [{ id: 'question-call-1', name: 'request_user_input', arguments: questionArguments }]
            : [],
          artifacts: null,
          tokenCount: 0,
          createdAt: new Date(now + index * 2_000 + 1_000).toISOString(),
          sortOrder: index * 2 + 1,
          thinking: null,
          imageAttachments: null,
        },
      ];
      if (index === 0) {
        rows.push({
          id: 'm-tool-question-1',
          conversationId: conversation.id,
          role: 'tool',
          content: 'Questions displayed.',
          toolCallId: 'question-call-1',
          toolCalls: [],
          artifacts: {
            kind: 'questionRequest',
            version: 1,
            callId: 'question-call-1',
            status: 'pending',
            questions: JSON.parse(questionArguments).questions,
          },
          tokenCount: 0,
          createdAt: new Date(now + 1_500).toISOString(),
          sortOrder: 1.5,
          thinking: null,
          imageAttachments: null,
        });
      }
      return rows;
    }).flat();
    const turns = Array.from({ length: 4 }, (_, index) => {
      const number = index + 1;
      return {
        id: `turn-${number}`,
        conversationId: conversation.id,
        userMessageId: `m-user-${number}`,
        assistantMessageId: `m-assistant-${number}`,
        status: 'completed',
        routeKind: 'DirectResponse',
        trace: null,
        createdAt: new Date(now + index * 2_000).toISOString(),
        updatedAt: new Date(now + index * 2_000 + 1_000).toISOString(),
        finishedAt: new Date(now + index * 2_000 + 1_000).toISOString(),
      };
    });
    const defaultAgentConfig = {
      id: 'cfg-turn-navigation',
      name: 'Turn Navigation Config',
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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    const agentChatCalls: Array<Record<string, unknown>> = [];

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
        case 'plugin:event|unlisten':
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        case 'agent_chat_cmd':
          agentChatCalls.push(clone(args));
          return null;
        case 'get_wizard_state':
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), clone(messages)];
        case 'get_conversation_turns_cmd':
          return clone(turns);
        case 'get_agent_task_runs_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_personas_cmd':
        case 'list_projects_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
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
        case 'get_video_config_cmd':
          return { enabled: false, ffmpegPath: '', whisperModel: '', maxDurationSeconds: 0 };
        case 'get_package_host_snapshot_cmd':
          return { packages: [], components: [] };
        default:
          return null;
      }
    };

    (window as unknown as { __questionAgentChatCalls__: Array<Record<string, unknown>> }).__questionAgentChatCalls__ = agentChatCalls;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbackMap.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbackMap.delete(id),
      convertFileSrc: (filePath: string) => filePath,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => listeners.delete(eventId),
    };
  });
});

test('navigates to every conversation turn from the right-side timeline', async ({ page }) => {
  await page.goto('/chat/conv-turn-navigation');

  const log = page.getByRole('log');
  const navigator = page.getByTestId('chat-turn-navigator');
  await expect(navigator).toBeVisible();
  await expect(navigator).toHaveAttribute('data-variant', 'thread-minimap');
  await expect(navigator).toHaveAttribute('aria-orientation', 'vertical');
  await expect(navigator.getByRole('button')).toHaveCount(4);
  await expect(navigator.getByTestId('chat-turn-minimap-marker')).toHaveCount(4);
  await expect(navigator.getByTestId('chat-turn-minimap-progress')).toHaveCount(1);
  await expect(navigator.getByTestId('chat-turn-position')).toHaveCount(0);

  const logBox = await log.boundingBox();
  const navigatorBox = await navigator.boundingBox();
  expect(logBox).not.toBeNull();
  expect(navigatorBox).not.toBeNull();
  expect(navigatorBox!.x).toBeGreaterThan(logBox!.x + logBox!.width * 0.9);
  expect(Math.abs(logBox!.x + logBox!.width - (navigatorBox!.x + navigatorBox!.width))).toBeLessThan(16);

  await navigator.getByRole('button', { name: /^#1 ·/ }).click();
  await expect(navigator.getByRole('button', { name: /^#1 ·/ })).toHaveAttribute('aria-current', 'step');
  await expect(navigator.getByRole('button', { name: /^#1 ·/ }).getByTestId('chat-turn-minimap-marker')).toHaveAttribute('data-active', 'true');
  await expect.poll(async () => log.evaluate((element) => element.scrollTop)).toBeLessThan(120);

  await navigator.getByRole('button', { name: /^#1 ·/ }).hover();
  await expect(navigator.getByRole('button', { name: /^#1 ·/ }).getByTestId('chat-turn-preview')).toBeVisible();

  await navigator.getByRole('button', { name: /^#1 ·/ }).focus();
  await navigator.getByRole('button', { name: /^#1 ·/ }).press('End');
  await expect(navigator.getByRole('button', { name: /^#4 ·/ })).toHaveAttribute('aria-current', 'step');
  await expect(navigator.getByRole('button', { name: /^#4 ·/ }).getByTestId('chat-turn-minimap-marker')).toHaveAttribute('data-active', 'true');
  await expect.poll(async () => log.evaluate((element) => element.scrollTop)).toBeGreaterThan(500);
});

test('keeps the compact turn timeline out of narrow chat layouts', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 900 });
  await page.goto('/chat/conv-turn-navigation');

  await expect(page.getByTestId('chat-turn-navigator')).toBeHidden();
});

test('renders and submits agent-requested question cards', async ({ page }) => {
  await page.goto('/chat/conv-turn-navigation');

  const card = page.getByTestId('question-request-card');
  await expect(card).toBeVisible();
  await expect(card).toContainText('Which scope should I use?');
  await card.getByRole('radio', { name: /Repository/ }).click();
  await card.getByRole('button', { name: 'Submit answers' }).click();
  await expect(card).toContainText('Answered');

  await expect.poll(() => page.evaluate(() => {
    const call = (window as unknown as { __questionAgentChatCalls__: Array<Record<string, unknown>> })
      .__questionAgentChatCalls__[0];
    return (call?.userArtifacts as Record<string, unknown> | undefined)?.requestCallId;
  })).toBe('question-call-1');
});
