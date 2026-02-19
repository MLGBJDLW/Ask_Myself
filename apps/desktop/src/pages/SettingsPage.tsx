import { useState, useEffect, useCallback } from 'react';
import { motion } from 'framer-motion';
import {
  Database,
  Shield,
  RefreshCw,
  Zap,
  Plus,
  Trash2,
  Save,
  Languages,
  Brain,
  Download,
  CheckCircle,
  XCircle,
  Loader2,
  KeyRound,
  AlertTriangle,
  Bot,
  Star,
  Pencil,
  Settings2,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';
import type { IndexStats } from '../types/index-stats';
import type { PrivacyConfig, RedactRule } from '../types/privacy';
import type { EmbedderConfig } from '../types/embedder';
import type { AgentConfig, SaveAgentConfigInput } from '../types/conversation';
import type { ScanProgress, FtsProgress, DownloadProgress } from '../types/ingest';
import { useTranslation } from '../i18n';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { AgentConfigForm } from '../components/settings/AgentConfigForm';
import { PROVIDER_PRESETS, type ProviderPreset } from '../lib/providerPresets';

/* ── Section wrapper ──────────────────────────────────────────────── */
function Section({
  icon,
  title,
  children,
  delay = 0,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
  delay?: number;
}) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, delay, ease: [0.16, 1, 0.3, 1] }}
      className="rounded-xl border border-border bg-surface-1 p-6"
    >
      <div className="mb-5 flex items-center gap-2.5">
        <span className="text-accent">{icon}</span>
        <h2 className="text-base font-semibold text-text-primary">{title}</h2>
      </div>
      {children}
    </motion.section>
  );
}

/* ── Stat card ────────────────────────────────────────────────────── */
function StatCard({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="rounded-lg bg-surface-2 px-4 py-3">
      <p className="text-xs text-text-tertiary">{label}</p>
      <p className="mt-1 text-xl font-bold text-text-primary">{value}</p>
    </div>
  );
}

/* ── Helpers ───────────────────────────────────────────────────────── */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/* ── Settings page ────────────────────────────────────────────────── */
type SettingsTab = 'embedding' | 'index' | 'privacy' | 'language' | 'providers';

