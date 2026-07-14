import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

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
  await expect(page.locator('svg[id^="mermaid-"]')).toHaveCount(3);
  await expect(page.getByText('Could not render this Mermaid diagram')).toHaveCount(0);
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
