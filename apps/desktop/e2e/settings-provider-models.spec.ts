import { expect, type Locator, test } from "@playwright/test";

async function selectNexaOption(trigger: Locator, value: string) {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`).click();
}

async function expectNexaValue(trigger: Locator, value: string) {
  await expect(trigger).toHaveAttribute("data-value", value);
}

async function expectNexaOptions(trigger: Locator, expectedNames: string[]) {
  await trigger.click();
  const options = trigger.page().getByRole("option");
  const labels = await options.allTextContents();
  for (const expectedName of expectedNames) {
    expect(labels.some(label => label.includes(expectedName))).toBe(true);
  }
  await trigger.page().keyboard.press("Escape");
}

async function expectNexaOptionCount(trigger: Locator, count: number) {
  await trigger.click();
  await expect(trigger.page().getByRole("option")).toHaveCount(count);
  await trigger.page().keyboard.press("Escape");
}

async function expectNexaOption(
  trigger: Locator,
  value: string,
  expectation: "visible" | "absent",
  text?: string,
) {
  await trigger.click();
  const option = trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`);
  if (expectation === "visible") {
    await expect(option).toBeVisible();
    if (text) await expect(option).toContainText(text);
  } else {
    await expect(option).toHaveCount(0);
  }
  await trigger.page().keyboard.press("Escape");
}

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
      confidenceThreshold: 0.5,
      llmFallbackEnabled: false,
      detLimitSideLen: 2048,
      useCls: false,
      modelPath: "",
      languages: ["en"],
    };

    const videoConfig = {
      enabled: false,
      transcriptionMode: "inherit_speech_to_text",
      failurePolicy: "best_effort",
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
      cacheTtlHours: 24,
      defaultSearchLimit: 20,
      minSearchSimilarity: 0.2,
      maxTextFileSize: 104857600,
      maxVideoFileSize: 2147483648,
      maxAudioFileSize: 536870912,
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
        case "save_agent_config_cmd":
          (window as unknown as { __savedAgentConfig?: unknown }).__savedAgentConfig = clone(
            _args.config,
          );
          return null;
        case "save_app_config_cmd":
          (window as unknown as { __savedAppConfig?: unknown }).__savedAppConfig = clone(
            _args.config,
          );
          return null;
        case "list_capability_packages_cmd":
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
        case "test_agent_connection_cmd":
        case "refresh_provider_model_catalog_cmd": {
          const config = _args.config as {
            provider?: string;
            baseUrl?: string | null;
            providerEndpointId?: string | null;
          };
          if (cmd === "test_agent_connection_cmd") {
            (
              window as unknown as { __lastTestAgentConfig?: unknown }
            ).__lastTestAgentConfig = clone(config);
          }
          return {
            provider: config.provider ?? "alibaba_model_studio",
            baseUrl: config.baseUrl ?? null,
            refreshedAt: "2026-07-31T08:00:00Z",
            liveDiscoverySucceeded: true,
            models: [
              {
                id: "qwen3.7-max",
                name: "Qwen3.7 Max",
                tagKey: "providers.tagLatest",
                recommended: true,
                capabilities: { vision: false },
                source: "official",
                status: "active",
                regions: ["cn-beijing"],
                lastVerifiedAt: "2026-07-31T08:00:00Z",
                modalities: ["text"],
                supportsTools: true,
                supportsStructuredOutput: null,
                reasoningEfforts: [],
              },
              {
                id: "account-only-model",
                name: "account-only-model",
                recommended: false,
                capabilities: null,
                source: "discovered",
                status: "active",
                regions: ["cn-beijing"],
                lastVerifiedAt: "2026-07-31T08:00:00Z",
                modalities: ["text"],
                supportsTools: false,
                supportsStructuredOutput: false,
                reasoningEfforts: [],
              },
            ],
          };
        }
        case "refresh_tts_voice_catalog_cmd": {
          const config = _args.config as {
            provider?: string;
            apiStyle?: string;
            baseUrl?: string | null;
            model?: string;
          };
          return {
            provider: config.provider ?? "qwen",
            apiStyle: config.apiStyle ?? "dashscope_speech",
            baseUrl: config.baseUrl ?? null,
            model: config.model ?? "qwen-audio-3.0-tts-flash",
            refreshedAt: "2026-07-31T09:00:00Z",
            liveDiscoverySucceeded: true,
            voices: [
              {
                id: "account-designed-voice",
                name: "Account Designed Voice",
                recommended: false,
                source: "discovered",
                modelIds: [],
                languages: ["zh-CN", "en-US"],
                gender: null,
                description: "Private account voice",
                previewUrl: null,
              },
            ],
          };
        }
        case "synthesize_speech_preview_cmd": {
          const preview = {
            assetId: "speech-preview",
            path: "C:\\Nexa\\cache\\speech-preview.wav",
            mediaType: "audio/wav",
            bytes: 128,
          };
          if (_args.text === "Delay this preview.") {
            return await new Promise((resolve) => {
              (
                window as unknown as { __resolveDelayedSpeechPreview?: () => void }
              ).__resolveDelayedSpeechPreview = () => resolve(preview);
            });
          }
          return preview;
        }
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
    page.getByTestId("default-model-field");
  const expectModelOptions = async (
    _modelSelect: Locator,
    expectedNames: string[],
  ) => {
    const currentSelect = modelField().locator("[data-nexa-select-trigger]");
    if (await currentSelect.count()) {
      await expectNexaOptions(currentSelect, expectedNames);
      return;
    }
    for (const expectedName of expectedNames) {
      await expect(
        modelField().getByRole("button").filter({ hasText: expectedName }).first(),
      ).toBeVisible();
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

  let modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expect(modelSelect).toBeVisible();
  await expectModelOptions(modelSelect, [
    "Claude Opus 4.8",
    "Claude Opus 4.7",
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);

  await selectNexaOption(providerField().locator("[data-nexa-select-trigger]"), "google");
  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expectModelOptions(modelSelect, [
    "Gemini 3.6 Flash",
    "Gemini 3.5 Flash-Lite",
    "Gemini 3.1 Pro Preview",
    "Gemini 2.5 Pro",
    "Gemini 3 Flash Preview",
  ]);

  await selectNexaOption(providerField().locator("[data-nexa-select-trigger]"), "alibaba_model_studio");
  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expectModelOptions(modelSelect, [
    "Qwen3.8 Max",
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

  await selectNexaOption(providerField().locator("[data-nexa-select-trigger]"), "zhipu");
  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expectModelOptions(modelSelect, [
    "GLM-5.2",
    "GLM-5.1",
    "GLM-5",
    "GLM-4.7",
    "GLM-4.6V",
    "GLM-4.1V Thinking FlashX",
  ]);

  await selectNexaOption(providerField().locator("[data-nexa-select-trigger]"), "deep_seek");
  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expectModelOptions(modelSelect, [
    "DeepSeek V4 Pro",
    "DeepSeek V4 Flash",
  ]);

  await selectNexaOption(providerField().locator("[data-nexa-select-trigger]"), "moonshot");
  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expectModelOptions(modelSelect, ["Kimi K3", "Kimi K2.7"]);

  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByTitle("Edit").first().click();

  modelSelect = modelField().locator("[data-nexa-select-trigger]");
  await expect(modelSelect).toBeVisible();
  await expectModelOptions(modelSelect, [
    "Claude Opus 4.8",
    "Claude Opus 4.7",
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);
});

test("provider output limit is automatic when the explicit cap is cleared", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByTitle("Edit").first().click();

  const maxTokensField = page
    .locator("label")
    .filter({ hasText: "Max Tokens" })
    .locator("xpath=..");
  const input = maxTokensField.getByRole("spinbutton");
  await expect(input).toHaveValue("4096");
  await input.fill("");
  await expect(input).toHaveAttribute("placeholder", "Auto (provider default)");
  await expect(maxTokensField).toContainText("provider's native output limit");

  await maxTokensField
    .locator("xpath=ancestor::form")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAgentConfig?: { maxTokens?: number | null } }
  ).__savedAgentConfig?.maxTokens)).toBeNull();
});

