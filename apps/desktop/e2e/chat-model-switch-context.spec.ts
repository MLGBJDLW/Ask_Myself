import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem(
      'chat-token-usage-v1',
      JSON.stringify({
        'conv-model-switch': {
          promptTokens: 10000,
          completionTokens: 250,
          totalTokens: 10250,
          thinkingTokens: 0,
          lastPromptTokens: 10000,
          updatedAt: Date.now(),
        },
      }),
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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const conversations: Record<string, Conversation> = {
      'conv-model-switch': {
        id: 'conv-model-switch',
        title: 'Model Switch Context',
        provider: 'open_ai',
        model: 'tiny-context',
        systemPrompt: '',
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    };

    let configs = [
      {
        id: 'cfg-tiny',
        name: 'Tiny Context',
        provider: 'open_ai',
        apiKey: '',
        baseUrl: null,
        model: 'tiny-context',
        temperature: 0.3,
        maxTokens: 4096,
        contextWindow: 16384,
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
      },
      {
        id: 'cfg-large',
        name: 'Large Context',
        provider: 'open_ai',
        apiKey: '',
        baseUrl: null,
        model: 'large-context',
        temperature: 0.3,
        maxTokens: 4096,
        contextWindow: 1000000,
        isDefault: false,
        reasoningEnabled: null,
        thinkingBudget: null,
        reasoningEffort: null,
        maxIterations: null,
        summarizationModel: null,
        summarizationProvider: null,
        subagentAllowedTools: null,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null }).__lastAgentChatArgs = null;

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'get_wizard_state_cmd':
          return { completed: true };
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
          return configs.map(clone);
        case 'set_default_agent_config_cmd': {
          const id = String(args.id ?? '');
          configs = configs.map((config) => ({
            ...config,
            isDefault: config.id === id,
          }));
          return null;
        }
        case 'update_conversation_model_cmd': {
          const id = String(args.id ?? '');
          const conversation = conversations[id];
          if (!conversation) return null;
          conversation.provider = String(args.provider ?? conversation.provider);
          conversation.model = String(args.model ?? conversation.model);
          conversation.updatedAt = new Date().toISOString();
          return clone(conversation);
        }
        case 'get_model_context_window': {
          const model = String(args.model ?? '');
          return model === 'large-context' ? 1000000 : 16384;
        }
        case 'list_conversations_cmd':
          return Object.values(conversations).map(clone);
        case 'list_projects_cmd':
          return [];
        case 'get_conversation_cmd': {
          const id = String(args.id ?? '');
          return [clone(conversations[id]), []];
        }
        case 'get_conversation_turns_cmd':
          return [];
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
        case 'agent_chat_cmd':
          (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null }).__lastAgentChatArgs = clone(args);
          return null;
        case 'agent_stop_cmd':
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
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('model selector and context usage follow the active chat model', async ({ page }) => {
  await page.goto('/chat/conv-model-switch');

  await expect(page.getByText('61% context used').first()).toBeVisible();
  const modelSelect = page.getByLabel('Default Model');
  await expect(modelSelect).toHaveValue('cfg-tiny');

  await modelSelect.selectOption('cfg-large');

  await expect(modelSelect).toHaveValue('cfg-large');
  await expect(modelSelect).toHaveAttribute('title', 'open_ai / large-context');
  await expect(page.getByText('1% context used').first()).toBeVisible();
  await expect(page.getByText('61% context used')).toHaveCount(0);

  await page.getByTestId('chat-input-textarea').fill('Use the selected model.');
  await page.getByTestId('chat-send').click();

  await expect.poll(async () =>
    page.evaluate(() =>
      (window as unknown as { __lastAgentChatArgs?: Record<string, unknown> | null })
        .__lastAgentChatArgs?.agentConfigId,
    ),
  ).toBe('cfg-large');
});
