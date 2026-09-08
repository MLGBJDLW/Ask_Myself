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
          content: 'Open `D:\\Vault\\scripts\\server.py` and inspect `D:\\Vault\\docs\\manual.pdf`. Also improve `D:\\Vault\\notes\\agent-edit.md`, preview `D:\\Vault\\web\\index.html`, inspect `D:\\Vault\\docs\\office-proposal.docx`, `D:\\Vault\\docs\\structured-report.docx`, and `D:\\Vault\\sheets\\budget.xlsx`.',
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
        case 'list_builtin_skills_cmd':
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
          return 0;
        case 'preview_file_cmd':
          if (String(args.path ?? '').endsWith('index.html')) {
            return {
              path: 'D:\\Vault\\web\\index.html',
              displayName: 'index.html',
              sourceId: 'src-web-preview',
              sourceName: 'Web Preview',
              extension: '.html',
              mimeType: 'text/html',
              kind: 'html',
              language: 'html',
              content: [
                '<!doctype html>',
                '<html>',
                '<head><style>body { font-family: sans-serif; color: #123456; }</style></head>',
                '<body><h1>Original preview</h1></body>',
                '</html>',
              ].join('\n'),
              encoding: 'utf-8',
              editable: true,
              sizeBytes: 156,
              modifiedAt: nowIso,
              hash: 'sha256-html-preview',
              lineCount: 5,
              truncated: false,
              warning: null,
              structuredPreview: null,
              renderedPreview: null,
              capabilities: {
                canRenderStructured: false,
                canExtractText: true,
                needsExternalRuntime: false,
                structuredUnavailableReason: null,
              },
            };
          }
          if (String(args.path ?? '').endsWith('structured-report.docx')) {
            return {
              path: 'D:\\Vault\\docs\\structured-report.docx',
              displayName: 'structured-report.docx',
              sourceId: 'src-office-docs',
              sourceName: 'Office Docs',
              extension: '.docx',
              mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
              kind: 'document',
              language: null,
              content: [
                'Quarterly Report',
                'Revenue increased for enterprise accounts.',
                'North America: $125K',
              ].join('\n'),
              encoding: 'extracted-text',
              editable: false,
              sizeBytes: 92000,
              modifiedAt: nowIso,
              hash: 'sha256-structured-report',
              lineCount: 3,
              truncated: false,
              warning: null,
              structuredPreview: {
                type: 'document',
                assets: [],
                blocks: [
                  {
                    type: 'heading',
                    level: 1,
                    alignment: null,
                    runs: [
                      {
                        text: 'Quarterly Report',
                        bold: true,
                        italic: false,
                        underline: false,
                        color: null,
                        backgroundColor: null,
                        fontSize: 'xlarge',
                        hyperlink: null,
                      },
                    ],
                  },
                  {
                    type: 'paragraph',
                    alignment: null,
                    runs: [
                      {
                        text: 'Revenue increased for enterprise accounts.',
                        bold: false,
                        italic: false,
                        underline: false,
                        color: null,
                        backgroundColor: null,
                        fontSize: null,
                        hyperlink: null,
                      },
                    ],
                  },
                  {
                    type: 'table',
                    rows: [
                      {
                        cells: [
                          {
                            blocks: [
                              {
                                type: 'paragraph',
                                alignment: null,
                                runs: [
                                  {
                                    text: 'Region',
                                    bold: true,
                                    italic: false,
                                    underline: false,
                                    color: null,
                                    backgroundColor: null,
                                    fontSize: null,
                                    hyperlink: null,
                                  },
                                ],
                              },
                            ],
                          },
                          {
                            blocks: [
                              {
                                type: 'paragraph',
                                alignment: null,
                                runs: [
                                  {
                                    text: 'Revenue',
                                    bold: true,
                                    italic: false,
                                    underline: false,
                                    color: null,
                                    backgroundColor: null,
                                    fontSize: null,
                                    hyperlink: null,
                                  },
                                ],
                              },
                            ],
                          },
                        ],
                      },
                      {
                        cells: [
                          {
                            blocks: [
                              {
                                type: 'paragraph',
                                alignment: null,
                                runs: [
                                  {
                                    text: 'North America',
                                    bold: false,
                                    italic: false,
                                    underline: false,
                                    color: null,
                                    backgroundColor: null,
                                    fontSize: null,
                                    hyperlink: null,
                                  },
                                ],
                              },
                            ],
                          },
                          {
                            blocks: [
                              {
                                type: 'paragraph',
                                alignment: null,
                                runs: [
                                  {
                                    text: '$125K',
                                    bold: false,
                                    italic: false,
                                    underline: false,
                                    color: null,
                                    backgroundColor: null,
                                    fontSize: null,
                                    hyperlink: null,
                                  },
                                ],
                              },
                            ],
                          },
                        ],
                      },
                    ],
                  },
                ],
              },
              renderedPreview: null,
              capabilities: {
                canRenderStructured: true,
                canExtractText: true,
                needsExternalRuntime: false,
                structuredUnavailableReason: null,
              },
            };
          }
          if (String(args.path ?? '').endsWith('budget.xlsx')) {
            return {
              path: 'D:\\Vault\\sheets\\budget.xlsx',
              displayName: 'budget.xlsx',
              sourceId: 'src-office-docs',
              sourceName: 'Office Docs',
              extension: '.xlsx',
              mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
              kind: 'document',
              language: null,
              content: ['Name\tTotal', 'Q1\t3'].join('\n'),
              encoding: 'extracted-text',
              editable: false,
              sizeBytes: 64000,
              modifiedAt: nowIso,
              hash: 'sha256-budget',
              lineCount: 2,
              truncated: false,
              warning: null,
              structuredPreview: {
                type: 'workbook',
                truncated: false,
                limits: { maxSheets: 20, maxRows: 500, maxColumns: 60 },
                sheets: [
                  {
                    name: 'Summary',
                    index: 0,
                    rowCount: 2,
                    columnCount: 2,
                    previewRowCount: 2,
                    previewColumnCount: 2,
                    truncated: false,
                    mergedRanges: [{ startRow: 0, startColumn: 0, endRow: 0, endColumn: 1 }],
                    cells: [
                      { row: 0, column: 0, value: 'Name', dataType: 'string', formula: null },
                      { row: 0, column: 1, value: 'Total', dataType: 'string', formula: null },
                      { row: 1, column: 0, value: 'Q1', dataType: 'string', formula: null },
                      { row: 1, column: 1, value: '3', dataType: 'number', formula: '1+2' },
                    ],
                  },
                  {
                    name: 'Detail',
                    index: 1,
                    rowCount: 1,
                    columnCount: 1,
                    previewRowCount: 1,
                    previewColumnCount: 1,
                    truncated: false,
                    mergedRanges: [],
                    cells: [{ row: 0, column: 0, value: 'Detail row', dataType: 'string', formula: null }],
                  },
                ],
              },
              renderedPreview: null,
              capabilities: {
                canRenderStructured: true,
                canExtractText: true,
                needsExternalRuntime: false,
                structuredUnavailableReason: null,
              },
            };
          }
          if (String(args.path ?? '').endsWith('office-proposal.docx')) {
            return {
              path: 'D:\\Vault\\docs\\office-proposal.docx',
              displayName: 'office-proposal.docx',
              sourceId: 'src-office-docs',
              agentEditAllowed: true,
              sourceName: 'Office Docs',
              extension: '.docx',
              mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
              kind: 'document',
              language: null,
              content: [
                'Executive Summary',
                'The team needs a softer launch statement for enterprise buyers.',
                'Budget remains unchanged.',
              ].join('\n'),
              encoding: 'extracted-text',
              editable: false,
              sizeBytes: 114000,
              modifiedAt: nowIso,
              hash: 'sha256-office-proposal',
              lineCount: 3,
              truncated: false,
              warning: null,
              structuredPreview: null,
              renderedPreview: null,
              capabilities: {
                canRenderStructured: false,
                canExtractText: true,
                needsExternalRuntime: false,
                structuredUnavailableReason: null,
              },
            };
          }
          return {
            path: 'D:\\Vault\\notes\\agent-edit.md',
            displayName: 'agent-edit.md',
            sourceId: localStorage.getItem('e2e-unindexed-file') ? null : 'src-agent-edit',
            agentEditAllowed: !localStorage.getItem('e2e-restricted-file'),
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
              '',
              '[OpenAI](https://openai.com/file-preview)',
            ].join('\n'),
            encoding: 'utf-8',
            editable: true,
            sizeBytes: 128,
            modifiedAt: nowIso,
            hash: 'sha256-agent-edit',
            lineCount: 7,
            truncated: false,
            warning: null,
            structuredPreview: null,
            renderedPreview: null,
            capabilities: {
              canRenderStructured: false,
              canExtractText: true,
              needsExternalRuntime: false,
              structuredUnavailableReason: null,
            },
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

