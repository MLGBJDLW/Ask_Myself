import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    const now = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    let callbackId = 1;
    let listenerId = 1;
    const callbacks = new Map<number, (event: unknown) => void>();
    const conversation = {
      id: 'conv-decision-tray',
      title: 'Durable decision',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      personaId: null,
      archivedAt: null,
      createdAt: now,
      updatedAt: now,
    };
    const questionArtifact = {
      kind: 'questionRequest',
      version: 2,
      interactionId: 'interaction-decision-1',
      callId: 'call-decision-1',
      status: 'pending',
      questions: [
        {
          id: 'strategy',
          header: 'Strategy',
          question: 'Which implementation strategy?',
          type: 'single_choice',
          options: [
            { label: 'Architectural refactor', description: 'Use the durable runtime seam.' },
            { label: 'Small patch', description: 'Change presentation only.' },
          ],
        },
        {
          id: 'compatibility',
          header: 'Compatibility',
          question: 'What compatibility constraint should be preserved?',
          type: 'short',
          placeholder: 'Describe the constraint',
        },
      ],
    };
    const messages: Array<Record<string, unknown>> = [
      {
        id: 'message-user-1',
        conversationId: conversation.id,
        role: 'user',
        content: 'Upgrade the interaction flow.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 4,
        createdAt: now,
        sortOrder: 0,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'message-tool-round-1',
        conversationId: conversation.id,
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{
          id: 'call-decision-1',
          name: 'request_user_input',
          arguments: JSON.stringify({ questions: questionArtifact.questions }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: now,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'message-tool-result-1',
        conversationId: conversation.id,
        role: 'tool',
        content: 'Waiting for the user.',
        toolCallId: 'call-decision-1',
        toolCalls: [],
        artifacts: questionArtifact,
        tokenCount: 0,
        createdAt: now,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
    ];
    const turn = {
      id: 'turn-decision-1',
      conversationId: conversation.id,
      userMessageId: 'message-user-1',
      assistantMessageId: null,
      status: 'awaiting_user_input',
      routeKind: 'Agentic',
      trace: {
        kind: 'turnTrace',
        routeKind: 'Agentic',
        items: [{
          kind: 'tool',
          toolCall: {
            callId: 'call-decision-1',
            toolName: 'request_user_input',
            arguments: JSON.stringify({ questions: questionArtifact.questions }),
            status: 'done',
            content: 'Waiting for the user.',
            isError: false,
            artifacts: questionArtifact,
          },
        }],
      },
      createdAt: now,
      updatedAt: now,
      finishedAt: null,
    };
    const taskRun = {
      id: 'run-decision-1',
      conversationId: conversation.id,
      turnId: turn.id,
      userMessageId: 'message-user-1',
      status: 'awaiting_user_input',
      phase: 'awaiting_user_input',
      title: 'Upgrade the interaction flow',
      routeKind: 'Agentic',
      summary: 'Waiting for user input',
      errorMessage: null,
      provider: 'open_ai',
      model: 'gpt-4.1',
      plan: null,
      artifacts: null,
      createdAt: now,
      updatedAt: now,
      startedAt: now,
      finishedAt: null,
    };
    const interaction = {
      schemaVersion: 1,
      interactionId: 'interaction-decision-1',
      conversationId: conversation.id,
      turnId: turn.id,
      toolCallId: 'call-decision-1',
      kind: 'user_input',
      title: 'Input required',
      description: null,
      questions: questionArtifact.questions,
      required: true,
      status: 'pending',
      riskPriority: 100,
      queueSequence: 1,
      createdAt: now,
      updatedAt: now,
      expiresAt: null,
      resumeToken: 'private-resume-token',
    };
    const agentConfig = {
      id: 'cfg-decision',
      name: 'Decision Config',
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
      createdAt: now,
      updatedAt: now,
    };

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'plugin:event|listen': return listenerId++;
        case 'plugin:event|unlisten': return null;
        case 'list_agent_configs_cmd': return [clone(agentConfig)];
        case 'get_model_context_window': return 1047576;
        case 'list_conversations_cmd': return [clone(conversation)];
        case 'list_archived_conversations_cmd': return [];
        case 'get_conversation_cmd': return [clone(conversation), clone(messages)];
        case 'get_conversation_turns_cmd': return [clone(turn)];
        case 'get_agent_task_runs_cmd': return [clone(taskRun)];
        case 'get_agent_task_run_events_cmd': return [];
        case 'list_interaction_requests_cmd':
          return ['pending', 'presented', 'partially_answered', 'submitted'].includes(interaction.status)
            ? [clone(interaction)]
            : [];
        case 'mark_interaction_presented_cmd':
          if (interaction.status === 'pending') interaction.status = 'presented';
          return clone(interaction);
        case 'mark_interaction_partially_answered_cmd':
          if (interaction.status === 'presented') interaction.status = 'partially_answered';
          return clone(interaction);
        case 'append_interaction_supplement_cmd': {
          const message = {
            id: `supplement-${messages.length}`,
            conversationId: conversation.id,
            role: 'user',
            content: String(args.content ?? ''),
            toolCallId: null,
            toolCalls: [],
            artifacts: { kind: 'interactionSupplement', version: 1, interactionId: interaction.interactionId },
            tokenCount: 2,
            createdAt: new Date().toISOString(),
            sortOrder: messages.length,
            thinking: null,
            imageAttachments: null,
          };
          messages.push(message);
          return clone(message);
        }
        case 'agent_chat_cmd': {
          const request = args.request as { message?: string; userArtifacts?: Record<string, unknown> } | undefined;
          const artifact = request?.userArtifacts ?? {};
          interaction.status = 'submitted';
          taskRun.status = 'queued';
          taskRun.phase = 'queued';
          messages.push({
            id: 'message-response-1',
            conversationId: conversation.id,
            role: 'user',
            content: request?.message ?? '',
            toolCallId: null,
            toolCalls: [],
            artifacts: clone(artifact),
            tokenCount: 5,
            createdAt: new Date().toISOString(),
            sortOrder: messages.length,
            thinking: null,
            imageAttachments: null,
          });
          return {
            sessionId: conversation.id,
            runId: taskRun.id,
            turnId: turn.id,
            state: 'starting',
          };
        }
        case 'agent_stop_cmd':
          interaction.status = 'cancelled';
          taskRun.status = 'cancelled';
          taskRun.phase = 'done';
          return null;
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_personas_cmd':
        case 'list_projects_cmd':
          return [];
        case 'get_app_config_cmd':
          return null;
        case 'get_index_stats':
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case 'get_privacy_config':
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case 'get_embedder_config_cmd':
          return { provider: 'tfidf', apiKey: '', apiBaseUrl: '', apiModel: '', localModel: '', modelPath: '', vectorDimensions: 384 };
        case 'get_ocr_config_cmd':
          return { enabled: false, minConfidence: 0.5, llmFallback: false, detectionLimit: 2048, useCls: false };
        case 'check_ocr_models_cmd': return false;
        case 'clear_answer_cache': return 0;
        default: return null;
      }
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      convertFileSrc: (path: string) => path,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => {},
    };
  });
});

