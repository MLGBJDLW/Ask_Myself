import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-locale", "en");

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let updateCheckCount = 0;

    const invoke = async (cmd: string, _args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:app|version":
          return "0.2.9";
        case "check_update_from_source_cmd":
          updateCheckCount += 1;
          (window as unknown as { __updateCheckCount: number }).__updateCheckCount = updateCheckCount;
          (window as unknown as { __lastUpdateSource: string }).__lastUpdateSource = String(_args.source ?? "");
          return null;
        case "plugin:updater|check":
          return null;
        case "plugin:event|listen": {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(_args.event ?? ""),
            handlerId: Number(_args.handler ?? 0),
          });
          return listenerId;
        }
        case "plugin:event|unlisten":
          listeners.delete(Number(_args.eventId ?? 0));
          return null;
        case "get_wizard_state_cmd":
          return { completed: true };
        case "list_agent_configs_cmd":
        case "list_conversations_cmd":
        case "list_sources":
        case "get_conversation_sources_cmd":
        case "list_checkpoints_cmd":
        case "list_user_memories_cmd":
        case "list_skills_cmd":
        case "list_mcp_servers_cmd":
          return [];
        case "list_tool_approval_policies_cmd":
          return { persisted: [], session: [] };
        case "get_app_config_cmd":
          return {
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
            confirmDestructive: false,
            shellAccessMode: "open",
            toolApprovalMode: "allow_all",
            hfMirrorBaseUrl: "https://hf-mirror.com",
            ghproxyBaseUrl: "https://mirror.ghproxy.com",
          };
        case "get_index_stats":
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case "get_privacy_config":
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case "get_embedder_config_cmd":
          return {
            provider: "tfidf",
            apiKey: "",
            apiBaseUrl: "",
            apiModel: "",
            localModel: "",
            modelPath: "",
            vectorDimensions: 384,
          };
        case "get_ocr_config_cmd":
          return {
            enabled: false,
            minConfidence: 0.5,
            llmFallback: false,
            detectionLimit: 2048,
            useCls: false,
          };
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
    (window as unknown as { __updateCheckCount: number }).__updateCheckCount = 0;
    (window as unknown as { __lastUpdateSource: string }).__lastUpdateSource = "";

    (
      window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }
    ).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test("settings appearance tab owns version and update controls", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();

  await expect(page.getByRole("heading", { name: "App update" })).toBeVisible();
  await expect(page.getByText("Update source")).toBeVisible();
  await expect(page.getByRole("button", { name: /Official GitHub Releases/ })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator('[aria-pressed="true"]')).toHaveCount(1);
  await expect(page.getByText("Current version")).toBeVisible();
  await expect(page.getByRole("main").getByText("v0.2.9")).toBeVisible();
  await expect(page.getByRole("button", { name: "Check for Updates" })).toBeVisible();
});

test("unsupported stored update source falls back to GitHub", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-update-source", "legacy-mirror");
  });
  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();

  await expect(page.getByRole("button", { name: /Official GitHub Releases/ })).toHaveAttribute("aria-pressed", "true");
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("nexa-update-source")))
    .toBe("legacy-mirror");

  await page.getByRole("button", { name: "Check for Updates" }).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __lastUpdateSource: string }).__lastUpdateSource))
    .toBe("github");
});

test("layout performs the silent startup update check", async ({ page }) => {
  await page.goto("/settings");

  await page.waitForFunction(
    () => (window as unknown as { __updateCheckCount?: number }).__updateCheckCount === 1,
    undefined,
    { timeout: 7000 },
  );
});

test("settings agent behavior controls use the selected locale", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();
  await page.getByRole("button", { name: "简体中文" }).click();
  await page
    .locator("button")
    .filter({ has: page.getByRole("heading", { name: "高级设置" }) })
    .click();

  await expect(page.getByText("Shell 权限模式")).toBeVisible();
  await expect(page.getByText("工具审批")).toBeVisible();
  await expect(page.getByText("全部允许")).toBeVisible();
  await expect(page.getByText("全部拒绝")).toBeVisible();
  await expect(page.getByText("已记住的决定")).toBeVisible();
  await expect(page.getByText("暂无已记住的审批决定。")).toBeVisible();
  await expect(page.getByText("Tool Approval")).toHaveCount(0);
});
