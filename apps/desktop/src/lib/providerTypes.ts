export type ReasoningEffortLevel =
  | 'none'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'max'
  | 'xhigh';

export interface ThinkingBudgetCapability {
  enabled: boolean;
  defaultTokens?: number;
  minTokens?: number;
  maxTokens?: number;
  step?: number;
  allowZero?: boolean;
}

export interface ReasoningCapability {
  effortLevels?: ReasoningEffortLevel[];
  defaultEffort?: ReasoningEffortLevel;
  thinkingBudget?: ThinkingBudgetCapability;
}

export interface ProviderCapabilities {
  reasoning?: ReasoningCapability | null;
  vision?: boolean | null;
}

export type ModelCatalogSource = 'official' | 'discovered' | 'curated';
export type ModelLifecycleStatus =
  | 'active'
  | 'preview'
  | 'legacy'
  | 'deprecated'
  | 'removed';

export interface ProviderModelPreset {
  id: string;
  name: string;
  tagKey?: string;
  recommended?: boolean;
  capabilities?: ProviderCapabilities;
  source?: ModelCatalogSource;
  status?: ModelLifecycleStatus;
  regions?: string[];
  lastVerifiedAt?: string | null;
  modalities?: string[];
  supportsTools?: boolean | null;
  supportsStructuredOutput?: boolean | null;
  reasoningEfforts?: ReasoningEffortLevel[];
}
