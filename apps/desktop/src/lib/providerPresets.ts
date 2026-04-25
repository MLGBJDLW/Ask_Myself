import providerPresets from "../../../../shared/provider-presets.json";

export type ReasoningEffortLevel =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "max"
  | "xhigh";

export interface ThinkingBudgetCapability {
  enabled: boolean;
  defaultTokens?: number;
  minTokens?: number;
  maxTokens?: number;
  step?: number;
}

export interface ReasoningCapability {
  effortLevels?: ReasoningEffortLevel[];
  defaultEffort?: ReasoningEffortLevel;
  thinkingBudget?: ThinkingBudgetCapability;
}

export interface ProviderCapabilities {
  reasoning?: ReasoningCapability | null;
}

export interface ProviderModelPreset {
  id: string;
  name: string;
  tagKey?: string;
  recommended?: boolean;
  capabilities?: ProviderCapabilities;
}

export interface ProviderPreset {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  models: ProviderModelPreset[];
  requiresApiKey: boolean;
  icon: string;
  description: string;
  capabilities?: ProviderCapabilities;
}

export const PROVIDER_PRESETS: ProviderPreset[] =
  providerPresets as ProviderPreset[];

const OPENAI_O_REASONING: ReasoningCapability = {
  effortLevels: ["low", "medium", "high"],
  defaultEffort: "medium",
  thinkingBudget: { enabled: false },
};

const OPENAI_GPT55_REASONING: ReasoningCapability = {
  effortLevels: ["none", "low", "medium", "high", "xhigh"],
  defaultEffort: "medium",
  thinkingBudget: { enabled: false },
};

const OPENAI_GPT5_FRONTIER_REASONING: ReasoningCapability = {
  effortLevels: ["none", "low", "medium", "high", "xhigh"],
  defaultEffort: "none",
  thinkingBudget: { enabled: false },
};

const OPENAI_GPT5_PRO_REASONING: ReasoningCapability = {
  effortLevels: ["medium", "high", "xhigh"],
  defaultEffort: "medium",
  thinkingBudget: { enabled: false },
};

const OPENAI_GPT51_REASONING: ReasoningCapability = {
  effortLevels: ["none", "low", "medium", "high"],
  defaultEffort: "none",
  thinkingBudget: { enabled: false },
};

const OPENAI_GPT5_REASONING: ReasoningCapability = {
  effortLevels: ["minimal", "low", "medium", "high"],
  defaultEffort: "medium",
  thinkingBudget: { enabled: false },
};

const OPENAI_CODEX_REASONING: ReasoningCapability = {
  effortLevels: ["low", "medium", "high", "xhigh"],
  defaultEffort: "high",
  thinkingBudget: { enabled: false },
};

const DEEPSEEK_REASONING: ReasoningCapability = {
  effortLevels: ["high", "max"],
  defaultEffort: "high",
  thinkingBudget: { enabled: false },
};

const ANTHROPIC_BUDGET_REASONING: ReasoningCapability = {
  thinkingBudget: {
    enabled: true,
    defaultTokens: 10000,
    minTokens: 1024,
    step: 1024,
  },
};

const GEMINI_BUDGET_REASONING: ReasoningCapability = {
  thinkingBudget: {
    enabled: true,
    defaultTokens: 10000,
    minTokens: 128,
    maxTokens: 32768,
    step: 128,
  },
};

function normalizePresetBaseUrl(baseUrl: string | null | undefined): string {
  return (baseUrl ?? "").trim().replace(/\/+$/, "").toLowerCase();
}

export function findProviderPreset(input: {
  provider: string;
  baseUrl?: string | null;
}): ProviderPreset | null {
  const provider = input.provider.trim();
  const normalizedBaseUrl = normalizePresetBaseUrl(input.baseUrl);

  if (normalizedBaseUrl) {
    const exactMatch = PROVIDER_PRESETS.find(
      (preset) =>
        preset.provider === provider &&
        normalizePresetBaseUrl(preset.baseUrl) === normalizedBaseUrl,
    );
    if (exactMatch) {
      return exactMatch;
    }
  }

  const providerMatches = PROVIDER_PRESETS.filter(
    (preset) => preset.provider === provider,
  );
  if (providerMatches.length === 1) {
    return providerMatches[0];
  }

  return null;
}

