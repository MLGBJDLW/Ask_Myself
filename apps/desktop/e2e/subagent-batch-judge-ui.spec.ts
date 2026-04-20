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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {
      'conv-batch-judge': {
        id: 'conv-batch-judge',
        title: 'Batch Judge Demo',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const batchArtifact = {
      kind: 'subagent_batch_result',
      batchGoal: 'Parallel audit of three approaches',
      parallelGroup: 'approach-review',
      requestedMaxParallel: 3,
      effectiveMaxParallel: 2,
      completedRuns: 2,
      failedRuns: 0,
      budgetBefore: {
        maxParallel: 3,
        maxCallsPerTurn: 6,
        callsStarted: 0,
        remainingCalls: 6,
        tokenBudget: 12000,
        tokensSpent: 0,
        remainingTokens: 12000,
      },
      budgetAfter: {
        maxParallel: 3,
        maxCallsPerTurn: 6,
        callsStarted: 2,
        remainingCalls: 4,
        tokenBudget: 12000,
        tokensSpent: 1800,
        remainingTokens: 10200,
      },
      runs: [
        {
          id: 'worker-a',
          status: 'done',
          task: 'Audit approach A',
          role: 'Critic',
          expectedOutput: 'Short critique',
          acceptanceCriteria: ['Find the main risk.'],
          evidenceChunkIds: ['chunk-a'],
          evidenceHandoff: [
            {
              chunkId: 'chunk-a',
              path: 'notes/a.md',
              title: 'Approach A',
              excerpt: 'Approach A retries without a hard cap.',
            },
          ],
          requestedSourceScope: ['source-a'],
          effectiveSourceScope: ['source-a'],
          requestedAllowedTools: ['search_knowledge_base'],
          allowedTools: ['search_knowledge_base'],
          parallelGroup: 'approach-review',
          deliverableStyle: 'critique',
          returnSections: ['Conclusion', 'Risk'],
          result: 'Approach A needs a hard retry cap.',
          finishReason: 'stop',
          usageTotal: { promptTokens: 600, completionTokens: 120, totalTokens: 720, thinkingTokens: 0 },
          toolEvents: [],
          thinking: ['Checked for retry limit handling.'],
          sourceScopeApplied: true,
          isError: false,
          errorMessage: null,
        },
        {
          id: 'worker-b',
          status: 'done',
          task: 'Audit approach B',
          role: 'Verifier',
          expectedOutput: 'Short verification note',
          acceptanceCriteria: ['State whether the timeout guard exists.'],
          evidenceChunkIds: ['chunk-b'],
          evidenceHandoff: [
            {
              chunkId: 'chunk-b',
              path: 'notes/b.md',
              title: 'Approach B',
              excerpt: 'Approach B includes a timeout guard and capped retries.',
            },
          ],
          requestedSourceScope: ['source-b'],
          effectiveSourceScope: ['source-b'],
          requestedAllowedTools: ['search_knowledge_base', 'retrieve_evidence'],
          allowedTools: ['search_knowledge_base', 'retrieve_evidence'],
          parallelGroup: 'approach-review',
          deliverableStyle: 'verification',
          returnSections: ['Conclusion', 'Evidence'],
          result: 'Approach B already includes the required timeout guard.',
          finishReason: 'stop',
          usageTotal: { promptTokens: 700, completionTokens: 140, totalTokens: 840, thinkingTokens: 0 },
          toolEvents: [],
          thinking: ['Verified the guard is present.'],
          sourceScopeApplied: true,
          isError: false,
          errorMessage: null,
        },
      ],
    };

    const judgeArtifact = {
      kind: 'subagent_judgement',
      task: 'Choose the safer approach',
      rubric: ['Prefer capped retries.', 'Prefer explicit timeout guards.'],
      decisionMode: 'single_best',
      expectedOutput: 'Pick one winner.',
      parallelGroup: 'approach-review',
      winnerIds: ['worker-b'],
      confidence: 'high',
      summary: 'Approach B is the safer choice.',
      rationale: 'It includes both capped retries and an explicit timeout guard.',
      rawResponse: '{"winnerIds":["worker-b"]}',
      candidates: [
        { id: 'worker-a', label: 'Approach A', result: 'Approach A needs a hard retry cap.' },
        { id: 'worker-b', label: 'Approach B', result: 'Approach B already includes the required timeout guard.' },
      ],
      usageTotal: { promptTokens: 300, completionTokens: 90, totalTokens: 390, thinkingTokens: 0 },
      budget: {
        maxParallel: 3,
        maxCallsPerTurn: 6,
        callsStarted: 3,
        remainingCalls: 3,
        tokenBudget: 12000,
        tokensSpent: 2190,
        remainingTokens: 9810,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-batch-judge': [
        {
          id: 'm-user',
          conversationId: 'conv-batch-judge',
          role: 'user',
          content: 'Compare two approaches and pick the safer one.',
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
          id: 'm-assistant-batch',
          conversationId: 'conv-batch-judge',
          role: 'assistant',
          content: '',
          toolCallId: null,
          toolCalls: [{ id: 'call-batch', name: 'spawn_subagent_batch', arguments: '{"tasks":[]}' }],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 1,
          thinking: 'Running parallel delegated audits.',
          imageAttachments: null,
        },
        {
          id: 'm-tool-batch',
          conversationId: 'conv-batch-judge',
          role: 'tool',
          content: 'Batch delegated result.',
          toolCallId: 'call-batch',
          toolCalls: [],
          artifacts: batchArtifact,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 2,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-assistant-judge',
          conversationId: 'conv-batch-judge',
          role: 'assistant',
          content: '',
          toolCallId: null,
          toolCalls: [{ id: 'call-judge', name: 'judge_subagent_results', arguments: '{"candidates":[]}' }],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 3,
          thinking: 'Adjudicating the delegated results.',
          imageAttachments: null,
        },
        {
          id: 'm-tool-judge',
          conversationId: 'conv-batch-judge',
          role: 'tool',
          content: 'Approach B is the safer choice.',
          toolCallId: 'call-judge',
          toolCalls: [],
          artifacts: judgeArtifact,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 4,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-assistant-final',
          conversationId: 'conv-batch-judge',
          role: 'assistant',
          content: 'Final answer: choose approach B.',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 5,
          thinking: null,
          imageAttachments: null,
        },
      ],
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const defaultAgentConfig = {
      id: 'cfg-batch-judge',
      name: 'Batch Judge Config',
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

test('renders batch subagent and judgement artifacts', async ({ page }) => {
  await page.goto('/chat/conv-batch-judge');

  const chatLog = page.getByLabel('Chat messages');
  await expect(chatLog.getByText('Parallel audit of three approaches')).toBeVisible();
  await expect(chatLog.getByText('parallel 2')).toBeVisible();
  const firstBatchRun = chatLog.getByRole('button', { name: /Critic\s+Complete\s+Audit approach A/i }).first();
  await expect(firstBatchRun).toBeVisible();
  await firstBatchRun.click();
  await expect(chatLog.getByText('Evidence handoff')).toBeVisible();

  await expect(chatLog.getByText('Choose the safer approach')).toBeVisible();
  await expect(chatLog.getByText('single_best')).toBeVisible();
  await expect(chatLog.getByText('Approach B is the safer choice.')).toBeVisible();
  await expect(chatLog.getByText('winners worker-b')).toBeVisible();
  await expect(chatLog.getByText('Rubric')).toBeVisible();
});
