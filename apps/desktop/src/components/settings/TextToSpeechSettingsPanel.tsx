import { convertFileSrc } from '@tauri-apps/api/core';
import { useEffect, useMemo, useState } from 'react';
import { ChevronDown, Cloud, Eye, EyeOff, Laptop, Play, RefreshCw, Save, Search, Trash2, Volume2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { clearSpeechCache, refreshTtsVoiceCatalog, synthesizeSpeechPreview } from '../../lib/api';
import { ProviderIcon } from '../../lib/providerIcons';
import {
  findSharedProviderCredential,
  providerCredentialScope,
} from '../../lib/providerCredentials';
import { defaultTtsItem, findTtsProviderPreset, TTS_PROVIDER_PRESETS } from '../../lib/ttsProviderPresets';
import {
  isTtsVoiceCatalogStale,
  loadTtsVoiceCatalog,
  saveTtsVoiceCatalog,
  ttsVoiceCatalogMatches,
  type TtsVoiceCatalogSnapshot,
} from '../../lib/ttsVoiceCatalog';
import type { AgentConfig, AppConfig, TextToSpeechConfig } from '../../types/conversation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { SharedCredentialNotice } from './SharedCredentialNotice';

interface TextToSpeechSettingsPanelProps {
  appConfig: AppConfig;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: (config?: AppConfig) => void | Promise<void>;
  agentConfigs?: AgentConfig[];
  providerScope?: 'all' | 'cloud' | 'local';
  defaultExpanded?: boolean;
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
  autoSpeakFinalAnswers: false,
};

export function TextToSpeechSettingsPanel({
  appConfig,
  loading,
  onChange,
  onMarkDirty,
  onSave,
  agentConfigs = [],
  providerScope = 'all',
  defaultExpanded = false,
}: TextToSpeechSettingsPanelProps) {
  const { t } = useTranslation();
  const config = appConfig.textToSpeech ?? DEFAULT_TTS_CONFIG;
  const [expanded, setExpanded] = useState(defaultExpanded);
  const [showKey, setShowKey] = useState(false);
  const [clearingCache, setClearingCache] = useState(false);
  const [cacheStatus, setCacheStatus] = useState<string | null>(null);
  const [voiceSearch, setVoiceSearch] = useState('');
  const [voiceCatalogLoading, setVoiceCatalogLoading] = useState(false);
  const [voiceCatalogError, setVoiceCatalogError] = useState<string | null>(null);
  const [voiceCatalog, setVoiceCatalog] = useState<TtsVoiceCatalogSnapshot | null>(() =>
    loadTtsVoiceCatalog(config),
  );
  const [previewText, setPreviewText] = useState('Hello from Nexa.');
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const scopedPresets = useMemo(
    () => TTS_PROVIDER_PRESETS.filter((preset) => providerScope === 'all'
      || (providerScope === 'local' ? preset.local === true : preset.local !== true)),
    [providerScope],
  );
  const cloudPresets = useMemo(() => scopedPresets.filter((preset) => preset.local !== true), [scopedPresets]);
  const localPresets = useMemo(() => scopedPresets.filter((preset) => preset.local === true), [scopedPresets]);
  const groupedCatalog = cloudPresets.length > 0 && localPresets.length > 0;
  const matchedPreset = useMemo(
    () => findTtsProviderPreset(config),
    [config],
  );
  const scopeActive = Boolean(matchedPreset && scopedPresets.some((preset) => preset.id === matchedPreset.id));
  const activePreset = scopeActive ? matchedPreset! : scopedPresets[0];
  const localProvider = Boolean(activePreset.local || config.apiStyle === 'sherpa_onnx');
  const localFamilyNeedsVoices = config.model === 'kokoro' || config.model === 'kitten';
  const sharedKeySource = !localProvider
    ? findSharedProviderCredential(agentConfigs, config.provider, config.baseUrl)
    : null;
  const resolvedApiKey = config.apiKey.trim() || sharedKeySource?.apiKey.trim() || '';
  const materializedConfig = { ...config, apiKey: resolvedApiKey };
  const configured = scopeActive && (localProvider
    ? Boolean(
        config.executablePath?.trim()
        && config.modelPath?.trim()
        && config.tokensPath?.trim()
        && (!localFamilyNeedsVoices || config.voicesPath?.trim()),
      )
    : Boolean(resolvedApiKey && config.model.trim() && config.voice.trim()));
  const matchingVoiceCatalog = voiceCatalog && ttsVoiceCatalogMatches(voiceCatalog, config)
    ? voiceCatalog
    : null;
  const catalogVoices = matchingVoiceCatalog?.voices ?? activePreset.voices;
  const filteredVoices = useMemo(() => {
    const query = voiceSearch.trim().toLowerCase();
    return catalogVoices.filter((voice) => {
      const supportsModel = !voice.modelIds?.length
        || voice.modelIds.some((modelId) => modelId.toLowerCase() === config.model.trim().toLowerCase());
      if (!supportsModel) return false;
      if (!query) return true;
      return [voice.id, voice.name, voice.description, ...(voice.languages ?? [])]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(query));
    });
  }, [catalogVoices, config.model, voiceSearch]);

  useEffect(() => {
    setVoiceCatalog(loadTtsVoiceCatalog(config));
    setVoiceSearch('');
    setVoiceCatalogError(null);
    setPreviewPath(null);
    setPreviewError(null);
  }, [config.apiStyle, config.baseUrl, config.model, config.provider]);

  const update = (patch: Partial<TextToSpeechConfig>) => {
    onChange({ ...appConfig, textToSpeech: { ...config, ...patch } });
    onMarkDirty();
  };

  const applyPreset = (presetId: string) => {
    const preset = scopedPresets.find((candidate) => candidate.id === presetId);
    if (!preset) return;
    const preservesCredential = providerCredentialScope(config.provider, config.baseUrl) ===
      providerCredentialScope(preset.provider, preset.baseUrl);
    update({
      provider: preset.provider,
      apiStyle: preset.apiStyle,
      baseUrl: preset.baseUrl,
      model: defaultTtsItem(preset.models)?.id ?? '',
      voice: defaultTtsItem(preset.voices)?.id ?? '',
      apiKey: preservesCredential ? config.apiKey : '',
      outputFormat: preset.outputFormats[0] ?? 'mp3',
      executablePath: preset.local ? (config.executablePath || 'sherpa-onnx-offline-tts') : config.executablePath,
    });
  };

  const clearCache = async () => {
    setClearingCache(true);
    setCacheStatus(null);
    try {
      const result = await clearSpeechCache();
      setCacheStatus(`Removed ${result.removedFiles} cached audio file${result.removedFiles === 1 ? '' : 's'}.`);
    } catch (error) {
      setCacheStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setClearingCache(false);
    }
  };

  const refreshVoiceCatalog = async () => {
    setVoiceCatalogLoading(true);
    setVoiceCatalogError(null);
    try {
      const snapshot = await refreshTtsVoiceCatalog(materializedConfig);
      saveTtsVoiceCatalog(snapshot);
      setVoiceCatalog(snapshot);
    } catch (error) {
      setVoiceCatalogError(error instanceof Error ? error.message : String(error));
    } finally {
      setVoiceCatalogLoading(false);
    }
  };

  const previewVoice = async () => {
    setPreviewLoading(true);
    setPreviewError(null);
    setPreviewPath(null);
    try {
      const preview = await synthesizeSpeechPreview(previewText, materializedConfig);
      setPreviewPath(convertFileSrc(preview.path));
    } catch (error) {
      setPreviewError(error instanceof Error ? error.message : String(error));
    } finally {
      setPreviewLoading(false);
    }
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
              {!scopeActive
                ? t('settings.speechProviderNotActive')
                : configured
                ? t('settings.configured')
                : localProvider
                  ? t('settings.ttsNeedsLocalFiles')
                  : t('settings.needsApiKey')}
            </Badge>
            <Badge variant="default" className="gap-1 text-[10px]">
              {localProvider ? <Laptop size={10} /> : <Cloud size={10} />}
              {localProvider ? t('settings.speechRuntimeLocal') : t('settings.speechRuntimeCloud')}
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
          {providerScope !== 'local' && <label className="mb-4 flex items-center justify-between gap-4 rounded-md border border-border/60 bg-surface-1/60 px-3 py-2.5">
            <span>
              <span className="block text-sm font-medium text-text-primary">{t('settings.ttsAutoSpeakFinal')}</span>
              <span className="block text-[11px] leading-5 text-text-tertiary">{t('settings.ttsAutoSpeakFinalDesc')}</span>
            </span>
            <input
              type="checkbox"
              checked={config.autoSpeakFinalAnswers === true}
              onChange={(event) => update({ autoSpeakFinalAnswers: event.target.checked })}
              className="h-4 w-4 accent-accent"
            />
          </label>}
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.provider')}</label>
              <select
                data-testid="tts-provider-select"
                value={scopeActive ? activePreset.id : ''}
                onChange={(event) => applyPreset(event.target.value)}
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {!scopeActive && <option value="" disabled>{t('settings.selectSpeechProvider')}</option>}
                {groupedCatalog ? (
                  <>
                    <optgroup label={t('settings.speechProviderGroupCloud')}>
                      {cloudPresets.map((preset) => <option key={preset.id} value={preset.id}>{preset.name}</option>)}
                    </optgroup>
                    <optgroup label={t('settings.speechProviderGroupLocal')}>
                      {localPresets.map((preset) => <option key={preset.id} value={preset.id}>{preset.name}</option>)}
                    </optgroup>
                  </>
                ) : (
                  scopedPresets.map((preset) => <option key={preset.id} value={preset.id}>{preset.name}</option>)
                )}
              </select>
              {groupedCatalog && (
                <p className="text-[11px] leading-5 text-text-tertiary">{t('settings.speechProviderCatalogHint')}</p>
              )}
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
                {sharedKeySource && (
                  <SharedCredentialNotice
                    source={sharedKeySource}
                    hasOwnKey={Boolean(config.apiKey.trim())}
                    onApply={() => update({ apiKey: sharedKeySource.apiKey })}
                    onReset={() => update({ apiKey: '' })}
                  />
                )}
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

            <div className="space-y-2 md:col-span-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <label className="text-sm font-medium text-text-primary">
                  {localProvider ? t('settings.ttsSpeakerId') : t('settings.ttsVoice')}
                </label>
                {!localProvider && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    icon={<RefreshCw size={13} />}
                    loading={voiceCatalogLoading}
                    disabled={!resolvedApiKey}
                    onClick={() => void refreshVoiceCatalog()}
                  >
                    {t('settings.ttsRefreshVoices')}
                  </Button>
                )}
              </div>
              <div className="relative">
                <Search size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
                <Input
                  data-testid="tts-voice-search"
                  value={voiceSearch}
                  onChange={(event) => setVoiceSearch(event.target.value)}
                  placeholder={t('settings.ttsVoiceSearchPlaceholder')}
                  className="pl-9"
                />
              </div>
              <div className="max-h-40 overflow-y-auto rounded-md border border-border bg-surface-1 p-1" data-testid="tts-voice-catalog">
                {filteredVoices.length === 0 ? (
                  <p className="px-2 py-3 text-center text-xs text-text-tertiary">{t('settings.ttsVoiceNoMatches')}</p>
                ) : filteredVoices.map((voice) => (
                  <button
                    key={voice.id}
                    type="button"
                    onClick={() => update({ voice: voice.id })}
                    className={`flex w-full items-start justify-between gap-3 rounded px-2 py-1.5 text-left text-xs transition-colors ${config.voice === voice.id ? 'bg-accent/12 text-accent' : 'text-text-secondary hover:bg-surface-2'}`}
                  >
                    <span className="min-w-0">
                      <span className="block truncate font-medium">{voice.name}</span>
                      <span className="block truncate text-[10px] text-text-tertiary">{voice.id}{voice.languages?.length ? ` · ${voice.languages.join(', ')}` : ''}</span>
                    </span>
                    {'source' in voice && voice.source === 'discovered' && (
                      <span className="shrink-0 rounded-full bg-success/10 px-1.5 py-0.5 text-[10px] text-success">{t('settings.ttsVoiceDiscovered')}</span>
                    )}
                  </button>
                ))}
              </div>
              <label className="block text-[11px] text-text-tertiary">{t('settings.ttsCustomVoiceId')}</label>
              <Input
                data-testid="tts-voice-input"
                value={config.voice}
                onChange={(event) => update({ voice: event.target.value })}
              />
              {matchingVoiceCatalog && (
                <p className="text-[11px] text-text-tertiary" data-testid="tts-voice-catalog-status">
                  {matchingVoiceCatalog.liveDiscoverySucceeded
                    ? t('settings.ttsVoiceCatalogLive')
                    : t('settings.ttsVoiceCatalogCurated')}
                  {' · '}{new Date(matchingVoiceCatalog.refreshedAt).toLocaleString()}
                  {isTtsVoiceCatalogStale(matchingVoiceCatalog) ? ` · ${t('settings.ttsVoiceCatalogStale')}` : ''}
                </p>
              )}
              {voiceCatalogError && <p className="text-[11px] text-danger">{voiceCatalogError}</p>}
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
            <div className="space-y-2 md:col-span-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.ttsPreviewText')}</label>
              <div className="flex flex-col gap-2 sm:flex-row">
                <Input value={previewText} onChange={(event) => setPreviewText(event.target.value)} />
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  icon={<Play size={14} />}
                  loading={previewLoading}
                  disabled={!configured || !previewText.trim()}
                  onClick={() => void previewVoice()}
                >
                  {t('settings.ttsPreview')}
                </Button>
              </div>
              {previewPath && <audio controls preload="metadata" src={previewPath} className="h-9 w-full" />}
              {previewError && <p className="text-[11px] text-danger">{previewError}</p>}
            </div>
          </div>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
            <div className="flex items-center gap-3">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                icon={<Trash2 size={14} />}
                loading={clearingCache}
                onClick={() => void clearCache()}
              >
                Clear speech cache
              </Button>
              {cacheStatus && <span className="text-[11px] text-text-tertiary">{cacheStatus}</span>}
            </div>
            <Button
              type="button"
              variant="primary"
              size="sm"
              icon={<Save size={14} />}
              loading={loading}
              onClick={() => void onSave({ ...appConfig, textToSpeech: materializedConfig })}
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
