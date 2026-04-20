import { expect, test } from '@playwright/test';

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

    type Playbook = {
      id: string;
      title: string;
      description: string;
      queryText?: string | null;
      citations: Array<{ id: string; playbookId: string; chunkId: string; annotation: string; order: number }>;
      createdAt: string;
      updatedAt: string;
    };

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();

    const playbook: Playbook = {
      id: 'pb-connected',
      title: 'Retry Collection',
      description: 'Saved evidence about retries.',
      queryText: 'retry timeout guard',
      citations: [
        {
          id: 'cit-1',
          playbookId: 'pb-connected',
          chunkId: 'chunk-1234567890abcdef',
          annotation: 'Important timeout note',
          order: 0,
        },
      ],
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const defaultAgentConfig = {
      id: 'cfg-playbooks',
      name: 'Playbooks Config',
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
          return [] as Conversation[];
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
        case 'list_playbooks':
          return [clone(playbook)];
        case 'get_playbook':
          return clone(playbook);
        case 'get_evidence_card':
          return {
            chunkId: 'chunk-1234567890abcdef',
            documentId: 'doc-1',
            sourceName: 'Knowledge Base',
            documentPath: 'D:/notes/retries.md',
            documentTitle: 'Retries Guide',
            content: 'Keep the timeout guard and show the retry limit in the UI.',
            headingPath: ['Timeouts', 'Retries'],
            score: 0.91,
            highlights: [],
            snippet: 'Keep the timeout guard and show the retry limit in the UI.',
          };
        case 'open_file_in_default_app':
        case 'show_in_file_explorer':
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
      unregisterListener: () => {},
    };
  });
});

test('connects collections list and detail view to playbook and evidence APIs', async ({ page }) => {
  await page.goto('/playbooks');

  await expect(page.getByText('Retry Collection')).toBeVisible();
  await expect(page.getByText('1 citations')).toBeVisible();

  await page.getByRole('button', { name: /Retry Collection/ }).click();

  await expect(page.getByText('retry timeout guard')).toBeVisible();
  await expect(page.getByText('Retries Guide')).toBeVisible();
  await expect(page.getByText('Knowledge Base')).toBeVisible();
  await expect(page.getByText('Keep the timeout guard and show the retry limit in the UI.')).toBeVisible();
  await expect(page.getByText('Important timeout note')).toBeVisible();
});
