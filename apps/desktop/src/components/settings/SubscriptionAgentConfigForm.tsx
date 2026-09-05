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
  const [effort, setEffort] = useState(config?.reasoningEffort ?? '');
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
  const save = async () => {
    if (!current || !name.trim()) return;
    setError(null);
    try { await onSave({
      id: config?.id ?? null, name: name.trim(), provider: preset.provider, apiKey: '', baseUrl: null, model,
      modelId: model, providerEndpointId: null, temperature: null, maxTokens: null, contextWindow: null,
      isDefault: config?.isDefault ?? false, reasoningEnabled: null, thinkingBudget: null,
      reasoningEffort: current.reasoningEfforts.includes(effort) ? effort : null,
      maxIterations: config?.maxIterations ?? null, summarizationModel: null, summarizationProvider: null,
      imageGenerationModel: null, subagentAllowedTools: config?.subagentAllowedTools ?? null,
    }); } catch (error) { setError(String(error)); }
  };
  return <div className="space-y-4" data-testid="subscription-agent-form">
    <SubscriptionAccountsPanel runtime={preset.runtime!} />
    <label className="block text-sm text-text-secondary">{t('settings.providerName')}
      <input className="mt-1 w-full rounded-md border border-border bg-surface-2 p-2 text-text-primary" value={name}
        onChange={event => { setName(event.target.value); onDirtyChange(true); }} />
    </label>
    <label className="block text-sm text-text-secondary">{t('settings.model')}
      <select data-testid="subscription-model-select" className="mt-1 w-full rounded-md border border-border bg-surface-2 p-2 text-text-primary" value={model}
        onChange={event => { setModel(event.target.value); setEffort(''); onDirtyChange(true); }}>
        {!current && <option value={model} disabled>{loading ? t('common.loading') : t('settings.subscriptionSelectModel')}</option>}
        {models.map(model => <option key={model.id} value={model.id}>{model.name}</option>)}
      </select>
    </label>
    {!!current?.reasoningEfforts.length && <select aria-label={t('settings.reasoningEffort')} className="rounded-md border border-border bg-surface-2 p-2 text-text-primary" value={effort}
      onChange={event => { setEffort(event.target.value); onDirtyChange(true); }}>
      <option value="">{t('settings.isDefault')}</option>
      {current.reasoningEfforts.map(value => <option key={value} value={value}>{value}</option>)}
    </select>}
    {error && <p role="alert" className="text-sm text-danger">{error}</p>}
    <div className="flex gap-2">
      <Button variant="secondary" loading={loading} onClick={() => void refresh()}>{t('settings.subscriptionRefresh')}</Button>
      <Button loading={isSaving} disabled={!current || !name.trim() || loading} onClick={() => void save()}>{t('common.save')}</Button>
      <Button variant="secondary" onClick={onCancel}>{t('common.cancel')}</Button>
    </div>
  </div>;
}
