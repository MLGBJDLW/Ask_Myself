import { useEffect, useMemo, useState } from 'react';
import {
  AudioLines,
  Bot,
  CircleAlert,
  Database,
  Image as ImageIcon,
  Loader2,
  Pencil,
  Plus,
  Search,
  Settings2,
  Star,
  Trash2,
  X,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import {
  findProviderPreset,
  PROVIDER_PRESETS,
  type ProviderPreset,
} from '../../lib/providerPresets';
import { ProviderIcon } from '../../lib/providerIcons';
import type { AgentConfig, AppConfig, SaveAgentConfigInput } from '../../types/conversation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { AgentConfigForm } from './AgentConfigForm';
import { ImageGenerationSettingsPanel } from './ImageGenerationSettingsPanel';
import { TextToSpeechSettingsPanel } from './TextToSpeechSettingsPanel';
import { SpeechToTextSettingsPanel } from './SpeechToTextSettingsPanel';
import { SubscriptionAccountsPanel } from './SubscriptionAccountsPanel';
import { CollapsiblePanel, Section } from './SettingsSection';
import {
  CapabilityRegistryPanel,
  summarizeCapabilityRegistry,
  type CapabilityRegistrySummary,
} from './CapabilityRegistryPanel';

export type ProviderView = 'list' | 'selector' | 'form';

interface RegistrySummaryState {
  loading: boolean;
  error: string | null;
  value: CapabilityRegistrySummary | null;
}

function normalizeProviderSearch(value: string): string {
  return value.trim().toLocaleLowerCase();
}

interface ProvidersSettingsTabProps {
  providerView: ProviderView;
  agentConfigs: AgentConfig[];
  editingConfig: AgentConfig | undefined;
  selectedPreset: ProviderPreset | null;
  agentSaveLoading: boolean;
  appConfig: AppConfig | null;
  appConfigLoading: boolean;
  onSaveAgent: (input: SaveAgentConfigInput) => Promise<void>;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: (config?: AppConfig) => void | Promise<void>;
  onMarkAppConfigDirty: () => void;
  onProviderViewChange: (view: ProviderView) => void;
  onProviderFormDirtyChange: (dirty: boolean) => void;
  onEditingConfigChange: (config: AgentConfig | undefined) => void;
  onSelectedPresetChange: (preset: ProviderPreset | null) => void;
  onSetDefault: (id: string) => void;
  onDeleteTargetChange: (config: AgentConfig) => void;
  micDevices?: MediaDeviceInfo[];
  micDeviceId?: string | null;
  onMicDeviceChange?: (deviceId: string | null) => void;
  onRefreshMics?: () => void;
  localSpeechRuntimeReady?: boolean | null;
}

export function ProvidersSettingsTab({
  providerView,
  agentConfigs,
  editingConfig,
  selectedPreset,
  agentSaveLoading,
  appConfig,
  appConfigLoading,
  onSaveAgent,
  onAppConfigChange,
  onAppConfigSave,
  onMarkAppConfigDirty,
  onProviderViewChange,
  onProviderFormDirtyChange,
  onEditingConfigChange,
  onSelectedPresetChange,
  onSetDefault,
  onDeleteTargetChange,
  micDevices,
  micDeviceId,
  onMicDeviceChange,
  onRefreshMics,
  localSpeechRuntimeReady,
}: ProvidersSettingsTabProps) {
  const { t } = useTranslation();
  const [providerQuery, setProviderQuery] = useState('');
  const [registrySummary, setRegistrySummary] = useState<RegistrySummaryState>({
    loading: true,
    error: null,
    value: null,
  });
  const providerLabels: Record<string, string> = {
    open_ai: t('settings.providerOpenAI'),
    openrouter: t('settings.providerOpenRouter'),
    anthropic: t('settings.providerAnthropic'),
    google: t('settings.providerGoogle'),
    deep_seek: t('settings.providerDeepSeek'),
    ollama: t('settings.providerOllama'),
    lm_studio: t('settings.providerLMStudio'),
    azure_open_ai: t('settings.providerAzure'),
    zhipu: t('settings.providerZhipu'),
    moonshot: t('settings.providerMoonshot'),
    qwen: t('settings.providerQwen'),
    alibaba_model_studio: t('settings.providerAlibabaModelStudio'),
    siliconflow: t('settings.providerSiliconFlow'),
    doubao: t('settings.providerDoubao'),
    yi: t('settings.providerYi'),
    baichuan: t('settings.providerBaichuan'),
    custom: t('settings.providerCustom'),
  };
  const showProviderList = () => {
    onProviderFormDirtyChange(false);
    onProviderViewChange('list');
    onEditingConfigChange(undefined);
    onSelectedPresetChange(null);
  };
  const defaultAgent = agentConfigs.find((config) => config.isDefault);
  const defaultAgentId = defaultAgent?.id ?? agentConfigs[0]?.id;
  const registryRefreshToken = agentConfigs
    .map((config) => `${config.id}:${config.updatedAt ?? ''}:${config.isDefault}`)
    .join('|');
  const configuredPresetIds = useMemo(() => new Set(
    agentConfigs
      .map((config) => findProviderPreset({
        provider: config.provider,
        baseUrl: config.baseUrl,
      })?.id)
      .filter((id): id is string => Boolean(id)),
  ), [agentConfigs]);
  const normalizedProviderQuery = normalizeProviderSearch(providerQuery);
  const providerPresetResults = useMemo(() => PROVIDER_PRESETS
    .filter((preset) => {
      if (!normalizedProviderQuery) return true;
      return normalizeProviderSearch([
        preset.name,
        preset.id,
        preset.provider,
        preset.description,
        ...preset.models.map((model) => `${model.id} ${model.name}`),
      ].join(' ')).includes(normalizedProviderQuery);
    })
    .sort((left, right) => (
      Number(configuredPresetIds.has(right.id)) - Number(configuredPresetIds.has(left.id))
    )), [configuredPresetIds, normalizedProviderQuery]);
  const showCustomProvider = !normalizedProviderQuery || normalizeProviderSearch([
    t('settings.customProvider'),
    t('settings.customProviderDesc'),
    'custom manual openai compatible',
  ].join(' ')).includes(normalizedProviderQuery);

  useEffect(() => {
    if (providerView !== 'list') return undefined;
    let active = true;
    setRegistrySummary((current) => ({ ...current, loading: true, error: null }));
    void api.getCapabilityRegistryProjection({ agentId: defaultAgentId })
      .then((projection) => {
        if (!active) return;
        setRegistrySummary({
          loading: false,
          error: null,
          value: summarizeCapabilityRegistry(projection),
        });
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setRegistrySummary({
          loading: false,
          error: cause instanceof Error ? cause.message : String(cause),
          value: null,
        });
      });
    return () => {
      active = false;
    };
  }, [defaultAgentId, providerView, registryRefreshToken]);

  const registryDescription = registrySummary.value
    ? `${t('settings.capabilityRegistryAdvancedDisclosureDesc')} ${t('settings.capabilityRegistryAdvancedSummary', {
      connections: registrySummary.value.connectionCount,
      models: registrySummary.value.modelCount,
      capabilities: registrySummary.value.capabilityCount,
    })}`
    : t('settings.capabilityRegistryAdvancedDisclosureDesc');

  return (
    <Section icon={<Bot size={20} />} title={t('settings.aiProviders')} delay={0.03}>
      {providerView === 'form' ? (
        <AgentConfigForm
          config={editingConfig}
          preset={editingConfig ? undefined : selectedPreset}
          onSave={onSaveAgent}
          onCancel={showProviderList}
          isSaving={agentSaveLoading}
          onDirtyChange={onProviderFormDirtyChange}
        />
      ) : providerView === 'selector' ? (
        <div className="space-y-4">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div>
              <h3 className="text-lg font-medium text-text-primary">{t('settings.selectProvider')}</h3>
              <p className="mt-1 text-xs leading-5 text-text-tertiary">
                {t('settings.providerCatalogDesc')}
              </p>
            </div>
            <button
              onClick={() => onProviderViewChange('list')}
              className="flex shrink-0 items-center gap-1.5 self-end rounded-md px-2.5 py-1.5 text-sm text-text-tertiary transition-colors hover:bg-surface-3/50 hover:text-text-secondary sm:self-auto"
            >
              <X size={16} /> {t('common.cancel')}
            </button>
          </div>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <label className="relative block min-w-0 flex-1">
              <span className="sr-only">{t('settings.providerSearchLabel')}</span>
              <Search
                aria-hidden="true"
                size={15}
                className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary"
              />
              <input
                type="search"
                value={providerQuery}
                onChange={(event) => setProviderQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') setProviderQuery('');
                }}
                placeholder={t('settings.providerSearchPlaceholder')}
                className="w-full rounded-lg border border-border bg-surface-2 py-2 pl-9 pr-3 text-sm text-text-primary outline-none transition-colors placeholder:text-text-tertiary focus:border-accent"
              />
            </label>
            <Badge variant="muted" className="self-end sm:self-auto">
              {t('settings.providerCatalogResults', {
                count: providerPresetResults.length + Number(showCustomProvider),
              })}
            </Badge>
          </div>
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
            {providerPresetResults.map((preset) => {
              const isConfigured = configuredPresetIds.has(preset.id);
              return (
                <button
                  key={preset.id}
                  data-provider-preset-id={preset.id}
                  onClick={() => { onSelectedPresetChange(preset); onProviderViewChange('form'); }}
                  className="flex min-w-0 flex-col items-start gap-3 overflow-hidden rounded-lg border border-border bg-surface-2 p-3 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 sm:flex-row sm:p-4"
                >
                  <ProviderIcon provider={preset.provider} providerId={preset.id} baseUrl={preset.baseUrl} size="lg" />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium text-text-primary">{preset.name}</span>
                      {isConfigured && (
                        <Badge variant="success">{t('settings.providerConfiguredStatus')}</Badge>
                      )}
                    </div>
                    <div className="mt-1 break-words text-sm leading-5 text-text-tertiary [overflow-wrap:anywhere]">{preset.description}</div>
                  </div>
                </button>
              );
            })}
            {showCustomProvider && (
              <button
                data-provider-preset-id="custom"
                onClick={() => { onSelectedPresetChange(null); onProviderViewChange('form'); }}
                className="flex min-w-0 flex-col items-start gap-3 overflow-hidden rounded-lg border border-dashed border-border bg-surface-2 p-3 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 sm:flex-row sm:p-4"
              >
                <Settings2 className="mt-0.5 shrink-0 text-text-tertiary" size={24} />
                <div className="min-w-0">
                  <div className="font-medium text-text-primary">{t('settings.customProvider')}</div>
                  <div className="mt-1 break-words text-sm leading-5 text-text-tertiary [overflow-wrap:anywhere]">{t('settings.customProviderDesc')}</div>
                </div>
              </button>
            )}
          </div>
          {providerPresetResults.length === 0 && !showCustomProvider && (
            <div className="rounded-lg border border-dashed border-border bg-surface-2/60 px-4 py-8 text-center">
              <Search aria-hidden="true" size={24} className="mx-auto text-text-tertiary" />
              <p className="mt-2 text-sm font-medium text-text-secondary">
                {t('settings.providerSearchNoResults')}
              </p>
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-4">
          <SubscriptionAccountsPanel />

          <div
            className="flex flex-col gap-3 rounded-lg border border-border bg-surface-2/60 p-4 sm:flex-row sm:items-center sm:justify-between"
            data-provider-category="chat-reasoning"
          >
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                <Bot size={15} className="shrink-0 text-accent" />
                <span>{t('settings.commonLlm')}</span>
              </div>
              <p className="mt-1 text-xs leading-5 text-text-tertiary">
                {t('settings.providerConfiguredSummary', { count: agentConfigs.length })}
                {defaultAgent
                  ? ` · ${t('settings.providerDefaultSummary', {
                    name: defaultAgent.name,
                  })}`
                  : ''}
              </p>
            </div>
            <Button
              variant="primary"
              size="sm"
              className="self-end sm:self-auto"
              icon={<Plus size={14} />}
              onClick={() => { onEditingConfigChange(undefined); onSelectedPresetChange(null); onProviderViewChange('selector'); }}
            >
              {t('settings.addProvider')}
            </Button>
          </div>

          {/* Config list */}
          {agentConfigs.length === 0 ? (
            <div className="py-8 text-center">
              <Bot size={32} className="mx-auto mb-3 text-text-tertiary" />
              <p className="text-sm font-medium text-text-secondary">{t('settings.noProviders')}</p>
              <p className="mt-1 text-xs text-text-tertiary">{t('settings.noProvidersDesc')}</p>
            </div>
          ) : (
            <div className="space-y-5">
              <div className="space-y-3">
                  {agentConfigs.map((config) => (
                    <div
                      key={config.id}
                      className="flex flex-col gap-3 rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <div className="flex min-w-0 items-start gap-3 sm:items-center">
                        <ProviderIcon
                          provider={config.provider}
                          baseUrl={config.baseUrl}
                          label={`${config.name} ${config.model}`}
                        />
                        <div className="min-w-0">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <p className="min-w-0 break-words text-sm font-medium text-text-primary">{config.name}</p>
                            <Badge variant="success" className="text-[10px]">
                              {t('settings.providerConfiguredStatus')}
                            </Badge>
                            {config.isDefault && (
                              <Badge
                                variant="warning"
                                icon={<Star size={11} className="fill-current" />}
                                className="text-[10px]"
                              >
                                {t('settings.providerDefaultStatus')}
                              </Badge>
                            )}
                            <Badge variant="default" className="text-[10px] shrink-0">
                              {providerLabels[config.provider] ?? config.provider}
                            </Badge>
                          </div>
                          <p className="mt-1 break-all text-xs leading-5 text-text-tertiary" title={config.baseUrl ?? undefined}>
                            {config.model}
                            {config.baseUrl ? ` · ${config.baseUrl}` : ''}
                          </p>
                        </div>
                      </div>

                      <div className="flex w-full shrink-0 items-center justify-end gap-1 sm:ml-3 sm:w-auto">
                        {!config.isDefault && (
                          <button
                            onClick={() => onSetDefault(config.id)}
                            className="rounded p-1.5 text-text-tertiary hover:text-warning hover:bg-warning/10 transition-colors cursor-pointer"
                            aria-label={t('settings.setDefault')}
                            title={t('settings.setDefault')}
                          >
                            <Star size={14} />
                          </button>
                        )}
                        <button
                          onClick={() => { onEditingConfigChange(config); onProviderViewChange('form'); }}
                          className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                          aria-label={t('common.edit')}
                          title={t('common.edit')}
                        >
                          <Pencil size={14} />
                        </button>
                        <button
                          onClick={() => onDeleteTargetChange(config)}
                          className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                          aria-label={t('common.delete')}
                          title={t('common.delete')}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                  ))}
              </div>
            </div>
          )}

          {appConfig && (
            <>
              <div className="space-y-2" data-provider-category="image-generation">
                <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                  <ImageIcon size={14} />
                  <span>{t('settings.providerCategoryImage')}</span>
                </div>
                <ImageGenerationSettingsPanel
                  appConfig={appConfig}
                  agentConfigs={agentConfigs}
                  loading={appConfigLoading}
                  onChange={onAppConfigChange}
                  onMarkDirty={onMarkAppConfigDirty}
                  onSave={onAppConfigSave}
                />
              </div>
              <div className="space-y-3" data-provider-category="speech">
                <div className="flex flex-wrap items-center gap-2 text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                  <AudioLines size={14} />
                  <span>{t('settings.providerCategorySpeech')}</span>
                </div>
                <p className="text-xs leading-5 text-text-tertiary">{t('settings.providerCategorySpeechDesc')}</p>
                <TextToSpeechSettingsPanel
                  appConfig={appConfig}
                  agentConfigs={agentConfigs}
                  loading={appConfigLoading}
                  onChange={onAppConfigChange}
                  onMarkDirty={onMarkAppConfigDirty}
                  onSave={onAppConfigSave}
                />
                <SpeechToTextSettingsPanel
                  appConfig={appConfig}
                  agentConfigs={agentConfigs}
                  loading={appConfigLoading}
                  onChange={onAppConfigChange}
                  onMarkDirty={onMarkAppConfigDirty}
                  onSave={onAppConfigSave}
                  micDevices={micDevices}
                  micDeviceId={micDeviceId}
                  onMicDeviceChange={onMicDeviceChange}
                  onRefreshMics={onRefreshMics}
                  localRuntimeReady={localSpeechRuntimeReady}
                />
              </div>
            </>
          )}

          <div data-provider-category="capability-registry">
            <CollapsiblePanel
              title={t('settings.capabilityRegistryAdvancedDisclosureTitle')}
              description={registryDescription}
              defaultOpen={false}
              testId="capability-registry-disclosure"
              summary={registrySummary.loading ? (
                <span className="inline-flex items-center text-text-tertiary" role="status">
                  <Loader2 aria-hidden="true" size={14} className="animate-spin" />
                  <span className="sr-only">{t('settings.capabilityRegistryAdvancedLoading')}</span>
                </span>
              ) : registrySummary.error ? (
                <Badge
                  variant="danger"
                  icon={<CircleAlert aria-hidden="true" size={11} />}
                  title={registrySummary.error}
                >
                  {t('settings.capabilityRegistryAdvancedNeedsAttention')}
                </Badge>
              ) : (
                <Database aria-hidden="true" size={15} className="text-text-tertiary" />
              )}
            >
              <CapabilityRegistryPanel
                agentId={defaultAgentId}
                refreshToken={registryRefreshToken}
              />
            </CollapsiblePanel>
          </div>
        </div>
      )}
    </Section>
  );
}
