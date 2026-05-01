import { expect, test } from '@playwright/test';

declare global {
  interface Window {
    __lastAgentPrompt?: string;
    __lastSourceIds?: string[];
  }
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
    let seq = 0;
    const nextId = (prefix: string) => `${prefix}-${Date.now()}-${seq++}`;
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    window.__lastAgentPrompt = undefined;
    window.__lastSourceIds = undefined;

    const defaultAgentConfig = {
      id: 'cfg-agent-edit',
      name: 'Agent Edit Config',
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
      subagentAllowedSkillIds: null,
      subagentMaxParallel: null,
      subagentMaxCallsPerTurn: null,
      subagentTokenBudget: null,
      toolTimeoutSecs: null,
      agentTimeoutSecs: null,
      dynamicToolVisibility: null,
      traceEnabled: null,
      requireToolConfirmation: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const conversations: Record<string, Conversation> = {
      'conv-agent-edit': {
        id: 'conv-agent-edit',
        title: 'Agent Edit Source',
        provider: 'open_ai',
        model: 'gpt-4.1',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    const messagesByConversation: Record<string, Message[]> = {
      'conv-agent-edit': [
        {
          id: 'm-assistant-file',
          conversationId: 'conv-agent-edit',
          role: 'assistant',
          content: 'Open `notes/agent-edit.md` and improve the action item.',
          toolCallId: null,
          toolCalls: [],
          artifacts: null,
          tokenCount: 0,
          createdAt: nowIso,
          sortOrder: 0,
          thinking: null,
          imageAttachments: null,
        },
      ],
    };

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
        case 'get_wizard_state_cmd':
          return { completed: true, completedAt: nowIso };
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'save_agent_config_cmd':
          return clone(defaultAgentConfig);
        case 'set_default_agent_config_cmd':
          return null;
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'list_projects_cmd':
          return [];
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [clone(conversations[id]), clone(messagesByConversation[id] ?? [])] as const;
        }
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
          return [];
        case 'set_conversation_sources_cmd':
          window.__lastSourceIds = Array.isArray(args.sourceIds)
            ? args.sourceIds.map(String)
            : [];
          return null;
        case 'update_conversation_system_prompt_cmd':
        case 'update_conversation_collection_context_cmd':
        case 'compact_conversation_cmd':
        case 'agent_stop_cmd':
          return null;
        case 'get_index_stats':
          return { totalDocuments: 1, totalChunks: 1, ftsRows: 1 };
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
        case 'clear_answer_cache':
          return 0;
        case 'preview_file_cmd':
          return {
            path: 'D:\\Vault\\notes\\agent-edit.md',
            displayName: 'agent-edit.md',
            sourceId: 'src-agent-edit',
            sourceName: 'Notes',
            extension: '.md',
            mimeType: 'text/markdown',
            kind: 'markdown',
            language: 'markdown',
            content: [
              '# Release Notes',
              '',
              'Alpha is ready.',
              'Beta needs a clearer action item before launch.',
              'Gamma is stable.',
            ].join('\n'),
            encoding: 'utf-8',
            editable: true,
            sizeBytes: 128,
            modifiedAt: nowIso,
            hash: 'sha256-agent-edit',
            lineCount: 5,
            truncated: false,
            warning: null,
          };
        case 'create_conversation_cmd': {
          const id = 'conv-agent-edit-created';
          const conversation: Conversation = {
            id,
            title: 'Selected text edit',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
            createdAt: nowIso,
            updatedAt: nowIso,
          };
          conversations[id] = conversation;
          messagesByConversation[id] = [];
          return clone(conversation);
        }
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          const message = String(args.message ?? '');
          window.__lastAgentPrompt = message;
          messagesByConversation[conversationId] = [
            ...(messagesByConversation[conversationId] ?? []),
            {
              id: nextId('m-user'),
              conversationId,
              role: 'user',
              content: message,
              toolCallId: null,
              toolCalls: [],
              artifacts: null,
              tokenCount: 0,
              createdAt: new Date().toISOString(),
              sortOrder: 0,
              thinking: null,
              imageAttachments: null,
            },
          ];
          return null;
        }
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

test('sends an exact selected file range to the agent edit flow', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /agent-edit\.md/i }).click();
  await expect(page.getByLabel('File Preview')).toBeVisible();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();

  const editor = page.getByTestId('file-preview-editor');
  await expect(editor).toHaveValue(/Beta needs a clearer action item before launch\./);

  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    const selected = 'Beta needs a clearer action item before launch.';
    const start = textarea.value.indexOf(selected);
    textarea.focus();
    textarea.setSelectionRange(start, start + selected.length);
    textarea.dispatchEvent(new Event('select', { bubbles: true }));
    textarea.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  });

  await expect(page.getByTestId('file-preview-agent-panel')).toBeVisible();
  await expect(page.getByText(/Selected 47 chars/)).toBeVisible();

  await page
    .getByTestId('file-preview-agent-instruction')
    .fill('Make this a direct launch checklist item.');
  await page.getByTestId('file-preview-agent-send').click();

  await expect
    .poll(() => page.evaluate(() => window.__lastAgentPrompt ?? ''), {
      timeout: 10_000,
    })
    .toContain('Make this a direct launch checklist item.');

  const prompt = await page.evaluate(() => window.__lastAgentPrompt ?? '');
  expect(prompt).toContain('File: D:\\Vault\\notes\\agent-edit.md');
  expect(prompt).toContain('Line range: 4');
  expect(prompt).toContain('Beta needs a clearer action item before launch.');
  expect(prompt).toContain('Use read_file first');
  expect(prompt).toContain('Use edit_file to modify the file');

  await expect
    .poll(() => page.evaluate(() => window.__lastSourceIds ?? []))
    .toEqual(['src-agent-edit']);
});
