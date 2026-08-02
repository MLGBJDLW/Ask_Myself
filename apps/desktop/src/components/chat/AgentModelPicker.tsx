import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { createPortal } from 'react-dom';
import { AnimatePresence, motion } from 'framer-motion';
import { Check, ChevronDown, ChevronLeft, Search, SlidersHorizontal } from 'lucide-react';
import { useTranslation, type TranslationKey } from '../../i18n';
import {
  PROVIDER_PRESETS,
  findProviderPreset,
  type ProviderModelPreset,
  type ProviderPreset,
  type ReasoningCapability,
  type ReasoningEffortLevel,
} from '../../lib/providerPresets';
import { ProviderIcon } from '../../lib/providerIcons';
import {
  canonicalModelProviderId,
  modelEndpointId,
  projectModelDescriptor,
} from '../../lib/modelCatalog';
import type { AgentConfig } from '../../types/conversation';
import { useOverlayRoot } from '../ui/overlay';

export interface AgentModelSelection {
  config: AgentConfig;
  model: string;
  reasoningEnabled: boolean | null;
  thinkingBudget: number | null;
  reasoningEffort: string | null;
}

interface AgentModelPickerProps {
  agentConfigs: AgentConfig[];
  selectedConfig: AgentConfig | null;
  onSelect: (selection: AgentModelSelection) => void | Promise<void>;
}

interface ProviderRow {
  config: AgentConfig;
  preset: ProviderPreset | null;
  label: string;
  detail: string;
}

interface ModelRow {
  key: string;
  providerRow: ProviderRow;
  model: ProviderModelPreset;
  reasoning: ReasoningCapability | null;
  searchText: string;
}

type PickerStep = 'providers' | 'models' | 'reasoning';

const REASONING_EFFORT_LABEL_KEYS: Record<ReasoningEffortLevel, TranslationKey> = {
  none: 'settings.reasoningNone',
  minimal: 'settings.reasoningMinimal',
  low: 'settings.reasoningLow',
  medium: 'settings.reasoningMedium',
  high: 'settings.reasoningHigh',
  max: 'settings.reasoningMax',
  xhigh: 'settings.reasoningXHigh',
};

const GLOBAL_SEARCH_LIMIT = 120;

function normalizeBaseUrl(value: string | null | undefined): string {
  return (value ?? '').trim().replace(/\/+$/, '').toLowerCase();
}

function normalizeSearch(value: string): string {
  return value.trim().toLowerCase().replace(/\s+/g, ' ');
}

function findPresetForConfig(config: AgentConfig): ProviderPreset | null {
  const exact = findProviderPreset({
    provider: config.provider,
    baseUrl: config.baseUrl,
  });
  if (exact) return exact;

  const baseUrl = normalizeBaseUrl(config.baseUrl);
  if (!baseUrl && config.provider === 'open_ai') {
    return PROVIDER_PRESETS.find((preset) => preset.id === 'openai') ?? null;
  }

  if (baseUrl) {
    return (
      PROVIDER_PRESETS.find(
        (preset) =>
          preset.provider === config.provider &&
          normalizeBaseUrl(preset.baseUrl) === baseUrl,
      ) ?? null
    );
  }

  const providerMatches = PROVIDER_PRESETS.filter(
    (preset) => preset.provider === config.provider,
  );
  return providerMatches.length === 1 ? providerMatches[0] : null;
}

function hasOwnReasoning(model: ProviderModelPreset): boolean {
  return Object.prototype.hasOwnProperty.call(model.capabilities ?? {}, 'reasoning');
}

function reasoningForModel(
  preset: ProviderPreset | null,
  model: ProviderModelPreset,
): ReasoningCapability | null {
  if (hasOwnReasoning(model)) {
    return model.capabilities?.reasoning ?? null;
  }
  return preset?.capabilities?.reasoning ?? null;
}

function defaultReasoningEffort(
  capability: ReasoningCapability | null,
): ReasoningEffortLevel | null {
  const levels = capability?.effortLevels ?? [];
  if (levels.length === 0) return null;
  return capability?.defaultEffort && levels.includes(capability.defaultEffort)
    ? capability.defaultEffort
    : levels[0];
}

function defaultThinkingBudget(capability: ReasoningCapability | null): number | null {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) return null;
  return budget.defaultTokens ?? budget.minTokens ?? 10000;
}