for (const mode of ['indexed', 'unindexed-open', 'unindexed-restricted']) {
const unindexed = mode !== 'indexed';
const restricted = mode === 'unindexed-restricted';
test(`${restricted ? 'explains existing agent access restrictions for' : 'sends an exact selected file range to the agent edit flow for'} ${mode} files`, async ({ page }) => {
  if (unindexed) await page.addInitScript(() => localStorage.setItem('e2e-unindexed-file', '1'));
  if (restricted) await page.addInitScript(() => localStorage.setItem('e2e-restricted-file', '1'));
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
  if (restricted) {
    await expect(page.getByTestId('file-preview-agent-send')).toBeDisabled();
    await expect(page.getByText(/This file is outside the agent’s allowed directories/)).toBeVisible();
    await page.getByTestId('file-preview-agent-instruction').fill('Make this clearer.');
    await page.getByTestId('file-preview-agent-instruction').press('Enter');
    await expect(page.getByLabel('File Preview')).toBeVisible();
    expect(await page.evaluate(() => window.__lastAgentPrompt ?? '')).toBe('');
    await expect(editor).toBeEditable();
    return;
  }

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
    .toEqual(unindexed ? [] : ['src-agent-edit']);
});
}

test('renders dedicated SVG icons for code and document file badges', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  const pythonBadge = page.locator('[data-file-icon="python"]');
  const pdfBadge = page.locator('[data-file-icon="pdf"]');
  await expect(pythonBadge).toContainText('server.py');
  await expect(pdfBadge).toContainText('manual.pdf');
  await expect(page.locator('[data-file-icon="markdown"]')).toContainText('agent-edit.md');
  await expect(page.locator('[data-file-icon="word"]')).toHaveCount(2);
  await expect(page.locator('[data-file-icon="excel"]')).toContainText('budget.xlsx');
  await expect(pythonBadge.locator('svg')).toHaveCount(1);
  await expect(pdfBadge.locator('svg')).toHaveCount(1);
  await expect(pythonBadge).toHaveAttribute('data-file-treatment', 'brand-accent');
  await expect(pythonBadge.locator('[data-file-icon-accent="true"]')).toHaveCount(1);
  await expect(pythonBadge.locator('svg')).toHaveCSS('color', 'rgb(55, 118, 171)');
  await expect(pdfBadge).toHaveAttribute('data-file-treatment', 'mono');
});