test("catalog model picker stays compact and searchable in a narrow settings column", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();
  await page.getByRole("button", { name: /^OpenAI/ }).click();

  await page.setViewportSize({ width: 320, height: 760 });
  const picker = page.getByTestId("default-model-picker");
  await expect(picker).toBeVisible();
  expect(await picker.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
    overflow: getComputedStyle(element).overflow,
  }))).toEqual(expect.objectContaining({ overflow: "hidden" }));
  expect(await picker.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);

  await picker.click();
  const search = page.getByPlaceholder("Search models by name, provider, or ID");
  await search.fill("gpt-5.6-sol");
  const option = page.locator('[role="option"][data-value="gpt-5.6-sol"]');
  await expect(option).toBeVisible();
  await expect(option).toContainText("gpt-5.6-sol");
  await expect(option).toContainText("text→text");
});

test("provider refresh keeps account-discovered models selectable and cached", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();
  await page.getByRole("button", { name: /^Alibaba Cloud Model Studio/ }).click();

  await page.locator('input[placeholder="sk-..."]').fill("sk-account");
  await page.getByRole("button", { name: "Refresh models" }).click();

  await expect(page.getByTestId("provider-model-catalog-status")).toContainText("Live account catalog");
  const modelField = page
    .getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaOption(modelSelect, "account-only-model", "visible", "Discovered");
  await selectNexaOption(modelSelect, "account-only-model");
  await expectNexaValue(modelSelect, "account-only-model");

  await expect.poll(() => page.evaluate(() => Object.keys(localStorage)
    .some((key) => key.startsWith("nexa-provider-model-catalog-v1:")))).toBe(true);
});

