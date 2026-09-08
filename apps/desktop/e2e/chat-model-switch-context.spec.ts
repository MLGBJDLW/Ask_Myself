import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

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

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {
      'conv-model-switch': {
        id: 'conv-model-switch',
        title: 'Model Switch Context',
        provider: 'open_ai',
        model: 'tiny-context',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    let configs = [
      {
        id: 'cfg-tiny',
        name: 'Tiny Context',
        provider: 'open_ai',
        apiKey: '',
        baseUrl: null,
        model: 'tiny-context',
        temperature: 0.3,
        maxTokens: 4096,
        contextWindow: 16384,
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
      },
      {
        id: 'cfg-large',
        name: 'Large Context',
        provider: 'open_ai',
        apiKey: '',
        baseUrl: null,
        model: 'large-context',
        temperature: 0.3,
        maxTokens: 4096,
        contextWindow: 1000000,
        isDefault: false,
        reasoningEnabled: null,
        thinkingBudget: null,
        reasoningEffort: null,
        maxIterations: null,
        summarizationModel: null,
        summarizationProvider: null,
        subagentAllowedTools: null,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];
    configs.push({ ...configs[0], id: 'cfg-subscription', name: 'My Copilot plan', provider: 'github_copilot', model: 'unavailable-old-model', isDefault: false });
    configs.push({ ...configs[0], id: 'cfg-codex', name: 'My Codex plan', provider: 'openai_codex', model: 'unavailable-old-model', isDefault: false });
    configs.push({ ...configs[0], id: 'cfg-glm', name: 'GLM gateway', model: 'glm-4.7', isDefault: false });
    const savedAgentConfigInputs: Array<Record<string, unknown>> = [];
    (window as unknown as { __savedAgentConfigInputs?: Array<Record<string, unknown>> }).__savedAgentConfigInputs = savedAgentConfigInputs;

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null }).__lastAgentChatArgs = null;

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      if (cmd === 'agent_chat_cmd') args = (args.request as Record<string, unknown>) ?? {};
      switch (cmd) {
        case 'get_conversation_file_changes_cmd': {
          if (!localStorage.getItem('e2e-change-fixture')) return [];
          const revision = Number(localStorage.getItem('e2e-change-revision') ?? 1);
          return [{ turnId: 'turn-changes', revision, partial: true, additions: revision === 1 ? 11 : 2, deletions: revision === 1 ? 11 : 1, unknownFiles: 1,
            files: Array.from({ length: revision === 1 ? 12 : 2 }, (_, index) => ({ path: `src/file-${index}.txt`, absolutePath: `C:/workspace/src/file-${index}.txt`, operation: 'edit', additions: index === 11 ? null : 1, deletions: index === 11 ? null : 1, contentKind: index === 11 ? 'binary' : 'text', partial: false, revision })) }];
        }
        case 'get_turn_file_diff_cmd': {
          const calls = window as unknown as { __diffRequests?: string[] };
          (calls.__diffRequests ??= []).push(String(args.absolutePath));
          return { path: 'src/file-0.txt', absolutePath: 'C:/workspace/src/file-0.txt', operation: 'edit', additions: 1, deletions: 1, hunks: [{ oldStart: 1, newStart: 1, oldLines: 1, newLines: 1, lines: [{ type: 'deletion', oldLine: 1, newLine: null, content: 'before version' }, { type: 'addition', oldLine: null, newLine: 1, content: 'saved version' }] }] };
        }
        case 'list_font_assets_cmd':
          return JSON.parse(localStorage.getItem('e2e-font-assets') ?? '[]');
        case 'plugin:dialog|open':
          return ['/selected/font.woff2'];
        case 'import_font_assets_cmd': {
          const fonts = [{ id: 'font-custom', name: 'My imported font', family: 'ImportedNexa', format: 'woff2', path: '/test-user-font.woff2', bytes: 12000 }];
          localStorage.setItem('e2e-font-assets', JSON.stringify(fonts));
          return fonts;
        }
        case 'remove_font_asset_cmd':
          localStorage.setItem('e2e-font-assets', '[]');
          return null;
        case 'get_wizard_state_cmd':
          return { completed: true };
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
        case 'start_context_compaction_cmd':
          localStorage.setItem('e2e-compact-started', '1');
          throw new Error('Unexpected subscription compaction API request');
        case 'list_subscription_models_cmd': {
          const fixture = window as unknown as { __catalogCalls?: string[]; __catalogDelay?: number; __catalogError?: string };
          (fixture.__catalogCalls ??= []).push(String(args.provider));
          if (fixture.__catalogDelay) await new Promise(resolve => setTimeout(resolve, fixture.__catalogDelay));
          if (fixture.__catalogError) throw new Error(fixture.__catalogError);
          return [{ id: 'gpt-native', name: 'Native GPT', reasoningEfforts: ['low', 'ultra'] }];
        }
        case 'list_agent_configs_cmd':
          return configs.map(clone);
        case 'set_default_agent_config_cmd': {
          const id = String(args.id ?? '');
          configs = configs.map((config) => ({
            ...config,
            isDefault: config.id === id,
          }));
          return null;
        }
        case 'save_agent_config_cmd': {
          const input = clone((args.config ?? {}) as Record<string, unknown>);
          savedAgentConfigInputs.push(input);
          const id = String(input.id ?? '');
          const existing = configs.find((config) => config.id === id) ?? configs[0];
          const next = {
            ...existing,
            ...input,
            id: id || existing.id,
            createdAt: existing.createdAt,
            updatedAt: new Date().toISOString(),
          };
          configs = configs.map((config) =>
            config.id === next.id
              ? { ...next, isDefault: true }
              : { ...config, isDefault: false },
          );
          return clone(next);
        }
        case 'update_conversation_model_cmd': {
          const id = String(args.id ?? '');
          const conversation = conversations[id];
          if (!conversation) return null;
          conversation.provider = String(args.provider ?? conversation.provider);
          conversation.model = String(args.model ?? conversation.model);
          conversation.updatedAt = new Date().toISOString();
          return clone(conversation);
        }
        case 'get_model_context_window': {
          const model = String(args.model ?? '');
          return model === 'large-context' ? 1000000 : 16384;
        }
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'list_projects_cmd':
          return [];
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          if (localStorage.getItem('e2e-change-fixture')) return [clone(conversations[id]), ['user', 'assistant'].map((role, index) => ({
            id: `changes-${role}`, conversationId: id, role, content: role === 'user' ? 'Update the workspace files.' : 'The file edits are complete.', toolCalls: [], toolCallId: null, artifacts: null, tokenCount: 0, createdAt: nowIso, sortOrder: index, thinking: null, imageAttachments: null,
          }))];
          return [clone(conversations[id]), []];
        }
        case 'get_conversation_turns_cmd':
          if (localStorage.getItem('e2e-change-fixture')) return [{ id: 'turn-changes', conversationId: args.conversationId, userMessageId: 'changes-user', assistantMessageId: 'changes-assistant', status: 'completed', createdAt: nowIso, updatedAt: nowIso, finishedAt: nowIso }];
          return [];
        case 'get_conversation_usage_snapshot_cmd':
          return {
            source: 'provider',
            promptTokens: 10000,
            completionTokens: 250,
            totalTokens: 10250,
            thinkingTokens: 0,
            cacheReadTokens: 0,
            cacheMissTokens: 10000,
            cacheCreationTokens: 0,
            lastPromptTokens: 10000,
            providerRaw: {},
          };
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
        case 'agent_chat_cmd':
          (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null }).__lastAgentChatArgs = clone(args);
          return null;
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

