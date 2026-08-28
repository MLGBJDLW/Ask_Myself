import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    const plugin = {
      manifestVersion: 2,
      kind: 'theme-resource',
      id: 'mermaid-extreme-theme',
      name: 'Mermaid Extreme Theme',
      theme: {
        baseTheme: 'dark',
        mode: 'dark',
        colors: {
          surface0: '#000000',
          surface1: '#050505',
          surface2: '#0a0a0a',
          surface3: '#101010',
          surface4: '#161616',
          textPrimary: '#f8fafc',
          textSecondary: '#cbd5e1',
          textTertiary: '#94a3b8',
          thinkingText: '#f472b6',
          replyText: '#f8fafc',
          accent: '#22d3ee',
        },
        effects: { densityScale: 1.3, radiusScale: 1.4 },
        typography: {
          fontFamily: '"Georgia", serif',
          baseSize: 24,
          lineHeight: 2.2,
          letterSpacing: 0.18,
        },
        motion: {},
        brand: {},
        content: {},
        components: {},
        background: { kind: 'none' },
      },
    };
    localStorage.setItem('nexa-theme-resource-plugins-v2', JSON.stringify([plugin]));
    localStorage.setItem('nexa-active-theme-v1', plugin.id);

    const nowIso = new Date().toISOString();
    const conversation = {
      id: 'conv-mermaid',
      title: 'Mermaid rendering',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const messages = [
      {
        id: 'm-assistant-mermaid',
        conversationId: conversation.id,
        role: 'assistant',
        content: [
          'Here is the flow:',
          '',
          '```mermaid',
          'flowchart TD',
          '  A[Start] --> B{Ready?}',
          '  B -->|Yes| C[Render diagram]',
          '  B -->|No| D[Show source]',
          '  click C "https://example.com/remote"',
          '```',
          '',
          '```mermaid',
          'sequenceDiagram',
          '  participant A as Alice',
          '  participant B as Bob',
          '  A->>B: Render safely',
          '  B-->>A: Visible result',
          '```',
          '',
          '```mermaid',
          'timeline',
          '  title Accessible release timeline',
          '  Jan-Feb : Research',
          '  Mar : Planning',
          '  Apr : Delivery',
          '  May-Jun : Review',
          '```',
          '',
          '```mermaid',
          'graph TD',
          '  A[综合得分 满分100+] --> B[纯利部分: AC/AT × 65<br/>不封顶]',
          '  A --> C[销售额部分: 25×(SV/ST+RSV/RST)/2<br/>封顶30分]',
          '  A --> D[特定目标: 0-10+分<br/>前三项叠加,不封顶]',
          '  B --> E[AC=实际净利, AT=年初业绩指标]',
          '  C --> F[SV=实际含税销售额, ST=年初销售指标]',
          '  C --> G[RSV=实际试剂含税, RST=年初试剂销售指标]',
          '  D --> H[三级医院开发/新项目/装机等]',
          '```',
          '',
          '```mermaid',
          '%%{init: {"theme":"base","themeVariables":{"primaryColor":"#000000","primaryTextColor":"#000000","lineColor":"#000000"}}}%%',
          'flowchart TD',
          '  classDef default fill:#000000,color:#000000,stroke:#000000',
          '  ROOT[核心要素] --> A[因果链路]',
          '  ROOT --> B[反馈效应]',
          '  ROOT --> C[速度变化]',
          '  A --> D[事件子环]',
          '  B --> E[算法逃逸]',
          '```',
        ].join('\n'),
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 0,
        thinking: [
          'Trace diagram:',
          '',
          '```mermaid',
          'flowchart LR',
          '  T[Thinking] --> U[Tooling]',
          '  U --> V[Reply]',
          '```',
        ].join('\n'),
        imageAttachments: null,
      },
    ];
    const defaultAgentConfig = {
      id: 'cfg-mermaid',
      name: 'Mermaid Config',
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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

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
        case 'get_wizard_state':
          return null;
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), clone(messages)];
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_personas_cmd':
        case 'list_projects_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
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
        case 'get_video_config_cmd':
          return { enabled: false, ffmpegPath: '', whisperModel: '', maxDurationSeconds: 0 };
        case 'get_package_host_snapshot_cmd':
          return { packages: [], components: [] };
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

