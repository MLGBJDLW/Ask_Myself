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
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { IndexStats } from '../types/index-stats';
import type { PrivacyConfig, RedactRule } from '../types/privacy';
import type { EmbedderConfig } from '../types/embedder';
import { useTranslation } from '../i18n';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';

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

/* ── Settings page ────────────────────────────────────────────────── */
export function SettingsPage() {
  const { t, locale, setLocale, availableLocales } = useTranslation();
  /* ── Index state ─────────────────────────────────────────────────── */
  const [stats, setStats] = useState<IndexStats | null>(null);
  const [rebuildLoading, setRebuildLoading] = useState(false);
  const [optimizeLoading, setOptimizeLoading] = useState(false);

  const loadStats = useCallback(() => {
    api.getIndexStats().then(setStats).catch(() => {
      toast.error(t('settings.loadStatsError'));
    });
  }, []);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

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
  const [testLoading, setTestLoading] = useState(false);
  const [embedSaveLoading, setEmbedSaveLoading] = useState(false);
  const [rebuildEmbedLoading, setRebuildEmbedLoading] = useState(false);

  useEffect(() => {
    api.getEmbedderConfig().then((cfg) => {
      setEmbedConfig(cfg);
      if (cfg.provider === 'local') {
        api.checkLocalModel().then(setLocalModelReady).catch(() => setLocalModelReady(false));
      }
    }).catch(() => {});
  }, []);

  useEffect(() => {
    if (embedConfig?.provider === 'local') {
      api.checkLocalModel().then(setLocalModelReady).catch(() => setLocalModelReady(false));
    }
  }, [embedConfig?.provider]);

  const handleDownloadModel = async () => {
    setDownloadLoading(true);
    try {
      await api.downloadLocalModel();
      setLocalModelReady(true);
      toast.success(t('settings.embeddingDownloaded'));
    } catch {
      toast.error(t('settings.embeddingTestFail'));
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

      {/* ── Section 0: AI Embedding ─────────────────────────────────── */}
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
              <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
                <p className="text-sm text-text-primary">
                  {t('settings.embeddingLocalModel')}
                </p>
                <div className="flex items-center gap-2 text-sm">
                  <span className="text-text-secondary">{t('settings.embeddingLocalStatus')}:</span>
                  {localModelReady === null ? (
                    <Loader2 size={14} className="animate-spin text-text-tertiary" />
                  ) : localModelReady ? (
                    <Badge variant="default" className="gap-1">
                      <CheckCircle size={12} className="text-green-500" />
                      {t('settings.embeddingDownloaded')}
                    </Badge>
                  ) : (
                    <Badge variant="default" className="gap-1">
                      <XCircle size={12} className="text-red-400" />
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
              <div className="flex items-start gap-2 rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-3">
                <AlertTriangle size={16} className="mt-0.5 shrink-0 text-yellow-500" />
                <p className="text-sm text-yellow-600 dark:text-yellow-400">{t('settings.embeddingTfidfWarning')}</p>
              </div>
            )}

            {/* Provider change warning + actions */}
            <div className="space-y-3 border-t border-border pt-4">
              <div className="flex items-start gap-2 rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-3">
                <AlertTriangle size={16} className="mt-0.5 shrink-0 text-yellow-500" />
                <p className="text-sm text-yellow-600 dark:text-yellow-400">{t('settings.embeddingProviderChangeWarning')}</p>
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
            </div>
          </div>
        )}
      </Section>

      {/* ── Section 1: 索引管理 ──────────────────────────────────────── */}
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
      </Section>

      {/* ── Section 2: 隐私配置 ──────────────────────────────────────── */}
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

      {/* ── Section 3: Language ──────────────────────────────────────── */}
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
    </div>
  );
}
