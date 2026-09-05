import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    const now = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const testParams = new URLSearchParams(window.location.search);
    let callbackId = 1;
    let listenerId = 1;
    let recoveryVisible = testParams.get('recovery') === '1';
    let failNextLaunch = testParams.get('failure') === '1';
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
    const requestKind = testParams.get('risk') === 'high'
      ? 'high_risk_confirmation'
      : 'user_input';
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
        {
          id: 'verification',
          header: 'Verification',
          question: 'Which verification depth should be used?',
          type: 'single_choice',
          options: [
            { label: 'Focused and full', description: 'Run focused checks and the full suite.' },
            { label: 'Focused only', description: 'Run only directly affected checks.' },
          ],
        },
        {
          id: 'delivery',
          header: 'Delivery',
          question: 'Which delivery constraint should be preserved?',
          type: 'short',
          placeholder: 'Describe the delivery constraint',
        },
      ].slice(0, testParams.get('single') === '1' ? 1 : testParams.get('long') === '1' ? 4 : 2),
    };
    const requestArguments = JSON.stringify({
      questions: questionArtifact.questions,
      kind: requestKind,
    });
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
          arguments: requestArguments,
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
            arguments: requestArguments,
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
      kind: requestKind,
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
    const savedAnswers = Object.fromEntries(questionArtifact.questions.map((question) => [
      question.id,
      [question.id === 'strategy' ? 'Architectural refactor' : 'Preserve legacy configuration'],
    ]));
    if (recoveryVisible) {
      interaction.status = 'submitted';
      taskRun.status = 'failed';
      taskRun.phase = 'done';
      turn.status = 'error';
      messages.push({
        id: 'message-recoverable-response',
        conversationId: conversation.id,
        role: 'user',
        content: 'Saved response',
        toolCallId: null,
        toolCalls: [],
        artifacts: {
          kind: 'questionResponse',
          version: 2,
          interactionId: interaction.interactionId,
          requestCallId: interaction.toolCallId,
          answers: questionArtifact.questions.map((question) => ({
            id: question.id,
            question: question.question,
            answers: savedAnswers[question.id],
          })),
        },
        tokenCount: 5,
        createdAt: now,
        sortOrder: messages.length,
        thinking: null,
        imageAttachments: null,
      });
    }
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
          return (
            ['pending', 'presented', 'partially_answered'].includes(interaction.status)
            || (recoveryVisible && ['submitted', 'acknowledged'].includes(interaction.status))
          )
            ? [clone(interaction)]
            : [];
        case 'get_interaction_response_cmd':
          return {
            schemaVersion: 1,
            interactionId: interaction.interactionId,
            answers: clone(savedAnswers),
            submittedAt: now,
          };
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
          if (failNextLaunch) {
            failNextLaunch = false;
            throw new Error('Injected continuation launch failure');
          }
          const request = args.request as { message?: string; userArtifacts?: Record<string, unknown> } | undefined;
          const artifact = request?.userArtifacts ?? {};
          recoveryVisible = false;
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
          messages.push({
            id: `message-follow-up-tool-${messages.length}`,
            conversationId: conversation.id,
            role: 'assistant',
            content: '',
            toolCallId: null,
            toolCalls: [{
              id: 'call-follow-up-1',
              name: 'run_shell',
              arguments: JSON.stringify({ command: 'verify' }),
            }],
            artifacts: null,
            tokenCount: 0,
            createdAt: new Date().toISOString(),
            sortOrder: messages.length,
            thinking: null,
            imageAttachments: null,
          });
          messages.push({
            id: `message-follow-up-result-${messages.length}`,
            conversationId: conversation.id,
            role: 'tool',
            content: 'Follow-up tool completed',
            toolCallId: 'call-follow-up-1',
            toolCalls: [],
            artifacts: null,
            tokenCount: 1,
            createdAt: new Date().toISOString(),
            sortOrder: messages.length,
            thinking: null,
            imageAttachments: null,
          });
          messages.push({
            id: `message-final-${messages.length}`,
            conversationId: conversation.id,
            role: 'assistant',
            content: 'Continuation completed.',
            toolCallId: null,
            toolCalls: [],
            artifacts: null,
            tokenCount: 2,
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
  await expect(tray).toHaveAttribute('data-theme-surface', 'panel');
  await expect(tray.locator('..')).toHaveAttribute('data-theme-surface', 'content');
  await expect(tray.locator('..')).toHaveAttribute('data-theme-blur-owner', 'false');
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
  const finalReply = page.getByText('Continuation completed.');
  await expect(finalReply).toBeVisible();
  await expect.poll(async () => summary.evaluate((node, finalNode) => (
    Boolean(node.compareDocumentPosition(finalNode as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
  ), await finalReply.elementHandle())).toBe(true);
});

test('an interrupted saved response can be retried without re-entering answers', async ({ page }) => {
  await page.goto('/chat/conv-decision-tray?recovery=1');

  const recovery = page.getByTestId('decision-tray-recovery');
  await expect(recovery).toContainText('Architectural refactor');
  await expect(recovery).toContainText('Preserve legacy configuration');
  await page.getByTestId('decision-tray').getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByTestId('decision-tray')).toBeHidden();
});

test('a rejected continuation keeps the decision tray retryable', async ({ page }) => {
  await page.goto('/chat/conv-decision-tray?single=1&failure=1');

  const tray = page.getByTestId('decision-tray');
  await tray.getByRole('radio', { name: /Architectural refactor/ }).click();
  const submit = tray.getByRole('button', { name: 'Submit answers' });
  await submit.click();
  await expect(tray).toBeVisible();
  await expect(submit).toBeEnabled();
});

test('single-choice questions advance to review before submission', async ({ page }) => {
  await page.goto('/chat/conv-decision-tray?single=1');

  const tray = page.getByTestId('decision-tray');
  await expect(tray).toContainText('Question 1 of 1');
  await tray.getByRole('radio', { name: /Architectural refactor/ }).click();

  await expect(page.getByTestId('decision-tray-review')).toBeVisible();
  await expect(tray.getByRole('button', { name: 'Submit answers' })).toBeVisible();
});

test('four-question requests remain a progressive wizard', async ({ page }) => {
  await page.goto('/chat/conv-decision-tray?long=1');

  const tray = page.getByTestId('decision-tray');
  await expect(tray).toContainText('Question 1 of 4');
  await tray.getByRole('radio', { name: /Architectural refactor/ }).click();
  await expect(tray).toContainText('Question 2 of 4');
  await expect(tray.getByPlaceholder('Describe the constraint')).toBeVisible();
});

test('high-risk requests block the chat in an accessible modal', async ({ page }) => {
  await page.addInitScript(() => {
    const plugin = {
      manifestVersion: 2,
      kind: 'theme-resource',
      id: 'decision-wallpaper',
      name: 'Decision Wallpaper',
      theme: {
        baseTheme: 'light',
        mode: 'light',
        colors: {
          surface0: 'rgba(246, 238, 232, 0.12)',
          surface1: 'rgba(255, 248, 242, 0.15)',
          textPrimary: '#251913',
          textSecondary: '#59443a',
          textTertiary: '#786056',
          accent: '#c85d2e',
        },
        effects: { surfaceOpacity: 0.4, glassBlur: 20 },
        typography: {},
        motion: {},
        brand: {},
        content: {},
        components: {},
        background: {
          kind: 'gradient',
          value: 'linear-gradient(135deg, #4f2418, #e09158)',
        },
      },
    };
    localStorage.setItem('nexa-theme-resource-plugins-v2', JSON.stringify([plugin]));
    localStorage.setItem('nexa-active-theme-v1', plugin.id);
  });
  await page.goto('/chat/conv-decision-tray?risk=high');

  const modal = page.getByRole('alertdialog', { name: 'A decision is needed' });
  await expect(modal).toBeVisible();
  await expect(modal).toHaveAttribute('aria-modal', 'true');
  await expect(modal.getByRole('heading', { name: 'Input required' })).toBeVisible();
  const modalBackdrop = page.getByTestId('decision-tray-modal-backdrop');
  await expect(modalBackdrop).toBeVisible();
  await expect(modal.locator('..')).toHaveAttribute('data-theme-blur-owner', 'false');
  const viewport = page.viewportSize();
  const chatViewportBounds = await page.locator("main").first().boundingBox();
  const backdropBounds = await modalBackdrop.boundingBox();
  const modalBounds = await modal.boundingBox();
  expect(viewport).not.toBeNull();
  expect(chatViewportBounds).not.toBeNull();
  expect(backdropBounds).not.toBeNull();
  expect(modalBounds).not.toBeNull();
  const edgeTolerance = 3;
  expect(backdropBounds!.x).toBeLessThanOrEqual(chatViewportBounds!.x + edgeTolerance);
  expect(backdropBounds!.y).toBeLessThanOrEqual(chatViewportBounds!.y + edgeTolerance);
  expect(backdropBounds!.x + backdropBounds!.width)
    .toBeGreaterThanOrEqual(chatViewportBounds!.x + chatViewportBounds!.width - edgeTolerance);
  expect(backdropBounds!.y + backdropBounds!.height)
    .toBeGreaterThanOrEqual(chatViewportBounds!.y + chatViewportBounds!.height - edgeTolerance);
  expect(Math.abs(
    modalBounds!.x + modalBounds!.width / 2
      - (chatViewportBounds!.x + chatViewportBounds!.width / 2),
  )).toBeLessThan(2);
  expect(Math.abs(
    modalBounds!.y + modalBounds!.height / 2
      - (chatViewportBounds!.y + chatViewportBounds!.height / 2),
  )).toBeLessThan(2);
  const modalBackgroundAlpha = await modal.evaluate((element) => {
    const color = getComputedStyle(element).backgroundColor;
    const parts = color.match(/^rgba?\((.+)\)$/i)?.[1]
      .split(/[\s,\/]+/)
      .filter(Boolean);
    return parts && parts.length >= 4 ? Number(parts[3]) : 1;
  });
  expect(modalBackgroundAlpha).toBe(1);
  const cdpSession = await page.context().newCDPSession(page);
  await cdpSession.send('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-reduced-transparency', value: 'reduce' }],
  });
  await expect(modalBackdrop).toHaveCSS('backdrop-filter', 'none');
  await cdpSession.send('Emulation.setEmulatedMedia', { features: [] });
  await cdpSession.detach();
  await expect.poll(() => modal.evaluate((element) => element.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Shift+Tab');
  await expect.poll(() => modal.evaluate((element) => element.contains(document.activeElement))).toBe(true);
});
