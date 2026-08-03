import { AudioLines, Bot, Image as ImageIcon, Pencil, Plus, Settings2, Star, Trash2, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { DEFAULT_SUBAGENT_TOOL_NAMES } from '../../lib/subagentTools';
import { PROVIDER_PRESETS, type ProviderPreset } from '../../lib/providerPresets';
import { ProviderIcon } from '../../lib/providerIcons';
import type { AgentConfig, AppConfig, SaveAgentConfigInput } from '../../types/conversation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { AgentConfigForm } from './AgentConfigForm';
import { ImageGenerationSettingsPanel } from './ImageGenerationSettingsPanel';
import { TextToSpeechSettingsPanel } from './TextToSpeechSettingsPanel';
import { SpeechToTextSettingsPanel } from './SpeechToTextSettingsPanel';
import { Section } from './SettingsSection';

export type ProviderView = 'list' | 'selector' | 'form';

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
          <div className="flex items-center justify-between">
            <h3 className="text-lg font-medium text-text-primary">{t('settings.selectProvider')}</h3>
            <button
              onClick={() => onProviderViewChange('list')}
              className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm text-text-tertiary hover:text-text-secondary hover:bg-surface-3/50 transition-colors cursor-pointer"
            >
              <X size={16} /> {t('common.cancel')}
            </button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            {PROVIDER_PRESETS.map((preset) => (
              <button
                key={preset.id}
                onClick={() => { onSelectedPresetChange(preset); onProviderViewChange('form'); }}
                className="flex items-start gap-3 rounded-lg border border-border bg-surface-2 p-4 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 cursor-pointer"
              >
                <ProviderIcon provider={preset.provider} providerId={preset.id} baseUrl={preset.baseUrl} size="lg" />
                <div>
                  <div className="font-medium text-text-primary">{preset.name}</div>
                  <div className="text-sm text-text-tertiary">{preset.description}</div>
                </div>
              </button>
            ))}
            {/* Custom / Manual option */}
            <button
              onClick={() => { onSelectedPresetChange(null); onProviderViewChange('form'); }}
              className="flex items-start gap-3 rounded-lg border border-dashed border-border bg-surface-2 p-4 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 cursor-pointer"
            >
              <Settings2 className="mt-0.5 text-text-tertiary" size={24} />
              <div>
                <div className="font-medium text-text-primary">{t('settings.customProvider')}</div>
                <div className="text-sm text-text-tertiary">{t('settings.customProviderDesc')}</div>
              </div>
            </button>
          </div>
        </div>
      ) : (
        <div className="space-y-4">
          {/* Add button */}
          <div className="flex justify-end">
            <Button
              variant="primary"
              size="sm"
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
              <div className="space-y-2" data-provider-category="chat-reasoning">
                <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                  <Bot size={14} />
                  <span>{t('settings.commonLlm')}</span>
                </div>
                <div className="space-y-3">
                  {agentConfigs.map((config) => (
                    <div
                      key={config.id}
                      className="flex items-center justify-between rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <ProviderIcon
                          provider={config.provider}
                          baseUrl={config.baseUrl}
                          label={`${config.name} ${config.model}`}
                        />
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <p className="text-sm font-medium text-text-primary truncate">{config.name}</p>
                            {config.isDefault && (
                              <Star size={14} className="shrink-0 fill-warning text-warning" />
                            )}
                            <Badge variant="default" className="text-[10px] shrink-0">
                              {providerLabels[config.provider] ?? config.provider}
                            </Badge>
                            <Badge variant="default" className="text-[10px] shrink-0 bg-accent/10 text-accent border-accent/20">
                              {t('settings.subagentsCount', {
                                count: (config.subagentAllowedTools ?? DEFAULT_SUBAGENT_TOOL_NAMES).length,
                              })}
                            </Badge>
                          </div>
                          <p className="mt-0.5 text-xs text-text-tertiary truncate">
                            {config.model}
                            {config.baseUrl ? ` · ${config.baseUrl}` : ''}
                          </p>
                        </div>
                      </div>

                      <div className="flex items-center gap-1 shrink-0 ml-3">
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
        </div>
      )}
    </Section>
  );
}
