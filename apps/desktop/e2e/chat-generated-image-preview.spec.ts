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
    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const imageDataUrl =
      'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=';

    const conversation: Conversation = {
      id: 'conv-generated-image',
      title: 'Generated image',
      provider: 'open_ai',
      model: 'gpt-4.1',
      systemPrompt: '',
      collectionContext: null,
      projectId: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const messages: Message[] = [
      {
        id: 'm-user-image',
        conversationId: conversation.id,
        role: 'user',
        content: 'Generate a small blue square.',
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 0,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-image',
        conversationId: conversation.id,
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{
          id: 'call-generate-image',
          name: 'generate_image',
          arguments: JSON.stringify({
            prompt: 'A small blue square on a neutral background',
            prompt_mode: 'verbatim',
            size: '1024x1024',
          }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 1,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-image',
        conversationId: conversation.id,
        role: 'tool',
        content: 'Generated image ready for preview. It has not been saved to the workspace.',
        toolCallId: 'call-generate-image',
        toolCalls: [],
        artifacts: {
          kind: 'generatedImage',
          provider: 'OpenAI',
          model: 'gpt-image-1',
          dataUrl: imageDataUrl,
          mediaType: 'image/png',
          bytes: 68,
          prompt: 'A small blue square on a neutral background',
          requestedPrompt: 'A small blue square on a neutral background',
          promptMode: 'verbatim',
          effectivePrompt: 'A centered small blue square on a neutral background',
          promptIntegrity: 'revised',
          providerPromptEnhanced: false,
          promptRewriteObservable: true,
          suggestedFilename: 'blue-square.png',
          saved: false,
          transient: true,
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 2,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-assistant-provider-enhancement',
        conversationId: conversation.id,
        role: 'assistant',
        content: '',
        toolCallId: null,
        toolCalls: [{
          id: 'call-provider-enhancement',
          name: 'generate_image',
          arguments: JSON.stringify({
            prompt: 'A red circle on a neutral background',
            prompt_mode: 'provider_enhanced',
            size: '1024x1024',
          }),
        }],
        artifacts: null,
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 3,
        thinking: null,
        imageAttachments: null,
      },
      {
        id: 'm-tool-provider-enhancement',
        conversationId: conversation.id,
        role: 'tool',
        content: 'Generated image ready for preview. Provider prompt enhancement is unavailable for the selected provider.',
        toolCallId: 'call-provider-enhancement',
        toolCalls: [],
        artifacts: {
          kind: 'generatedImage',
          provider: 'OpenAI',
          model: 'gpt-image-1',
          dataUrl: imageDataUrl,
          mediaType: 'image/png',
          bytes: 68,
          prompt: 'A red circle on a neutral background',
          requestedPrompt: 'A red circle on a neutral background',
          promptMode: 'provider_enhanced',
          effectivePrompt: 'A red circle on a neutral background',
          promptIntegrity: 'exact',
          providerPromptEnhanced: false,
          providerPromptEnhancementRequested: true,
          providerPromptEnhancementSupported: false,
          promptRewriteObservable: false,
          suggestedFilename: 'red-circle.png',
          saved: false,
          transient: true,
        },
        tokenCount: 0,
        createdAt: nowIso,
        sortOrder: 4,
        thinking: null,
        imageAttachments: null,
      },
    ];

    const providerEnhancementConversation: Conversation = {
      ...conversation,
      id: 'conv-provider-enhancement',
      title: 'Provider enhancement unavailable',
    };
    const conversations: Record<string, Conversation> = {
      [conversation.id]: conversation,
      [providerEnhancementConversation.id]: providerEnhancementConversation,
    };
    const messagesByConversation: Record<string, Message[]> = {
      [conversation.id]: messages.slice(0, 3),
      [providerEnhancementConversation.id]: messages.slice(3).map((message) => ({
        ...message,
        conversationId: providerEnhancementConversation.id,
      })),
    };
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: (event: unknown) => void }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const invoke = async (cmd: string, args?: Record<string, unknown>) => {
      switch (cmd) {
        case 'list_conversations_cmd':
          return clone(Object.values(conversations));
        case 'get_conversation_cmd': {
          const id = String(args?.id ?? '');
          return [clone(conversations[id]), clone(messagesByConversation[id] ?? [])];
        }
        case 'get_conversation_turns_cmd':
        case 'get_agent_task_runs_cmd':
        case 'get_agent_task_run_events_cmd':
        case 'get_agent_subtask_runs_cmd':
        case 'get_agent_task_artifacts_cmd':
        case 'list_persisted_agent_task_artifacts_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_personas_cmd':
        case 'list_projects_cmd':
        case 'list_project_memories_cmd':
        case 'list_user_memories_cmd':
        case 'list_agent_procedural_memories_cmd':
          return [];
        case 'list_agent_configs_cmd':
          return [{
            id: 'cfg-default',
            name: 'Default',
            provider: 'open_ai',
            apiKey: 'sk-test',
            baseUrl: '',
            model: 'gpt-4.1',
            temperature: null,
            maxTokens: null,
            contextWindow: 128000,
            isDefault: true,
            reasoningEnabled: null,
            thinkingBudget: null,
            reasoningEffort: null,
            maxIterations: null,
            summarizationModel: null,
            summarizationProvider: null,
            imageGenerationModel: 'gpt-image-1',
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
          }];
        case 'get_model_context_window':
          return 128000;
        case 'get_agent_execution_graph_cmd':
          return { nodes: [], edges: [] };
        case 'get_app_config_cmd':
          return {
            theme: 'system',
            language: 'en',
            provider: 'open_ai',
            apiKey: '',
            baseUrl: '',
            model: 'gpt-4.1',
            embedding: { provider: 'tfidf', apiKey: '', apiBaseUrl: '', apiModel: '', localModel: '', modelPath: '', vectorDimensions: 384 },
            privacy: { allowTelemetry: false, allowCrashReports: false },
            imageGeneration: { provider: 'open_ai', apiKey: '', baseUrl: '', model: 'gpt-image-1', size: '', quality: '', outputFormat: 'png', apiStyle: 'openai_images' },
          };
        case 'get_embedder_config_cmd':
          return { provider: 'tfidf', apiKey: '', apiBaseUrl: '', apiModel: '', localModel: '', modelPath: '', vectorDimensions: 384 };
        case 'get_ocr_config_cmd':
          return { enabled: false, minConfidence: 0.5, llmFallback: false, detectionLimit: 2048, useCls: false };
        case 'check_ocr_models_cmd':
          return false;
        case 'get_conversation_stats_cmd':
          return { totalConversations: 1, totalMessages: messages.length, oldestConversation: nowIso, dbSizeBytes: 0 };
        case 'plugin:event|listen': {
          const event = String(args?.event ?? '');
          const handlerId = Number(args?.handler ?? 0);
          const eventId = listenerSeq++;
          const callback = callbackMap.get(handlerId);
          if (callback) listeners.set(eventId, { event, handler: callback });
          return eventId;
        }
        case 'plugin:dialog|save':
          return 'D:\\Exports\\blue-square.png';
        case 'save_generated_image_cmd': {
          (window as unknown as { __savedGeneratedImage?: unknown }).__savedGeneratedImage = clone(args?.input);
          const input = args?.input as { outputPath?: string } | undefined;
          return { path: input?.outputPath ?? '', bytesWritten: 68 };
        }
        case 'compact_conversation_cmd':
        case 'save_agent_config_cmd':
        case 'agent_stop_cmd':
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
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test('generate_image renders an unsaved preview and saves only when requested', async ({ page }) => {
  await page.goto('/chat/conv-generated-image');

  const preview = page.getByTestId('generated-image-preview').first();
  await expect(preview).toBeVisible();
  await expect(preview.getByTestId('generated-image-img')).toBeVisible();
  await expect(preview).toContainText('Unsaved preview');
  await expect(preview).toContainText('OpenAI');
  await expect(preview).toContainText('PNG');
  await expect(preview.getByTestId('generated-image-prompt-mode')).toHaveText('Verbatim');
  await expect(preview.getByTestId('generated-image-requested-prompt'))
    .toContainText('A small blue square on a neutral background');
  await expect(preview.getByTestId('generated-image-effective-prompt'))
    .toContainText('A centered small blue square on a neutral background');
  await expect(preview.getByTestId('generated-image-prompt-revised'))
    .toContainText('Provider reported a revised prompt.');

  await page.screenshot({ path: 'test-results/generated-image-prompt-audit.png', fullPage: true });

  await preview.getByRole('button', { name: 'Save as...' }).click();
  await expect(preview).toContainText('D:\\Exports\\blue-square.png');

  const saved = await page.evaluate(() =>
    (window as unknown as { __savedGeneratedImage?: { outputPath?: string; dataUrl?: string; sourcePath?: string | null } }).__savedGeneratedImage,
  );
  expect(saved?.outputPath).toBe('D:\\Exports\\blue-square.png');
  expect(saved?.sourcePath).toBeNull();
  expect(saved?.dataUrl).toContain('data:image/png;base64,');
});

test('generate_image reports when requested provider enhancement is unavailable', async ({ page }) => {
  await page.goto('/chat/conv-provider-enhancement');

  const preview = page.getByTestId('generated-image-preview').first();
  await expect(preview).toBeVisible();
  await expect(preview.getByTestId('generated-image-prompt-mode'))
    .toHaveText('Provider enhancement unavailable');
});
