import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-locale", "en");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (value: string) => {
          (window as unknown as { __copiedQualityReport?: string }).__copiedQualityReport = value;
        },
      },
    });

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let qualityEvalCount = 0;

    const qualityReport = {
      status: "passed",
      total: 6,
      passed: 6,
      failed: 0,
      suites: [
        {
          id: "behavioral_routing",
          label: "Behavioral routing",
          total: 2,
          passed: 2,
          failed: 0,
          cases: [
            {
              id: "knowledge-question-searches-first",
              label: "Behavioral route: knowledge-question-searches-first",
              severity: "high",
              passed: true,
              checks: [
                { id: "route", passed: true, detail: "route=KnowledgeRetrieval expected=KnowledgeRetrieval" },
              ],
            },
          ],
        },
        {
          id: "evidence_policy",
          label: "Evidence and execution policy",
          total: 2,
          passed: 2,
          failed: 0,
          cases: [
            {
              id: "knowledge-retrieval-requires-verification",
              label: "Knowledge retrieval requires grounded verification",
              severity: "critical",
              passed: true,
              checks: [
                { id: "requiredTools", passed: true, detail: "search, retrieval, and verification tools are in the task plan" },
              ],
            },
          ],
        },
        {
          id: "workflow_catalog",
          label: "Workflow catalog",
          total: 1,
          passed: 1,
          failed: 0,
          cases: [
            {
              id: "catalog-templates-are-actionable",
              label: "Every workflow template has actionable prompt and task metadata",
              severity: "high",
              passed: true,
              checks: [
                { id: "acceptanceCriteria", passed: true, detail: "tasksWithCriteria=18/18" },
              ],
            },
          ],
        },
        {
          id: "checkpoint_recovery",
          label: "Checkpoint recovery and branching",
          total: 1,
          passed: 1,
          failed: 0,
          cases: [
            {
              id: "checkpoint-restore-and-branch",
              label: "Checkpoint restore and branch preserve recoverable context",
              severity: "critical",
              passed: true,
              checks: [
                { id: "restoreDropsSummary", passed: true, detail: "compaction summary is replaced with archived messages" },
              ],
            },
          ],
        },
      ],
      behavioralEval: {
        status: "passed",
        total: 2,
        passed: 2,
        failed: 0,
        cases: [],
      },
    };

    const invoke = async (cmd: string, _args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:app|version":
          return "0.2.9";
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
        case "check_office_runtime_cmd":
          return {
            status: "blocked",
            summary: "Python required",
            python: { status: "missing", executable: null, version: null, detail: "missing" },
            requiredPackages: [],
            optionalPackages: [],
          };
        case "run_agent_quality_eval_cmd":
          qualityEvalCount += 1;
          (window as unknown as { __qualityEvalCount: number }).__qualityEvalCount = qualityEvalCount;
          return qualityReport;
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
    (window as unknown as { __qualityEvalCount: number }).__qualityEvalCount = 0;

    (
      window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }
    ).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => {
        listeners.delete(eventId);
      },
    };
  });
});

test("settings exposes a runnable agent quality eval panel", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Agent Quality" }).click();

  await expect(page.getByRole("heading", { name: "Agent Quality" })).toBeVisible();
  await expect(page.getByText("Not run").first()).toBeVisible();

  await page.getByRole("button", { name: "Run local eval" }).click();

  await page.waitForFunction(
    () => (window as unknown as { __qualityEvalCount?: number }).__qualityEvalCount === 1,
    undefined,
    { timeout: 5000 },
  );
  await expect(page.getByText("Passed").first()).toBeVisible();
  await expect(page.getByText("6 / 6")).toBeVisible();
  await expect(page.getByText("Behavioral routing")).toBeVisible();
  await expect(page.getByText("Evidence and execution policy")).toBeVisible();
  await expect(page.getByText("Workflow catalog")).toBeVisible();
  await expect(page.getByText("Checkpoint recovery and branching")).toBeVisible();
  await expect(page.getByText("Checkpoint restore and branch preserve recoverable context")).toBeVisible();
  await expect(page.getByText("compaction summary is replaced with archived messages")).toBeVisible();

  await page.getByRole("button", { name: "Copy JSON report" }).click();
  const copied = await page.evaluate(() => (window as unknown as { __copiedQualityReport?: string }).__copiedQualityReport ?? "");
  const parsed = JSON.parse(copied) as { status: string; suites: Array<{ id: string }> };
  expect(parsed.status).toBe("passed");
  expect(parsed.suites.map((suite) => suite.id)).toContain("workflow_catalog");
});
