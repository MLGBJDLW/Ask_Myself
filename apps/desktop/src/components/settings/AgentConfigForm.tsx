import { useState, useEffect, useCallback, useRef } from 'react';
import {
  Eye,
  EyeOff,
  Loader2,
  Zap,
  Save,
  X,
  CheckCircle,
  ChevronDown,
  BrainCircuit,
} from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { useTranslation, type TranslationKey } from '../../i18n';
import * as api from '../../lib/api';
import type { AgentConfig, SaveAgentConfigInput, ProviderType } from '../../types/conversation';
import { findProviderPreset, type ProviderPreset } from '../../lib/providerPresets';
import {
  DEFAULT_SUBAGENT_TOOL_NAMES,
  SUBAGENT_TOOL_CATALOG,
  normalizeSubagentToolSelection,
  usesDefaultSubagentToolSelection,
} from '../../lib/subagentTools';

interface AgentConfigFormProps {
  config?: AgentConfig;
  preset?: ProviderPreset | null;
  onSave: (input: SaveAgentConfigInput) => Promise<void>;
  onCancel: () => void;
  isSaving: boolean;
  onDirtyChange?: (dirty: boolean) => void;
}

const PROVIDER_LABEL_KEYS: { value: ProviderType; labelKey: string }[] = [
  { value: 'open_ai', labelKey: 'settings.providerOpenAI' },
  { value: 'anthropic', labelKey: 'settings.providerAnthropic' },
  { value: 'google', labelKey: 'settings.providerGoogleGemini' },
  { value: 'deep_seek', labelKey: 'settings.providerDeepSeek' },
  { value: 'ollama', labelKey: 'settings.providerOllama' },
  { value: 'lm_studio', labelKey: 'settings.providerLMStudio' },
  { value: 'azure_open_ai', labelKey: 'settings.providerAzureOpenAI' },
  { value: 'custom', labelKey: 'settings.providerCustom' },
];

const BASE_URL_PLACEHOLDERS: Record<ProviderType, string> = {
  open_ai: 'https://api.openai.com/v1',
  anthropic: 'https://api.anthropic.com/v1',
  google: 'https://generativelanguage.googleapis.com/v1beta',
  deep_seek: 'https://api.deepseek.com/v1',
  ollama: 'http://localhost:11434',
  lm_studio: 'http://localhost:1234/v1',
  azure_open_ai: 'https://{resource}.openai.azure.com',
  custom: 'https://...',
};

const LOCAL_PROVIDERS: ProviderType[] = ['ollama', 'lm_studio'];

