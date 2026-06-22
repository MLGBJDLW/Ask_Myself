import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('last-route', '/workflows');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const savedAutomationInputs: Array<Record<string, unknown>> = [];
    (window as unknown as { __savedAutomationInputs?: Array<Record<string, unknown>> }).__savedAutomationInputs = savedAutomationInputs;

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
          savedAutomationInputs.push(input);
          return {
            ...(input as Record<string, unknown>),
            id: 'automation-recorded',
            triggerKind: 'manual',
            status: 'ready',
            lastRunAt: null,
            nextRunAt: null,
            createdAt: nowIso,
            updatedAt: nowIso,
          };
        }
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
