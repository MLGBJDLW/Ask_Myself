import { expect, type Locator, test } from "@playwright/test";
import { readFileSync } from "node:fs";

const imageProviderPresets = JSON.parse(
  readFileSync(new URL("../../../shared/image-provider-presets.json", import.meta.url), "utf8"),
) as unknown[];

async function selectNexaOption(trigger: Locator, value: string) {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value=${JSON.stringify(value)}]`).click();
}

async function expectNexaValue(trigger: Locator, value: string) {
  await expect(trigger).toHaveAttribute("data-value", value);
}

async function backgroundAlpha(locator: Locator): Promise<number> {
  return locator.evaluate((element) => {
    const color = getComputedStyle(element).backgroundColor.trim();
    if (color === "transparent") return 0;
    const rgb = color.match(/^rgba?\((.+)\)$/i);
    if (rgb) {
      const parts = rgb[1].split(/[\s,\/]+/).filter(Boolean);
      return parts.length >= 4 ? Number(parts[3]) : 1;
    }
    const functionalAlpha = color.match(/\/\s*([0-9.]+)\s*\)$/);
    return functionalAlpha ? Number(functionalAlpha[1]) : 1;
  });
}

async function largeOpaqueSurfaceClasses(root: Locator): Promise<string[]> {
  return root.locator('*').evaluateAll((elements) => elements.flatMap((element) => {
    if (element.closest('[role="dialog"]')) return [];
    const surfaceClass = Array.from(element.classList).find((className) => /^bg-surface-[0-2]$/.test(className));
    if (!surfaceClass) return [];
    const bounds = element.getBoundingClientRect();
    if (bounds.width < 300 || bounds.height < 48 || bounds.width * bounds.height < 20_000) return [];
    const color = getComputedStyle(element).backgroundColor.trim();
    const rgb = color.match(/^rgba?\((.+)\)$/i);
    const functionalAlpha = color.match(/\/\s*([0-9.]+)\s*\)$/);
    const alpha = rgb
      ? (rgb[1].split(/[\s,\/]+/).filter(Boolean).length >= 4
          ? Number(rgb[1].split(/[\s,\/]+/).filter(Boolean)[3])
          : 1)
      : (functionalAlpha ? Number(functionalAlpha[1]) : (color === 'transparent' ? 0 : 1));
    if (alpha < 0.86) return [];
    return [`${element.tagName.toLowerCase()}.${surfaceClass}:${alpha.toFixed(2)}`];
  }));
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
  await page.addInitScript((runtimeImageProviderPresets) => {
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

    let registryReadMode = "registry";
    let registryRevision = 3;
    let appearanceRegistry = {
      version: 2,
      initialized: false,
      revision: 0,
      activeThemeId: "dark",
      previousThemeId: null as string | null,
      plugins: [] as Array<Record<string, unknown>>,
    };
    const registryProjection = () => ({
      schemaVersion: 1,
      settingsRevisions: [{
        profileId: "settings-v2:agent:cfg-qwen",
        scope: { kind: "agent", id: "cfg-qwen" },
        revision: 3,
      }],
      connections: [{
        schemaVersion: 1,
        id: "connection:qwen",
        revision: 3,
        adapterProviderId: "qwen",
        providerId: "alibaba_model_studio",
        endpointId: "text:qwen-cloud-cn",
        baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        endpointFingerprint: "endpoint:qwen",
        credentialRef: "legacy-agent-config:cfg-qwen",
        enabled: true,
        health: "configured",
        source: { kind: "agent", id: "cfg-qwen" },
        sourceRevision: 3,
      }],
      modelDefinitions: [],
      modelTargets: [{
        id: "target:qwen",
        revision: 3,
        connectionId: "connection:qwen",
        upstreamModelId: "qwen3.6-plus",
        availability: "callable",
        source: { kind: "agent", id: "cfg-qwen" },
        sourceRevision: 3,
      }],
      capabilities: [{
        bindingId: "binding:qwen",
        bindingRevision: 3,
        capabilityId: "text_generation",
        source: { kind: "agent", id: "cfg-qwen" },
        sourceRevision: 3,
        primary: {
          target: {
            id: "target:qwen",
            revision: 3,
            connectionId: "connection:qwen",
            upstreamModelId: "qwen3.6-plus",
            availability: "callable",
            source: { kind: "agent", id: "cfg-qwen" },
            sourceRevision: 3,
          },
          connection: {
            schemaVersion: 1,
            id: "connection:qwen",
            revision: 3,
            adapterProviderId: "qwen",
            providerId: "alibaba_model_studio",
            endpointId: "text:qwen-cloud-cn",
            baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
            endpointFingerprint: "endpoint:qwen",
            credentialRef: "legacy-agent-config:cfg-qwen",
            enabled: true,
            health: "configured",
            source: { kind: "agent", id: "cfg-qwen" },
            sourceRevision: 3,
          },
          eligibility: { eligible: true, reasonCodes: [] },
        },
        fallbacks: [],
        fallbackMode: "disabled",
        constraints: {
          requireSameConnection: true,
          allowCrossProvider: false,
          allowCrossRegion: false,
          requiresStreaming: false,
          allowedRegions: [],
          dataClasses: [],
        },
      }],
      activations: [{
        capabilityId: "text_generation",
        scope: { kind: "agent", id: "cfg-qwen" },
        readMode: registryReadMode,
        registryRevision,
        parityStatus: "matched",
        parity: { status: "matched" },
      }],
    });

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
        case "plugin:shell|open":
          localStorage.setItem("nexa-e2e-opened-subscription-url", String(_args.path ?? ""));
          return null;
        case "get_wizard_state_cmd":
          return { completed: true };
        case "hydrate_appearance_registry_cmd":
          if (!appearanceRegistry.initialized) {
            appearanceRegistry = {
              ...appearanceRegistry,
              initialized: true,
              revision: appearanceRegistry.revision + 1,
              activeThemeId: String(_args.activeThemeId ?? "dark"),
              plugins: clone((_args.plugins as Array<Record<string, unknown>> | undefined) ?? []),
            };
          }
          return clone(appearanceRegistry);
        case "get_appearance_registry_cmd":
          return clone(appearanceRegistry);
        case "apply_appearance_plugin_cmd": {
          if (localStorage.getItem("nexa-test-reject-appearance") === "true") {
            throw new Error("simulated appearance persistence failure");
          }
          const plugin = clone(_args.plugin as Record<string, unknown>);
          const id = String(plugin.id ?? "");
          appearanceRegistry = {
            ...appearanceRegistry,
            initialized: true,
            revision: appearanceRegistry.revision + 1,
            previousThemeId: appearanceRegistry.activeThemeId === id
              ? appearanceRegistry.previousThemeId
              : appearanceRegistry.activeThemeId,
            activeThemeId: id,
            plugins: [...appearanceRegistry.plugins.filter(item => item.id !== id), plugin],
          };
          return clone(appearanceRegistry);
        }
        case "activate_appearance_cmd": {
          const nextId = String(_args.themeId ?? "dark");
          appearanceRegistry = {
            ...appearanceRegistry,
            revision: appearanceRegistry.revision + 1,
            previousThemeId: appearanceRegistry.activeThemeId,
            activeThemeId: nextId,
          };
          return clone(appearanceRegistry);
        }
        case "rollback_appearance_cmd": {
          const previous = appearanceRegistry.previousThemeId;
          if (!previous) throw new Error("no previous appearance");
          appearanceRegistry = {
            ...appearanceRegistry,
            revision: appearanceRegistry.revision + 1,
            activeThemeId: previous,
            previousThemeId: appearanceRegistry.activeThemeId,
          };
          return clone(appearanceRegistry);
        }
        case "remove_appearance_cmd": {
          const removedId = String(_args.themeId ?? "");
          appearanceRegistry = {
            ...appearanceRegistry,
            revision: appearanceRegistry.revision + 1,
            activeThemeId: appearanceRegistry.activeThemeId === removedId ? "dark" : appearanceRegistry.activeThemeId,
            plugins: appearanceRegistry.plugins.filter(item => item.id !== removedId),
          };
          return clone(appearanceRegistry);
        }
        case "list_agent_configs_cmd":
          return [clone(anthropicConfig), clone(qwenConfig)];
        case "get_codex_account_snapshot_cmd": {
          const state = localStorage.getItem("nexa-e2e-codex-account") ?? "signed-in";
          const oneShotDelayMs = Number(
            localStorage.getItem("nexa-e2e-codex-next-snapshot-delay-ms") ?? 0,
          );
          if (oneShotDelayMs > 0) {
            localStorage.removeItem("nexa-e2e-codex-next-snapshot-delay-ms");
            await new Promise((resolve) => window.setTimeout(resolve, oneShotDelayMs));
          }
          if (state === "pending") {
            const delayMs = Number(localStorage.getItem("nexa-e2e-codex-snapshot-delay-ms") ?? 0);
            if (delayMs > 0) {
              const active = Number(localStorage.getItem("nexa-e2e-codex-snapshot-active") ?? 0) + 1;
              const maximum = Math.max(
                active,
                Number(localStorage.getItem("nexa-e2e-codex-snapshot-max-active") ?? 0),
              );
              localStorage.setItem("nexa-e2e-codex-snapshot-active", String(active));
              localStorage.setItem("nexa-e2e-codex-snapshot-max-active", String(maximum));
              await new Promise((resolve) => window.setTimeout(resolve, delayMs));
              localStorage.setItem("nexa-e2e-codex-snapshot-active", String(active - 1));
            }
          }
          const base = {
            available: true,
            runtimeVersion: "codex-cli 0.153.0",
            errorCode: null,
            requiresOpenaiAuth: true,
            rateLimits: [],
            usage: null,
            pendingLogin: null,
            lastLogin: null,
          };
          if (state === "signed-in") {
            return {
              ...base,
              account: { accountType: "chatgpt", email: "reader@example.com", planType: "pro" },
              rateLimits: [{
                id: "codex",
                name: "Codex",
                planType: "pro",
                primary: { usedPercent: 18, windowDurationMins: 300, resetsAt: 1788426673 },
                secondary: null,
              }],
              usage: { lifetimeTokens: 1234567, currentStreakDays: 4 },
            };
          }
          if (state === "pending") {
            return {
              ...base,
              account: null,
              pendingLogin: {
                loginId: "login-device",
                kind: "deviceCode",
                authUrl: null,
                verificationUrl: "https://auth.openai.com/codex/device",
                userCode: "ABCD-EFGH",
              },
            };
          }
          return { ...base, account: null };
        }
        case "start_codex_account_login_cmd": {
          const kind = String(_args.kind ?? "browser");
          localStorage.setItem("nexa-e2e-codex-account", "pending");
          return kind === "deviceCode"
            ? {
                loginId: "login-device",
                kind: "deviceCode",
                authUrl: null,
                verificationUrl: "https://auth.openai.com/codex/device",
                userCode: "ABCD-EFGH",
              }
            : {
                loginId: "login-browser",
                kind: "browser",
                authUrl: "https://auth.openai.com/oauth/authorize",
                verificationUrl: null,
                userCode: null,
              };
        }
        case "cancel_codex_account_login_cmd":
          localStorage.setItem("nexa-e2e-codex-account", "signed-out");
          return null;
        case "logout_codex_account_cmd":
          localStorage.setItem("nexa-e2e-codex-account", "signed-out");
          return {
            available: true,
            runtimeVersion: "codex-cli 0.153.0",
            errorCode: null,
            requiresOpenaiAuth: true,
            account: null,
            rateLimits: [],
            usage: null,
            pendingLogin: null,
            lastLogin: null,
          };
        case "get_copilot_account_snapshot_cmd": {
          const state = localStorage.getItem("nexa-e2e-copilot-account") ?? "verified";
          const base = {
            available: true,
            runtimeVersion: "1.0.79",
            errorCode: null,
            authenticated: false,
            entitlementVerified: false,
            authType: null,
            login: null,
            host: "https://github.com",
            models: [],
            quotas: [],
            loginPending: false,
            loginError: null,
          };
          if (state === "pending") return { ...base, loginPending: true };
          if (state === "signed-out") return base;
          return {
            ...base,
            authenticated: true,
            entitlementVerified: true,
            authType: "user",
            login: "octocat",
            models: [
              { id: "claude-sonnet-4.6", name: "Claude Sonnet 4.6", reasoningEfforts: [] },
              { id: "gpt-5.4", name: "GPT-5.4", reasoningEfforts: ["low", "medium", "high"] },
            ],
            quotas: [{
              id: "premium_interactions",
              remainingPercent: 64,
              resetDate: "2026-10-01T00:00:00Z",
              unlimited: false,
            }],
          };
        }
        case "start_copilot_account_login_cmd":
          localStorage.setItem("nexa-e2e-copilot-account", "pending");
          return null;
        case "cancel_copilot_account_login_cmd":
          localStorage.setItem("nexa-e2e-copilot-account", "signed-out");
          return null;
        case "list_agent_task_run_summaries_cmd":
          return { items: [], nextCursor: null };
        case "list_conversations_cmd":
          return [];
        case "list_sources":
        case "list_workflow_templates_cmd":
        case "list_workflow_automations_cmd":
        case "list_workflow_automation_approvals_cmd":
        case "list_due_workflow_automations_cmd":
        case "list_projects_cmd":
        case "get_conversation_sources_cmd":
        case "list_checkpoints_cmd":
        case "list_user_memories_cmd":
        case "list_agent_procedural_memories_cmd":
        case "list_personas_cmd":
        case "list_skills_cmd":
        case "get_web_search_status_cmd":
          return [];
        case "list_mcp_servers_cmd": {
          if (localStorage.getItem("e2e-mcp-reload-race") !== "1") return [];
          const reloaded = localStorage.getItem("e2e-mcp-config-reloaded") === "1";
          if (reloaded) {
            await new Promise(resolve => setTimeout(resolve, 250));
          }
          return [{
            id: "user-json:docs",
            name: "Docs",
            transport: "streamable_http",
            command: null,
            args: null,
            url: "https://example.com/mcp",
            envJson: null,
            headersJson: null,
            enabled: !reloaded,
            createdAt: nowIso,
            updatedAt: nowIso,
            builtinId: null,
          }];
        }
        case "list_mcp_tools_cmd": {
          const calls = Number(localStorage.getItem("e2e-mcp-list-tools-calls") ?? "0");
          localStorage.setItem("e2e-mcp-list-tools-calls", String(calls + 1));
          return [];
        }
        case "get_user_extension_layout_cmd":
          return {
            version: 1,
            root: "C:\\Users\\Test\\.nexa",
            capabilitiesDir: "C:\\Users\\Test\\.nexa\\capabilities",
            skillsDir: "C:\\Users\\Test\\.nexa\\skills",
            themesDir: "C:\\Users\\Test\\.nexa\\themes",
            workflowsDir: "C:\\Users\\Test\\.nexa\\workflows",
            connectorsDir: "C:\\Users\\Test\\.nexa\\connectors",
            mcpConfigPath: "C:\\Users\\Test\\.nexa\\connectors\\mcp.json",
            legacyAppDataDir: "C:\\Users\\Test\\AppData\\Roaming\\com.nexa.desktop",
          };
        case "reload_user_skill_files_cmd":
          localStorage.setItem("e2e-user-skill-files-reloaded", "1");
          return { updated: 1, unchanged: 0, unregistered: 0, rejected: [] };
        case "prepare_mcp_config_file_cmd":
          return "C:\\Users\\Test\\.nexa\\connectors\\mcp.json";
        case "reload_mcp_config_file_cmd":
          localStorage.setItem("e2e-mcp-config-reloaded", "1");
          return {
            path: "C:\\Users\\Test\\.nexa\\connectors\\mcp.json",
            imported: 0,
            removed: 0,
            disabledAfterChange: 0,
          };
        case "open_file_in_default_app":
          localStorage.setItem("e2e-opened-mcp-config", String(_args.path ?? ""));
          return null;
        case "get_recent_queries": {
          const recentQueries = JSON.parse(localStorage.getItem("nexa-e2e-recent-queries") ?? "[]") as unknown;
          return clone(Array.isArray(recentQueries) ? recentQueries : []);
        }
        case "list_skill_change_proposals_cmd": {
          const proposals = JSON.parse(localStorage.getItem("nexa-e2e-skill-change-proposals") ?? "[]") as unknown;
          return clone(Array.isArray(proposals) ? proposals : []);
        }
        case "set_conversation_sources_cmd":
        case "update_conversation_system_prompt_cmd":
        case "compact_conversation_cmd":
        case "agent_stop_cmd":
        case "get_learning_governance_snapshot_cmd":
          return null;
        case "get_index_stats":
          return { totalDocuments: 0, totalChunks: 0, ftsRows: 0 };
        case "get_privacy_config":
          return { enabled: false, excludePatterns: [], redactPatterns: [] };
        case "get_embedder_config_cmd":
          return clone(embedderConfig);
        case "get_app_config_cmd":
          return clone(appConfig);
        case "get_capability_registry_projection_cmd":
          return clone(registryProjection());
        case "set_capability_registry_read_mode_cmd":
          registryReadMode = String(_args.mode);
          registryRevision += 1;
          (window as unknown as { __registryModeArgs?: unknown }).__registryModeArgs = clone(_args);
          return clone(registryProjection().activations[0]);
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
              providerCatalogs: [
                {
                  id: "imageProviders",
                  label: "Image providers",
                  itemKind: "imageProviderPreset",
                  items: runtimeImageProviderPresets,
                },
              ],
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
        case "generate_theme_resource_plugin_cmd":
          return {
            manifestVersion: 2,
            kind: "theme-resource",
            id: "theme-generated-ocean",
            name: "Generated Ocean",
            description: String(_args.description ?? "").slice(0, 500),
            theme: {
              baseTheme: "dark",
              mode: "dark",
              colors: {
                surface0: "#08131f",
                surface1: "#102235",
                textPrimary: "#f2f8ff",
                textSecondary: "#a8bed1",
                thinkingText: "#7dd3fc",
                replyText: "#fef3c7",
                accent: "#38bdf8",
              },
              effects: { surfaceOpacity: 0.9, glassBlur: 14 },
              typography: {},
              motion: { cursorStyle: "fluid" },
              brand: { logoVariant: "auto" },
              content: { tagline: "Quiet focus", statusText: "Ready to explore" },
              components: {},
              background: {
                kind: "gradient",
                value: "linear-gradient(145deg, #08131f, #164e63)",
                fit: "cover",
                position: "center",
              },
            },
          };
        case "generate_theme_background_cmd":
        case "resolve_theme_background_cmd":
          return {
            assetId: "a".repeat(64),
            path: "C:\\Nexa\\themes\\generated-ocean.png",
            mediaType: "image/png",
            bytes: 2048,
          };
        case "garbage_collect_theme_assets_cmd":
          return { removedFiles: 0, removedBytes: 0 };
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
  }, imageProviderPresets);
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
    "Gemini 3.8 Flash",
    "Gemini 3.7 Flash",
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
    "Qwen3.8 Flash",
    "Qwen3.8 2.4T A95B",
    "Qwen3.8 27B",
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

test("settings exposes the secret-free registry and durable runtime rollback", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const registryDisclosure = page.getByTestId("capability-registry-disclosure-trigger");
  await expect(registryDisclosure).toBeVisible();
  await expect(registryDisclosure).toHaveAttribute("aria-expanded", "false");
  await expect(registryDisclosure).toHaveAttribute("aria-controls", /.+-panel$/);
  await expect(registryDisclosure).toContainText("1 connections · 1 models · 1 routes");
  await expect(page.getByTestId("capability-registry-panel")).toHaveCount(0);

  await registryDisclosure.click();
  await expect(registryDisclosure).toHaveAttribute("aria-expanded", "true");
  const registry = page.getByTestId("capability-registry-panel");
  await expect(registry).toBeVisible();
  await expect(registry.getByTestId("registry-connections")).toContainText("Alibaba Model Studio");
  await expect(registry.getByTestId("registry-connections")).toContainText("Configured");
  await expect(registry.getByTestId("registry-models")).toContainText("qwen3.6-plus");
  await expect(registry.getByTestId("registry-capabilities")).toContainText("Text Generation");
  await expect(registry.getByTestId("registry-capabilities")).toContainText("Registry");
  await expect(page.getByText("sk-qwen-demo")).toHaveCount(0);

  await registry.getByRole("button", { name: "Use legacy" }).click();
  await expect(registry.getByTestId("registry-capabilities")).toContainText("Legacy");
  const args = await page.evaluate(() => (
    window as unknown as { __registryModeArgs?: unknown }
  ).__registryModeArgs);
  expect(args).toMatchObject({
    capabilityId: "text_generation",
    mode: "legacy",
    expectedRevision: 3,
    scope: { kind: "agent", id: "cfg-qwen" },
  });
});

test("provider catalog prioritizes configured entries and reflows at 320px", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const providerCards = page.locator("[data-provider-preset-id]");
  await expect(providerCards.nth(0)).toHaveAttribute("data-provider-preset-id", "anthropic");
  await expect(providerCards.nth(1)).toHaveAttribute("data-provider-preset-id", "alibaba-model-studio");
  await expect(providerCards.nth(0)).toContainText("Configured");
  await expect(providerCards.nth(1)).toContainText("Configured");
  await expect(providerCards.last()).toHaveAttribute("data-provider-preset-id", "custom");

  const search = page.getByRole("searchbox", { name: "Search providers" });
  await search.fill("qwen3.6-plus");
  expect(await providerCards.count()).toBeGreaterThan(0);
  await expect(page.locator('[data-provider-preset-id="alibaba-model-studio"]')).toBeVisible();
  await expect(page.locator('[data-provider-preset-id="custom"]')).toHaveCount(0);

  await search.fill("no-provider-can-match-this");
  await expect(providerCards).toHaveCount(0);
  await expect(page.getByText("No providers match this search.")).toBeVisible();
  await search.press("Escape");
  await expect(providerCards).not.toHaveCount(0);

  await page.setViewportSize({ width: 320, height: 760 });
  const providerGrid = providerCards.first().locator("xpath=..");
  const gridSize = await providerGrid.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  expect(gridSize.scrollWidth).toBeLessThanOrEqual(gridSize.clientWidth);
});

test("provider output limit is automatic when the explicit cap is cleared", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByTitle("Edit").first().click();

  const maxTokensField = page
    .locator("label")
    .filter({ hasText: "Per-request output limit" })
    .locator("xpath=..");
  const input = maxTokensField.getByRole("spinbutton");
  await expect(input).toHaveValue("4096");
  await input.fill("");
  await expect(input).toHaveAttribute("placeholder", "Auto (verified model capability)");
  await expect(maxTokensField).toContainText("verified model-catalog output capability");
  const toolRoundsInput = page
    .locator("label")
    .filter({ hasText: "Max Verified Tool Rounds" })
    .locator("xpath=..")
    .getByRole("spinbutton");
  await toolRoundsInput.fill("0");

  const form = maxTokensField.locator("xpath=ancestor::form");
  const invalidInputs = await form.locator("input:invalid").evaluateAll((inputs) =>
    inputs.map((input) => {
      const element = input as HTMLInputElement;
      return {
        name: element.getAttribute("name"),
        value: element.value,
        min: element.min,
        max: element.max,
        validationMessage: element.validationMessage,
      };
    }),
  );
  expect(invalidInputs).toEqual([]);
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __savedAgentConfig?: { maxTokens?: number | null; maxIterations?: number | null };
    }
  ).__savedAgentConfig)).toEqual(expect.objectContaining({
    maxTokens: null,
    maxIterations: 0,
  }));
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

test("settings exposes Meta Model API with Muse Spark 1.3 as its verified default", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const metaCard = page.getByRole("button", { name: /^Meta Model API/ });
  await expect(metaCard).toBeVisible();
  await expect(metaCard.locator('[title="Meta"]')).toBeVisible();
  await metaCard.click();

  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await expect(baseUrlField.getByRole("textbox")).toHaveValue("https://api.meta.ai/v1");

  const modelField = page.getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaValue(modelSelect, "muse-spark-1.3");
  await expectNexaOptions(modelSelect, ["Muse Spark 1.3"]);
  await expect(modelField.getByTestId("model-descriptor-badges")).toContainText("text+image→text");

  await page.getByRole("button", { name: /^Advanced Settings/ }).click();
  const alwaysOn = page.getByRole("checkbox", {
    name: "Reasoning is always on for this model.",
  });
  await expect(alwaysOn).toBeChecked();
  await expect(alwaysOn).toBeDisabled();
  const effortSelect = page
    .locator("label")
    .filter({ hasText: "Reasoning Effort" })
    .locator("xpath=..")
    .locator("[data-nexa-select-trigger]");
  await expectNexaOptions(effortSelect, ["Minimal", "Low", "Medium", "High"]);
  await expectNexaOptionCount(effortSelect, 4);
});

test("settings exposes current Qwen3.8 Token Plan models with the retired preview disabled", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  const tokenPlanCard = page.getByRole("button", { name: /^Qwen Token Plan/ });
  await expect(tokenPlanCard).toContainText("sk-sp key");
  const globalTokenPlanCard = page.getByRole("button", {
    name: /^QwenCloud Token Plan \(Global\)/,
  });
  await expect(globalTokenPlanCard).toBeVisible();
  await expect(globalTokenPlanCard).toContainText("sk-sp key");
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
  await expectNexaOptions(modelSelect, ["Qwen3.8 Max", "Qwen3.8 Flash"]);
  await expectNexaOptionCount(modelSelect, 3);
  await expectNexaOption(modelSelect, "qwen3.7-flash", "absent");
  await modelSelect.click();
  const retiredPreview = page.locator('[role="option"][data-value="qwen3.8-max-preview"]');
  await expect(retiredPreview).toBeVisible();
  await expect(retiredPreview).toHaveAttribute("data-disabled", "true");
  await page.keyboard.press("Escape");
  await selectNexaOption(modelSelect, "qwen3.8-flash");
  await expect(modelField.getByTestId("model-descriptor-badges")).toContainText("Access: account enablement");
});

test("settings projects the Codex subscription account and usage without an API key", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const account = page.getByTestId("codex-subscription-account");
  await expect(page.getByText("Accounts & subscription agents", { exact: true })).toBeVisible();
  await expect(account).toContainText("reader@example.com");
  await expect(account).toContainText("Pro");
  await expect(account).toContainText("82% remaining");
  await expect(account).toContainText("1,234,567 lifetime tokens");
  await expect(account).not.toContainText("API key");
});

test("settings completes the official Codex device-code launch and cancellation flow", async ({ page }) => {
  await page.goto("/settings");
  await page.evaluate(() => {
    localStorage.setItem("nexa-e2e-codex-account", "signed-out");
    localStorage.setItem("nexa-e2e-codex-snapshot-delay-ms", "2000");
    localStorage.setItem("nexa-e2e-codex-snapshot-active", "0");
    localStorage.setItem("nexa-e2e-codex-snapshot-max-active", "0");
  });
  await page.reload();
  await page.getByRole("button", { name: "AI Providers" }).click();

  const account = page.getByTestId("codex-subscription-account");
  await account.getByRole("button", { name: "Use device code" }).click();
  await expect(account.getByTestId("codex-login-pending")).toContainText("ABCD-EFGH");
  await expect.poll(() => page.evaluate(() => (
    localStorage.getItem("nexa-e2e-opened-subscription-url")
  ))).toBe("https://auth.openai.com/codex/device");
  await page.waitForTimeout(3_800);
  await expect.poll(() => page.evaluate(() => Number(
    localStorage.getItem("nexa-e2e-codex-snapshot-max-active") ?? 0,
  ))).toBe(1);
  await page.evaluate(() => localStorage.removeItem("nexa-e2e-codex-snapshot-delay-ms"));
  await account.getByRole("button", { name: "Cancel" }).click();
  await expect(account.getByTestId("codex-login-pending")).toHaveCount(0);
});

test("settings discards a stale account refresh after login starts", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("nexa-e2e-codex-account", "signed-out"));
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const account = page.getByTestId("codex-subscription-account");
  await expect(account.getByRole("button", { name: "Use device code" })).toBeVisible();
  await page.evaluate(() => {
    localStorage.setItem("nexa-e2e-codex-next-snapshot-delay-ms", "600");
  });
  await account.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    localStorage.getItem("nexa-e2e-codex-next-snapshot-delay-ms")
  ))).toBeNull();
  await account.getByRole("button", { name: "Use device code" }).click();
  await expect(account.getByTestId("codex-login-pending")).toContainText("ABCD-EFGH");

  await page.waitForTimeout(900);
  await expect(account.getByTestId("codex-login-pending")).toContainText("ABCD-EFGH");
});

test("settings verifies GitHub Copilot subscription models and quota through the SDK", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();

  const account = page.getByTestId("copilot-subscription-account");
  await expect(account).toContainText("Subscription verified");
  await expect(account).toContainText("octocat");
  await expect(account).toContainText("2 subscription models available");
  await expect(account).toContainText("Claude Sonnet 4.6");
  await expect(account).toContainText("GPT-5.4");
  await expect(account).toContainText("64% remaining");
  await expect(account).not.toContainText(/(?:ghu_|gho_|github_pat_)[A-Za-z0-9_]+/);
});

test("settings starts and cancels the official Copilot CLI browser login", async ({ page }) => {
  await page.goto("/settings");
  await page.evaluate(() => localStorage.setItem("nexa-e2e-copilot-account", "signed-out"));
  await page.reload();
  await page.getByRole("button", { name: "AI Providers" }).click();

  const account = page.getByTestId("copilot-subscription-account");
  await account.getByRole("button", { name: "Sign in with GitHub" }).click();
  await expect(account.getByTestId("copilot-login-pending")).toBeVisible();
  await account.getByRole("button", { name: "Cancel" }).click();
  await expect(account.getByTestId("copilot-login-pending")).toHaveCount(0);
});

test("settings selects the public GLM-5.3 flagship route by default", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "AI Providers" }).click();
  await page.getByRole("button", { name: "Add Provider" }).click();

  await expect(page.getByRole("button", { name: /GLM Coding Plan/ })).toBeVisible();
  const zhipuCard = page.getByRole("button", { name: /^Zhipu \(GLM\)/ });
  await expect(zhipuCard).toContainText("GLM-5.3 flagship and GLM-5.3-Flash multimodal Model APIs");
  await zhipuCard.click();

  const baseUrlField = page
    .locator("label")
    .filter({ hasText: "Base URL" })
    .locator("xpath=..");
  await expect(baseUrlField.getByRole("textbox")).toHaveValue(
    "https://open.bigmodel.cn/api/paas/v4",
  );

  const modelField = page.getByTestId("default-model-field");
  const modelSelect = modelField.locator("[data-nexa-select-trigger]");
  await expectNexaValue(modelSelect, "glm-5.3");
  await modelSelect.click();
  const glm53 = page.locator('[role="option"][data-value="glm-5.3"]');
  await expect(glm53).toBeVisible();
  await expect(glm53).not.toContainText("Unavailable for this credential");
  await expect(glm53).toHaveAttribute("data-disabled", "false");
  await page.keyboard.press("Escape");
});

test("settings exposes the complete current Qwen3.8 family through QwenCloud international", async ({ page }) => {
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
  await expectNexaValue(modelSelect, "qwen3.8-max");
  await expectNexaOptions(modelSelect, [
    "Qwen3.8 Max",
    "Qwen3.8 Flash",
    "Qwen3.8 2.4T A95B",
    "Qwen3.8 27B",
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
  await expectNexaOptions(modelSelect, [
    "Qwen3.8 Max",
    "Qwen3.8 Flash",
    "Qwen3.8 2.4T A95B",
    "Qwen3.8 27B",
    "Qwen3.7 Max",
    "DeepSeek V4 Pro",
  ]);
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
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.message));

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
  expect(pageErrors).toEqual([]);
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

test("appearance installs the backend-normalized theme draft only after explicit save", async ({ page }) => {
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

  const requestedDescription = `calm moonlit ocean with cyan glass ${"and quiet stars ".repeat(40)}`;
  const normalizedDescription = requestedDescription.slice(0, 500);
  await page.getByPlaceholder(/calm moonlit ocean/i).fill(requestedDescription);
  await page.getByRole("button", { name: "Generate theme draft" }).click();
  await expect(page.getByLabel("Name")).toHaveValue("Generated Ocean");
  await expect(page.getByTestId("theme-preview-thinking-text")).toHaveCSS("color", "rgb(125, 211, 252)");
  await expect(page.getByTestId("theme-preview-reply-text")).toHaveCSS("color", "rgb(254, 243, 199)");
  await page.getByRole("button", { name: "Controlled component styles" }).click();
  await page.getByRole("group", { name: "Navigation rail" }).getByLabel("Background").fill("rgba(8, 19, 31, 0.92)");
  await expect.poll(() => page.evaluate(() => (
    JSON.parse(localStorage.getItem("nexa-theme-resource-plugins-v2") ?? "[]") as unknown[]
  ).length)).toBe(0);

  await page.getByRole("button", { name: "Generate matching image" }).click();
  await page.getByRole("button", { name: "Save and apply" }).click();
  await expect.poll(() => page.evaluate(() => {
    const stored = JSON.parse(localStorage.getItem("nexa-theme-resource-plugins-v2") ?? "[]") as Array<{
      id?: string;
      description?: string;
      theme?: { background?: { assetId?: string }; components?: { rail?: { background?: string } } };
    }>;
    return stored[0];
  })).toEqual(expect.objectContaining({
    id: "theme-generated-ocean",
    description: normalizedDescription,
    theme: expect.objectContaining({
      background: expect.objectContaining({ assetId: "a".repeat(64) }),
      components: expect.objectContaining({
        rail: expect.objectContaining({ background: "rgba(8, 19, 31, 0.92)" }),
      }),
    }),
  }));

  await page.evaluate(() => localStorage.setItem("nexa-test-reject-appearance", "true"));
  await page.getByLabel("Name").fill("Rejected Update");
  await page.getByRole("button", { name: "Save and apply" }).click();
  await expect.poll(() => page.evaluate(() => {
    const stored = JSON.parse(localStorage.getItem("nexa-theme-resource-plugins-v2") ?? "[]") as Array<{ name?: string }>;
    return { name: stored[0]?.name, active: localStorage.getItem("nexa-active-theme-v1") };
  })).toEqual({ name: "Generated Ocean", active: "theme-generated-ocean" });
});

test("web search settings expose native provider support and persist OpenRouter engine priority", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Extensions", exact: true }).click();

  const section = page.locator("section").filter({
    has: page.getByRole("heading", { name: "Web search", exact: true }),
  });
  await section.getByRole("button").first().click();

  const support = section.getByTestId("provider-native-search-support");
  await expect(support).toContainText("OpenAI");
  await expect(support).toContainText("xAI");
  await expect(support).toContainText("OpenRouter");
  await expect(support).toContainText("openRouterServerTool");

  const engineField = section.locator("label").filter({ hasText: "Provider-native engine" });
  await selectNexaOption(engineField.locator("[data-nexa-select-trigger]"), "exa");
  await section.getByRole("button", { name: "Save", exact: true }).click();

  await expect.poll(() => page.evaluate(() => (
    window as unknown as {
      __savedAppConfig?: { webSearch?: { providerNativeEngine?: string } };
    }
  ).__savedAppConfig?.webSearch?.providerNativeEngine)).toBe("exa");
});

test("Theme Studio isolates a dark draft from the active light palette", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("nexa-active-theme-v1", "light");
    localStorage.setItem("nexa-theme", "light");
  });
  await page.goto("/settings");
  await page.getByTestId("theme-summary-card").getByRole("button", { name: "Open Theme Studio" }).click();

  const previewSurface2 = await page.getByTestId("theme-live-preview").evaluate((element) => (
    getComputedStyle(element).getPropertyValue("--color-surface-2").trim().toLowerCase()
  ));
  expect(previewSurface2).toBe("#1a1a25");
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
  const appearanceHeading = page.getByRole("heading", { name: "Appearance", exact: true });
  await expect(appearanceHeading).toBeVisible();
  const shell = page.locator('[data-app-area]');
  const workspace = page.locator('[data-theme-surface="workspace"]');
  await shell.evaluate((element) => element.setAttribute('data-app-area', 'home'));
  const homeBackdrop = shell.locator('.app-theme-backdrop');
  await expect(homeBackdrop).toHaveCSS('pointer-events', 'none');
  await expect(homeBackdrop).toHaveCSS('opacity', '0.92');
  expect(await backgroundAlpha(workspace)).toBe(0);
  await page.screenshot({ path: 'test-results/dream-home.png', fullPage: true });

  await shell.evaluate((element) => element.setAttribute('data-app-area', 'task'));
  await expect(shell.locator('.app-theme-backdrop')).toHaveCSS('opacity', '0.72');
  const appearancePanel = appearanceHeading.locator('xpath=ancestor::section[1]');
  const appearancePanelAlpha = await backgroundAlpha(appearancePanel);
  expect(appearancePanelAlpha).toBeGreaterThanOrEqual(0.62);
  expect(appearancePanelAlpha).toBeLessThanOrEqual(0.78);
  await page.screenshot({ path: 'test-results/dream-settings.png', fullPage: true });

  const cdpSession = await page.context().newCDPSession(page);
  await cdpSession.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-transparency", value: "reduce" }],
  });
  await expect(homeBackdrop).toBeHidden();
  expect(await backgroundAlpha(workspace)).toBe(1);
  await expect(workspace).toHaveCSS("backdrop-filter", "none");
  await expect(workspace).not.toHaveCSS("background-color", "rgb(255, 255, 255)");
  await cdpSession.send("Emulation.setEmulatedMedia", { features: [] });
  await cdpSession.detach();
});

test("custom wallpaper appearances cover every workspace surface without sacrificing chat readability", async ({ page }) => {
  await page.addInitScript(() => {
    const plugin = {
      manifestVersion: 2,
      kind: "theme-resource",
      id: "wallpaper-coverage",
      name: "Wallpaper Coverage",
      theme: {
        baseTheme: "light",
        mode: "light",
        colors: {
          surface0: "#f6eee8",
          surface1: "#fff8f2",
          surface2: "#f4ded2",
          surface3: "#ead0c2",
          surface4: "#dfbca9",
          textPrimary: "#251913",
          textSecondary: "#59443a",
          textTertiary: "#786056",
          accent: "#c85d2e",
        },
        effects: { surfaceOpacity: 0.72, glassBlur: 18 },
        typography: {},
        motion: {},
        brand: {},
        content: {},
        components: {},
        background: {
          kind: "gradient",
          value: "linear-gradient(135deg, #4f2418, #e09158)",
          opacity: 1,
          dim: 0.18,
          overlayColor: "#1f100b",
        },
      },
    };
    localStorage.setItem("nexa-theme-resource-plugins-v2", JSON.stringify([plugin]));
    localStorage.setItem("nexa-active-theme-v1", plugin.id);
    localStorage.setItem("nexa-e2e-recent-queries", JSON.stringify([{
      id: "recent-wallpaper-query",
      queryText: "wallpaper surface roles",
      resultCount: 3,
      searchTimeMs: 12,
      createdAt: "2026-08-19T00:00:00Z",
    }]));
    localStorage.setItem("nexa-e2e-skill-change-proposals", JSON.stringify([{
      id: "proposal-surface-review",
      action: "create",
      skillId: null,
      name: "Surface Review",
      description: "Dialog readability regression fixture",
      content: "# Surface Review",
      resourceBundle: [],
      rationale: "Keep overlays separate from wallpaper panels.",
      warnings: [],
      status: "pending",
      conversationId: null,
      source: "manual",
      confidence: 0.9,
      evidence: null,
      createdAt: "2026-08-19T00:00:00Z",
      updatedAt: "2026-08-19T00:00:00Z",
      appliedAt: null,
      rejectedAt: null,
    }]));
  });

  await page.goto("/settings");
  await expect(page.locator("html")).toHaveAttribute("data-custom-theme", "true");
  await expect(page.locator(".app-theme-backdrop")).toBeVisible();
  const settingsPage = page.getByTestId("settings-page");
  await expect(settingsPage).toBeVisible();

  const settingsContent = page.locator("main");
  const rail = page.getByTestId("app-navigation-rail");
  const titlebar = page.getByTestId("app-titlebar");
  const backdropBounds = await page.locator(".app-theme-backdrop").boundingBox();
  const frameBounds = await page.locator(".app-window-frame").boundingBox();
  expect(backdropBounds).not.toBeNull();
  expect(frameBounds).not.toBeNull();
  expect(backdropBounds!.x).toBeLessThanOrEqual(frameBounds!.x);
  expect(backdropBounds!.y).toBeLessThanOrEqual(frameBounds!.y);
  expect(backdropBounds!.x + backdropBounds!.width).toBeGreaterThanOrEqual(frameBounds!.x + frameBounds!.width);
  expect(backdropBounds!.y + backdropBounds!.height).toBeGreaterThanOrEqual(frameBounds!.y + frameBounds!.height);
  const surfaceAlphas = {
    settingsContent: await backgroundAlpha(settingsContent),
    settingsHeader: await backgroundAlpha(page.getByTestId("settings-page-header")),
    settingsPanel: await backgroundAlpha(
      page.getByRole("heading", { name: "Appearance", exact: true }).locator('xpath=ancestor::section[1]'),
    ),
    rail: await backgroundAlpha(rail),
    titlebar: await backgroundAlpha(titlebar),
    searchPage: 0,
    sourcesPage: 0,
    knowledgePage: 0,
    taskPage: 0,
    workflowPage: 0,
    chatSidebar: 0,
    chatContent: 0,
  };

  for (const tabName of [
    "Appearance",
    "Theme",
    "Models & Embedding",
    "AI Providers",
    "AI Usage",
    "Media Processing",
    "Data & Privacy",
    "Extensions",
  ]) {
    await page.getByRole("button", { name: tabName, exact: true }).click();
    await expect.poll(() => largeOpaqueSurfaceClasses(settingsPage)).toEqual([]);
    if (tabName === "Extensions") {
      const skillsSection = page.locator("section").filter({
        has: page.getByRole("heading", { name: "Skills", exact: true }),
      });
      await skillsSection.getByRole("button").first().click();
      await skillsSection.getByRole("button", { name: "Preview", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Surface Review" });
      await expect(dialog).toBeVisible();
      expect(await backgroundAlpha(dialog)).toBe(1);
      await dialog.getByRole("button", { name: "Close", exact: true }).click();
    }
  }
  await page.getByRole("button", { name: "Appearance", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Appearance", exact: true })).toBeVisible();
  const nestedAppearancePanel = settingsPage
    .locator('[data-theme-surface="panel"] [data-theme-surface="panel"]')
    .first();
  await expect(nestedAppearancePanel).toBeVisible();
  const nestedPanelBlurOwners = await nestedAppearancePanel.evaluate((target) => {
    const owners: string[] = [];
    let element: Element | null = target;
    while (element) {
      const style = getComputedStyle(element);
      const filter = style.backdropFilter
        || style.getPropertyValue("-webkit-backdrop-filter")
        || "none";
      if (filter !== "none") owners.push(filter);
      element = element.parentElement;
    }
    return owners;
  });
  expect(nestedPanelBlurOwners).toEqual(["blur(18px)"]);
  await page.screenshot({ path: 'test-results/custom-wallpaper-settings.png', fullPage: true });

  await page.goto("/");
  const routeSurface = page.locator("main[data-theme-surface]").first();
  await expect(routeSurface).toHaveAttribute("data-theme-surface", "content");
  await expect(routeSurface).toHaveAttribute("data-theme-unified-canvas", "true");
  surfaceAlphas.searchPage = await backgroundAlpha(routeSurface);
  await page.getByRole("textbox").first().focus();
  const recentQueryDropdown = page.getByTestId("recent-query-dropdown");
  await expect(recentQueryDropdown).toBeVisible();
  expect(await backgroundAlpha(recentQueryDropdown)).toBe(1);
  await page.keyboard.press("Escape");
  const searchEmptyState = page.locator('[data-theme-unified-canvas="true"] [data-theme-component="card"]').first();
  await expect(searchEmptyState).toBeVisible();
  expect(await backgroundAlpha(searchEmptyState)).toBe(0);
  await page.screenshot({ path: 'test-results/custom-wallpaper-search.png', fullPage: true });

  await page.goto("/sources");
  await expect(routeSurface).toHaveAttribute("data-theme-surface", "content");
  await expect(routeSurface).toHaveAttribute("data-theme-unified-canvas", "true");
  await expect(page.getByRole("heading", { name: "Source Management", exact: true })).toBeVisible();
  surfaceAlphas.sourcesPage = await backgroundAlpha(routeSurface);
  const sourcesEmptyState = page.locator('[data-theme-unified-canvas="true"] [data-theme-component="card"]').first();
  await expect(sourcesEmptyState).toBeVisible();
  expect(await backgroundAlpha(sourcesEmptyState)).toBe(0);
  await page.screenshot({ path: 'test-results/custom-wallpaper-sources.png', fullPage: true });

  await page.goto("/knowledge");
  await expect(routeSurface).toHaveAttribute("data-theme-surface", "content");
  await expect(routeSurface).toHaveAttribute("data-theme-unified-canvas", "true");
  await expect(page.getByRole("heading", { name: "Knowledge", exact: true })).toBeVisible();
  surfaceAlphas.knowledgePage = await backgroundAlpha(routeSurface);
  await page.screenshot({ path: 'test-results/custom-wallpaper-knowledge.png', fullPage: true });

  await page.goto("/tasks");
  await expect(routeSurface).toHaveAttribute("data-theme-surface", "page");
  surfaceAlphas.taskPage = await backgroundAlpha(routeSurface);
  const refreshTasks = page.getByRole("button", { name: "Refresh", exact: true });
  await expect(refreshTasks).toBeVisible();
  const refreshRestingBackground = await refreshTasks.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  );
  await refreshTasks.hover();
  await expect.poll(() => refreshTasks.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  ))
    .not.toBe(refreshRestingBackground);

  await page.goto("/workflows");
  await expect(routeSurface).toHaveAttribute("data-theme-surface", "page");
  const workflowSurface = page.locator("main .min-h-full.bg-surface-0").first();
  await expect(workflowSurface).toBeVisible();
  surfaceAlphas.workflowPage = await backgroundAlpha(workflowSurface);

  await page.goto("/chat");
  const chatSidebar = page.getByTestId("chat-history-sidebar").locator(":scope > div > div");
  const chatContent = page.getByTestId("chat-reading-surface");
  const chatWorkspace = page.getByTestId("chat-workspace-surface");
  await expect(chatSidebar).toBeVisible();
  await expect(page.getByTestId("chat-input")).toHaveAttribute("data-placement", "center");
  await expect(chatContent).toHaveCSS("backdrop-filter", "none");
  await expect(chatWorkspace).toHaveCSS("backdrop-filter", "blur(18px)");
  surfaceAlphas.chatSidebar = await backgroundAlpha(chatSidebar);
  surfaceAlphas.chatContent = await backgroundAlpha(chatWorkspace);

  expect(surfaceAlphas).toEqual({
    settingsContent: 0,
    settingsHeader: 0.72,
    settingsPanel: 0.72,
    rail: 0.72,
    titlebar: 0.72,
    searchPage: 0.82,
    sourcesPage: 0.82,
    knowledgePage: 0.82,
    taskPage: 0,
    workflowPage: 0,
    chatSidebar: 0.72,
    chatContent: 0.82,
  });
});

test("extensions exposes one user-owned .nexa home and reloads registered skill files", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "Extensions", exact: true }).click();

  const extensionHome = page.getByTestId("user-extension-home");
  await expect(extensionHome).toContainText("Nexa extension home");
  await expect(extensionHome).toContainText("C:\\Users\\Test\\.nexa");
  await extensionHome.getByRole("button", { name: "Open folder", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("e2e-opened-mcp-config")))
    .toBe("C:\\Users\\Test\\.nexa");
  await extensionHome.getByRole("button", { name: "Reload skill files", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("e2e-user-skill-files-reloaded")))
    .toBe("1");
});

test("extensions exposes a user-owned MCP JSON workflow", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("e2e-mcp-reload-race", "1"));
  await page.goto("/settings");
  await page.getByRole("button", { name: "Extensions", exact: true }).click();

  const mcpSection = page.locator("section").filter({
    has: page.getByRole("heading", { name: "MCP Connectors", exact: true }),
  });
  await mcpSection.getByRole("button").first().click();
  await expect(mcpSection.getByText(/\.nexa\\connectors\\mcp\.json/)).toBeVisible();
  await expect(mcpSection.getByRole("button", { name: "Open JSON", exact: true })).toBeVisible();
  await expect(mcpSection.getByRole("button", { name: "Reload JSON", exact: true })).toBeVisible();

  await expect.poll(() => page.evaluate(() => localStorage.getItem("e2e-mcp-list-tools-calls")))
    .toBe("1");

  await mcpSection.getByRole("button", { name: "Open JSON", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("e2e-opened-mcp-config")))
    .toContain(".nexa\\connectors\\mcp.json");
  await mcpSection.getByRole("button", { name: "Reload JSON", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("e2e-mcp-config-reloaded")))
    .toBe("1");
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => localStorage.getItem("e2e-mcp-list-tools-calls"))).toBe("1");
});

test("custom appearances without a visual background keep the ordinary opaque shell", async ({ page }) => {
  await page.addInitScript(() => {
    const plugin = {
      manifestVersion: 2,
      kind: "theme-resource",
      id: "palette-only",
      name: "Palette Only",
      theme: {
        baseTheme: "dark",
        mode: "dark",
        colors: { accent: "#22c55e" },
        effects: { surfaceOpacity: 0.35, glassBlur: 24 },
        typography: {},
        motion: {},
        brand: {},
        content: {},
        components: {
          header: { background: "rgba(20, 32, 45, 0.74)" },
          card: { background: "rgba(32, 48, 64, 0.66)" },
        },
        background: { kind: "none" },
      },
    };
    localStorage.setItem("nexa-theme-resource-plugins-v2", JSON.stringify([plugin]));
    localStorage.setItem("nexa-active-theme-v1", plugin.id);
  });

  await page.goto("/settings");
  await expect(page.locator("html")).toHaveAttribute("data-theme-backdrop", "false");
  await expect(page.locator(".app-theme-backdrop")).toBeHidden();
  expect(await backgroundAlpha(page.getByTestId("app-navigation-rail"))).toBe(1);
  expect(await backgroundAlpha(page.getByTestId("app-titlebar"))).toBe(0.74);
  const appearancePanel = page.getByRole("heading", { name: "Appearance", exact: true })
    .locator("xpath=ancestor::section[1]");
  expect(await backgroundAlpha(appearancePanel)).toBe(0.66);
  await expect(appearancePanel).toHaveCSS("backdrop-filter", "none");
});