export function SettingsPage() {
  const { t, locale, setLocale, availableLocales } = useTranslation();
  const [activeTab, setActiveTab] = useState<SettingsTab>('embedding');
  /* ── Index state ─────────────────────────────────────────────────── */
  const [stats, setStats] = useState<IndexStats | null>(null);
  const [rebuildLoading, setRebuildLoading] = useState(false);
  const [optimizeLoading, setOptimizeLoading] = useState(false);
  const [ftsProgress, setFtsProgress] = useState<FtsProgress | null>(null);
  const [embedRebuildProgress, setEmbedRebuildProgress] = useState<ScanProgress | null>(null);

  const loadStats = useCallback(() => {
    api.getIndexStats().then(setStats).catch(() => {
      toast.error(t('settings.loadStatsError'));
    });
  }, []);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

  /* ── FTS & rebuild progress listeners ───────────────────────────── */

  useEffect(() => {
    let cancelled = false;
    let unlistenFts: (() => void) | undefined;
    let unlistenRebuild: (() => void) | undefined;

    listen<FtsProgress>('batch:fts-progress', (event) => {
      if (cancelled) return;
      const p = event.payload;
      if (p.phase === 'complete') {
        setFtsProgress(null);
      } else {
        setFtsProgress(p);
      }
    }).then((fn) => {
      if (cancelled) { fn(); } else { unlistenFts = fn; }
    });

    listen<ScanProgress>('batch:rebuild-progress', (event) => {
      if (cancelled) return;
      setEmbedRebuildProgress(event.payload);
    }).then((fn) => {
      if (cancelled) { fn(); } else { unlistenRebuild = fn; }
    });

    return () => {
      cancelled = true;
      unlistenFts?.();
      unlistenRebuild?.();
    };
  }, []);

  const handleRebuild = async () => {
    setRebuildLoading(true);
    try {
      await api.rebuildIndex();
      toast.success(t('settings.indexRebuilt'));
      loadStats();
    } catch {
      toast.error(t('settings.indexRebuildError'));
    } finally {
      setRebuildLoading(false);
    }
  };

  const handleOptimize = async () => {
    setOptimizeLoading(true);
    try {
      await api.optimizeFtsIndex();
      toast.success(t('settings.ftsOptimized'));
    } catch {
      toast.error(t('settings.ftsOptimizeError'));
    } finally {
      setOptimizeLoading(false);
    }
  };

  /* ── Privacy state ───────────────────────────────────────────────── */
  const [privacyConfig, setPrivacyConfig] = useState<PrivacyConfig | null>(null);
  const [newPattern, setNewPattern] = useState('');
  const [newRule, setNewRule] = useState<RedactRule>({ name: '', pattern: '', replacement: '' });
  const [saveLoading, setSaveLoading] = useState(false);

  /* ── Embedding state ─────────────────────────────────────────────── */
  const [embedConfig, setEmbedConfig] = useState<EmbedderConfig | null>(null);
  const [localModelReady, setLocalModelReady] = useState<boolean | null>(null);
  const [downloadLoading, setDownloadLoading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [testLoading, setTestLoading] = useState(false);
  const [embedSaveLoading, setEmbedSaveLoading] = useState(false);
  const [rebuildEmbedLoading, setRebuildEmbedLoading] = useState(false);

  useEffect(() => {
    if (!rebuildEmbedLoading) {
      setEmbedRebuildProgress(null);
    }
  }, [rebuildEmbedLoading]);

  useEffect(() => {
    if (!downloadLoading) {
      setDownloadProgress(null);
      return;
    }
    const unlisten = listen<DownloadProgress>('model:download-progress', (event) => {
      setDownloadProgress(event.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, [downloadLoading]);

  useEffect(() => {
    api.getEmbedderConfig().then((cfg) => {
      setEmbedConfig(cfg);
      if (cfg.provider === 'local') {
        api.checkLocalModel(cfg.localModel).then(setLocalModelReady).catch(() => setLocalModelReady(false));
      }
    }).catch(() => {});
  }, []);

  useEffect(() => {
    if (embedConfig?.provider === 'local') {
      api.checkLocalModel(embedConfig.localModel).then(setLocalModelReady).catch(() => setLocalModelReady(false));
    }
  }, [embedConfig?.provider, embedConfig?.localModel]);

  const handleDownloadModel = async () => {
    if (!embedConfig) return;
    setDownloadLoading(true);
    try {
      await api.downloadLocalModel(embedConfig.localModel);
      setLocalModelReady(true);
      toast.success(t('settings.embeddingDownloaded'));
    } catch (e) {
      toast.error(t('settings.embeddingDownloadFail') + ': ' + String(e));
    } finally {
      setDownloadLoading(false);
    }
  };

  const handleTestConnection = async () => {
    if (!embedConfig) return;
    setTestLoading(true);
    try {
      const ok = await api.testApiConnection(embedConfig.apiKey, embedConfig.apiBaseUrl);
      if (ok) {
        toast.success(t('settings.embeddingTestSuccess'));
      } else {
        toast.error(t('settings.embeddingTestFail'));
      }
    } catch {
      toast.error(t('settings.embeddingTestFail'));
    } finally {
      setTestLoading(false);
    }
  };

  const handleSaveEmbedConfig = async () => {
    if (!embedConfig) return;
    setEmbedSaveLoading(true);
    try {
      await api.saveEmbedderConfig(embedConfig);
      toast.success(t('settings.privacySaved'));
    } catch {
      toast.error(t('settings.privacySaveError'));
    } finally {
      setEmbedSaveLoading(false);
    }
  };

  const handleRebuildEmbeddings = async () => {
    setRebuildEmbedLoading(true);
    try {
      await api.rebuildEmbeddings();
      toast.success(t('cmd.rebuildComplete'));
    } catch {
      toast.error(t('cmd.rebuildError'));
    } finally {
      setRebuildEmbedLoading(false);
    }
  };

  useEffect(() => {
    api.getPrivacyConfig().then(setPrivacyConfig).catch(() => {
      toast.error(t('settings.loadPrivacyError'));
    });
  }, []);

  const addPattern = () => {
    const trimmed = newPattern.trim();
    if (!trimmed || !privacyConfig) return;
    if (privacyConfig.excludePatterns.includes(trimmed)) {
      toast.error(t('settings.patternExists'));
      return;
    }
    setPrivacyConfig({
      ...privacyConfig,
      excludePatterns: [...privacyConfig.excludePatterns, trimmed],
    });
    setNewPattern('');
  };

  const removePattern = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      excludePatterns: privacyConfig.excludePatterns.filter((_, i) => i !== idx),
    });
  };

  const addRule = () => {
    if (!newRule.name.trim() || !newRule.pattern.trim() || !privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: [...privacyConfig.redactPatterns, { ...newRule }],
    });
    setNewRule({ name: '', pattern: '', replacement: '' });
  };

  const removeRule = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: privacyConfig.redactPatterns.filter((_, i) => i !== idx),
    });
  };

  const handleSavePrivacy = async () => {
    if (!privacyConfig) return;
    setSaveLoading(true);
    try {
      await api.savePrivacyConfig(privacyConfig);
      toast.success(t('settings.privacySaved'));
    } catch {
      toast.error(t('settings.privacySaveError'));
    } finally {
      setSaveLoading(false);
    }
  };

  /* ── AI Providers state ──────────────────────────────────────────── */
  const [agentConfigs, setAgentConfigs] = useState<AgentConfig[]>([]);
  type ProviderView = 'list' | 'selector' | 'form';
  const [providerView, setProviderView] = useState<ProviderView>('list');
  const [selectedPreset, setSelectedPreset] = useState<ProviderPreset | null>(null);
  const [editingConfig, setEditingConfig] = useState<AgentConfig | undefined>(undefined);
  const [agentSaveLoading, setAgentSaveLoading] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AgentConfig | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);

  const loadAgentConfigs = useCallback(() => {
    api.listAgentConfigs().then(setAgentConfigs).catch(() => {});
  }, []);

  useEffect(() => {
    loadAgentConfigs();
  }, [loadAgentConfigs]);

  const handleSaveAgent = async (input: SaveAgentConfigInput) => {
    setAgentSaveLoading(true);
    try {
      await api.saveAgentConfig(input);
      toast.success(t('settings.providerSaved'));
      setProviderView('list');
      setEditingConfig(undefined);
      setSelectedPreset(null);
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    } finally {
      setAgentSaveLoading(false);
    }
  };

  const handleDeleteAgent = async () => {
    if (!deleteTarget) return;
    setDeleteLoading(true);
    try {
      await api.deleteAgentConfig(deleteTarget.id);
      toast.success(t('settings.providerDeleted'));
      setDeleteTarget(null);
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    } finally {
      setDeleteLoading(false);
    }
  };

  const handleSetDefault = async (id: string) => {
    try {
      await api.setDefaultAgentConfig(id);
      toast.success(t('settings.defaultSet'));
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const PROVIDER_LABELS: Record<string, string> = {
    open_ai: 'OpenAI',
    anthropic: 'Anthropic',
    google: 'Google',
    deep_seek: 'DeepSeek',
    ollama: 'Ollama',
    lm_studio: 'LM Studio',
    azure_open_ai: 'Azure',
    custom: 'Custom',
  };

  const tabs: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
    { id: 'embedding', label: t('settings.embeddingSection'), icon: <Brain size={16} /> },
    { id: 'providers', label: t('settings.aiProviders'), icon: <Bot size={16} /> },
    { id: 'index', label: t('settings.indexSection'), icon: <Database size={16} /> },
    { id: 'privacy', label: t('settings.privacySection'), icon: <Shield size={16} /> },
    { id: 'language', label: t('settings.languageSection'), icon: <Languages size={16} /> },
  ];

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div className="mx-auto max-w-3xl space-y-6 p-6">
      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: -8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25 }}
      >
        <h1 className="text-xl font-bold text-text-primary">{t('settings.title')}</h1>
        <p className="mt-1 text-sm text-text-secondary">{t('settings.subtitle')}</p>
      </motion.div>

      {/* Tab Navigation */}
      <div className="flex gap-1 rounded-lg border border-border bg-surface-1 p-1 overflow-x-auto">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`flex items-center gap-1.5 rounded-md px-3 py-2 text-xs font-medium transition-all duration-fast cursor-pointer whitespace-nowrap ${
              activeTab === tab.id
                ? 'bg-accent text-white shadow-sm'
                : 'text-text-tertiary hover:text-text-secondary hover:bg-surface-2'
            }`}
          >
            {tab.icon}
            {tab.label}
          </button>
        ))}
      </div>

      {/* ── Tab: Embedding ──────────────────────────────────────── */}
      {activeTab === 'embedding' && (
      <Section icon={<Brain size={20} />} title={t('settings.embeddingSection')} delay={0.03}>
        {embedConfig && (
          <div className="space-y-5">
            {/* Provider pills */}
            <div>
              <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.embeddingProvider')}</p>
              <div className="inline-flex rounded-full border border-border bg-surface-1 p-0.5">
                {(['local', 'api', 'tfidf'] as const).map((p) => (
                  <button
                    key={p}
                    onClick={() => setEmbedConfig({ ...embedConfig, provider: p })}
                    className={`rounded-full px-4 py-1.5 text-xs font-medium transition-all duration-fast cursor-pointer ${
                      embedConfig.provider === p
                        ? 'bg-accent text-white shadow-sm'
                        : 'text-text-tertiary hover:text-text-secondary'
                    }`}
                  >
                    {p === 'local' ? t('settings.embeddingLocal') : p === 'api' ? t('settings.embeddingApi') : t('settings.embeddingTfidf')}
                  </button>
                ))}
              </div>
            </div>

            {/* Local model panel */}
            {embedConfig.provider === 'local' && (
              <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-4">
                {/* Model selector */}
                <div>
                  <p className="mb-2 text-sm font-medium text-text-primary">
                    {t('settings.embeddingLocalModelSelect')}
                  </p>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                    {([
                      {
                        id: 'MultilingualMiniLM' as const,
                        label: t('settings.embeddingModelLight'),
                        desc: t('settings.embeddingModelLightDesc'),
                      },
                      {
                        id: 'MultilingualE5Base' as const,
                        label: t('settings.embeddingModelQuality'),
                        desc: t('settings.embeddingModelQualityDesc'),
                      },
                    ]).map((opt) => (
                      <button
                        key={opt.id}
                        onClick={() => {
                          setEmbedConfig({ ...embedConfig, localModel: opt.id });
                          setLocalModelReady(null);
                        }}
                        className={`rounded-lg border p-3 text-left transition-all duration-fast cursor-pointer ${
                          embedConfig.localModel === opt.id
                            ? 'border-accent bg-accent-subtle ring-1 ring-accent/20'
                            : 'border-border bg-surface-1 hover:border-border-hover hover:bg-surface-3/50'
                        }`}
                      >
                        <div className="text-sm font-medium text-text-primary">{opt.label}</div>
                        <div className="mt-1 text-xs text-text-tertiary">{opt.desc}</div>
                      </button>
                    ))}
                  </div>
                  <div className="mt-2 flex items-start gap-2 rounded-lg border border-info/30 bg-info/5 p-2">
                    <AlertTriangle size={14} className="mt-0.5 shrink-0 text-info" />
                    <p className="text-xs text-info">{t('settings.embeddingModelChangeWarning')}</p>
                  </div>
                </div>

                {/* Download status */}
                <div className="flex items-center gap-2 text-sm">
                  <span className="text-text-secondary">{t('settings.embeddingLocalStatus')}:</span>
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
                {localModelReady === false && (
                  <Button
                    variant="secondary"
                    size="sm"
                    icon={downloadLoading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                    loading={downloadLoading}
                    onClick={handleDownloadModel}
                  >
                    {downloadLoading ? t('settings.embeddingDownloading') : t('settings.embeddingDownload')}
                  </Button>
                )}
                {downloadLoading && downloadProgress && (
                  <div className="mt-2">
                    <div className="flex items-center gap-2 text-xs text-text-tertiary mb-1">
                      <Loader2 size={12} className="animate-spin" />
                      <span>
                        {t('settings.downloadingFile', {
                          filename: downloadProgress.filename,
                          current: String(downloadProgress.fileIndex + 1),
                          total: String(downloadProgress.totalFiles),
                        })}
                      </span>
                    </div>
                    {downloadProgress.totalBytes ? (
                      <>
                        <div className="flex justify-between text-[10px] text-text-tertiary/70 mb-0.5">
                          <span>{formatBytes(downloadProgress.bytesDownloaded)} / {formatBytes(downloadProgress.totalBytes)}</span>
                          <span>{Math.round((downloadProgress.bytesDownloaded / downloadProgress.totalBytes) * 100)}%</span>
                        </div>
                        <div className="w-full bg-surface-3 rounded h-1.5">
                          <div
                            className="bg-accent h-1.5 rounded transition-all duration-300"
                            style={{ width: `${Math.min(100, (downloadProgress.bytesDownloaded / downloadProgress.totalBytes) * 100)}%` }}
                          />
                        </div>
                      </>
                    ) : (
                      <div className="w-full bg-surface-3 rounded h-1.5 overflow-hidden">
                        <div className="bg-accent h-1.5 rounded animate-pulse w-full" />
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}

            {/* API panel */}
            {embedConfig.provider === 'api' && (
              <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingApiKey')}</label>
                  <div className="relative">
                    <KeyRound size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
                    <Input
                      type="password"
                      value={embedConfig.apiKey}
                      onChange={(e) => setEmbedConfig({ ...embedConfig, apiKey: e.target.value })}
                      className="pl-9"
                      placeholder="sk-..."
                    />
                  </div>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingBaseUrl')}</label>
                  <Input
                    value={embedConfig.apiBaseUrl}
                    onChange={(e) => setEmbedConfig({ ...embedConfig, apiBaseUrl: e.target.value })}
                    placeholder="https://api.openai.com/v1"
                  />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingModel')}</label>
                  <Input
                    value={embedConfig.apiModel}
                    onChange={(e) => setEmbedConfig({ ...embedConfig, apiModel: e.target.value })}
                    placeholder="text-embedding-3-small"
                  />
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  icon={testLoading ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
                  loading={testLoading}
                  onClick={handleTestConnection}
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
                  onClick={handleSaveEmbedConfig}
                >
                  {t('settings.embeddingSave')}
                </Button>
                <Button
                  variant="secondary"
                  size="md"
                  icon={rebuildEmbedLoading ? <Loader2 size={16} className="animate-spin" /> : <RefreshCw size={16} />}
                  loading={rebuildEmbedLoading}
                  onClick={handleRebuildEmbeddings}
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
      )}

      {/* ── Tab: AI Providers ──────────────────────────────────────── */}
      {activeTab === 'providers' && (
      <Section icon={<Bot size={20} />} title={t('settings.aiProviders')} delay={0.03}>
        {providerView === 'form' ? (
          <AgentConfigForm
            config={editingConfig}
            preset={editingConfig ? undefined : selectedPreset}
            onSave={handleSaveAgent}
            onCancel={() => { setProviderView('list'); setEditingConfig(undefined); setSelectedPreset(null); }}
            isSaving={agentSaveLoading}
          />
        ) : providerView === 'selector' ? (
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <h3 className="text-lg font-medium text-text-primary">{t('settings.selectProvider')}</h3>
              <button
                onClick={() => setProviderView('list')}
                className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm text-text-tertiary hover:text-text-secondary hover:bg-surface-3/50 transition-colors cursor-pointer"
              >
                <X size={16} /> {t('common.cancel')}
              </button>
            </div>
            <div className="grid grid-cols-2 gap-3">
              {PROVIDER_PRESETS.map(preset => (
                <button
                  key={preset.id}
                  onClick={() => { setSelectedPreset(preset); setProviderView('form'); }}
                  className="flex items-start gap-3 rounded-lg border border-border bg-surface-2 p-4 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 cursor-pointer"
                >
                  <span className="text-2xl">{preset.icon}</span>
                  <div>
                    <div className="font-medium text-text-primary">{preset.name}</div>
                    <div className="text-sm text-text-tertiary">{preset.description}</div>
                  </div>
                </button>
              ))}
              {/* Custom / Manual option */}
              <button
                onClick={() => { setSelectedPreset(null); setProviderView('form'); }}
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
                onClick={() => { setEditingConfig(undefined); setSelectedPreset(null); setProviderView('selector'); }}
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
              <div className="space-y-3">
                {agentConfigs.map((cfg) => (
                  <div
                    key={cfg.id}
                    className="flex items-center justify-between rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                  >
                    <div className="flex items-center gap-3 min-w-0">
                      {cfg.isDefault && (
                        <Star size={14} className="shrink-0 fill-warning text-warning" />
                      )}
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <p className="text-sm font-medium text-text-primary truncate">{cfg.name}</p>
                          <Badge variant="default" className="text-[10px] shrink-0">
                            {PROVIDER_LABELS[cfg.provider] ?? cfg.provider}
                          </Badge>
                        </div>
                        <p className="mt-0.5 text-xs text-text-tertiary truncate">
                          {cfg.model}
                          {cfg.baseUrl ? ` · ${cfg.baseUrl}` : ''}
                        </p>
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0 ml-3">
                      {!cfg.isDefault && (
                        <button
                          onClick={() => handleSetDefault(cfg.id)}
                          className="rounded p-1.5 text-text-tertiary hover:text-warning hover:bg-warning/10 transition-colors cursor-pointer"
                          aria-label={t('settings.setDefault')}
                          title={t('settings.setDefault')}
                        >
                          <Star size={14} />
                        </button>
                      )}
                      <button
                        onClick={() => { setEditingConfig(cfg); setProviderView('form'); }}
                        className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                        aria-label={t('common.edit')}
                        title={t('common.edit')}
                      >
                        <Pencil size={14} />
                      </button>
                      <button
                        onClick={() => setDeleteTarget(cfg)}
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
            )}
          </div>
        )}
      </Section>
      )}

      {/* Delete confirm dialog */}
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDeleteAgent}
        title={t('settings.deleteProvider')}
        message={t('settings.deleteProviderConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
        loading={deleteLoading}
      />

      {/* ── Tab: Index ─────────────────────────────────────────────── */}
      {activeTab === 'index' && (
      <Section icon={<Database size={20} />} title={t('settings.indexSection')} delay={0.05}>
        {/* Stats grid */}
        <div className="mb-5 grid grid-cols-3 gap-3">
          <StatCard label={t('settings.totalDocs')} value={stats?.totalDocuments ?? '—'} />
          <StatCard label={t('settings.totalChunks')} value={stats?.totalChunks ?? '—'} />
          <StatCard label={t('settings.ftsEntries')} value={stats?.ftsRows ?? '—'} />
        </div>

        {/* Actions */}
        <div className="flex items-center gap-3">
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            loading={rebuildLoading}
            onClick={handleRebuild}
          >
            {t('settings.rebuildIndex')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<Zap size={14} />}
            loading={optimizeLoading}
            onClick={handleOptimize}
          >
            {t('settings.optimizeIndex')}
          </Button>
        </div>
        {ftsProgress && (
          <div className="mt-2">
            <div className="flex items-center gap-2 text-xs text-muted">
              <RefreshCw size={12} className="animate-spin" />
              <span>{ftsProgress.operation === 'rebuild-fts' ? t('settings.rebuildingIndex') : t('settings.optimizingIndex')}</span>
            </div>
            <div className="w-full bg-surface-3 rounded h-1 mt-1 overflow-hidden">
              <div className="bg-accent h-1 rounded animate-pulse w-full" />
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: Privacy ───────────────────────────────────────────── */}
      {activeTab === 'privacy' && (
      <Section icon={<Shield size={20} />} title={t('settings.privacySection')} delay={0.1}>
        {privacyConfig && (
          <div className="space-y-6">
            {/* Exclude patterns */}
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.excludePatterns')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.excludePatternsDesc')}</p>

              {/* Pattern chips */}
              {privacyConfig.excludePatterns.length > 0 && (
                <div className="mb-3 flex flex-wrap gap-2">
                  {privacyConfig.excludePatterns.map((pat, i) => (
                    <Badge key={i} variant="default" className="gap-1.5 pl-2.5 pr-1.5 py-1">
                      <span className="font-mono text-[11px]">{pat}</span>
                      <button
                        onClick={() => removePattern(i)}
                        className="ml-0.5 rounded hover:bg-surface-4 p-0.5 text-text-tertiary hover:text-danger transition-colors cursor-pointer"
                        aria-label={`${t('common.remove')} ${pat}`}
                      >
                        <Trash2 size={12} />
                      </button>
                    </Badge>
                  ))}
                </div>
              )}

              {/* Add pattern */}
              <div className="flex gap-2">
                <Input
                  placeholder="*.log, .git/**, node_modules/**"
                  value={newPattern}
                  onChange={(e) => setNewPattern(e.target.value)}
                  onKeyDown={(e) => { if (e.key === 'Enter') addPattern(); }}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="md"
                  icon={<Plus size={16} />}
                  onClick={addPattern}
                  disabled={!newPattern.trim()}
                >
                  {t('settings.addPattern')}
                </Button>
              </div>
            </div>

            {/* Redaction rules */}
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.redactRules')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.redactRulesDesc')}</p>

              {/* Rules table */}
              {privacyConfig.redactPatterns.length > 0 && (
                <div className="mb-3 overflow-hidden rounded-lg border border-border">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-border bg-surface-2">
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleName')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.rulePattern')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleReplacement')}</th>
                        <th className="w-10 px-3 py-2" />
                      </tr>
                    </thead>
                    <tbody>
                      {privacyConfig.redactPatterns.map((rule, i) => (
                        <tr
                          key={i}
                          className="border-b border-border last:border-0 hover:bg-surface-2/50 transition-colors"
                        >
                          <td className="px-3 py-2 text-text-primary">{rule.name}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.pattern}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.replacement}</td>
                          <td className="px-3 py-2 text-right">
                            <button
                              onClick={() => removeRule(i)}
                              className="rounded p-1 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                              aria-label={`${t('common.delete')} ${rule.name}`}
                            >
                              <Trash2 size={14} />
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}

              {/* Add rule form */}
              <div className="flex gap-2">
                <Input
                  placeholder={t('settings.ruleName')}
                  value={newRule.name}
                  onChange={(e) => setNewRule({ ...newRule, name: e.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.rulePattern')}
                  value={newRule.pattern}
                  onChange={(e) => setNewRule({ ...newRule, pattern: e.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.ruleReplacement')}
                  value={newRule.replacement}
                  onChange={(e) => setNewRule({ ...newRule, replacement: e.target.value })}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="md"
                  icon={<Plus size={16} />}
                  onClick={addRule}
                  disabled={!newRule.name.trim() || !newRule.pattern.trim()}
                >
                  {t('settings.addRule')}
                </Button>
              </div>
            </div>

            {/* Save button */}
            <div className="flex justify-end border-t border-border pt-4">
              <Button
                variant="primary"
                size="md"
                icon={<Save size={16} />}
                loading={saveLoading}
                onClick={handleSavePrivacy}
              >
                {t('settings.saveConfig')}
              </Button>
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: Language ──────────────────────────────────────────── */}
      {activeTab === 'language' && (
      <Section icon={<Languages size={20} />} title={t('settings.languageSection')} delay={0.15}>
        <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-5 gap-2">
          {availableLocales.map((l) => (
            <button
              key={l.code}
              onClick={() => setLocale(l.code)}
              className={`rounded-lg border px-3 py-2.5 text-sm font-medium transition-all duration-fast cursor-pointer ${
                locale === l.code
                  ? 'border-accent bg-accent-subtle text-accent ring-1 ring-accent/20'
                  : 'border-border bg-surface-2 text-text-secondary hover:border-border-hover hover:bg-surface-3'
              }`}
            >
              {l.name}
            </button>
          ))}
        </div>
      </Section>
      )}
    </div>
  );
}
