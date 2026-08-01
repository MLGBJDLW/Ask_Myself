import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import {
  Eye,
  EyeOff,
  Loader2,
  Zap,
  Save,
  X,
  CheckCircle,
  BrainCircuit,
  Search,
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
  type ReasoningCapability,
  type ReasoningEffortLevel,
  type ProviderModelPreset,
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
  catalogEndpointIdForSelection,
  modelDescriptorSummary,
  selectImplicitDefault,
} from "../../lib/modelCatalog";
import { ModelDescriptorBadges } from "./ModelDescriptorBadges";

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
const MODEL_SEARCH_THRESHOLD = 20;

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

function modelNamespace(modelId: string): string {
  const raw = modelId.split("/")[0] ?? "";
  return raw.replace(/^~+/, "").replace(/[-_]/g, " ");
}

function defaultReasoningEffort(
  capability: ReasoningCapability | null,
): ReasoningEffortLevel | null {
  const levels = capability?.effortLevels ?? [];
  if (levels.length === 0) {
    return null;
  }
  return capability?.defaultEffort && levels.includes(capability.defaultEffort)
    ? capability.defaultEffort
    : levels[0];
}

function normalizeReasoningEffort(
  value: string | null,
  capability: ReasoningCapability | null,
): ReasoningEffortLevel | null {
  const levels = capability?.effortLevels ?? [];
  if (levels.length === 0) {
    return null;
  }
  return levels.includes(value as ReasoningEffortLevel)
    ? (value as ReasoningEffortLevel)
    : defaultReasoningEffort(capability);
}

function defaultThinkingBudget(
  capability: ReasoningCapability | null,
): number | null {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) {
    return null;
  }
  return budget.defaultTokens ?? budget.minTokens ?? 10000;
}