test('renders Mermaid code blocks as SVG diagrams', async ({ page }) => {
  await page.goto('/chat/conv-mermaid');

  await expect(page.locator('svg[id^="mermaid-"]').first()).toBeVisible();
  await expect(page.locator('.timeline-node')).toHaveCount(8);
  await page.getByRole('button', { name: /Thinking completed/ }).click();
  await expect(page.locator('svg[id^="mermaid-"]')).toHaveCount(6);
  await expect(page.getByText('Could not render this Mermaid diagram')).toHaveCount(0);
});


test('keeps sanitized Mermaid structure readable across application themes', async ({ page }) => {
  await page.goto('/chat/conv-mermaid');

  const surfaces = page.getByTestId('mermaid-surface');
  await expect(page.locator('html')).toHaveAttribute('data-custom-theme', 'true');
  await expect(surfaces).toHaveCount(5);
  await expect(page.locator('svg[id^="mermaid-"]')).toHaveCount(5);
  await expect(page.locator('svg style')).toHaveCount(5);
  await expect(page.locator('svg foreignObject')).toHaveCount(0);
  await expect(page.locator('svg [href^="http"], svg [xlink\\:href^="http"]')).toHaveCount(0);

  const renderedLabels = await page.locator('svg').evaluateAll((svgs) =>
    svgs.map((svg) => svg.textContent ?? '').join(' | '),
  );
  expect(renderedLabels).toContain('Start');
  expect(renderedLabels).toContain('Alice');
  expect(renderedLabels).toContain('Accessible release timeline');
  expect(renderedLabels).toContain('综合得分 满分100+');

  const expectedSurfaceColors = await surfaces.evaluateAll((elements) =>
    elements.map((element) => getComputedStyle(element).color),
  );
  await expect(surfaces.first()).toHaveClass(/text-slate-900/);

  for (const theme of ['dark', 'light', 'dream']) {
    await page.evaluate((nextTheme) => {
      const root = document.documentElement;
      root.classList.remove('theme-light', 'theme-midnight', 'theme-aurora', 'theme-bloom', 'theme-dream');
      if (nextTheme !== 'dark') root.classList.add(`theme-${nextTheme}`);
    }, theme);

    for (const surface of await surfaces.all()) {
      await expect(surface).toHaveCSS('background-color', 'rgb(255, 255, 255)');
    }
    await expect.poll(() => surfaces.evaluateAll((elements) =>
      elements.map((element) => getComputedStyle(element).color),
    )).toEqual(expectedSurfaceColors);
  }
});

