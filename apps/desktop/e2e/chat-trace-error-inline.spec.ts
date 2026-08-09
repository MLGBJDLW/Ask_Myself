import { expect, test } from '@playwright/test';
import { RUN_EVENT_FIXTURE_INIT_SCRIPT } from './run-event-fixture';

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ content: RUN_EVENT_FIXTURE_INIT_SCRIPT });
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: number }).__ASK_STREAM_TIMEOUT_MS__ = 120;
    history.replaceState(
      { usr: { initialMessage: 'Why did the connection fail?' }, key: 'e2e-inline-error', idx: 0 },
      '',
      '/chat',
    );

    type Conversation = {
      id: string;
      title: string;
      provider: string;
      model: string;
      systemPrompt: string;
      createdAt: string;
      updatedAt: string;
    };

    const nowIso = new Date().toISOString();
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();

    const emitEvent = (eventName: string, payload: Record<string, unknown>) => {
      const convert = (window as unknown as {
        __toRunEventFixture?: (
          name: string,
          value: Record<string, unknown>,
        ) => { eventName: string; payload: Record<string, unknown> };
      }).__toRunEventFixture;
      const converted = convert?.(eventName, payload);
      if (converted) {
        eventName = converted.eventName;
        payload = converted.payload;
      }
      for (const [listenerId, listener] of listeners.entries()) {
        if (listener.event !== eventName) continue;
        const callback = callbackMap.get(listener.handlerId);
        if (callback) {
          callback({ event: eventName, id: listenerId, payload });
        }
      }
    };

    const defaultAgentConfig = {
      id: 'cfg-inline-error',
      name: 'Inline Error Config',
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

    const conversations: Record<string, Conversation> = {};

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
        case 'list_agent_configs_cmd':
          return [JSON.parse(JSON.stringify(defaultAgentConfig))];
        case 'get_model_context_window':
          return 1047576;
        case 'list_conversations_cmd':
          return Object.values(conversations).map(item => JSON.parse(JSON.stringify(item)));
        case 'create_conversation_cmd': {
          const id = 'conv-inline-error';
          const conversation: Conversation = {
            id,
            title: 'Inline Error',
            provider: String(args.provider ?? 'open_ai'),
            model: String(args.model ?? 'gpt-4.1'),
            systemPrompt: String(args.systemPrompt ?? ''),
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          };
          conversations[id] = conversation;
          return JSON.parse(JSON.stringify(conversation));
        }
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [JSON.parse(JSON.stringify(conversations[id])), []];
        }
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
          return JSON.parse(JSON.stringify(defaultAgentConfig));
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
        case 'agent_chat_cmd': {
          const conversationId = String(args.conversationId ?? '');
          queueMicrotask(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'thinking',
              content: 'Investigating the failing connection path.',
            });
          });
          setTimeout(() => {
            emitEvent('agent://run-event', {
              conversationId,
              type: 'toolCallStart',
              callId: 'tool-inline-error',
              toolName: 'search_knowledge_base',
              arguments: JSON.stringify({ query: 'connection lost' }),
            });
          }, 30);
          return {
            sessionId: 'session-inline-error',
            runId: 'run-inline-error',
            turnId: 'turn-inline-error',
            state: 'running',
          };
        }
        case 'get_agent_task_runs_cmd':
          return [{
            id: 'run-inline-error',
            conversationId: 'conv-inline-error',
            turnId: 'turn-inline-error',
            userMessageId: 'user-inline-error',
            status: 'failed',
            phase: 'done',
            title: 'Inline error recovery',
            errorMessage: 'Backend confirmed worker failure.',
            provider: 'open_ai',
            model: 'gpt-4.1',
            createdAt: nowIso,
            updatedAt: new Date().toISOString(),
          }];
        case 'get_agent_run_events_cmd':
          return [{
            version: 2,
            runId: 'run-inline-error',
            turnId: 'turn-inline-error',
            eventSeq: 1,
            kind: 'error',
            phase: 'done',
            visibility: 'user',
            persistence: 'durable',
            displayKind: 'error',
            importance: 'high',
            label: 'Backend confirmed worker failure.',
            status: 'failed',
            payload: { message: 'Backend confirmed worker failure.' },
            createdAt: new Date().toISOString(),
          }];
        case 'get_agent_task_run_events_cmd':
          return [];
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

test('recovers a stalled stream from confirmed durable backend failure', async ({ page }) => {
  await page.goto('/chat');

  await expect(page.getByText('Investigating the failing connection path.').first()).toBeVisible();
  await expect(page.getByLabel('Chat messages').getByTestId('tool-call-card')).toBeVisible();
  await expect(page.getByText('Backend confirmed worker failure.', { exact: true })).toBeVisible();
  await expect(page.getByText('Connection lost', { exact: true })).toHaveCount(0);
  await expect(page.getByText('An error occurred')).toHaveCount(0);
});
