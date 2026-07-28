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
      modelPath: "",
    };

    const videoConfig = {
      enabled: false,
      whisperModel: "base",
      language: null,
      translateToEnglish: false,
      ffmpegPath: null,
      frameExtractionEnabled: false,
      frameIntervalSecs: 10,
      modelPath: "C:\\Nexa\\models\\whisper",
      sceneThreshold: 0.4,
      useGpu: true,
      preferEmbeddedSubtitles: true,
      beamSize: 5,
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
      localModelRoot: "",
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
          return true;
        case "get_video_config_cmd":
          return clone(videoConfig);
        case "check_whisper_model_cmd":
          return false;
        case "get_managed_model_paths_cmd": {
          const customRoot = typeof _args.root === "string" && _args.root.trim()
            ? _args.root.trim()
            : null;
          const root = customRoot || "C:\\Nexa\\models";
          return {
            root,
            embedding: `${root}\\paraphrase-multilingual-MiniLM-L12-v2`,
            ocr: `${root}\\paddleocr`,
            whisper: customRoot ? `${root}\\whisper` : "C:\\Nexa\\legacy-whisper",
          };
        }
        case "save_embedder_config_cmd":
          (window as unknown as { __savedEmbedConfig?: unknown }).__savedEmbedConfig = clone(_args.config);
          return null;
        case "save_ocr_config_cmd":
          (window as unknown as { __savedOcrConfig?: unknown }).__savedOcrConfig = clone(_args.config);
          return null;
        case "save_video_config_cmd":
          Object.assign(videoConfig, clone(_args.config));
          (window as unknown as { __savedVideoConfig?: unknown }).__savedVideoConfig = clone(videoConfig);
          return null;
        case "delete_ocr_models_cmd":
          (window as unknown as { __ocrDeleted?: boolean }).__ocrDeleted = true;
          return null;
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
      .locator("xpath=../..");
  const expectModelOptions = async (
    modelSelect: Locator,
    expectedNames: string[],
  ) => {
    const options = await modelField().locator("option, button").allTextContents();
    for (const expectedName of expectedNames) {
      expect(
        options.some((option) => option.includes(expectedName)),
        `expected model options to include ${expectedName}`,
      ).toBe(true);
    }
  };
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

  await providerField().getByRole("combobox").selectOption("alibaba_model_studio");
  modelSelect = modelField().getByRole("combobox");
  await expectModelOptions(modelSelect, [
    "DeepSeek V4 Pro",
    "Kimi K2.7 Code",
    "GLM-5.2",
    "MiniMax M2.5",
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

  const minimaxCard = page
    .getByRole("button")
    .filter({ has: page.locator('[title="MiniMax"]') });
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

test("settings migrates legacy Qwen pay-as-you-go configs to the Alibaba catalog", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByTitle("Edit").nth(1).click();

  const modelField = page
    .locator("label")
    .filter({ hasText: "Default Model" })
    .locator("xpath=../..");
  const options = await modelField.locator("option, button").allTextContents();
  expect(options.some((option) => option.includes("Qwen3.7 Max"))).toBe(true);
  expect(options.some((option) => option.includes("DeepSeek V4 Pro"))).toBe(true);
  expect(options.some((option) => option.includes("Qwen3.8 Max Preview"))).toBe(false);
});

test("settings exposes Alibaba Model Studio and SiliconFlow as router presets", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const alibaba = page.getByRole("button", { name: /^Alibaba Cloud Model Studio/ });
  await expect(alibaba).toContainText("DeepSeek, Kimi, GLM, and MiniMax");
  await expect(alibaba.locator('[title="Alibaba Cloud"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/alibabacloud\.svg/,
  );

  const siliconFlow = page.getByRole("button", { name: /^SiliconFlow/ });
  await expect(siliconFlow).toContainText("GLM, DeepSeek, Qwen");
  await expect(siliconFlow.locator('[title="SiliconFlow"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/siliconflow\.svg/,
  );
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

  await selects.nth(0).selectOption("siliconflow");
  await expect(selects.nth(1)).toHaveValue("fnlp/MOSS-TTSD-v0.5");
  await expect(panel.locator('[title="SiliconFlow"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/siliconflow\.svg/,
  );

  const sttPanel = page.getByTestId("speech-to-text-settings-panel");
  await expect(sttPanel.getByRole("heading", { name: "Speech to Text" })).toBeVisible();
  await sttPanel.locator("button").first().click();
  const sttProvider = sttPanel.getByTestId("stt-provider-select");
  await expect(sttProvider.locator("option")).toHaveCount(6);

  await sttProvider.selectOption("groq");
  await expect(sttPanel.locator('input[list="nexa-stt-models"]')).toHaveValue(
    "whisper-large-v3-turbo",
  );

  await sttProvider.selectOption("alibaba-qwen-asr");
  await expect(sttPanel.locator('input[list="nexa-stt-models"]')).toHaveValue("qwen3-asr-flash");
  await expect(sttPanel.locator('[title="Alibaba Cloud"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/alibabacloud\.svg/,
  );

  await sttProvider.selectOption("siliconflow");
  await expect(sttPanel.locator('input[list="nexa-stt-models"]')).toHaveValue(
    "FunAudioLLM/SenseVoiceSmall",
  );
});

test("settings keeps sherpa local models with downloaded models and separates provider categories", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await expect(page.locator('[data-provider-category="image-generation"]')).toContainText("Image generation");
  await expect(page.locator('[data-provider-category="text-to-speech"]')).toContainText("Cloud text to speech");
  await expect(page.locator('[data-provider-category="speech-to-text"]')).toContainText("Cloud speech to text");

  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();
  await expect(page.getByText("Local speech models")).toBeVisible();
  const localTts = page.getByTestId("text-to-speech-settings-panel");
  await localTts.locator("button").first().click();
  await localTts.locator("select").first().selectOption("sherpa-onnx");
  await expect(localTts.getByTestId("tts-local-executable")).toHaveValue("sherpa-onnx-offline-tts");

  const localStt = page.getByTestId("speech-to-text-settings-panel");
  await localStt.locator("button").first().click();
  await localStt.getByTestId("stt-provider-select").selectOption("sherpa-zipformer");
  await expect(localStt.getByTestId("stt-sherpa-executable")).toHaveValue("sherpa-onnx");
});

test("settings applies a managed model root and exposes OCR deletion", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();

  const rootInput = page.getByRole("textbox", { name: "Local model storage" });
  await expect(rootInput).toHaveValue("C:\\Nexa\\models");
  await rootInput.fill("D:\\NexaModels");
  await page.getByRole("button", { name: "Use this location" }).click();

  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { localModelRoot?: string } }
  ).__savedAppConfig?.localModelRoot)).toBe("D:\\NexaModels");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedOcrConfig?: { modelPath?: string } }
  ).__savedOcrConfig?.modelPath)).toBe("D:\\NexaModels\\paddleocr");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedVideoConfig?: { modelPath?: string } }
  ).__savedVideoConfig?.modelPath)).toBe("D:\\NexaModels\\whisper");

  await page.getByRole("button", { name: "Restore default" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { localModelRoot?: string } }
  ).__savedAppConfig?.localModelRoot)).toBe("");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedVideoConfig?: { modelPath?: string } }
  ).__savedVideoConfig?.modelPath)).toBe("C:\\Nexa\\legacy-whisper");

  const ocrCard = page.getByRole("heading", { name: "OCR Model" }).locator("xpath=../../..");
  await ocrCard.getByRole("button", { name: "Delete model" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __ocrDeleted?: boolean }
  ).__ocrDeleted)).toBe(true);
});

test("settings discard restores local speech while keeping persisted Whisper edits", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();

  const localTts = page.getByTestId("text-to-speech-settings-panel");
  await localTts.locator("button").first().click();
  await localTts.locator("select").first().selectOption("sherpa-onnx");
  let whisperCard = page
    .getByRole("heading", { name: "Speech Recognition Model" })
    .locator("xpath=../../..");
  await whisperCard.getByRole("button", { name: "Expand" }).click();
  await page.getByRole("button", { name: /^Small / }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedVideoConfig?: { whisperModel?: string } }
  ).__savedVideoConfig?.whisperModel)).toBe("small");

  await page.getByRole("button", { name: "Appearance" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByRole("heading", { name: "Appearance", exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();
  const restoredTts = page.getByTestId("text-to-speech-settings-panel");
  await restoredTts.locator("button").first().click();
  await expect(restoredTts.locator("select").first()).toHaveValue("");
  whisperCard = page
    .getByRole("heading", { name: "Speech Recognition Model" })
    .locator("xpath=../../..");
  await whisperCard.getByRole("button", { name: "Expand" }).click();
  await expect(page.getByRole("button", { name: /^Small / })).toHaveClass(/border-accent/);
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
