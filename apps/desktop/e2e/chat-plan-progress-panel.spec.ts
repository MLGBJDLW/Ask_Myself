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
      collectionContext: null;
      projectId: null;
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

    const conversation: Conversation = {
      id: 'conv-plan-progress',
      title: 'Plan progress',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const autoPlanOnlyConversation: Conversation = {
      id: 'conv-auto-plan-only',
      title: 'Auto plan only',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const planToolCallId = 'call-update-plan';
    const updatePlanArtifact = {
      kind: 'plan',
      title: 'Detailed execution plan',
      explanation: 'Only the live checklist progress should be visible here.',
      steps: [
        { id: 'step-1', title: 'Inspect context', status: 'completed' },
        { id: 'step-2', title: 'Apply change', status: 'in_progress' },
        { id: 'step-3', title: 'Verify result', status: 'pending' },
      ],
    };
    const messages: Message[] = [
      {
        id: 'm-assistant-update-plan',
        conversationId: conversation.id,
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{ id: planToolCallId, name: 'update_plan', arguments: '{}' }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 0,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-update-plan',
        conversationId: conversation.id,
        role: 'tool',
        content: 'Plan updated',
        toolCallId: planToolCallId,
        toolCalls: [],
        artifacts: updatePlanArtifact,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-edit-file',
        conversationId: conversation.id,
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{
          id: 'call-edit-file',
          name: 'edit_file',
          arguments: JSON.stringify({
            path: 'src/example.ts',
            action: 'str_replace',
            old_str: 'const answer = 42;',
            new_str: 'const answer = 43;',
          }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-edit-file',
        conversationId: conversation.id,
        role: 'tool',
        content: 'Successfully replaced text in src/example.ts',
        toolCallId: 'call-edit-file',
        toolCalls: [],
        artifacts: {
          kind: 'fileCheckpoint',
          checkpoint: {
            id: 'checkpoint-edit-file',
            conversationId: conversation.id,
            toolCallId: 'call-edit-file',
            toolName: 'edit_file',
            operation: 'str_replace',
            path: 'src/example.ts',
            absolutePath: 'D:/workspace/src/example.ts',
            existedBefore: true,
            bytesBefore: 18,
            hashBefore: 'hash-before',
            createdAt: nowIso,
          },
          bytesAfter: 18,
          diff: {
            path: 'src/example.ts',
            operation: 'str_replace',
            additions: 1,
            deletions: 1,
            hunks: [{
              oldStart: 1,
              newStart: 1,
              oldLines: 2,
              newLines: 2,
              lines: [
                { type: 'deletion', oldLine: 1, newLine: null, content: 'const answer = 42;' },
                { type: 'addition', oldLine: null, newLine: 1, content: 'const answer = 43;' },
                { type: 'context', oldLine: 2, newLine: 2, content: 'export default answer;' },
              ],
            }],
          },
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 3,
        thinking: null,
        imageAttachments: null,
      },
    ];
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const defaultAgentConfig = {
      id: 'cfg-plan-progress',
      name: 'Plan Progress Config',
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

    const taskRun = {
      id: 'task-plan-progress',
      conversationId: conversation.id,
      turnId: 'turn-plan-progress',
      userMessageId: 'm-user-plan-progress',
      status: 'running',
      phase: 'tooling',
      title: 'Verbose task title that should not render in the lower panel',
      routeKind: 'FileOperation',
      summary: 'Task summary that belongs outside the lower plan panel',
      errorMessage: null,
      provider: 'open_ai',
      model: 'gpt-4.1',
      plan: {
        routeKind: 'DirectResponse',
        version: 1,
        steps: [
          {
            id: 'auto-step-1',
            title: 'Answer directly unless a tool is clearly needed for accuracy.',
            status: 'pending',
          },
        ],
      },
      artifacts: {
        subtasks: [
          {
            id: 'subtask-1',
            label: 'Subagent result that should not render',
            status: 'running',
          },
        ],
        verification: {
          kind: 'verification',
          summary: 'Verification summary that should not render',
          checks: [{ name: 'Hidden verification check', status: 'pending' }],
        },
      },
      createdAt: nowIso,
      updatedAt: nowIso,
      startedAt: nowIso,
      finishedAt: null,
    };
    const autoPlanOnlyTaskRun = {
      ...taskRun,
      id: 'task-auto-plan-only',
      conversationId: autoPlanOnlyConversation.id,
      title: 'Auto plan only task run',
      plan: {
        routeKind: 'DirectResponse',
        version: 1,
        steps: [
          {
            id: 'auto-step-1',
            title: 'Answer directly unless a tool is clearly needed for accuracy.',
            status: 'pending',
          },
        ],
      },
      artifacts: null,
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
          return [clone(conversation), clone(autoPlanOnlyConversation)];
        case 'get_conversation_cmd': {
          const conversationId = String(args.id ?? '');
          if (conversationId === autoPlanOnlyConversation.id) {
            return [clone(autoPlanOnlyConversation), []];
          }
          return [clone(conversation), clone(messages)];
        }
        case 'get_conversation_turns_cmd':
          return [];
        case 'get_agent_task_runs_cmd': {
          const conversationId = String(args.conversationId ?? '');
          return [
            clone(conversationId === autoPlanOnlyConversation.id ? autoPlanOnlyTaskRun : taskRun),
          ];
        }
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_projects_cmd':
        case 'list_personas_cmd':
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
        case 'compact_conversation_cmd':
        case 'save_agent_config_cmd':
        case 'agent_stop_cmd':
          return null;
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

test('lower plan progress panel renders only the update_plan checklist', async ({ page }) => {
  await page.goto('/chat/conv-plan-progress');

  const board = page.getByTestId('task-board');
  await expect(board).toBeVisible();
  await expect(board.getByTestId('task-board-progress')).toHaveText('1/3');
  await expect(board).toContainText('Apply change');
  await expect(board).not.toContainText('Verify result');

  await board.getByRole('button').click();
  await expect(board).toContainText('Inspect context');
  await expect(board).toContainText('Apply change');
  await expect(board).toContainText('Verify result');

  await expect(board).not.toContainText('Work Tracker');
  await expect(board).not.toContainText('Progress');
  await expect(board).not.toContainText('Plan');
  await expect(board).not.toContainText('Detailed execution plan');
  await expect(board).not.toContainText('Only the live checklist progress should be visible here.');
  await expect(board).not.toContainText('Subagent result that should not render');
  await expect(board).not.toContainText('Verification summary that should not render');
});

test('lower plan progress panel ignores automatic task run plans', async ({ page }) => {
  await page.goto('/chat/conv-auto-plan-only');

  await expect(page.getByTestId('task-board')).toHaveCount(0);
  await expect(page.getByText('Answer directly unless a tool is clearly needed for accuracy.')).toHaveCount(0);
});

test('edit_file tool result renders a structured diff preview', async ({ page }) => {
  await page.goto('/chat/conv-plan-progress');

  const diffCard = page.getByTestId('file-diff-preview').last();
  await expect(diffCard).toContainText('Modified');
  await expect(diffCard).toContainText('example.ts');
  await expect(diffCard.getByText('+1')).toBeVisible();
  await expect(diffCard.getByText('-1')).toBeVisible();
  await expect(diffCard.getByRole('button').first()).toHaveAttribute('aria-expanded', 'false');
  await expect(diffCard.getByText('const answer = 42;')).toHaveCount(0);

  await diffCard.getByRole('button').first().click();

  await expect(diffCard.getByRole('button').first()).toHaveAttribute('aria-expanded', 'true');
  await expect(diffCard.getByText('const answer = 42;')).toBeVisible();
  await expect(diffCard.getByText('const answer = 43;')).toBeVisible();
});