function normalizeThinkingBudget(
  value: number | null,
  capability: ReasoningCapability | null,
): number | null {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) {
    return null;
  }

  const fallback = defaultThinkingBudget(capability) ?? 10000;
  let next = Number.isFinite(value) && value !== null ? value : fallback;
  if (budget.allowZero && next === 0) {
    return 0;
  }
  if (budget.minTokens != null) {
    next = Math.max(next, budget.minTokens);
  }
  if (budget.maxTokens != null) {
    next = Math.min(next, budget.maxTokens);
  }
  return Math.round(next);
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
  const [maxTokens, setMaxTokens] = useState(config?.maxTokens ?? 4096);
  const [contextWindow, setContextWindow] = useState<number | null>(
    config?.contextWindow ?? null,
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
    config?.subagentMaxParallel ?? 3,
  );
  const [subagentMaxCallsPerTurn, setSubagentMaxCallsPerTurn] = useState<
    number | null
  >(config?.subagentMaxCallsPerTurn ?? 6);
  const [subagentTokenBudget, setSubagentTokenBudget] = useState<number | null>(
    config?.subagentTokenBudget ?? 32000,
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
  const [modelSearch, setModelSearch] = useState("");
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
    providerEndpointId: catalogEndpointIdForSelection(
      initialPresetModel?.descriptor,
      initialPreset?.baseUrl,
      initialBaseUrl,
    ),
    modelId: initialPresetModel?.descriptor.id ?? config?.modelId ?? initialModel,
    temperature: config?.temperature ?? 0.3,
    maxTokens: config?.maxTokens ?? 4096,
    contextWindow: config?.contextWindow ?? null,
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
    subagentMaxParallel: config?.subagentMaxParallel ?? 3,
    subagentMaxCallsPerTurn: config?.subagentMaxCallsPerTurn ?? 6,
    subagentTokenBudget: config?.subagentTokenBudget ?? 32000,
  });

  const isLocal =
    LOCAL_PROVIDERS.includes(provider) ||
    (preset ? !preset.requiresApiKey : false);
  const curatedPreset =
    findProviderPreset({ provider, baseUrl }) ??
    (preset?.provider === provider ? preset : null);
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
  const usesSearchableModelPicker =
    (activePreset?.models.length ?? 0) > MODEL_SEARCH_THRESHOLD;
  const filteredPresetModels = useMemo(() => {
    if (!activePreset) {
      return [];
    }
    const needle = modelSearch.trim().toLowerCase();
    if (!needle) {
      return activePreset.models;
    }

    return activePreset.models.filter((candidate) => {
      const tagLabel = candidate.tagKey
        ? t(candidate.tagKey as TranslationKey)
        : "";
      return [
        candidate.name,
        candidate.id,
        modelNamespace(candidate.id),
        tagLabel,
      ]
        .filter(Boolean)
        .some((part) => part.toLowerCase().includes(needle));
    });
  }, [activePreset, modelSearch, t]);
  const reasoningCapability = useMemo(
    () => getReasoningCapability({ provider, baseUrl, model }),
    [provider, baseUrl, model],
  );
  const reasoningEffortOptions = reasoningCapability?.effortLevels ?? [];
  const thinkingBudgetCapability = reasoningCapability?.thinkingBudget;
  const supportsReasoning = reasoningCapability !== null;
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
    setModelSearch("");
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

    if (reasoningEnabled !== true) {
      if (thinkingBudget !== null) {
        setThinkingBudget(null);
      }
      if (reasoningEffort !== null) {
        setReasoningEffort(null);
      }
      return;
    }

    const nextThinkingBudget = normalizeThinkingBudget(
      thinkingBudget,
      reasoningCapability,
    );
    if (thinkingBudget !== nextThinkingBudget) {
      setThinkingBudget(nextThinkingBudget);
    }

    const nextReasoningEffort = normalizeReasoningEffort(
      reasoningEffort,
      reasoningCapability,
    );
    if (reasoningEffort !== nextReasoningEffort) {
      setReasoningEffort(nextReasoningEffort);
    }
  }, [
    reasoningCapability,
    reasoningEffort,
    reasoningEnabled,
    supportsReasoning,
    thinkingBudget,
  ]);

  const buildInput = useCallback(
    (): SaveAgentConfigInput => {
      const normalizedReasoningEnabled =
        supportsReasoning && reasoningEnabled === true ? true : null;
      const normalizedThinkingBudget =
        normalizedReasoningEnabled && supportsThinkingBudget
          ? normalizeThinkingBudget(thinkingBudget, reasoningCapability)
          : null;
      const normalizedReasoningEffort =
        normalizedReasoningEnabled && supportsReasoningEffort
          ? normalizeReasoningEffort(reasoningEffort, reasoningCapability)
          : null;

      return {
        id: config?.id ?? null,
        name: name.trim(),
        provider,
        apiKey: isLocal ? "" : apiKey,
        baseUrl: normalizeBaseUrl(baseUrl) || null,
        model: model.trim(),
        providerEndpointId: catalogEndpointIdForSelection(
          selectedPresetModel?.descriptor ?? activePreset?.models[0]?.descriptor,
          activePreset?.baseUrl,
          baseUrl,
        ),
        modelId: selectedPresetModel?.descriptor.id ?? model.trim(),
        temperature,
        maxTokens,
        contextWindow: contextWindow,
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
      isDefault,
      reasoningEnabled,
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
        <select
          value={provider}
          onChange={(e) => {
            setProvider(e.target.value as ProviderType);
            setContextWindow(null);
          }}
          className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
        >
          {PROVIDER_LABEL_KEYS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {t(opt.labelKey as any)}
            </option>
          ))}
        </select>
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
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-3">
            <label className="text-sm font-medium text-text-primary">
              {t("settings.defaultModel")}
            </label>
            <span className="flex items-center gap-2">
              {usesSearchableModelPicker && (
                <span className="text-xs text-text-tertiary">
                  {filteredPresetModels.length}/{activePreset.models.length}
                </span>
              )}
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
          {usesSearchableModelPicker ? (
            <div className="space-y-2">
              <Input
                value={modelSearch}
                onChange={(e) => setModelSearch(e.target.value)}
                placeholder={t("settings.modelSearchPlaceholder")}
                icon={<Search size={15} />}
              />
              <div className="max-h-72 overflow-y-auto rounded-md border border-border bg-surface-1">
                {filteredPresetModels.length === 0 ? (
                  <div className="px-3 py-6 text-center text-sm text-text-tertiary">
                    {t("settings.modelSearchNoResults")}
                  </div>
                ) : (
                  filteredPresetModels.map((m: ProviderModelPreset) => {
                    const selected = m.id === model;
                    const tag = m.tagKey
                      ? t(m.tagKey as TranslationKey)
                      : null;
                    return (
                      <button
                        key={m.id}
                        type="button"
                        aria-pressed={selected}
                        disabled={m.descriptor.availableToCredential === false}
                        onClick={() => {
                          setModel(m.id);
                          setContextWindow(null);
                        }}
                        className={`flex w-full items-center justify-between gap-3 border-b border-border/60 px-3 py-2.5 text-left transition-colors last:border-b-0 disabled:cursor-not-allowed disabled:opacity-50 ${
                          selected
                            ? "bg-accent/10 text-text-primary"
                            : "text-text-secondary hover:bg-surface-2 hover:text-text-primary"
                        }`}
                      >
                        <span className="min-w-0">
                          <span className="block truncate text-sm font-medium">
                            {m.name}
                          </span>
                          <span className="block truncate text-xs text-text-tertiary">
                            {m.id} · {modelDescriptorSummary(m.descriptor)}
                          </span>
                        </span>
                        <span className="flex shrink-0 items-center gap-1.5">
                          {tag && (
                            <span className="rounded-full border border-border bg-surface-2 px-2 py-0.5 text-[11px] text-text-secondary">
                              {tag}
                            </span>
                          )}
                          {m.descriptor.source === "discovered" && (
                            <span className="rounded-full border border-warning/40 bg-warning/10 px-2 py-0.5 text-[11px] text-warning">
                              {t("settings.modelSourceDiscovered")}
                            </span>
                          )}
                          {(m.descriptor.lifecycle === "preview" || m.descriptor.lifecycle === "gated" || m.descriptor.lifecycle === "legacy" || m.descriptor.lifecycle === "deprecated") && (
                            <span className="rounded-full border border-border bg-surface-2 px-2 py-0.5 text-[11px] text-text-tertiary">
                              {t(`settings.modelStatus.${m.descriptor.lifecycle}` as TranslationKey)}
                            </span>
                          )}
                          {m.recommended && (
                            <span className="rounded-full bg-accent px-2 py-0.5 text-[11px] font-medium text-white">
                              ★
                            </span>
                          )}
                        </span>
                      </button>
                    );
                  })
                )}
              </div>
              {selectedPresetModel && (
                <div className="space-y-2">
                  <p className="truncate text-xs text-text-tertiary">
                    {selectedPresetModel.id}
                  </p>
                  <ModelDescriptorBadges descriptor={selectedPresetModel.descriptor} />
                </div>
              )}
            </div>
          ) : (
            <select
              value={model}
              onChange={(e) => {
                setModel(e.target.value);
                setContextWindow(null);
              }}
              className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
            >
              {!activePreset.models.some((candidate) => candidate.id === model) && (
                <option value="" disabled>—</option>
              )}
              {activePreset.models.map((m) => (
                <option key={m.id} value={m.id} disabled={m.descriptor.availableToCredential === false}>
                  {m.tagKey
                    ? `${m.name} (${t(m.tagKey as TranslationKey)})`
                    : m.name}
                   {m.descriptor.source === "discovered" ? ` · ${t("settings.modelSourceDiscovered")}` : ""}
                  {m.recommended ? " ★" : ""}
                  {` · ${modelDescriptorSummary(m.descriptor)}`}
                </option>
              ))}
            </select>
          )}
          {!usesSearchableModelPicker && (
            <ModelDescriptorBadges descriptor={selectedPresetModel?.descriptor} />
          )}
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
            {t("settings.selectedToolsSummary", {
              selected: String(visibleSelectedToolCount),
              total: String(subagentToolCatalog.length),
            })}
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
                value={maxTokens}
                onChange={(e) => setMaxTokens(parseInt(e.target.value) || 4096)}
                min={1}
                max={128000}
                step={256}
              />
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
              step={1024}
            />
            <p className="text-xs text-text-tertiary">
              {t("settings.contextWindowHelp")}
            </p>
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
            disabled={!supportsReasoning}
            onChange={(e) => {
              if (!supportsReasoning) {
                return;
              }
              const enabled = e.target.checked;
              setReasoningEnabled(enabled ? true : null);
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
            {t("settings.enableReasoning")}
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
                  }}
                  placeholder={String(
                    defaultThinkingBudget(reasoningCapability) ?? 10000,
                  )}
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
                <select
                  value={
                    normalizeReasoningEffort(
                      reasoningEffort,
                      reasoningCapability,
                    ) ??
                    reasoningEffortOptions[0] ??
                    ""
                  }
                  onChange={(e) => setReasoningEffort(e.target.value)}
                  className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
                >
                  {reasoningEffortOptions.map((level) => (
                    <option key={level} value={level}>
                      {t(REASONING_EFFORT_LABEL_KEYS[level])}
                    </option>
                  ))}
                </select>
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
            <select
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
            </select>
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
