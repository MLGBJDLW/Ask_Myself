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
        case "list_agent_configs_cmd":
          return [clone(anthropicConfig)];
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

  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByTitle("Edit").click();

  modelSelect = modelField().getByRole("combobox");
  await expect(modelSelect).toBeVisible();
  await expect(modelSelect.locator("option")).toContainText([
    "Claude Sonnet 4.6",
    "Claude Sonnet 4.5",
    "Claude Haiku 4.5",
  ]);
});
