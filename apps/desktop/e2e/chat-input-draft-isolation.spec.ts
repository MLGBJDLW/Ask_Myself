import { expect, type Locator, test } from '@playwright/test';

async function paintedSurfaceStyle(locator: Locator, includeSelf = true) {
  return locator.evaluate((target, includeTarget) => {
    const alphaOf = (color: string): number => {
      const normalized = color.trim();
      if (normalized === 'transparent') return 0;
      const rgb = normalized.match(/^rgba?\((.+)\)$/i);
      if (rgb) {
        const parts = rgb[1].split(/[\s,\/]+/).filter(Boolean);
        if (parts.length < 4) return 1;
        const alpha = parts[3];
        return alpha.endsWith('%') ? Number(alpha.slice(0, -1)) / 100 : Number(alpha);
      }
      const functionalAlpha = normalized.match(/\/\s*([0-9.]+)(%)?\s*\)$/);
      if (!functionalAlpha) return 1;
      const alpha = Number(functionalAlpha[1]);
      return functionalAlpha[2] ? alpha / 100 : alpha;
    };
    let element: Element | null = includeTarget ? target : target.parentElement;
    while (element) {
      const style = getComputedStyle(element);
      const backgroundAlpha = alphaOf(style.backgroundColor);
      const backdropFilter = style.backdropFilter
        || style.getPropertyValue('-webkit-backdrop-filter')
        || 'none';
      if (backgroundAlpha > 0 || backdropFilter !== 'none') {
        return {
          surface: element.getAttribute('data-theme-surface'),
          backgroundAlpha: Number(backgroundAlpha.toFixed(2)),
          backdropFilter,
        };
      }
      element = element.parentElement;
    }
    throw new Error('No painted surface found');
  }, includeSelf);
}

