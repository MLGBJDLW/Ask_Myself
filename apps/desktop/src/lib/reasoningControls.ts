import type {
  ReasoningCapability,
  ReasoningEffortLevel,
} from './providerPresets';

export function defaultReasoningEffort(
  capability: ReasoningCapability | null,
): ReasoningEffortLevel | null {
  const levels = capability?.effortLevels ?? [];
  if (levels.length === 0) return null;
  return capability?.defaultEffort && levels.includes(capability.defaultEffort)
    ? capability.defaultEffort
    : levels.find((level) => level !== 'none') ?? levels[0];
}

export function normalizeReasoningEffort(
  value: string | null,
  capability: ReasoningCapability | null,
): ReasoningEffortLevel | null {
  const levels = capability?.effortLevels ?? [];
  if (levels.length === 0) return null;
  return levels.includes(value as ReasoningEffortLevel)
    ? value as ReasoningEffortLevel
    : defaultReasoningEffort(capability);
}

export function defaultThinkingBudget(
  capability: ReasoningCapability | null,
): number | null {
  const budget = capability?.thinkingBudget;
  return budget?.enabled ? budget.defaultTokens ?? null : null;
}

export function normalizeThinkingBudget(
  value: number | null,
  capability: ReasoningCapability | null,
): number | null {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) return null;
  const fallback = defaultThinkingBudget(capability);
  if ((!Number.isFinite(value) || value === null) && fallback === null) return null;
  let next = Number.isFinite(value) && value !== null ? value : fallback!;
  if (budget.allowZero && next === 0) return 0;
  if (budget.minTokens != null) next = Math.max(next, budget.minTokens);
  if (budget.maxTokens != null) next = Math.min(next, budget.maxTokens);
  return Math.round(next);
}

export function thinkingBudgetOptions(capability: ReasoningCapability | null): number[] {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) return [];
  const fallback = defaultThinkingBudget(capability);
  const candidates = [
    budget.allowZero ? 0 : undefined,
    budget.minTokens,
    fallback == null ? undefined : Math.round(fallback / 2),
    fallback,
    budget.maxTokens,
  ];
  return Array.from(new Set(candidates
    .map((candidate) => normalizeThinkingBudget(candidate ?? null, capability))
    .filter((candidate): candidate is number => candidate !== null)))
    .sort((left, right) => left - right);
}
