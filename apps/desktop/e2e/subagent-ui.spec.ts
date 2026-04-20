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
      'conv-subagent': {
        id: 'conv-subagent',
        title: 'Subagent Demo',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-subagent': [],
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
            content: 'Always return a compact critique with explicit risks.',
            enabled: true,
            createdAt: nowIso,
            updatedAt: nowIso,
          }];
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
            builtinId: 'open-websearch',
          }];
        case 'list_mcp_tools_cmd':
          if (String(args.serverId ?? '') === 'mcp-web') {
            return [{
              name: 'mcp__web_search__search',
              description: 'Search the public web.',
              inputSchema: { type: 'object' },
            }];
          }
          return [];
        case 'clear_answer_cache':
          return 0;
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const currentMessages = messagesByConversation[conversationId] ?? [];
          const userText = String(args.message ?? '');
          const toolCallId = nextId('subagent-call');
          const toolArguments = JSON.stringify({
            task: 'Audit the last answer for risks',
            role: 'Critic',
            expected_output: 'Short risk report',
            acceptance_criteria: ['Identify at least one concrete risk or state that none were found.'],
            evidence_chunk_ids: ['chunk-retry-1'],
            source_ids: ['source-research'],
            allowed_tools: ['search_knowledge_base', 'mcp__web_search__search'],
            parallel_group: 'review-pass',
            deliverable_style: 'critique',
            return_sections: ['Conclusion', 'Evidence', 'Risks'],
          });
          const toolArtifact = {
            kind: 'subagent_result',
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
            requestedAllowedTools: ['search_knowledge_base', 'mcp__web_search__search'],
            allowedSkills: [
              {
                id: 'skill-critic-format',
                name: 'Critic Format',
              },
            ],
            parallelGroup: 'review-pass',
            deliverableStyle: 'critique',
            returnSections: ['Conclusion', 'Evidence', 'Risks'],
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
            allowedTools: ['search_knowledge_base', 'mcp__web_search__search'],
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

          messagesByConversation[conversationId] = [
            ...currentMessages,
            userMessage,
            assistantToolMessage,
            toolMessage,
            assistantFinalMessage,
          ];

          setTimeout(() => {
            emitEvent('agent:event', {
              conversationId,
              type: 'toolCallStart',
              callId: toolCallId,
              toolName: 'spawn_subagent',
              arguments: toolArguments,
            });
          }, 20);

          setTimeout(() => {
            emitEvent('agent:event', {
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
            emitEvent('agent:event', {
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

test('shows subagent cards in chat and tool permissions in settings', async ({ page }) => {
  await page.goto('/chat/conv-subagent');

  await page.getByTestId('chat-input-textarea').fill('Please review the answer.');
  await page.getByTestId('chat-send').click();

  await expect(page.getByText(/Subagents 1(?:\/1 active)?/)).toBeVisible();

  const subagentCard = page.getByRole('button', {
    name: /Critic\s+Complete\s+1 tool\s+Audit the last answer for risks/i,
  }).first();
  await expect(subagentCard).toBeVisible();
  await subagentCard.click();

  const chatLog = page.getByLabel('Chat messages');
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
  await expect(page.getByText('subagents 16')).toBeVisible();
  await page.getByRole('button', { name: 'Add Provider' }).click();
  await page.getByRole('button', { name: 'Custom / Manual' }).click();
  await page.getByRole('button', { name: 'Advanced Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Subagents' })).toBeVisible();
  await expect(page.getByText('Max parallel workers')).toBeVisible();
  await expect(page.getByText('Max worker calls / turn')).toBeVisible();
  await expect(page.getByText('Token budget / turn')).toBeVisible();
  await expect(page.getByText('Knowledge Search', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Record Verification', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('mcp__web_search__search').first()).toBeVisible();
  await expect(page.getByText('Critic Format')).toBeVisible();
});