test("an edited public base URL cannot inherit catalog identity or capabilities", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();
  await page.getByRole("button", { name: /^OpenAI/ }).click();

  await page.locator('input[placeholder="sk-..."]').fill("sk-account");
  await selectNexaOption(
    page.getByTestId("default-model-field").locator("[data-nexa-select-trigger]"),
    "gpt-5.6",
  );
  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await baseUrlField.getByRole("textbox").fill("https://api.openai.com/evil?tenant=1");
  await page.getByRole("button", { name: /^Advanced Settings/ }).click();
  await expect(page.getByText("No configurable reasoning controls are available for this model.")).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "Enable reasoning" })).toBeDisabled();
  await expect(page.getByTestId("default-model-picker")).toHaveCount(0);
  await page.getByRole("button", { name: "Test Connection" }).click();

  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __lastTestAgentConfig?: { providerEndpointId?: string | null };
    }
  ).__lastTestAgentConfig?.providerEndpointId)).toBeNull();
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

test("settings keeps Qwen3.8 isolated to the Token Plan endpoint", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const tokenPlanCard = page.getByRole("button", { name: /^Qwen Token Plan/ });
  await expect(tokenPlanCard).toContainText("sk-sp key");
  await tokenPlanCard.click();

  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await expect(baseUrlField.getByRole("textbox")).toHaveValue(
    "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
  );

  const modelField = page
    .getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaValue(modelSelect, "");
  await expectNexaOptions(modelSelect, ["Qwen3.8 Max", "Qwen3.8 Max Preview"]);
  await expectNexaOptionCount(modelSelect, 2);
  await expectNexaOption(modelSelect, "qwen3.7-flash", "absent");
  await selectNexaOption(modelSelect, "qwen3.8-max-preview");
  await expect(modelField.getByTestId("model-descriptor-badges")).toContainText("Status: preview");
  await expect(modelField.getByTestId("model-descriptor-badges")).toContainText("Access: account enablement");
});

test("settings exposes Qwen3.7 Flash through QwenCloud international", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const qwenCloudCard = page.getByRole("button", { name: /^QwenCloud \(International\)/ });
  await expect(qwenCloudCard).toContainText("pay-as-you-go international endpoint");
  await qwenCloudCard.click();

  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await expect(baseUrlField.getByRole("textbox")).toHaveValue(
    "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
  );

  const modelField = page
    .getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaValue(modelSelect, "");
  await selectNexaOption(modelSelect, "qwen3.7-flash");
  await expectNexaValue(modelSelect, "qwen3.7-flash");
  await expectNexaOptions(modelSelect, [
    "Qwen3.8 Max",
    "Qwen3.7 Flash",
    "Qwen3.7 Plus",
    "Qwen3.7 Max",
  ]);
});

