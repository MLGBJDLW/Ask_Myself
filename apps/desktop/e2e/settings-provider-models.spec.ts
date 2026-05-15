import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-locale", "en");

    const nowIso = new Date().toISOString();
    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;

    const anthropicConfig = {
      id: "cfg-anthropic",
      name: "Anthropic Team",
      provider: "anthropic",
      apiKey: "sk-ant-demo",
      baseUrl: "https://api.anthropic.com/v1",
      model: "claude-sonnet-4-6",
      temperature: 0.3,
      maxTokens: 4096,
      contextWindow: 200000,
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
    };

    const qwenConfig = {
      ...anthropicConfig,
      id: "cfg-qwen",
      name: "Qwen CN",
      provider: "qwen",
      apiKey: "sk-qwen-demo",
      baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      model: "qwen3.6-plus",
      isDefault: true,
    };

    const embedderConfig = {
      provider: "tfidf",
      apiKey: "",
      apiBaseUrl: "",
      apiModel: "",
      localModel: "",
      modelPath: "",
      vectorDimensions: 384,
    };

    const ocrConfig = {
      enabled: false,
      minConfidence: 0.5,
      llmFallback: false,
      detectionLimit: 2048,
      useCls: false,
    };

    const appConfig = {
      toolTimeoutSecs: 30,
      agentTimeoutSecs: 180,
      cacheTtlHours: 24,
      defaultSearchLimit: 20,
      minSearchSimilarity: 0.2,
      maxTextFileSize: 104857600,
      maxVideoFileSize: 2147483648,
      maxAudioFileSize: 536870912,
      llmTimeoutSecs: 300,
      mcpCallTimeoutSecs: 60,
      confirmDestructive: true,
      shellAccessMode: "restricted",
      toolApprovalMode: "ask",
      hfMirrorBaseUrl: "https://hf-mirror.com",
      ghproxyBaseUrl: "https://mirror.ghproxy.com",
      imageGeneration: {
        provider: "open_ai",
        apiStyle: "openai_images",
        apiKey: "",
        baseUrl: "https://api.openai.com/v1",
        model: "gpt-image-2",
        size: "1024x1024",
        quality: null,
        outputFormat: "png",
      },
    };

    const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

    const invoke = async (cmd: string, _args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:event|listen": {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(_args.event ?? ""),
            handlerId: Number(_args.handler ?? 0),
          });
          return listenerId;
        }
        case "plugin:event|unlisten": {
          listeners.delete(Number(_args.eventId ?? 0));
          return null;
        }
        case "get_wizard_state_cmd":
          return { completed: true };
        case "list_agent_configs_cmd":
          return [clone(anthropicConfig), clone(qwenConfig)];
        case "list_conversations_cmd":
          return [];
        case "list_sources":
        case "get_conversation_sources_cmd":
        case "list_checkpoints_cmd":
        case "list_user_memories_cmd":
        case "list_skills_cmd":
        case "list_mcp_servers_cmd":
          return [];
        case "set_conversation_sources_cmd":
        case "update_conversation_system_prompt_cmd":
        case "compact_conversation_cmd":
        case "agent_stop_cmd":
          return null;
        case "get_index_stats":
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case "get_privacy_config":
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case "get_embedder_config_cmd":
          return clone(embedderConfig);
        case "get_app_config_cmd":
          return clone(appConfig);
        case "save_app_config_cmd":
          (window as unknown as { __savedAppConfig?: unknown }).__savedAppConfig = clone(
            _args.config,
          );
          return null;
        case "list_builtin_plugins_cmd":
          return [
            {
              id: "image-generation",
              name: "Image Generation",
              capability: "Image creation",
              description: "Routes image requests through provider-specific adapters.",
              builtIn: true,
              tools: ["generate_image"],
              settingsSurfaces: ["image-generation"],
              workflows: ["generate-image"],
              settingsSchema: null,
              providerCatalogs: [],
              runtimeChecks: [],
            },
          ];
        case "get_ocr_config_cmd":
          return clone(ocrConfig);
        case "check_ocr_models_cmd":
          return false;
        default:
          return null;
      }
    };

    (
      window as unknown as { __TAURI_INTERNALS__: unknown }
    ).__TAURI_INTERNALS__ = {
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

    (
      window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }
    ).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test("settings provider form shows updated preset models for add and edit flows", async ({
  page,
}) => {
  const modelField = () =>
    page
      .locator("label")
      .filter({ hasText: "Default Model" })
      .locator("xpath=..");
  const providerField = () =>
    page
      .locator("label")
      .filter({ hasText: "Provider Type" })
      .locator("xpath=..");

  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  await page.getByRole("button", { name: "Add Provider" }).click();
  await page.getByRole("button", { name: /Anthropic/i }).click();

  let modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect).toBeVisible();
  await expect(modelSelect.locator("option")).toContainText([
    "Claude Opus 4.6",
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);

  await providerField().getByRole("combobox").selectOption("google");
  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect.locator("option")).toContainText([
    "Gemini 2.5 Pro",
    "Gemini 3.1 Pro Preview",
    "Gemini 3.1 Flash-Lite Preview",
  ]);

  await providerField().getByRole("combobox").selectOption("qwen");
  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect.locator("option")).toContainText([
    "Qwen3 Max",
    "Qwen3.5 Plus",
    "Qwen3.6 Plus",
    "Qwen3 VL Plus",
    "QVQ Max",
  ]);

  await providerField().getByRole("combobox").selectOption("zhipu");
  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect.locator("option")).toContainText([
    "GLM-5",
    "GLM-4.7",
    "GLM-4.6V",
    "GLM-4.1V Thinking FlashX",
  ]);

  await providerField().getByRole("combobox").selectOption("deep_seek");
  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect.locator("option")).toContainText([
    "DeepSeek V4 Pro",
    "DeepSeek V4 Flash",
    "DeepSeek Reasoner",
  ]);

  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByTitle("Edit").first().click();

  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect).toBeVisible();
  await expect(modelSelect.locator("option")).toContainText([
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);
});

test("settings exposes image generation model config under AI providers", async ({
  page,
}) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const panel = page.getByTestId("image-generation-settings-panel");
  await expect(panel).toBeVisible();
  await expect(panel.getByRole("heading", { name: "Image Generation" })).toBeVisible();
  await expect(panel.getByText("Qwen Image (DashScope Beijing)")).toBeVisible();
  await expect(panel.getByText("Qwen CN API key")).toBeVisible();
  await expect(panel.locator("select")).toHaveCount(0);

  await panel.getByRole("button", { name: "Expand image generation settings" }).click();
  await expect(panel.getByText("Image provider defaults for generate_image")).toBeVisible();
  const selects = panel.locator("select");
  await expect(selects.nth(0)).toHaveValue("qwen-dashscope-cn");
  await expect(selects.nth(1)).toHaveValue("qwen-image-2.0-pro");

  await panel.getByRole("button", { name: "Save" }).click();
  await page.waitForFunction(() => {
    const saved = (window as unknown as { __savedAppConfig?: { imageGeneration?: { provider?: string; apiKey?: string } } })
      .__savedAppConfig;
    return saved?.imageGeneration?.provider === "qwen" &&
      saved.imageGeneration.apiKey === "sk-qwen-demo";
  });
});
