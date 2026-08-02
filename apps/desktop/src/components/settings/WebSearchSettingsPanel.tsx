import { RefreshCw, SearchCheck } from 'lucide-react';
import { NexaSelect } from '../ui/overlay';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation, type TranslationKey } from '../../i18n';
import * as api from '../../lib/api';
import type {
  AppConfig,
  WebSearchCustomProviderConfig,
  WebSearchCustomProviderPreset,
  WebSearchConfig,
  WebSearchProviderHealth,
  WebSearchProviderMode,
  WebSearchProviderProfile,
  WebSearchProviderStatus,
  WebSearchReranker,
} from '../../types/conversation';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Section } from './SettingsSection';

interface WebSearchSettingsPanelProps {
  appConfig: AppConfig;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: () => void;
}

const DEFAULT_WEB_SEARCH_CONFIG: WebSearchConfig = {
  providerProfile: 'default',
  reranker: 'auto',
  providerMode: 'built_in_first',
  customProviders: [
    {
      id: 'brave',
      preset: 'brave',
      name: 'Brave Search API',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://api.search.brave.com/res/v1/web/search',
      priority: 10,
    },
    {
      id: 'tavily',
      preset: 'tavily',
      name: 'Tavily Search',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://api.tavily.com/search',
      priority: 20,
    },
    {
      id: 'anysearch',
      preset: 'anysearch',
      name: 'AnySearch',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://api.anysearch.com/v1/search',
      priority: 25,
    },
    {
      id: 'serpapi_google',
      preset: 'serpapi_google',
      name: 'SerpAPI Google',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://serpapi.com/search.json',
      priority: 30,
    },
    {
      id: 'searxng',
      preset: 'searxng',
      name: 'SearXNG',
      enabled: false,
      apiKey: '',
      baseUrl: '',
      priority: 40,
    },
  ],
};

const PROFILE_OPTIONS: WebSearchProviderProfile[] = [
  'default',
  'free',
  'free_verified',
  'max_evidence',
];

const RERANKER_OPTIONS: WebSearchReranker[] = [
  'auto',
  'none',
  'docs_first',
  'research',
  'news_balanced',
];

const PROVIDER_MODE_OPTIONS: WebSearchProviderMode[] = [
  'built_in_first',
  'custom_first',
  'custom_only',
];

const CUSTOM_PROVIDER_PRESETS: WebSearchCustomProviderPreset[] = [
  'brave',
  'tavily',
  'anysearch',
  'serpapi_google',
  'searxng',
];

const PROFILE_LABEL_KEYS: Record<WebSearchProviderProfile, TranslationKey> = {
  default: 'settings.webSearchProfile.default',
  free: 'settings.webSearchProfile.free',
  free_verified: 'settings.webSearchProfile.free_verified',
  max_evidence: 'settings.webSearchProfile.max_evidence',
};

const PROFILE_DESC_KEYS: Record<WebSearchProviderProfile, TranslationKey> = {
  default: 'settings.webSearchProfile.default.desc',
  free: 'settings.webSearchProfile.free.desc',
  free_verified: 'settings.webSearchProfile.free_verified.desc',
  max_evidence: 'settings.webSearchProfile.max_evidence.desc',
};

const RERANKER_LABEL_KEYS: Record<WebSearchReranker, TranslationKey> = {
  auto: 'settings.webSearchReranker.auto',
  none: 'settings.webSearchReranker.none',
  docs_first: 'settings.webSearchReranker.docs_first',
  research: 'settings.webSearchReranker.research',
  news_balanced: 'settings.webSearchReranker.news_balanced',
};

const RERANKER_DESC_KEYS: Record<WebSearchReranker, TranslationKey> = {
  auto: 'settings.webSearchReranker.auto.desc',
  none: 'settings.webSearchReranker.none.desc',
  docs_first: 'settings.webSearchReranker.docs_first.desc',
  research: 'settings.webSearchReranker.research.desc',
  news_balanced: 'settings.webSearchReranker.news_balanced.desc',
};

const HEALTH_LABEL_KEYS: Record<WebSearchProviderHealth, TranslationKey> = {
  healthy: 'settings.webSearchHealth.healthy',
  degraded: 'settings.webSearchHealth.degraded',
  blocked: 'settings.webSearchHealth.blocked',
  disabled: 'settings.webSearchHealth.disabled',
};