export function AgentConfigForm({ config, preset, onSave, onCancel, isSaving, onDirtyChange }: AgentConfigFormProps) {
  const { t } = useTranslation();

  const initialProvider = (config?.provider as ProviderType) ?? (preset?.provider as ProviderType) ?? 'open_ai';
  const initialBaseUrl = config?.baseUrl ?? preset?.baseUrl ?? '';
  const initialPreset = preset ?? findProviderPreset({ provider: initialProvider, baseUrl: initialBaseUrl });
  const presetDefaultModel = initialPreset?.models.find(m => m.recommended)?.id || initialPreset?.models[0]?.id || '';
  const initialIsLocal = LOCAL_PROVIDERS.includes(initialProvider) || (initialPreset ? !initialPreset.requiresApiKey : false);
  const initialModel = config?.model ?? presetDefaultModel;
  const initialUsesCustomModel = !!config && !!initialPreset && !initialPreset.models.some(m => m.id === initialModel);

  const [name, setName] = useState(config?.name ?? preset?.name ?? '');
  const [provider, setProvider] = useState<ProviderType>(initialProvider);
  const [apiKey, setApiKey] = useState(config?.apiKey ?? '');
  const [baseUrl, setBaseUrl] = useState(initialBaseUrl);
  const [model, setModel] = useState(initialModel);
  const [temperature, setTemperature] = useState(config?.temperature ?? 0.3);
  const [maxTokens, setMaxTokens] = useState(config?.maxTokens ?? 4096);
  const [contextWindow, setContextWindow] = useState<number | null>(config?.contextWindow ?? null);
  const [isDefault, setIsDefault] = useState(config?.isDefault ?? false);
  const [reasoningEnabled, setReasoningEnabled] = useState<boolean | null>(config?.reasoningEnabled ?? null);
  const [thinkingBudget, setThinkingBudget] = useState<number | null>(config?.thinkingBudget ?? null);
  const [reasoningEffort, setReasoningEffort] = useState<string | null>(config?.reasoningEffort ?? null);
  const [maxIterations, setMaxIterations] = useState<number | null>(config?.maxIterations ?? null);
  const [summarizationModel, setSummarizationModel] = useState<string | null>(config?.summarizationModel ?? null);
  const [summarizationProvider, setSummarizationProvider] = useState<string | null>(config?.summarizationProvider ?? null);
  const [subagentAllowedTools, setSubagentAllowedTools] = useState<string[]>(
    normalizeSubagentToolSelection(config?.subagentAllowedTools),
  );
  const [subagentMaxParallel, setSubagentMaxParallel] = useState<number | null>(config?.subagentMaxParallel ?? 3);
  const [subagentMaxCallsPerTurn, setSubagentMaxCallsPerTurn] = useState<number | null>(config?.subagentMaxCallsPerTurn ?? 6);
  const [subagentTokenBudget, setSubagentTokenBudget] = useState<number | null>(config?.subagentTokenBudget ?? 12000);
  const [showKey, setShowKey] = useState(false);
  const [testLoading, setTestLoading] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [useCustomModel, setUseCustomModel] = useState(initialUsesCustomModel);
  const [showAdvanced, setShowAdvanced] = useState(!!config);
  const initialDraftRef = useRef<SaveAgentConfigInput>({
    id: config?.id ?? null,
    name: config?.name ?? preset?.name ?? '',
    provider: initialProvider,
    apiKey: initialIsLocal ? '' : (config?.apiKey ?? ''),
    baseUrl: initialBaseUrl || null,
    model: initialModel,
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
    subagentAllowedTools: usesDefaultSubagentToolSelection(config?.subagentAllowedTools)
      ? null
      : (config?.subagentAllowedTools ?? null),
    subagentMaxParallel: config?.subagentMaxParallel ?? 3,
    subagentMaxCallsPerTurn: config?.subagentMaxCallsPerTurn ?? 6,
    subagentTokenBudget: config?.subagentTokenBudget ?? 12000,
  });

  const isLocal = LOCAL_PROVIDERS.includes(provider) || (preset ? !preset.requiresApiKey : false);
  const activePreset = findProviderPreset({ provider, baseUrl }) ?? (preset?.provider === provider ? preset : null);
  const activePresetDefaultModel = activePreset?.models.find(m => m.recommended)?.id || activePreset?.models[0]?.id || '';

  // Reset test result when provider changes
  useEffect(() => {
    setTestResult(null);
  }, [provider]);

  useEffect(() => {
    if (useCustomModel || !activePreset || !activePresetDefaultModel) {
      return;
    }
    if (!activePreset.models.some(m => m.id === model)) {
      setModel(activePresetDefaultModel);
    }
  }, [activePreset, activePresetDefaultModel, model, useCustomModel]);

  const buildInput = useCallback((): SaveAgentConfigInput => ({
    id: config?.id ?? null,
    name: name.trim(),
    provider,
    apiKey: isLocal ? '' : apiKey,
    baseUrl: baseUrl.trim() || null,
    model: model.trim(),
    temperature,
    maxTokens,
    contextWindow: contextWindow,
    isDefault,
    reasoningEnabled,
    thinkingBudget,
    reasoningEffort,
    maxIterations,
    summarizationModel: summarizationModel?.trim() || null,
    summarizationProvider: summarizationProvider || null,
    subagentAllowedTools: usesDefaultSubagentToolSelection(subagentAllowedTools)
      ? null
      : subagentAllowedTools,
    subagentMaxParallel,
    subagentMaxCallsPerTurn,
    subagentTokenBudget,
  }), [config?.id, name, provider, apiKey, baseUrl, model, temperature, maxTokens, contextWindow, isDefault, reasoningEnabled, thinkingBudget, reasoningEffort, maxIterations, summarizationModel, summarizationProvider, subagentAllowedTools, subagentMaxParallel, subagentMaxCallsPerTurn, subagentTokenBudget, isLocal]);

  useEffect(() => {
    if (!onDirtyChange) return;

    const dirty = JSON.stringify(buildInput()) !== JSON.stringify(initialDraftRef.current);
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
      const models = await api.testAgentConnection(buildInput());
      setTestResult({
        ok: true,
        message: t('settings.modelsFound').replace('{count}', String(models.length)),
      });
      toast.success(t('settings.connectionSuccess'));
    } catch (err) {
      const msg = err instanceof Error ? err.message : t('settings.connectionFailed');
      setTestResult({ ok: false, message: msg });
      toast.error(t('settings.connectionFailed'));
    } finally {
      setTestLoading(false);
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
        <label className="text-sm font-medium text-text-primary">{t('settings.providerName')}</label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t('settings.providerNamePlaceholder')}
        />
      </div>

      {/* Provider Type */}
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.providerType')}</label>
        <select
          value={provider}
          onChange={(e) => setProvider(e.target.value as ProviderType)}
          className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
        >
          {PROVIDER_LABEL_KEYS.map((opt) => (
            <option key={opt.value} value={opt.value}>{t(opt.labelKey as any)}</option>
          ))}
        </select>
      </div>

      {/* API Key — hidden for local providers */}
      {!isLocal && (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.apiKey')}</label>
          <div className="relative">
            <Input
              type={showKey ? 'text' : 'password'}
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..."
              className="pr-10"
            />
            <button
              type="button"
              onClick={() => setShowKey(!showKey)}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer"
              aria-label={showKey ? t('settings.hideKey') : t('settings.showKey')}
            >
              {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
            </button>
          </div>
        </div>
      )}

      {/* Base URL */}
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.baseUrl')}</label>
        <Input
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder={BASE_URL_PLACEHOLDERS[provider]}
        />
      </div>

      {/* Model */}
      {activePreset && activePreset.models.length > 0 && !useCustomModel ? (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.defaultModel')}</label>
          <select
            value={model}
            onChange={e => setModel(e.target.value)}
            className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
          >
            {activePreset.models.map(m => (
              <option key={m.id} value={m.id}>
                {m.tagKey ? `${m.name} (${t(m.tagKey as TranslationKey)})` : m.name}{m.recommended ? ' ★' : ''}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => setUseCustomModel(true)}
            className="text-xs text-text-tertiary hover:text-accent transition-colors cursor-pointer"
          >
            {t('settings.useCustomModel')}
          </button>
        </div>
      ) : (
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.defaultModel')}</label>
          <Input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={provider === 'open_ai' ? 'gpt-5.4' : provider === 'anthropic' ? 'claude-sonnet-4-6' : provider === 'google' ? 'gemini-2.5-pro' : provider === 'deep_seek' ? 'deepseek-chat' : provider === 'ollama' ? 'llama3.1' : provider === 'lm_studio' ? 'local-model' : 'model-name'}
          />
          {activePreset && activePreset.models.length > 0 && (
            <button
              type="button"
              onClick={() => { setUseCustomModel(false); setModel(activePresetDefaultModel); }}
              className="text-xs text-text-tertiary hover:text-accent transition-colors cursor-pointer"
            >
              {t('settings.usePresetModels')}
            </button>
          )}
        </div>
      )}

      {/* Advanced Settings Toggle */}
      <button
        type="button"
        onClick={() => setShowAdvanced(!showAdvanced)}
        className="flex items-center gap-1 text-sm text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer"
      >
        <ChevronDown size={14} className={`transition-transform ${showAdvanced ? 'rotate-180' : ''}`} />
        {t('settings.advancedSettings')}
      </button>

      {showAdvanced && (
      <div className="space-y-4 rounded-lg border border-border bg-surface-2 p-4">
      {/* Temperature + Max Tokens — side by side */}
      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.temperature')}</label>
          <Input
            type="number"
            value={temperature}
            onChange={(e) => setTemperature(parseFloat(e.target.value) || 0)}
            min={0}
            max={2}
            step={0.1}
          />
        </div>
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.maxTokens')}</label>
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
        <label className="text-sm font-medium text-text-primary">{t('settings.contextWindow')}</label>
        <Input
          type="number"
          value={contextWindow ?? ''}
          onChange={(e) => {
            const val = e.target.value.trim();
            setContextWindow(val ? parseInt(val) || null : null);
          }}
          placeholder={t('settings.contextWindowPlaceholder')}
          min={1024}
          step={1024}
        />
        <p className="text-xs text-text-tertiary">
          {t('settings.contextWindowHelp')}
        </p>
      </div>
      </div>
      )}

      {/* Reasoning / Thinking */}
      <div className="space-y-3">
        <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
          <BrainCircuit size={16} className="text-accent" />
          {t('settings.reasoningSection')}
        </div>

        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={reasoningEnabled === true}
            onChange={(e) => {
              const enabled = e.target.checked;
              setReasoningEnabled(enabled ? true : null);
              if (enabled && !thinkingBudget) {
                setThinkingBudget(10000);
              } else if (!enabled) {
                setThinkingBudget(null);
              }
            }}
            className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
          />
          <span className="text-sm text-text-primary">{t('settings.enableReasoning')}</span>
        </label>

        {reasoningEnabled && (
          <div className="space-y-4 rounded-lg border border-border bg-surface-2 p-4 ml-1">
            {/* Thinking Budget */}
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.thinkingBudget')}</label>
              <Input
                type="number"
                value={thinkingBudget ?? ''}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  setThinkingBudget(val ? parseInt(val) || null : null);
                }}
                placeholder="10000"
                min={1}
                step={1}
              />
              <p className="text-xs text-text-tertiary">
                {t('settings.thinkingBudgetHelp')}
              </p>
            </div>

            {/* Reasoning Effort */}
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.reasoningEffort')}</label>
              <select
                value={reasoningEffort ?? 'medium'}
                onChange={(e) => setReasoningEffort(e.target.value)}
                className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
              >
                <option value="low">{t('settings.reasoningLow')}</option>
                <option value="medium">{t('settings.reasoningMedium')}</option>
                <option value="high">{t('settings.reasoningHigh')}</option>
              </select>
              <p className="text-xs text-text-tertiary">
                {t('settings.reasoningEffortHelp')}
              </p>
            </div>
          </div>
        )}
      </div>

      {/* Max Tool Iterations */}
      {showAdvanced && (
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.maxIterations')}</label>
        <Input
          type="number"
          value={maxIterations ?? ''}
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
          {t('settings.maxIterationsHelp')}
        </p>
      </div>
      )}

      {/* Summarization Model (cost optimization) */}
      {showAdvanced && (
      <div className="space-y-3 border-t border-border pt-4">
        <h4 className="text-sm font-semibold text-text-primary">{t('settings.summarizationSection')}</h4>
        <p className="text-xs text-text-tertiary">
          {t('settings.summarizationHelp')}
        </p>
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.summarizationModel')}</label>
          <Input
            value={summarizationModel ?? ''}
            onChange={(e) => setSummarizationModel(e.target.value || null)}
            placeholder={t('settings.summarizationModelPlaceholder')}
          />
        </div>
        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">{t('settings.summarizationProvider')}</label>
          <select
            value={summarizationProvider ?? ''}
            onChange={(e) => setSummarizationProvider(e.target.value || null)}
            className="w-full h-10 bg-surface-1 border border-border rounded-md text-sm text-text-primary px-3.5 transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none cursor-pointer"
          >
            <option value="">{t('settings.sameAsMain')}</option>
            {PROVIDER_LABEL_KEYS.map((opt) => (
              <option key={opt.value} value={opt.value}>{t(opt.labelKey as any)}</option>
            ))}
          </select>
          <p className="text-xs text-text-tertiary">
            {t('settings.summarizationProviderHelp')}
          </p>
        </div>
      </div>
      )}

      {showAdvanced && (
      <div className="space-y-3 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h4 className="text-sm font-semibold text-text-primary">Subagents</h4>
            <p className="text-xs text-text-tertiary">
              Choose which built-in tools delegated subagents may use, and set concurrency and budget limits for delegated workers and adjudicators.
            </p>
          </div>
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {subagentAllowedTools.length}/{DEFAULT_SUBAGENT_TOOL_NAMES.length} enabled
          </span>
        </div>

        <div className="grid gap-4 md:grid-cols-3">
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Max parallel workers</label>
            <Input
              type="number"
              value={subagentMaxParallel ?? ''}
              onChange={(e) => {
                const val = e.target.value.trim();
                setSubagentMaxParallel(val ? parseInt(val) || null : null);
              }}
              min={1}
              max={12}
              step={1}
            />
            <p className="text-xs text-text-tertiary">
              Hard cap on how many delegated workers may run at the same time.
            </p>
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Max worker calls / turn</label>
            <Input
              type="number"
              value={subagentMaxCallsPerTurn ?? ''}
              onChange={(e) => {
                const val = e.target.value.trim();
                setSubagentMaxCallsPerTurn(val ? parseInt(val) || null : null);
              }}
              min={1}
              max={32}
              step={1}
            />
            <p className="text-xs text-text-tertiary">
              Limits total delegated worker and judge invocations in one parent turn.
            </p>
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Token budget / turn</label>
            <Input
              type="number"
              value={subagentTokenBudget ?? ''}
              onChange={(e) => {
                const val = e.target.value.trim();
                setSubagentTokenBudget(val ? parseInt(val) || null : null);
              }}
              min={256}
              step={256}
            />
            <p className="text-xs text-text-tertiary">
              Soft total token budget for delegated workers and result adjudication.
            </p>
          </div>
        </div>

        <div className="grid gap-2 md:grid-cols-2">
          {SUBAGENT_TOOL_CATALOG.map((tool) => {
            const checked = subagentAllowedTools.includes(tool.name);
            return (
              <label
                key={tool.name}
                className={`flex cursor-pointer items-start gap-3 rounded-xl border px-3 py-3 transition-colors ${
                  checked
                    ? 'border-accent/35 bg-accent/8'
                    : 'border-border/70 bg-surface-2 hover:border-border-hover'
                }`}
              >
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(event) => {
                    setSubagentAllowedTools(prev => {
                      if (event.target.checked) {
                        return normalizeSubagentToolSelection([...prev, tool.name]);
                      }
                      return prev.filter(name => name !== tool.name);
                    });
                  }}
                  className="mt-0.5 h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
                />
                <span className="min-w-0">
                  <span className="block text-sm font-medium text-text-primary">{tool.label}</span>
                  <span className="mt-1 block text-xs text-text-tertiary">{tool.description}</span>
                  <span className="mt-1 block font-mono text-[11px] text-text-tertiary">{tool.name}</span>
                </span>
              </label>
            );
          })}
        </div>
      </div>
      )}

      {/* Set as Default */}
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={isDefault}
          onChange={(e) => setIsDefault(e.target.checked)}
          className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
        />
        <span className="text-sm text-text-primary">{t('settings.setDefault')}</span>
      </label>

      {/* Test Connection */}
      <div className="space-y-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          icon={testLoading ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
          loading={testLoading}
          onClick={handleTest}
          disabled={!model.trim() || (!isLocal && !apiKey.trim())}
        >
          {testLoading ? t('settings.testing') : t('settings.testConnection')}
        </Button>
        {testResult && (
          <div className={`flex items-center gap-2 text-xs ${testResult.ok ? 'text-success' : 'text-danger'}`}>
            {testResult.ok ? <CheckCircle size={12} /> : <X size={12} />}
            <span>{testResult.message}</span>
          </div>
        )}
      </div>

      {/* Actions */}
      <div className="flex items-center justify-end gap-3 border-t border-border pt-4">
        <Button type="button" variant="ghost" size="md" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <Button
          type="submit"
          variant="primary"
          size="md"
          icon={<Save size={16} />}
          loading={isSaving}
          disabled={!canSubmit}
        >
          {t('common.save')}
        </Button>
      </div>
    </form>
  );
}
