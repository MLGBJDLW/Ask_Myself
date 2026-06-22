import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('last-route', '/tasks');

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let stopCount = 0;
    let openFileCount = 0;

    const runningTask = {
      run: {
        id: 'run-live',
        conversationId: 'conv-live',
        turnId: 'turn-live',
        userMessageId: 'msg-live',
        status: 'running',
        phase: 'tooling',
        title: 'Prepare board brief',
        routeKind: 'workflow',
        summary: 'Researcher and verifier are active',
        errorMessage: null,
        provider: 'open_ai',
        model: 'gpt-4.1',
        plan: null,
        artifacts: {
          kind: 'brief',
          verification: { kind: 'verification' },
          files: [{ path: 'board-brief.docx' }],
        },
        createdAt: nowIso,
        updatedAt: nowIso,
        startedAt: nowIso,
        finishedAt: null,
      },
      conversationTitle: 'Board launch thread',
      projectId: 'proj-launch',
      projectName: 'Launch plan',
      userMessagePreview: 'Research the launch risks and prepare a board-ready brief.',
      eventCount: 3,
      subtaskTotal: 3,
      subtaskCompleted: 1,
      subtaskFailed: 0,
      subtaskRunning: 2,
      artifactKinds: ['brief', 'files', 'verification'],
    };
    const failedTask = {
      run: {
        ...runningTask.run,
        id: 'run-failed',
        conversationId: 'conv-failed',
        turnId: 'turn-failed',
        userMessageId: 'msg-failed',
        status: 'failed',
        phase: 'done',
        title: 'Verify market claims',
        summary: 'Verifier blocked the result',
        errorMessage: 'Missing citations',
        finishedAt: nowIso,
      },
      conversationTitle: 'Evidence cleanup',
      projectId: null,
      projectName: null,
      userMessagePreview: 'Verify the market claims again with citations.',
      eventCount: 2,
      subtaskTotal: 2,
      subtaskCompleted: 1,
      subtaskFailed: 1,
      subtaskRunning: 0,
      artifactKinds: ['verification'],
    };
    const events = [
      { id: 'event-1', runId: 'run-live', eventType: 'status', label: 'Task queued', status: 'queued', payload: null, createdAt: nowIso },
      { id: 'event-2', runId: 'run-live', eventType: 'status', label: 'Researcher started', status: 'running', payload: null, createdAt: nowIso },
      { id: 'event-3', runId: 'run-live', eventType: 'tool', label: 'record_verification', status: 'completed', payload: null, createdAt: nowIso },
    ];
    const subtasks = [
      { id: 'sub-1', parentRunId: 'run-live', label: 'Collect evidence', role: 'Researcher', status: 'completed', phase: 'done', input: null, output: { summary: 'Evidence collected' }, errorMessage: null, tokenBudget: 1600, createdAt: nowIso, updatedAt: nowIso, startedAt: nowIso, finishedAt: nowIso },
      { id: 'sub-2', parentRunId: 'run-live', label: 'Check citations', role: 'Verifier', status: 'running', phase: 'verification', input: null, output: null, errorMessage: null, tokenBudget: 900, createdAt: nowIso, updatedAt: nowIso, startedAt: nowIso, finishedAt: null },
      { id: 'sub-3', parentRunId: 'run-live', label: 'Challenge assumptions', role: 'Critic', status: 'queued', phase: 'queued', input: null, output: null, errorMessage: null, tokenBudget: 700, createdAt: nowIso, updatedAt: nowIso, startedAt: null, finishedAt: null },
    ];
    const graph = {
      runId: 'run-live',
      nodes: [
        { id: 'run-live', nodeType: 'supervisor', label: 'Prepare board brief', role: 'Supervisor', status: 'running', phase: 'tooling', summary: 'Researcher and verifier are active', errorMessage: null, input: null, output: null, tokenBudget: null, startedAt: nowIso, finishedAt: null },
        { id: 'sub-1', nodeType: 'subtask', label: 'Collect evidence', role: 'Researcher', status: 'completed', phase: 'done', summary: 'Evidence collected', errorMessage: null, input: null, output: { summary: 'Evidence collected' }, tokenBudget: 1600, startedAt: nowIso, finishedAt: nowIso },
        { id: 'sub-2', nodeType: 'subtask', label: 'Check citations', role: 'Verifier', status: 'running', phase: 'verification', summary: null, errorMessage: null, input: null, output: null, tokenBudget: 900, startedAt: nowIso, finishedAt: null },
        { id: 'sub-3', nodeType: 'subtask', label: 'Challenge assumptions', role: 'Critic', status: 'queued', phase: 'queued', summary: null, errorMessage: null, input: null, output: null, tokenBudget: 700, startedAt: null, finishedAt: null },
      ],
      edges: [
        { from: 'run-live', to: 'sub-1', label: 'delegates' },
        { from: 'run-live', to: 'sub-2', label: 'delegates' },
        { from: 'run-live', to: 'sub-3', label: 'delegates' },
      ],
    };
    const artifactSummaries = [
      { id: 'run-live:root', runId: 'run-live', kind: 'brief', title: 'Prepare board brief', summary: 'Board-ready launch brief', paths: ['board-brief.docx'], source: 'task_run', createdAt: nowIso, payload: runningTask.run.artifacts },
      { id: 'run-live:files', runId: 'run-live', kind: 'files', title: 'Files', summary: '1 item(s)', paths: ['board-brief.docx'], source: 'task_run', createdAt: nowIso, payload: [{ path: 'board-brief.docx' }] },
      { id: 'run-live:verification', runId: 'run-live', kind: 'verification', title: 'Verification', summary: 'Claims checked', paths: [], source: 'task_run', createdAt: nowIso, payload: { kind: 'verification' } },
    ];
    let savedArtifacts = [
      {
        id: 'artifact-1',
        runId: 'run-live',
        kind: 'brief',
        title: 'Editable board brief',
        summary: 'Saved editable draft',
        content: 'Draft v1',
        paths: ['board-brief.docx'],
        payload: { format: 'docx' },
        source: 'task_center',
        version: 1,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];
    let artifactVersions: Record<string, unknown[]> = {
      'artifact-1': [
        {
          id: 'artifact-1-v1',
          artifactId: 'artifact-1',
          version: 1,
          title: 'Editable board brief',
          summary: 'Saved editable draft',
          content: 'Draft v1',
          paths: ['board-brief.docx'],
          payload: { format: 'docx' },
          createdAt: nowIso,
        },
      ],
    };
    let artifactSeq = 2;
    let artifactUpdateCount = 0;
    const toolAccess = [
      { name: 'run_shell', category: 'system', canRead: true, canWrite: true, canExecute: true, canAccessNetwork: true, needsApproval: true, riskLevel: 'high', riskReason: 'Executes local shell commands.' },
      { name: 'edit_file', category: 'filesystem', canRead: true, canWrite: true, canExecute: false, canAccessNetwork: false, needsApproval: true, riskLevel: 'high', riskReason: 'Modifies files.' },
      { name: 'fetch_url', category: 'web', canRead: true, canWrite: false, canExecute: false, canAccessNetwork: true, needsApproval: false, riskLevel: 'low', riskReason: 'Reads remote URLs.' },
      { name: 'get_document_info', category: 'document_analysis', canRead: true, canWrite: false, canExecute: false, canAccessNetwork: false, needsApproval: false, riskLevel: 'low', riskReason: 'Reads Office/PDF metadata.' },
    ];

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'plugin:app|version':
          return '0.2.9';
        case 'check_update_from_source_cmd':
          return null;
        case 'plugin:updater|check':
          return null;
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
          return { completed: true };
        case 'list_recent_agent_task_runs_cmd':
          return [clone(runningTask), clone(failedTask)];
        case 'get_agent_task_run_events_cmd':
          return clone(events);
        case 'get_agent_subtask_runs_cmd':
          return clone(subtasks);
        case 'get_agent_execution_graph_cmd':
          return clone(graph);
        case 'get_agent_task_artifacts_cmd':
          return clone(artifactSummaries);
        case 'list_persisted_agent_task_artifacts_cmd':
          return clone(savedArtifacts.filter((artifact) => artifact.runId === String(args.runId ?? '')));
        case 'list_agent_task_artifact_versions_cmd':
          return clone(artifactVersions[String(args.artifactId ?? '')] ?? []);
        case 'create_agent_task_artifact_cmd': {
          const input = args.input as Record<string, unknown>;
          const createdAt = new Date().toISOString();
          const created = {
            id: `artifact-${artifactSeq++}`,
            runId: String(args.runId ?? ''),
            kind: String(input.kind ?? 'artifact'),
            title: String(input.title ?? 'Artifact'),
            summary: input.summary == null ? null : String(input.summary),
            content: String(input.content ?? ''),
            paths: Array.isArray(input.paths) ? input.paths : [],
            payload: input.payload ?? null,
            source: String(input.source ?? 'task_center'),
            version: 1,
            createdAt,
            updatedAt: createdAt,
          };
          savedArtifacts = [created, ...savedArtifacts];
          artifactVersions[created.id] = [
            {
              id: `${created.id}-v1`,
              artifactId: created.id,
              version: 1,
              title: created.title,
              summary: created.summary,
              content: created.content,
              paths: created.paths,
              payload: created.payload,
              createdAt,
            },
          ];
          return clone(created);
        }
        case 'update_agent_task_artifact_cmd': {
          const input = args.input as Record<string, unknown>;
          const id = String(args.artifactId ?? '');
          const updatedAt = new Date().toISOString();
          const existing = savedArtifacts.find((artifact) => artifact.id === id);
          if (!existing) return null;
          const updated = {
            ...existing,
            title: String(input.title ?? existing.title),
            summary: input.summary == null ? null : String(input.summary),
            content: String(input.content ?? existing.content),
            paths: Array.isArray(input.paths) ? input.paths : existing.paths,
            payload: input.payload ?? null,
            version: existing.version + 1,
            updatedAt,
          };
          savedArtifacts = savedArtifacts.map((artifact) => (artifact.id === id ? updated : artifact));
          artifactVersions[id] = [
            {
              id: `${id}-v${updated.version}`,
              artifactId: id,
              version: updated.version,
              title: updated.title,
              summary: updated.summary,
              content: updated.content,
              paths: updated.paths,
              payload: updated.payload,
              createdAt: updatedAt,
            },
            ...(artifactVersions[id] ?? []),
          ];
          artifactUpdateCount += 1;
          (window as unknown as { __artifactUpdateCount: number }).__artifactUpdateCount = artifactUpdateCount;
          return clone(updated);
        }
        case 'list_tool_access_map_cmd':
          return clone(toolAccess);
        case 'list_tool_approval_policies_cmd':
          return {
            session: [{ toolName: 'run_shell', decision: 'allow_session' }],
            persisted: [{ toolName: 'edit_file', decision: 'never', createdAt: nowIso }],
          };
        case 'list_project_memories_cmd':
          return [
            {
              id: 'mem-1',
              projectId: 'proj-launch',
              kind: 'decision',
              title: 'Board format',
              content: 'Use a concise board-ready brief with risks first.',
              source: 'manual',
              pinned: true,
              archived: false,
              confidence: 0.9,
              expiresAt: null,
              conflictStatus: 'clear',
              createdAt: nowIso,
              updatedAt: nowIso,
            },
          ];
        case 'create_project_memory_cmd':
          return {
            id: 'mem-created',
            projectId: String(args.projectId ?? ''),
            kind: 'decision',
            title: 'Saved task memory',
            content: 'Saved',
            source: 'task_center',
            pinned: true,
            archived: false,
            confidence: 0.8,
            expiresAt: null,
            conflictStatus: 'clear',
            createdAt: nowIso,
            updatedAt: nowIso,
          };
        case 'agent_stop_cmd':
          stopCount += 1;
          (window as unknown as { __taskStopCount: number }).__taskStopCount = stopCount;
          return null;
        case 'open_file_in_default_app':
          openFileCount += 1;
          (window as unknown as { __openFileCount: number }).__openFileCount = openFileCount;
          return null;
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
    (window as unknown as { __taskStopCount: number }).__taskStopCount = 0;
    (window as unknown as { __openFileCount: number }).__openFileCount = 0;
    (window as unknown as { __artifactUpdateCount: number }).__artifactUpdateCount = 0;
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('task center manages runs, graph, project memory, artifacts, and risk map', async ({ page }) => {
  await page.goto('/tasks');

  await expect(page.getByRole('heading', { name: 'Task Center' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Prepare board brief' })).toBeVisible();
  await expect(page.getByText('Verify market claims')).toBeVisible();
  await expect(page.getByText('Execution Graph', { exact: true })).toBeVisible();
  await expect(page.locator('body')).toContainText('Collect evidence');
  await expect(page.locator('body')).toContainText('Check citations');
  await expect(page.locator('body')).toContainText('Challenge assumptions');
  await expect(page.getByText('Artifacts', { exact: true })).toBeVisible();
  await expect(page.getByText('brief').first()).toBeVisible();
  await expect(page.getByText('board-brief.docx')).toBeVisible();
  await page.getByRole('button', { name: 'Open file: board-brief.docx' }).click();
  await page.waitForFunction(
    () => (window as unknown as { __openFileCount?: number }).__openFileCount === 1,
    undefined,
    { timeout: 5000 },
  );
  await expect(page.getByText('Saved Artifacts', { exact: true })).toBeVisible();
  await expect(page.getByText('Editable board brief')).toBeVisible();
  await page.getByRole('button', { name: 'Save as editable' }).first().click();
  await expect(page.getByLabel('Title')).toHaveValue('Prepare board brief');
  await page.getByLabel('Content').fill('Updated board brief content');
  await page.getByRole('button', { name: 'Save artifact' }).click();
  await page.waitForFunction(
    () => (window as unknown as { __artifactUpdateCount?: number }).__artifactUpdateCount === 1,
    undefined,
    { timeout: 5000 },
  );
  await expect(page.locator('body')).toContainText('v2');
  await expect(page.getByText('Project Memory', { exact: true })).toBeVisible();
  await expect(page.getByText('Board format')).toBeVisible();
  await expect(page.getByText('Tool Risk Map', { exact: true })).toBeVisible();
  await expect(page.getByText('run_shell')).toBeVisible();
  await expect(page.getByText('get_document_info')).toBeVisible();
  await expect(page.locator('body')).toContainText('Policy: allow session');
  await expect(page.locator('body')).toContainText('Policy: deny forever');

  await page.getByRole('button', { name: 'Cancel task' }).click();
  await page.waitForFunction(
    () => (window as unknown as { __taskStopCount?: number }).__taskStopCount === 1,
    undefined,
    { timeout: 5000 },
  );
});
