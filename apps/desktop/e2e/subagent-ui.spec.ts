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
      'conv-subagent': {
        id: 'conv-subagent',
        title: 'Subagent Demo',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      'conv-subagent-controls': {
        id: 'conv-subagent-controls',
        title: 'Subagent Controls',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-subagent': [],
      'conv-subagent-controls': [
        {
          id: 'm-controls-assistant',
          conversationId: 'conv-subagent-controls',
          role: 'assistant',
          content: '',
          toolCallId: null,
          toolCalls: [{
            id: 'subagent-call-controls',
            name: 'spawn_subagent',
            arguments: JSON.stringify({ task: 'Continue background research', model_policy: 'fast' }),
          }],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: 'Delegating a background worker.',
          imageAttachments: null,
        },
        {
          id: 'm-controls-tool',
          conversationId: 'conv-subagent-controls',
          role: 'tool',
          content: 'Subagent spawned.',
          toolCallId: 'subagent-call-controls',
          toolCalls: [],
          artifacts: {
            kind: 'subagent_result',
            id: 'agent-controls',
            status: 'running',
            task: 'Continue background research',
            result: '',
            toolEvents: [],
            lifecycleTools: {
              observe: 'observe_subagent',
              wait: 'wait_subagent',
              sendInput: 'send_subagent_input',
              cancel: 'cancel_subagent',
              close: 'close_subagent',
            },
          },
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 1,
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

    const defaultAgentConfig = {
      id: 'cfg-subagent',
      name: 'Subagent Config',
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

    const embedderConfig = {
      provider: 'tfidf',
      apiKey: '',
      apiBaseUrl: '',
      apiModel: '',
      localModel: '',
      modelPath: '',
      vectorDimensions: 384,
    };

    const ocrConfig = {
      enabled: false,
      minConfidence: 0.5,
      llmFallback: false,
      detectionLimit: 2048,
      useCls: false,
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
          return { completed: true };
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_capability_registry_projection_cmd':
          return {
            schemaVersion: 1,
            settingsRevisions: [],
            connections: [],
            modelDefinitions: [],
            modelTargets: [],
            capabilities: [{
              bindingId: 'binding:subagent',
              bindingRevision: 1,
              capabilityId: 'text_generation',
              source: { kind: 'agent', id: 'cfg-subagent' },
              sourceRevision: 1,
              primary: null,
              fallbacks: [],
              fallbackMode: 'disabled',
              constraints: {
                requireSameConnection: true,
                allowCrossProvider: false,
                allowCrossRegion: false,
                requiresStreaming: false,
                allowedRegions: [],
                dataClasses: [],
              },
            }],
            activations: [],
          };
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
          return clone(embedderConfig);
        case 'get_ocr_config_cmd':
          return clone(ocrConfig);
        case 'check_ocr_models_cmd':
          return false;
        case 'list_user_memories_cmd':
          return [];
        case 'list_skills_cmd':
          return [{
            id: 'skill-critic-format',
            name: 'Critic Format',
            description: 'Format delegated critique results with explicit risks.',
            content: 'Always return a compact critique with explicit risks.',
            enabled: true,
            createdAt: nowIso,
            updatedAt: nowIso,
          }];
        case 'list_personas_cmd':
          return [];
        case 'list_mcp_servers_cmd':
          return [{
            id: 'mcp-web',
            name: 'Web Search',
            transport: 'streamable_http',
            command: null,
            args: null,
            url: 'https://example.com/mcp',
            envJson: null,
            headersJson: null,
            enabled: true,
            createdAt: nowIso,
            updatedAt: nowIso,
            builtinId: null,
          }];
        case 'list_mcp_tools_cmd':
          if (String(args.serverId ?? '') === 'mcp-web') {
            return [{
              name: 'mcp__github__search_repos',
              description: 'Search GitHub repositories.',
              inputSchema: { type: 'object' },
            }];
          }
          return [];
          return 0;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const currentMessages = messagesByConversation[conversationId] ?? [];
          const userText = String(args.message ?? '');
          const keepRunning = /keep running/i.test(userText);
          const toolCallId = nextId('subagent-call');
          const toolArguments = JSON.stringify({
            task: 'Audit the last answer for risks',
            role: 'Critic',
            expected_output: 'Short risk report',
            acceptance_criteria: ['Identify at least one concrete risk or state that none were found.'],
            evidence_chunk_ids: ['chunk-retry-1'],
            source_ids: ['source-research'],
            allowed_tools: ['search_knowledge_base', 'web_search'],
            parallel_group: 'review-pass',
            deliverable_style: 'critique',
            return_sections: ['Conclusion', 'Evidence', 'Risks'],
            model_policy: 'independentReviewer',
          });
          const toolArtifact = {
            kind: 'subagent_result',
            status: 'done',
            task: 'Audit the last answer for risks',
            role: 'Critic',
            expectedOutput: 'Short risk report',
            acceptanceCriteria: ['Identify at least one concrete risk or state that none were found.'],
            evidenceChunkIds: ['chunk-retry-1'],
            evidenceHandoff: [
              {
                chunkId: 'chunk-retry-1',
                path: 'notes/retries.md',
                title: 'Retry notes',
                excerpt: 'Retries should stop after the configured threshold.',
              },
            ],
            requestedSourceScope: ['source-research'],
            effectiveSourceScope: ['source-research'],
            requestedAllowedTools: ['search_knowledge_base', 'web_search'],
            allowedSkills: [
              {
                id: 'skill-critic-format',
                name: 'Critic Format',
              },
            ],
            parallelGroup: 'review-pass',
            deliverableStyle: 'critique',
            returnSections: ['Conclusion', 'Evidence', 'Risks'],
            modelPolicy: 'independentReviewer',
            effectiveModel: 'private-model',
            modelRouteFallback: true,
            result: '1. Conclusion\\nThe proposed answer is acceptable.\\n\\n2. Key evidence or reasoning\\nThe referenced facts are consistent.\\n\\n3. Risks or open questions\\nDouble-check the edge case around retries.',
            finishReason: 'stop',
            usageTotal: {
              promptTokens: 1200,
              completionTokens: 240,
              totalTokens: 1440,
              thinkingTokens: 0,
            },
            toolEvents: [
              {
                phase: 'start',
                callId: 'inner-search',
                toolName: 'search_knowledge_base',
                arguments: '{\"query\":\"retry edge cases\"}',
              },
              {
                phase: 'result',
                callId: 'inner-search',
                toolName: 'search_knowledge_base',
                content: 'Found 2 relevant notes.',
                isError: false,
                artifacts: null,
              },
            ],
            thinking: ['Checked whether the answer missed operational risks.'],
            sourceScopeApplied: true,
            allowedTools: ['search_knowledge_base', 'web_search'],
            contextSnapshot: {
              id: 'snapshot-1',
              selectedMessageIds: ['m-prior'],
              tokenEstimate: 1800,
              contextCapacity: null,
              contextAuthority: 'provider_managed',
              handoffTokenBudget: 24000,
              droppedInvalidMessages: 1,
            },
            effectiveModelBudgets: {
              contextCapacity: null,
              parentHistoryHandoff: 24000,
              maxOutputPerStep: 8192,
              maxActualTokensPerWorker: 48000,
              contextAuthority: 'provider_managed',
              outputAuthority: 'safe_default',
            },
            preflight: {
              schemaVersion: 1,
              completedStages: ['history', 'provider', 'policy', 'budget', 'timeout'],
              providerId: 'Custom',
              effectiveModel: 'private-model',
              contextMessageCount: 3,
              droppedInvalidContextMessages: 1,
              reservedTokens: 12000,
              remainingTokenBudget: 48000,
              remainingCallBudget: 2,
              runDeadlineMs: 60000,
            },
          };

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
            toolCalls: [{ id: toolCallId, name: 'spawn_subagent', arguments: toolArguments }],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 1,
            thinking: 'Delegating an isolated critique pass.',
            imageAttachments: null,
          };
          const toolMessage: Message = {
            id: nextId('m-tool'),
            conversationId,
            role: 'tool',
            content: 'Subagent result (Critic):\\n1. Conclusion\\nThe proposed answer is acceptable.',
            toolCallId,
            toolCalls: [],
            artifacts: toolArtifact,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 2,
            thinking: null,
            imageAttachments: null,
          };
          const assistantFinalMessage: Message = {
            id: nextId('m-assistant-final'),
            conversationId,
            role: 'assistant',
            content: 'Supervisor synthesis complete.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: currentMessages.length + 3,
            thinking: null,
            imageAttachments: null,
          };

          messagesByConversation[conversationId] = keepRunning
            ? [...currentMessages, userMessage, assistantToolMessage]
            : [
                ...currentMessages,
                userMessage,
                assistantToolMessage,
                toolMessage,
                assistantFinalMessage,
              ];

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallStart',
              callId: toolCallId,
              toolName: 'spawn_subagent',
              arguments: toolArguments,
            });
          }, 20);

          if (keepRunning) return null;

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallResult',
              callId: toolCallId,
              toolName: 'spawn_subagent',
              content: toolMessage.content,
              isError: false,
              artifacts: toolArtifact,
            });
          }, 80);

          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'done',
              message: assistantFinalMessage,
              usageTotal: {
                promptTokens: 2000,
                completionTokens: 500,
                totalTokens: 2500,
                thinkingTokens: 0,
              },
              lastPromptTokens: 2000,
              finishReason: 'stop',
              cached: false,
            });
          }, 120);

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

