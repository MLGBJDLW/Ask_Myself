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
      'conv-persisted-artifact-trace': {
        id: 'conv-persisted-artifact-trace',
        title: 'Persisted Trace Artifact',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-persisted-artifact-trace': [
        {
          id: 'm-user-artifact',
          conversationId: 'conv-persisted-artifact-trace',
          role: 'user',
          content: 'Check the retry issue.',
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
          id: 'm-assistant-artifact',
          conversationId: 'conv-persisted-artifact-trace',
          role: 'assistant',
          content: 'Final answer from persisted trace artifacts.',
          toolCallId: null,
          toolCalls: [],
          artifacts: {
            kind: 'traceTimeline',
            version: 1,
            items: [
              { kind: 'thinking', text: 'Investigating retry behaviour from persisted artifacts.' },
              {
                kind: 'tool',
                toolCall: {
                  callId: 'tool-plan-1',
                  toolName: 'update_plan',
                  arguments: '{"steps":[{"step":"Draft fix","status":"in_progress"}]}',
                  status: 'done',
                  content: 'Plan updated',
                  isError: false,
                  artifacts: {
                    kind: 'plan',
                    title: 'Trace plan',
                    steps: [
                      { title: 'Draft fix', status: 'in_progress' },
                      { title: 'Verify fix', status: 'pending' },
                    ],
                    counts: { total: 2, completed: 0, inProgress: 1, pending: 1 },
                  },
                },
              },
              {
                kind: 'tool',
                toolCall: {
                  callId: 'tool-artifact-1',
                  toolName: 'search_knowledge_base',
                  arguments: '{"query":"retry"}',
                  status: 'done',
                  content: 'Found 2 retry notes.',
                  isError: false,
                  artifacts: null,
                },
              },
              {
                kind: 'tool',
                toolCall: {
                  callId: 'tool-create-file-1',
                  toolName: 'create_file',
                  arguments: '{"path":"H:/novel/chapter-1.md"}',
                  status: 'done',
                  content: 'Created H:/novel/chapter-1.md',
                  isError: false,
                  artifacts: {
                    kind: 'fileChangeSet',
                    source: 'create_file',
                    diffStats: {
                      kind: 'diffStats',
                      filesChanged: 1,
                      additions: 140,
                      deletions: 0,
                      hunks: 1,
                      operation: 'create',
                      paths: ['H:/novel/chapter-1.md'],
                    },
                  },
                },
              },
              { kind: 'status', text: 'Recovered from persisted trace data.', tone: 'success' },
            ],
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
    let callbackSeq = 1;
    let listenerSeq = 1;

    const defaultAgentConfig = {
      id: 'cfg-persisted-artifact-trace',
      name: 'Persisted Artifact Trace Config',
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
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
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

test('renders persisted trace artifacts as a single unified timeline', async ({ page }) => {
  await page.goto('/chat/conv-persisted-artifact-trace');
  const thinkingToggle = page.getByRole('button', { name: /Thinking completed/ });
  await expect(thinkingToggle).toHaveAttribute('aria-expanded', 'false');
  await thinkingToggle.click();

  await expect(page.getByText('Investigating retry behaviour from persisted artifacts.')).toBeVisible();
  await expect(page.getByRole('button', { name: /Search.*retry.*done/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /Create File.*chapter-1\.md.*\+140.*-0/ })).toBeVisible();
  await expect(page.getByText('Recovered from persisted trace data.')).toBeVisible();
  await expect(page.getByText('Final answer from persisted trace artifacts.')).toBeVisible();
  await expect(page.getByTestId('task-board-collapsed').getByText('Draft fix', { exact: true })).toBeVisible();

  const headerStats = page.getByTestId('tool-card-header-diff-stats').first();
  await expect(headerStats).toBeVisible();
  await expect(headerStats.getByTestId('tool-card-header-additions')).toHaveAttribute('data-value', '+140');
  await expect(headerStats.getByTestId('tool-card-header-deletions')).toHaveAttribute('data-value', '-0');
  await expect(headerStats.getByTestId('tool-card-header-additions')).toContainText('+140');
  await expect(headerStats.getByTestId('tool-card-header-deletions')).toContainText('-0');

  const chatLogText = await page.getByLabel('Chat messages').textContent();
  const text = chatLogText ?? '';
  const thinkingIndex = text.indexOf('Investigating retry behaviour from persisted artifacts.');
  const searchIndex = text.indexOf('"retry"');
  const createFileIndex = text.indexOf('Create File');
  expect(thinkingIndex).toBeGreaterThanOrEqual(0);
  expect(searchIndex).toBeGreaterThan(thinkingIndex);
  expect(createFileIndex).toBeGreaterThan(searchIndex);
  expect(text.indexOf('Recovered from persisted trace data.')).toBeGreaterThan(createFileIndex);
  expect(text.indexOf('Final answer from persisted trace artifacts.')).toBeGreaterThan(text.indexOf('Recovered from persisted trace data.'));
  expect(text).not.toContain('update_plan');
  expect(text).not.toContain('Draft fix');
});