test('turn file changes stay collapsed, load saved details on demand and survive reload', async ({ page }, testInfo) => {
  await page.addInitScript(() => localStorage.setItem('e2e-change-fixture', '1'));
  await page.goto('/chat/conv-model-switch');
  const capsule = page.getByTestId('turn-file-changes');
  await expect(capsule.getByRole('button', { name: /Changed 12 files/ })).toHaveAttribute('aria-expanded', 'false');
  await expect(capsule.getByTestId('turn-file-change-detail')).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __diffRequests?: string[] }).__diffRequests ?? [])).toEqual([]);
  await capsule.getByRole('button', { name: /Changed 12 files/ }).click();
  await expect(capsule.locator('li')).toHaveCount(10);
  await capsule.getByRole('button', { name: /Show more/ }).click();
  await expect(capsule.locator('li')).toHaveCount(12);
  await capsule.locator('li').first().getByRole('button', { name: /src\/file-0.txt/ }).click();
  await expect(capsule.getByText('saved version', { exact: true })).toBeVisible();
  await expect(capsule.getByText('before version', { exact: true })).toBeVisible();
  await expect(capsule.getByText('No line counts', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __diffRequests?: string[] }).__diffRequests)).toEqual(['C:/workspace/src/file-0.txt']);
  await capsule.screenshot({ path: testInfo.outputPath('turn-file-changes.png') });
  await page.reload();
  await expect(capsule.getByRole('button', { name: /Changed 12 files/ })).toHaveAttribute('aria-expanded', 'false');
  await page.evaluate(() => localStorage.setItem('e2e-change-revision', '2'));
  await page.reload();
  await expect(capsule.getByRole('button', { name: /Changed 2 files/ })).toBeVisible();
  await expect(capsule.getByText('+2', { exact: true })).toBeVisible();
  await expect(capsule.getByText('+11', { exact: true })).toHaveCount(0);
});