const PROVIDER_MODE_LABEL_KEYS: Record<WebSearchProviderMode, TranslationKey> = {
  built_in_first: 'settings.webSearchProviderMode.built_in_first',
  custom_first: 'settings.webSearchProviderMode.custom_first',
  custom_only: 'settings.webSearchProviderMode.custom_only',
};

const PROVIDER_MODE_DESC_KEYS: Record<WebSearchProviderMode, TranslationKey> = {
  built_in_first: 'settings.webSearchProviderMode.built_in_first.desc',
  custom_first: 'settings.webSearchProviderMode.custom_first.desc',
  custom_only: 'settings.webSearchProviderMode.custom_only.desc',
};

const CUSTOM_PROVIDER_LABEL_KEYS: Record<WebSearchCustomProviderPreset, TranslationKey> = {
  brave: 'settings.webSearchCustomProvider.brave',
  tavily: 'settings.webSearchCustomProvider.tavily',
  anysearch: 'settings.webSearchCustomProvider.anysearch',
  serpapi_google: 'settings.webSearchCustomProvider.serpapi_google',
  searxng: 'settings.webSearchCustomProvider.searxng',
};

const CUSTOM_PROVIDER_DESC_KEYS: Record<WebSearchCustomProviderPreset, TranslationKey> = {
  brave: 'settings.webSearchCustomProvider.brave.desc',
  tavily: 'settings.webSearchCustomProvider.tavily.desc',
  anysearch: 'settings.webSearchCustomProvider.anysearch.desc',
  serpapi_google: 'settings.webSearchCustomProvider.serpapi_google.desc',
  searxng: 'settings.webSearchCustomProvider.searxng.desc',
};

function mergeCustomProviders(
  providers: WebSearchCustomProviderConfig[] | undefined,
): WebSearchCustomProviderConfig[] {
  const byPreset = new Map<WebSearchCustomProviderPreset, WebSearchCustomProviderConfig>();
  for (const provider of providers ?? []) {
    byPreset.set(provider.preset, provider);
  }
  return DEFAULT_WEB_SEARCH_CONFIG.customProviders.map((fallback) => ({
    ...fallback,
    ...(byPreset.get(fallback.preset) ?? {}),
  }));
}

function webSearchConfig(appConfig: AppConfig): WebSearchConfig {
  const incoming = appConfig.webSearch ?? DEFAULT_WEB_SEARCH_CONFIG;
  return {
    ...DEFAULT_WEB_SEARCH_CONFIG,
    ...incoming,
    customProviders: mergeCustomProviders(incoming.customProviders),
  };
}

function healthClass(health: WebSearchProviderHealth): string {
  switch (health) {
    case 'healthy':
      return 'bg-success/10 text-success';
    case 'degraded':
      return 'bg-warning/10 text-warning';
    case 'blocked':
    case 'disabled':
      return 'bg-danger/10 text-danger';
    default:
      return 'bg-surface-3 text-text-tertiary';
  }
}

function isCustomProviderConfigured(provider: WebSearchCustomProviderConfig): boolean {
  if (!provider.enabled) return false;
  if (provider.preset === 'searxng') return Boolean(provider.baseUrl?.trim());
  if (provider.preset === 'anysearch') return true;
  return Boolean(provider.apiKey.trim());
}

