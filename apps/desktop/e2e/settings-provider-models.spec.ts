import { expect, type Locator, test } from "@playwright/test";

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
      textToSpeech: {
        provider: "open_ai",
        apiStyle: "openai_speech",
        apiKey: "",
        baseUrl: "https://api.openai.com/v1",
        model: "gpt-4o-mini-tts",
        voice: "coral",
        outputFormat: "wav",
        speed: 1,
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
  const expectModelOptions = async (
    modelSelect: Locator,
    expectedNames: string[],
  ) => {
    const options = await modelSelect.locator("option").allTextContents();
    for (const expectedName of expectedNames) {
      expect(
        options.some((option) => option.includes(expectedName)),
        `expected model options to include ${expectedName}`,
      ).toBe(true);
    }
  };
  const modelField = () =>
    page
      .locator("label")
      .filter({ hasText: "Default Model" })
      .locator("xpath=../..");
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
  await expectModelOptions(modelSelect, [
    "Claude Opus 4.8",
    "Claude Opus 4.7",
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);

  await providerField().getByRole("combobox").selectOption("google");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, [
    "Gemini 3.6 Flash",
    "Gemini 3.5 Flash-Lite",
    "Gemini 3.1 Pro Preview",
    "Gemini 2.5 Pro",
    "Gemini 3 Flash Preview",
  ]);

  await providerField().getByRole("combobox").selectOption("qwen");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, [
    "Qwen3.7 Max",
    "Qwen3 Max",
    "Qwen3.5 Plus",
    "Qwen3.6 Plus",
    "Qwen3 VL Plus",
  ]);

  await providerField().getByRole("combobox").selectOption("zhipu");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, [
    "GLM-5.2",
    "GLM-5.1",
    "GLM-5",
    "GLM-4.7",
    "GLM-4.6V",
    "GLM-4.1V Thinking FlashX",
  ]);

  await providerField().getByRole("combobox").selectOption("deep_seek");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, [
    "DeepSeek V4 Pro",
    "DeepSeek V4 Flash",
  ]);

  await providerField().getByRole("combobox").selectOption("moonshot");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, ["Kimi K3", "Kimi K2.7"]);

  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByTitle("Edit").first().click();

  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect).toBeVisible();
  await expectModelOptions(modelSelect, [
    "Claude Opus 4.8",
    "Claude Opus 4.7",
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);
});

test("settings uses the MiniMax logo for its OpenAI-compatible preset", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const minimaxCard = page.getByRole("button", { name: /MiniMax/ });
  await expect(minimaxCard).toBeVisible();
  const minimaxGlyph = minimaxCard.locator('[title="MiniMax"] > span');
  await expect(minimaxGlyph).toHaveAttribute("style", /provider-icons\/minimax\.svg/);
  await expect(minimaxGlyph).not.toHaveAttribute("style", /provider-icons\/openai\.svg/);
});

test("settings exposes Qwen3.8 only through the Token Plan endpoint", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const tokenPlanCard = page.getByRole("button", { name: /^Qwen Token Plan/ });
  await expect(tokenPlanCard).toContainText("sk-sp API key");
  await tokenPlanCard.click();

  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await expect(baseUrlField.getByRole("textbox")).toHaveValue(
    "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
  );

  const modelField = page
    .locator("label")
    .filter({ hasText: "Default Model" })
    .locator("xpath=../..");
  const modelSelect = modelField.getByRole("combobox");
  await expect(modelSelect).toHaveValue("qwen3.8-max-preview");
  await expect(modelSelect.locator("option")).toContainText(["Qwen3.8 Max Preview"]);
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

  await selects.nth(0).selectOption("google-gemini");
  await expect(selects.nth(1)).toHaveValue("gemini-3.1-flash-image");
  await expect(selects.nth(1).locator("option")).toContainText([
    "Gemini 3.1 Flash Image",
    "Gemini 3.1 Flash Lite Image",
    "Gemini 3 Pro Image",
  ]);
  await selects.nth(0).selectOption("qwen-dashscope-cn");

  await panel.getByRole("button", { name: "Save" }).click();
  await page.waitForFunction(() => {
    const saved = (window as unknown as { __savedAppConfig?: { imageGeneration?: { provider?: string; apiKey?: string } } })
      .__savedAppConfig;
    return saved?.imageGeneration?.provider === "qwen" &&
      saved.imageGeneration.apiKey === "sk-qwen-demo";
  });
});

