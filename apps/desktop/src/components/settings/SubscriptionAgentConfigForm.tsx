import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { ProviderPreset } from '../../lib/providerPresets';
import type { AgentConfig, SaveAgentConfigInput } from '../../types/conversation';
import { SubscriptionAccountsPanel } from './SubscriptionAccountsPanel';
import { Button } from '../ui/Button';

export function SubscriptionAgentConfigForm({ preset, config, onSave, onCancel, isSaving, onDirtyChange }: {
  preset: ProviderPreset; config?: AgentConfig; onSave: (input: SaveAgentConfigInput) => Promise<void>;
  onCancel: () => void; isSaving: boolean; onDirtyChange: (dirty: boolean) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(config?.name ?? preset.name);
  const [model, setModel] = useState(config?.model ?? '');
  const [models, setModels] = useState<api.CopilotModelSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    setLoading(true); setError(null); setModels([]);
    try {
      const next = await api.listSubscriptionModels(preset.provider);
      if (generation.current !== request) return;
      setModels(next);
      setModel(previous => previous || next[0]?.id || '');
    } catch (error) { if (generation.current === request) setError(String(error)); }
    finally { if (generation.current === request) setLoading(false); }
  }, [preset.provider]);
  useEffect(() => { void refresh(); return () => { generation.current += 1; }; }, [refresh]);
  const current = models.find(item => item.id === model);
  // Renaming an enrolled account does not select a model or start inference.
  // Retain its exact selection even if the catalog is unavailable or retired it.
  const preservesSavedSelection = config?.provider === preset.provider
    && config?.model === model && !!model.trim();
  const canSave = !!name.trim() && !isSaving
    && (preservesSavedSelection || (!!current && !loading));
  const save = async () => {
    if (!canSave) return;
    setError(null);
    try { await onSave({
      id: config?.id ?? null, name: name.trim(), provider: preset.provider, apiKey: '', baseUrl: null, model,
      modelId: model, providerEndpointId: null, temperature: null, maxTokens: null, contextWindow: null,
      isDefault: config?.isDefault ?? false,
      reasoningEnabled: preservesSavedSelection ? config?.reasoningEnabled ?? null : null,
      thinkingBudget: preservesSavedSelection ? config?.thinkingBudget ?? null : null,
      reasoningEffort: preservesSavedSelection ? config?.reasoningEffort ?? null : null,
      maxIterations: config?.maxIterations ?? null, summarizationModel: null, summarizationProvider: null,
      imageGenerationModel: null, subagentAllowedTools: config?.subagentAllowedTools ?? null,
      subagentAllowedSkillIds: config?.subagentAllowedSkillIds,
      subagentMaxParallel: config?.subagentMaxParallel,
      subagentMaxCallsPerTurn: config?.subagentMaxCallsPerTurn,
      subagentTokenBudget: config?.subagentTokenBudget,
      delegationLimitsV2: config?.delegationLimitsV2,
      providerStreaming: config?.providerStreaming,
    }); } catch (error) { setError(String(error)); }
  };
  return <div className="space-y-4" data-testid="subscription-agent-form">
    <SubscriptionAccountsPanel runtime={preset.runtime!} />
    <label className="block text-sm text-text-secondary">{t('settings.providerName')}
      <input className="mt-1 w-full rounded-md border border-border bg-surface-2 p-2 text-text-primary" value={name}
        onChange={event => { setName(event.target.value); onDirtyChange(true); }} />
    </label>
    <p className="text-sm text-text-secondary">{t('settings.subscriptionModelsInChat')}</p>
    {error && <p role="alert" className="text-sm text-danger">{error}</p>}
    <div className="flex gap-2">
      <Button variant="secondary" loading={loading} onClick={() => void refresh()}>{t('settings.subscriptionRefresh')}</Button>
      <Button loading={isSaving} disabled={!canSave} onClick={() => void save()}>{t('common.save')}</Button>
      <Button variant="secondary" onClick={onCancel}>{t('common.cancel')}</Button>
    </div>
  </div>;
}
