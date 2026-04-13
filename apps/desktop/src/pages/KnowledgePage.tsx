import { useState, useEffect, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Brain,
  FileText,
  Activity,
  Search,
  RefreshCw,
  AlertTriangle,
  Info,
  AlertCircle,
  Network,
  Layers,
  CheckCircle2,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import { useProgress, progressStore } from '../lib/progressStore';
import type {
  CompileStats,
  CompileResult,
  KnowledgeMap,
  Entity,
  HealthReport,
  HealthIssue,
  Severity,
  CheckType,
} from '../types/knowledge';
import { useTranslation } from '../i18n';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';
import { CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';

/* ── Constants ─────────────────────────────────────────────────────── */

type Tab = 'compile' | 'map' | 'health';

const listContainer = {
  hidden: {},
  show: { transition: { staggerChildren: 0.06 } },
};

const listItem = {
  hidden: { opacity: 0, y: 12 },
  show: { opacity: 1, y: 0, transition: { duration: 0.25, ease: [0.16, 1, 0.3, 1] as const } },
};

function severityVariant(s: Severity): 'info' | 'warning' | 'danger' {
  if (s === 'critical') return 'danger';
  if (s === 'warning') return 'warning';
  return 'info';
}

function checkTypeIcon(ct: CheckType) {
  switch (ct) {
    case 'stale': return <AlertTriangle size={14} />;
    case 'orphan': return <FileText size={14} />;
    case 'duplicate': return <Layers size={14} />;
    case 'gap': return <AlertCircle size={14} />;
    case 'contradiction': return <AlertCircle size={14} />;
  }
}

/* ── Component ─────────────────────────────────────────────────────── */

export function KnowledgePage() {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<Tab>('compile');

  // Compile state
  const [stats, setStats] = useState<CompileStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(true);
  const [compiling, setCompiling] = useState(false);
  const progress = useProgress();
  const compileProgress = progress.compileProgress;
  const [compileResults, setCompileResults] = useState<CompileResult[]>([]);

  // Map state
  const [knowledgeMap, setKnowledgeMap] = useState<KnowledgeMap | null>(null);
  const [mapLoading, setMapLoading] = useState(false);
  const [entitySearch, setEntitySearch] = useState('');

  // Health state
  const [healthReport, setHealthReport] = useState<HealthReport | null>(null);
  const [healthLoading, setHealthLoading] = useState(false);

  /* ── Data fetchers ─────────────────────────────────────────────── */

  const loadStats = useCallback(async () => {
    setStatsLoading(true);
    try {
      const s = await api.getCompileStats();
      setStats(s);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setStatsLoading(false);
    }
  }, []);

  const loadMap = useCallback(async () => {
    setMapLoading(true);
    try {
      const m = await api.getKnowledgeMap(100);
      setKnowledgeMap(m);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setMapLoading(false);
    }
  }, []);

  const handleCompile = useCallback(async () => {
    setCompiling(true);
    progressStore.update('compileProgress', null);
    try {
      const results = await api.compilePendingDocuments(20);
      setCompileResults(results);
      toast.success(`Processed ${results.length} documents`);
      await loadStats();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setCompiling(false);
      progressStore.update('compileProgress', null);
    }
  }, [loadStats]);

  const handleHealthCheck = useCallback(async () => {
    setHealthLoading(true);
    try {
      const report = await api.runKnowledgeHealthCheck();
      setHealthReport(report);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setHealthLoading(false);
    }
  }, []);

  /* ── Load data on tab change ───────────────────────────────────── */

  useEffect(() => {
    if (activeTab === 'compile') loadStats();
    if (activeTab === 'map') loadMap();
  }, [activeTab, loadStats, loadMap]);

  /* ── Filtered entities ─────────────────────────────────────────── */

  const filteredEntities: Entity[] =
    knowledgeMap?.entities.filter((e: Entity) =>
      entitySearch === '' ||
      e.name.toLowerCase().includes(entitySearch.toLowerCase()) ||
      e.entityType.toLowerCase().includes(entitySearch.toLowerCase())
    ) ?? [];

  /* ── Grouped health issues ─────────────────────────────────────── */

  const allIssues: HealthIssue[] = healthReport
    ? [
        ...healthReport.staleDocuments,
        ...healthReport.orphanDocuments,
        ...healthReport.lowCoverageEntities,
        ...healthReport.duplicateCandidates,
      ]
    : [];

  /* ── Tab buttons ───────────────────────────────────────────────── */

  const tabs: { key: Tab; label: string; icon: typeof FileText }[] = [
    { key: 'compile', label: t('knowledge.compile'), icon: FileText },
    { key: 'map', label: t('knowledge.knowledgeMap'), icon: Network },
    { key: 'health', label: t('knowledge.healthCheck'), icon: Activity },
  ];

  /* ── Progress percentage ───────────────────────────────────────── */

  const progressPct = stats && stats.totalDocs > 0
    ? Math.round((stats.compiledDocs / stats.totalDocs) * 100)
    : 0;

  return (
    <div className="flex h-full flex-col min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-border shrink-0">
        <div className="flex items-center gap-2.5">
          <Brain size={20} className="text-accent" />
          <h2 className="text-base font-semibold text-text-primary">{t('knowledge.title')}</h2>
        </div>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 px-6 pt-3 pb-0 shrink-0">
        {tabs.map(({ key, label, icon: Icon }) => (
          <button
            key={key}
            onClick={() => setActiveTab(key)}
            className={`flex items-center gap-1.5 px-3 py-2 text-sm rounded-md transition-colors duration-fast ease-out
              ${activeTab === key
                ? 'bg-accent-subtle text-accent-hover font-medium'
                : 'text-text-secondary hover:bg-surface-2 hover:text-text-primary'
              }`}
          >
            <Icon size={15} />
            {label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-4">
        <AnimatePresence mode="wait">
          {activeTab === 'compile' && (
            <motion.div
              key="compile"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.15 }}
            >
              {statsLoading ? (
                <div className="space-y-3">
                  <CardSkeleton />
                  <CardSkeleton />
                </div>
              ) : stats ? (
                <div className="space-y-5">
                  {/* Stats cards */}
                  <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                    <StatCard label={t('knowledge.totalDocs')} value={stats.totalDocs} />
                    <StatCard label={t('knowledge.compiledDocs')} value={stats.compiledDocs} />
                    <StatCard label={t('knowledge.totalEntities')} value={stats.totalEntities} />
                    <StatCard label={t('knowledge.totalLinks')} value={stats.totalLinks} />
                  </div>

                  {/* Progress bar */}
                  <div className="space-y-2">
                    <div className="flex items-center justify-between text-xs text-text-secondary">
                      <span>{stats.compiledDocs} / {stats.totalDocs}</span>
                      <span>{progressPct}%</span>
                    </div>
                    <div className="h-2 rounded-full bg-surface-3 overflow-hidden">
                      <motion.div
                        className="h-full rounded-full bg-accent"
                        initial={{ width: 0 }}
                        animate={{ width: `${progressPct}%` }}
                        transition={{ duration: 0.5, ease: [0.16, 1, 0.3, 1] }}
                      />
                    </div>
                  </div>

                  {/* Compile button */}
                  <Button
                    variant="primary"
                    size="md"
                    loading={compiling}
                    icon={<RefreshCw size={15} />}
                    onClick={handleCompile}
                    disabled={compiling || stats.compiledDocs >= stats.totalDocs}
                  >
                    {compiling ? t('knowledge.compiling') : t('knowledge.compilePending')}
                  </Button>

                  {/* Compile progress detail */}
                  {compiling && compileProgress && compileProgress.total > 0 && (
                    <div className="p-3 rounded-lg bg-surface-2 border border-border space-y-2">
                      <div className="flex items-center justify-between text-xs text-text-secondary">
                        <span className="flex items-center gap-1.5">
                          {compileProgress.phase === 'error' ? (
                            <AlertTriangle size={12} className="text-danger" />
                          ) : compileProgress.phase === 'timeout' ? (
                            <AlertCircle size={12} className="text-warning" />
                          ) : (
                            <RefreshCw size={12} className="animate-spin text-accent" />
                          )}
                          <span className="font-medium">
                            {compileProgress.phase === 'error'
                              ? t('knowledge.compilePhase.error')
                              : compileProgress.phase === 'timeout'
                                ? t('knowledge.compilePhase.timeout')
                                : t('knowledge.compilePhase.compiling')}
                          </span>
                          <span className="text-text-tertiary">
                            {t('knowledge.compileProgress', { current: compileProgress.current, total: compileProgress.total })}
                          </span>
                        </span>
                        <span className="text-[11px] font-medium text-accent">
                          {Math.round((compileProgress.current / compileProgress.total) * 100)}%
                        </span>
                      </div>
                      {(compileProgress.documentTitle || compileProgress.documentId) && (
                        <div className="text-[10px] text-text-tertiary truncate max-w-sm">
                          {compileProgress.documentTitle || compileProgress.documentId}
                        </div>
                      )}
                      <div className="w-full bg-surface-3 rounded-full h-2">
                        <div
                          className={`h-2 rounded-full transition-all duration-300 ease-out ${
                            compileProgress.phase === 'error' ? 'bg-danger' :
                            compileProgress.phase === 'timeout' ? 'bg-warning' : 'bg-accent'
                          }`}
                          style={{ width: `${Math.min(100, (compileProgress.current / compileProgress.total) * 100)}%` }}
                        />
                      </div>
                    </div>
                  )}

                  {/* Recent compile results */}
                  {compileResults.length > 0 && (
                    <motion.div
                      variants={listContainer}
                      initial="hidden"
                      animate="show"
                      className="space-y-2"
                    >
                      {compileResults.map((r: CompileResult) => (
                        <motion.div
                          key={r.documentId}
                          variants={listItem}
                          className="flex items-center justify-between p-3 rounded-lg border border-border bg-surface-1"
                        >
                          <div className="flex items-center gap-2 min-w-0">
                            <CheckCircle2 size={14} className="text-success shrink-0" />
                            <span className="text-sm text-text-primary truncate">
                              Doc #{r.documentId}
                            </span>
                          </div>
                          <div className="flex items-center gap-2 shrink-0">
                            <Badge variant="info">{r.entitiesFound} topics</Badge>
                            <Badge variant="default">{r.linksCreated} connections</Badge>
                          </div>
                        </motion.div>
                      ))}
                    </motion.div>
                  )}
                </div>
              ) : null}
            </motion.div>
          )}

          {activeTab === 'map' && (
            <motion.div
              key="map"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.15 }}
              className="space-y-4"
            >
              {/* Search */}
              <Input
                icon={<Search size={15} />}
                placeholder={t('knowledge.searchEntities')}
                value={entitySearch}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => setEntitySearch(e.target.value)}
              />

              {mapLoading ? (
                <div className="space-y-3">
                  <CardSkeleton />
                  <CardSkeleton />
                  <CardSkeleton />
                </div>
              ) : filteredEntities.length === 0 ? (
                <EmptyState
                  icon={<Network size={32} />}
                  title={t('knowledge.noEntities')}
                  description=""
                />
              ) : (
                <div className="border border-border rounded-lg overflow-hidden">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="bg-surface-2 text-text-secondary text-left">
                        <th className="px-4 py-2.5 font-medium">Entity</th>
                        <th className="px-4 py-2.5 font-medium">{t('knowledge.entityType')}</th>
                        <th className="px-4 py-2.5 font-medium text-right">{t('knowledge.mentions')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredEntities.map((entity) => (
                        <tr
                          key={entity.id}
                          className="border-t border-border hover:bg-surface-1 transition-colors"
                        >
                          <td className="px-4 py-2.5">
                            <div>
                              <span className="text-text-primary font-medium">{entity.name}</span>
                              {entity.description && (
                                <p className="text-xs text-text-tertiary mt-0.5 line-clamp-1">
                                  {entity.description}
                                </p>
                              )}
                            </div>
                          </td>
                          <td className="px-4 py-2.5">
                            <Badge variant="default">{entity.entityType}</Badge>
                          </td>
                          <td className="px-4 py-2.5 text-right text-text-secondary">
                            {entity.mentionCount}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}

              {/* Summary */}
              {knowledgeMap && !mapLoading && (
                <div className="flex gap-4 text-xs text-text-tertiary">
                  <span>{t('knowledge.totalEntities')}: {knowledgeMap.totalEntities}</span>
                  <span>{t('knowledge.totalLinks')}: {knowledgeMap.totalLinks}</span>
                </div>
              )}
            </motion.div>
          )}

          {activeTab === 'health' && (
            <motion.div
              key="health"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.15 }}
              className="space-y-4"
            >
              {/* Run check button */}
              <Button
                variant="secondary"
                size="md"
                loading={healthLoading}
                icon={<Activity size={15} />}
                onClick={handleHealthCheck}
                disabled={healthLoading}
              >
                {healthLoading ? t('knowledge.checking') : t('knowledge.runCheck')}
              </Button>

              {healthLoading ? (
                <div className="space-y-3">
                  <CardSkeleton />
                  <CardSkeleton />
                </div>
              ) : healthReport ? (
                allIssues.length === 0 ? (
                  <EmptyState
                    icon={<CheckCircle2 size={32} />}
                    title={t('knowledge.noIssues')}
                    description=""
                  />
                ) : (
                  <motion.div
                    variants={listContainer}
                    initial="hidden"
                    animate="show"
                    className="space-y-2"
                  >
                    {allIssues.map((issue) => (
                      <motion.div
                        key={issue.id}
                        variants={listItem}
                        className="p-3 rounded-lg border border-border bg-surface-1 space-y-1.5"
                      >
                        <div className="flex items-center gap-2">
                          {checkTypeIcon(issue.checkType)}
                          <Badge variant={severityVariant(issue.severity)}>
                            {t(`knowledge.${issue.severity}`)}
                          </Badge>
                          <Badge variant="default">
                            {t(`knowledge.${issue.checkType}`)}
                          </Badge>
                        </div>
                        <p className="text-sm text-text-primary">{issue.description}</p>
                        {issue.suggestion && (
                          <p className="text-xs text-text-tertiary flex items-start gap-1">
                            <Info size={12} className="shrink-0 mt-0.5" />
                            {issue.suggestion}
                          </p>
                        )}
                      </motion.div>
                    ))}
                  </motion.div>
                )
              ) : null}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

/* ── Stat Card ─────────────────────────────────────────────────────── */

function StatCard({ label, value }: { label: string; value: number }) {
  return (
    <div className="p-3 rounded-lg border border-border bg-surface-1">
      <p className="text-xs text-text-tertiary mb-1">{label}</p>
      <p className="text-lg font-semibold text-text-primary">{value}</p>
    </div>
  );
}