function clampThinkingBudget(
  value: number | null,
  capability: ReasoningCapability | null,
): number | null {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) return null;
  const fallback = defaultThinkingBudget(capability) ?? 10000;
  let next = Number.isFinite(value) && value !== null ? value : fallback;
  if (budget.allowZero && next === 0) return 0;
  if (budget.minTokens != null) next = Math.max(next, budget.minTokens);
  if (budget.maxTokens != null) next = Math.min(next, budget.maxTokens);
  return Math.round(next);
}

function thinkingBudgetOptions(capability: ReasoningCapability | null): number[] {
  const budget = capability?.thinkingBudget;
  if (!budget?.enabled) return [];
  const fallback = defaultThinkingBudget(capability) ?? 10000;
  const candidates = [
    budget.allowZero ? 0 : undefined,
    budget.minTokens,
    Math.round(fallback / 2),
    fallback,
    budget.maxTokens ?? fallback * 2,
  ];
  return Array.from(
    new Set(
      candidates
        .map((value) => clampThinkingBudget(value ?? null, capability))
        .filter((value): value is number => value !== null),
    ),
  ).sort((a, b) => a - b);
}

function makeProviderLabel(config: AgentConfig, preset: ProviderPreset | null): string {
  return config.name?.trim() || preset?.name || `${config.provider}/${config.model}`;
}

function makeModelRows(
  providerRow: ProviderRow,
  t: (key: TranslationKey, params?: Record<string, string>) => string,
): ModelRow[] {
  const models = providerRow.preset?.models ?? [];
  const modelMap = new Map<string, ProviderModelPreset>();
  for (const model of models) {
    modelMap.set(model.id, model);
  }
  if (providerRow.config.model && !modelMap.has(providerRow.config.model)) {
    const presetId = providerRow.preset?.id ?? providerRow.config.provider;
    const fallbackModel = {
      id: providerRow.config.model,
      name: providerRow.config.model,
      status: 'legacy' as const,
      productReadiness: 'known' as const,
    };
    modelMap.set(providerRow.config.model, {
      ...fallbackModel,
      descriptor: projectModelDescriptor(fallbackModel, {
        surface: 'text',
        providerId: canonicalModelProviderId(presetId, providerRow.config.provider),
        endpointId: modelEndpointId('text', presetId),
      }),
    });
  }

  return Array.from(modelMap.values()).map((model) => {
    const tagLabel = model.tagKey ? t(model.tagKey as TranslationKey) : '';
    const searchText = normalizeSearch([
      providerRow.label,
      providerRow.detail,
      providerRow.config.provider,
      providerRow.config.baseUrl,
      model.id,
      model.name,
      tagLabel,
    ].filter(Boolean).join(' '));
    return {
      key: `${providerRow.config.id}:${model.id}`,
      providerRow,
      model,
      reasoning: reasoningForModel(providerRow.preset, model),
      searchText,
    };
  });
}

function scoreModelRow(row: ModelRow, query: string): number {
  if (!query) {
    return row.model.recommended ? 30 : 10;
  }
  const modelId = row.model.id.toLowerCase();
  const modelName = row.model.name.toLowerCase();
  if (modelId === query || modelName === query) return 100;
  if (modelName.startsWith(query)) return 88;
  if (modelId.startsWith(query)) return 84;
  if (row.searchText.includes(query)) return 62;
  const parts = query.split(/[-_:./\s]+/).filter(Boolean);
  return parts.length > 1 && parts.every((part) => row.searchText.includes(part)) ? 52 : 0;
}

function formatBudget(value: number): string {
  return value >= 1000 ? `${Math.round(value / 1000)}k` : String(value);
}