async function backdropFilterOwners(locator: Locator) {
  return locator.evaluate((target) => {
    const owners: Array<{ surface: string | null; filter: string }> = [];
    let element: Element | null = target;
    while (element) {
      const style = getComputedStyle(element);
      const filter = style.backdropFilter
        || style.getPropertyValue('-webkit-backdrop-filter')
        || 'none';
      if (filter !== 'none') {
        owners.push({
          surface: element.getAttribute('data-theme-surface'),
          filter,
        });
      }
      element = element.parentElement;
    }
    return owners;
  });
}

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
      'conv-draft-a': {
        id: 'conv-draft-a',
        title: 'Draft A',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      'conv-draft-b': {
        id: 'conv-draft-b',
        title: 'Draft B',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-draft-a': [
        {
          id: 'm-draft-user-1',
          conversationId: 'conv-draft-a',
          role: 'user',
          content: 'first saved prompt',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 4,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-draft-assistant-1',
          conversationId: 'conv-draft-a',
          role: 'assistant',
          content: [
            'first saved answer',
            '',
            '```text',
            'theme-surface-local-scroll-contract-0123456789abcdefghijklmnopqrstuvwxyz-0123456789abcdefghijklmnopqrstuvwxyz-0123456789abcdefghijklmnopqrstuvwxyz-0123456789abcdefghijklmnopqrstuvwxyz',
            '```',
          ].join('\n'),
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 4,
          createdAt: nowIso,
          sortOrder: 1,
          thinking: null,
          imageAttachments: null,
        },
        {
          id: 'm-draft-user-2',
          conversationId: 'conv-draft-a',
          role: 'user',
          content: 'second saved prompt',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 4,
          createdAt: nowIso,
          sortOrder: 2,
          thinking: null,
          imageAttachments: null,
        },
      ],
      'conv-draft-b': [],
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const defaultAgentConfig = {
      id: 'cfg-draft-isolation',
      name: 'Draft Isolation Config',
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

test('keeps input drafts scoped to each conversation', async ({ page }) => {
  await page.goto('/chat/conv-draft-a');

  const input = page.getByTestId('chat-input-textarea');
  await input.fill('draft for A');

  await page.getByRole('button', { name: /Draft B/ }).click();
  await expect(input).toHaveValue('');

  await input.fill('draft for B');

  await page.getByRole('button', { name: /Draft A/ }).click();
  await expect(input).toHaveValue('draft for A');

  await page.getByRole('button', { name: /Draft B/ }).click();
  await expect(input).toHaveValue('draft for B');
});

test('keeps an input draft after leaving and returning to chat', async ({ page }) => {
  await page.goto('/chat/conv-draft-a');

  const input = page.getByTestId('chat-input-textarea');
  await input.fill('draft survives page switch');

  await page.getByRole('link', { name: 'Search' }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.getByRole('link', { name: 'Chat' }).click();
  await expect(page).toHaveURL(/\/chat$/);

  await page.getByRole('button', { name: /Draft A/ }).click();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('draft survives page switch');
});

test('centers only the composer for an empty chat and moves it to the bottom after the first send', async ({ page }) => {
  await page.goto('/chat/conv-draft-b');

  const composer = page.getByTestId('chat-input');
  const textarea = page.getByTestId('chat-input-textarea');
  await expect(composer).toHaveAttribute('data-placement', 'center');
  await expect(page.getByRole('button', { name: 'Search Knowledge', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Summarize', exact: true })).toHaveCount(0);

  await page.waitForTimeout(500);
  const centeredBounds = await composer.boundingBox();
  expect(centeredBounds).not.toBeNull();
  await page.screenshot({ path: 'test-results/new-chat-centered-composer.png', fullPage: true });
  await composer.evaluate((element) => {
    const targetWindow = window as typeof window & { __composerMotionSamples?: number[] };
    targetWindow.__composerMotionSamples = [];
    const startedAt = performance.now();
    const sample = () => {
      targetWindow.__composerMotionSamples?.push(element.getBoundingClientRect().y);
      if (performance.now() - startedAt < 800) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });

  await textarea.fill('Start from the centered composer.');
  await textarea.press('Enter');

  await expect(composer).toHaveAttribute('data-placement', 'bottom');
  await page.waitForTimeout(500);
  const bottomBounds = await composer.boundingBox();
  expect(bottomBounds).not.toBeNull();
  expect(bottomBounds!.y).toBeGreaterThan(centeredBounds!.y + 80);
  const motionSamples = await page.evaluate(() => (
    (window as typeof window & { __composerMotionSamples?: number[] }).__composerMotionSamples ?? []
  ));
  const distinctPositions = new Set(motionSamples.map((value) => Math.round(value)));
  expect(distinctPositions.size).toBeGreaterThan(3);
  await page.screenshot({ path: 'test-results/new-chat-bottom-composer.png', fullPage: true });
});

test('moves the empty-chat composer without travel animation when reduced motion is requested', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/chat/conv-draft-b');

  const composer = page.getByTestId('chat-input');
  const textarea = page.getByTestId('chat-input-textarea');
  await expect(composer).toHaveAttribute('data-placement', 'center');
  await page.waitForTimeout(100);
  await composer.evaluate((element) => {
    const targetWindow = window as typeof window & { __reducedComposerMotionSamples?: number[] };
    targetWindow.__reducedComposerMotionSamples = [];
    const startedAt = performance.now();
    const sample = () => {
      targetWindow.__reducedComposerMotionSamples?.push(element.getBoundingClientRect().y);
      if (performance.now() - startedAt < 250) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });

  await textarea.fill('Move without animation.');
  await textarea.press('Enter');
  await expect(composer).toHaveAttribute('data-placement', 'bottom');
  await page.waitForTimeout(300);

  const motionSamples = await page.evaluate(() => (
    (window as typeof window & { __reducedComposerMotionSamples?: number[] }).__reducedComposerMotionSamples ?? []
  ));
  const distinctPositions = new Set(motionSamples.map((value) => Math.round(value)));
  expect(distinctPositions.size).toBeLessThanOrEqual(2);
});

test('routes one custom wallpaper chrome surface through the active chat sidebar, toolbar, and composer', async ({ page }) => {
  await page.addInitScript(() => {
    const plugin = {
      manifestVersion: 2,
      kind: 'theme-resource',
      id: 'chat-chrome-contract',
      name: 'Chat Chrome Contract',
      theme: {
        baseTheme: 'light',
        mode: 'light',
        colors: {
          surface0: 'rgba(246, 238, 232, 0.12)',
          surface1: 'rgba(255, 248, 242, 0.15)',
          surface2: 'rgba(244, 222, 210, 0.18)',
          surface3: '#ead0c2', surface4: '#dfbca9', textPrimary: '#251913',
          textSecondary: '#59443a', textTertiary: '#786056', accent: '#c85d2e',
        },
        effects: { surfaceOpacity: 0.37, glassBlur: 23 },
        typography: {}, motion: {}, brand: {}, content: {},
        components: {
          header: { background: 'rgba(247, 240, 228, 0.43)' },
          card: { background: 'rgba(251, 246, 237, 0.52)' },
        },
        background: {
          kind: 'gradient', value: 'linear-gradient(135deg, #4f2418, #e09158)',
          opacity: 1, dim: 0.18, overlayColor: '#1f100b',
        },
      },
    };
    localStorage.setItem('nexa-theme-resource-plugins-v2', JSON.stringify([plugin]));
    localStorage.setItem('nexa-active-theme-v1', plugin.id);
  });

  await page.goto('/chat/conv-draft-a');
  await expect(page.locator('html')).toHaveAttribute('data-theme-backdrop', 'true');

  const sidebar = page.getByTestId('chat-history-sidebar').locator('[data-theme-surface=chrome]');
  const toolbarControl = page.getByTestId('chat-auto-tts-toggle');
  const composer = page.getByTestId('chat-input');
  const composerPanel = page.getByTestId('chat-composer-surface');
  const readingSurface = page.getByTestId('chat-reading-surface');
  const workspaceSurface = page.getByTestId('chat-workspace-surface');
  await expect(sidebar).toBeVisible();
  await expect(toolbarControl).toBeVisible();
  await expect(composer).toBeVisible();
  await expect(workspaceSurface).toHaveAttribute('data-theme-surface', 'content');
  await expect(composer).toHaveAttribute('data-theme-surface', 'transparent');
  await expect(readingSurface).toHaveAttribute('data-theme-surface', 'transparent');
  await expect(composerPanel).toHaveAttribute('data-theme-surface', 'panel');
  await expect(readingSurface).toBeVisible();

  expect({
    sidebar: await paintedSurfaceStyle(sidebar),
    toolbar: await paintedSurfaceStyle(toolbarControl, false),
    composer: await paintedSurfaceStyle(composer),
    composerPanel: await paintedSurfaceStyle(composerPanel),
    reading: await paintedSurfaceStyle(readingSurface),
  }).toEqual({
    sidebar: { surface: 'chrome', backgroundAlpha: 0.37, backdropFilter: 'blur(23px)' },
    toolbar: { surface: 'content', backgroundAlpha: 0.82, backdropFilter: 'blur(23px)' },
    composer: { surface: 'content', backgroundAlpha: 0.82, backdropFilter: 'blur(23px)' },
    composerPanel: { surface: 'panel', backgroundAlpha: 0.68, backdropFilter: 'none' },
    reading: { surface: 'content', backgroundAlpha: 0.82, backdropFilter: 'blur(23px)' },
  });
  const composerRecipe = await composerPanel.evaluate((element) => (
    getComputedStyle(element).backgroundImage
  ));
  expect(composerRecipe).toContain('rgba(251, 246, 237, 0.52)');
  expect({
    toolbar: await backdropFilterOwners(toolbarControl),
    composer: await backdropFilterOwners(page.getByTestId('chat-input-textarea')),
    reading: await backdropFilterOwners(readingSurface),
  }).toEqual({
    toolbar: [{ surface: 'content', filter: 'blur(23px)' }],
    composer: [{ surface: 'content', filter: 'blur(23px)' }],
    reading: [{ surface: 'content', filter: 'blur(23px)' }],
  });

  const textarea = page.getByTestId('chat-input-textarea');
  const restingComposerFocus = await composerPanel.evaluate((element) => {
    const style = getComputedStyle(element);
    return { borderColor: style.borderColor, outlineWidth: style.outlineWidth };
  });
  await textarea.focus();
  await expect(textarea).toBeFocused();
  const activeComposerFocus = await composerPanel.evaluate((element) => {
    const style = getComputedStyle(element);
    return { borderColor: style.borderColor, outlineWidth: style.outlineWidth };
  });
  expect(activeComposerFocus.borderColor).not.toBe(restingComposerFocus.borderColor);
  expect(activeComposerFocus.outlineWidth).not.toBe(restingComposerFocus.outlineWidth);
  const composerPanelLayout = await composerPanel.evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      paddingTop: style.paddingTop,
      paddingBottom: style.paddingBottom,
      height: element.getBoundingClientRect().height,
    };
  });
  expect(composerPanelLayout).toEqual({
    paddingTop: '0px',
    paddingBottom: '0px',
    height: expect.any(Number),
  });
  expect(composerPanelLayout.height).toBeLessThan(210);

  const paletteFocusStyle = await composerPanel.evaluate((panel) => {
    const root = document.documentElement;
    root.setAttribute('data-theme-backdrop', 'false');
    const style = getComputedStyle(panel);
    const result = {
      borderColor: style.borderColor,
      outlineWidth: style.outlineWidth,
      backgroundImage: style.backgroundImage,
    };
    root.setAttribute('data-theme-backdrop', 'true');
    return result;
  });
  expect(paletteFocusStyle.borderColor).not.toBe(restingComposerFocus.borderColor);
  expect(paletteFocusStyle.outlineWidth).toBe('1px');
  expect(paletteFocusStyle.backgroundImage).toContain('rgba(251, 246, 237, 0.52)');

  await page.screenshot({
    path: 'test-results/custom-wallpaper-chat-surfaces.png',
    fullPage: true,
  });
  await page.setViewportSize({ width: 820, height: 720 });
  await expect(toolbarControl).toBeVisible();
  await expect(page.getByTestId('chat-input-textarea')).toBeVisible();
  const compactViewport = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(compactViewport.scrollWidth).toBeLessThanOrEqual(compactViewport.clientWidth);
  const compactComposerBounds = await composer.boundingBox();
  expect(compactComposerBounds).not.toBeNull();
  expect(compactComposerBounds!.x).toBeGreaterThanOrEqual(0);
  expect(compactComposerBounds!.x + compactComposerBounds!.width).toBeLessThanOrEqual(820);
  await page.screenshot({
    path: 'test-results/custom-wallpaper-chat-surfaces-compact.png',
    fullPage: true,
  });

  const cdpSession = await page.context().newCDPSession(page);
  await cdpSession.send('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-reduced-transparency', value: 'reduce' }],
  });
  await expect(page.locator('.app-theme-backdrop')).toBeHidden();
  await expect.poll(async () => (
    await paintedSurfaceStyle(composer)
  ).backgroundAlpha).toBe(1);
  expect({
    toolbar: await paintedSurfaceStyle(toolbarControl, false),
    composer: await paintedSurfaceStyle(composer),
    reading: await paintedSurfaceStyle(readingSurface),
  }).toEqual({
    toolbar: { surface: 'transparent', backgroundAlpha: 1, backdropFilter: 'none' },
    composer: { surface: 'transparent', backgroundAlpha: 1, backdropFilter: 'none' },
    reading: { surface: 'transparent', backgroundAlpha: 1, backdropFilter: 'none' },
  });
  await cdpSession.send('Emulation.setEmulatedMedia', { features: [] });
  await cdpSession.detach();
  await expect(page.locator('.app-theme-backdrop')).toBeVisible();

  await page.emulateMedia({ forcedColors: 'active' });
  await expect(page.locator('.app-theme-backdrop')).toBeHidden();
  expect({
    toolbar: await backdropFilterOwners(toolbarControl),
    composer: await backdropFilterOwners(textarea),
    reading: await backdropFilterOwners(readingSurface),
  }).toEqual({ toolbar: [], composer: [], reading: [] });
});

test('keeps the chat message root vertical-only', async ({ page }) => {
  await page.goto('/chat/conv-draft-a');

  const messageRoot = page.locator('[data-chat-scroll-root=true]');
  await expect(messageRoot).toBeVisible();

  const scrollContract = await messageRoot.evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      overflowX: style.overflowX,
      horizontalOverflowPx: Math.max(0, element.scrollWidth - element.clientWidth),
    };
  });
  const rootCanShowHorizontalScrollbar = scrollContract.horizontalOverflowPx > 0
    && !['hidden', 'clip'].includes(scrollContract.overflowX);
  expect(
    rootCanShowHorizontalScrollbar,
    `chat message root must not own horizontal scrolling: ${JSON.stringify(scrollContract)}`,
  ).toBe(false);

  const baselineRootOverflow = scrollContract.horizontalOverflowPx;
  const userMessageText = page.getByTestId('chat-user-message-text').last();
  await userMessageText.evaluate((element) => {
    element.textContent = `https://example.invalid/${'x'.repeat(2_000)}`;
  });
  const rootOverflowAfterLongUserText = await messageRoot.evaluate((element) => (
    Math.max(0, element.scrollWidth - element.clientWidth)
  ));
  expect(rootOverflowAfterLongUserText).toBeLessThanOrEqual(baselineRootOverflow);
  const longUserTextLayout = await userMessageText.evaluate((element) => {
    const bubble = element.parentElement;
    const textRects = Array.from(element.getClientRects());
    const bubbleBounds = bubble?.getBoundingClientRect();
    return {
      overflowWrap: getComputedStyle(element).overflowWrap,
      lineCount: textRects.length,
      rightEdge: Math.max(...textRects.map((rect) => rect.right)),
      bubbleRightEdge: bubbleBounds?.right ?? 0,
    };
  });
  expect(longUserTextLayout.overflowWrap).toBe('anywhere');
  expect(longUserTextLayout.lineCount).toBeGreaterThan(1);
  expect(longUserTextLayout.rightEdge).toBeLessThanOrEqual(longUserTextLayout.bubbleRightEdge + 1);

  const localCodeScroller = messageRoot.locator('pre.overflow-x-auto').first();
  await expect(localCodeScroller).toBeVisible();
  const localScrollContract = await localCodeScroller.evaluate((element) => ({
    overflowX: getComputedStyle(element).overflowX,
    horizontalOverflowPx: Math.max(0, element.scrollWidth - element.clientWidth),
  }));
  expect(localScrollContract.overflowX).toBe('auto');
  expect(localScrollContract.horizontalOverflowPx).toBeGreaterThan(0);
});

test('navigates previous user inputs from the textarea boundary and restores the draft', async ({ page }) => {
  await page.goto('/chat/conv-draft-a');

  const input = page.getByTestId('chat-input-textarea');
  await input.fill('draft before history');

  await input.press('ArrowUp');
  await expect(input).toHaveValue('second saved prompt');

  await input.press('ArrowUp');
  await expect(input).toHaveValue('first saved prompt');

  await input.press('ArrowDown');
  await expect(input).toHaveValue('second saved prompt');

  await input.press('ArrowDown');
  await expect(input).toHaveValue('draft before history');
});