test('appearance fonts and streaming preferences apply, import, survive reload and remove', async ({ page }, testInfo) => {
  const font = readFileSync(join(process.cwd(), 'node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2'));
  await page.route('**/test-user-font.woff2', route => route.fulfill({ contentType: 'font/woff2', body: font }));
  await page.goto('/settings');
  const ui = page.getByTestId('ui-font-select');
  const code = page.getByTestId('code-font-select');
  await expect(ui.locator('option')).toHaveCount(17);
  await ui.selectOption('noto-sans-sc');
  await code.selectOption('jetbrains-mono');
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.body).fontFamily)).toContain('Noto Sans SC');
  await expect.poll(() => page.getByTestId('font-preview').locator('code').evaluate(el => getComputedStyle(el).fontFamily)).toContain('JetBrains Mono');
  await page.getByTestId('streaming-mode-smooth').click();
  await expect(page.getByTestId('streaming-mode-smooth')).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Import fonts', exact: true }).click();
  await expect(ui.locator('option')).toHaveCount(18);
  await ui.selectOption('font-custom');
  await expect.poll(() => page.evaluate(() => document.fonts.check('16px ImportedNexa') && getComputedStyle(document.body).fontFamily)).toContain('ImportedNexa');
  await page.reload();
  await expect(ui).toHaveValue('font-custom');
  await expect(code).toHaveValue('jetbrains-mono');
  await expect(page.getByTestId('streaming-mode-smooth')).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.body).fontFamily)).toContain('ImportedNexa');
  await page.getByTestId('display-settings').screenshot({ path: testInfo.outputPath('display-settings.png') });
  await page.getByRole('button', { name: 'Remove My imported font' }).click();
  await expect(ui).toHaveValue('theme');
  await expect(ui.locator('option')).toHaveCount(17);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.body).fontFamily)).not.toContain('ImportedNexa');
  await page.getByTestId('streaming-mode-chunked').click();
  await expect(page.getByTestId('streaming-mode-chunked')).toHaveAttribute('aria-pressed', 'true');
});

test('model search excludes provider metadata in global and provider results', async ({ page }) => {
  await page.goto('/chat/conv-model-switch');
  await page.getByTestId('agent-model-picker-trigger').click();
  const search = page.getByTestId('agent-model-picker-menu').getByRole('searchbox');
  await search.fill('GLM');
  await expect(page.getByTestId('agent-model-option-cfg-glm-glm-4.7')).toBeVisible();
  await expect(page.getByTestId('agent-model-option-cfg-glm-gpt-5.5')).toHaveCount(0);
  await search.press('Escape');
  await page.getByTestId('agent-model-picker-trigger').click();
  await search.fill('');
  await page.getByTestId('agent-model-provider-cfg-glm').click();
  await search.fill(' glm 4.7 ');
  await expect(page.getByTestId('agent-model-option-cfg-glm-glm-4.7')).toBeVisible();
  await expect(page.getByTestId('agent-model-option-cfg-glm-gpt-5.5')).toHaveCount(0);
});

