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
      collectionContext: null;
      projectId: null;
      createdAt: string;
      updatedAt: string;
    };

    const nowIso = new Date().toISOString();
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversation: Conversation = {
      id: 'conv-terminal-dock',
      title: 'Terminal dock',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const terminalDiagnostics = {
      starts: [] as Array<Record<string, unknown>>,
      writes: [] as string[],
      resizes: [] as Array<Record<string, unknown>>,
      closes: [] as string[],
    };

    const emitEvent = (eventName: string, payload: Record<string, unknown>) => {
      for (const [listenerId, listener] of listeners.entries()) {
        if (listener.event !== eventName) continue;
        const callback = callbackMap.get(listener.handlerId);
        if (callback) {
          callback({ event: eventName, id: listenerId, payload });
        }
      }
    };

    const defaultAgentConfig = {
      id: 'cfg-terminal-dock',
      name: 'Terminal Dock Config',
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
        case 'list_agent_configs_cmd':
          return [clone(defaultAgentConfig)];
        case 'get_model_context_window':
          return 1047576;
        case 'get_wizard_state_cmd':
          return { completed: true, language: 'en', aiProvider: 'open_ai', sourceAdded: true };
        case 'list_conversations_cmd':
          return [clone(conversation)];
        case 'get_conversation_cmd':
          return [clone(conversation), []];
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'get_agent_run_events_cmd':
        case 'get_agent_task_run_events_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_user_memories_cmd':
        case 'list_skills_cmd':
        case 'list_mcp_servers_cmd':
        case 'list_projects_cmd':
        case 'list_personas_cmd':
          return [];
        case 'terminal_start_session_cmd': {
          terminalDiagnostics.starts.push(clone(args));
          const session = {
            id: 'terminal-session-1',
            shell: 'PowerShell',
            cwd: 'D:\\Apps\\ask_myself',
            processId: 4242,
          };
          queueMicrotask(() => {
            emitEvent('terminal:event', {
              sessionId: session.id,
              kind: 'data',
              data: 'PS D:\\Apps\\ask_myself> ',
              exitCode: null,
              signal: null,
            });
          });
          return session;
        }
        case 'terminal_write_session_cmd':
          terminalDiagnostics.writes.push(String(args.data ?? ''));
          return null;
        case 'terminal_resize_session_cmd':
          terminalDiagnostics.resizes.push(clone(args));
          return null;
        case 'terminal_close_session_cmd':
          terminalDiagnostics.closes.push(String(args.sessionId ?? ''));
          return null;
        case 'terminal_list_sessions_cmd':
          return [];
        default:
          return null;
      }
    };

    (window as unknown as { __terminalDiagnostics__: unknown }).__terminalDiagnostics__ = terminalDiagnostics;
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

test('opens an interactive terminal dock from the chat screen', async ({ page }) => {
  await page.goto('/chat/conv-terminal-dock');

  await page.getByRole('button', { name: 'Toggle terminal' }).click();

  await expect(page.locator('.xterm')).toBeVisible();
  await expect(page.getByText('Running')).toBeVisible();
  await expect(page.getByText('PowerShell #4242')).toBeVisible();

  await page.keyboard.press('Control+KeyJ');
  await expect(page.locator('.xterm')).toHaveCount(0);

  await page.keyboard.press('Control+KeyJ');
  await expect(page.locator('.xterm')).toBeVisible();

  const starts = await page.evaluate(() => {
    const diagnostics = (window as unknown as {
      __terminalDiagnostics__: { starts: Array<Record<string, unknown>> };
    }).__terminalDiagnostics__;
    return diagnostics.starts;
  });
  expect(starts).toHaveLength(1);
  expect(starts[0]).toMatchObject({
    input: {
      shell: 'default',
    },
  });

  await page.getByRole('button', { name: 'Stop terminal' }).click();
  await expect(page.getByText('Exited')).toBeVisible();

  const afterStop = await page.evaluate(() => {
    const diagnostics = (window as unknown as {
      __terminalDiagnostics__: {
        starts: Array<Record<string, unknown>>;
        closes: string[];
      };
    }).__terminalDiagnostics__;
    return {
      startCount: diagnostics.starts.length,
      closes: diagnostics.closes,
    };
  });
  expect(afterStop).toEqual({
    startCount: 1,
    closes: ['terminal-session-1'],
  });
});