test('uses a compact flowing border for a stable running subagent', async ({ page }) => {
  await page.goto('/chat/conv-subagent');

  await page.getByTestId('chat-input-textarea').fill('Keep running while I inspect the subagent card.');
  await page.getByTestId('chat-send').click();

  const chatLog = page.getByLabel('Chat messages');
  const toolCard = chatLog.getByTestId('tool-call-card').first();
  await expect(toolCard).toHaveAttribute('data-tool-state', 'running');
  await toolCard.click();

  const subagentShell = chatLog.getByTestId('subagent-card').first();
  const subagentTrigger = subagentShell.getByTestId('subagent-card-trigger');
  await expect(subagentShell).toHaveAttribute('data-tool-state', 'running');
  await expect(subagentShell).toHaveAttribute('aria-busy', 'true');
  await expect(subagentShell).not.toContainText(/Running/i);
  await expect(subagentShell.locator('.animate-spin')).toHaveCount(0);
  await expect.poll(async () => {
    const box = await subagentTrigger.boundingBox();
    return box?.height ?? 999;
  }).toBeLessThanOrEqual(48);
  await expect.poll(() => subagentShell.evaluate((element) =>
    getComputedStyle(element, '::after').animationName,
  )).toBe('chat-tool-card-border-flow');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect.poll(() => subagentShell.evaluate((element) =>
    getComputedStyle(element, '::after').animationName,
  )).toBe('none');
  await expect.poll(() => subagentShell.evaluate((element) =>
    getComputedStyle(element, '::after').opacity,
  )).not.toBe('0');
  await expect.poll(() => subagentShell.evaluate((element) =>
    getComputedStyle(element).backgroundImage,
  )).not.toBe('none');
});