test("settings migrates legacy Qwen pay-as-you-go configs to the Alibaba catalog", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByTitle("Edit").nth(1).click();

  const modelField = page
    .getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaValue(modelSelect, "qwen3.6-plus");
  await expectNexaOptions(modelSelect, ["Qwen3.8 Max", "Qwen3.7 Max", "DeepSeek V4 Pro"]);
  await expectNexaOption(modelSelect, "qwen3.8-max-preview", "absent");
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
  await expect(panel.locator("[data-nexa-select-trigger]")).toHaveCount(0);

  await panel.getByRole("button", { name: "Expand image generation settings" }).click();
  await expect(panel.getByText("Image provider defaults for generate_image")).toBeVisible();
  const selects = panel.locator("[data-nexa-select-trigger]");
  await expectNexaValue(selects.nth(0), "qwen-dashscope-cn");
  await selectNexaOption(selects.nth(1), "qwen-image-2.0-pro");
  await expectNexaValue(selects.nth(1), "qwen-image-2.0-pro");
  await selectNexaOption(selects.nth(1), "qwen-image-3.0-pro");
  await expect(panel.getByTestId("model-descriptor-badges")).toContainText("Status: preview");
  await expect(panel.getByTestId("model-descriptor-badges")).toContainText("Access: application");
  await selectNexaOption(selects.nth(1), "qwen-image-2.0-pro");

  await selectNexaOption(selects.nth(0), "google-gemini");
  await selectNexaOption(selects.nth(1), "gemini-3.1-flash-image");
  await expectNexaValue(selects.nth(1), "gemini-3.1-flash-image");
  await expectNexaOptions(selects.nth(1), [
    "Gemini 3.1 Flash Image",
    "Gemini 3.1 Flash Lite Image",
    "Gemini 3 Pro Image",
  ]);
  await selectNexaOption(selects.nth(0), "qwen-dashscope-cn");
  await selectNexaOption(selects.nth(1), "qwen-image-2.0-pro");

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

  const selects = panel.locator("[data-nexa-select-trigger]");
  await expectNexaOptionCount(selects.nth(0), 8);
  await selectNexaOption(selects.nth(1), "gpt-4o-mini-tts");
  await selectNexaOption(selects.nth(0), "groq");
  await selectNexaOption(selects.nth(1), "canopylabs/orpheus-v1-english");
  await expectNexaValue(selects.nth(1), "canopylabs/orpheus-v1-english");
  await expectNexaValue(selects.nth(2), "wav");
  await expect(panel.locator('[title="Groq"]')).toContainText("GQ");
  await expect(panel.getByTestId("tts-voice-catalog")).toContainText("Hannah");
  await expect(panel.getByTestId("tts-voice-catalog")).not.toContainText("Fahad");
  await selectNexaOption(selects.nth(1), "canopylabs/orpheus-arabic-saudi");
  await expect(panel.getByTestId("tts-voice-catalog")).toContainText("Fahad");
  await expect(panel.getByTestId("tts-voice-catalog")).not.toContainText("Hannah");

  await selectNexaOption(selects.nth(0), "elevenlabs");
  await selectNexaOption(selects.nth(1), "eleven_flash_v2_5");
  await expectNexaValue(selects.nth(1), "eleven_flash_v2_5");
  await expect(panel.locator('[title="ElevenLabs"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/elevenlabs\.svg/,
  );

  await selectNexaOption(selects.nth(0), "minimax");
  await selectNexaOption(selects.nth(1), "speech-2.8-turbo");
  await expectNexaValue(selects.nth(1), "speech-2.8-turbo");

  await selectNexaOption(selects.nth(0), "dashscope-cosyvoice");
  await selectNexaOption(selects.nth(1), "qwen-audio-3.0-tts-flash");
  await expectNexaValue(selects.nth(1), "qwen-audio-3.0-tts-flash");
  await expect(panel.getByTestId("tts-voice-input")).toHaveValue("longanhuan_v3.6");
  await expect(panel.getByTestId("shared-credential-notice")).toHaveAttribute("data-state", "reusing");
  await panel.getByRole("button", { name: "Refresh voices" }).click();
  await expect(panel.getByTestId("tts-voice-catalog-status")).toContainText("Live account voice catalog");
  await panel.getByTestId("tts-voice-search").fill("designed");
  await panel.getByRole("button", { name: /Account Designed Voice/ }).click();
  await expect(panel.getByTestId("tts-voice-input")).toHaveValue("account-designed-voice");
  await panel.getByRole("button", { name: "Preview voice" }).click();
  await expect(panel.locator("audio")).toHaveCount(1);
  await panel.getByTestId("tts-voice-input").fill("account-designed-voice-v2");
  await expect(panel.locator("audio")).toHaveCount(0);
  await panel.getByRole("button", { name: "Save" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { textToSpeech?: { apiKey?: string } } }
  ).__savedAppConfig?.textToSpeech?.apiKey)).toBe("sk-qwen-demo");

  await selectNexaOption(selects.nth(0), "siliconflow");
  await selectNexaOption(selects.nth(1), "fnlp/MOSS-TTSD-v0.5");
  await expectNexaValue(selects.nth(1), "fnlp/MOSS-TTSD-v0.5");
  await expect(panel.locator('[title="SiliconFlow"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/siliconflow\.svg/,
  );

  const sttPanel = page.getByTestId("speech-to-text-settings-panel");
  await expect(sttPanel.getByRole("heading", { name: "Speech to Text" })).toBeVisible();
  await sttPanel.locator("button").first().click();
  const sttProvider = sttPanel.getByTestId("stt-provider-select");
  const sttModel = sttPanel.locator("[data-nexa-select-trigger]").nth(1);
  await expectNexaOptionCount(sttProvider, 9);

  await selectNexaOption(sttProvider, "openai-live");
  await selectNexaOption(sttModel, "gpt-live-transcribe");
  await expectNexaValue(sttModel, "gpt-live-transcribe");

  await selectNexaOption(sttProvider, "groq");
  await selectNexaOption(sttModel, "whisper-large-v3-turbo");
  await expectNexaValue(sttModel, "whisper-large-v3-turbo");

  await selectNexaOption(sttProvider, "alibaba-qwen-asr");
  await selectNexaOption(sttModel, "qwen3-asr-flash");
  await expectNexaValue(sttModel, "qwen3-asr-flash");
  await expect(sttPanel.getByTestId("shared-credential-notice")).toHaveAttribute("data-state", "reusing");
  await expect(sttPanel.locator('[title="Alibaba Cloud"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/alibabacloud\.svg/,
  );
  await sttPanel.getByRole("button", { name: "Save" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { speechToText?: { apiKey?: string } } }
  ).__savedAppConfig?.speechToText?.apiKey)).toBe("sk-qwen-demo");

  await selectNexaOption(sttProvider, "siliconflow");
  await selectNexaOption(sttModel, "FunAudioLLM/SenseVoiceSmall");
  await expectNexaValue(sttModel, "FunAudioLLM/SenseVoiceSmall");
});

