import { expect, test } from '@playwright/test';

const THEMES = ['dark', 'light', 'midnight', 'aurora', 'bloom', 'dream'] as const;

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('nexa-theme', 'dark');

    const nowIso = new Date().toISOString();
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbacks = new Map<number, (event: unknown) => void>();
    const graph = {
      nodes: [
        {
          id: 'anchor',
          label: 'Anchor Topic',
          entityType: 'concept',
          description: 'Primary visual-regression node',
          mentionCount: 18,
          documentCount: 2,
          linkCount: 1,
          firstSeenDoc: 'doc-1',
          documents: [],
        },
        {
          id: 'neighbor',
          label: 'Neighbor Topic',
          entityType: 'technology',
          description: 'Connected visual-regression node',
          mentionCount: 8,
          documentCount: 1,
          linkCount: 1,
          firstSeenDoc: 'doc-1',
          documents: [],
        },
      ],
      edges: [
        {
          id: 'edge-1',
          source: 'anchor',
          target: 'neighbor',
          relationType: 'related_to',
          strength: 0.8,
          evidenceDocId: 'doc-1',
          evidenceTitle: 'Fixture',
          evidencePath: 'D:/fixture.md',
        },
      ],
      totalNodes: 2,
      totalEdges: 1,
      scopeLabel: 'visual-regression',
    };

    const invoke = async (cmd: string) => {
      switch (cmd) {
        case 'plugin:event|listen':
          return listenerSeq++;
        case 'plugin:event|unlisten':
          return null;
        case 'get_compile_stats_cmd':
          return { totalDocs: 1, compiledDocs: 1, totalEntities: 2, totalLinks: 1 };
        case 'get_knowledge_graph_cmd':
          return graph;
        case 'list_agent_configs_cmd':
          return [{
            id: 'cfg-knowledge',
            name: 'Knowledge Config',
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
          }];
        case 'get_model_context_window':
          return 1047576;
        case 'list_sources':
        case 'list_conversations_cmd':
        case 'get_conversation_sources_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
          return [];
        case 'get_index_stats':
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case 'get_privacy_config':
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case 'get_embedder_config_cmd':
          return { provider: 'tfidf', vectorDimensions: 384 };
        case 'get_ocr_config_cmd':
          return { enabled: false, minConfidence: 0.5, llmFallback: false, detectionLimit: 2048, useCls: false };
        case 'check_ocr_models_cmd':
          return false;
        default:
          return null;
      }
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      convertFileSrc: (filePath: string) => filePath,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => {},
    };
  });
});

test('masks node frost to the node alpha and renders theme-owned graph colors', async ({ page }) => {
  await page.goto('/knowledge');
  await page.getByRole('button', { name: 'Topics & Connections' }).click();

  const graph = page.locator('svg[role="img"]');
  await expect(graph.locator('.kg-node-core')).toHaveCount(2);
  await expect(graph.locator('#knowledge-node-frost feComposite[in2="SourceAlpha"][operator="in"]')).toHaveCount(1);
  await expect(graph.locator('#knowledge-node-frost feBlend[in2="masked-grain"]')).toHaveCount(1);
  await expect(graph.locator('.stroke-white')).toHaveCount(0);

  const themeColors = new Map<string, string>();
  for (const theme of THEMES) {
    await page.evaluate((nextTheme) => {
      const root = document.documentElement;
      root.classList.remove('theme-light', 'theme-midnight', 'theme-aurora', 'theme-bloom', 'theme-dream');
      if (nextTheme !== 'dark') root.classList.add(`theme-${nextTheme}`);
    }, theme);

    const colors = await graph.evaluate((element) => {
      const rootStyle = getComputedStyle(document.documentElement);
      const label = element.querySelector('.kg-label-chip');
      const core = element.querySelector('.kg-node-core');
      if (!label || !core) throw new Error('Missing graph visual fixture');
      return {
        canvas: rootStyle.getPropertyValue('--graph-canvas-glow-center').trim(),
        frost: rootStyle.getPropertyValue('--graph-node-frost').trim(),
        labelBackground: getComputedStyle(label).fill,
        labelBorder: getComputedStyle(label).stroke,
        coreFilter: getComputedStyle(core).filter,
      };
    });

    expect(colors.canvas).not.toBe('');
    expect(colors.frost).not.toBe('');
    expect(colors.labelBackground).not.toBe('none');
    expect(colors.labelBorder).not.toBe('none');
    expect(colors.coreFilter).toContain('knowledge-node-frost');
    themeColors.set(theme, `${colors.canvas}|${colors.labelBackground}|${colors.labelBorder}`);

    await test.info().attach(`knowledge-graph-${theme}`, {
      body: await graph.screenshot({ animations: 'disabled' }),
      contentType: 'image/png',
    });
  }

  expect(new Set(themeColors.values()).size).toBe(THEMES.length);

  await page.getByRole('button', { name: /Anchor Topic, Concept/i }).click();
  await expect(page.getByRole('button', { name: 'Focus' })).toHaveClass(/bg-accent-subtle/);
  await page.getByRole('button', { name: 'Atlas' }).click();
  await expect(page.getByRole('button', { name: 'Atlas' })).toHaveClass(/bg-accent-subtle/);
});
