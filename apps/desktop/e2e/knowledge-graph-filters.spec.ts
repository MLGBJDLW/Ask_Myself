import { expect, test } from '@playwright/test';

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