test("settings discards a stale voice preview after synthesis settings change", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const panel = page.getByTestId("text-to-speech-settings-panel");
  await panel.locator("button").first().click();
  await selectNexaOption(panel.locator("[data-nexa-select-trigger]").first(), "dashscope-cosyvoice");
  await selectNexaOption(panel.locator("[data-nexa-select-trigger]").nth(1), "qwen-audio-3.0-tts-flash");
  const previewText = panel
    .locator("label")
    .filter({ hasText: "Preview text" })
    .locator("xpath=..")
    .getByRole("textbox");
  await previewText.fill("Delay this preview.");
  await panel.getByRole("button", { name: "Preview voice" }).click();

  await panel.getByTestId("tts-voice-input").fill("longanhuan_v3.6-updated");
  await expect(panel.getByRole("button", { name: "Preview voice" })).toBeEnabled();
  await page.evaluate(() => {
    (
      window as unknown as { __resolveDelayedSpeechPreview?: () => void }
    ).__resolveDelayedSpeechPreview?.();
  });

  await expect(panel.locator("audio")).toHaveCount(0);
});

test("settings never reuses a provider key for a user-edited endpoint", async ({ page }) => {
  const baseUrlInput = (panel: Locator) => panel
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..")
    .getByRole("textbox");

  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const imagePanel = page.getByTestId("image-generation-settings-panel");
  await expect(imagePanel.getByText("Qwen CN API key")).toBeVisible();
  await imagePanel.getByRole("button", { name: "Expand image generation settings" }).click();
  await baseUrlInput(imagePanel).fill("https://proxy.example.com/v1");
  await expect(imagePanel.getByText("Qwen CN API key")).toHaveCount(0);
  await expect(imagePanel.getByRole("button", { name: "Save" })).toBeDisabled();

  const ttsPanel = page.getByTestId("text-to-speech-settings-panel");
  await ttsPanel.locator("button").first().click();
  await selectNexaOption(ttsPanel.locator("[data-nexa-select-trigger]").first(), "dashscope-cosyvoice");
  await expect(ttsPanel.getByTestId("shared-credential-notice")).toHaveAttribute("data-state", "reusing");
  await baseUrlInput(ttsPanel).fill("http://dashscope.aliyuncs.com/api/v1/services/audio/tts");
  await expect(ttsPanel.getByTestId("shared-credential-notice")).toHaveCount(0);
  await expect(ttsPanel.getByRole("button", { name: "Save" })).toBeDisabled();

  const sttPanel = page.getByTestId("speech-to-text-settings-panel");
  await sttPanel.locator("button").first().click();
  await selectNexaOption(sttPanel.getByTestId("stt-provider-select"), "alibaba-qwen-asr");
  await expect(sttPanel.getByTestId("shared-credential-notice")).toHaveAttribute("data-state", "reusing");
  await baseUrlInput(sttPanel).fill("https://dashscope.aliyuncs.com:8443/api/v1/services/audio/asr/transcription");
  await expect(sttPanel.getByTestId("shared-credential-notice")).toHaveCount(0);
  await expect(sttPanel.getByRole("button", { name: "Save" })).toBeDisabled();
});