test('shows subagent cards in chat and tool permissions in settings', async ({ page }) => {
  await page.goto('/chat/conv-subagent');

  await page.getByTestId('chat-input-textarea').fill('Please review the answer.');
  await page.getByTestId('chat-send').click();

  const thinkingToggle = page.getByRole('button', { name: /Thinking completed/ });
  if (await thinkingToggle.getAttribute('aria-expanded') !== 'true') {
    await thinkingToggle.click();
  }

  const chatLog = page.getByLabel('Chat messages');
  await chatLog.getByRole('button', { name: /Spawn Subagent/i }).click();
  const subagentCard = chatLog.getByRole('button', {
    name: /Critic\s+Complete\s+1 tools?\s+Audit the last answer for risks/i,
  }).first();
  await expect(subagentCard).toBeVisible();
  const subagentShell = subagentCard.locator('xpath=..');
  await expect(subagentShell).toHaveAttribute('data-testid', 'subagent-card');
  await expect(subagentShell).toHaveAttribute('data-tool-state', 'done');
  await expect(subagentShell).toHaveAttribute('aria-busy', 'false');
  await expect(subagentShell.locator('.animate-spin')).toHaveCount(0);

  await chatLog.getByText('Allowed tools').scrollIntoViewIfNeeded();
  await expect(chatLog.getByText('Allowed tools')).toBeVisible();
  await expect(chatLog.getByTitle('search_knowledge_base').first()).toBeVisible();
  await expect(chatLog.getByText('Allowed skills')).toBeVisible();
  await expect(chatLog.getByText('Critic Format')).toBeVisible();
  await expect(chatLog.getByText('Acceptance criteria')).toBeVisible();
  await expect(chatLog.getByText('Effective source scope')).toBeVisible();
  await expect(chatLog.getByText('Evidence handoff')).toBeVisible();
  await expect(chatLog.getByText('parallel: review-pass')).toBeVisible();
  await expect(chatLog.getByText('Inner trace')).toBeVisible();
  await expect(page.getByText('Supervisor synthesis complete.')).toBeVisible();

  await page.goto('/settings');
  await page.getByRole('button', { name: 'AI Providers' }).click();
  await page.getByRole('button', { name: /Advanced capability routing/ }).click();
  await expect(page.getByTestId('registry-capabilities')).toContainText('Agent Capabilities');
  await expect(page.getByTestId('registry-permissions-owner')).toContainText('Permissions');
  await expect(page.getByText(/subagents \d+/)).toHaveCount(0);
  await page.getByRole('button', { name: 'Add Provider' }).click();
  await page.getByRole('button', { name: 'Custom / Manual' }).click();
  await page.getByRole('button', { name: /Advanced Settings/ }).click();
  await expect(page.getByRole('heading', { name: 'Subagents' })).toBeVisible();
  await expect(page.getByText('Max parallel workers')).toBeVisible();
  await expect(page.getByText('Max worker calls / turn')).toBeVisible();
  await expect(page.getByText('Token budget / turn')).toBeVisible();
  await page.getByRole('button', { name: /^Research/ }).click();
  await page.getByRole('button', { name: /^Workflow Plans/ }).click();
  await page.getByRole('button', { name: /^Delegated skills/ }).click();
  await expect(page.getByText('Knowledge Search', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Record Verification', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Web Search', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Critic Format')).toBeVisible();
});

test('projects resolved context budgets and preflight into the subagent card', async ({ page }) => {
  await page.goto('/chat/conv-subagent');
  await page.getByTestId('chat-input-textarea').fill('Please review the context contract.');
  await page.getByTestId('chat-send').click();

  const thinkingToggle = page.getByRole('button', { name: /Thinking completed/ });
  if (await thinkingToggle.getAttribute('aria-expanded') !== 'true') {
    await thinkingToggle.click();
  }
  const chatLog = page.getByLabel('Chat messages');
  await chatLog.getByRole('button', { name: /Spawn Subagent/i }).click();
  const budgets = chatLog.getByTestId('subagent-model-budgets');
  await expect(budgets).toContainText('provider managed');
  await expect(budgets).toContainText('24,000');
  const route = chatLog.getByTestId('subagent-runtime-route');
  await expect(route).toContainText('Runtime provider: Custom');
  await expect(route).toContainText('Effective model: private-model');
  await expect(route).toContainText('Requested model route: independent reviewer');
  await expect(chatLog.getByTestId('subagent-preflight')).toContainText('Preflight passed 5 stages');
  await expect(chatLog.getByTestId('subagent-preflight')).toContainText('1 invalid context messages dropped');
  const preflightBudgets = chatLog.getByTestId('subagent-preflight-budgets');
  await expect(preflightBudgets).toContainText('Reserved estimate: 12,000');
  await expect(preflightBudgets).toContainText('Tokens remaining at preflight: 48,000');
  await expect(preflightBudgets).toContainText('Calls remaining at preflight: 2');
  await expect(preflightBudgets).toContainText('Run deadline: 1 min');
});

test('marks persisted lifecycle handles interrupted instead of presenting stale controls', async ({ page }) => {
  await page.goto('/chat/conv-subagent-controls');

  const thinkingToggle = page.getByRole('button', { name: /Thinking completed/ });
  if (await thinkingToggle.getAttribute('aria-expanded') !== 'true') {
    await thinkingToggle.click();
  }
  const chatLog = page.getByLabel('Chat messages');
  await chatLog.getByRole('button', { name: /Spawn Subagent/i }).click();
  await expect(chatLog.getByText('Interrupted by restart')).toBeVisible();
  await expect(chatLog.getByTestId('subagent-parent-controls')).toHaveCount(0);
});
