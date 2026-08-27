import { expect, type Locator, test } from '@playwright/test';

async function selectNexaOption(trigger: Locator, value: string) {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`).click();
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('last-route', '/workflows');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const savedAutomationInputs: Array<Record<string, unknown>> = [];
    (window as unknown as { __savedAutomationInputs?: Array<Record<string, unknown>> }).__savedAutomationInputs = savedAutomationInputs;
    const schedulePreviewInputs: Array<Record<string, unknown>> = [];
    (window as unknown as { __schedulePreviewInputs?: Array<Record<string, unknown>> }).__schedulePreviewInputs = schedulePreviewInputs;

    const workflowCatalog = [
      {
        id: 'connector_background',
        label: 'Connector + Background Task',
        description: 'Assess connector setup and background-task lifecycle risks.',
        maxParallel: 3,
        promptTemplate: 'Run the Connector + Background Task workflow for this local automation goal:\n\nGoal:\n',
        tasks: [
          {
            id: 'plan',
            roleId: 'planner',
            roleLabel: 'Planner',
            task: 'Plan the workflow.',
            expectedOutput: 'Plan.',
            deliverableStyle: 'workflow plan',
            acceptanceCriteria: ['Keep approvals explicit.'],
          },
        ],
      },
    ];
    const sources = [
      {
        id: 'source-1',
        kind: 'local_folder',
        rootPath: 'D:\\Reports',
        includeGlobs: ['**/*'],
        excludeGlobs: [],
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];
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
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_workflow_templates_cmd':
          return clone(workflowCatalog);
        case 'list_workflow_automations_cmd':
        case 'list_due_workflow_automations_cmd':
          return [];
        case 'list_sources':
          return clone(sources);
        case 'list_agent_configs_cmd':
          return [{
            id: 'cfg-scheduled',
            name: 'Scheduled GLM',
            provider: 'alibaba_model_studio',
            providerEndpointId: 'text:alibaba-model-studio',
            baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
            model: 'ZHIPU/GLM-5.3',
            contextWindow: null,
            isDefault: true,
          }];
        case 'get_learning_governance_snapshot_cmd':
          return {
            skillStats: [],
            pendingProposals: 0,
            proceduralMemoryCount: 0,
            memoryInjectionCount: 0,
            recommendations: [],
          };
        case 'save_workflow_automation_cmd': {
          const input = clone(args.input as Record<string, unknown>);
          const scheduleConfig = clone(args.scheduleConfig as Record<string, unknown> | null);
          savedAutomationInputs.push({ ...input, scheduleConfig });
          return {
            ...(input as Record<string, unknown>),
            scheduleConfig,
            id: 'automation-recorded',
            triggerKind: 'manual',
            status: 'ready',
            lastRunAt: null,
            nextRunAt: null,
            createdAt: nowIso,
            updatedAt: nowIso,
          };
        }
        case 'preview_workflow_automation_schedule_cmd':
          schedulePreviewInputs.push(clone(args));
          return {
            cron: String(args.cron),
            timezone: String(args.timezone),
            occurrences: [1, 2, 3, 4, 5].map((day) => ({
              scheduledFor: `2026-09-0${day}T01:00:00Z`,
              localTime: `2026-09-0${day}T09:00:00+08:00[Asia/Shanghai]`,
            })),
          };
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
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('recorded workflow can be previewed and saved as an automation', async ({ page }) => {
  await page.goto('/workflows');

  await page.getByRole('button', { name: 'Record & Replay' }).click();
  await page.getByPlaceholder('Weekly report export').fill('Weekly report export');
  await page.getByPlaceholder('What the workflow should accomplish').fill('Download the recurring weekly report and summarize anomalies.');
  await page.getByPlaceholder('client\nperiod\noutput format').fill('period\nrecipient');
  await page.getByPlaceholder('client = Acme\nperiod = last week').fill('period = 2026-W25');
  await page.getByPlaceholder('Open report page and export CSV').fill('Open the report page, choose the weekly date range, and export CSV.');
  await page.getByPlaceholder('Export is complete\nResult was verified').fill('CSV exported\nSummary includes anomalies');

  await expect(page.getByText('Replay this recorded workflow as an adaptable Nexa procedure.')).toBeVisible();
  await expect(page.getByText('Workflow: Weekly report export')).toBeVisible();
  await expect(page.getByText('Runtime values for this replay')).toBeVisible();

  await page.getByRole('button', { name: 'Save as automation' }).click();

  const saved = await page.evaluate(() => (
    window as unknown as { __savedAutomationInputs?: Array<Record<string, unknown>> }
  ).__savedAutomationInputs?.[0]);
  expect(saved?.name).toBe('Weekly report export');
  expect(saved?.workflowTemplateId).toBe('connector_background');
  expect(saved?.trigger).toEqual({ kind: 'manual' });
  expect(String(saved?.prompt)).toContain('Workflow: Weekly report export');
  expect(String(saved?.prompt)).toContain('Success criteria');
});

test('scheduled workflow previews timezone and saves provider-managed execution policy', async ({ page }) => {
  await page.goto('/workflows');
  await page.getByRole('button', { name: 'Automations' }).click();

  await page.getByLabel('Name').fill('Weekday GLM review');
  await page.getByLabel('Description').fill('Review the latest report every weekday.');
  await page.getByLabel('Prompt').fill('Review the latest report and return only material changes.');
  await page.getByRole('button', { name: 'Schedule', exact: true }).click();
  await page.getByLabel('Cron').fill('0 9 * * 1-5');
  await page.getByLabel('Timezone (IANA)').fill('Asia/Shanghai');

  await expect(page.getByText('Next 5 runs')).toBeVisible();
  await expect(page.getByText('2026-09-01T09:00:00+08:00[Asia/Shanghai]')).toBeVisible();
  await selectNexaOption(
    page.getByText('Agent configuration', { exact: true }).locator('..').locator('[data-nexa-select-trigger]'),
    'cfg-scheduled',
  );
  await expect(page.getByLabel('Model override')).toHaveValue('ZHIPU/GLM-5.3');
  await expect(page.getByLabel('Context window')).toHaveValue('');
  await expect(page.getByLabel('Context window')).toHaveAttribute('placeholder', 'Provider default (Auto)');
  await selectNexaOption(
    page.getByText('Power mode', { exact: true }).locator('..').locator('[data-nexa-select-trigger]'),
    'nexus',
  );
  await selectNexaOption(
    page.getByText('Orchestration profile', { exact: true }).locator('..').locator('[data-nexa-select-trigger]'),
    'codeUltra',
  );
  await page.getByLabel('Approval before run').uncheck();
  await page.getByLabel('Allowed tools').fill('read_file, web_search');
  await page.getByRole('button', { name: 'Save automation' }).click();

  const state = await page.evaluate(() => ({
    saved: (window as unknown as { __savedAutomationInputs?: Array<Record<string, unknown>> })
      .__savedAutomationInputs?.[0],
    previews: (window as unknown as { __schedulePreviewInputs?: Array<Record<string, unknown>> })
      .__schedulePreviewInputs,
  }));
  expect(state.previews?.at(-1)).toMatchObject({
    cron: '0 9 * * 1-5',
    timezone: 'Asia/Shanghai',
    limit: 5,
  });
  expect(state.saved?.trigger).toEqual({ kind: 'schedule', cron: '0 9 * * 1-5' });
  expect(state.saved?.approvalPolicy).toEqual({
    requireBeforeRun: false,
    allowedTools: ['read_file', 'web_search'],
    riskLevel: 'medium',
  });
  expect(state.saved?.scheduleConfig).toMatchObject({
    version: 2,
    timezone: 'Asia/Shanghai',
    misfirePolicy: 'run_latest',
    overlapPolicy: 'skip',
    legacyNeedsReview: false,
    executionPolicy: {
      agentConfigId: 'cfg-scheduled',
      provider: 'alibaba_model_studio',
      providerEndpointId: 'text:alibaba-model-studio',
      model: 'ZHIPU/GLM-5.3',
      contextWindow: null,
      powerMode: 'nexus',
      orchestrationProfile: 'codeUltra',
    },
  });
});
