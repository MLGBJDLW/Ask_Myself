import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-locale", "en");

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let downloadModelCalls = 0;
    let activeModelProbes = 0;
    let maxConcurrentModelProbes = 0;
    let modelProbeCalls = 0;

    const runModelProbe = async <T,>(value: T): Promise<T> => {
      activeModelProbes += 1;
      modelProbeCalls += 1;
      maxConcurrentModelProbes = Math.max(maxConcurrentModelProbes, activeModelProbes);
      (window as unknown as {
        __modelProbeCalls: number;
        __maxConcurrentModelProbes: number;
      }).__modelProbeCalls = modelProbeCalls;
      (window as unknown as {
        __modelProbeCalls: number;
        __maxConcurrentModelProbes: number;
      }).__maxConcurrentModelProbes = maxConcurrentModelProbes;
      await new Promise((resolve) => setTimeout(resolve, 30));
      activeModelProbes -= 1;
      return value;
    };

    const appConfig = {
      cacheTtlHours: 24,
      defaultSearchLimit: 20,
      minSearchSimilarity: 0.2,
      maxTextFileSize: 104857600,
      maxVideoFileSize: 2147483648,
      maxAudioFileSize: 536870912,
      confirmDestructive: false,
      shellAccessMode: "open",
      toolApprovalMode: "allow_all",
      autoMemoryExtraction: true,
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

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:app|version":
          return "0.6.3";
        case "check_update_from_source_cmd":
          return null;
        case "plugin:updater|check":
          return null;
        case "plugin:event|listen": {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(args.event ?? ""),
            handlerId: Number(args.handler ?? 0),
          });
          return listenerId;
        }
        case "plugin:event|unlisten":
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        case "get_wizard_state_cmd":
          return { completed: true, language: "en", aiProvider: "open_ai", sourceAdded: true };
        case "list_agent_configs_cmd":
        case "list_conversations_cmd":
        case "list_sources":
        case "get_conversation_sources_cmd":
        case "list_checkpoints_cmd":
        case "list_user_memories_cmd":
        case "list_skills_cmd":
        case "list_mcp_servers_cmd":
        case "list_projects_cmd":
        case "list_capability_packages_cmd":
          return [];
        case "list_tool_permission_policies_cmd":
          return { persisted: [], session: [] };
        case "get_app_config_cmd":
          return appConfig;
        case "get_index_stats":
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case "get_privacy_config":
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case "get_embedder_config_cmd":
          return {
            provider: "local",
            apiKey: "",
            apiBaseUrl: "",
            apiModel: "",
            localModel: "MultilingualMiniLM",
            modelPath: "",
            vectorDimensions: 384,
          };
        case "check_local_model_cmd":
          return runModelProbe(false);
        case "download_local_model_cmd":
          downloadModelCalls += 1;
          (window as unknown as { __downloadModelCalls: number }).__downloadModelCalls = downloadModelCalls;
          return new Promise((resolve) => setTimeout(resolve, 250));
        case "get_ocr_config_cmd":
          return {
            enabled: false,
            confidenceThreshold: 0.5,
            llmFallbackEnabled: false,
            detLimitSideLen: 960,
            useCls: false,
            modelPath: "",
            languages: ["en"],
          };
        case "check_ocr_models_cmd":
          return runModelProbe(false);
        case "get_video_config_cmd":
          return {
            enabled: false,
            whisperModel: "tiny",
            language: null,
            translateToEnglish: false,
            ffmpegPath: null,
            frameExtractionEnabled: false,
            frameIntervalSecs: 30,
            modelPath: "",
            sceneThreshold: 0.5,
            useGpu: false,
            preferEmbeddedSubtitles: true,
            beamSize: 5,
          };
        case "check_whisper_model_cmd":
        case "check_ffmpeg_cmd":
          return runModelProbe(false);
        case "check_office_runtime_cmd":
          return runModelProbe({
            status: "ready",
            summary: "Ready",
            pythonPath: "python",
            appManagedPythonPath: null,
            appManagedEnvPath: "C:\\nexa\\office",
            skillScriptPath: "C:\\nexa\\office\\skill.py",
            requirementsPath: "C:\\nexa\\office\\requirements.txt",
            canPrepare: false,
            canInstallPythonPackages: false,
            needsPythonInstall: false,
            pythonDownloadUrl: "https://www.python.org/downloads/",
            dependencies: [],
          });
        default:
          return null;
      }
    };

    (window as unknown as { __downloadModelCalls: number }).__downloadModelCalls = 0;
    (window as unknown as {
      __modelProbeCalls: number;
      __maxConcurrentModelProbes: number;
    }).__modelProbeCalls = 0;
    (window as unknown as {
      __modelProbeCalls: number;
      __maxConcurrentModelProbes: number;
    }).__maxConcurrentModelProbes = 0;
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

test("model downloads coalesce rapid repeated clicks into one backend call", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();
  await page.getByRole("button", { name: /^Models Manage AI models/ }).click();

  const downloadButton = page.getByRole("button", { name: "Download Model" }).first();
  await expect(downloadButton).toBeEnabled();

  await downloadButton.evaluate((button) => {
    const el = button as HTMLButtonElement;
    el.click();
    el.click();
    el.click();
  });

  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __downloadModelCalls: number }).__downloadModelCalls))
    .toBe(1);
});

test("model status probes are serialized when the tab opens", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Models & Embedding" }).click();

  await expect
    .poll(() => page.evaluate(() => (
      window as unknown as { __modelProbeCalls: number }
    ).__modelProbeCalls))
    .toBeGreaterThanOrEqual(5);

  await expect
    .poll(() => page.evaluate(() => (
      window as unknown as { __maxConcurrentModelProbes: number }
    ).__maxConcurrentModelProbes))
    .toBe(1);
});