test("settings promotes low-latency speech providers with their own logos", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const panel = page.getByTestId("text-to-speech-settings-panel");
  await expect(panel.getByRole("heading", { name: "Text to Speech" })).toBeVisible();
  await panel.locator("button").first().click();

  const selects = panel.locator("select");
  await expect(selects.nth(0).locator("option")).toHaveCount(7);
  await expect(selects.nth(1)).toHaveValue("gpt-4o-mini-tts");
  await selects.nth(0).selectOption("groq");
  await expect(selects.nth(1)).toHaveValue("canopylabs/orpheus-v1-english");
  await expect(selects.nth(2)).toHaveValue("wav");
  await expect(panel.locator('[title="Groq"]')).toContainText("GQ");

  await selects.nth(0).selectOption("elevenlabs");
  await expect(selects.nth(1)).toHaveValue("eleven_flash_v2_5");
  await expect(panel.locator('[title="ElevenLabs"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/elevenlabs\.svg/,
  );

  await selects.nth(0).selectOption("minimax");
  await expect(selects.nth(1)).toHaveValue("speech-2.8-turbo");

  await selects.nth(0).selectOption("dashscope-cosyvoice");
  await expect(selects.nth(1)).toHaveValue("qwen-audio-3.0-tts-flash");
  await expect(panel.getByTestId("tts-voice-input")).toHaveValue("longanhuan_v3.6");

  await selects.nth(0).selectOption("sherpa-onnx");
  await expect(selects.nth(1)).toHaveValue("vits");
  await expect(panel.locator('[title="sherpa-onnx"]')).toContainText("S");
  await expect(panel.getByTestId("tts-local-executable")).toHaveValue("sherpa-onnx-offline-tts");
  await expect(panel.getByTestId("tts-local-model")).toBeVisible();
  await expect(panel.locator("label").filter({ hasText: "API Key" })).toHaveCount(0);

  const sttPanel = page.getByTestId("speech-to-text-settings-panel");
  await expect(sttPanel.getByRole("heading", { name: "Speech to Text" })).toBeVisible();
  await sttPanel.locator("button").first().click();
  const sttProvider = sttPanel.getByTestId("stt-provider-select");
  await expect(sttProvider.locator("option")).toHaveCount(6);

  await sttProvider.selectOption("groq");
  await expect(sttPanel.locator('input[list="nexa-stt-models"]')).toHaveValue(
    "whisper-large-v3-turbo",
  );

  await sttProvider.selectOption("sherpa-zipformer");
  await expect(sttPanel.getByTestId("stt-sherpa-executable")).toHaveValue("sherpa-onnx");
  await expect(sttPanel.locator("label").filter({ hasText: "encoder" })).toBeVisible();
  await expect(sttPanel.locator("label").filter({ hasText: "decoder" })).toBeVisible();
  await expect(sttPanel.locator("label").filter({ hasText: "joiner" })).toBeVisible();
});

test("settings promotes Jina and Mistral embedding presets with fixed dimensions", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();

  const section = page.locator("section").filter({
    has: page.getByRole("heading", { name: "Embedding Configuration" }),
  });
  await section.locator("button").first().click();
  await section.getByRole("button", { name: "API", exact: true }).click();

  const selects = section.locator("select");
  await selects.nth(0).selectOption("jina");
  await expect(selects.nth(1)).toHaveValue("jina-embeddings-v5-text-small");
  await expect(section.getByRole("spinbutton")).toHaveValue("1024");
  await expect(section.getByRole("spinbutton")).toBeDisabled();
  await expect(section.locator('[title="Jina AI"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/jina\.svg/,
  );

  await selects.nth(0).selectOption("mistral");
  await expect(selects.nth(1)).toHaveValue("mistral-embed");
  await expect(section.getByRole("spinbutton")).toHaveValue("1024");
});

test("dream theme is decorative and quieter away from home", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("nexa-theme", "dream"));

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Appearance", exact: true })).toBeVisible();
  const shell = page.locator('[data-app-area]');
  await shell.evaluate((element) => element.setAttribute('data-app-area', 'home'));
  const homeBackdrop = shell.locator('.dream-backdrop');
  await expect(homeBackdrop).toHaveCSS('pointer-events', 'none');
  await expect(homeBackdrop).toHaveCSS('opacity', '0.92');
  await page.screenshot({ path: 'test-results/dream-home.png', fullPage: true });

  await shell.evaluate((element) => element.setAttribute('data-app-area', 'task'));
  await expect(shell.locator('.dream-backdrop')).toHaveCSS('opacity', '0.42');
  await page.screenshot({ path: 'test-results/dream-settings.png', fullPage: true });
});
