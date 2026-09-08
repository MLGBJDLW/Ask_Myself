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
      collectionContext?: null;
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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();

    const conversation: Conversation = {
      id: 'conv-turn-trace',
      title: 'Turn Trace',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const messages: Message[] = [
      {
        id: 'm-user-turn',
        conversationId: 'conv-turn-trace',
        role: 'user',
        content: 'Why did the retry guard fail?',
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
        id: 'm-assistant-turn',
        conversationId: 'conv-turn-trace',
        role: 'assistant',
        content: 'The retry guard was bypassed because the timeout branch did not return early.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-user-reasoning-only',
        conversationId: 'conv-turn-trace',
        role: 'user',
        content: 'Give me the final summary.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-reasoning-only',
        conversationId: 'conv-turn-trace',
        role: 'assistant',
        content: 'private reasoning accidentally copied into reply',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 3,
        thinking: 'private reasoning accidentally copied into reply',
        imageAttachments: null,
      },
    ];

    const turns = [
      {
        id: 'turn-1',
        conversationId: 'conv-turn-trace',
        userMessageId: 'm-user-turn',
        assistantMessageId: 'm-assistant-turn',
        status: 'success',
        routeKind: 'KnowledgeRetrieval',
        trace: {
          kind: 'turnTrace',
          routeKind: 'KnowledgeRetrieval',
          items: [
            {
              kind: 'tool',
              toolCall: {
                callId: 'skill-turn-1',
                toolName: 'manage_skill',
                arguments: '{"action":"activate_skill","skill_id":"diagnose"}',
                status: 'done',
                isError: false,
                artifacts: {
                  kind: 'skillActivation',
                  skill: {
                    id: 'builtin-diagnose',
                    name: 'diagnose',
                    interface: {
                      displayName: 'Diagnose',
                    },
                  },
                },
              },
            },
            { kind: 'thinking', text: 'Checking the retry path through the saved evidence first.' },
            {
              kind: 'tool',
              toolCall: {
                callId: 'tool-turn-1',
                toolName: 'search_knowledge_base',
                arguments: '{"query":"retry guard"}',
                status: 'done',
                content: 'Found 2 retry notes.',
                isError: false,
                artifacts: null,
              },
            },
          ],
        },
        createdAt: nowIso,
        updatedAt: nowIso,
        finishedAt: nowIso,
      },
      {
        id: 'turn-2',
        conversationId: 'conv-turn-trace',
        userMessageId: 'm-user-reasoning-only',
        assistantMessageId: 'm-assistant-reasoning-only',
        status: 'success',
        routeKind: 'Direct',
        trace: {
          kind: 'turnTrace',
          routeKind: 'Direct',
          items: [
            { kind: 'thinking', text: 'private reasoning accidentally copied into reply' },
          ],
        },
        createdAt: nowIso,
        updatedAt: nowIso,
        finishedAt: nowIso,
      },
    ];

    const defaultAgentConfig = {
      id: 'cfg-turn-trace',
      name: 'Turn Trace Config',
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
        case 'plugin:event|listen':
          return listenerSeq++;
        case 'plugin:event|unlisten':
          return null;
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
        case 'list_sources':
          return [];
        case 'get_conversation_sources_cmd':
          return [];
        case 'set_conversation_sources_cmd':
          return null;
        case 'update_conversation_system_prompt_cmd':
          return null;
        case 'update_conversation_collection_context_cmd':
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
      unregisterListener: () => {},
    };
  });
});

test('renders persisted turn traces from conversation_turns data', async ({ page }) => {
  await page.goto('/chat/conv-turn-trace');

  await expect(page.getByTestId('turn-skill-strip')).toHaveCount(0);
  await page.evaluate(() => localStorage.setItem('nexa-developer-mode', 'true'));
  await page.reload();

  await expect(page.getByTestId('turn-skill-strip')).toBeVisible();
  await expect(page.getByText('Skills loaded this turn')).toBeVisible();
  await expect(page.getByText('Diagnose')).toBeVisible();

  await page.getByRole('button', { name: /Thinking completed/ }).first().click();
  await expect(page.getByText('Route: Knowledge Retrieval')).toBeVisible();
  await expect(page.getByText('Loaded skills: Diagnose')).toBeVisible();
  await expect(page.getByText('Checking the retry path through the saved evidence first.')).toBeVisible();
  await expect(page.getByRole('button', { name: /search_knowledge_base.*retry guard.*done/ })).toBeVisible();
  await expect(page.getByText('The retry guard was bypassed because the timeout branch did not return early.')).toBeVisible();
});