test('model selector and context usage follow the active chat model', async ({ page }) => {
  await page.goto('/chat/conv-model-switch');

  await expect(page.getByRole('button', { name: /61% context used/ })).toBeVisible();
  const modelSelect = page.getByTestId('agent-model-picker-trigger');
  const reasoningSelect = page.getByTestId('agent-reasoning-picker-trigger');
  await expect(modelSelect).toContainText('Tiny Context');
  await expect(reasoningSelect).toBeVisible();

  await page.evaluate(() => document.fonts.ready);
  const pickerSeamGap = await page.evaluate(() => {
    const model = document.querySelector<HTMLElement>('[data-testid="agent-model-picker-trigger"]');
    const reasoning = document.querySelector<HTMLElement>('[data-testid="agent-reasoning-picker-trigger"]');
    if (!model || !reasoning) return null;
    const modelBox = model.getBoundingClientRect();
    const reasoningBox = reasoning.getBoundingClientRect();
    return Math.abs(modelBox.right - reasoningBox.left);
  });
  expect(pickerSeamGap).not.toBeNull();
  expect(pickerSeamGap ?? Number.POSITIVE_INFINITY).toBeLessThan(2);

  await modelSelect.click();
  await page.getByTestId('agent-model-provider-cfg-large').click();
  await page.getByTestId('agent-model-option-cfg-large-large-context').click();

  await expect(modelSelect).toHaveAttribute('title', 'open_ai / large-context');
  await expect(page.getByRole('button', { name: /1% context used/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /61% context used/ })).toHaveCount(0);

  await page.getByTestId('chat-input-textarea').fill('Use the selected model.');
  await page.getByTestId('chat-send').click();

  await expect.poll(async () =>
    page.evaluate(() =>
      (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null })
        .__lastAgentChatArgs?.agentConfigId,
    ),
  ).toBe('cfg-large');
});

test('model selector saves model and reasoning changes to the agent config', async ({ page }) => {
  await page.goto('/chat/conv-model-switch');

  const modelSelect = page.getByTestId('agent-model-picker-trigger');
  await modelSelect.click();
  await page.getByTestId('agent-model-provider-cfg-tiny').click();
  await page.getByTestId('agent-model-option-cfg-tiny-gpt-5.5').click();
  await expect(modelSelect).toHaveAttribute('title', 'open_ai / gpt-5.5');

  const reasoningSelect = page.getByTestId('agent-reasoning-picker-trigger');
  await reasoningSelect.click();
  await page.getByTestId('agent-model-reasoning-high').click();

  await expect(modelSelect).toHaveAttribute('title', 'open_ai / gpt-5.5');
  const savedInput = await page.evaluate(() =>
    (window as unknown as { __savedAgentConfigInputs?: Array<Record<string, unknown>> })
      .__savedAgentConfigInputs?.at(-1),
  );
  expect(savedInput).toMatchObject({
    id: 'cfg-tiny',
    model: 'gpt-5.5',
    reasoningEnabled: true,
    thinkingBudget: null,
    reasoningEffort: 'high',
  });
});


