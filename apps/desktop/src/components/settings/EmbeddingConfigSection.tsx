import { AlertTriangle, Brain, CheckCircle, KeyRound, Loader2, RefreshCw, Save, XCircle, Zap } from 'lucide-react';
import { useTranslation } from '../../i18n';
import {
  defaultEmbeddingModel,
  EMBEDDING_PROVIDER_PRESETS,
  findEmbeddingProviderPreset,
} from '../../lib/embeddingProviderPresets';
import { ProviderIcon } from '../../lib/providerIcons';
import type { EmbedderConfig } from '../../types/embedder';
import type { ScanProgress } from '../../types/ingest';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Section } from './SettingsSection';

interface EmbeddingConfigSectionProps {
  embedConfig: EmbedderConfig | null;
  localModelReady: boolean | null;
  testLoading: boolean;
  embedSaveLoading: boolean;
  rebuildEmbedLoading: boolean;
  embedRebuildProgress: ScanProgress | null;
  onConfigChange: (config: EmbedderConfig) => void;
  onMarkDirty: () => void;
  onTestConnection: () => void;
  onSave: () => void;
  onRebuild: () => void;
}

export function EmbeddingConfigSection({
  embedConfig,
  localModelReady,
  testLoading,
  embedSaveLoading,
  rebuildEmbedLoading,
  embedRebuildProgress,
  onConfigChange,
  onMarkDirty,
  onTestConnection,
  onSave,
  onRebuild,
}: EmbeddingConfigSectionProps) {
  const { t } = useTranslation();

  const updateConfig = (patch: Partial<EmbedderConfig>) => {
    if (!embedConfig) return;
    onConfigChange({ ...embedConfig, ...patch });
    onMarkDirty();
  };

  const activeApiPreset = embedConfig
    ? findEmbeddingProviderPreset(embedConfig.apiBaseUrl)
    : null;

  const applyApiPreset = (presetId: string) => {
    const preset = EMBEDDING_PROVIDER_PRESETS.find((candidate) => candidate.id === presetId);
    if (!preset || !embedConfig) return;
    const model = defaultEmbeddingModel(preset);
    updateConfig({
      apiBaseUrl: preset.baseUrl,
      apiModel: model?.id ?? '',
      vectorDimensions: model?.dimensions ?? embedConfig.vectorDimensions,
    });
  };

  const applyApiModel = (modelId: string) => {
    const model = activeApiPreset?.models.find((candidate) => candidate.id === modelId);
    updateConfig({
      apiModel: modelId,
      ...(model ? { vectorDimensions: model.dimensions } : {}),
    });
  };

  return (
    <Section
      icon={<Brain size={20} />}
      title={t('settings.embeddingSection')}
      delay={0.06}
      collapsible
      defaultOpen={false}
      summary={embedConfig ? (
        <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
          {embedConfig.provider}
        </span>
      ) : undefined}
    >
      {embedConfig && (
        <div className="space-y-5">
          {/* Provider pills */}
          <div>
            <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.embeddingProvider')}</p>
            <div className="inline-flex rounded-full border border-border bg-surface-1 p-0.5">
              {(['local', 'api', 'tfidf'] as const).map((provider) => (
                <button
                  key={provider}
                  onClick={() => updateConfig({ provider })}
                  className={`rounded-full px-4 py-1.5 text-xs font-medium transition-all duration-fast cursor-pointer ${
                    embedConfig.provider === provider
                      ? 'bg-accent text-white shadow-sm'
                      : 'text-text-tertiary hover:text-text-secondary'
                  }`}
                >
                  {provider === 'local'
                    ? t('settings.embeddingLocal')
                    : provider === 'api'
                      ? t('settings.embeddingApi')
                      : t('settings.embeddingTfidf')}
                </button>
              ))}
            </div>
          </div>

          {/* Local model status */}
          {embedConfig.provider === 'local' && (
            <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
              <div className="flex items-center justify-between">
                <p className="text-sm font-medium text-text-primary">{t('settings.embeddingLocalModel')}</p>
                <div className="flex items-center gap-2 text-sm">
                  {localModelReady === null ? (
                    <Loader2 size={14} className="animate-spin text-text-tertiary" />
                  ) : localModelReady ? (
                    <Badge variant="default" className="gap-1">
                      <CheckCircle size={12} className="text-success" />
                      {t('settings.embeddingDownloaded')}
                    </Badge>
                  ) : (
                    <Badge variant="default" className="gap-1">
                      <XCircle size={12} className="text-danger" />
                      {t('settings.embeddingNotDownloaded')}
                    </Badge>
                  )}
                </div>
              </div>
              <p className="text-xs text-text-tertiary">
                {embedConfig.localModel === 'MultilingualE5Base' ? t('settings.embeddingModelQuality') : t('settings.embeddingModelLight')}
              </p>
            </div>
          )}

          {/* API panel */}
          {embedConfig.provider === 'api' && (
            <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.provider')}</label>
                <select
                  value={activeApiPreset?.id ?? 'custom'}
                  onChange={(event) => applyApiPreset(event.target.value)}
                  className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
                >
                  {EMBEDDING_PROVIDER_PRESETS.map((preset) => (
                    <option key={preset.id} value={preset.id}>{preset.name}</option>
                  ))}
                </select>
                {activeApiPreset && (
                  <div className="flex items-start gap-2 rounded-md border border-border/60 bg-surface-1/60 p-2.5">
                    <ProviderIcon
                      provider={activeApiPreset.provider}
                      providerId={activeApiPreset.id}
                      baseUrl={activeApiPreset.baseUrl}
                      size="sm"
                    />
                    <p className="text-xs leading-5 text-text-tertiary">{activeApiPreset.description}</p>
                  </div>
                )}
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.embeddingApiKey')}</label>
                <div className="relative">
                  <KeyRound size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
                  <Input
                    type="password"
                    value={embedConfig.apiKey}
                    onChange={(e) => updateConfig({ apiKey: e.target.value })}
                    className="pl-9"
                    placeholder="sk-..."
                  />
                </div>
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.embeddingBaseUrl')}</label>
                <Input
                  value={embedConfig.apiBaseUrl}
                  onChange={(e) => updateConfig({ apiBaseUrl: e.target.value })}
                  placeholder="https://api.openai.com/v1"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.embeddingModel')}</label>
                {activeApiPreset && activeApiPreset.models.length > 0 ? (
                  <select
                    value={embedConfig.apiModel}
                    onChange={(event) => applyApiModel(event.target.value)}
                    className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
                  >
                    {activeApiPreset.models.map((model) => (
                      <option key={model.id} value={model.id}>
                        {model.name} · {model.dimensions}d{model.recommended ? ' *' : ''}
                      </option>
                    ))}
                  </select>
                ) : (
                  <Input
                    value={embedConfig.apiModel}
                    onChange={(e) => updateConfig({ apiModel: e.target.value })}
                    placeholder="text-embedding-3-small"
                  />
                )}
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.embeddingDimensions')}</label>
                <Input
                  type="number"
                  min={1}
                  value={embedConfig.vectorDimensions}
                  onChange={(event) => updateConfig({ vectorDimensions: Math.max(1, Number(event.target.value) || 1) })}
                  disabled={Boolean(activeApiPreset && activeApiPreset.models.length > 0 && !activeApiPreset.models.find((model) => model.id === embedConfig.apiModel)?.supportsDimensionOverride)}
                />
              </div>
              <Button
                variant="secondary"
                size="sm"
                icon={testLoading ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
                loading={testLoading}
                onClick={onTestConnection}
                disabled={!embedConfig.apiKey.trim() || !embedConfig.apiBaseUrl.trim()}
              >
                {t('settings.embeddingTestConnection')}
              </Button>
            </div>
          )}

          {/* TF-IDF warning */}
          {embedConfig.provider === 'tfidf' && (
            <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-3">
              <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
              <p className="text-sm text-warning">{t('settings.embeddingTfidfWarning')}</p>
            </div>
          )}

          {/* Provider change warning + actions */}
          <div className="space-y-3 border-t border-border pt-4">
            <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-3">
              <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
              <p className="text-sm text-warning">{t('settings.embeddingProviderChangeWarning')}</p>
            </div>
            <div className="flex items-center gap-3">
              <Button
                variant="primary"
                size="md"
                icon={<Save size={16} />}
                loading={embedSaveLoading}
                onClick={onSave}
              >
                {t('settings.embeddingSave')}
              </Button>
              <Button
                variant="secondary"
                size="md"
                icon={rebuildEmbedLoading ? <Loader2 size={16} className="animate-spin" /> : <RefreshCw size={16} />}
                loading={rebuildEmbedLoading}
                onClick={onRebuild}
              >
                {rebuildEmbedLoading ? t('settings.embeddingRebuilding') : t('settings.embeddingRebuild')}
              </Button>
            </div>
            {embedRebuildProgress && (
              <div className="mt-2">
                <div className="flex items-center gap-2 text-xs text-muted">
                  <RefreshCw size={12} className="animate-spin" />
                  <span>{embedRebuildProgress.current}/{embedRebuildProgress.total}</span>
                </div>
                {embedRebuildProgress.total > 0 && (
                  <div className="w-full bg-surface-3 rounded h-1 mt-1">
                    <div
                      className="bg-accent h-1 rounded transition-all duration-300"
                      style={{ width: `${Math.min(100, (embedRebuildProgress.current / embedRebuildProgress.total) * 100)}%` }}
                    />
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </Section>
  );
}