test('restores a per-question draft, distinguishes supplements, and collapses after submit', async ({ page }) => {
  await page.goto('/chat/conv-decision-tray');

  const tray = page.getByTestId('decision-tray');
  await expect(tray).toBeVisible();
  await expect(tray).toContainText('Question 1 of 2');
  await tray.getByRole('radio', { name: /Architectural refactor/ }).click();
  await expect(tray).toContainText('Question 2 of 2');

  const constraint = tray.getByPlaceholder('Describe the constraint');
  await constraint.fill('Preserve legacy configuration');
  await page.reload();
  await expect(page.getByTestId('decision-tray')).toContainText('Question 2 of 2');
  await expect(page.getByPlaceholder('Describe the constraint')).toHaveValue('Preserve legacy configuration');

  const composer = page.getByTestId('chat-input-textarea');
  await composer.fill('Also retain the old import path');
  await composer.press('Enter');
  await expect(page.getByText('Also retain the old import path').last()).toBeVisible();
  await expect(page.getByTestId('decision-tray')).toBeVisible();

  await page.getByPlaceholder('Describe the constraint').press('Control+Enter');
  await expect(page.getByTestId('decision-tray-review')).toBeVisible();
  await page.getByTestId('decision-tray').getByRole('button', { name: 'Submit answers' }).click();
  await expect(page.getByTestId('decision-tray')).toBeHidden();

  const summary = page.getByTestId('question-request-summary');
  await expect(summary).toContainText('Agent asked 2 questions');
  await expect(summary).toContainText('Answered');
  await summary.click();
  await expect(summary).toContainText('Architectural refactor');
  await expect(summary).toContainText('Preserve legacy configuration');
});