test('opens file preview as a large panel and closes it from outside clicks', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /agent-edit\.md/i }).click();

  const previewPanel = page.getByLabel('File Preview');
  await expect(previewPanel).toBeVisible();
  await expect
    .poll(async () => {
      const box = await previewPanel.boundingBox();
      return box?.width ?? 0;
    })
    .toBeGreaterThan(800);

  await page.getByTestId('file-preview-backdrop').click({ position: { x: 32, y: 120 } });
  await expect(previewPanel).toBeHidden();
});

test('opens editable HTML in a live sandboxed split preview', async ({ page }) => {
  const escapedRequests: string[] = [];
  page.on('request', (request) => {
    if (request.url().startsWith('https://preview-leak.invalid/')) {
      escapedRequests.push(request.url());
    }
  });
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /index\.html/i }).click();
  await expect(page.getByLabel('File Preview')).toBeVisible();
  const editor = page.getByTestId('file-preview-editor');
  const previewFrame = page.getByTestId('file-preview-html-preview');
  await expect(editor).toBeVisible();
  await expect(previewFrame).toBeVisible();
  await expect(previewFrame).toHaveAttribute('sandbox', '');
  await expect(previewFrame).toHaveAttribute('referrerpolicy', 'no-referrer');
  await expect(previewFrame.contentFrame().getByRole('heading', { name: 'Original preview' })).toBeVisible();

  await editor.fill([
    '<!-- decoy <head><meta http-equiv="Content-Security-Policy" content="default-src *"></head> -->',
    '<!doctype html><html><head><style>h1 { color: rgb(20, 80, 160); }</style></head>',
    '<body><h1>Updated live preview</h1>',
    '<img src="https://preview-leak.invalid/pixel.png" alt="blocked external image">',
    '<script>parent.document.body.dataset.htmlPreviewEscaped = "true"</script></body></html>',
  ].join('\n'));

  const updatedHeading = previewFrame.contentFrame().getByRole('heading', { name: 'Updated live preview' });
  await expect(updatedHeading).toBeVisible();
  await expect(updatedHeading).toHaveCSS('color', 'rgb(20, 80, 160)');
  expect(await page.evaluate(() => document.body.dataset.htmlPreviewEscaped)).toBeUndefined();
  await expect(previewFrame.contentFrame().locator('head meta[http-equiv="Content-Security-Policy"]')).toHaveCount(1);
  expect(escapedRequests).toEqual([]);
});