export function AgentModelPicker({
  agentConfigs,
  selectedConfig,
  onSelect,
}: AgentModelPickerProps) {
  const { t } = useTranslation();
  const overlayRoot = useOverlayRoot();
  const [open, setOpen] = useState(false);
  const [panelStyle, setPanelStyle] = useState<CSSProperties>({});
  const [activeConfigId, setActiveConfigId] = useState(selectedConfig?.id ?? agentConfigs[0]?.id ?? '');
  const [activeModelId, setActiveModelId] = useState(selectedConfig?.model ?? '');
  const [pickerStep, setPickerStep] = useState<PickerStep>('providers');
  const [query, setQuery] = useState('');
  const [budgetDraft, setBudgetDraft] = useState('');
  const triggerRef = useRef<HTMLButtonElement>(null);
  const reasoningTriggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const providerRows = useMemo<ProviderRow[]>(
    () =>
      agentConfigs.map((config) => {
        const preset = findPresetForConfig(config);
        return {
          config,
          preset,
          label: makeProviderLabel(config, preset),
          detail: preset?.name
            ? `${preset.name} / ${config.model}`
            : `${config.provider} / ${config.model}`,
        };
      }),
    [agentConfigs],
  );

  const allModelRows = useMemo(
    () => providerRows.flatMap((row) => makeModelRows(row, t)),
    [providerRows, t],
  );

  const normalizedQuery = normalizeSearch(query);
  const activeProviderModelRows = useMemo(
    () => allModelRows.filter((row) => row.providerRow.config.id === activeConfigId),
    [activeConfigId, allModelRows],
  );
  const searchModelRows = useMemo(
    () =>
      normalizedQuery
        ? allModelRows
            .map((row) => ({ row, score: scoreModelRow(row, normalizedQuery) }))
            .filter((entry) => entry.score > 0)
            .sort((a, b) => {
              if (b.score !== a.score) return b.score - a.score;
              if (a.row.providerRow.label !== b.row.providerRow.label) {
                return a.row.providerRow.label.localeCompare(b.row.providerRow.label);
              }
              return a.row.model.name.localeCompare(b.row.model.name);
            })
            .slice(0, GLOBAL_SEARCH_LIMIT)
            .map((entry) => entry.row)
        : [],
    [allModelRows, normalizedQuery],
  );
  const visibleModelRows = normalizedQuery ? searchModelRows : activeProviderModelRows;

  const activeProviderRow =
    providerRows.find((row) => row.config.id === activeConfigId) ??
    providerRows[0] ??
    null;
  const activeModelRow =
    visibleModelRows.find(
      (row) => row.providerRow.config.id === activeConfigId && row.model.id === activeModelId,
    ) ??
    allModelRows.find(
      (row) => row.providerRow.config.id === activeConfigId && row.model.id === activeModelId,
    ) ??
      activeProviderModelRows[0] ??
      searchModelRows[0] ??
      null;

  const selectedModelRow = selectedConfig
    ? allModelRows.find(
      (row) => row.providerRow.config.id === selectedConfig.id && row.model.id === selectedConfig.model,
    ) ?? null
    : null;
  const selectedTitle = selectedConfig
    ? `${selectedConfig.provider} / ${selectedConfig.model}`
    : t('settings.defaultModel');
  const selectedLabel = selectedModelRow?.model.name || selectedConfig?.model || t('settings.defaultModel');
  const selectedDetail = selectedModelRow?.providerRow.label || selectedConfig?.name?.trim() || selectedConfig?.provider || t('settings.provider');
  const selectedReasoningLabel = selectedConfig?.reasoningEffort
    ? t(REASONING_EFFORT_LABEL_KEYS[selectedConfig.reasoningEffort as ReasoningEffortLevel] ?? 'settings.reasoningEffort')
    : selectedConfig?.thinkingBudget
      ? formatBudget(selectedConfig.thinkingBudget)
      : t('settings.reasoningNone');
  const selectedReasoningTitle = `${t('settings.reasoningEffort')}: ${selectedReasoningLabel}`;

  const panelStepRef = useRef<PickerStep>('providers');
  const panelQueryRef = useRef('');
  panelStepRef.current = pickerStep;
  panelQueryRef.current = normalizedQuery;

  const updatePanelPosition = useCallback(() => {
    const rect = (panelStepRef.current === 'reasoning' ? reasoningTriggerRef.current : triggerRef.current)
      ?.getBoundingClientRect();
    if (!rect) return;
    const availableWidth = Math.max(280, window.innerWidth - 16);
    const currentStep = panelStepRef.current;
    const targetWidth = currentStep === 'reasoning'
      ? 300
      : panelQueryRef.current || currentStep === 'models'
        ? 520
        : 340;
    const width = Math.min(targetWidth, availableWidth);
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    setPanelStyle({
      bottom: window.innerHeight - rect.top + 8,
      left,
      width,
    });
  }, []);

  const closeMenu = useCallback(() => setOpen(false), []);

  useEffect(() => {
    if (!open) return;
    updatePanelPosition();
  }, [normalizedQuery, open, pickerStep, updatePanelPosition]);

  useEffect(() => {
    if (!open) return;
    setActiveConfigId(selectedConfig?.id ?? agentConfigs[0]?.id ?? '');
    setActiveModelId(selectedConfig?.model ?? agentConfigs[0]?.model ?? '');
    setQuery('');
    updatePanelPosition();
    requestAnimationFrame(() => searchRef.current?.focus());
  }, [agentConfigs, open, selectedConfig, updatePanelPosition]);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (triggerRef.current?.contains(event.target as Node)) return;
      if (reasoningTriggerRef.current?.contains(event.target as Node)) return;
      if (panelRef.current?.contains(event.target as Node)) return;
      closeMenu();
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeMenu();
        (panelStepRef.current === 'reasoning' ? reasoningTriggerRef.current : triggerRef.current)?.focus();
      }
    };
    window.addEventListener('resize', updatePanelPosition);
    window.addEventListener('scroll', updatePanelPosition, true);
    document.addEventListener('mousedown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('resize', updatePanelPosition);
      window.removeEventListener('scroll', updatePanelPosition, true);
      document.removeEventListener('mousedown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [closeMenu, open, updatePanelPosition]);

  useEffect(() => {
    if (!open || visibleModelRows.length === 0 || (pickerStep === 'providers' && !normalizedQuery)) return;
    const hasActive = visibleModelRows.some(
      (row) => row.providerRow.config.id === activeConfigId && row.model.id === activeModelId,
    );
    if (!hasActive) {
      const first = visibleModelRows[0];
      setActiveConfigId(first.providerRow.config.id);
      setActiveModelId(first.model.id);
    }
  }, [activeConfigId, activeModelId, normalizedQuery, open, pickerStep, visibleModelRows]);

  const applyModelSelection = useCallback(
    (row: ModelRow) => {
      const isCurrent = selectedConfig?.id === row.providerRow.config.id && selectedConfig.model === row.model.id;
      const defaultEffort = defaultReasoningEffort(row.reasoning);
      const defaultBudget = defaultThinkingBudget(row.reasoning);
      setOpen(false);
      void onSelect({
        config: row.providerRow.config,
        model: row.model.id,
        reasoningEnabled: isCurrent
          ? selectedConfig.reasoningEnabled
          : row.reasoning
            ? true
            : null,
        thinkingBudget: isCurrent
          ? selectedConfig.thinkingBudget
          : defaultEffort
            ? null
            : defaultBudget,
        reasoningEffort: isCurrent ? selectedConfig.reasoningEffort : defaultEffort,
      });
      requestAnimationFrame(() => triggerRef.current?.focus());
    },
    [onSelect, selectedConfig],
  );

  useEffect(() => {
    if (!activeModelRow) return;
    const currentConfig = activeModelRow.providerRow.config;
    setBudgetDraft(String(currentConfig.thinkingBudget ?? defaultThinkingBudget(activeModelRow.reasoning) ?? ''));
  }, [activeModelRow?.key]);

  const applyReasoningSelection = useCallback(
    (reasoning: {
      reasoningEnabled: boolean | null;
      thinkingBudget: number | null;
      reasoningEffort: string | null;
    }) => {
      if (!activeModelRow) return;
      setOpen(false);
      void onSelect({
        config: activeModelRow.providerRow.config,
        model: activeModelRow.model.id,
        ...reasoning,
      });
      requestAnimationFrame(() => reasoningTriggerRef.current?.focus());
    },
    [activeModelRow, onSelect],
  );

  const applyBudget = useCallback(() => {
    const parsed = Number.parseInt(budgetDraft, 10);
    const budget = clampThinkingBudget(
      Number.isFinite(parsed) ? parsed : null,
      activeModelRow?.reasoning ?? null,
    );
    applyReasoningSelection({
      reasoningEnabled: budget !== null ? true : null,
      thinkingBudget: budget,
      reasoningEffort: null,
    });
  }, [activeModelRow?.reasoning, applyReasoningSelection, budgetDraft]);

  const selected = selectedConfig;
  const isSearching = normalizedQuery.length > 0;
  const visibleCount = isSearching
    ? searchModelRows.length
    : pickerStep === 'providers'
      ? providerRows.length
      : activeProviderModelRows.length;

  if (agentConfigs.length === 0 || !selected) {
    return null;
  }

  return (
    <div className="relative inline-flex shrink-0 overflow-hidden rounded-md border border-border/60 bg-surface-1/70">
      <button
        ref={triggerRef}
        type="button"
        data-testid="agent-model-picker-trigger"
        className={`group flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center gap-0 overflow-hidden border-r border-border/60 px-1.5 text-xs font-medium transition-colors duration-fast ease-out hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/20 sm:w-auto sm:max-w-[12rem] sm:justify-start sm:gap-1.5 ${
          open && pickerStep !== 'reasoning' ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:text-text-primary'
        }`}
        aria-haspopup="dialog"
        aria-expanded={open && pickerStep !== 'reasoning'}
        aria-label={t('settings.defaultModel')}
        title={selectedTitle}
        onClick={() => {
          if (open && pickerStep !== 'reasoning') {
            setOpen(false);
            return;
          }
          setPickerStep('providers');
          setOpen(true);
        }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            setPickerStep('providers');
            setOpen(true);
          }
        }}
      >
        <ProviderIcon
          provider={selected.provider}
          baseUrl={selected.baseUrl}
          label={`${selected.name} ${selected.model}`}
          size="sm"
        />
        <span className="hidden min-w-0 sm:flex sm:flex-col sm:items-start">
          <span className="max-w-[9rem] truncate text-xs font-medium leading-4 text-text-secondary group-hover:text-text-primary">
            {selectedLabel}
          </span>
          <span className="max-w-[9rem] truncate text-[10px] leading-3 text-text-tertiary">
            {selectedDetail}
          </span>
        </span>
        <ChevronDown className={`hidden h-3 w-3 shrink-0 text-text-tertiary transition-transform group-hover:text-text-secondary sm:block ${open && pickerStep !== 'reasoning' ? 'rotate-180' : ''}`} />
      </button>

      <button
        ref={reasoningTriggerRef}
        type="button"
        data-testid="agent-reasoning-picker-trigger"
        className={`group flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center gap-0 px-1.5 text-xs font-medium transition-colors duration-fast ease-out hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/20 sm:w-auto sm:max-w-[9rem] sm:justify-start sm:gap-1.5 ${
          open && pickerStep === 'reasoning' ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:text-text-primary'
        }`}
        aria-haspopup="dialog"
        aria-expanded={open && pickerStep === 'reasoning'}
        aria-label={t('settings.reasoningEffort')}
        title={selectedReasoningTitle}
        onClick={() => {
          if (open && pickerStep === 'reasoning') {
            setOpen(false);
            return;
          }
          setActiveConfigId(selected.id);
          setActiveModelId(selected.model);
          setQuery('');
          setPickerStep('reasoning');
          setOpen(true);
        }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            setActiveConfigId(selected.id);
            setActiveModelId(selected.model);
            setQuery('');
            setPickerStep('reasoning');
            setOpen(true);
          }
        }}
      >
        <SlidersHorizontal className="h-3.5 w-3.5 shrink-0 text-accent" />
        <span className="hidden min-w-0 sm:flex sm:flex-col sm:items-start">
          <span className="max-w-[6rem] truncate text-xs font-medium leading-4 text-text-secondary group-hover:text-text-primary">
            {selectedReasoningLabel}
          </span>
          <span className="max-w-[6rem] truncate text-[10px] leading-3 text-text-tertiary">
            {t('settings.reasoningEffort')}
          </span>
        </span>
        <ChevronDown className={`hidden h-3 w-3 shrink-0 text-text-tertiary transition-transform group-hover:text-text-secondary sm:block ${open && pickerStep === 'reasoning' ? 'rotate-180' : ''}`} />
      </button>

      {createPortal(
        <AnimatePresence>
          {open && (
            <motion.div
              ref={panelRef}
              data-testid="agent-model-picker-menu"
              initial={{ opacity: 0, y: 4, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 4, scale: 0.98 }}
              transition={{ duration: 0.14, ease: [0.16, 1, 0.3, 1] }}
              className="fixed z-50 overflow-hidden rounded-lg border border-border/70 bg-surface-0 shadow-2xl shadow-black/25 ring-1 ring-white/[0.04]"
              style={panelStyle}
              role="dialog"
              aria-label={t('settings.defaultModel')}
            >
              {pickerStep !== 'reasoning' && (
                <>
                  <div className="flex items-center gap-2 border-b border-border/60 px-2.5 py-2">
                <div className="relative min-w-0 flex-1">
                  <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text-tertiary" />
                  <input
                    ref={searchRef}
                    type="search"
                    value={query}
                    onChange={(event) => {
                      const nextQuery = event.target.value;
                      setQuery(nextQuery);
                      if (nextQuery.trim()) {
                        setPickerStep('models');
                      }
                    }}
                    placeholder={t('settings.modelSearchPlaceholder')}
                    className="h-7 w-full rounded-md border border-border/60 bg-surface-1 pl-7 pr-2 text-xs text-text-primary outline-none transition-colors placeholder:text-text-tertiary focus:border-accent/60 focus:ring-1 focus:ring-accent/25"
                  />
                </div>
                <span className="shrink-0 rounded-md border border-border/50 bg-surface-1 px-1.5 py-0.5 text-[10px] tabular-nums text-text-tertiary">
                  {visibleCount}
                </span>
                  </div>

                  <div className="border-b border-border/50 px-2.5 py-1.5">
                    <div className="flex min-w-0 items-center gap-1 text-[10px] text-text-tertiary">
                  {(pickerStep !== 'providers' || isSearching) && (
                    <button
                      type="button"
                      onClick={() => {
                        if (isSearching) {
                          setQuery('');
                          setPickerStep('providers');
                          return;
                        }
                        setPickerStep('providers');
                      }}
                      className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-text-tertiary transition-colors hover:bg-surface-1 hover:text-text-primary"
                      aria-label="Back"
                    >
                      <ChevronLeft className="h-3.5 w-3.5" />
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => {
                      setQuery('');
                      setPickerStep('providers');
                    }}
                    className={`rounded px-1.5 py-0.5 transition-colors ${
                      pickerStep === 'providers' && !isSearching
                        ? 'bg-accent-subtle text-accent'
                        : 'hover:bg-surface-1 hover:text-text-secondary'
                    }`}
                  >
                    {t('settings.provider')}
                  </button>
                  <span>/</span>
                  <button
                    type="button"
                    disabled={!activeProviderRow}
                    onClick={() => {
                      setQuery('');
                      setPickerStep('models');
                    }}
                    className={`rounded px-1.5 py-0.5 transition-colors disabled:opacity-45 ${
                      (pickerStep === 'models' || isSearching)
                        ? 'bg-accent-subtle text-accent'
                        : 'hover:bg-surface-1 hover:text-text-secondary'
                    }`}
                  >
                    {t('settings.model')}
                  </button>
                    </div>
                  </div>
                </>
              )}

              <div className="h-[19rem] min-w-0 overflow-hidden">
                {!isSearching && pickerStep === 'providers' && (
                  <div className="h-full overflow-y-auto p-1.5">
                    {providerRows.map((row) => {
                      const active = row.config.id === activeConfigId;
                      return (
                        <button
                          key={row.config.id}
                          type="button"
                          data-testid={`agent-model-provider-${row.config.id}`}
                          onMouseEnter={() => {
                            setActiveConfigId(row.config.id);
                            setActiveModelId(row.config.model);
                          }}
                          onClick={() => {
                            setActiveConfigId(row.config.id);
                            setActiveModelId(row.config.model);
                            setPickerStep('models');
                          }}
                          className={`grid w-full grid-cols-[1.75rem_minmax(0,1fr)_auto] items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors ${
                            active
                              ? 'bg-accent-subtle text-text-primary ring-1 ring-accent/25'
                              : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                          }`}
                        >
                          <ProviderIcon
                            provider={row.config.provider}
                            providerId={row.preset?.id}
                            baseUrl={row.config.baseUrl}
                            label={`${row.label} ${row.config.model}`}
                            size="sm"
                          />
                          <span className="min-w-0">
                            <span className="block truncate text-xs font-medium text-text-primary">
                              {row.label}
                            </span>
                            <span className="block truncate text-[10px] leading-3 text-text-tertiary">
                              {row.config.model}
                            </span>
                          </span>
                          <ChevronDown className="-rotate-90 h-3.5 w-3.5 text-text-tertiary" />
                        </button>
                      );
                    })}
                  </div>
                )}

                {(isSearching || pickerStep === 'models') && (
                  <div className="h-full overflow-y-auto p-1.5">
                    {visibleModelRows.length > 0 ? (
                      visibleModelRows.map((row) => {
                        const active = row.providerRow.config.id === activeConfigId && row.model.id === activeModelId;
                        const current =
                          selectedConfig?.id === row.providerRow.config.id &&
                          selectedConfig.model === row.model.id;
                        const reasoningAvailable = row.reasoning !== null;
                        return (
                          <button
                            key={row.key}
                            type="button"
                            data-testid={`agent-model-option-${row.providerRow.config.id}-${row.model.id}`}
                            onMouseEnter={() => {
                              setActiveConfigId(row.providerRow.config.id);
                              setActiveModelId(row.model.id);
                            }}
                            onClick={() => {
                              setActiveConfigId(row.providerRow.config.id);
                              setActiveModelId(row.model.id);
                              applyModelSelection(row);
                            }}
                            className={`grid w-full grid-cols-[1.75rem_minmax(0,1fr)_auto] items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors ${
                              active
                                ? 'bg-surface-2 text-text-primary ring-1 ring-border/70'
                                : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                            }`}
                          >
                            <ProviderIcon
                              provider={row.providerRow.config.provider}
                              providerId={row.providerRow.preset?.id}
                              baseUrl={row.providerRow.config.baseUrl}
                              label={row.providerRow.label}
                              size="sm"
                            />
                            <span className="min-w-0">
                              <span className="truncate text-xs font-medium text-text-primary">
                                {row.model.name}
                              </span>
                              <span className="mt-0.5 block truncate text-[10px] leading-3 text-text-tertiary">
                                {isSearching ? `${row.providerRow.label} / ${row.model.id}` : row.model.id}
                              </span>
                            </span>
                            <span className="flex items-center gap-1">
                              {reasoningAvailable && <SlidersHorizontal className="h-3 w-3 text-accent" />}
                              {current && <Check className="h-3.5 w-3.5 text-accent" />}
                            </span>
                          </button>
                        );
                      })
                    ) : (
                      <div className="px-3 py-8 text-center text-xs text-text-tertiary">
                        {t('settings.modelSearchNoResults')}
                      </div>
                    )}
                  </div>
                )}

                {!isSearching && pickerStep === 'reasoning' && (
                  <div className="flex h-full min-w-0 flex-col p-2">
                    {activeModelRow ? (
                      <>
                        <div className="grid grid-cols-[1.75rem_minmax(0,1fr)] items-center gap-2 border-b border-border/50 pb-2">
                          <ProviderIcon
                            provider={activeModelRow.providerRow.config.provider}
                            providerId={activeModelRow.providerRow.preset?.id}
                            baseUrl={activeModelRow.providerRow.config.baseUrl}
                            label={activeModelRow.providerRow.label}
                            size="sm"
                          />
                          <div className="min-w-0">
                            <div className="truncate text-xs font-medium text-text-primary">
                              {activeModelRow.model.name}
                            </div>
                            <div className="mt-0.5 truncate text-[10px] text-text-tertiary">
                              {activeModelRow.providerRow.label} / {activeModelRow.model.id}
                            </div>
                          </div>
                        </div>

                        <div className="min-h-0 flex-1 overflow-y-auto py-2">
                          {!activeModelRow.reasoning ? (
                            <p className="text-[11px] leading-4 text-text-tertiary">
                              {t('settings.reasoningUnsupported')}
                            </p>
                          ) : activeModelRow.reasoning.effortLevels?.length ? (
                            <div className="grid gap-1">
                              {!activeModelRow.reasoning.effortLevels.includes('none') && (
                                <button
                                  type="button"
                                  data-testid="agent-model-reasoning-none"
                                  onClick={() =>
                                    applyReasoningSelection({
                                      reasoningEnabled: null,
                                      thinkingBudget: null,
                                      reasoningEffort: null,
                                    })
                                  }
                                  className={`flex h-7 items-center justify-between rounded-md px-2 text-xs transition-colors ${
                                    selectedConfig?.id === activeModelRow.providerRow.config.id &&
                                    selectedConfig.model === activeModelRow.model.id &&
                                    !selectedConfig.reasoningEffort
                                      ? 'bg-accent-subtle text-text-primary ring-1 ring-accent/25'
                                      : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                                  }`}
                                >
                                  <span className="truncate">{t('settings.reasoningNone')}</span>
                                </button>
                              )}
                              {activeModelRow.reasoning.effortLevels.map((level) => {
                                const current =
                                  selectedConfig?.id === activeModelRow.providerRow.config.id &&
                                  selectedConfig.model === activeModelRow.model.id &&
                                  selectedConfig.reasoningEffort === level;
                                return (
                                  <button
                                    key={level}
                                    type="button"
                                    data-testid={`agent-model-reasoning-${level}`}
                                    onClick={() =>
                                      applyReasoningSelection({
                                        reasoningEnabled: true,
                                        thinkingBudget: null,
                                        reasoningEffort: level,
                                      })
                                    }
                                    className={`flex h-7 items-center justify-between rounded-md px-2 text-xs transition-colors ${
                                      current
                                        ? 'bg-accent-subtle text-text-primary ring-1 ring-accent/25'
                                        : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                                    }`}
                                  >
                                    <span className="truncate">{t(REASONING_EFFORT_LABEL_KEYS[level])}</span>
                                    {current && <Check className="h-3.5 w-3.5 text-accent" />}
                                  </button>
                                );
                              })}
                            </div>
                          ) : activeModelRow.reasoning.thinkingBudget?.enabled ? (
                            <div className="space-y-2">
                              <div className="grid grid-cols-2 gap-1">
                                {thinkingBudgetOptions(activeModelRow.reasoning).map((budget) => {
                                  const current =
                                    selectedConfig?.id === activeModelRow.providerRow.config.id &&
                                    selectedConfig.model === activeModelRow.model.id &&
                                    selectedConfig.thinkingBudget === budget;
                                  return (
                                    <button
                                      key={budget}
                                      type="button"
                                      onClick={() => {
                                        applyReasoningSelection({
                                          reasoningEnabled: true,
                                          thinkingBudget: budget,
                                          reasoningEffort: null,
                                        });
                                      }}
                                      className={`h-7 rounded-md px-2 text-xs transition-colors ${
                                        current
                                          ? 'bg-accent-subtle text-text-primary ring-1 ring-accent/25'
                                          : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                                      }`}
                                    >
                                      {formatBudget(budget)}
                                    </button>
                                  );
                                })}
                              </div>
                              <input
                                type="number"
                                value={budgetDraft}
                                min={activeModelRow.reasoning.thinkingBudget.allowZero ? 0 : activeModelRow.reasoning.thinkingBudget.minTokens ?? 1}
                                max={activeModelRow.reasoning.thinkingBudget.maxTokens}
                                step={activeModelRow.reasoning.thinkingBudget.step ?? 1}
                                onChange={(event) => setBudgetDraft(event.target.value)}
                                onKeyDown={(event) => {
                                  if (event.key === 'Enter') {
                                    event.preventDefault();
                                    applyBudget();
                                  }
                                }}
                                className="h-7 w-full rounded-md border border-border/60 bg-surface-1 px-2 text-xs text-text-primary outline-none focus:border-accent/60 focus:ring-1 focus:ring-accent/25"
                              />
                            </div>
                          ) : (
                            <p className="text-[11px] leading-4 text-text-tertiary">
                              {t('settings.reasoningUnsupported')}
                            </p>
                          )}
                        </div>

                        {activeModelRow.reasoning?.thinkingBudget?.enabled && (
                          <div className="flex gap-1 border-t border-border/50 pt-2">
                            <button
                              type="button"
                              data-testid="agent-model-apply"
                              onClick={applyBudget}
                              className="h-7 flex-1 rounded-md bg-accent px-2 text-xs font-medium text-on-accent transition-colors hover:bg-accent-hover"
                            >
                              {t('common.save')}
                            </button>
                            <button
                              type="button"
                              data-testid="agent-model-reasoning-none"
                              onClick={() =>
                                applyReasoningSelection({
                                  reasoningEnabled: null,
                                  thinkingBudget: null,
                                  reasoningEffort: null,
                                })
                              }
                              className="h-7 rounded-md border border-border/60 px-2 text-xs text-text-secondary transition-colors hover:bg-surface-1 hover:text-text-primary"
                            >
                              {t('settings.reasoningNone')}
                            </button>
                          </div>
                        )}
                      </>
                    ) : (
                      <div className="px-2 py-8 text-center text-xs text-text-tertiary">
                        {t('settings.modelSearchNoResults')}
                      </div>
                    )}
                  </div>
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>,
        overlayRoot ?? document.body,
      )}
    </div>
  );
}