function normalizeModelId(model: string | null | undefined): string {
  return (model ?? "").trim().toLowerCase();
}

function hasExplicitReasoningCapability(
  capabilities: ProviderCapabilities | undefined,
): boolean {
  return Object.prototype.hasOwnProperty.call(capabilities ?? {}, "reasoning");
}

function getExplicitReasoningCapability(
  capabilities: ProviderCapabilities | undefined,
): ReasoningCapability | null | undefined {
  if (!hasExplicitReasoningCapability(capabilities)) {
    return undefined;
  }
  return capabilities?.reasoning ?? null;
}

export function findProviderModelPreset(input: {
  provider: string;
  baseUrl?: string | null;
  model?: string | null;
}): ProviderModelPreset | null {
  const preset = findProviderPreset(input);
  const model = normalizeModelId(input.model);
  if (!preset || !model) {
    return null;
  }
  return (
    preset.models.find((candidate) => normalizeModelId(candidate.id) === model) ??
    null
  );
}

function inferReasoningCapability(input: {
  provider: string;
  model?: string | null;
}): ReasoningCapability | null {
  const provider = input.provider.trim();
  const model = normalizeModelId(input.model);

  if (!model) {
    return null;
  }

  if (model.includes("deepseek")) {
    if (model.includes("deepseek-chat")) {
      return null;
    }
    if (
      model.includes("deepseek-v4") ||
      model.includes("deepseek-reasoner") ||
      model.includes("deepseek-r1")
    ) {
      return DEEPSEEK_REASONING;
    }
  }

  if (
    provider === "open_ai" ||
    provider === "azure_open_ai" ||
    provider === "custom"
  ) {
    if (/^o[134]/.test(model)) {
      return OPENAI_O_REASONING;
    }
    if (/^gpt-5.*codex/.test(model)) {
      return OPENAI_CODEX_REASONING;
    }
    if (model.startsWith("gpt-5.5-pro")) {
      return null;
    }
    if (/^gpt-5\.(2|4).*?-pro$/.test(model)) {
      return OPENAI_GPT5_PRO_REASONING;
    }
    if (model.startsWith("gpt-5.5")) {
      return OPENAI_GPT55_REASONING;
    }
    if (model.startsWith("gpt-5.4") || model.startsWith("gpt-5.2")) {
      return OPENAI_GPT5_FRONTIER_REASONING;
    }
    if (model.startsWith("gpt-5.1")) {
      return OPENAI_GPT51_REASONING;
    }
    if (model.startsWith("gpt-5")) {
      return OPENAI_GPT5_REASONING;
    }
  }

  if (provider === "google") {
    if (model.includes("gemini-2.5") || model.startsWith("gemini-3")) {
      return GEMINI_BUDGET_REASONING;
    }
  }

  if (provider === "anthropic") {
    if (/^claude-(opus|sonnet|haiku)-4/.test(model) || model.includes("claude-3-7")) {
      return ANTHROPIC_BUDGET_REASONING;
    }
  }

  return null;
}

export function getReasoningCapability(input: {
  provider: string;
  baseUrl?: string | null;
  model?: string | null;
}): ReasoningCapability | null {
  const preset = findProviderPreset(input);
  const modelPreset = findProviderModelPreset(input);
  const modelCapability = getExplicitReasoningCapability(
    modelPreset?.capabilities,
  );
  if (modelCapability !== undefined) {
    return modelCapability;
  }

  const providerCapability = getExplicitReasoningCapability(
    preset?.capabilities,
  );
  if (providerCapability !== undefined) {
    return providerCapability;
  }

  return inferReasoningCapability(input);
}