test('closes file preview only after a dirty web link is confirmed and routed', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /agent-edit\.md/i }).click();
  const previewPanel = page.getByLabel('File Preview');
  await expect(previewPanel).toBeVisible();

  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await page.getByTestId('file-preview-editor').fill('# Unsaved release notes\n\n[OpenAI](https://openai.com/file-preview)');
  await page.getByRole('button', { name: 'Preview', exact: true }).click();

  page.once('dialog', async (dialog) => {
    expect(dialog.type()).toBe('confirm');
    await dialog.dismiss();
  });
  await page.getByRole('link', { name: 'OpenAI' }).click();
  await expect(previewPanel).toBeVisible();
  await expect(page.getByTestId('browser-dock')).toHaveCount(0);

  page.once('dialog', async (dialog) => {
    expect(dialog.type()).toBe('confirm');
    await dialog.accept();
  });
  await page.getByRole('link', { name: 'OpenAI' }).click();
  await expect(previewPanel).toBeHidden();
  await expect(page.getByTestId('browser-dock')).toBeVisible();
});

test('renders structured DOCX without requesting layout rendering', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /structured-report\.docx/i }).click();
  await expect(page.getByLabel('File Preview')).toBeVisible();
  await expect(page.getByTestId('file-preview-structured-document')).toBeVisible();
  await expect(page.getByTestId('file-preview-structured-document')).toContainText('Quarterly Report');
  await expect(page.getByTestId('file-preview-structured-document')).toContainText('North America');
  await expect(page.getByTestId('file-preview-rendered-content')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Layout', exact: true })).toHaveCount(0);
});

test('renders structured XLSX sheets, formulas, and extracted text fallback', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /budget\.xlsx/i }).click();
  await expect(page.getByLabel('File Preview')).toBeVisible();

  const workbook = page.getByTestId('file-preview-workbook');
  await expect(workbook).toBeVisible();
  await expect(workbook).toContainText('Summary');
  await expect(workbook).toContainText('Q1');
  await expect(workbook).toContainText('fx');
  await expect(workbook).toContainText('3');

  await page.getByRole('button', { name: 'Detail', exact: true }).click();
  await expect(workbook).toContainText('Detail row');

  await page.getByRole('button', { name: 'Extracted Text', exact: true }).click();
  await expect(page.getByTestId('file-preview-readable-content')).toContainText('Name');
  await expect(page.getByTestId('file-preview-readable-content')).toContainText('Q1');
});

test('shows the agent panel for read-only extracted Office text and routes to Python document skills', async ({ page }) => {
  await page.goto('/chat/conv-agent-edit');

  await page.getByRole('button', { name: /office-proposal\.docx/i }).click();
  await expect(page.getByLabel('File Preview')).toBeVisible();
  await expect(page.getByText('Read-only')).toBeVisible();

  const readable = page.getByTestId('file-preview-readable-content');
  await expect(readable).toContainText('softer launch statement');
  await readable.evaluate((node) => {
    const codeNodes = Array.from(node.querySelectorAll('code'));
    const target = codeNodes.find((candidate) =>
      candidate.textContent?.includes('softer launch statement'),
    );
    if (!target?.firstChild) {
      throw new Error('target text node not found');
    }
    const text = target.textContent ?? '';
    const selected = 'The team needs a softer launch statement for enterprise buyers.';
    const start = text.indexOf(selected);
    const range = document.createRange();
    range.setStart(target.firstChild, start);
    range.setEnd(target.firstChild, start + selected.length);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    node.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  });

  await expect(page.getByTestId('file-preview-agent-panel')).toBeVisible();
  await page
    .getByTestId('file-preview-agent-instruction')
    .fill('Make this more confident but still enterprise-safe.');
  await page.getByTestId('file-preview-agent-send').click();

  await expect
    .poll(() => page.evaluate(() => window.__lastAgentPrompt ?? ''), {
      timeout: 10_000,
    })
    .toContain('doc-script-editor skill');

  const prompt = await page.evaluate(() => window.__lastAgentPrompt ?? '');
  expect(prompt).toContain('File: D:\\Vault\\docs\\office-proposal.docx');
  expect(prompt).toContain('Extracted text line range: 2');
  expect(prompt).toContain('The team needs a softer launch statement for enterprise buyers.');
  expect(prompt).toContain('prepare_document_tools');
  expect(prompt).toContain('run_shell');
  expect(prompt).toContain('replace --dry-run');
  expect(prompt).not.toContain('edit_document');
  expect(prompt).not.toContain('Use edit_file to modify the file');

  await expect
    .poll(() => page.evaluate(() => window.__lastSourceIds ?? []))
    .toEqual(['src-office-docs']);
});