test("settings keeps local and cloud speech engines in one provider category", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await expect(page.locator('[data-provider-category="image-generation"]')).toContainText("Image generation");
  const speechCategory = page.locator('[data-provider-category="speech"]');
  await expect(speechCategory).toContainText("Speech");
  const localTts = speechCategory.getByTestId("text-to-speech-settings-panel");
  await localTts.locator("button").first().click();
  await selectNexaOption(localTts.locator("[data-nexa-select-trigger]").first(), "sherpa-onnx");
  await expect(localTts.getByTestId("tts-local-executable")).toHaveValue("sherpa-onnx-offline-tts");

  const localStt = speechCategory.getByTestId("speech-to-text-settings-panel");
  await localStt.locator("button").first().click();
  await selectNexaOption(localStt.getByTestId("stt-provider-select"), "sherpa-zipformer");
  await expect(localStt.getByTestId("stt-sherpa-executable")).toHaveValue("sherpa-onnx");
});

test("media settings expose truthful policies and keep microphone controls with voice input", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Media Processing" }).click();

  const videoSection = page
    .getByRole("heading", { name: "Video Analysis" })
    .locator("xpath=ancestor::section");
  await videoSection.locator("button").first().click();
  await expect(videoSection.getByText("Transcription source")).toBeVisible();
  await expect(videoSection.getByText("Failure policy")).toBeVisible();
  await expect(videoSection.getByText("GPU Acceleration")).toHaveCount(0);
  await expect(videoSection.getByText("Beam Search Width")).toHaveCount(0);
  await expect(videoSection.getByText("Microphone", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "AI Providers" }).click();
  const speechInput = page.getByTestId("speech-to-text-settings-panel");
  await speechInput.locator("button").first().click();
  await expect(speechInput.getByText("Voice Input", { exact: true })).toBeVisible();
  await expect(speechInput.getByText("Runtime not ready")).toBeVisible();
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

test("settings discard leaves speech drafts unpersisted while keeping saved Whisper edits", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();

  let whisperCard = page
    .getByRole("heading", { name: "Speech Recognition Model" })
    .locator("xpath=../../..");
  await whisperCard.getByRole("button", { name: "Expand" }).click();
  await page.getByRole("button", { name: /^Small / }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedVideoConfig?: { whisperModel?: string } }
  ).__savedVideoConfig?.whisperModel)).toBe("small");

  await page.getByRole("button", { name: "AI Providers" }).click();
  const localTts = page.getByTestId("text-to-speech-settings-panel");
  await localTts.locator("button").first().click();
  await selectNexaOption(localTts.locator("[data-nexa-select-trigger]").first(), "sherpa-onnx");

  await page.getByRole("button", { name: "Appearance" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByRole("heading", { name: "Appearance", exact: true })).toBeVisible();

  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: unknown }
  ).__savedAppConfig)).toBeUndefined();

  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();
  whisperCard = page
    .getByRole("heading", { name: "Speech Recognition Model" })
    .locator("xpath=../../..");
  await whisperCard.getByRole("button", { name: "Expand" }).click();
  await expect(page.getByRole("button", { name: /^Small / })).toHaveClass(/border-accent/);
});

