import { expect, test, type Locator } from '@playwright/test';

async function selectNexaOption(trigger: Locator, value: string) {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`).click();
}

async function expectVerticallyCentered(container: Locator, icon: Locator) {
  const [containerBox, iconBox] = await Promise.all([container.boundingBox(), icon.boundingBox()]);
  if (!containerBox || !iconBox) throw new Error('Missing icon geometry');
  const containerCenter = containerBox.y + containerBox.height / 2;
  const iconCenter = iconBox.y + iconBox.height / 2;
  expect(Math.abs(containerCenter - iconCenter)).toBeLessThanOrEqual(1);
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');

    const nowIso = new Date().toISOString();
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const emptyGraph = { nodes: [], edges: [], totalNodes: 0, totalEdges: 0, scopeLabel: null };

    const defaultAgentConfig = {
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
    };

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'plugin:event|listen':
          return listenerSeq++;
        case 'plugin:event|unlisten':
          return null;
        case 'get_compile_stats_cmd':
          return { totalDocs: 2, compiledDocs: 2, totalEntities: 0, totalLinks: 0 };
        case 'list_sources':
          return [
            {
              id: 'source-novel',
              rootPath: 'D:/Books',
              includeGlobs: [],
              excludeGlobs: [],
              watchEnabled: true,
              createdAt: nowIso,
              updatedAt: nowIso,
            },
          ];
        case 'get_knowledge_graph_cmd':
          (window as unknown as { __lastGraphArgs?: Record<string, unknown> }).__lastGraphArgs = args;
          return (
            window as unknown as {
              __knowledgeGraphMock?: unknown;
            }
          ).__knowledgeGraphMock ?? { ...emptyGraph, scopeLabel: args.pathPrefix ?? null };
        case 'list_agent_configs_cmd':
          return [defaultAgentConfig];
        case 'get_model_context_window':
          return 1047576;
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

test('keeps folder scope usable across all sources and makes empty filters recoverable', async ({ page }) => {
  await page.goto('/knowledge');

  await page.getByRole('button', { name: 'Topics & Connections' }).click();

  const folderInput = page.getByPlaceholder('e.g. novel/volume-1');
  await expect(folderInput).toBeEnabled();

  await folderInput.fill('novel');

  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __lastGraphArgs?: unknown }).__lastGraphArgs))
    .toMatchObject({ sourceId: null, pathPrefix: 'novel' });
  await expect(page.getByText('No matching graph nodes')).toBeVisible();

  await page.getByRole('button', { name: 'Clear Filters' }).click();

  await expect(folderInput).toHaveValue('');
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __lastGraphArgs?: unknown }).__lastGraphArgs))
    .toMatchObject({ sourceId: null, pathPrefix: null });
});

test('keeps topic and relationship icons vertically centered', async ({ page }) => {
  await page.goto('/knowledge');
  await page.evaluate(() => {
    (window as unknown as { __knowledgeGraphMock?: unknown }).__knowledgeGraphMock = {
      nodes: [
        {
          id: 'aligned-topic',
          label: 'Aligned Topic',
          entityType: 'concept',
          description: 'Icon alignment fixture',
          mentionCount: 4,
          documentCount: 1,
          linkCount: 0,
          firstSeenDoc: 'doc-1',
          documents: [],
        },
      ],
      edges: [],
      totalNodes: 1,
      totalEdges: 0,
      scopeLabel: 'alignment',
    };
  });

  const mapTab = page.getByRole('button', { name: 'Topics & Connections' });
  await expectVerticallyCentered(mapTab, mapTab.locator('svg'));
  await mapTab.click();

  const refreshButton = page.getByRole('button', { name: 'Refresh' });
  await expectVerticallyCentered(refreshButton, refreshButton.locator('svg'));

  const searchInput = page.getByPlaceholder('Search nodes...');
  await expectVerticallyCentered(searchInput, searchInput.locator('xpath=..').locator('svg'));

  const graphTitle = page.getByText('Relationship Graph', { exact: true });
  await expectVerticallyCentered(graphTitle, graphTitle.locator('svg'));

  const detailHeading = page.getByRole('heading', { name: 'Aligned Topic' });
  const detailHeaderRow = detailHeading.locator('../..');
  const detailIcon = detailHeaderRow.locator('svg').first();
  await expectVerticallyCentered(detailIcon.locator('..'), detailIcon);
});

test('shows multi-relation bundles and stores bundled agent context', async ({ page }) => {
  await page.goto('/knowledge');
  await page.evaluate(() => {
    (window as unknown as { __knowledgeGraphMock?: unknown }).__knowledgeGraphMock = {
      nodes: [
        {
          id: 'lin',
          label: 'Lin',
          entityType: 'person',
          description: 'Lead character',
          mentionCount: 12,
          documentCount: 2,
          linkCount: 3,
          firstSeenDoc: 'doc-1',
          documents: [
            { documentId: 'doc-1', title: 'Chapter One', path: 'D:/Books/novel/chapter-1.md', sourceId: 'source-novel' },
          ],
        },
        {
          id: 'city',
          label: 'Mirror City',
          entityType: 'place',
          description: 'Main city',
          mentionCount: 8,
          documentCount: 2,
          linkCount: 3,
          firstSeenDoc: 'doc-1',
          documents: [
            { documentId: 'doc-1', title: 'Chapter One', path: 'D:/Books/novel/chapter-1.md', sourceId: 'source-novel' },
          ],
        },
      ],
      edges: [
        {
          id: 'edge-located',
          source: 'lin',
          target: 'city',
          relationType: 'located_in',
          strength: 0.9,
          evidenceDocId: 'doc-1',
          evidenceTitle: 'Chapter One',
          evidencePath: 'D:/Books/novel/chapter-1.md',
        },
        {
          id: 'edge-protects',
          source: 'lin',
          target: 'city',
          relationType: 'protects',
          strength: 0.8,
          evidenceDocId: 'doc-1',
          evidenceTitle: 'Chapter One',
          evidencePath: 'D:/Books/novel/chapter-1.md',
        },
        {
          id: 'edge-threatens',
          source: 'city',
          target: 'lin',
          relationType: 'threatens',
          strength: 0.7,
          evidenceDocId: 'doc-1',
          evidenceTitle: 'Chapter One',
          evidencePath: 'D:/Books/novel/chapter-1.md',
        },
      ],
      totalNodes: 2,
      totalEdges: 3,
      scopeLabel: 'novel',
    };
  });

  await page.getByRole('button', { name: 'Topics & Connections' }).click();

  await expect(page.getByText('1 Bundles').first()).toBeVisible();
  await expect(page.getByText('3 Relations').first()).toBeVisible();

  const bundle = page.getByRole('button', { name: /Lin.*Mirror City.*3 relations/i });
  await expect(bundle).toBeVisible();
  await bundle.click();

  const detail = page.locator('aside');
  await expect(detail.getByText('Relationship Bundle')).toBeVisible();
  await expect(detail.getByText('located in')).toBeVisible();
  await expect(detail.getByText('protects')).toBeVisible();
  await expect(detail.getByText('threatens')).toBeVisible();

  await page.getByRole('button', { name: 'Expand Relations' }).click();
  await expect(page.getByRole('button', { name: 'Bundle Relations' })).toBeVisible();

  await page.getByRole('button', { name: 'Use as Context' }).click();
  const context = await page.evaluate(() => {
    const raw = window.localStorage.getItem('nexa-graph-agent-context-v1');
    return raw ? JSON.parse(raw) : null;
  });
  expect(context.relationBundles[0].relationCount).toBe(3);
  expect(context.edges).toHaveLength(3);
});

test('starts with a hub overview, drills into a readable focus, and expands to atlas', async ({ page }) => {
  await page.goto('/knowledge');
  await page.evaluate(() => {
    const nodes = Array.from({ length: 65 }, (_, index) => ({
      id: `node-${index}`,
      label: index === 0 ? 'Anchor Topic' : index === 64 ? 'Remote Topic' : `Side Topic ${index}`,
      entityType: index % 5 === 0 ? 'person' : index % 5 === 1 ? 'place' : index % 5 === 2 ? 'organization' : index % 5 === 3 ? 'event' : 'concept',
      description: `Node ${index}`,
      mentionCount: index === 0 ? 40 : index === 64 ? 0 : 1,
      documentCount: index === 0 ? 9 : index === 64 ? 0 : 1,
      linkCount: index < 8 ? 1 : 0,
      firstSeenDoc: 'doc-1',
      documents: [
        { documentId: 'doc-1', title: 'Chapter One', path: 'D:/Books/novel/chapter-1.md', sourceId: 'source-novel' },
      ],
    }));
    const edges = Array.from({ length: 7 }, (_, index) => ({
      id: `edge-${index}`,
      source: 'node-0',
      target: `node-${index + 1}`,
      relationType: 'related_to',
      strength: 0.7,
      evidenceDocId: 'doc-1',
      evidenceTitle: 'Chapter One',
      evidencePath: 'D:/Books/novel/chapter-1.md',
    }));

    (window as unknown as { __knowledgeGraphMock?: unknown }).__knowledgeGraphMock = {
      nodes,
      edges,
      totalNodes: nodes.length,
      totalEdges: edges.length,
      scopeLabel: 'novel',
    };
  });

  await page.getByRole('button', { name: 'Topics & Connections' }).click();

  await expect(page.getByRole('button', { name: /Anchor Topic, Person/i })).toBeVisible();
  await expect(page.getByRole('button', { name: /Remote Topic/i })).toHaveCount(0);
  await expect(page.getByText(/Hidden/)).toBeVisible();
  await expect(page.getByText('40/65 Nodes')).toBeVisible();

  const overviewViewBox = await page.locator('svg[role="img"]').getAttribute('viewBox');
  await page.getByRole('button', { name: /Anchor Topic, Person/i }).click();

  await expect(page.getByText('8/65 Nodes')).toBeVisible();
  await expect
    .poll(() => page.locator('svg[role="img"]').getAttribute('viewBox'))
    .not.toBe(overviewViewBox);

  const sideTopic = page.getByRole('button', { name: /^Side Topic 1, Place$/i });
  const sideTopicCore = sideTopic.locator('.kg-node-core');
  const beforeDragCx = await sideTopicCore.getAttribute('cx');
  const beforeDragCy = await sideTopicCore.getAttribute('cy');
  const svgBox = await page.locator('svg[role="img"]').boundingBox();
  const focusViewBox = (await page.locator('svg[role="img"]').getAttribute('viewBox'))?.split(' ').map(Number);
  if (!svgBox || !focusViewBox || !beforeDragCx || !beforeDragCy) {
    throw new Error('Missing graph geometry for drag test');
  }
  const [viewX, viewY, viewWidth, viewHeight] = focusViewBox;
  const startX = svgBox.x + ((Number(beforeDragCx) - viewX) / viewWidth) * svgBox.width;
  const startY = svgBox.y + ((Number(beforeDragCy) - viewY) / viewHeight) * svgBox.height;
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX + 80, startY + 36, { steps: 6 });
  await page.mouse.up();
  await expect
    .poll(async () => {
      const nextCx = await sideTopicCore.getAttribute('cx');
      if (!nextCx) return 0;
      return Math.abs(Number(nextCx) - Number(beforeDragCx));
    })
    .toBeGreaterThan(1);

  await page.getByRole('button', { name: 'Atlas' }).click();
  await selectNexaOption(page.getByLabel('Nodes Shown'), '100');

  await expect(page.getByRole('button', { name: /Remote Topic/i })).toBeVisible();
});