test('quarantines a legacy reasoning-only reply after persistence reload', async ({ page }) => {
  await page.goto('/chat/conv-turn-trace');

  const leakedReasoning = page.getByText('private reasoning accidentally copied into reply', {
    exact: true,
  });
  await expect(leakedReasoning).toHaveCount(0);
  await expect(
    page.getByText('The model ended before producing a final answer. Its reasoning was kept separate.'),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Generate final answer' })).toBeVisible();

  await page.getByRole('button', { name: /Thinking completed/ }).last().click();
  await expect(leakedReasoning).toHaveCount(1);
});


test('large code plan preserves complete source with bounded DOM after hydration', async ({ page }) => {
    await page.addInitScript(() => {
        const invoke = (window as any).__TAURI_INTERNALS__.invoke;
        const blocks = Array.from({ length: 30 }, (_, b) => Array.from({ length: 40 }, (_, i) => `const record${b}_${i} = { id: ${i}, label: "synthetic text", ready: true };`).join('\n'));
        const markdown = blocks.map(code => '```javascript\n' + code + '\n```').join('\n\n');
        const now = '2026-09-05T00:00:00Z';
        const common = { conversationId: 'conv-turn-trace', toolCallId: null, toolCalls: [], thinking: null, tokenCount: 1, createdAt: now, imageAttachments: null };
        const messages = [{ ...common, id: 'plan-user', role: 'user', content: 'Create a synthetic plan', sortOrder: 0, artifacts: null }, { ...common, id: 'plan-answer', role: 'assistant', content: 'Synthetic plan summary', sortOrder: 1, artifacts: { kind: 'proposedPlan', title: 'Synthetic Large Plan', markdown } }];
        (window as any).auditPlan = { blocks, markdownChars: markdown.length, copied: null, longTasks: [] };
        new PerformanceObserver(list => { for (const e of list.getEntries())
            (window as any).auditPlan.longTasks.push(e.duration); }).observe({ type: 'longtask', buffered: false });
        Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (value) => { (window as any).auditPlan.copied = value; } }, configurable: true });
        (window as any).__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
            if (cmd === 'get_conversation_cmd') {
                const pair = await invoke(cmd, args);
                return [pair[0], messages];
            }
            if (cmd === 'get_conversation_turns_cmd')
                return [{ id: 'plan-turn', conversationId: 'conv-turn-trace', userMessageId: 'plan-user', assistantMessageId: 'plan-answer', status: 'success', createdAt: now, updatedAt: now, finishedAt: now, trace: null }];
            return invoke(cmd, args);
        };
    });
    await page.goto('/chat/conv-turn-trace');
    await page.getByText('Synthetic Large Plan', { exact: true }).waitFor({ timeout: 20000 });
    await page.waitForTimeout(120);
    const beforeCopy = await page.evaluate(() => ({ sourceChars: (window as any).auditPlan.markdownChars, plainBlocks: document.querySelectorAll('[data-code-presentation="plain"]').length, codeBlocks: document.querySelectorAll('pre code').length, allCodeTextMatches: [...document.querySelectorAll('pre code')].every((node, i) => node.textContent.replace(/\n$/, '') === (window as any).auditPlan.blocks[i]), domElements: document.body.querySelectorAll('*').length, initialLoadLongTasks: (window as any).auditPlan.longTasks }));
    await page.locator('pre[data-code-presentation="plain"]').first().locator('..').locator('button').click({ force: true });
    const copied = await page.evaluate(() => ({ copyMatchesFirstBlock: (window as any).auditPlan.copied === (window as any).auditPlan.blocks[0], copiedChars: (window as any).auditPlan.copied?.length }));
    expect(beforeCopy.plainBlocks).toBe(30);
    expect(beforeCopy.allCodeTextMatches).toBe(true);
    expect(beforeCopy.domElements).toBeLessThan(1500);
    expect(copied.copyMatchesFirstBlock).toBe(true);
    await page.reload();
    await expect(page.locator('pre[data-code-presentation="plain"]')).toHaveCount(30);
});
test('live updates do not revisit unrelated history through changing callbacks', async ({ page }) => {
    await page.addInitScript(() => {
        const internals = (window as any).__TAURI_INTERNALS__, invoke = internals.invoke, transform = internals.transformCallback;
        const listeners = new Map(), callbacks = new Map();
        let traceReads = 0;
        const now = '2026-09-05T00:00:00Z', messages = [], turns = [];
        const message = (id, role, content, sortOrder) => ({ id, conversationId: 'conv-turn-trace', role, content, sortOrder, toolCallId: null, toolCalls: [], artifacts: null, thinking: null, tokenCount: 1, createdAt: now, imageAttachments: null });
        for (let i = 0; i < 400; i++) {
            messages.push(message('audit-u' + i, 'user', 'Synthetic audit request ' + i, i * 2), message('audit-a' + i, 'assistant', 'Synthetic audit answer ' + i, i * 2 + 1));
            const trace = { kind: 'turnTrace', routeKind: 'InteractionOperation', items: [{ kind: 'tool', toolCall: { callId: 'audit-tool' + i, toolName: 'audit_lookup', arguments: '{"query":"synthetic"}', status: 'done', content: 'Synthetic result' } }] };
            const turn = { id: 'audit-t' + i, conversationId: 'conv-turn-trace', userMessageId: 'audit-u' + i, assistantMessageId: 'audit-a' + i, status: 'success', createdAt: now, updatedAt: now, finishedAt: now };
            Object.defineProperty(turn, 'trace', { enumerable: true, get() { traceReads++; return trace; } });
            turns.push(turn);
        }
        messages.push(message('audit-active-user', 'user', 'Synthetic current live request', 800));
        internals.transformCallback = cb => { const id = transform(cb); callbacks.set(id, cb); return id; };
        internals.invoke = async (cmd, args = {}) => {
            if (cmd === 'plugin:event|listen') {
                const id = await invoke(cmd, args);
                listeners.set(id, { event: args.event, handler: args.handler });
                return id;
            }
            if (cmd === 'plugin:event|unlisten') {
                listeners.delete(args.eventId);
                return invoke(cmd, args);
            }
            if (cmd === 'get_conversation_turns_cmd')
                return turns;
            if (cmd === 'get_agent_task_runs_cmd')
                return [];
            if (cmd === 'get_conversation_cmd') {
                const value = await invoke(cmd, args);
                return [value[0], messages];
            }
            return invoke(cmd, args);
        };
        (window as any).audit = { reset() { traceReads = 0; }, count() { return traceReads; }, emit(event) { for (const [id, l] of listeners)
                if (l.event === 'agent://run-event')
                    callbacks.get(l.handler)?.({ event: l.event, id, payload: { conversationId: 'conv-turn-trace', runEvent: event } }); } };
    });
    await page.goto('/chat/conv-turn-trace');
    await page.locator('[data-chat-virtual-list]').waitFor({ timeout: 20000 });
    await page.waitForTimeout(500);
    await page.evaluate(async () => {
        const { streamStore } = await import('/src/lib/streamStore.ts');
        streamStore.startStream('conv-turn-trace');
        streamStore.bindTurnHandle('conv-turn-trace', { conversationId: 'conv-turn-trace', runId: 'audit-live', turnId: 'audit-active', state: 'running' });
        (window as any).audit.emit({ version: 2, runId: 'audit-live', turnId: 'audit-active', eventSeq: 1, kind: 'status', phase: 'responding', label: 'Synthetic live run', status: 'running', visibility: 'user', persistence: 'durable', displayKind: 'status', importance: 'normal', payload: {}, createdAt: '2026-09-05T00:00:00Z' });
    });
    await page.waitForTimeout(300);
    await page.evaluate(() => (window as any).audit.reset());
    for (let i = 0; i < 6; i++) {
        await page.evaluate(i => (window as any).audit.emit({ version: 2, runId: 'audit-live', turnId: 'audit-active', eventSeq: i + 2, kind: 'outputDelta', phase: 'responding', label: 'Text', status: 'running', visibility: 'user', persistence: 'durable', displayKind: 'output', importance: 'normal', payload: { blockId: 'audit-answer', channel: 'answer', offset: i * 4, delta: 'tick' }, createdAt: '2026-09-05T00:00:00Z' }), i);
        await page.waitForTimeout(70);
    }
    const result = await page.evaluate(() => ({ scenario: 'actual ChatPage + StreamProvider + useChatSession + 400 persisted turns + six live events', tracePropertyReads: (window as any).audit.count(), virtualRows: document.querySelectorAll('[data-chat-virtual-row]').length, liveTextVisible: document.body.textContent.includes('ticktickticktickticktick'), domElements: document.body.querySelectorAll('*').length }));
    expect(result.tracePropertyReads).toBe(0);
    expect(result.liveTextVisible).toBe(true);
    expect(result.virtualRows).toBeLessThan(30);
});