test("appearance keeps a compact theme summary and opens the dedicated Theme tab", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("nexa-active-theme-v1", "dark"));
  await page.goto("/settings");

  const summary = page.getByTestId("theme-summary-card");
  await expect(summary).toContainText("Dark");
  await expect(page.getByTestId("theme-studio")).toHaveCount(0);

  await summary.getByRole("button", { name: "Open Theme Studio" }).click();
  await expect(page.getByRole("button", { name: "Theme", exact: true })).toHaveClass(/bg-accent/);
  await expect(page.getByTestId("theme-studio")).toBeVisible();
  await expect(page.getByRole("button", { name: "Advanced colors" })).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByRole("button", { name: "Background & effects" })).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByRole("button", { name: "Import & export" })).toHaveAttribute("aria-expanded", "false");
});

test("settings offers Qwen key reuse plus Jina and Mistral embedding presets", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();

  const section = page.locator("section").filter({
    has: page.getByRole("heading", { name: "Embedding Configuration" }),
  });
  await section.locator("button").first().click();
  await section.getByRole("button", { name: "API", exact: true }).click();

  const selects = section.locator("[data-nexa-select-trigger]");
  await selectNexaOption(selects.nth(0), "alibaba-model-studio-cn");
  await selectNexaOption(selects.nth(1), "text-embedding-v4");
  await expectNexaValue(selects.nth(1), "text-embedding-v4");
  await expect(section.getByRole("spinbutton")).toHaveValue("1024");
  await expect(section.getByTestId("shared-credential-notice")).toHaveAttribute("data-state", "reusing");
  await section.getByRole("button", { name: "Save Config" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedEmbedConfig?: { apiKey?: string; apiModel?: string } }
  ).__savedEmbedConfig)).toEqual(expect.objectContaining({
    apiKey: "sk-qwen-demo",
    apiModel: "text-embedding-v4",
  }));

  await selectNexaOption(selects.nth(0), "jina");
  await selectNexaOption(selects.nth(1), "jina-embeddings-v5-text-small");
  await expectNexaValue(selects.nth(1), "jina-embeddings-v5-text-small");
  await expect(section.getByRole("spinbutton")).toHaveValue("1024");
  await expect(section.getByRole("spinbutton")).toBeDisabled();
  await expect(section.locator('[title="Jina AI"] > span')).toHaveAttribute(
    "style",
    /provider-icons\/jina\.svg/,
  );

  await selectNexaOption(selects.nth(0), "mistral");
  await selectNexaOption(selects.nth(1), "mistral-embed");
  await expectNexaValue(selects.nth(1), "mistral-embed");
  await expect(section.getByRole("spinbutton")).toHaveValue("1024");
});

test("dream theme is decorative and quieter away from home", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("nexa-theme", "dream"));

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Appearance", exact: true })).toBeVisible();
  const shell = page.locator('[data-app-area]');
  await shell.evaluate((element) => element.setAttribute('data-app-area', 'home'));
  const homeBackdrop = shell.locator('.app-theme-backdrop');
  await expect(homeBackdrop).toHaveCSS('pointer-events', 'none');
  await expect(homeBackdrop).toHaveCSS('opacity', '0.92');
  await page.screenshot({ path: 'test-results/dream-home.png', fullPage: true });

  await shell.evaluate((element) => element.setAttribute('data-app-area', 'task'));
  await expect(shell.locator('.app-theme-backdrop')).toHaveCSS('opacity', '0.42');
  await page.screenshot({ path: 'test-results/dream-settings.png', fullPage: true });
});
