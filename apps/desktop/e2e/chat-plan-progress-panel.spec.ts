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
    const diffMergeConversation: Conversation = {
      id: 'conv-diff-merge',
      title: 'Diff merge',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const runShellDiffConversation: Conversation = {
      id: 'conv-run-shell-diffs',
      title: 'Run shell diffs',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const mixedPathDiffConversation: Conversation = {
      id: 'conv-mixed-path-diffs',
      title: 'Mixed path diffs',
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
      {
        id: 'm-user-followup',
        conversationId: conversation.id,
        role: 'user',
        content: 'Continue after the edit.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 4,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-followup',
        conversationId: conversation.id,
        role: 'assistant',
        content: 'Continuing with the next check.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 5,
        thinking: null,
        imageAttachments: null,
      },
    ];
    const diffMergeMessages: Message[] = [
      {
        id: 'm-user-diff-merge',
        conversationId: diffMergeConversation.id,
        role: 'user',
        content: 'Update example.ts in two edits.',
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
        id: 'm-assistant-diff-merge',
        conversationId: diffMergeConversation.id,
        role: 'assistant',
        content: 'Updated the file.',
        toolCallId: null,
        toolCalls: [
          {
            id: 'call-edit-answer',
            name: 'edit_file',
            arguments: JSON.stringify({
              path: 'src/example.ts',
              action: 'str_replace',
              old_str: 'const answer = 42;',
              new_str: 'const answer = 43;',
            }),
          },
          {
            id: 'call-edit-label',
            name: 'edit_file',
            arguments: JSON.stringify({
              path: 'src/example.ts',
              action: 'str_replace',
              old_str: 'export const label = "old";',
              new_str: 'export const label = "new";',
            }),
          },
        ],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-edit-answer',
        conversationId: diffMergeConversation.id,
        role: 'tool',
        content: 'Successfully replaced text in src/example.ts',
        toolCallId: 'call-edit-answer',
        toolCalls: [],
        artifacts: {
          kind: 'fileCheckpoint',
          checkpoint: {
            id: 'checkpoint-edit-answer',
            conversationId: diffMergeConversation.id,
            toolCallId: 'call-edit-answer',
            toolName: 'edit_file',
            operation: 'str_replace',
            path: 'src/example.ts',
            absolutePath: 'D:/workspace/src/example.ts',
            existedBefore: true,
            bytesBefore: 64,
            hashBefore: 'hash-before-answer',
            createdAt: nowIso,
          },
          bytesAfter: 64,
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
                { type: 'context', oldLine: 2, newLine: 2, content: 'export const label = "old";' },
              ],
            }],
          },
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-edit-label',
        conversationId: diffMergeConversation.id,
        role: 'tool',
        content: 'Successfully replaced text in src/example.ts',
        toolCallId: 'call-edit-label',
        toolCalls: [],
        artifacts: {
          kind: 'fileCheckpoint',
          checkpoint: {
            id: 'checkpoint-edit-label',
            conversationId: diffMergeConversation.id,
            toolCallId: 'call-edit-label',
            toolName: 'edit_file',
            operation: 'str_replace',
            path: 'src/example.ts',
            absolutePath: 'D:/workspace/src/example.ts',
            existedBefore: true,
            bytesBefore: 64,
            hashBefore: 'hash-before-label',
            createdAt: nowIso,
          },
          bytesAfter: 64,
          diff: {
            path: 'src/example.ts',
            operation: 'str_replace',
            additions: 1,
            deletions: 1,
            hunks: [{
              oldStart: 2,
              newStart: 2,
              oldLines: 1,
              newLines: 1,
              lines: [
                { type: 'deletion', oldLine: 2, newLine: null, content: 'export const label = "old";' },
                { type: 'addition', oldLine: null, newLine: 2, content: 'export const label = "new";' },
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
    const runShellDiffMessages: Message[] = [
      {
        id: 'm-user-run-shell-diffs',
        conversationId: runShellDiffConversation.id,
        role: 'user',
        content: 'Generate files with run_shell.',
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
        id: 'm-assistant-run-shell-diffs',
        conversationId: runShellDiffConversation.id,
        role: 'assistant',
        content: 'Generated the files.',
        toolCallId: null,
        toolCalls: [{
          id: 'call-run-shell-diffs',
          name: 'run_shell',
          arguments: JSON.stringify({
            program: 'python',
            args: ['-'],
            cwd: 'D:/workspace',
          }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-run-shell-diffs',
        conversationId: runShellDiffConversation.id,
        role: 'tool',
        content: 'Exit code: 0\n\n── file changes ──\nFile changes: 2 file(s), +2, -0, 2 text diff(s): generated/a.txt, generated/b.txt\n',
        toolCallId: 'call-run-shell-diffs',
        toolCalls: [],
        artifacts: {
          kind: 'fileChangeSet',
          source: 'run_shell',
          fileChanges: [
            { path: 'generated/a.txt', operation: 'create', textDiff: true },
            { path: 'generated/b.txt', operation: 'create', textDiff: true },
          ],
          diffStats: {
            kind: 'diffStats',
            filesChanged: 2,
            additions: 2,
            deletions: 0,
            hunks: 2,
            operation: 'run_shell',
            paths: ['generated/a.txt', 'generated/b.txt'],
          },
          diffs: [
            {
              path: 'generated/a.txt',
              operation: 'create',
              additions: 1,
              deletions: 0,
              hunks: [{
                oldStart: 0,
                newStart: 1,
                oldLines: 0,
                newLines: 1,
                lines: [
                  { type: 'addition', oldLine: null, newLine: 1, content: 'alpha' },
                ],
              }],
            },
            {
              path: 'generated/b.txt',
              operation: 'create',
              additions: 1,
              deletions: 0,
              hunks: [{
                oldStart: 0,
                newStart: 1,
                oldLines: 0,
                newLines: 1,
                lines: [
                  { type: 'addition', oldLine: null, newLine: 1, content: 'beta' },
                ],
              }],
            },
          ],
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
    ];
    const mixedPathDiffMessages: Message[] = [
      {
        id: 'm-user-mixed-path-diffs',
        conversationId: mixedPathDiffConversation.id,
        role: 'user',
        content: 'Edit the same file through mixed path artifacts.',
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
        id: 'm-assistant-mixed-path-diffs',
        conversationId: mixedPathDiffConversation.id,
        role: 'assistant',
        content: 'Updated the file twice.',
        toolCallId: null,
        toolCalls: [{
          id: 'call-mixed-path-diffs',
          name: 'run_shell',
          arguments: JSON.stringify({ program: 'python', args: ['-'], cwd: 'D:/workspace' }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-mixed-path-diffs',
        conversationId: mixedPathDiffConversation.id,
        role: 'tool',
        content: 'Exit code: 0\n\nFile changes: 1 file(s), +2, -2\n',
        toolCallId: 'call-mixed-path-diffs',
        toolCalls: [],
        artifacts: {
          kind: 'fileChangeSet',
          source: 'run_shell',
          fileChanges: [
            { path: 'src/example.ts', absolutePath: 'D:/workspace/src/example.ts', operation: 'modify', textDiff: true },
          ],
          diffs: [
            {
              path: 'src/example.ts',
              absolutePath: 'D:/workspace/src/example.ts',
              operation: 'run_shell',
              additions: 1,
              deletions: 1,
              hunks: [{
                oldStart: 1,
                newStart: 1,
                oldLines: 1,
                newLines: 1,
                lines: [
                  { type: 'deletion', oldLine: 1, newLine: null, content: 'const answer = 42;' },
                  { type: 'addition', oldLine: null, newLine: 1, content: 'const answer = 43;' },
                ],
              }],
            },
            {
              path: './src/example.ts',
              operation: 'run_shell',
              additions: 1,
              deletions: 1,
              hunks: [{
                oldStart: 2,
                newStart: 2,
                oldLines: 1,
                newLines: 1,
                lines: [
                  { type: 'deletion', oldLine: 2, newLine: null, content: 'export const label = "old";' },
                  { type: 'addition', oldLine: null, newLine: 2, content: 'export const label = "new";' },
                ],
              }],
            },
          ],
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
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
          return [
            clone(conversation),
            clone(autoPlanOnlyConversation),
            clone(diffMergeConversation),
            clone(runShellDiffConversation),
            clone(mixedPathDiffConversation),
          ];
        case 'get_conversation_cmd': {
          const conversationId = String(args.id ?? '');
          if (conversationId === autoPlanOnlyConversation.id) {
            return [clone(autoPlanOnlyConversation), []];
          }
          if (conversationId === diffMergeConversation.id) {
            return [clone(diffMergeConversation), clone(diffMergeMessages)];
          }
          if (conversationId === runShellDiffConversation.id) {
            return [clone(runShellDiffConversation), clone(runShellDiffMessages)];
          }
          if (conversationId === mixedPathDiffConversation.id) {
            return [clone(mixedPathDiffConversation), clone(mixedPathDiffMessages)];
          }
          return [clone(conversation), clone(messages)];
        }
        case 'get_conversation_turns_cmd':
          return [];
        case 'get_agent_task_runs_cmd': {
          const conversationId = String(args.conversationId ?? '');
          if (
            conversationId === diffMergeConversation.id ||
            conversationId === runShellDiffConversation.id ||
            conversationId === mixedPathDiffConversation.id
          ) {
            return [];
          }
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

test('floating plan capsule renders only the update_plan checklist', async ({ page }) => {
  await page.goto('/chat/conv-plan-progress');

  const board = page.getByTestId('task-board');
  const collapsed = board.getByTestId('task-board-collapsed');
  const expanded = board.getByTestId('task-board-expanded');
  await expect(board).toBeVisible();
  await expect(board).toHaveCSS('position', 'absolute');
  await expect(collapsed).toBeVisible();
  await expect(collapsed).toHaveAttribute('aria-expanded', 'false');
  await expect(expanded).toBeHidden();
  await expect(collapsed).toContainText('Plan');
  await expect(collapsed.getByTestId('task-board-progress')).toHaveText('1/3');
  await expect(collapsed).toContainText('Apply change');

  const stableWidth = (await board.boundingBox())?.width;
  const transitionProperties = await expanded.evaluate((element) =>
    getComputedStyle(element).transitionProperty,
  );
  expect(transitionProperties).toContain('transform');
  expect(transitionProperties).toContain('opacity');
  expect(transitionProperties).not.toContain('width');
  expect(transitionProperties).not.toContain('border-radius');

  await collapsed.click();
  await expect(expanded).toBeVisible();
  await expect(expanded.getByRole('button')).toHaveAttribute('aria-expanded', 'true');
  await expect(expanded).toContainText('Inspect context');
  await expect(expanded).toContainText('Apply change');
  await expect(expanded).toContainText('Verify result');
  expect((await board.boundingBox())?.width).toBe(stableWidth);

  await expect(expanded).not.toContainText('Work Tracker');
  await expect(expanded).not.toContainText('Progress');
  await expect(expanded).not.toContainText('Detailed execution plan');
  await expect(expanded).not.toContainText('Only the live checklist progress should be visible here.');
  await expect(expanded).not.toContainText('Subagent result that should not render');
  await expect(expanded).not.toContainText('Verification summary that should not render');

  await expanded.getByRole('button').click();
  await expect(collapsed).toBeVisible();
  await expect(collapsed).toHaveAttribute('aria-expanded', 'false');
  await expect(expanded).toBeHidden();
  expect((await board.boundingBox())?.width).toBe(stableWidth);
});

test('lower plan progress panel ignores automatic task run plans', async ({ page }) => {
  await page.goto('/chat/conv-auto-plan-only');

  await expect(page.getByTestId('task-board')).toHaveCount(0);
  await expect(page.getByText('Answer directly unless a tool is clearly needed for accuracy.')).toHaveCount(0);
});

test('pasted images use a compact thumbnail and open a large preview', async ({ page }) => {
  await page.goto('/chat/conv-plan-progress');

  await page.getByTestId('chat-input-textarea').evaluate((textarea) => {
    const event = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', {
      value: {
        files: [],
        items: [],
        getData: (type: string) => type === 'text/plain'
          ? 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
          : '',
      },
    });
    textarea.dispatchEvent(event);
  });

  const thumbnail = page.getByTestId('chat-attachment-thumbnail');
  await expect(thumbnail).toBeVisible();
  await expect(thumbnail).toHaveCSS('width', '40px');
  await expect(thumbnail).toHaveCSS('height', '40px');

  await thumbnail.click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByTestId('chat-attachment-preview')).toBeVisible();
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

test('file diff preview stays attached before later user messages', async ({ page }) => {
  await page.goto('/chat/conv-plan-progress');

  const log = page.getByRole('log');
  const diffCard = log.getByTestId('file-diff-preview').last();
  await expect(diffCard).toBeVisible();
  await expect(log.getByText('Continue after the edit.', { exact: true })).toBeVisible();

  const diffBeforeFollowup = await diffCard.evaluate((diff) => {
    const followup = Array.from(document.querySelectorAll('[role="log"] *'))
      .find((element) => element.textContent?.trim() === 'Continue after the edit.');
    return Boolean(
      followup &&
      (diff.compareDocumentPosition(followup) & Node.DOCUMENT_POSITION_FOLLOWING),
    );
  });

  expect(diffBeforeFollowup).toBe(true);
});

test('file diff previews merge repeated changes to the same file', async ({ page }) => {
  await page.goto('/chat/conv-diff-merge');

  const previewGroup = page.getByTestId('turn-file-diff-previews');
  await expect(previewGroup).toBeVisible();
  await expect(previewGroup.getByTestId('file-diff-preview')).toHaveCount(1);

  const diffCard = previewGroup.getByTestId('file-diff-preview').first();
  await expect(diffCard).toContainText('example.ts');
  await expect(diffCard.getByText('+2')).toBeVisible();
  await expect(diffCard.getByText('-2')).toBeVisible();

  await diffCard.getByRole('button').first().click();
  await expect(diffCard.getByText('const answer = 42;')).toBeVisible();
  await expect(diffCard.getByText('const answer = 43;')).toBeVisible();
  await expect(diffCard.getByText('export const label = "old";').first()).toBeVisible();
  await expect(diffCard.getByText('export const label = "new";')).toBeVisible();
});

test('file diff previews render run_shell diff arrays', async ({ page }) => {
  await page.goto('/chat/conv-run-shell-diffs');

  const previewGroup = page.getByTestId('turn-file-diff-previews');
  await expect(previewGroup).toBeVisible();
  const summaryPanel = previewGroup.getByTestId('file-diff-summary-panel');
  await expect(summaryPanel).toBeVisible();
  await expect(summaryPanel).toContainText('Edited 2 files');
  await expect(summaryPanel.getByText('+2')).toBeVisible();
  await expect(previewGroup.getByTestId('file-diff-preview')).toHaveCount(2);
  await expect(previewGroup).toContainText('a.txt');
  await expect(previewGroup).toContainText('b.txt');

  const summaryToggle = summaryPanel.getByRole('button').first();
  await expect(summaryToggle).toHaveAttribute('aria-expanded', 'true');
  await summaryToggle.click();
  await expect(summaryToggle).toHaveAttribute('aria-expanded', 'false');
  await expect(summaryPanel.getByTestId('file-diff-preview')).toHaveCount(0);
  await summaryToggle.click();

  const firstDiff = previewGroup.getByTestId('file-diff-preview').first();
  await expect(firstDiff.getByText('+1')).toBeVisible();
  await firstDiff.getByRole('button').first().click();
  await expect(firstDiff.getByText('alpha')).toBeVisible();
});

test('file diff summary merges mixed path aliases for the same file', async ({ page }) => {
  await page.goto('/chat/conv-mixed-path-diffs');

  const previewGroup = page.getByTestId('turn-file-diff-previews');
  await expect(previewGroup).toBeVisible();

  const summaryPanel = previewGroup.getByTestId('file-diff-summary-panel');
  await expect(summaryPanel).toContainText('Edited 1 files');
  await expect(summaryPanel.getByTestId('file-diff-preview')).toHaveCount(1);
  await expect(summaryPanel.getByText('+2').first()).toBeVisible();
  await expect(summaryPanel.getByText('-2').first()).toBeVisible();

  const diffCard = summaryPanel.getByTestId('file-diff-preview').first();
  await diffCard.getByRole('button').first().click();
  await expect(diffCard.getByText('const answer = 43;')).toBeVisible();
  await expect(diffCard.getByText('export const label = "new";')).toBeVisible();
});