for (const [provider, configId] of [['github_copilot', 'cfg-subscription'], ['openai_codex', 'cfg-codex']]) {
test(`${provider} model and native reasoning selection reach the chat request`, async ({ page }) => {
  await page.goto('/chat/conv-model-switch');
  const picker = page.getByTestId('agent-model-picker-trigger');
  await picker.click();
  await page.getByTestId(`agent-model-provider-${configId}`).click();
  await expect(page.getByTestId(`agent-model-option-${configId}-unavailable-old-model`)).toHaveCount(0);
  await page.getByTestId(`agent-model-option-${configId}-gpt-native`).click();
  await expect(picker).toHaveAttribute('title', `${provider} / gpt-native`);
  await page.getByTestId('agent-reasoning-picker-trigger').click();
  await expect(page.getByTestId('agent-model-reasoning-none')).toHaveCount(0);
  await page.getByTestId('agent-model-reasoning-ultra').click();
  await page.locator('textarea').fill('Please inspect the current page');
  await page.locator('textarea').press('Enter');
  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastAgentChatArgs?: Record<string,unknown> }).__lastAgentChatArgs)).toMatchObject({ agentConfigId: configId });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __savedAgentConfigInputs?: Array<Record<string,unknown>> }).__savedAgentConfigInputs?.at(-1))).toMatchObject({ provider, model: 'gpt-native', reasoningEffort: 'ultra' });
});
}


test('subscription compact command is rejected without starting an API summarizer', async ({ page }) => {
  await page.goto('/chat/conv-model-switch');
  await page.getByTestId('agent-model-picker-trigger').click();
  await page.getByTestId('agent-model-provider-cfg-subscription').click();
  await page.getByTestId('agent-model-option-cfg-subscription-gpt-native').click();
  await page.locator('textarea').fill('/compact');
  await page.locator('textarea').press('Escape');
  await page.locator('textarea').press('Enter');
  await expect(page.getByText('Manual compaction is unavailable for this conversation.', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem('e2e-compact-started'))).toBeNull();
});

for (const [provider, configId] of [['github_copilot', 'cfg-subscription'], ['openai_codex', 'cfg-codex']]) {
  test(`${provider} shows catalog loading and reuses it for direct reasoning changes`, async ({ page }, testInfo) => {
    await page.addInitScript(() => { (window as unknown as { __catalogDelay: number }).__catalogDelay = 1800; });
    await page.goto('/chat/conv-model-switch');
    const picker = page.getByTestId('agent-model-picker-trigger');
    await picker.click();
    await page.getByTestId(`agent-model-provider-${configId}`).click();
    await expect(page.getByTestId('agent-model-catalog-status')).toContainText('Loading models');
    await page.getByTestId('agent-model-picker-menu').screenshot({ path: testInfo.outputPath('model-loading.png') });
    await page.getByTestId(`agent-model-option-${configId}-gpt-native`).click();
    await page.getByTestId('agent-reasoning-picker-trigger').click();
    await page.getByTestId('agent-model-picker-menu').screenshot({ path: testInfo.outputPath('native-reasoning.png') });
    await page.getByTestId('agent-model-reasoning-ultra').click();
    await page.getByTestId('agent-reasoning-picker-trigger').click();
    await page.getByTestId('agent-model-reasoning-low').click();
    const calls = await page.evaluate(() => (window as unknown as { __catalogCalls: string[] }).__catalogCalls);
    expect(calls.filter(entry => entry === provider)).toHaveLength(1);
  });
}

test('subscription catalog errors offer retry and retain cached models during refresh', async ({ page }) => {
  await page.addInitScript(() => { (window as unknown as { __catalogError: string }).__catalogError = 'offline'; });
  await page.goto('/chat/conv-model-switch');
  await page.getByTestId('agent-model-picker-trigger').click();
  await page.getByTestId('agent-model-provider-cfg-subscription').click();
  await expect(page.getByTestId('agent-model-catalog-status')).toContainText('Could not load models');
  await page.evaluate(() => { (window as unknown as { __catalogError: string }).__catalogError = ''; });
  await page.getByTestId('agent-model-catalog-refresh').click();
  const model = page.getByTestId('agent-model-option-cfg-subscription-gpt-native');
  await expect(model).toBeVisible();
  await page.evaluate(() => { (window as unknown as { __catalogError: string }).__catalogError = 'offline'; });
  await page.getByTestId('agent-model-catalog-refresh').click();
  await expect(page.getByTestId('agent-model-catalog-status')).toContainText('Using the previous model list');
  await expect(model).toBeEnabled();
});