export function WebSearchSettingsPanel({
  appConfig,
  loading,
  onChange,
  onMarkDirty,
  onSave,
}: WebSearchSettingsPanelProps) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<WebSearchProviderStatus[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const config = useMemo(() => webSearchConfig(appConfig), [appConfig]);

  const refreshStatus = async () => {
    setRefreshing(true);
    try {
      setStatus(await api.getWebSearchStatus(config));
    } catch {
      setStatus([]);
    } finally {
      setRefreshing(false);
    }
  };

  useEffect(() => {
    void refreshStatus();
  }, [config.providerProfile, config.providerMode, config.customProviders]);

  const updateConfig = (webSearch: WebSearchConfig) => {
    onChange({ ...appConfig, webSearch });
    onMarkDirty();
  };

  const updateCustomProvider = (
    preset: WebSearchCustomProviderPreset,
    patch: Partial<WebSearchCustomProviderConfig>,
  ) => {
    updateConfig({
      ...config,
      customProviders: config.customProviders.map((provider) =>
        provider.preset === preset ? { ...provider, ...patch } : provider,
      ),
    });
  };

  const statusById = new Map(status.map((provider) => [provider.id, provider]));
  const configuredProviderCount = config.customProviders.filter(isCustomProviderConfigured).length;

  return (
    <Section
      icon={<SearchCheck size={20} />}
      title={t('settings.webSearchTitle')}
      description={t('settings.webSearchDesc')}
      collapsible
      defaultOpen={false}
      summary={
        <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
          {configuredProviderCount}/{config.customProviders.length}
        </span>
      }
    >
      <div className="space-y-4">
        <div className="grid gap-3 md:grid-cols-2">
          <label className="space-y-2">
            <span className="text-sm font-medium text-text-primary">
              {t('settings.webSearchProfile')}
            </span>
            <NexaSelect
              value={config.providerProfile}
              onChange={(event) =>
                updateConfig({
                  ...config,
                  providerProfile: event.target.value as WebSearchProviderProfile,
                })
              }
              className="w-full rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary outline-none focus:border-accent"
            >
              {PROFILE_OPTIONS.map((profile) => (
                <option key={profile} value={profile}>
                  {t(PROFILE_LABEL_KEYS[profile])}
                </option>
              ))}
            </NexaSelect>
            <span className="block text-xs leading-relaxed text-text-tertiary">
              {t(PROFILE_DESC_KEYS[config.providerProfile])}
            </span>
          </label>

          <label className="space-y-2">
            <span className="text-sm font-medium text-text-primary">
              {t('settings.webSearchReranker')}
            </span>
            <NexaSelect
              value={config.reranker}
              onChange={(event) =>
                updateConfig({
                  ...config,
                  reranker: event.target.value as WebSearchReranker,
                })
              }
              className="w-full rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary outline-none focus:border-accent"
            >
              {RERANKER_OPTIONS.map((reranker) => (
                <option key={reranker} value={reranker}>
                  {t(RERANKER_LABEL_KEYS[reranker])}
                </option>
              ))}
            </NexaSelect>
            <span className="block text-xs leading-relaxed text-text-tertiary">
              {t(RERANKER_DESC_KEYS[config.reranker])}
            </span>
          </label>
        </div>

        <div className="space-y-2">
          <label className="text-sm font-medium text-text-primary">
            {t('settings.webSearchProviderMode')}
          </label>
          <div className="grid gap-2 md:grid-cols-3">
            {PROVIDER_MODE_OPTIONS.map((mode) => (
              <label
                key={mode}
                className={`cursor-pointer rounded-lg border p-3 transition-colors ${
                  config.providerMode === mode
                    ? 'border-accent bg-accent/10'
                    : 'border-border bg-surface-2'
                }`}
              >
                <div className="flex items-start gap-3">
                  <input
                    type="radio"
                    name="web-search-provider-mode"
                    value={mode}
                    checked={config.providerMode === mode}
                    onChange={() => updateConfig({ ...config, providerMode: mode })}
                    className="mt-1"
                  />
                  <div className="space-y-1">
                    <div className="text-sm font-medium text-text-primary">
                      {t(PROVIDER_MODE_LABEL_KEYS[mode])}
                    </div>
                    <div className="text-xs leading-relaxed text-text-tertiary">
                      {t(PROVIDER_MODE_DESC_KEYS[mode])}
                    </div>
                  </div>
                </div>
              </label>
            ))}
          </div>
        </div>

        <div className="space-y-3">
          <div>
            <p className="text-sm font-medium text-text-primary">
              {t('settings.webSearchCustomProviders')}
            </p>
            <p className="mt-1 text-xs text-text-tertiary">
              {t('settings.webSearchCustomProvidersDesc')}
            </p>
          </div>
          <div className="space-y-2">
            {CUSTOM_PROVIDER_PRESETS.map((preset) => {
              const provider = config.customProviders.find((item) => item.preset === preset);
              if (!provider) return null;
              const providerStatus = statusById.get(provider.id);
              const needsBaseUrl = preset === 'searxng';
              return (
                <div
                  key={preset}
                  className="rounded-lg border border-border bg-surface-2 p-3"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <label className="flex min-w-0 flex-1 cursor-pointer items-start gap-3">
                      <input
                        type="checkbox"
                        checked={provider.enabled}
                        onChange={(event) =>
                          updateCustomProvider(preset, { enabled: event.target.checked })
                        }
                        className="mt-1 rounded border-border"
                      />
                      <span className="min-w-0">
                        <span className="block text-sm font-medium text-text-primary">
                          {t(CUSTOM_PROVIDER_LABEL_KEYS[preset])}
                        </span>
                        <span className="mt-1 block text-xs leading-relaxed text-text-tertiary">
                          {t(CUSTOM_PROVIDER_DESC_KEYS[preset])}
                        </span>
                      </span>
                    </label>
                    {providerStatus && (
                      <span
                        className={`shrink-0 rounded-full px-2 py-0.5 text-xs font-medium ${healthClass(
                          providerStatus.health,
                        )}`}
                      >
                        {providerStatus.configured
                          ? t(HEALTH_LABEL_KEYS[providerStatus.health])
                          : t('settings.webSearchProviderNeedsConfig')}
                      </span>
                    )}
                  </div>
                  <div className="mt-3 grid gap-3 md:grid-cols-[minmax(0,1fr)_7rem]">
                    <div className="space-y-2">
                      {needsBaseUrl ? (
                        <>
                          <label className="text-xs font-medium text-text-secondary">
                            {t('settings.webSearchProviderBaseUrl')}
                          </label>
                          <Input
                            value={provider.baseUrl ?? ''}
                            placeholder="https://searx.example.org"
                            onChange={(event) =>
                              updateCustomProvider(preset, { baseUrl: event.target.value })
                            }
                          />
                        </>
                      ) : (
                        <>
                          <label className="text-xs font-medium text-text-secondary">
                            {t('settings.webSearchProviderApiKey')}
                          </label>
                          <Input
                            type="password"
                            value={provider.apiKey}
                            placeholder={t('settings.webSearchProviderApiKeyPlaceholder')}
                            onChange={(event) =>
                              updateCustomProvider(preset, { apiKey: event.target.value })
                            }
                          />
                        </>
                      )}
                    </div>
                    <div className="space-y-2">
                      <label className="text-xs font-medium text-text-secondary">
                        {t('settings.webSearchProviderPriority')}
                      </label>
                      <Input
                        type="number"
                        min={1}
                        max={100}
                        value={provider.priority}
                        onChange={(event) =>
                          updateCustomProvider(preset, {
                            priority: Math.max(
                              1,
                              Math.min(
                                100,
                                Number.parseInt(event.target.value, 10) || provider.priority,
                              ),
                            ),
                          })
                        }
                      />
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        <div className="space-y-2">
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm font-medium text-text-primary">
              {t('settings.webSearchProviderStatus')}
            </span>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              icon={<RefreshCw size={14} />}
              loading={refreshing}
              onClick={() => void refreshStatus()}
            >
              {t('settings.webSearchRefresh')}
            </Button>
          </div>
          <div className="grid gap-2 md:grid-cols-2">
            {status.map((provider) => (
              <div
                key={provider.engine}
                className="rounded-lg border border-border bg-surface-2 p-3"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium text-text-primary">
                      {provider.label}
                    </div>
                    <div className="mt-1 text-xs text-text-tertiary">
                      {provider.enabledByProfile
                        ? t('settings.webSearchProviderActive')
                        : t('settings.webSearchProviderFallback')}
                    </div>
                  </div>
                  <span
                    className={`shrink-0 rounded-full px-2 py-0.5 text-xs font-medium ${healthClass(
                      provider.health,
                    )}`}
                  >
                    {t(HEALTH_LABEL_KEYS[provider.health])}
                  </span>
                </div>
                {provider.lastErrorCode && (
                  <p className="mt-2 text-xs text-text-tertiary">
                    {t('settings.webSearchLastError')}: {provider.lastErrorCode}
                    {provider.nextRetrySeconds
                      ? `, ${t('settings.webSearchRetryIn', {
                          seconds: provider.nextRetrySeconds,
                        })}`
                      : ''}
                  </p>
                )}
              </div>
            ))}
          </div>
        </div>

        <div className="flex justify-end">
          <Button size="sm" onClick={onSave} disabled={loading}>
            {loading ? '...' : t('common.save')}
          </Button>
        </div>
      </div>
    </Section>
  );
}
