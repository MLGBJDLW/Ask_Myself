import { expect, test, type Locator } from '@playwright/test';

async function selectNexaOption(trigger: Locator, value: string) {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`).click();
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const testLocale = new URLSearchParams(window.location.search).get('locale') ?? 'en';
    localStorage.setItem('nexa-locale', testLocale);

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const agentChatCalls: Array<Record<string, unknown>> = [];

    const conversation = {
      id: 'conv-slash',
      title: 'Slash commands',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      personaId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const defaultAgentConfig = {
      id: 'cfg-slash',
      name: 'Slash Config',
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

    const frontendSkill = {
      id: 'builtin-frontend-design',
      name: 'frontend-design',
      description: 'Create distinctive production-grade frontend interfaces.',
      content: 'Use this skill when building web UI.',
      enabled: true,
      createdAt: nowIso,
      updatedAt: nowIso,
      builtin: true,
      interface: {
        displayName: 'Frontend Design',
        shortDescription: 'Design and implement refined UI.',
        defaultPrompt: 'Use frontend-design for this UI task.\n\nTask:\n{{input}}',
      },
      dependencies: { tools: [] },
      policy: { allowImplicitInvocation: true },
      sourcePath: null,
      resources: [],
    };
    const extraSkills = Array.from({ length: 20 }, (_, index) => ({
      ...frontendSkill,
      id: `builtin-extra-${index}`,
      name: `z-extra-${String(index).padStart(2, '0')}`,
      interface: {
        ...frontendSkill.interface,
        displayName: `Z Extra ${index}`,
      },
    }));

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

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
        case 'list_workflow_templates_cmd':
          return [];
        case 'list_builtin_skills_cmd':
          return [clone(frontendSkill), ...clone(extraSkills)];
        case 'list_skills_cmd':
          return [];
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), []];
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
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
        default:
          return null;
      }
    };

    (window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> }).__slashAgentChatCalls__ = agentChatCalls;
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

test('slash command menu can pin a skill for the next send', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  const textarea = page.getByTestId('chat-input-textarea');
  await textarea.fill('/front');

  const menu = page.getByTestId('slash-command-menu');
  await expect(menu).toBeVisible();
  await expect(menu).toContainText('/frontend-design');

  await page.keyboard.press('Enter');
  await expect(textarea).toHaveValue('');
  const capsule = page.getByTestId('active-slash-command');
  await expect(capsule).toBeVisible();
  await expect(capsule).toContainText('/frontend-design');

  await textarea.fill('build a dense dashboard');
  await page.getByTestId('chat-send').click();

  await expect.poll(
    () => page.evaluate(() =>
      (window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0]?.skillIds,
    ),
  ).toEqual(['builtin-frontend-design']);
  await expect.poll(
    () => page.evaluate(() =>
      String(((window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0]?.userArtifacts as Record<string, unknown> | undefined)
        ?.llmContextContent ?? ''),
    ),
  ).toContain('Use frontend-design for this UI task.');
  await expect.poll(
    () => page.evaluate(() =>
      String(((window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0]?.userArtifacts as Record<string, unknown> | undefined)
        ?.llmContextContent ?? ''),
    ),
  ).toContain('build a dense dashboard');
  await expect.poll(
    () => page.evaluate(() =>
      String((window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0]?.message ?? ''),
    ),
  ).toBe('build a dense dashboard');
});

test('an activated slash command can be cancelled without editing the prompt', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  const textarea = page.getByTestId('chat-input-textarea');
  await textarea.fill('/front');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('active-slash-command')).toBeVisible();

  await page.getByTestId('remove-active-slash-command').click();

  await expect(page.getByTestId('active-slash-command')).toHaveCount(0);
  await expect(textarea).toHaveValue('');
  await expect(textarea).toBeFocused();
});

test('slash command tabs filter the second-level option list', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  const textarea = page.getByTestId('chat-input-textarea');
  await textarea.fill('/');

  const menu = page.getByTestId('slash-command-menu');
  await expect(menu).toBeVisible();
  await expect(page.getByTestId('slash-command-tab-all')).toHaveAttribute('aria-selected', 'true');

  await page.getByTestId('slash-command-tab-skill').click();
  await expect(page.getByTestId('slash-command-tab-skill')).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('slash-command-option-frontend-design')).toBeVisible();
  await expect(page.getByTestId('slash-command-option-plan')).toHaveCount(0);

  await page.getByTestId('slash-command-tab-command').click();
  await expect(page.getByTestId('slash-command-option-plan')).toHaveCount(1);
  await expect(page.getByTestId('slash-command-option-frontend-design')).toHaveCount(0);
});

test('slash command menu uses the shared collision-aware overlay portal', async ({ page }) => {
  await page.setViewportSize({ width: 640, height: 520 });
  await page.goto('/chat/conv-slash');
  await page.getByTestId('chat-input-textarea').fill('/');
  await page.waitForTimeout(180);

  const bounds = await page.getByTestId('slash-command-menu').evaluate((menu) => {
    const rect = menu.getBoundingClientRect();
    const styles = getComputedStyle(menu);
    return {
      inOverlayRoot: Boolean(menu.closest('[data-nexa-overlay-root="true"]')),
      backdropFilter: styles.backdropFilter,
      animationName: styles.animationName,
      left: rect.left,
      right: rect.right,
      top: rect.top,
      bottom: rect.bottom,
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
    };
  });

  expect(bounds.inOverlayRoot).toBe(true);
  expect(bounds.backdropFilter).toMatch(/^(none|blur\(0px\))$/);
  expect(bounds.animationName).toContain('nexa-command-overlay');
  expect(bounds.left).toBeGreaterThanOrEqual(0);
  expect(bounds.top).toBeGreaterThanOrEqual(0);
  expect(bounds.right).toBeLessThanOrEqual(bounds.viewportWidth);
  expect(bounds.bottom).toBeLessThanOrEqual(bounds.viewportHeight);
});

test('slash command menu caps mounted rows and reports hidden matches', async ({ page }) => {
  await page.goto('/chat/conv-slash');
  await page.getByTestId('chat-input-textarea').fill('/');

  const list = page.getByTestId('slash-command-list');
  await expect(list.getByRole('option')).toHaveCount(16);
  await expect(page.getByTestId('slash-command-hidden-count')).toContainText('+');
});

test('slash command keyboard selection scrolls with the active row', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  const textarea = page.getByTestId('chat-input-textarea');
  await textarea.fill('/');

  const list = page.getByTestId('slash-command-list');
  await expect(list).toBeVisible();

  for (let i = 0; i < 13; i += 1) {
    await page.keyboard.press('ArrowDown');
  }

  await expect.poll(async () => page.evaluate(() => {
    const listEl = document.querySelector('[data-testid=\"slash-command-list\"]');
    const activeEl = document.querySelector('[data-testid=\"slash-command-list\"] [aria-selected=\"true\"]');
    if (!listEl || !activeEl) return false;
    const listRect = listEl.getBoundingClientRect();
    const activeRect = activeEl.getBoundingClientRect();
    return activeRect.top >= listRect.top - 1 && activeRect.bottom <= listRect.bottom + 1;
  })).toBe(true);
});

test('slash command menu uses localized chrome and built-in command labels', async ({ page }) => {
  await page.goto('/chat/conv-slash?locale=zh-CN');

  const textarea = page.getByTestId('chat-input-textarea');
  await textarea.fill('/plan');

  const menu = page.getByTestId('slash-command-menu');
  await expect(menu).toBeVisible();
  await expect(menu).toContainText('斜杠命令');
  await expect(menu).toContainText('规划');
  await expect(menu).toContainText('进入只读规划模式，生成可审批的实现计划。');
});

test('plan mode switch keeps its divider centered between labels', async ({ page }) => {
  await page.goto('/chat/conv-slash?locale=zh-CN');

  await expect(page.getByTestId('chat-mode-segment')).toBeVisible();

  const metrics = await page.evaluate(() => {
    const segment = document.querySelector('[data-testid="chat-mode-segment"]');
    const plan = document.querySelector('[data-testid="chat-plan-mode"]');
    const normal = document.querySelector('[data-testid="chat-normal-mode"]');
    const divider = document.querySelector('[data-testid="chat-mode-divider"]');
    if (!segment || !plan || !normal || !divider) {
      throw new Error('mode switch elements missing');
    }

    const segmentRect = segment.getBoundingClientRect();
    const planRect = plan.getBoundingClientRect();
    const normalRect = normal.getBoundingClientRect();
    const dividerRect = divider.getBoundingClientRect();

    return {
      segmentCenter: segmentRect.left + segmentRect.width / 2,
      buttonBoundary: planRect.right,
      normalLeft: normalRect.left,
      dividerCenter: dividerRect.left + dividerRect.width / 2,
    };
  });

  expect(Math.abs(metrics.buttonBoundary - metrics.normalLeft)).toBeLessThan(0.5);
  expect(Math.abs(metrics.dividerCenter - metrics.segmentCenter)).toBeLessThan(0.75);
  expect(Math.abs(metrics.dividerCenter - metrics.buttonBoundary)).toBeLessThan(1);
});

test('Nexus mode explains its cost, persists per conversation, and reaches the backend', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  const nexusSwitch = page.getByTestId('chat-nexus-mode');
  await expect(nexusSwitch).toHaveAttribute('aria-pressed', 'false');
  await nexusSwitch.click();

  const dialog = page.getByTestId('chat-nexus-dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'About Nexus mode' })).toHaveCSS('opacity', '1');
  await expect(dialog).toContainText('96K delegated tokens');
  await expect(dialog).toContainText('same blind spot');
  await page.getByTestId('chat-nexus-confirm').click();

  await expect(page.getByTestId('nexus-activation-effect')).toBeVisible();
  await expect(page.getByTestId('nexus-activation-effect')).toBeHidden();
  await expect(nexusSwitch).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('chat-nexus-mode-banner')).toContainText('25% verification reserve');

  await page.getByTestId('chat-input-textarea').fill('Review the cross-module change');
  await page.getByTestId('chat-send').click();
  await expect.poll(
    () => page.evaluate(() =>
      (window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0]?.powerMode,
    ),
  ).toBe('nexus');

  await page.reload();
  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'true');
  await page.getByTestId('chat-nexus-mode').click();
  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'false');

  await page.getByTestId('chat-nexus-mode').click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'true');
  await page.getByTestId('chat-nexus-mode-banner').getByRole('button', { name: 'Details' }).click();
  await expect(dialog).toBeVisible();
});

test('Nexus activation respects reduced-motion preferences', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/chat/conv-slash');

  await page.getByTestId('chat-nexus-mode').click();
  await page.getByTestId('chat-nexus-confirm').click();

  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('nexus-activation-effect')).toHaveCount(0);
});

test('the unified quality select supports Home End Enter and Escape', async ({ page }) => {
  await page.goto('/chat/conv-slash');
  const trigger = page.getByTestId('chat-quality-profile');

  await trigger.focus();
  await trigger.press('ArrowDown');
  await page.getByRole('option').last().press('End');
  await page.getByRole('option').last().press('Enter');
  await expect(trigger).toHaveAttribute('data-value', 'custom');

  await trigger.press('ArrowDown');
  await page.getByRole('option').first().press('Home');
  await page.getByRole('option').first().press('Escape');
  await expect(trigger).toBeFocused();
});

test('MoA and orchestration profiles remain independent from Nexus and reach the backend', async ({ page }) => {
  await page.goto('/chat/conv-slash');

  await selectNexaOption(page.getByTestId('chat-moa-preset'), 'crossModelCodeReview');
  await expect(page.getByTestId('chat-moa-mode-banner')).toContainText('Code Review');
  await expect(page.getByTestId('chat-moa-mode-banner')).toContainText('Independent from Nexus');

  await selectNexaOption(page.getByTestId('chat-quality-profile'), 'codeUltra');
  await expect(page.getByTestId('chat-quality-profile-banner')).toContainText('Code Ultra');
  await expect(page.getByTestId('chat-quality-profile-banner')).toContainText('provider reasoning stays separate');

  await page.getByTestId('chat-nexus-mode').click();
  await page.getByTestId('chat-nexus-confirm').click();
  await expect(page.getByTestId('chat-moa-mode-banner')).toContainText('Nexus + MoA');

  await page.getByTestId('chat-input-textarea').fill('Review and verify the implementation');
  await page.getByTestId('chat-send').click();
  await expect.poll(
    () => page.evaluate(() => {
      const request = (window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0];
      return [
        request?.powerMode,
        request?.collaborationMode,
        request?.moaPreset,
        request?.orchestrationProfile,
      ];
    }),
  ).toEqual(['nexus', 'mixtureOfAgents', 'crossModelCodeReview', 'codeUltra']);

  await page.reload();
  await expect(page.getByTestId('chat-moa-mode-banner')).toContainText('Nexus + MoA');
  await page.getByTestId('chat-nexus-mode').click();
  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByTestId('chat-moa-mode-banner')).toBeVisible();
});

test('Custom orchestration exposes bounded runtime controls', async ({ page }) => {
  await page.goto('/chat/conv-slash');
  await selectNexaOption(page.getByTestId('chat-quality-profile'), 'custom');
  await page.getByTestId('chat-quality-custom-maxIterations').fill('48');
  await page.getByTestId('chat-quality-custom-maxParallel').fill('8');
  await page.getByTestId('chat-quality-custom-maxCallsPerTurn').fill('10');
  await page.getByTestId('chat-quality-custom-delegatedTokenBudget').fill('96000');
  await page.getByTestId('chat-quality-custom-retryLimit').fill('3');
  await page.getByTestId('chat-quality-custom-minEvidenceSources').fill('4');
  await page.getByTestId('chat-quality-custom-verificationReservePercent').fill('40');
  await page.getByTestId('chat-input-textarea').fill('Run a custom verified workflow');
  await page.getByTestId('chat-send').click();

  await expect.poll(
    () => page.evaluate(() => {
      const request = (window as unknown as { __slashAgentChatCalls__: Array<Record<string, unknown>> })
        .__slashAgentChatCalls__[0];
      return request?.customOrchestration;
    }),
  ).toMatchObject({
    maxIterations: 48,
    maxParallel: 8,
    maxCallsPerTurn: 10,
    delegatedTokenBudget: 96000,
    retryLimit: 3,
    minEvidenceSources: 4,
    verificationReservePercent: 40,
  });
});
