import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-locale", "en");

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let updateCheckCount = 0;
    const realFetch = window.fetch.bind(window);
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/repos/MLGBJDLW/Nexa/releases")) {
        return new Response(JSON.stringify([
          {
            tag_name: "nexa-monorepo-v0.10.2",
            name: "v0.10.2",
            body: "Second ranged release note.",
            draft: false,
            prerelease: false,
          },
          {
            tag_name: "nexa-monorepo-v0.10.1",
            name: "v0.10.1",
            body: "First ranged release note.",
            draft: false,
            prerelease: false,
          },
          {
            tag_name: "nexa-monorepo-v0.10.0",
            name: "v0.10.0",
            body: "Already installed release note.",
            draft: false,
            prerelease: false,
          },
        ]), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      return realFetch(input, init);
    };

    const invoke = async (cmd: string, _args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:app|version":
          return "0.2.9";
        case "check_update_from_source_cmd":
          updateCheckCount += 1;
          (window as unknown as { __updateCheckCount: number }).__updateCheckCount = updateCheckCount;
          (window as unknown as { __lastUpdateSource: string }).__lastUpdateSource = String(_args.source ?? "");
          if (localStorage.getItem("e2e-update-available") === "1") {
            return {
              rid: 1,
              currentVersion: "0.10.0",
              version: "0.10.2",
              body: "Latest-only release note.",
              rawJson: {},
            };
          }
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
        case "list_tool_permission_policies_cmd":
          return { persisted: [], session: [] };
        case "get_app_config_cmd":
          return {
            cacheTtlHours: 24,
            defaultSearchLimit: 20,
            minSearchSimilarity: 0.2,
            maxTextFileSize: 104857600,
            maxVideoFileSize: 2147483648,
            maxAudioFileSize: 536870912,
            confirmDestructive: false,
            shellAccessMode: "open",
            toolApprovalMode: "allow_all",
            windowCloseBehavior: "exit",
            hfMirrorBaseUrl: "https://hf-mirror.com",
            ghproxyBaseUrl: "https://mirror.ghproxy.com",
          };
        case "scan_companion_packs_cmd":
          return { packs: [], errors: [] };
        case "save_app_config_cmd":
          (window as unknown as { __savedAppConfig?: unknown }).__savedAppConfig = _args.config;
          return null;
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
  await expect(page.getByText("Current version")).toBeVisible();
  await expect(page.getByRole("main").getByText("v0.2.9")).toBeVisible();
  await expect(page.getByRole("button", { name: "Check for Updates" })).toBeVisible();
});

test("close-to-tray is visible in appearance and saves immediately", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();

  const trayOption = page.getByRole("button", { name: /Keep in system tray/ });
  await expect(trayOption).toBeVisible();
  await trayOption.click();
  await expect(trayOption).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { windowCloseBehavior?: string } }
  ).__savedAppConfig?.windowCloseBehavior)).toBe("minimize_to_tray");
});

test("desktop pet settings are visible and persist locally", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();

  const card = page.getByTestId("companion-settings-card");
  await expect(card.getByRole("heading", { name: "Desktop Pets" })).toBeVisible();
  await card.getByRole("button", { name: "Configure" }).click();
  await expect(card.getByText("Codex home")).toBeVisible();
  await card.getByRole("checkbox", { name: "Enable desktop pet" }).check();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __savedAppConfig?: { companion?: { enabled?: boolean } } }
  ).__savedAppConfig?.companion?.enabled)).toBe(true);
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

test("release notes include every GitHub release between current and latest", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("e2e-update-available", "1");
  });

  await page.goto("/settings");
  await page.getByRole("button", { name: "Appearance" }).click();
  await page.getByRole("button", { name: "Check for Updates" }).click();

  await expect(page.locator("p").filter({ hasText: /^v0\.10\.2$/ })).toBeVisible();
  await page.getByText("Release notes").click();
  await expect(page.getByText("First ranged release note.")).toBeVisible();
  await expect(page.getByText("Second ranged release note.")).toBeVisible();
  await expect(page.getByText("Already installed release note.")).toHaveCount(0);
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
