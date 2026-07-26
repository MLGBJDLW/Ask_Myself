import { useMemo, useState } from 'react';
import { ChevronDown, Eye, EyeOff, Mic2, Save } from 'lucide-react';

import { useTranslation } from '../../i18n';
import { ProviderIcon } from '../../lib/providerIcons';
import { defaultSttItem, STT_PROVIDER_PRESETS } from '../../lib/sttProviderPresets';
import type { AppConfig, SpeechToTextConfig } from '../../types/conversation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';

interface SpeechToTextSettingsPanelProps {
  appConfig: AppConfig;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: (config?: AppConfig) => void | Promise<void>;
}

export const DEFAULT_STT_CONFIG: SpeechToTextConfig = {
  provider: 'local_whisper',
  apiStyle: 'local_whisper',
  apiKey: '',
  baseUrl: null,
  model: 'whisper-local',
  language: null,
  executablePath: null,
  sherpaModelFamily: 'sense_voice',
  modelPath: null,
  tokensPath: null,
  encoderPath: null,
  decoderPath: null,
  joinerPath: null,
  numThreads: 2,
};

export function SpeechToTextSettingsPanel({
  appConfig,
  loading,
  onChange,
  onMarkDirty,
  onSave,
}: SpeechToTextSettingsPanelProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const config = appConfig.speechToText ?? DEFAULT_STT_CONFIG;
  const activePreset = useMemo(
    () => STT_PROVIDER_PRESETS.find((preset) =>
      preset.provider === config.provider
      && preset.apiStyle === config.apiStyle
      && (preset.sherpaModelFamily ?? null) === (
        config.apiStyle === 'sherpa_onnx' ? config.sherpaModelFamily ?? 'sense_voice' : null
      ),
    ) ?? STT_PROVIDER_PRESETS[0],
    [config.apiStyle, config.provider, config.sherpaModelFamily],
  );
  const isWhisper = config.apiStyle === 'local_whisper';
  const isSherpa = config.apiStyle === 'sherpa_onnx';
  const isZipformer = isSherpa && config.sherpaModelFamily === 'zipformer';
  const configured = isWhisper || (isSherpa
    ? Boolean(
        config.executablePath?.trim()
        && config.tokensPath?.trim()
        && (isZipformer
          ? config.encoderPath?.trim() && config.decoderPath?.trim() && config.joinerPath?.trim()
          : config.modelPath?.trim()),
      )
    : Boolean(config.apiKey.trim() && config.baseUrl?.trim() && config.model.trim()));

  const update = (patch: Partial<SpeechToTextConfig>) => {
    onChange({ ...appConfig, speechToText: { ...config, ...patch } });
    onMarkDirty();
  };
  const applyPreset = (id: string) => {
    const preset = STT_PROVIDER_PRESETS.find((candidate) => candidate.id === id);
    if (!preset) return;
    update({
      provider: preset.provider,
      apiStyle: preset.apiStyle,
      baseUrl: preset.baseUrl || null,
      model: defaultSttItem(preset.models)?.id ?? '',
      sherpaModelFamily: preset.sherpaModelFamily ?? 'sense_voice',
      executablePath: preset.apiStyle === 'sherpa_onnx'
        ? (config.executablePath || (preset.sherpaModelFamily === 'zipformer' ? 'sherpa-onnx' : 'sherpa-onnx-offline'))
        : config.executablePath,
    });
  };

  return (
    <div className="rounded-lg border border-border bg-surface-2" data-testid="speech-to-text-settings-panel">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
        className="flex w-full items-center gap-3 p-3 text-left transition-colors hover:bg-surface-3/40"
      >
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
          <Mic2 size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary">{t('settings.speechToText')}</h3>
            <Badge variant="default" className={configured
              ? 'border-success/20 bg-success/10 text-success'
              : 'border-warning/25 bg-warning/10 text-warning'}>
              {configured ? t('settings.configured') : (isSherpa ? t('settings.sttNeedsLocalFiles') : t('settings.needsApiKey'))}
            </Badge>
          </div>
          <p className="mt-0.5 truncate text-xs text-text-tertiary">{activePreset.name} · {config.model}</p>
        </div>
        <ChevronDown size={16} className={`shrink-0 text-text-tertiary transition-transform ${expanded ? 'rotate-180' : ''}`} />
      </button>

      {expanded && (
        <div className="border-t border-border px-4 py-4">
          <p className="mb-4 text-xs text-text-tertiary">{t('settings.speechToTextDesc')}</p>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2 md:col-span-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.provider')}</label>
              <select
                data-testid="stt-provider-select"
                value={activePreset.id}
                onChange={(event) => applyPreset(event.target.value)}
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {STT_PROVIDER_PRESETS.map((preset) => <option key={preset.id} value={preset.id}>{preset.name}</option>)}
              </select>
              <div className="flex items-start gap-2 rounded-md border border-border/60 bg-surface-1/60 p-2.5">
                <ProviderIcon provider={activePreset.provider} providerId={activePreset.id} baseUrl={activePreset.baseUrl} size="sm" />
                <p className="text-xs leading-5 text-text-tertiary">{activePreset.description}</p>
              </div>
            </div>

            {!isWhisper && !isSherpa && (
              <>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.apiKey')}</label>
                  <div className="relative">
                    <Input type={showKey ? 'text' : 'password'} value={config.apiKey} onChange={(event) => update({ apiKey: event.target.value })} className="pr-10" />
                    <button type="button" onClick={() => setShowKey((value) => !value)} className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary" aria-label={showKey ? t('settings.hideKey') : t('settings.showKey')}>
                      {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
                    </button>
                  </div>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.baseUrl')}</label>
                  <Input value={config.baseUrl ?? ''} onChange={(event) => update({ baseUrl: event.target.value || null })} />
                </div>
              </>
            )}

            {!isWhisper && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.model')}</label>
                <Input value={config.model} onChange={(event) => update({ model: event.target.value })} list="nexa-stt-models" />
                <datalist id="nexa-stt-models">
                  {activePreset.models.map((model) => <option key={model.id} value={model.id}>{model.name}</option>)}
                </datalist>
              </div>
            )}
            {!isWhisper && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.sttLanguage')}</label>
                <Input value={config.language ?? ''} onChange={(event) => update({ language: event.target.value || null })} placeholder={isSherpa ? 'auto' : 'zh / en'} />
              </div>
            )}

            {isWhisper && (
              <p className="rounded-md border border-border/60 bg-surface-1/60 p-3 text-xs leading-5 text-text-tertiary md:col-span-2">
                {t('settings.sttLocalWhisperHint')}
              </p>
            )}

            {isSherpa && (
              <>
                <div className="space-y-2 md:col-span-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.sttSherpaExecutable')}</label>
                  <Input data-testid="stt-sherpa-executable" value={config.executablePath ?? ''} onChange={(event) => update({ executablePath: event.target.value || null })} />
                </div>
                <div className="space-y-2 md:col-span-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.sttTokensPath')}</label>
                  <Input value={config.tokensPath ?? ''} onChange={(event) => update({ tokensPath: event.target.value || null })} />
                </div>
                {isZipformer ? (
                  <>
                    {(['encoderPath', 'decoderPath', 'joinerPath'] as const).map((field) => (
                      <div key={field} className="space-y-2">
                        <label className="text-sm font-medium capitalize text-text-primary">{field.replace('Path', '')}</label>
                        <Input value={config[field] ?? ''} onChange={(event) => update({ [field]: event.target.value || null })} />
                      </div>
                    ))}
                  </>
                ) : (
                  <div className="space-y-2 md:col-span-2">
                    <label className="text-sm font-medium text-text-primary">{t('settings.sttModelPath')}</label>
                    <Input value={config.modelPath ?? ''} onChange={(event) => update({ modelPath: event.target.value || null })} />
                  </div>
                )}
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsThreads')}</label>
                  <Input type="number" min={1} max={32} value={config.numThreads ?? 2} onChange={(event) => update({ numThreads: Math.min(32, Math.max(1, Number(event.target.value) || 2)) })} />
                </div>
              </>
            )}
          </div>
          <div className="mt-4 flex justify-end border-t border-border pt-3">
            <Button type="button" variant="primary" size="sm" icon={<Save size={14} />} loading={loading} onClick={() => void onSave({ ...appConfig, speechToText: config })} disabled={!configured}>
              {t('common.save')}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
