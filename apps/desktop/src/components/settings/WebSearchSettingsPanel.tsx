import { RefreshCw, SearchCheck } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation, type TranslationKey } from '../../i18n';
import * as api from '../../lib/api';
import type {
  AppConfig,
  WebSearchConfig,
  WebSearchProviderHealth,
  WebSearchProviderProfile,
  WebSearchProviderStatus,
  WebSearchReranker,
} from '../../types/conversation';
import { Button } from '../ui/Button';
import { CollapsiblePanel } from './SettingsSection';

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

function webSearchConfig(appConfig: AppConfig): WebSearchConfig {
  return {
    ...DEFAULT_WEB_SEARCH_CONFIG,
    ...(appConfig.webSearch ?? {}),
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
      setStatus(await api.getWebSearchStatus(config.providerProfile));
    } catch {
      setStatus([]);
    } finally {
      setRefreshing(false);
    }
  };

  useEffect(() => {
    void refreshStatus();
  }, [config.providerProfile]);

  const updateConfig = (webSearch: WebSearchConfig) => {
    onChange({ ...appConfig, webSearch });
    onMarkDirty();
  };

  return (
    <CollapsiblePanel
      title={t('settings.webSearchTitle')}
      description={t('settings.webSearchDesc')}
      summary={<SearchCheck size={14} className="text-text-tertiary" />}
    >
      <div className="space-y-4">
        <div className="grid gap-3 md:grid-cols-2">
          <label className="space-y-2">
            <span className="text-sm font-medium text-text-primary">
              {t('settings.webSearchProfile')}
            </span>
            <select
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
            </select>
            <span className="block text-xs leading-relaxed text-text-tertiary">
              {t(PROFILE_DESC_KEYS[config.providerProfile])}
            </span>
          </label>

          <label className="space-y-2">
            <span className="text-sm font-medium text-text-primary">
              {t('settings.webSearchReranker')}
            </span>
            <select
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
            </select>
            <span className="block text-xs leading-relaxed text-text-tertiary">
              {t(RERANKER_DESC_KEYS[config.reranker])}
            </span>
          </label>
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
    </CollapsiblePanel>
  );
}
