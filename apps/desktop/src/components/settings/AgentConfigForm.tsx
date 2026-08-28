import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { NexaSelect } from "../ui/overlay";
import {
  Eye,
  EyeOff,
  Loader2,
  Zap,
  Save,
  X,
  CheckCircle,
  BrainCircuit,
  RefreshCw,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { useTranslation, type TranslationKey } from "../../i18n";
import * as api from "../../lib/api";
import type {
  AgentConfig,
  SaveAgentConfigInput,
  ProviderType,
} from "../../types/conversation";
import type { Skill } from "../../types/extensions";
import {
  findProviderPreset,
  getReasoningCapability,
  type ReasoningEffortLevel,
  type ProviderPreset,
} from "../../lib/providerPresets";
import {
  bindProviderModelCatalogCredential,
  catalogModelsForSnapshot,
  catalogMatchesProvider,
  isProviderModelCatalogStale,
  loadProviderModelCatalog,
  saveProviderModelCatalog,
  type ProviderModelCatalogSnapshot,
} from "../../lib/providerModelCatalog";
import {
  buildMcpSubagentToolDescriptors,
  canonicalSubagentToolName,
  DEFAULT_SUBAGENT_TOOL_NAMES,
  getSubagentToolGroup,
  mergeSubagentToolCatalog,
  SUBAGENT_TOOL_GROUPS,
  usesDefaultSubagentToolSelection,
} from "../../lib/subagentTools";
import { CollapsiblePanel } from "./SettingsSection";
import {
  endpointIdForSavedSelection,
  selectImplicitDefault,
} from "../../lib/modelCatalog";
import { ModelDescriptorBadges } from "./ModelDescriptorBadges";
import {
  defaultReasoningEffort,
  defaultThinkingBudget,
  normalizeReasoningEffort,
  normalizeThinkingBudget,
} from "../../lib/reasoningControls";
import { CatalogModelPicker } from "./CatalogModelPicker";

interface AgentConfigFormProps {
  config?: AgentConfig;
  preset?: ProviderPreset | null;
  onSave: (input: SaveAgentConfigInput) => Promise<void>;
  onCancel: () => void;
  isSaving: boolean;
  onDirtyChange?: (dirty: boolean) => void;
}

const PROVIDER_LABEL_KEYS: { value: ProviderType; labelKey: string }[] = [
  { value: "open_ai", labelKey: "settings.providerOpenAI" },
  { value: "openrouter", labelKey: "settings.providerOpenRouter" },
  { value: "anthropic", labelKey: "settings.providerAnthropic" },
  { value: "google", labelKey: "settings.providerGoogleGemini" },
  { value: "deep_seek", labelKey: "settings.providerDeepSeek" },
  { value: "zhipu", labelKey: "settings.providerZhipu" },
  { value: "moonshot", labelKey: "settings.providerMoonshot" },
  { value: "qwen", labelKey: "settings.providerQwen" },
  { value: "alibaba_model_studio", labelKey: "settings.providerAlibabaModelStudio" },
  { value: "siliconflow", labelKey: "settings.providerSiliconFlow" },
  { value: "doubao", labelKey: "settings.providerDoubao" },
  { value: "yi", labelKey: "settings.providerYi" },
  { value: "baichuan", labelKey: "settings.providerBaichuan" },
  { value: "ollama", labelKey: "settings.providerOllama" },
  { value: "lm_studio", labelKey: "settings.providerLMStudio" },
  { value: "azure_open_ai", labelKey: "settings.providerAzureOpenAI" },
  { value: "custom", labelKey: "settings.providerCustom" },
];

const BASE_URL_PLACEHOLDERS: Record<ProviderType, string> = {
  open_ai: "https://api.openai.com/v1",
  openrouter: "https://openrouter.ai/api/v1",
  anthropic: "https://api.anthropic.com/v1",
  google: "https://generativelanguage.googleapis.com/v1beta",
  deep_seek: "https://api.deepseek.com",
  zhipu: "https://open.bigmodel.cn/api/paas/v4",
  moonshot: "https://api.moonshot.cn/v1",
  qwen: "https://dashscope.aliyuncs.com/compatible-mode/v1",
  alibaba_model_studio: "https://dashscope.aliyuncs.com/compatible-mode/v1",
  siliconflow: "https://api.siliconflow.cn/v1",
  doubao: "https://ark.cn-beijing.volces.com/api/v3",
  yi: "https://api.lingyiwanwu.com/v1",
  baichuan: "https://api.baichuan-ai.com/v1",
  ollama: "http://localhost:11434",
  lm_studio: "http://localhost:1234/v1",
  azure_open_ai: "https://{resource}.openai.azure.com",
  custom: "https://...",
};

const LOCAL_PROVIDERS: ProviderType[] = ["ollama", "lm_studio"];

const REASONING_EFFORT_LABEL_KEYS: Record<
  ReasoningEffortLevel,
  TranslationKey
> = {
  none: "settings.reasoningNone",
  minimal: "settings.reasoningMinimal",
  low: "settings.reasoningLow",
  medium: "settings.reasoningMedium",
  high: "settings.reasoningHigh",
  max: "settings.reasoningMax",
  xhigh: "settings.reasoningXHigh",
};

function normalizeBaseUrl(value: string | null | undefined): string {
  return (value ?? "").trim().replace(/\/+$/, "");
}

export function AgentConfigForm({
  config,
  preset,
  onSave,
  onCancel,
  isSaving,
  onDirtyChange,
}: AgentConfigFormProps) {
  const { t } = useTranslation();

  const initialProvider =
    (config?.provider as ProviderType) ??
    (preset?.provider as ProviderType) ??
    "open_ai";
  const initialBaseUrl = normalizeBaseUrl(
    config?.baseUrl ?? preset?.baseUrl ?? "",
  );
  const initialPreset =
    preset ??
    findProviderPreset({ provider: initialProvider, baseUrl: initialBaseUrl });
  const presetDefaultModel =
    selectImplicitDefault(initialPreset?.models ?? [])?.id ||
    "";
  const initialIsLocal =
    LOCAL_PROVIDERS.includes(initialProvider) ||
    (initialPreset ? !initialPreset.requiresApiKey : false);
  const initialModel = config?.model ?? presetDefaultModel;
  const initialPresetModel = initialPreset?.models.find((candidate) => candidate.id === initialModel);
  const initialUsesCustomModel =
    !!config &&
    !!initialPreset &&
    !initialPreset.models.some((m) => m.id === initialModel);
  const previousProviderRef = useRef<ProviderType>(initialProvider);

  const [name, setName] = useState(config?.name ?? preset?.name ?? "");
  const [provider, setProvider] = useState<ProviderType>(initialProvider);
  const [apiKey, setApiKey] = useState(config?.apiKey ?? "");
  const [baseUrl, setBaseUrl] = useState(initialBaseUrl);
  const [model, setModel] = useState(initialModel);
  const [temperature, setTemperature] = useState(config?.temperature ?? 0.3);
  const [maxTokens, setMaxTokens] = useState<number | null>(
    config?.maxTokens ?? null,
  );
  const [contextWindow, setContextWindow] = useState<number | null>(
    config?.contextWindow ?? null,
  );
  const [streamIdleTimeoutMs, setStreamIdleTimeoutMs] = useState<number | null>(
    config?.providerStreaming?.streamIdleTimeoutMs
      ?? initialPreset?.streaming?.streamIdleTimeoutMs
      ?? null,
  );
  const [connectTimeoutMs, setConnectTimeoutMs] = useState<number | null>(
    config?.providerStreaming?.connectTimeoutMs
      ?? initialPreset?.streaming?.connectTimeoutMs
      ?? null,
  );
  const [streamMaxRetries, setStreamMaxRetries] = useState<number | null>(
    config?.providerStreaming?.streamMaxRetries
      ?? initialPreset?.streaming?.streamMaxRetries
      ?? null,
  );
  const [isDefault, setIsDefault] = useState(config?.isDefault ?? false);
  const [reasoningEnabled, setReasoningEnabled] = useState<boolean | null>(
    config?.reasoningEnabled ?? null,
  );
  const [thinkingBudget, setThinkingBudget] = useState<number | null>(
    config?.thinkingBudget ?? null,
  );
  const [reasoningEffort, setReasoningEffort] = useState<string | null>(
    config?.reasoningEffort ?? null,
  );
  const [maxIterations, setMaxIterations] = useState<number | null>(
    config?.maxIterations ?? null,
  );
  const [summarizationModel, setSummarizationModel] = useState<string | null>(
    config?.summarizationModel ?? null,
  );
  const [summarizationProvider, setSummarizationProvider] = useState<
    string | null
  >(config?.summarizationProvider ?? null);
  const [subagentAllowedTools, setSubagentAllowedTools] = useState<string[]>(
    (config?.subagentAllowedTools ?? DEFAULT_SUBAGENT_TOOL_NAMES).map(
      canonicalSubagentToolName,
    ),
  );
  const [subagentAllowedSkillIds, setSubagentAllowedSkillIds] = useState<
    string[]
  >(config?.subagentAllowedSkillIds ?? []);
  const [subagentMaxParallel, setSubagentMaxParallel] = useState<number | null>(
    config?.delegationLimitsV2?.maxParallel ?? config?.subagentMaxParallel ?? 3,
  );
  const [subagentMaxCallsPerTurn, setSubagentMaxCallsPerTurn] = useState<
    number | null
  >(
    config?.delegationLimitsV2?.maxCallsPerTurn
      ?? config?.subagentMaxCallsPerTurn
      ?? 6,
  );
  const [subagentTokenBudget, setSubagentTokenBudget] = useState<number | null>(
    config?.delegationLimitsV2?.totalActualTokensSoftLimit
      ?? config?.subagentTokenBudget
      ?? 32000,
  );
  const [subagentInputContextLimit, setSubagentInputContextLimit] = useState<number | null>(
    config?.delegationLimitsV2?.inputContextLimit ?? null,
  );
  const [subagentHandoffContextTokens, setSubagentHandoffContextTokens] = useState<number | null>(
    config?.delegationLimitsV2?.handoffContextTokensPerWorker ?? null,
  );
  const [subagentMaxOutputTokens, setSubagentMaxOutputTokens] = useState<number | null>(
    config?.delegationLimitsV2?.maxOutputTokensPerStep
      ?? config?.delegationLimitsV2?.maxOutputTokensPerWorker
      ?? null,
  );
  const [subagentMaxActualTokens, setSubagentMaxActualTokens] = useState<number | null>(
    config?.delegationLimitsV2?.maxActualTokensPerWorker ?? null,
  );
  const [subagentCostLimitMicros, setSubagentCostLimitMicros] = useState<number | null>(
    config?.delegationLimitsV2?.totalCostSoftLimitMicros ?? null,
  );
  const [subagentQueueDeadlineMs, setSubagentQueueDeadlineMs] = useState<number | null>(
    config?.delegationLimitsV2?.queueDeadlineMs ?? null,
  );
  const [subagentConnectDeadlineMs, setSubagentConnectDeadlineMs] = useState<number | null>(
    config?.delegationLimitsV2?.connectDeadlineMs ?? null,
  );
  const [subagentFirstTokenDeadlineMs, setSubagentFirstTokenDeadlineMs] = useState<number | null>(
    config?.delegationLimitsV2?.firstTokenDeadlineMs ?? null,
  );
  const [subagentRunDeadlineMs, setSubagentRunDeadlineMs] = useState<number | null>(
    config?.delegationLimitsV2?.runDeadlineMs ?? null,
  );
  const [enabledSkills, setEnabledSkills] = useState<Skill[]>([]);
  const [mcpToolDescriptors, setMcpToolDescriptors] = useState<
    ReturnType<typeof buildMcpSubagentToolDescriptors>
  >([]);
  const [showKey, setShowKey] = useState(false);
  const [testLoading, setTestLoading] = useState(false);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);
  const [useCustomModel, setUseCustomModel] = useState(initialUsesCustomModel);
  const [modelCatalog, setModelCatalog] = useState<ProviderModelCatalogSnapshot | null>(() =>
    loadProviderModelCatalog(initialProvider, initialBaseUrl, initialIsLocal ? "" : (config?.apiKey ?? "")),
  );
  const [showAdvanced, setShowAdvanced] = useState(!!config);
  const initialDraftRef = useRef<SaveAgentConfigInput>({
    id: config?.id ?? null,
    name: config?.name ?? preset?.name ?? "",
    provider: initialProvider,
    apiKey: initialIsLocal ? "" : (config?.apiKey ?? ""),
    baseUrl: initialBaseUrl || null,
    model: initialModel,
    providerEndpointId: endpointIdForSavedSelection({
      descriptor: initialPresetModel?.descriptor,
      catalogBaseUrl: initialPreset?.baseUrl,
      configuredBaseUrl: initialBaseUrl,
      persistedEndpointId: config?.providerEndpointId,
      persistedProvider: config?.provider,
      persistedBaseUrl: config?.baseUrl,
      currentProvider: initialProvider,
    }),
    modelId: initialPresetModel?.descriptor.id ?? config?.modelId ?? initialModel,
    temperature: config?.temperature ?? 0.3,
    maxTokens: config?.maxTokens ?? null,
    contextWindow: config?.contextWindow ?? null,
    providerStreaming: {
      streamIdleTimeoutMs:
        config?.providerStreaming?.streamIdleTimeoutMs
        ?? initialPreset?.streaming?.streamIdleTimeoutMs
        ?? null,
      connectTimeoutMs:
        config?.providerStreaming?.connectTimeoutMs
        ?? initialPreset?.streaming?.connectTimeoutMs
        ?? null,
      streamMaxRetries:
        config?.providerStreaming?.streamMaxRetries
        ?? initialPreset?.streaming?.streamMaxRetries
        ?? null,
    },
    isDefault: config?.isDefault ?? false,
    reasoningEnabled: config?.reasoningEnabled ?? null,
    thinkingBudget: config?.thinkingBudget ?? null,
    reasoningEffort: config?.reasoningEffort ?? null,
    maxIterations: config?.maxIterations ?? null,
    summarizationModel: config?.summarizationModel ?? null,
    summarizationProvider: config?.summarizationProvider ?? null,
    imageGenerationModel: null,
    subagentAllowedTools: usesDefaultSubagentToolSelection(
      config?.subagentAllowedTools,
    )
      ? null
      : (config?.subagentAllowedTools?.map(canonicalSubagentToolName) ?? null),
    subagentAllowedSkillIds: config?.subagentAllowedSkillIds ?? null,
    subagentMaxParallel:
      config?.delegationLimitsV2?.maxParallel ?? config?.subagentMaxParallel ?? 3,
    subagentMaxCallsPerTurn:
      config?.delegationLimitsV2?.maxCallsPerTurn
      ?? config?.subagentMaxCallsPerTurn
      ?? 6,
    subagentTokenBudget:
      config?.delegationLimitsV2?.totalActualTokensSoftLimit
      ?? config?.subagentTokenBudget
      ?? 32000,
    delegationLimitsV2: {
      inputContextLimit: config?.delegationLimitsV2?.inputContextLimit ?? null,
      handoffContextTokensPerWorker:
        config?.delegationLimitsV2?.handoffContextTokensPerWorker ?? null,
      maxOutputTokensPerStep:
        config?.delegationLimitsV2?.maxOutputTokensPerStep
        ?? config?.delegationLimitsV2?.maxOutputTokensPerWorker
        ?? null,
      maxOutputTokensPerWorker:
        config?.delegationLimitsV2?.maxOutputTokensPerWorker ?? null,
      maxActualTokensPerWorker:
        config?.delegationLimitsV2?.maxActualTokensPerWorker ?? null,
      totalActualTokensSoftLimit:
        config?.delegationLimitsV2?.totalActualTokensSoftLimit
        ?? config?.subagentTokenBudget
        ?? 32000,
      totalCostSoftLimitMicros:
        config?.delegationLimitsV2?.totalCostSoftLimitMicros ?? null,
      maxParallel:
        config?.delegationLimitsV2?.maxParallel ?? config?.subagentMaxParallel ?? 3,
      maxCallsPerTurn:
        config?.delegationLimitsV2?.maxCallsPerTurn
        ?? config?.subagentMaxCallsPerTurn
        ?? 6,
      queueDeadlineMs: config?.delegationLimitsV2?.queueDeadlineMs ?? null,
      connectDeadlineMs: config?.delegationLimitsV2?.connectDeadlineMs ?? null,
      firstTokenDeadlineMs:
        config?.delegationLimitsV2?.firstTokenDeadlineMs ?? null,
      runDeadlineMs: config?.delegationLimitsV2?.runDeadlineMs ?? null,
    },
  });

  const isLocal =
    LOCAL_PROVIDERS.includes(provider) ||
    (preset ? !preset.requiresApiKey : false);
  const curatedPreset =
    findProviderPreset({ provider, baseUrl }) ??
    (!baseUrl.trim() && preset?.provider === provider ? preset : null);
  const matchingModelCatalog = modelCatalog
    && catalogMatchesProvider(modelCatalog, provider, baseUrl, isLocal ? "" : apiKey)
    ? modelCatalog
    : null;
  const activePreset = useMemo(() => {
    if (!matchingModelCatalog) return curatedPreset;
    if (curatedPreset) {
      return { ...curatedPreset, models: catalogModelsForSnapshot(matchingModelCatalog) };
    }
    return {
      id: `discovered-${provider}`,
      name: name || provider,
      provider,
      baseUrl,
      models: catalogModelsForSnapshot(matchingModelCatalog),
      requiresApiKey: !LOCAL_PROVIDERS.includes(provider),
      icon: "",
      description: "",
    } satisfies ProviderPreset;
  }, [baseUrl, curatedPreset, matchingModelCatalog, name, provider]);
  const activePresetDefaultModel = selectImplicitDefault(activePreset?.models ?? [])?.id ?? "";
  const selectedPresetModel =
    activePreset?.models.find((candidate) => candidate.id === model) ?? null;
  const reasoningCapability = useMemo(
    () => getReasoningCapability({ provider, baseUrl, model }),
    [provider, baseUrl, model],
  );
  const reasoningEffortOptions = reasoningCapability?.effortLevels ?? [];
  const thinkingBudgetCapability = reasoningCapability?.thinkingBudget;
  const supportsReasoning = reasoningCapability !== null;
  const reasoningAlwaysOn = reasoningCapability?.mode === "always";
  const reasoningControlsExclusive = reasoningCapability?.effortBudgetExclusive === true;
  const supportsThinkingBudget = thinkingBudgetCapability?.enabled === true;
  const supportsReasoningEffort = reasoningEffortOptions.length > 0;
  const subagentToolCatalog = useMemo(
    () => mergeSubagentToolCatalog(mcpToolDescriptors),
    [mcpToolDescriptors],
  );
  const subagentToolsByGroup = useMemo(
    () =>
      SUBAGENT_TOOL_GROUPS.map((group) => ({
        ...group,
        tools: subagentToolCatalog.filter(
          (tool) => getSubagentToolGroup(tool) === group.id,
        ),
      })).filter((group) => group.tools.length > 0),
    [subagentToolCatalog],
  );
  const availableSkillIds = useMemo(
    () => enabledSkills.map((skill) => skill.id),
    [enabledSkills],
  );
  const visibleSelectedToolCount = useMemo(
    () =>
      subagentAllowedTools.filter((name) =>
        subagentToolCatalog.some((tool) => tool.name === name),
      ).length,
    [subagentAllowedTools, subagentToolCatalog],
  );
  const usesAllEnabledSkills = useMemo(() => {
    if (availableSkillIds.length === 0) {
      return subagentAllowedSkillIds.length === 0;
    }
    if (subagentAllowedSkillIds.length !== availableSkillIds.length) {
      return false;
    }
    const selected = new Set(subagentAllowedSkillIds);
    return availableSkillIds.every((id) => selected.has(id));
  }, [availableSkillIds, subagentAllowedSkillIds]);

  const orderToolSelection = useCallback(
    (selection: string[]) => {
      const selected = new Set(selection);
      const ordered = subagentToolCatalog
        .filter((tool) => selected.has(tool.name))
        .map((tool) => tool.name);
      const extras = selection.filter(
        (name) => !subagentToolCatalog.some((tool) => tool.name === name),
      );
      return [...ordered, ...extras];
    },
    [subagentToolCatalog],
  );

  const orderSkillSelection = useCallback(
    (selection: string[]) => {
      const selected = new Set(selection);
      const ordered = enabledSkills
        .filter((skill) => selected.has(skill.id))
        .map((skill) => skill.id);
      const extras = selection.filter(
        (id) => !enabledSkills.some((skill) => skill.id === id),
      );
      return [...ordered, ...extras];
    },
    [enabledSkills],
  );

  const setRecommendedSubagentTools = useCallback(() => {
    setSubagentAllowedTools(orderToolSelection(DEFAULT_SUBAGENT_TOOL_NAMES));
  }, [orderToolSelection]);

  const setAllSubagentTools = useCallback(() => {
    setSubagentAllowedTools(orderToolSelection(subagentToolCatalog.map((tool) => tool.name)));
  }, [orderToolSelection, subagentToolCatalog]);

  const clearSubagentTools = useCallback(() => {
    setSubagentAllowedTools([]);
  }, []);

  const setSubagentToolGroupSelection = useCallback(
    (toolNames: string[], enabled: boolean) => {
      setSubagentAllowedTools((prev) => {
        const next = new Set(prev);
        for (const name of toolNames) {
          if (enabled) {
            next.add(name);
          } else {
            next.delete(name);
          }
        }
        return orderToolSelection(Array.from(next));
      });
    },
    [orderToolSelection],
  );

  // Reset test result when provider changes
  useEffect(() => {
    setTestResult(null);
  }, [provider]);

  useEffect(() => {
    setModelCatalog(loadProviderModelCatalog(provider, baseUrl, isLocal ? "" : apiKey));
  }, [apiKey, baseUrl, isLocal, provider]);

  useEffect(() => {
    const previousProvider = previousProviderRef.current;
    if (previousProvider === provider) {
      return;
    }

    const normalizedCurrentBaseUrl = normalizeBaseUrl(baseUrl);
    const previousPreset = findProviderPreset({
      provider: previousProvider,
      baseUrl: normalizedCurrentBaseUrl,
    });
    const previousPresetBaseUrl = normalizeBaseUrl(
      previousPreset?.baseUrl ??
        (preset?.provider === previousProvider ? preset.baseUrl : ""),
    );
    const previousPlaceholder = normalizeBaseUrl(
      BASE_URL_PLACEHOLDERS[previousProvider],
    );
    const shouldReplaceBaseUrl =
      !normalizedCurrentBaseUrl ||
      normalizedCurrentBaseUrl === previousPlaceholder ||
      (!!previousPresetBaseUrl &&
        normalizedCurrentBaseUrl === previousPresetBaseUrl);

    if (shouldReplaceBaseUrl) {
      const nextPreset =
        findProviderPreset({ provider, baseUrl: null }) ??
        (preset?.provider === provider ? preset : null);
      setBaseUrl(
        normalizeBaseUrl(
          nextPreset?.baseUrl ?? BASE_URL_PLACEHOLDERS[provider],
        ),
      );
    }

    if (!useCustomModel) {
      const nextPreset =
        findProviderPreset({ provider, baseUrl: null }) ??
        (preset?.provider === provider ? preset : null);
      const nextModel = selectImplicitDefault(nextPreset?.models ?? [])?.id ?? "";
      if (nextModel) {
        setModel(nextModel);
      }
    }

    setContextWindow(null);
    previousProviderRef.current = provider;
  }, [baseUrl, provider, preset, useCustomModel]);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const [servers, skills] = await Promise.all([
          api.listMcpServers(),
          api.listActiveSkills(),
        ]);

        const enabledServerTools = await Promise.all(
          servers
            .filter((server) => server.enabled)
            .map(async (server) => {
              try {
                const tools = await api.listMcpTools(server.id);
                return tools.map((tool) => ({
                  name: tool.name,
                  description: tool.description,
                  serverName: server.name,
                }));
              } catch {
                return [];
              }
            }),
        );

        if (cancelled) return;
        setMcpToolDescriptors(
          buildMcpSubagentToolDescriptors(enabledServerTools.flat()),
        );
        setEnabledSkills(skills);
      } catch {
        if (cancelled) return;
        setMcpToolDescriptors([]);
        setEnabledSkills([]);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (config?.subagentAllowedSkillIds != null) return;
    setSubagentAllowedSkillIds(orderSkillSelection(availableSkillIds));
  }, [availableSkillIds, config?.subagentAllowedSkillIds, orderSkillSelection]);

  useEffect(() => {
    if (useCustomModel || !activePreset || !activePresetDefaultModel) {
      return;
    }
    if (!activePreset.models.some((m) => m.id === model)) {
      setModel(activePresetDefaultModel);
      setContextWindow(null);
    }
  }, [activePreset, activePresetDefaultModel, model, useCustomModel]);

  useEffect(() => {
    if (!supportsReasoning) {
      if (reasoningEnabled !== null) {
        setReasoningEnabled(null);
      }
      if (thinkingBudget !== null) {
        setThinkingBudget(null);
      }
      if (reasoningEffort !== null) {
        setReasoningEffort(null);
      }
      return;
    }

    if (reasoningAlwaysOn && reasoningEnabled !== true) {
      setReasoningEnabled(true);
      return;
    }

    if (reasoningEnabled !== true) {
      if (thinkingBudget !== null) {
        setThinkingBudget(null);
      }
      if (reasoningEffort !== null) {
        setReasoningEffort(null);
      }
      return;
    }

    const preferEffort = reasoningControlsExclusive &&
      (reasoningEffort !== null || thinkingBudget === null);
    const nextThinkingBudget = preferEffort
      ? null
      : normalizeThinkingBudget(thinkingBudget, reasoningCapability);
    if (thinkingBudget !== nextThinkingBudget) {
      setThinkingBudget(nextThinkingBudget);
    }

    const nextReasoningEffort = reasoningControlsExclusive && !preferEffort
      ? null
      : normalizeReasoningEffort(reasoningEffort, reasoningCapability);
    if (reasoningEffort !== nextReasoningEffort) {
      setReasoningEffort(nextReasoningEffort);
    }
  }, [
    reasoningCapability,
    reasoningAlwaysOn,
    reasoningControlsExclusive,
    reasoningEffort,
    reasoningEnabled,
    supportsReasoning,
    thinkingBudget,
  ]);

  const buildInput = useCallback(
    (): SaveAgentConfigInput => {
      const normalizedReasoningEnabled =
        supportsReasoning
          ? reasoningAlwaysOn
            ? true
            : reasoningEnabled
          : null;
      const preferEffort = reasoningControlsExclusive &&
        (reasoningEffort !== null || thinkingBudget === null);
      const normalizedThinkingBudget =
        normalizedReasoningEnabled && supportsThinkingBudget && !preferEffort
          ? normalizeThinkingBudget(thinkingBudget, reasoningCapability)
          : null;
      const normalizedReasoningEffort =
        normalizedReasoningEnabled && supportsReasoningEffort &&
          (!reasoningControlsExclusive || preferEffort)
          ? normalizeReasoningEffort(reasoningEffort, reasoningCapability)
          : null;

      return {
        id: config?.id ?? null,
        name: name.trim(),
        provider,
        apiKey: isLocal ? "" : apiKey,
        baseUrl: normalizeBaseUrl(baseUrl) || null,
        model: model.trim(),
        providerEndpointId: endpointIdForSavedSelection({
          descriptor: selectedPresetModel?.descriptor ?? activePreset?.models[0]?.descriptor,
          catalogBaseUrl: activePreset?.baseUrl,
          configuredBaseUrl: baseUrl,
          persistedEndpointId: config?.providerEndpointId,
          persistedProvider: config?.provider,
          persistedBaseUrl: config?.baseUrl,
          currentProvider: provider,
        }),
        modelId: selectedPresetModel?.descriptor.id ?? model.trim(),
        temperature,
        maxTokens,
        contextWindow: contextWindow,
        providerStreaming: {
          streamIdleTimeoutMs,
          connectTimeoutMs,
          streamMaxRetries,
        },
        isDefault,
        reasoningEnabled: normalizedReasoningEnabled,
        thinkingBudget: normalizedThinkingBudget,
        reasoningEffort: normalizedReasoningEffort,
        maxIterations,
        summarizationModel: summarizationModel?.trim() || null,
        summarizationProvider: summarizationProvider || null,
        imageGenerationModel: null,
        subagentAllowedTools: usesDefaultSubagentToolSelection(
          subagentAllowedTools,
        )
          ? null
          : orderToolSelection(subagentAllowedTools),
        subagentAllowedSkillIds: usesAllEnabledSkills
          ? null
          : orderSkillSelection(subagentAllowedSkillIds),
        subagentMaxParallel,
        subagentMaxCallsPerTurn,
        subagentTokenBudget,
        delegationLimitsV2: {
          inputContextLimit: subagentInputContextLimit,
          handoffContextTokensPerWorker: subagentHandoffContextTokens,
          maxOutputTokensPerStep: subagentMaxOutputTokens,
          maxOutputTokensPerWorker: null,
          maxActualTokensPerWorker: subagentMaxActualTokens,
          totalActualTokensSoftLimit: subagentTokenBudget,
          totalCostSoftLimitMicros: subagentCostLimitMicros,
          maxParallel: subagentMaxParallel,
          maxCallsPerTurn: subagentMaxCallsPerTurn,
          queueDeadlineMs: subagentQueueDeadlineMs,
          connectDeadlineMs: subagentConnectDeadlineMs,
          firstTokenDeadlineMs: subagentFirstTokenDeadlineMs,
          runDeadlineMs: subagentRunDeadlineMs,
        },
      };
    },
    [
      config?.id,
      name,
      provider,
      apiKey,
      baseUrl,
      model,
      selectedPresetModel,
      activePreset,
      temperature,
      maxTokens,
      contextWindow,
      streamIdleTimeoutMs,
      connectTimeoutMs,
      streamMaxRetries,
      isDefault,
      reasoningEnabled,
      reasoningAlwaysOn,
      reasoningControlsExclusive,
      thinkingBudget,
      reasoningEffort,
      reasoningCapability,
      supportsReasoning,
      supportsReasoningEffort,
      supportsThinkingBudget,
      maxIterations,
      summarizationModel,
      summarizationProvider,
      subagentAllowedTools,
      subagentAllowedSkillIds,
      subagentMaxParallel,
      subagentMaxCallsPerTurn,
      subagentTokenBudget,
      subagentInputContextLimit,
      subagentHandoffContextTokens,
      subagentMaxOutputTokens,
      subagentMaxActualTokens,
      subagentCostLimitMicros,
      subagentQueueDeadlineMs,
      subagentConnectDeadlineMs,
      subagentFirstTokenDeadlineMs,
      subagentRunDeadlineMs,
      isLocal,
      orderToolSelection,
      orderSkillSelection,
      usesAllEnabledSkills,
    ],
  );

  useEffect(() => {
    if (!onDirtyChange) return;

    const dirty =
      JSON.stringify(buildInput()) !== JSON.stringify(initialDraftRef.current);
    onDirtyChange(dirty);
  }, [buildInput, onDirtyChange]);

  useEffect(() => {
    if (!onDirtyChange) return;

    return () => {
      onDirtyChange(false);
    };
  }, [onDirtyChange]);

  const handleTest = async () => {
    setTestLoading(true);
    setTestResult(null);
    try {
      const input = buildInput();
      const catalog = bindProviderModelCatalogCredential(
        await api.testAgentConnection(input),
        input.apiKey,
      );
      saveProviderModelCatalog(catalog, input.apiKey);
      setModelCatalog(catalog);
      setTestResult({
        ok: true,
        message:
          catalog.models.length > 0
            ? t("settings.modelsFound").replace(
                "{count}",
                String(catalog.models.length),
              )
            : t("settings.connectionSuccess"),
      });
      toast.success(t("settings.connectionSuccess"));
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : t("settings.connectionFailed");
      setTestResult({ ok: false, message: msg });
      toast.error(t("settings.connectionFailed"));
    } finally {
      setTestLoading(false);
    }
  };

  const handleCatalogRefresh = async () => {
    setCatalogLoading(true);
    try {
      const input = buildInput();
      const catalog = bindProviderModelCatalogCredential(
        await api.refreshProviderModelCatalog(input),
        input.apiKey,
      );
      saveProviderModelCatalog(catalog, input.apiKey);
      setModelCatalog(catalog);
      toast.success(
        catalog.liveDiscoverySucceeded
          ? t("settings.modelCatalogRefreshed", { count: catalog.models.length })
          : t("settings.modelCatalogFallback"),
      );
    } catch {
      toast.error(t("settings.modelCatalogRefreshFailed"));
    } finally {
      setCatalogLoading(false);
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onSave(buildInput());
  };

  const canSubmit = name.trim() && model.trim() && (isLocal || apiKey.trim());

  return (
    <form onSubmit={handleSubmit} className="space-y-5">
      {/* Name */}
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">
          {t("settings.providerName")}
        </label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("settings.providerNamePlaceholder")}
        />
      </div>

      {/* Provider Type */}
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">
          {t("settings.providerType")}
        </label>
        <NexaSelect
          value={provider}
          onChange={(e) => {
            const nextProvider = e.target.value as ProviderType;
            const nextPreset = findProviderPreset({
              provider: nextProvider,
              baseUrl: null,
            });
            setProvider(nextProvider);
            setContextWindow(null);
            setStreamIdleTimeoutMs(nextPreset?.streaming?.streamIdleTimeoutMs ?? null);
            setConnectTimeoutMs(nextPreset?.streaming?.connectTimeoutMs ?? null);
            setStreamMaxRetries(nextPreset?.streaming?.streamMaxRetries ?? null);
          }}
          className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
        >
          {PROVIDER_LABEL_KEYS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {t(opt.labelKey as any)}
            </option>
          ))}
        </NexaSelect>
      </div>

      {/* API Key — hidden for local providers */}
      {!isLocal && (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">
            {t("settings.apiKey")}
          </label>
          <div className="relative">
            <Input
              type={showKey ? "text" : "password"}
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..."
              className="pr-10"
            />
            <button
              type="button"
              onClick={() => setShowKey(!showKey)}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer"
              aria-label={
                showKey ? t("settings.hideKey") : t("settings.showKey")
              }
            >
              {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
            </button>
          </div>
        </div>
      )}

      {/* Base URL */}
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">
          {t("settings.baseUrl")}
        </label>
        <Input
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder={BASE_URL_PLACEHOLDERS[provider]}
        />
      </div>

      {/* Model */}
      {activePreset && activePreset.models.length > 0 && !useCustomModel ? (
        <div className="space-y-2" data-testid="default-model-field">
          <div className="flex items-center justify-between gap-3">
            <label className="text-sm font-medium text-text-primary">
              {t("settings.defaultModel")}
            </label>
            <span className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleCatalogRefresh()}
                disabled={catalogLoading || (!isLocal && !apiKey.trim())}
                className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-text-secondary transition-colors hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <RefreshCw size={12} className={catalogLoading ? "animate-spin" : ""} />
                {t("settings.refreshModelCatalog")}
              </button>
            </span>
          </div>
          <CatalogModelPicker
            value={model}
            onValueChange={(nextModel) => {
              setModel(nextModel);
              setContextWindow(null);
            }}
            models={activePreset.models.map((candidate) => ({
              ...candidate,
              secondary: candidate.tagKey ? t(candidate.tagKey as TranslationKey) : null,
            }))}
            surface="text"
            dataTestId="default-model-picker"
          />
          <ModelDescriptorBadges descriptor={selectedPresetModel?.descriptor} surface="text" />
          {config?.modelSelectionResolution?.requiresUserNotice && (
            <p
              className="rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-xs text-warning"
              data-testid="model-selection-resolution-notice"
              role="status"
            >
              Saved model selection resolved to {config.modelSelectionResolution.modelId}
              {` (${config.modelSelectionResolution.kind})`}.
            </p>
          )}
          <button
            type="button"
            onClick={() => setUseCustomModel(true)}
            className="text-xs text-text-tertiary hover:text-accent transition-colors cursor-pointer"
          >
            {t("settings.useCustomModel")}
          </button>
          {matchingModelCatalog && (
            <div className="flex flex-wrap items-center gap-2 text-xs text-text-tertiary" data-testid="provider-model-catalog-status">
              <span>
                {matchingModelCatalog.liveDiscoverySucceeded
                  ? t("settings.modelCatalogLive")
                  : t("settings.modelCatalogCurated")}
              </span>
              <span aria-hidden="true">·</span>
              <span>{new Date(matchingModelCatalog.refreshedAt).toLocaleString()}</span>
              {isProviderModelCatalogStale(matchingModelCatalog) && (
                <span className="rounded-full bg-warning/10 px-2 py-0.5 text-warning">
                  {t("settings.modelCatalogStale")}
                </span>
              )}
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">
            {t("settings.defaultModel")}
          </label>
          <Input
            value={model}
            onChange={(e) => {
              setModel(e.target.value);
              setContextWindow(null);
            }}
            placeholder={
              provider === "open_ai"
                ? "gpt-5.6"
                : provider === "openrouter"
                  ? "qwen/qwen3.7-max"
                : provider === "anthropic"
                  ? "claude-sonnet-4-6"
                  : provider === "google"
                    ? "gemini-2.5-pro"
                    : provider === "deep_seek"
                      ? "deepseek-v4-pro"
                      : provider === "ollama"
                        ? "llama3.1"
                        : provider === "lm_studio"
                          ? "local-model"
                          : "model-name"
            }
          />
          {activePreset && activePreset.models.length > 0 && (
            <button
              type="button"
              onClick={() => {
                setUseCustomModel(false);
                setModel(activePresetDefaultModel);
                setContextWindow(null);
              }}
              className="text-xs text-text-tertiary hover:text-accent transition-colors cursor-pointer"
            >
              {t("settings.usePresetModels")}
            </button>
          )}
        </div>
      )}

      <CollapsiblePanel
        title={t("settings.advancedSettings")}
        description={t("settings.advancedSettingsDesc")}
        open={showAdvanced}
        onOpenChange={setShowAdvanced}
        summary={
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {t("settings.capabilityRegistryAdvancedDesc")}
          </span>
        }
      >

      {showAdvanced && (
        <div className="space-y-4 rounded-lg border border-border bg-surface-2 p-4">
          {/* Temperature + Max Tokens — side by side */}
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {t("settings.temperature")}
              </label>
              <Input
                type="number"
                value={temperature}
                onChange={(e) =>
                  setTemperature(parseFloat(e.target.value) || 0)
                }
                min={0}
                max={2}
                step={0.1}
              />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {t("settings.maxTokens")}
              </label>
              <Input
                type="number"
                value={maxTokens ?? ""}
                onChange={(e) => {
                  const value = e.target.value.trim();
                  const parsed = Number.parseInt(value, 10);
                  setMaxTokens(value === "" || Number.isNaN(parsed) ? null : parsed);
                }}
                placeholder={t("settings.maxTokensAutoPlaceholder")}
                min={1}
                max={4294967295}
                step={1}
              />
              <p className="text-xs text-text-tertiary">
                {t("settings.maxTokensHelp")}
              </p>
            </div>
          </div>

          {/* Context Window Override */}
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">
              {t("settings.contextWindow")}
            </label>
            <Input
              type="number"
              value={contextWindow ?? ""}
              onChange={(e) => {
                const val = e.target.value.trim();
                setContextWindow(val ? parseInt(val) || null : null);
              }}
              placeholder={t("settings.contextWindowPlaceholder")}
              min={1024}
              step={1}
            />
            <p className="text-xs text-text-tertiary">
              {t("settings.contextWindowHelp")}
            </p>
          </div>

          <div className="space-y-2 border-t border-border/70 pt-4">
            <div>
              <p className="text-sm font-medium text-text-primary">
                {t("settings.providerStreaming")}
              </p>
              <p className="text-xs text-text-tertiary">
                {t("settings.providerStreamingHelp")}
              </p>
            </div>
            <div className="grid gap-3 md:grid-cols-3">
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-text-secondary">
                  {t("settings.streamIdleTimeoutMs")}
                </label>
                <Input
                  type="number"
                  value={streamIdleTimeoutMs ?? ""}
                  onChange={(event) => {
                    const value = event.target.value.trim();
                    setStreamIdleTimeoutMs(value ? Number.parseInt(value, 10) || null : null);
                  }}
                  placeholder="300000"
                  min={1000}
                  max={3600000}
                  step={1000}
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-text-secondary">
                  {t("settings.connectTimeoutMs")}
                </label>
                <Input
                  type="number"
                  value={connectTimeoutMs ?? ""}
                  onChange={(event) => {
                    const value = event.target.value.trim();
                    setConnectTimeoutMs(value ? Number.parseInt(value, 10) || null : null);
                  }}
                  placeholder="10000"
                  min={1000}
                  max={300000}
                  step={1000}
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-text-secondary">
                  {t("settings.streamMaxRetries")}
                </label>
                <Input
                  type="number"
                  value={streamMaxRetries ?? ""}
                  onChange={(event) => {
                    const value = event.target.value.trim();
                    setStreamMaxRetries(value ? Number.parseInt(value, 10) || 0 : null);
                  }}
                  placeholder="2"
                  min={0}
                  max={10}
                  step={1}
                />
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Reasoning / Thinking */}
      <div className="space-y-3">
        <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
          <BrainCircuit size={16} className="text-accent" />
          {t("settings.reasoningSection")}
        </div>

        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={reasoningEnabled === true}
            disabled={!supportsReasoning || reasoningAlwaysOn}
            onChange={(e) => {
              if (!supportsReasoning) {
                return;
              }
              const enabled = e.target.checked;
              setReasoningEnabled(enabled);
              if (enabled) {
                setThinkingBudget(defaultThinkingBudget(reasoningCapability));
                setReasoningEffort(defaultReasoningEffort(reasoningCapability));
              } else {
                setThinkingBudget(null);
                setReasoningEffort(null);
              }
            }}
            className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
          />
          <span className="text-sm text-text-primary">
            {t(reasoningAlwaysOn ? "settings.reasoningAlwaysOn" : "settings.enableReasoning")}
          </span>
        </label>
        {!supportsReasoning && (
          <p className="text-xs text-text-tertiary">
            {t("settings.reasoningUnsupported")}
          </p>
        )}

        {reasoningEnabled === true && supportsReasoning && (
          <div className="space-y-4 rounded-lg border border-border bg-surface-2 p-4 ml-1">
            {/* Thinking Budget */}
            {supportsThinkingBudget && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">
                  {t("settings.thinkingBudget")}
                </label>
                <Input
                  type="number"
                  value={thinkingBudget ?? ""}
                  onChange={(e) => {
                    const val = e.target.value.trim();
                    if (!val) {
                      setThinkingBudget(null);
                      return;
                    }
                    const parsed = Number.parseInt(val, 10);
                    setThinkingBudget(Number.isNaN(parsed) ? null : parsed);
                    if (!Number.isNaN(parsed) && reasoningControlsExclusive) {
                      setReasoningEffort(null);
                    }
                  }}
                  placeholder={String(defaultThinkingBudget(reasoningCapability) ?? "")}
                  min={thinkingBudgetCapability?.allowZero ? 0 : thinkingBudgetCapability?.minTokens ?? 1}
                  max={thinkingBudgetCapability?.maxTokens}
                  step={thinkingBudgetCapability?.step ?? 1}
                />
                <p className="text-xs text-text-tertiary">
                  {t("settings.thinkingBudgetHelp")}
                </p>
              </div>
            )}

            {/* Reasoning Effort */}
            {supportsReasoningEffort && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">
                  {t("settings.reasoningEffort")}
                </label>
                <NexaSelect
                  value={
                    normalizeReasoningEffort(
                      reasoningEffort,
                      reasoningCapability,
                    ) ??
                    reasoningEffortOptions[0] ??
                    ""
                  }
                  onChange={(e) => {
                    setReasoningEffort(e.target.value);
                    if (reasoningControlsExclusive) {
                      setThinkingBudget(null);
                    }
                  }}
                  className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
                >
                  {reasoningEffortOptions.map((level) => (
                    <option key={level} value={level}>
                      {t(REASONING_EFFORT_LABEL_KEYS[level])}
                    </option>
                  ))}
                </NexaSelect>
                <p className="text-xs text-text-tertiary">
                  {t("settings.reasoningEffortHelp")}
                </p>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Max Tool Iterations */}
      {showAdvanced && (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">
            {t("settings.maxIterations")}
          </label>
          <Input
            type="number"
            value={maxIterations ?? ""}
            onChange={(e) => {
              const val = e.target.value.trim();
              setMaxIterations(val ? parseInt(val) || null : null);
            }}
            placeholder="6"
            min={1}
            max={50}
            step={1}
          />
          <p className="text-xs text-text-tertiary">
            {t("settings.maxIterationsHelp")}
          </p>
        </div>
      )}

      {/* Summarization Model (cost optimization) */}
      {showAdvanced && (
        <div className="space-y-3 border-t border-border pt-4">
          <h4 className="text-sm font-semibold text-text-primary">
            {t("settings.summarizationSection")}
          </h4>
          <p className="text-xs text-text-tertiary">
            {t("settings.summarizationHelp")}
          </p>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">
              {t("settings.summarizationModel")}
            </label>
            <Input
              value={summarizationModel ?? ""}
              onChange={(e) => setSummarizationModel(e.target.value || null)}
              placeholder={t("settings.summarizationModelPlaceholder")}
            />
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">
              {t("settings.summarizationProvider")}
            </label>
            <NexaSelect
              value={summarizationProvider ?? ""}
              onChange={(e) => setSummarizationProvider(e.target.value || null)}
              className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
            >
              <option value="">{t("settings.sameAsMain")}</option>
              {PROVIDER_LABEL_KEYS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {t(opt.labelKey as any)}
                </option>
              ))}
            </NexaSelect>
            <p className="text-xs text-text-tertiary">
              {t("settings.summarizationProviderHelp")}
            </p>
          </div>
        </div>
      )}

      {showAdvanced && (
        <div className="space-y-3 border-t border-border pt-4">
          <div className="flex items-center justify-between gap-3">
            <div>
              <h4 className="text-sm font-semibold text-text-primary">
                {t("settings.subagents")}
              </h4>
              <p className="text-xs text-text-tertiary">
                {t("settings.subagentsDesc")}
              </p>
            </div>
            <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
              {t("settings.selectedToolsSummary", {
                selected: String(visibleSelectedToolCount),
                total: String(subagentToolCatalog.length),
              })}
            </span>
          </div>

          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {t("settings.maxParallelWorkers")}
              </label>
              <Input
                type="number"
                value={subagentMaxParallel ?? ""}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  setSubagentMaxParallel(val ? parseInt(val) || null : null);
                }}
                min={1}
                max={12}
                step={1}
              />
              <p className="text-xs text-text-tertiary">
                {t("settings.maxParallelWorkersDesc")}
              </p>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {t("settings.maxWorkerCallsPerTurn")}
              </label>
              <Input
                type="number"
                value={subagentMaxCallsPerTurn ?? ""}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  setSubagentMaxCallsPerTurn(
                    val ? parseInt(val) || null : null,
                  );
                }}
                min={1}
                max={32}
                step={1}
              />
              <p className="text-xs text-text-tertiary">
                {t("settings.maxWorkerCallsPerTurnDesc")}
              </p>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {t("settings.tokenBudgetPerTurn")}
              </label>
              <Input
                type="number"
                value={subagentTokenBudget ?? ""}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  setSubagentTokenBudget(val ? parseInt(val) || null : null);
                }}
                min={256}
                step={256}
              />
              <p className="text-xs text-text-tertiary">
                {t("settings.tokenBudgetPerTurnDesc")}
              </p>
            </div>
          </div>

          <div className="grid gap-4 md:grid-cols-3">
            {[
              {
                label: "Worker model context capacity",
                value: subagentInputContextLimit,
                setValue: setSubagentInputContextLimit,
                min: 1024,
                step: 1024,
                placeholder: "Auto from model catalog",
              },
              {
                label: "Parent-history handoff per worker",
                value: subagentHandoffContextTokens,
                setValue: setSubagentHandoffContextTokens,
                min: 1024,
                step: 1024,
                placeholder: "Auto fair-share allocation",
              },
              {
                label: "Max output per model step",
                value: subagentMaxOutputTokens,
                setValue: setSubagentMaxOutputTokens,
                min: 256,
                step: 256,
                placeholder: "Auto: profile/fair share, catalog ceiling",
              },
              {
                label: "Max actual tokens per worker",
                value: subagentMaxActualTokens,
                setValue: setSubagentMaxActualTokens,
                min: 1024,
                step: 1024,
                placeholder: "Auto from Nexus fair share",
              },
              {
                label: "Total cost soft limit (µUSD)",
                value: subagentCostLimitMicros,
                setValue: setSubagentCostLimitMicros,
                min: 0,
                step: 1000,
                placeholder: "Disabled",
              },
              {
                label: "Queue deadline (ms)",
                value: subagentQueueDeadlineMs,
                setValue: setSubagentQueueDeadlineMs,
                min: 100,
                step: 100,
                placeholder: "Auto: 15000",
              },
              {
                label: "Provider connect deadline (ms)",
                value: subagentConnectDeadlineMs,
                setValue: setSubagentConnectDeadlineMs,
                min: 100,
                step: 100,
                placeholder: "Auto: 15000 / long model 90000",
              },
              {
                label: "First token deadline (ms)",
                value: subagentFirstTokenDeadlineMs,
                setValue: setSubagentFirstTokenDeadlineMs,
                min: 100,
                step: 100,
                placeholder: "Auto: 45000 / long model 150000",
              },
              {
                label: "Worker run deadline (ms)",
                value: subagentRunDeadlineMs,
                setValue: setSubagentRunDeadlineMs,
                min: 1000,
                step: 1000,
                placeholder: "Auto: 180000 / long model 360000",
              },
            ].map(({ label, value, setValue, min, step, placeholder }) => (
              <div key={label} className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{label}</label>
                <Input
                  type="number"
                  value={value ?? ""}
                  onChange={(event) => {
                    const raw = event.target.value.trim();
                    setValue(raw ? Number.parseInt(raw, 10) || null : null);
                  }}
                  min={min}
                  step={step}
                  placeholder={placeholder}
                />
              </div>
            ))}
          </div>

          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={setRecommendedSubagentTools}
              >
                {t("common.recommended")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={setAllSubagentTools}
              >
                {t("common.enableAll")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={clearSubagentTools}
              >
                {t("common.disableAll")}
              </Button>
            </div>

            {subagentToolsByGroup.map((group) => {
              const groupToolNames = group.tools.map((tool) => tool.name);
              const selectedCount = groupToolNames.filter((name) =>
                subagentAllowedTools.includes(name),
              ).length;

              return (
                <CollapsiblePanel
                  key={group.id}
                  title={group.label}
                  description={group.description}
                  defaultOpen={false}
                  summary={
                    <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
                      {selectedCount}/{group.tools.length}
                    </span>
                  }
                >
                  <div className="space-y-3">
                    <div className="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => setSubagentToolGroupSelection(groupToolNames, true)}
                      >
                        {t("common.selectGroup")}
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => setSubagentToolGroupSelection(groupToolNames, false)}
                      >
                        {t("common.clearGroup")}
                      </Button>
                    </div>
                    <div className="grid gap-2 md:grid-cols-2">
                      {group.tools.map((tool) => {
                        const checked = subagentAllowedTools.includes(tool.name);
                        return (
                          <label
                            key={tool.name}
                            className={`flex cursor-pointer items-start gap-2 rounded-lg border px-2.5 py-2 transition-colors ${
                              checked
                                ? "border-accent/35 bg-accent/8"
                                : "border-border/70 bg-surface-2 hover:border-border-hover"
                            }`}
                          >
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(event) => {
                                setSubagentAllowedTools((prev) => {
                                  const next = new Set(prev);
                                  if (event.target.checked) {
                                    next.add(tool.name);
                                  } else {
                                    next.delete(tool.name);
                                  }
                                  return orderToolSelection(Array.from(next));
                                });
                              }}
                              className="mt-0.5 h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
                            />
                            <span className="min-w-0">
                              <span className="block text-xs font-medium text-text-primary">
                                {tool.label}
                              </span>
                              <span className="mt-0.5 block line-clamp-2 text-[11px] text-text-tertiary">
                                {tool.description}
                              </span>
                              {tool.serverName && (
                                <span className="mt-0.5 block truncate text-[10px] text-text-tertiary">
                                  {t("settings.mcpServerPrefix", { server: tool.serverName })}
                                </span>
                              )}
                            </span>
                          </label>
                        );
                      })}
                    </div>
                  </div>
                </CollapsiblePanel>
              );
            })}
          </div>

          <CollapsiblePanel
            title={t("settings.delegatedSkills")}
            description={t("settings.delegatedSkillsDesc")}
            summary={
              <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
                {subagentAllowedSkillIds.length}/{enabledSkills.length}
              </span>
            }
          >
            <div className="space-y-3">
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setSubagentAllowedSkillIds(orderSkillSelection(availableSkillIds))}
                >
                  {t("common.enableAll")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setSubagentAllowedSkillIds([])}
                >
                  {t("common.disableAll")}
                </Button>
              </div>

              {enabledSkills.length > 0 ? (
                <div className="grid gap-2 md:grid-cols-2">
                {enabledSkills.map((skill) => {
                  const checked = subagentAllowedSkillIds.includes(skill.id);
                  return (
                    <label
                      key={skill.id}
                      className={`flex cursor-pointer items-start gap-3 rounded-xl border px-3 py-3 transition-colors ${
                        checked
                          ? "border-accent/35 bg-accent/8"
                          : "border-border/70 bg-surface-2 hover:border-border-hover"
                      }`}
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(event) => {
                          setSubagentAllowedSkillIds((prev) => {
                            const next = new Set(prev);
                            if (event.target.checked) {
                              next.add(skill.id);
                            } else {
                              next.delete(skill.id);
                            }
                            return orderSkillSelection(Array.from(next));
                          });
                        }}
                        className="mt-0.5 h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
                      />
                      <span className="min-w-0">
                        <span className="block text-sm font-medium text-text-primary">
                          {skill.name}
                        </span>
                        <span className="mt-1 block text-xs text-text-tertiary line-clamp-3">
                          {skill.content}
                        </span>
                        <span className="mt-1 block font-mono text-[11px] text-text-tertiary">
                          {skill.id}
                        </span>
                      </span>
                    </label>
                  );
                })}
                </div>
              ) : (
                <div className="rounded-xl border border-dashed border-border/70 bg-surface-2 px-3 py-4 text-xs text-text-tertiary">
                  {t("settings.noEnabledSkillsToDelegate")}
                </div>
              )}
            </div>
          </CollapsiblePanel>
        </div>
      )}

      </CollapsiblePanel>

      {/* Set as Default */}
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={isDefault}
          onChange={(e) => setIsDefault(e.target.checked)}
          className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
        />
        <span className="text-sm text-text-primary">
          {t("settings.setDefault")}
        </span>
      </label>

      {/* Test Connection */}
      <div className="space-y-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          icon={
            testLoading ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <Zap size={14} />
            )
          }
          loading={testLoading}
          onClick={handleTest}
          disabled={!model.trim() || (!isLocal && !apiKey.trim())}
        >
          {testLoading ? t("settings.testing") : t("settings.testConnection")}
        </Button>
        {testResult && (
          <div
            className={`flex items-center gap-2 text-xs ${testResult.ok ? "text-success" : "text-danger"}`}
          >
            {testResult.ok ? <CheckCircle size={12} /> : <X size={12} />}
            <span>{testResult.message}</span>
          </div>
        )}
      </div>

      {/* Actions */}
      <div className="flex items-center justify-end gap-3 border-t border-border pt-4">
        <Button type="button" variant="ghost" size="md" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button
          type="submit"
          variant="primary"
          size="md"
          icon={<Save size={16} />}
          loading={isSaving}
          disabled={!canSubmit}
        >
          {t("common.save")}
        </Button>
      </div>
    </form>
  );
}
