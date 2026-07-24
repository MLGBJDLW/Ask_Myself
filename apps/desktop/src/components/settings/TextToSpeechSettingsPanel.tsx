import { useMemo, useState } from 'react';
import { ChevronDown, Eye, EyeOff, Save, Volume2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { ProviderIcon } from '../../lib/providerIcons';
import { defaultTtsItem, TTS_PROVIDER_PRESETS } from '../../lib/ttsProviderPresets';
import type { AppConfig, TextToSpeechConfig } from '../../types/conversation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';

interface TextToSpeechSettingsPanelProps {
  appConfig: AppConfig;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: (config?: AppConfig) => void | Promise<void>;
}

export const DEFAULT_TTS_CONFIG: TextToSpeechConfig = {
  provider: 'open_ai',
  apiStyle: 'openai_speech',
  apiKey: '',
  baseUrl: 'https://api.openai.com/v1',
  model: 'gpt-4o-mini-tts',
  voice: 'coral',
  outputFormat: 'wav',
  speed: 1,
  executablePath: null,
  modelPath: null,
  tokensPath: null,
  voicesPath: null,
  dataDir: null,
  lexiconPath: null,
  numThreads: 2,
};

export function TextToSpeechSettingsPanel({
  appConfig,
  loading,
  onChange,
  onMarkDirty,
  onSave,
}: TextToSpeechSettingsPanelProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const config = appConfig.textToSpeech ?? DEFAULT_TTS_CONFIG;
  const activePreset = useMemo(
    () => TTS_PROVIDER_PRESETS.find((preset) =>
      preset.apiStyle === config.apiStyle && preset.provider === config.provider,
    ) ?? TTS_PROVIDER_PRESETS[0],
    [config.apiStyle, config.provider],
  );
  const localProvider = Boolean(activePreset.local || config.apiStyle === 'sherpa_onnx');
  const localFamilyNeedsVoices = config.model === 'kokoro' || config.model === 'kitten';
  const configured = localProvider
    ? Boolean(
        config.executablePath?.trim()
        && config.modelPath?.trim()
        && config.tokensPath?.trim()
        && (!localFamilyNeedsVoices || config.voicesPath?.trim()),
      )
    : Boolean(config.apiKey.trim() && config.model.trim() && config.voice.trim());

  const update = (patch: Partial<TextToSpeechConfig>) => {
    onChange({ ...appConfig, textToSpeech: { ...config, ...patch } });
    onMarkDirty();
  };

  const applyPreset = (presetId: string) => {
    const preset = TTS_PROVIDER_PRESETS.find((candidate) => candidate.id === presetId);
    if (!preset) return;
    update({
      provider: preset.provider,
      apiStyle: preset.apiStyle,
      baseUrl: preset.baseUrl,
      model: defaultTtsItem(preset.models)?.id ?? '',
      voice: defaultTtsItem(preset.voices)?.id ?? '',
      outputFormat: preset.outputFormats[0] ?? 'mp3',
      executablePath: preset.local ? (config.executablePath || 'sherpa-onnx-offline-tts') : config.executablePath,
    });
  };

  return (
    <div className="rounded-lg border border-border bg-surface-2" data-testid="text-to-speech-settings-panel">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
        className="flex w-full items-center gap-3 p-3 text-left transition-colors hover:bg-surface-3/40"
      >
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
          <Volume2 size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary">{t('settings.textToSpeech')}</h3>
            <Badge
              variant="default"
              className={configured
                ? 'border-success/20 bg-success/10 text-success'
                : 'border-warning/25 bg-warning/10 text-warning'}
            >
              {configured
                ? t('settings.configured')
                : localProvider
                  ? t('settings.ttsNeedsLocalFiles')
                  : t('settings.needsApiKey')}
            </Badge>
          </div>
          <p className="mt-0.5 truncate text-xs text-text-tertiary">
            {activePreset.name} · {config.model} · {config.voice}
          </p>
        </div>
        <ChevronDown size={16} className={`shrink-0 text-text-tertiary transition-transform ${expanded ? 'rotate-180' : ''}`} />
      </button>

      {expanded && (
        <div className="border-t border-border px-4 py-4">
          <p className="mb-4 text-xs text-text-tertiary">{t('settings.textToSpeechDesc')}</p>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.provider')}</label>
              <select
                value={activePreset.id}
                onChange={(event) => applyPreset(event.target.value)}
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {TTS_PROVIDER_PRESETS.map((preset) => <option key={preset.id} value={preset.id}>{preset.name}</option>)}
              </select>
              <div className="flex items-start gap-2 rounded-md border border-border/60 bg-surface-1/60 p-2.5">
                <ProviderIcon provider={activePreset.provider} providerId={activePreset.id} baseUrl={activePreset.baseUrl} size="sm" />
                <p className="text-xs leading-5 text-text-tertiary">{activePreset.description}</p>
              </div>
            </div>

            {!localProvider && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.apiKey')}</label>
                <div className="relative">
                  <Input
                    type={showKey ? 'text' : 'password'}
                    value={config.apiKey}
                    onChange={(event) => update({ apiKey: event.target.value })}
                    className="pr-10"
                  />
                  <button
                    type="button"
                    onClick={() => setShowKey((value) => !value)}
                    className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary"
                    aria-label={showKey ? t('settings.hideKey') : t('settings.showKey')}
                  >
                    {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                </div>
              </div>
            )}

            {!localProvider && (
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.baseUrl')}</label>
                <Input value={config.baseUrl ?? ''} onChange={(event) => update({ baseUrl: event.target.value || null })} />
              </div>
            )}

            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.model')}</label>
              <select
                value={config.model}
                onChange={(event) => update({ model: event.target.value })}
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.models.map((model) => (
                  <option key={model.id} value={model.id}>{model.name}{model.recommended ? ' *' : ''}</option>
                ))}
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">
                {localProvider ? t('settings.ttsSpeakerId') : t('settings.ttsVoice')}
              </label>
              <Input
                data-testid="tts-voice-input"
                value={config.voice}
                onChange={(event) => update({ voice: event.target.value })}
                list="nexa-tts-voices"
              />
              <datalist id="nexa-tts-voices">
                {activePreset.voices.map((voice) => <option key={voice.id} value={voice.id}>{voice.name}</option>)}
              </datalist>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.ttsOutputFormat')}</label>
              <select
                value={config.outputFormat}
                onChange={(event) => update({ outputFormat: event.target.value })}
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.outputFormats.map((format) => (
                  <option key={format} value={format}>{format.toUpperCase()}</option>
                ))}
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.ttsSpeed')}</label>
              <Input
                type="number"
                min={0.5}
                max={2}
                step={0.05}
                value={config.speed}
                onChange={(event) => update({ speed: Math.min(2, Math.max(0.5, Number(event.target.value) || 1)) })}
              />
            </div>

            {localProvider && (
              <>
                <div className="space-y-2 md:col-span-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsLocalExecutable')}</label>
                  <Input
                    data-testid="tts-local-executable"
                    value={config.executablePath ?? ''}
                    onChange={(event) => update({ executablePath: event.target.value || null })}
                    placeholder="sherpa-onnx-offline-tts"
                  />
                  <p className="text-[11px] leading-5 text-text-tertiary">{t('settings.ttsSherpaHint')}</p>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsModelPath')}</label>
                  <Input data-testid="tts-local-model" value={config.modelPath ?? ''} onChange={(event) => update({ modelPath: event.target.value || null })} />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsTokensPath')}</label>
                  <Input value={config.tokensPath ?? ''} onChange={(event) => update({ tokensPath: event.target.value || null })} />
                </div>
                {localFamilyNeedsVoices && (
                  <div className="space-y-2">
                    <label className="text-sm font-medium text-text-primary">{t('settings.ttsVoicesPath')}</label>
                    <Input value={config.voicesPath ?? ''} onChange={(event) => update({ voicesPath: event.target.value || null })} />
                  </div>
                )}
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsDataDir')}</label>
                  <Input value={config.dataDir ?? ''} onChange={(event) => update({ dataDir: event.target.value || null })} />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsLexiconPath')}</label>
                  <Input value={config.lexiconPath ?? ''} onChange={(event) => update({ lexiconPath: event.target.value || null })} />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.ttsThreads')}</label>
                  <Input
                    type="number"
                    min={1}
                    max={32}
                    value={config.numThreads ?? 2}
                    onChange={(event) => update({ numThreads: Math.min(32, Math.max(1, Number(event.target.value) || 2)) })}
                  />
                </div>
              </>
            )}
          </div>
          <div className="mt-4 flex justify-end border-t border-border pt-3">
            <Button
              type="button"
              variant="primary"
              size="sm"
              icon={<Save size={14} />}
              loading={loading}
              onClick={() => void onSave({ ...appConfig, textToSpeech: config })}
              disabled={!configured}
            >
              {t('common.save')}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