test('isolates Mermaid geometry and palette from extreme custom typography', async ({ page }) => {
  await page.goto('/chat/conv-mermaid');

  const surfaces = page.getByTestId('mermaid-surface');
  await expect(surfaces).toHaveCount(5);
  await expect(page.locator('svg[id^="mermaid-"]')).toHaveCount(5);
  const diagnostics = await surfaces.evaluateAll((elements) => elements.map((surface) => {
    const luminance = (value: string) => {
      const channels = value.match(/[\d.]+/g)?.slice(0, 3).map(Number);
      if (!channels || channels.length !== 3) throw new Error(`Unsupported color: ${value}`);
      const linear = channels.map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.04045
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
    };
    const svg = surface.querySelector('svg');
    if (!svg) throw new Error('missing Mermaid SVG');
    const nodeDiagnostics = Array.from(svg.querySelectorAll<SVGGElement>('g.node'))
      .slice(0, 12)
      .map((node) => {
        const shape = node.querySelector<SVGGraphicsElement>('rect, polygon, path, circle, ellipse');
        const label = node.querySelector<SVGGraphicsElement>('.label, .nodeLabel, text');
        if (!shape || !label) return null;
        const shapeBox = shape.getBoundingClientRect();
        const labelBox = label.getBoundingClientRect();
        const fill = getComputedStyle(shape).fill;
        const labelFill = getComputedStyle(label).fill;
        const lighter = Math.max(luminance(fill), luminance(labelFill));
        const darker = Math.min(luminance(fill), luminance(labelFill));
        const labelCenter = {
          x: labelBox.x + labelBox.width / 2,
          y: labelBox.y + labelBox.height / 2,
        };
        return {
          fill,
          labelFill,
          contrast: (lighter + 0.05) / (darker + 0.05),
          label: label.textContent ?? '',
          shapeBox: { x: shapeBox.x, y: shapeBox.y, width: shapeBox.width, height: shapeBox.height },
          labelBox: { x: labelBox.x, y: labelBox.y, width: labelBox.width, height: labelBox.height },
          centered:
            labelCenter.x >= shapeBox.x - 1
            && labelCenter.x <= shapeBox.x + shapeBox.width + 1
            && labelCenter.y >= shapeBox.y - 1
            && labelCenter.y <= shapeBox.y + shapeBox.height + 1,
        };
      })
      .filter((value): value is {
        fill: string;
        labelFill: string;
        contrast: number;
        label: string;
        shapeBox: { x: number; y: number; width: number; height: number };
        labelBox: { x: number; y: number; width: number; height: number };
        centered: boolean;
      } => value !== null);
    const style = getComputedStyle(svg);
    return {
      letterSpacing: style.letterSpacing,
      lineHeight: style.lineHeight,
      nodes: nodeDiagnostics,
    };
  }));

  for (const diagram of diagnostics) {
    expect(diagram.letterSpacing).toBe('normal');
    expect(diagram.lineHeight).toBe('normal');
    expect(
      diagram.nodes.every((node) => node.centered),
      JSON.stringify(diagram.nodes.filter((node) => !node.centered), null, 2),
    ).toBe(true);
    expect(diagram.nodes.every((node) => !/^rgb\(0, 0, 0\)$/.test(node.fill))).toBe(true);
    expect(
      diagram.nodes.every((node) => node.contrast >= 4.5),
      JSON.stringify(diagram.nodes.filter((node) => node.contrast < 4.5), null, 2),
    ).toBe(true);
  }
  expect(diagnostics.flatMap((diagram) => diagram.nodes).length).toBeGreaterThan(0);
});

test('keeps every Mermaid timeline section readable', async ({ page }) => {
  await page.goto('/chat/conv-mermaid');

  const timelineNodes = page.locator('.timeline-node');
  await expect(timelineNodes).toHaveCount(8);

  const contrasts = await timelineNodes.evaluateAll((nodes) => {
    const parseRgb = (value: string) => {
      const channels = value.match(/[\d.]+/g)?.slice(0, 3).map(Number);
      if (!channels || channels.length !== 3) throw new Error(`Unsupported color: ${value}`);
      return channels;
    };
    const luminance = (value: string) => {
      const channels = parseRgb(value).map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.04045
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
    };

    return nodes.map((node) => {
      const background = getComputedStyle(node.querySelector('.node-bkg') as SVGElement).fill;
      const foreground = getComputedStyle(node.querySelector('text') as SVGTextElement).fill;
      const lighter = Math.max(luminance(background), luminance(foreground));
      const darker = Math.min(luminance(background), luminance(foreground));
      return { background, foreground, ratio: (lighter + 0.05) / (darker + 0.05) };
    });
  });

  expect(contrasts, JSON.stringify(contrasts)).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ ratio: expect.any(Number) }),
    ]),
  );
  for (const contrast of contrasts) {
    expect(contrast.ratio, JSON.stringify(contrast)).toBeGreaterThanOrEqual(4.5);
  }
});
