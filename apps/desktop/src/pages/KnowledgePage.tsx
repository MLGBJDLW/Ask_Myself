import { useState, useEffect, useCallback, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Brain,
  FileText,
  Activity,
  RefreshCw,
  AlertTriangle,
  Info,
  AlertCircle,
  Network,
  Layers,
  CheckCircle2,
  Sparkles,
  Lightbulb,
  Check,
  X,
  RotateCcw,
  MessageSquare,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import { useProgress, progressStore } from '../lib/progressStore';
import type {
  CompileStats,
  CompileResult,
  HealthReport,
  HealthIssue,
  Severity,
  CheckType,
} from '../types/knowledge';
import { useTranslation, type TranslationKey } from '../i18n';
import { Button } from '../components/ui/Button';
import { Badge, type BadgeVariant } from '../components/ui/Badge';
import { CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { formatUserError } from '../lib/userError';
import { KnowledgeGraphView } from '../components/knowledge/KnowledgeGraphView';

/* ── Constants ─────────────────────────────────────────────────────── */

type Tab = 'compile' | 'map' | 'health' | 'insights';

interface DreamArtifactDraft {
  title: string;
  summary: string;
  payloadJson: string;
  evidenceJson: string;
  confidence: string;
}

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
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState<Tab>('compile');

  // Compile state
  const [stats, setStats] = useState<CompileStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(true);
  const [compiling, setCompiling] = useState(false);
  const progress = useProgress();
  const compileProgress = progress.compileProgress;
  const [compileResults, setCompileResults] = useState<CompileResult[]>([]);

  // Health state
  const [healthReport, setHealthReport] = useState<HealthReport | null>(null);
  const [healthLoading, setHealthLoading] = useState(false);

  // Dreaming / Insights state
  const [dreamRuns, setDreamRuns] = useState<api.DreamRun[]>([]);
  const [dreamArtifacts, setDreamArtifacts] = useState<api.DreamArtifact[]>([]);
  const [dreamLoading, setDreamLoading] = useState(false);
  const [dreamStarting, setDreamStarting] = useState(false);
  const [artifactActionId, setArtifactActionId] = useState<string | null>(null);
  const [editingArtifactId, setEditingArtifactId] = useState<string | null>(null);
  const [artifactDraft, setArtifactDraft] = useState<DreamArtifactDraft | null>(null);

  /* ── Data fetchers ─────────────────────────────────────────────── */

  const loadStats = useCallback(async () => {
    setStatsLoading(true);
    try {
      const s = await api.getCompileStats();
      setStats(s);
    } catch (e) {
      toast.error(formatUserError(t('knowledge.compileStats'), e));
    } finally {
      setStatsLoading(false);
    }
  }, [t]);

  const handleCompile = useCallback(async () => {
    setCompiling(true);
    progressStore.update('compileProgress', null);
    try {
      const results = await api.compilePendingDocuments(20);
      setCompileResults(results);
      toast.success(`${t('knowledge.compiledDocs')}: ${results.length}`);
      await loadStats();
    } catch (e) {
      toast.error(formatUserError(t('knowledge.compilePending'), e));
    } finally {
      setCompiling(false);
      progressStore.update('compileProgress', null);
    }
  }, [loadStats, t]);

  const handleHealthCheck = useCallback(async () => {
    setHealthLoading(true);
    try {
      const report = await api.runKnowledgeHealthCheck();
      setHealthReport(report);
    } catch (e) {
      toast.error(formatUserError(t('knowledge.healthCheck'), e));
    } finally {
      setHealthLoading(false);
    }
  }, [t]);

  const loadDreaming = useCallback(async () => {
    setDreamLoading(true);
    try {
      const [runs, artifacts] = await Promise.all([
        api.listDreamRuns(5),
        api.listDreamArtifacts({ limit: 50 }),
      ]);
      setDreamRuns(runs);
      setDreamArtifacts(artifacts);
    } catch (e) {
      toast.error(formatUserError(t('knowledge.dreamingLoad'), e));
    } finally {
      setDreamLoading(false);
    }
  }, [t]);

  const handleStartDream = useCallback(async () => {
    setDreamStarting(true);
    try {
      await api.startDream({
        triggerKind: 'manual',
        scopeJson: { surface: 'knowledge.insights' },
      });
      toast.success(t('knowledge.dreamingStarted'));
      await loadDreaming();
    } catch (e) {
      toast.error(formatUserError(t('knowledge.dreamingStart'), e));
    } finally {
      setDreamStarting(false);
    }
  }, [loadDreaming, t]);

  const handleArtifactAction = useCallback(async (artifact: api.DreamArtifact, action: 'apply' | 'reject' | 'undo') => {
    setArtifactActionId(artifact.id);
    try {
      if (action === 'apply') {
        await api.applyDreamArtifact(artifact.id);
        toast.success(t('knowledge.dreamingApplied'));
      } else if (action === 'reject') {
        await api.rejectDreamArtifact(artifact.id);
        toast.success(t('knowledge.dreamingRejected'));
      } else {
        await api.undoDreamArtifact(artifact.id);
        toast.success(t('knowledge.dreamingUndone'));
      }
      await loadDreaming();
    } catch (e) {
      toast.error(formatUserError(t(action === 'apply' ? 'knowledge.dreamingApply' : action === 'reject' ? 'knowledge.dreamingReject' : 'knowledge.dreamingUndo'), e));
    } finally {
      setArtifactActionId(null);
    }
  }, [loadDreaming, t]);

  const beginArtifactEdit = useCallback((artifact: api.DreamArtifact) => {
    setEditingArtifactId(artifact.id);
    setArtifactDraft({
      title: artifact.title,
      summary: artifact.summary,
      payloadJson: formatArtifactJson(artifact.payloadJson),
      evidenceJson: formatArtifactJson(artifact.evidenceJson),
      confidence: String(Math.round(artifact.confidence * 100)),
    });
  }, []);

  const cancelArtifactEdit = useCallback(() => {
    setEditingArtifactId(null);
    setArtifactDraft(null);
  }, []);

  const saveArtifactEdit = useCallback(async (artifact: api.DreamArtifact) => {
    if (!artifactDraft) return;
    let payloadJson: unknown;
    let evidenceJson: unknown;
    try {
      payloadJson = JSON.parse(artifactDraft.payloadJson);
      evidenceJson = JSON.parse(artifactDraft.evidenceJson);
    } catch {
      toast.error(t('knowledge.dreamingInvalidJson'));
      return;
    }

    setArtifactActionId(artifact.id);
    try {
      await api.updateDreamArtifact(artifact.id, {
        title: artifactDraft.title,
        summary: artifactDraft.summary,
        payloadJson,
        evidenceJson,
        confidence: Math.max(0, Math.min(1, (parseFloat(artifactDraft.confidence) || 0) / 100)),
      });
      toast.success(t('knowledge.dreamingUpdated'));
      cancelArtifactEdit();
      await loadDreaming();
    } catch (e) {
      toast.error(formatUserError(t('knowledge.dreamingEdit'), e));
    } finally {
      setArtifactActionId(null);
    }
  }, [artifactDraft, cancelArtifactEdit, loadDreaming, t]);

  const askAboutArtifact = useCallback((artifact: api.DreamArtifact) => {
    const payload = formatArtifactJson(artifact.payloadJson);
    const evidence = formatArtifactJson(artifact.evidenceJson);
    const prompt = [
      t('knowledge.dreamingAskPrompt'),
      '',
      `${t('knowledge.dreamingEditTitle')}: ${artifact.title}`,
      `${t('knowledge.dreamingKindLabel')}: ${artifactKindName(artifact.kind)}`,
      `${t('knowledge.dreamingConfidence')}: ${Math.round(artifact.confidence * 100)}%`,
      `${t('knowledge.dreamingEditSummary')}: ${artifact.summary}`,
      '',
      `${t('knowledge.dreamingPayload')}:`,
      payload.length > 3000 ? `${payload.slice(0, 3000)}...` : payload,
      '',
      `${t('knowledge.dreamingEvidence')}:`,
      evidence.length > 3000 ? `${evidence.slice(0, 3000)}...` : evidence,
    ].join('\n');
    navigate('/chat', {
      state: {
        initialMessage: prompt,
        sourceIds: artifactSourceIds(artifact),
      },
    });
  }, [navigate, t]);

  /* ── Load data on tab change ───────────────────────────────────── */

  useEffect(() => {
    if (activeTab === 'compile') loadStats();
    if (activeTab === 'insights') loadDreaming();
  }, [activeTab, loadDreaming, loadStats]);

  useEffect(() => {
    void loadDreaming();
  }, [loadDreaming]);

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
    { key: 'insights', label: t('knowledge.insights'), icon: Sparkles },
  ];

  /* ── Progress percentage ───────────────────────────────────────── */

  const progressPct = stats && stats.totalDocs > 0
    ? Math.round((stats.compiledDocs / stats.totalDocs) * 100)
    : 0;
  const latestDreamRun = dreamRuns[0] ?? null;
  const latestArtifactCount = getNumericStat(latestDreamRun?.statsJson, 'artifactsCreated');
  const learningStatus = getLearningStatus({
    compiling,
    compileProgressActive: Boolean(compileProgress),
    dreamStarting,
    latestDreamRun,
    pendingArtifactCount: dreamArtifacts.filter((artifact) => artifact.status === 'pending').length,
  });

  const artifactKindLabel = useCallback((kind: string) => {
    switch (kind) {
      case 'knowledge_gap': return t('knowledge.dreamingKind.knowledgeGap');
      case 'health_fix': return t('knowledge.dreamingKind.healthFix');
      case 'project_memory_candidate': return t('knowledge.dreamingKind.projectMemory');
      case 'user_memory_candidate': return t('knowledge.dreamingKind.userMemory');
      case 'graph_relation_candidate': return t('knowledge.dreamingKind.graphRelation');
      case 'entity_merge_candidate': return t('knowledge.dreamingKind.entityMerge');
      case 'procedural_memory_candidate': return t('knowledge.dreamingKind.agentLearning');
      case 'skill_proposal_candidate': return t('knowledge.dreamingKind.skillProposal');
      default: return kind.replace(/_/g, ' ');
    }
  }, [t]);

  const artifactStatusLabel = useCallback((status: string) => {
    switch (status) {
      case 'pending': return t('knowledge.dreamingStatus.pending');
      case 'applied': return t('knowledge.dreamingStatus.applied');
      case 'rejected': return t('knowledge.dreamingStatus.rejected');
      case 'expired': return t('knowledge.dreamingStatus.expired');
      case 'undone': return t('knowledge.dreamingStatus.undone');
      default: return status;
    }
  }, [t]);
  const pendingArtifactCount = learningStatus.pendingArtifactCount;

  return (
    <div className="flex h-full flex-col min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-border shrink-0">
        <div className="flex items-center gap-2.5">
          <Brain size={20} className="text-accent" />
          <h2 className="text-base font-semibold text-text-primary">{t('knowledge.title')}</h2>
        </div>
        <button
          type="button"
          onClick={() => setActiveTab('insights')}
          className="flex items-center gap-2 rounded-md border border-border bg-surface-1 px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-border-hover hover:bg-surface-2 hover:text-text-primary"
        >
          <span className={`h-2 w-2 rounded-full ${learningStatus.dotClass}`} />
          <span>{t(learningStatus.labelKey)}</span>
        </button>
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
                              {t('knowledge.documents')} {r.documentId.slice(0, 8)}
                            </span>
                          </div>
                          <div className="flex items-center gap-2 shrink-0">
                            <Badge variant="info">{r.entitiesFound} {t('knowledge.totalEntities')}</Badge>
                            <Badge variant="default">{r.linksCreated} {t('knowledge.totalLinks')}</Badge>
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
              className="min-h-0"
            >
              <KnowledgeGraphView onOpenInsights={() => setActiveTab('insights')} />
            </motion.div>
          )}

          {activeTab === 'insights' && (
            <motion.div
              key="insights"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.15 }}
              className="space-y-4"
            >
              <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                <div className="flex items-center gap-2">
                  <Badge variant="accent" icon={<Sparkles size={11} />}>
                    {t('knowledge.dreamingReviewQueue')}
                  </Badge>
                  {latestDreamRun && (
                    <Badge variant={runStatusVariant(latestDreamRun.status)}>
                      {latestDreamRun.status}
                    </Badge>
                  )}
                </div>
                <Button
                  variant="secondary"
                  size="md"
                  loading={dreamStarting}
                  icon={<Lightbulb size={15} />}
                  onClick={handleStartDream}
                  disabled={dreamStarting}
                >
                  {dreamStarting ? t('knowledge.dreamingRunning') : t('knowledge.dreamingStart')}
                </Button>
              </div>

              <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
                <StatCard label={t('knowledge.dreamingPending')} value={pendingArtifactCount} />
                <StatCard
                  label={t('knowledge.dreamingLatestRun')}
                  value={latestDreamRun ? artifactStatusLabel(latestDreamRun.status) : '—'}
                />
                <StatCard label={t('knowledge.dreamingArtifacts')} value={latestArtifactCount} />
              </div>

              {dreamLoading ? (
                <div className="space-y-3">
                  <CardSkeleton />
                  <CardSkeleton />
                </div>
              ) : dreamArtifacts.length === 0 ? (
                <EmptyState
                  icon={<Sparkles size={32} />}
                  title={t('knowledge.dreamingEmptyTitle')}
                  description={t('knowledge.dreamingEmptyDescription')}
                />
              ) : (
                <motion.div
                  variants={listContainer}
                  initial="hidden"
                  animate="show"
                  className="space-y-2"
                >
                  {dreamArtifacts.map((artifact) => (
                    <motion.div
                      key={artifact.id}
                      variants={listItem}
                      className="p-3 rounded-lg border border-border bg-surface-1 space-y-3"
                    >
                      {editingArtifactId === artifact.id && artifactDraft ? (
                        <div className="space-y-3">
                          <div className="grid gap-3 sm:grid-cols-[1fr_7rem]">
                            <label className="space-y-1">
                              <span className="text-xs text-text-tertiary">{t('knowledge.dreamingEditTitle')}</span>
                              <input
                                className="w-full rounded-md border border-border bg-surface-2 px-2 py-1.5 text-sm text-text-primary"
                                value={artifactDraft.title}
                                onChange={(event) => setArtifactDraft({ ...artifactDraft, title: event.target.value })}
                              />
                            </label>
                            <label className="space-y-1">
                              <span className="text-xs text-text-tertiary">{t('knowledge.dreamingConfidence')}</span>
                              <input
                                type="number"
                                min={0}
                                max={100}
                                className="w-full rounded-md border border-border bg-surface-2 px-2 py-1.5 text-sm text-text-primary"
                                value={artifactDraft.confidence}
                                onChange={(event) => setArtifactDraft({ ...artifactDraft, confidence: event.target.value })}
                              />
                            </label>
                          </div>
                          <label className="block space-y-1">
                            <span className="text-xs text-text-tertiary">{t('knowledge.dreamingEditSummary')}</span>
                            <textarea
                              className="min-h-16 w-full rounded-md border border-border bg-surface-2 px-2 py-1.5 text-sm text-text-primary"
                              value={artifactDraft.summary}
                              onChange={(event) => setArtifactDraft({ ...artifactDraft, summary: event.target.value })}
                            />
                          </label>
                          <div className="grid gap-3 lg:grid-cols-2">
                            <label className="block space-y-1">
                              <span className="text-xs text-text-tertiary">{t('knowledge.dreamingPayload')}</span>
                              <textarea
                                className="min-h-36 w-full rounded-md border border-border bg-surface-2 px-2 py-1.5 font-mono text-[11px] leading-relaxed text-text-primary"
                                value={artifactDraft.payloadJson}
                                onChange={(event) => setArtifactDraft({ ...artifactDraft, payloadJson: event.target.value })}
                              />
                            </label>
                            <label className="block space-y-1">
                              <span className="text-xs text-text-tertiary">{t('knowledge.dreamingEvidence')}</span>
                              <textarea
                                className="min-h-36 w-full rounded-md border border-border bg-surface-2 px-2 py-1.5 font-mono text-[11px] leading-relaxed text-text-primary"
                                value={artifactDraft.evidenceJson}
                                onChange={(event) => setArtifactDraft({ ...artifactDraft, evidenceJson: event.target.value })}
                              />
                            </label>
                          </div>
                          <div className="flex justify-end gap-2">
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={cancelArtifactEdit}
                              disabled={artifactActionId !== null}
                            >
                              {t('knowledge.dreamingCancelEdit')}
                            </Button>
                            <Button
                              variant="secondary"
                              size="sm"
                              loading={artifactActionId === artifact.id}
                              onClick={() => saveArtifactEdit(artifact)}
                              disabled={artifactActionId !== null}
                            >
                              {t('knowledge.dreamingSaveEdit')}
                            </Button>
                          </div>
                        </div>
                      ) : (
                        <>
                          <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                            <div className="min-w-0 space-y-1">
                              <div className="flex flex-wrap items-center gap-2">
                                <Badge variant={artifactKindVariant(artifact.kind)}>
                                  {artifactKindLabel(artifact.kind)}
                                </Badge>
                                <Badge variant={artifactStatusVariant(artifact.status)}>
                                  {artifactStatusLabel(artifact.status)}
                                </Badge>
                                <Badge variant="default">
                                  {t('knowledge.dreamingConfidence')}: {Math.round(artifact.confidence * 100)}%
                                </Badge>
                              </div>
                              <h3 className="text-sm font-medium text-text-primary">{artifact.title}</h3>
                              <p className="text-xs text-text-secondary leading-relaxed">{artifact.summary}</p>
                            </div>
                            <div className="flex items-center gap-2 shrink-0">
                              {artifact.status === 'pending' && (
                                <>
                                  <Button
                                    variant="secondary"
                                    size="sm"
                                    loading={artifactActionId === artifact.id}
                                    icon={<Check size={13} />}
                                    onClick={() => handleArtifactAction(artifact, 'apply')}
                                    disabled={artifactActionId !== null}
                                  >
                                    {t('knowledge.dreamingApply')}
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    onClick={() => beginArtifactEdit(artifact)}
                                    disabled={artifactActionId !== null}
                                  >
                                    {t('knowledge.dreamingEdit')}
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    icon={<MessageSquare size={13} />}
                                    onClick={() => askAboutArtifact(artifact)}
                                    disabled={artifactActionId !== null}
                                  >
                                    {t('knowledge.dreamingAsk')}
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    icon={<X size={13} />}
                                    onClick={() => handleArtifactAction(artifact, 'reject')}
                                    disabled={artifactActionId !== null}
                                  >
                                    {t('knowledge.dreamingReject')}
                                  </Button>
                                </>
                              )}
                              {artifact.status === 'applied' && (
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  loading={artifactActionId === artifact.id}
                                  icon={<RotateCcw size={13} />}
                                  onClick={() => handleArtifactAction(artifact, 'undo')}
                                  disabled={artifactActionId !== null}
                                >
                                  {t('knowledge.dreamingUndo')}
                                </Button>
                              )}
                            </div>
                          </div>
                          <div className="flex flex-wrap items-center gap-2 text-[11px] text-text-tertiary">
                            <span>{new Date(artifact.createdAt).toLocaleString()}</span>
                          </div>
                          <details className="border-t border-border pt-2">
                            <summary className="cursor-pointer text-[11px] text-text-tertiary">
                              {t('knowledge.dreamingEvidence')}: {evidenceCount(artifact.evidenceJson)}
                            </summary>
                            <pre className="mt-2 max-h-44 overflow-auto whitespace-pre-wrap break-words rounded-md bg-surface-2 p-2 text-[11px] leading-relaxed text-text-secondary">
                              {formatArtifactJson(artifact.evidenceJson)}
                            </pre>
                          </details>
                        </>
                      )}
                    </motion.div>
                  ))}
                </motion.div>
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
                    description={t('knowledge.runCheck')}
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

function StatCard({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="p-3 rounded-lg border border-border bg-surface-1">
      <p className="text-xs text-text-tertiary mb-1">{label}</p>
      <p className="text-lg font-semibold text-text-primary">{value}</p>
    </div>
  );
}

function getNumericStat(stats: Record<string, unknown> | undefined, key: string): number {
  const value = stats?.[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function evidenceCount(evidence: unknown): number {
  if (Array.isArray(evidence)) return evidence.length;
  if (evidence && typeof evidence === 'object') {
    return Object.keys(evidence as Record<string, unknown>).length;
  }
  return 0;
}

function formatArtifactJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function artifactKindName(kind: string): string {
  return kind.replace(/_/g, ' ');
}

function artifactSourceIds(artifact: api.DreamArtifact): string[] {
  const ids = new Set<string>();
  const payload = artifact.payloadJson;
  if (payload && typeof payload === 'object') {
    const record = payload as Record<string, unknown>;
    if (typeof record.sourceId === 'string' && record.sourceId.trim()) {
      ids.add(record.sourceId.trim());
    }
    if (Array.isArray(record.sourceIds)) {
      for (const value of record.sourceIds) {
        if (typeof value === 'string' && value.trim()) ids.add(value.trim());
      }
    }
  }
  return [...ids];
}

function getLearningStatus(input: {
  compiling: boolean;
  compileProgressActive: boolean;
  dreamStarting: boolean;
  latestDreamRun: api.DreamRun | null;
  pendingArtifactCount: number;
}): { labelKey: TranslationKey; dotClass: string; pendingArtifactCount: number } {
  if (input.compiling || input.compileProgressActive) {
    return {
      labelKey: 'knowledge.learningStatus.indexing',
      dotClass: 'bg-info animate-pulse',
      pendingArtifactCount: input.pendingArtifactCount,
    };
  }
  if (input.dreamStarting || input.latestDreamRun?.status === 'running') {
    return {
      labelKey: 'knowledge.learningStatus.dreaming',
      dotClass: 'bg-accent animate-pulse',
      pendingArtifactCount: input.pendingArtifactCount,
    };
  }
  if (input.latestDreamRun?.status === 'failed') {
    return {
      labelKey: 'knowledge.learningStatus.error',
      dotClass: 'bg-danger',
      pendingArtifactCount: input.pendingArtifactCount,
    };
  }
  if (input.pendingArtifactCount > 0) {
    return {
      labelKey: 'knowledge.learningStatus.needsReview',
      dotClass: 'bg-warning',
      pendingArtifactCount: input.pendingArtifactCount,
    };
  }
  return {
    labelKey: 'knowledge.learningStatus.idle',
    dotClass: 'bg-success',
    pendingArtifactCount: input.pendingArtifactCount,
  };
}

function artifactKindVariant(kind: string): BadgeVariant {
  switch (kind) {
    case 'knowledge_gap': return 'info';
    case 'health_fix': return 'warning';
    case 'entity_merge_candidate': return 'purple';
    case 'graph_relation_candidate': return 'cyan';
    case 'project_memory_candidate': return 'teal';
    case 'user_memory_candidate': return 'accent';
    case 'procedural_memory_candidate': return 'blue';
    case 'skill_proposal_candidate': return 'purple';
    default: return 'default';
  }
}

function artifactStatusVariant(status: string): BadgeVariant {
  switch (status) {
    case 'pending': return 'warning';
    case 'applied': return 'success';
    case 'rejected': return 'muted';
    case 'expired': return 'slate';
    case 'undone': return 'slate';
    default: return 'default';
  }
}

function runStatusVariant(status: string): BadgeVariant {
  switch (status) {
    case 'completed': return 'success';
    case 'running': return 'info';
    case 'failed': return 'danger';
    case 'cancelled': return 'muted';
    default: return 'default';
  }
}
