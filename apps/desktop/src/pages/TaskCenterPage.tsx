import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Boxes,
  Brain,
  CheckCircle2,
  Circle,
  ClipboardList,
  ExternalLink,
  FileText,
  FolderOpen,
  GitBranch,
  History,
  Loader2,
  Network,
  Pause,
  Pencil,
  Play,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldAlert,
  ShieldCheck,
  Square,
  TerminalSquare,
  XCircle,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation, type TranslationKey } from '../i18n';
import * as api from '../lib/api';
import {
  taskCenterHistoryFromEvents,
  type TaskCenterHistoryItem,
} from '../lib/streaming/taskCenterHistory';
import type {
  AgentExecutionGraph,
  AgentTaskArtifact,
  AgentTaskArtifactSummary,
  AgentTaskArtifactVersion,
  AgentTaskRunListItem,
  ApprovalPolicyList,
  ToolAccessInfo,
} from '../types/conversation';
import type {
  InvestigationGraph,
  TaskResumeCheckpoint,
} from '../types/workflows';

const COPY_KEYS = {
  title: 'taskCenter.title',
  subtitle: 'taskCenter.subtitle',
  refresh: 'taskCenter.refresh',
  loading: 'taskCenter.loading',
  emptyTitle: 'taskCenter.emptyTitle',
  emptyBody: 'taskCenter.emptyBody',
  running: 'taskCenter.running',
  queued: 'taskCenter.queued',
  waitingApproval: 'taskCenter.waitingApproval',
  cancelling: 'taskCenter.cancelling',
  paused: 'taskCenter.paused',
  cancelled: 'taskCenter.cancelled',
  completed: 'taskCenter.completed',
  failed: 'taskCenter.failed',
  timedOut: 'taskCenter.timedOut',
  unknown: 'taskCenter.unknown',
  openChat: 'taskCenter.openChat',
  retry: 'taskCenter.retry',
  cancel: 'taskCenter.cancel',
  cancelTask: 'taskCenter.cancelTask',
  pause: 'taskCenter.pause',
  resume: 'taskCenter.resume',
  pauseSaved: 'taskCenter.pauseSaved',
  pauseError: 'taskCenter.pauseError',
  resumeError: 'taskCenter.resumeError',
  resumeCheckpoint: 'taskCenter.resumeCheckpoint',
  noCheckpoint: 'taskCenter.noCheckpoint',
  checkpointReason: 'taskCenter.checkpointReason',
  investigationGraph: 'taskCenter.investigationGraph',
  citations: 'taskCenter.citations',
  openQuestions: 'taskCenter.openQuestions',
  noInvestigationGraph: 'taskCenter.noInvestigationGraph',
  runDetails: 'taskCenter.runDetails',
  executionGraph: 'taskCenter.executionGraph',
  history: 'taskCenter.history',
  artifacts: 'taskCenter.artifacts',
  projectMemory: 'taskCenter.projectMemory',
  noProject: 'taskCenter.noProject',
  noMemory: 'taskCenter.noMemory',
  saveMemory: 'taskCenter.saveMemory',
  savedMemory: 'taskCenter.savedMemory',
  toolRiskMap: 'taskCenter.toolRiskMap',
  highRisk: 'taskCenter.highRisk',
  policy: 'taskCenter.policy',
  askEachTime: 'taskCenter.askEachTime',
  allowSession: 'taskCenter.allowSession',
  denyForever: 'taskCenter.denyForever',
  denyOnce: 'taskCenter.denyOnce',
  allowOnce: 'taskCenter.allowOnce',
  noApprovalNeeded: 'taskCenter.noApprovalNeeded',
  approval: 'taskCenter.approval',
  read: 'taskCenter.read',
  write: 'taskCenter.write',
  execute: 'taskCenter.execute',
  network: 'taskCenter.network',
  subtasks: 'taskCenter.subtasks',
  events: 'taskCenter.events',
  failureReason: 'taskCenter.failureReason',
  artifactPaths: 'taskCenter.artifactPaths',
  openFile: 'taskCenter.openFile',
  showInFolder: 'taskCenter.showInFolder',
  openFileError: 'taskCenter.openFileError',
  noArtifacts: 'taskCenter.noArtifacts',
  savedArtifacts: 'taskCenter.savedArtifacts',
  noSavedArtifacts: 'taskCenter.noSavedArtifacts',
  saveEditable: 'taskCenter.saveEditable',
  editArtifact: 'taskCenter.editArtifact',
  saveArtifact: 'taskCenter.saveArtifact',
  cancelEdit: 'taskCenter.cancelEdit',
  versionHistory: 'taskCenter.versionHistory',
  artifactSaved: 'taskCenter.artifactSaved',
  artifactUpdated: 'taskCenter.artifactUpdated',
  titleLabel: 'taskCenter.titleLabel',
  summaryLabel: 'taskCenter.summaryLabel',
  contentLabel: 'taskCenter.contentLabel',
  stopped: 'taskCenter.stopped',
  stopError: 'taskCenter.stopError',
  retryHint: 'taskCenter.retryHint',
  artifactFallbackTitle: 'taskCenter.artifactFallbackTitle',
} as const;

type Copy = Record<keyof typeof COPY_KEYS, string>;
type TranslateFn = ReturnType<typeof useTranslation>['t'];

function createCopy(t: TranslateFn): Copy {
  return Object.fromEntries(
    Object.entries(COPY_KEYS).map(([key, translationKey]) => [key, t(translationKey as TranslationKey)]),
  ) as Copy;
}

function statusLabel(status: string, copy: Copy) {
  switch (status) {
    case 'queued':
      return copy.queued;
    case 'running':
      return copy.running;
    case 'waiting_approval':
      return copy.waitingApproval;
    case 'cancelling':
      return copy.cancelling;
    case 'paused':
      return copy.paused;
    case 'cancelled':
      return copy.cancelled;
    case 'completed':
      return copy.completed;
    case 'failed':
      return copy.failed;
    case 'timed_out':
      return copy.timedOut;
    default:
      return status || copy.unknown;
  }
}

function statusTone(status: string) {
  switch (status) {
    case 'running':
    case 'queued':
    case 'waiting_approval':
      return 'border-accent/25 bg-accent/10 text-accent';
    case 'completed':
      return 'border-success/25 bg-success/10 text-success';
    case 'failed':
    case 'timed_out':
      return 'border-danger/25 bg-danger/10 text-danger';
    case 'cancelled':
    case 'cancelling':
    case 'paused':
      return 'border-warning/25 bg-warning/10 text-warning';
    default:
      return 'border-border/70 bg-surface-1 text-text-secondary';
  }
}

function statusIcon(status: string) {
  if (status === 'running' || status === 'queued' || status === 'waiting_approval') {
    return <Loader2 className="h-3.5 w-3.5 animate-spin" />;
  }
  if (status === 'completed') return <CheckCircle2 className="h-3.5 w-3.5" />;
  if (status === 'failed' || status === 'timed_out') return <XCircle className="h-3.5 w-3.5" />;
  if (status === 'paused') return <Pause className="h-3.5 w-3.5" />;
  if (status === 'cancelled' || status === 'cancelling') return <Square className="h-3.5 w-3.5" />;
  return <Circle className="h-3.5 w-3.5" />;
}

function isActiveTask(status: string) {
  return ['queued', 'running', 'waiting_approval', 'cancelling'].includes(status);
}

function formatTime(value?: string | null) {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatCategory(value: string) {
  return value.replace(/_/g, ' ');
}

function approvalPolicyLabel(
  decision: string | undefined,
  needsApproval: boolean,
  copy: Copy,
) {
  switch (decision) {
    case 'allow_session':
      return copy.allowSession;
    case 'allow_once':
      return copy.allowOnce;
    case 'never':
      return copy.denyForever;
    case 'deny':
      return copy.denyOnce;
    default:
      return needsApproval ? copy.askEachTime : copy.noApprovalNeeded;
  }
}

function RiskPill({ children }: { children: ReactNode }) {
  return (
    <span className="rounded-md border border-border/60 bg-surface-0/75 px-1.5 py-0.5 text-[10px] text-text-secondary">
      {children}
    </span>
  );
}

type ArtifactDraft = {
  title: string;
  summary: string;
  content: string;
};

function artifactSummaryContent(artifact: AgentTaskArtifactSummary) {
  if (artifact.summary) return artifact.summary;
  if (typeof artifact.payload === 'string') return artifact.payload;
  if (artifact.payload == null) return '';
  return JSON.stringify(artifact.payload, null, 2);
}

function artifactDraftFromSaved(artifact: AgentTaskArtifact): ArtifactDraft {
  return {
    title: artifact.title,
    summary: artifact.summary ?? '',
    content: artifact.content,
  };
}

export function TaskCenterPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const copy = useMemo(() => createCopy(t), [t]);
  const [tasks, setTasks] = useState<AgentTaskRunListItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [events, setEvents] = useState<TaskCenterHistoryItem[]>([]);
  const [graph, setGraph] = useState<AgentExecutionGraph | null>(null);
  const [investigationGraph, setInvestigationGraph] = useState<InvestigationGraph | null>(null);
  const [resumeCheckpoints, setResumeCheckpoints] = useState<TaskResumeCheckpoint[]>([]);
  const [artifacts, setArtifacts] = useState<AgentTaskArtifactSummary[]>([]);
  const [savedArtifacts, setSavedArtifacts] = useState<AgentTaskArtifact[]>([]);
  const [artifactVersions, setArtifactVersions] = useState<Record<string, AgentTaskArtifactVersion[]>>({});
  const [editingArtifactId, setEditingArtifactId] = useState<string | null>(null);
  const [artifactDraft, setArtifactDraft] = useState<ArtifactDraft>({ title: '', summary: '', content: '' });
  const [savingArtifactId, setSavingArtifactId] = useState<string | null>(null);
  const [toolAccess, setToolAccess] = useState<ToolAccessInfo[]>([]);
  const [approvalPolicies, setApprovalPolicies] = useState<ApprovalPolicyList>({ persisted: [], session: [] });
  const [projectMemories, setProjectMemories] = useState<api.ProjectMemory[]>([]);
  const [loading, setLoading] = useState(true);
  const [detailLoading, setDetailLoading] = useState(false);
  const [stoppingId, setStoppingId] = useState<string | null>(null);
  const [pausingId, setPausingId] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [savingMemory, setSavingMemory] = useState(false);

  const selected = useMemo(
    () => tasks.find((task) => task.run.id === selectedId) ?? tasks[0] ?? null,
    [selectedId, tasks],
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [recentTasks, accessMap, policies] = await Promise.all([
        api.listRecentAgentTaskRuns(80),
        api.listToolAccessMap(),
        api.listToolApprovalPolicies().catch(() => ({ persisted: [], session: [] })),
      ]);
      setTasks(recentTasks);
      setToolAccess(accessMap);
      setApprovalPolicies(policies);
      setSelectedId((current) =>
        current && recentTasks.some((task) => task.run.id === current)
          ? current
          : recentTasks[0]?.run.id ?? null,
      );
    } catch (error) {
      toast.error(String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!selected) {
      setEvents([]);
      setGraph(null);
      setInvestigationGraph(null);
      setResumeCheckpoints([]);
      setArtifacts([]);
      setSavedArtifacts([]);
      setArtifactVersions({});
      setEditingArtifactId(null);
      setProjectMemories([]);
      return;
    }

    let cancelled = false;
    setDetailLoading(true);
    void (async () => {
      try {
        const [
          nextEvents,
          nextGraph,
          nextArtifacts,
          nextSavedArtifacts,
          nextMemories,
          nextCheckpoints,
          nextInvestigationGraph,
          nextRunEvents,
        ] = await Promise.all([
          api.getAgentTaskRunEvents(selected.run.id),
          api.getAgentExecutionGraph(selected.run.id),
          api.getAgentTaskArtifacts(selected.run.id),
          api.listPersistedAgentTaskArtifacts(selected.run.id),
          selected.projectId ? api.listProjectMemories(selected.projectId) : Promise.resolve([]),
          api.listTaskResumeCheckpoints(selected.run.id).catch(() => []),
          api.getInvestigationGraph(selected.run.id).catch(() => null),
          api.getAgentRunEvents(selected.run.id).catch(() => []),
        ]);
        const versionPairs = await Promise.all(
          nextSavedArtifacts.slice(0, 12).map(async (artifact) => {
            try {
              const versions = await api.listAgentTaskArtifactVersions(artifact.id);
              return [artifact.id, versions] as const;
            } catch {
              return [artifact.id, []] as const;
            }
          }),
        );
        if (cancelled) return;
        setEvents(taskCenterHistoryFromEvents(nextEvents, nextRunEvents));
        setGraph(nextGraph);
        setInvestigationGraph(nextInvestigationGraph);
        setResumeCheckpoints(nextCheckpoints);
        setArtifacts(nextArtifacts);
        setSavedArtifacts(nextSavedArtifacts);
        setArtifactVersions(Object.fromEntries(versionPairs));
        setEditingArtifactId((current) =>
          current && nextSavedArtifacts.some((artifact) => artifact.id === current) ? current : null,
        );
        setProjectMemories(nextMemories);
      } catch (error) {
        if (!cancelled) toast.error(String(error));
      } finally {
        if (!cancelled) setDetailLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const handleCancel = useCallback(async () => {
    if (!selected || !isActiveTask(selected.run.status)) return;
    setStoppingId(selected.run.id);
    try {
      await api.agentStop(selected.run.conversationId);
      toast.success(copy.stopped);
      await load();
    } catch (error) {
      toast.error(`${copy.stopError}: ${String(error)}`);
    } finally {
      setStoppingId(null);
    }
  }, [copy.stopError, copy.stopped, load, selected]);

  const handlePause = useCallback(async () => {
    if (!selected || !isActiveTask(selected.run.status)) return;
    setPausingId(selected.run.id);
    try {
      const checkpoint = await api.pauseAgentTaskRun(selected.run.id);
      setResumeCheckpoints((current) => [checkpoint, ...current.filter((item) => item.id !== checkpoint.id)]);
      toast.success(copy.pauseSaved);
      await load();
    } catch (error) {
      toast.error(`${copy.pauseError}: ${String(error)}`);
    } finally {
      setPausingId(null);
    }
  }, [copy.pauseError, copy.pauseSaved, load, selected]);

  const handleResume = useCallback(async () => {
    if (!selected) return;
    setResumingId(selected.run.id);
    try {
      const resume = await api.getTaskResumePrompt(selected.run.id);
      navigate(`/chat/${selected.run.conversationId}`, {
        state: { initialMessage: resume.prompt },
      });
    } catch (error) {
      toast.error(`${copy.resumeError}: ${String(error)}`);
    } finally {
      setResumingId(null);
    }
  }, [copy.resumeError, navigate, selected]);

  const handleRetry = useCallback(() => {
    if (!selected) return;
    toast.message(copy.retryHint);
    navigate(`/chat/${selected.run.conversationId}`, {
      state: { initialMessage: selected.userMessagePreview },
    });
  }, [copy.retryHint, navigate, selected]);

  const handleOpenChat = useCallback(() => {
    if (selected) navigate(`/chat/${selected.run.conversationId}`);
  }, [navigate, selected]);

  const handleSaveMemory = useCallback(async () => {
    if (!selected?.projectId) return;
    setSavingMemory(true);
    try {
      await api.createProjectMemory(selected.projectId, {
        kind: selected.run.status === 'failed' ? 'todo' : 'decision',
        title: selected.run.title,
        content: [
          selected.run.summary || selected.userMessagePreview,
          selected.run.errorMessage ? `${copy.failureReason}: ${selected.run.errorMessage}` : '',
        ].filter(Boolean).join('\n'),
        pinned: true,
        source: 'task_center',
        confidence: 0.8,
      });
      setProjectMemories(await api.listProjectMemories(selected.projectId));
      toast.success(copy.savedMemory);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingMemory(false);
    }
  }, [copy.failureReason, copy.savedMemory, selected]);

  const handleCreateEditableArtifact = useCallback(async (artifact: AgentTaskArtifactSummary) => {
    if (!selected) return;
    setSavingArtifactId(artifact.id);
    try {
      const created = await api.createAgentTaskArtifact(selected.run.id, {
        kind: artifact.kind || 'artifact',
        title: artifact.title || artifact.kind || copy.artifactFallbackTitle,
        summary: artifact.summary ?? null,
        content: artifactSummaryContent(artifact),
        paths: artifact.paths,
        payload: artifact.payload,
        source: artifact.source || 'task_center',
      });
      const versions = await api.listAgentTaskArtifactVersions(created.id);
      setSavedArtifacts((current) => [
        created,
        ...current.filter((item) => item.id !== created.id),
      ]);
      setArtifactVersions((current) => ({ ...current, [created.id]: versions }));
      setEditingArtifactId(created.id);
      setArtifactDraft(artifactDraftFromSaved(created));
      toast.success(copy.artifactSaved);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingArtifactId(null);
    }
  }, [copy.artifactFallbackTitle, copy.artifactSaved, selected]);

  const handleStartArtifactEdit = useCallback(async (artifact: AgentTaskArtifact) => {
    setEditingArtifactId(artifact.id);
    setArtifactDraft(artifactDraftFromSaved(artifact));
    if (!artifactVersions[artifact.id]) {
      try {
        const versions = await api.listAgentTaskArtifactVersions(artifact.id);
        setArtifactVersions((current) => ({ ...current, [artifact.id]: versions }));
      } catch (error) {
        toast.error(String(error));
      }
    }
  }, [artifactVersions]);

  const handleSaveArtifact = useCallback(async (artifact: AgentTaskArtifact) => {
    setSavingArtifactId(artifact.id);
    try {
      const updated = await api.updateAgentTaskArtifact(artifact.id, {
        title: artifactDraft.title,
        summary: artifactDraft.summary.trim() ? artifactDraft.summary : null,
        content: artifactDraft.content,
        paths: artifact.paths,
        payload: artifact.payload ?? null,
      });
      const versions = await api.listAgentTaskArtifactVersions(updated.id);
      setSavedArtifacts((current) =>
        current.map((item) => (item.id === updated.id ? updated : item)),
      );
      setArtifactVersions((current) => ({ ...current, [updated.id]: versions }));
      setArtifactDraft(artifactDraftFromSaved(updated));
      toast.success(copy.artifactUpdated);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingArtifactId(null);
    }
  }, [artifactDraft, copy.artifactUpdated]);

  const handleOpenArtifactPath = useCallback(async (path: string, reveal = false) => {
    try {
      if (reveal) {
        await api.showInFileExplorer(path);
      } else {
        await api.openFileInDefaultApp(path);
      }
    } catch (error) {
      toast.error(`${copy.openFileError}: ${String(error)}`);
    }
  }, [copy.openFileError]);

  const runningCount = tasks.filter((task) => isActiveTask(task.run.status)).length;
  const failedCount = tasks.filter((task) => ['failed', 'timed_out'].includes(task.run.status)).length;
  const completedCount = tasks.filter((task) => task.run.status === 'completed').length;
  const highRiskTools = toolAccess.filter((tool) => tool.riskLevel === 'high');
  const approvalPolicyByTool = useMemo(() => {
    const entries = new Map<string, string>();
    for (const policy of approvalPolicies.persisted) {
      entries.set(policy.toolName, policy.decision);
    }
    for (const policy of approvalPolicies.session) {
      entries.set(policy.toolName, policy.decision);
    }
    return entries;
  }, [approvalPolicies]);
  const graphNodes = graph?.nodes ?? [];
  const visibleGraphNodes = graphNodes.length > 0
    ? graphNodes
    : selected
      ? [{
          id: selected.run.id,
          nodeType: 'supervisor',
          label: selected.run.title,
          role: 'Supervisor',
          status: selected.run.status,
          phase: selected.run.phase,
          summary: selected.run.summary,
          errorMessage: selected.run.errorMessage,
          input: null,
          output: selected.run.artifacts ?? null,
          tokenBudget: null,
          startedAt: selected.run.startedAt,
          finishedAt: selected.run.finishedAt,
        }]
      : [];
  const artifactKinds = artifacts.length > 0
    ? [...new Set(artifacts.map((artifact) => artifact.kind))]
    : selected?.artifactKinds ?? [];
  const artifactPaths = [...new Set(artifacts.flatMap((artifact) => artifact.paths))].slice(0, 8);
  const latestCheckpoint = resumeCheckpoints[0] ?? null;
  const canResume = Boolean(selected && (selected.run.status === 'paused' || latestCheckpoint));
  const investigationNodes = investigationGraph?.nodes ?? [];
  const investigationEdges = investigationGraph?.edges ?? [];

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface-0">
      <header className="shrink-0 border-b border-border/70 bg-surface-1/85 px-5 py-4 backdrop-blur">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="flex h-9 w-9 items-center justify-center rounded-lg border border-accent/25 bg-accent/10 text-accent">
                <ClipboardList className="h-4.5 w-4.5" />
              </span>
              <div>
                <h1 className="text-xl font-semibold text-text-primary">{copy.title}</h1>
                <p className="mt-1 max-w-3xl text-sm leading-6 text-text-secondary">{copy.subtitle}</p>
              </div>
            </div>
          </div>
          <button
            type="button"
            onClick={() => void load()}
            className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-surface-0 px-3 text-sm text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary"
          >
            <RefreshCw className="h-4 w-4" />
            {copy.refresh}
          </button>
        </div>
        <div className="mt-4 grid gap-2 sm:grid-cols-4">
          {[
            [String(tasks.length), copy.title],
            [String(runningCount), copy.running],
            [String(failedCount), copy.failed],
            [String(completedCount), copy.completed],
          ].map(([value, label]) => (
            <div key={label} className="rounded-lg border border-border/70 bg-surface-0/75 px-3 py-2">
              <div className="text-lg font-semibold tabular-nums text-text-primary">{value}</div>
              <div className="text-[11px] text-text-tertiary">{label}</div>
            </div>
          ))}
        </div>
      </header>

      <main className="grid min-h-0 flex-1 grid-cols-1 overflow-hidden lg:grid-cols-[minmax(300px,380px)_minmax(0,1fr)]">
        <aside className="min-h-0 overflow-y-auto border-r border-border/70 bg-surface-1/45 p-3">
          {loading ? (
            <div className="flex items-center gap-2 rounded-lg border border-border/70 bg-surface-0/75 px-3 py-5 text-sm text-text-secondary">
              <Loader2 className="h-4 w-4 animate-spin text-accent" />
              {copy.loading}
            </div>
          ) : tasks.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border px-4 py-10 text-center">
              <div className="text-sm font-medium text-text-primary">{copy.emptyTitle}</div>
              <div className="mt-1 text-xs leading-5 text-text-tertiary">{copy.emptyBody}</div>
            </div>
          ) : (
            <div className="space-y-2">
              {tasks.map((task) => {
                const active = selected?.run.id === task.run.id;
                return (
                  <button
                    key={task.run.id}
                    type="button"
                    onClick={() => setSelectedId(task.run.id)}
                    className={`w-full rounded-lg border p-3 text-left transition-colors ${
                      active
                        ? 'border-accent/50 bg-accent-subtle/40'
                        : 'border-border/70 bg-surface-0/75 hover:border-border-hover hover:bg-surface-2/70'
                    }`}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <div className="truncate text-sm font-medium text-text-primary">{task.run.title}</div>
                        <div className="mt-1 truncate text-xs text-text-tertiary">
                          {task.conversationTitle || task.run.conversationId}
                        </div>
                      </div>
                      <span className={`inline-flex shrink-0 items-center gap-1 rounded-full border px-2 py-1 text-[10px] ${statusTone(task.run.status)}`}>
                        {statusIcon(task.run.status)}
                        {statusLabel(task.run.status, copy)}
                      </span>
                    </div>
                    <p className="mt-2 line-clamp-2 text-xs leading-5 text-text-secondary">
                      {task.userMessagePreview}
                    </p>
                    <div className="mt-2 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
                      {task.projectName && <RiskPill>{task.projectName}</RiskPill>}
                      <RiskPill>{task.subtaskTotal} {copy.subtasks}</RiskPill>
                      <RiskPill>{task.eventCount} {copy.events}</RiskPill>
                      {task.artifactKinds.slice(0, 3).map((kind) => <RiskPill key={kind}>{kind}</RiskPill>)}
                    </div>
                  </button>
                );
              })}
            </div>
          )}
        </aside>

        <section className="min-h-0 overflow-y-auto p-4">
          {!selected ? (
            <div className="rounded-lg border border-dashed border-border px-4 py-12 text-center text-sm text-text-tertiary">
              {copy.emptyBody}
            </div>
          ) : (
            <div className="space-y-4">
              <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h2 className="text-lg font-semibold text-text-primary">{selected.run.title}</h2>
                      <span className={`inline-flex items-center gap-1 rounded-full border px-2 py-1 text-[11px] ${statusTone(selected.run.status)}`}>
                        {statusIcon(selected.run.status)}
                        {statusLabel(selected.run.status, copy)}
                      </span>
                    </div>
                    <div className="mt-1 flex flex-wrap gap-2 text-xs text-text-tertiary">
                      <span>{selected.conversationTitle || selected.run.conversationId}</span>
                      {selected.projectName && <span>{selected.projectName}</span>}
                      {selected.run.routeKind && <span>{selected.run.routeKind}</span>}
                      <span>{formatTime(selected.run.updatedAt)}</span>
                    </div>
                    <p className="mt-2 max-w-3xl text-sm leading-6 text-text-secondary">
                      {selected.run.summary || selected.userMessagePreview}
                    </p>
                    {selected.run.errorMessage && (
                      <div className="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                        <span className="font-medium">{copy.failureReason}: </span>
                        {selected.run.errorMessage}
                      </div>
                    )}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      onClick={handleOpenChat}
                      className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-surface-0 px-3 text-sm text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary"
                    >
                      <ExternalLink className="h-4 w-4" />
                      {copy.openChat}
                    </button>
                    <button
                      type="button"
                      onClick={handleRetry}
                      className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-surface-0 px-3 text-sm text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary"
                    >
                      <RotateCcw className="h-4 w-4" />
                      {copy.retry}
                    </button>
                    {isActiveTask(selected.run.status) && (
                      <button
                        type="button"
                        onClick={() => void handlePause()}
                        disabled={pausingId === selected.run.id}
                        className="inline-flex h-9 items-center gap-1.5 rounded-md border border-warning/35 bg-warning/10 px-3 text-sm text-warning transition-colors hover:bg-warning/15 disabled:cursor-not-allowed disabled:opacity-60"
                      >
                        {pausingId === selected.run.id ? <Loader2 className="h-4 w-4 animate-spin" /> : <Pause className="h-4 w-4" />}
                        {copy.pause}
                      </button>
                    )}
                    {canResume && (
                      <button
                        type="button"
                        onClick={() => void handleResume()}
                        disabled={resumingId === selected.run.id}
                        className="inline-flex h-9 items-center gap-1.5 rounded-md border border-accent/35 bg-accent/10 px-3 text-sm text-accent transition-colors hover:bg-accent/15 disabled:cursor-not-allowed disabled:opacity-60"
                      >
                        {resumingId === selected.run.id ? <Loader2 className="h-4 w-4 animate-spin" /> : <Play className="h-4 w-4" />}
                        {copy.resume}
                      </button>
                    )}
                    {isActiveTask(selected.run.status) && (
                      <button
                        type="button"
                        aria-label={copy.cancelTask}
                        onClick={() => void handleCancel()}
                        disabled={stoppingId === selected.run.id}
                        className="inline-flex h-9 items-center gap-1.5 rounded-md border border-danger/35 bg-danger/10 px-3 text-sm text-danger transition-colors hover:bg-danger/15 disabled:cursor-not-allowed disabled:opacity-60"
                      >
                        {stoppingId === selected.run.id ? <Loader2 className="h-4 w-4 animate-spin" /> : <Square className="h-4 w-4" />}
                        {copy.cancel}
                      </button>
                    )}
                  </div>
                </div>
              </section>

              <div className="grid gap-4 xl:grid-cols-[minmax(0,1.25fr)_minmax(320px,0.75fr)]">
                <div className="space-y-4">
                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                        <GitBranch className="h-4 w-4 text-accent" />
                        {copy.executionGraph}
                      </div>
                      {detailLoading && <Loader2 className="h-4 w-4 animate-spin text-accent" />}
                    </div>
                    <div className="grid gap-3 md:grid-cols-3">
                      {visibleGraphNodes.map((node, index) => (
                        <div key={node.id} className="relative rounded-lg border border-border/70 bg-surface-0/75 p-3">
                          {index > 0 && (
                            <span className="absolute -left-3 top-1/2 hidden h-px w-3 bg-border md:block" />
                          )}
                          <div className="flex items-start justify-between gap-2">
                            <div className="min-w-0">
                              <div className="text-xs font-medium uppercase tracking-[0.14em] text-text-tertiary">
                                {node.role || 'Agent'}
                              </div>
                              <div className="mt-1 line-clamp-2 text-sm font-medium text-text-primary">{node.label}</div>
                            </div>
                            <span className={`inline-flex shrink-0 items-center gap-1 rounded-full border px-2 py-1 text-[10px] ${statusTone(node.status)}`}>
                              {statusIcon(node.status)}
                              {statusLabel(node.status, copy)}
                            </span>
                          </div>
                          <div className="mt-2 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
                            <RiskPill>{node.phase}</RiskPill>
                            <RiskPill>{node.nodeType}</RiskPill>
                            {node.tokenBudget != null && <RiskPill>{node.tokenBudget.toLocaleString()} tokens</RiskPill>}
                          </div>
                          {(node.errorMessage || node.summary) && (
                            <p className="mt-2 line-clamp-2 text-xs leading-5 text-text-secondary">
                              {node.errorMessage || node.summary}
                            </p>
                          )}
                        </div>
                      ))}
                    </div>
                  </section>

                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center gap-2 text-sm font-semibold text-text-primary">
                      <Network className="h-4 w-4 text-accent" />
                      {copy.investigationGraph}
                    </div>
                    {investigationNodes.length === 0 ? (
                      <div className="rounded-md border border-dashed border-border px-3 py-6 text-center text-sm text-text-tertiary">
                        {copy.noInvestigationGraph}
                      </div>
                    ) : (
                      <div className="space-y-3">
                        <div className="grid gap-2 md:grid-cols-2">
                          {investigationNodes.slice(0, 8).map((node) => (
                            <div key={node.id} className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                              <div className="flex items-start justify-between gap-2">
                                <div className="min-w-0">
                                  <div className="truncate text-sm font-medium text-text-primary">{node.label}</div>
                                  <div className="mt-1 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
                                    <RiskPill>{node.nodeType}</RiskPill>
                                    {node.status && <RiskPill>{node.status}</RiskPill>}
                                  </div>
                                </div>
                                {node.sourceUrl && <ExternalLink className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />}
                              </div>
                              {node.summary && (
                                <p className="mt-2 line-clamp-2 text-xs leading-5 text-text-secondary">{node.summary}</p>
                              )}
                              {node.sourceUrl && (
                                <p className="mt-2 break-all text-[11px] leading-5 text-text-tertiary">{node.sourceUrl}</p>
                              )}
                            </div>
                          ))}
                        </div>
                        {investigationEdges.length > 0 && (
                          <div className="flex flex-wrap gap-1">
                            {investigationEdges.slice(0, 10).map((edge) => (
                              <RiskPill key={`${edge.from}-${edge.to}-${edge.label}`}>
                                {edge.from}{' -> '}{edge.label}{' -> '}{edge.to}
                              </RiskPill>
                            ))}
                          </div>
                        )}
                        {investigationGraph?.citations.length ? (
                          <div>
                            <div className="mb-1 text-xs font-medium text-text-tertiary">{copy.citations}</div>
                            <div className="space-y-1">
                              {investigationGraph.citations.slice(0, 6).map((citation) => (
                                <div key={citation} className="break-all rounded-md border border-border/60 bg-surface-0/75 px-2 py-1.5 text-xs text-text-secondary">
                                  {citation}
                                </div>
                              ))}
                            </div>
                          </div>
                        ) : null}
                        {investigationGraph?.openQuestions.length ? (
                          <div>
                            <div className="mb-1 text-xs font-medium text-text-tertiary">{copy.openQuestions}</div>
                            <div className="space-y-1">
                              {investigationGraph.openQuestions.slice(0, 6).map((question) => (
                                <div key={question} className="rounded-md border border-warning/30 bg-warning/10 px-2 py-1.5 text-xs text-warning">
                                  {question}
                                </div>
                              ))}
                            </div>
                          </div>
                        ) : null}
                      </div>
                    )}
                  </section>

                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center gap-2 text-sm font-semibold text-text-primary">
                      <FileText className="h-4 w-4 text-accent" />
                      {copy.artifacts}
                    </div>
                    {artifactKinds.length === 0 && artifactPaths.length === 0 && savedArtifacts.length === 0 ? (
                      <div className="rounded-md border border-dashed border-border px-3 py-6 text-center text-sm text-text-tertiary">
                        {copy.noArtifacts}
                      </div>
                    ) : (
                      <div className="space-y-3">
                        <div className="flex flex-wrap gap-1.5">
                          {artifactKinds.map((kind) => (
                            <span key={kind} className="rounded-md border border-border/70 bg-surface-0 px-2 py-1 text-xs text-text-secondary">
                              {kind}
                            </span>
                          ))}
                        </div>
                        {artifacts.length > 0 && (
                          <div className="grid gap-2 sm:grid-cols-2">
                            {artifacts.slice(0, 6).map((artifact) => (
                              <div key={artifact.id} className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                                <div className="flex items-start justify-between gap-2">
                                  <div className="min-w-0">
                                    <div className="truncate text-xs font-medium text-text-primary">{artifact.title}</div>
                                    <div className="mt-0.5 text-[10px] text-text-tertiary">{artifact.kind}</div>
                                  </div>
                                  <FileText className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
                                </div>
                                {artifact.summary && (
                                  <p className="mt-2 line-clamp-2 text-xs leading-5 text-text-secondary">
                                    {artifact.summary}
                                  </p>
                                )}
                                <button
                                  type="button"
                                  onClick={() => void handleCreateEditableArtifact(artifact)}
                                  disabled={savingArtifactId === artifact.id}
                                  className="mt-3 inline-flex h-7 items-center gap-1.5 rounded-md border border-border bg-surface-1 px-2 text-[11px] text-text-secondary transition-colors hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-60"
                                >
                                  {savingArtifactId === artifact.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
                                  {copy.saveEditable}
                                </button>
                              </div>
                            ))}
                          </div>
                        )}
                        {artifactPaths.length > 0 && (
                          <div>
                            <div className="mb-1 text-xs font-medium text-text-tertiary">{copy.artifactPaths}</div>
                            <div className="space-y-1">
                              {artifactPaths.map((path) => (
                                <div key={path} className="flex items-center gap-1.5 rounded-md border border-border/60 bg-surface-0/75 px-2 py-1.5">
                                  <span className="min-w-0 flex-1 truncate text-xs text-text-secondary" title={path}>
                                    {path}
                                  </span>
                                  <button
                                    type="button"
                                    title={copy.openFile}
                                    aria-label={`${copy.openFile}: ${path}`}
                                    onClick={() => void handleOpenArtifactPath(path)}
                                    className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                                  >
                                    <ExternalLink className="h-3.5 w-3.5" />
                                  </button>
                                  <button
                                    type="button"
                                    title={copy.showInFolder}
                                    aria-label={`${copy.showInFolder}: ${path}`}
                                    onClick={() => void handleOpenArtifactPath(path, true)}
                                    className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                                  >
                                    <FolderOpen className="h-3.5 w-3.5" />
                                  </button>
                                </div>
                              ))}
                            </div>
                          </div>
                        )}
                        <div>
                          <div className="mb-2 flex items-center justify-between gap-2">
                            <div className="flex items-center gap-2 text-xs font-medium text-text-tertiary">
                              <History className="h-3.5 w-3.5" />
                              {copy.savedArtifacts}
                            </div>
                            <span className="rounded-md border border-border/60 bg-surface-0 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                              {savedArtifacts.length}
                            </span>
                          </div>
                          {savedArtifacts.length === 0 ? (
                            <div className="rounded-md border border-dashed border-border px-3 py-5 text-center text-xs text-text-tertiary">
                              {copy.noSavedArtifacts}
                            </div>
                          ) : (
                            <div className="space-y-2">
                              {savedArtifacts.map((artifact) => {
                                const isEditing = editingArtifactId === artifact.id;
                                const versions = artifactVersions[artifact.id] ?? [];
                                return (
                                  <div key={artifact.id} className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                                    <div className="flex items-start justify-between gap-2">
                                      <div className="min-w-0">
                                        <div className="truncate text-sm font-medium text-text-primary">{artifact.title}</div>
                                        <div className="mt-1 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
                                          <RiskPill>{artifact.kind}</RiskPill>
                                          <RiskPill>v{artifact.version}</RiskPill>
                                          <RiskPill>{formatTime(artifact.updatedAt)}</RiskPill>
                                        </div>
                                      </div>
                                      <button
                                        type="button"
                                        onClick={() => void handleStartArtifactEdit(artifact)}
                                        className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-border bg-surface-1 px-2 text-[11px] text-text-secondary transition-colors hover:bg-surface-2"
                                      >
                                        <Pencil className="h-3.5 w-3.5" />
                                        {copy.editArtifact}
                                      </button>
                                    </div>
                                    {artifact.summary && !isEditing && (
                                      <p className="mt-2 line-clamp-2 text-xs leading-5 text-text-secondary">{artifact.summary}</p>
                                    )}
                                    {isEditing ? (
                                      <div className="mt-3 space-y-2">
                                        <label className="block text-[11px] font-medium text-text-tertiary">
                                          {copy.titleLabel}
                                          <input
                                            value={artifactDraft.title}
                                            onChange={(event) => setArtifactDraft((current) => ({ ...current, title: event.target.value }))}
                                            className="mt-1 h-8 w-full rounded-md border border-border bg-surface-1 px-2 text-xs text-text-primary outline-none focus:border-accent"
                                          />
                                        </label>
                                        <label className="block text-[11px] font-medium text-text-tertiary">
                                          {copy.summaryLabel}
                                          <textarea
                                            value={artifactDraft.summary}
                                            onChange={(event) => setArtifactDraft((current) => ({ ...current, summary: event.target.value }))}
                                            rows={2}
                                            className="mt-1 w-full resize-y rounded-md border border-border bg-surface-1 px-2 py-1.5 text-xs leading-5 text-text-primary outline-none focus:border-accent"
                                          />
                                        </label>
                                        <label className="block text-[11px] font-medium text-text-tertiary">
                                          {copy.contentLabel}
                                          <textarea
                                            value={artifactDraft.content}
                                            onChange={(event) => setArtifactDraft((current) => ({ ...current, content: event.target.value }))}
                                            rows={6}
                                            className="mt-1 w-full resize-y rounded-md border border-border bg-surface-1 px-2 py-1.5 font-mono text-xs leading-5 text-text-primary outline-none focus:border-accent"
                                          />
                                        </label>
                                        <div className="flex flex-wrap gap-2">
                                          <button
                                            type="button"
                                            onClick={() => void handleSaveArtifact(artifact)}
                                            disabled={savingArtifactId === artifact.id}
                                            className="inline-flex h-8 items-center gap-1.5 rounded-md border border-accent/35 bg-accent/10 px-2.5 text-xs text-accent transition-colors hover:bg-accent/15 disabled:cursor-not-allowed disabled:opacity-60"
                                          >
                                            {savingArtifactId === artifact.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
                                            {copy.saveArtifact}
                                          </button>
                                          <button
                                            type="button"
                                            onClick={() => setEditingArtifactId(null)}
                                            className="inline-flex h-8 items-center rounded-md border border-border bg-surface-1 px-2.5 text-xs text-text-secondary transition-colors hover:bg-surface-2"
                                          >
                                            {copy.cancelEdit}
                                          </button>
                                        </div>
                                      </div>
                                    ) : (
                                      <p className="mt-2 line-clamp-3 whitespace-pre-wrap text-xs leading-5 text-text-secondary">
                                        {artifact.content}
                                      </p>
                                    )}
                                    {versions.length > 0 && (
                                      <div className="mt-3 border-t border-border/60 pt-2">
                                        <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                                          <History className="h-3.5 w-3.5" />
                                          {copy.versionHistory}
                                        </div>
                                        <div className="flex flex-wrap gap-1">
                                          {versions.slice(0, 5).map((version) => (
                                            <span key={version.id} className="rounded-md border border-border/60 bg-surface-1 px-1.5 py-0.5 text-[10px] text-text-tertiary" title={version.title}>
                                              v{version.version} · {formatTime(version.createdAt)}
                                            </span>
                                          ))}
                                        </div>
                                      </div>
                                    )}
                                  </div>
                                );
                              })}
                            </div>
                          )}
                        </div>
                      </div>
                    )}
                  </section>

                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center gap-2 text-sm font-semibold text-text-primary">
                      <Boxes className="h-4 w-4 text-accent" />
                      {copy.history}
                    </div>
                    <div className="space-y-2">
                      {events.slice().reverse().map((event) => (
                        <div key={event.id} className="flex items-start gap-2 rounded-md border border-border/60 bg-surface-0/75 px-3 py-2">
                          <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />
                          <div className="min-w-0 flex-1">
                            <div className="flex flex-wrap items-center gap-1.5 text-xs text-text-primary">
                              <span>{event.label}</span>
                              {event.status && <span className="text-text-tertiary">{event.status}</span>}
                            </div>
                            <div className="mt-0.5 text-[11px] text-text-tertiary">{formatTime(event.createdAt)}</div>
                          </div>
                        </div>
                      ))}
                      {events.length === 0 && (
                        <div className="rounded-md border border-dashed border-border px-3 py-6 text-center text-sm text-text-tertiary">
                          {copy.noArtifacts}
                        </div>
                      )}
                    </div>
                  </section>
                </div>

                <div className="space-y-4">
                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                        <Brain className="h-4 w-4 text-accent" />
                        {copy.projectMemory}
                      </div>
                      {selected.projectId && (
                        <button
                          type="button"
                          onClick={() => void handleSaveMemory()}
                          disabled={savingMemory}
                          className="inline-flex h-8 items-center gap-1.5 rounded-md border border-border bg-surface-0 px-2.5 text-xs text-text-secondary transition-colors hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-60"
                        >
                          {savingMemory ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
                          {copy.saveMemory}
                        </button>
                      )}
                    </div>
                    {!selected.projectId ? (
                      <div className="rounded-md border border-dashed border-border px-3 py-6 text-sm text-text-tertiary">
                        {copy.noProject}
                      </div>
                    ) : projectMemories.length === 0 ? (
                      <div className="rounded-md border border-dashed border-border px-3 py-6 text-sm text-text-tertiary">
                        {copy.noMemory}
                      </div>
                    ) : (
                      <div className="space-y-2">
                        {projectMemories.slice(0, 5).map((memory) => (
                          <div key={memory.id} className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                            <div className="mb-1 flex items-center gap-2">
                              <span className="rounded-md border border-border/60 px-1.5 py-0.5 text-[10px] text-text-tertiary">{memory.kind}</span>
                              {memory.pinned && <ShieldCheck className="h-3.5 w-3.5 text-success" />}
                              <span className="min-w-0 truncate text-xs font-medium text-text-primary">{memory.title || memory.kind}</span>
                            </div>
                            <p className="line-clamp-3 text-xs leading-5 text-text-secondary">{memory.content}</p>
                          </div>
                        ))}
                      </div>
                    )}
                  </section>

                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                        <ShieldAlert className="h-4 w-4 text-accent" />
                        {copy.toolRiskMap}
                      </div>
                      <span className="rounded-full border border-danger/25 bg-danger/10 px-2 py-1 text-[11px] text-danger">
                        {highRiskTools.length} {copy.highRisk}
                      </span>
                    </div>
                    <div className="max-h-[520px] space-y-2 overflow-y-auto pr-1">
                      {toolAccess.map((tool) => (
                        <div key={tool.name} className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                          <div className="flex items-start justify-between gap-2">
                            <div className="min-w-0">
                              <div className="flex items-center gap-1.5">
                                {tool.canExecute ? <TerminalSquare className="h-3.5 w-3.5 text-warning" /> : tool.canAccessNetwork ? <Network className="h-3.5 w-3.5 text-accent" /> : <ShieldCheck className="h-3.5 w-3.5 text-text-tertiary" />}
                                <span className="truncate text-sm font-medium text-text-primary">{tool.name}</span>
                              </div>
                              <div className="mt-1 text-[11px] text-text-tertiary">{formatCategory(tool.category)}</div>
                            </div>
                            <span className={`shrink-0 rounded-full border px-2 py-1 text-[10px] ${tool.riskLevel === 'high' ? 'border-danger/25 bg-danger/10 text-danger' : tool.riskLevel === 'medium' ? 'border-warning/25 bg-warning/10 text-warning' : 'border-border/60 bg-surface-1 text-text-secondary'}`}>
                              {tool.riskLevel}
                            </span>
                          </div>
                          <div className="mt-2 flex flex-wrap gap-1">
                            {tool.canRead && <RiskPill>{copy.read}</RiskPill>}
                            {tool.canWrite && <RiskPill>{copy.write}</RiskPill>}
                            {tool.canExecute && <RiskPill>{copy.execute}</RiskPill>}
                            {tool.canAccessNetwork && <RiskPill>{copy.network}</RiskPill>}
                            {tool.needsApproval && <RiskPill>{copy.approval}</RiskPill>}
                            <RiskPill>
                              {copy.policy}: {approvalPolicyLabel(approvalPolicyByTool.get(tool.name), tool.needsApproval, copy)}
                            </RiskPill>
                          </div>
                          <p className="mt-2 line-clamp-2 text-[11px] leading-5 text-text-tertiary">
                            {tool.riskReason}
                          </p>
                        </div>
                      ))}
                    </div>
                  </section>

                  <section className="rounded-lg border border-border/70 bg-surface-1/70 p-4">
                    <div className="mb-3 flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                        <History className="h-4 w-4 text-accent" />
                        {copy.resumeCheckpoint}
                      </div>
                      {canResume && (
                        <button
                          type="button"
                          onClick={() => void handleResume()}
                          disabled={resumingId === selected.run.id}
                          className="inline-flex h-8 items-center gap-1.5 rounded-md border border-accent/35 bg-accent/10 px-2.5 text-xs text-accent transition-colors hover:bg-accent/15 disabled:cursor-not-allowed disabled:opacity-60"
                        >
                          {resumingId === selected.run.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Play className="h-3.5 w-3.5" />}
                          {copy.resume}
                        </button>
                      )}
                    </div>
                    {!latestCheckpoint ? (
                      <div className="rounded-md border border-dashed border-border px-3 py-6 text-sm text-text-tertiary">
                        {copy.noCheckpoint}
                      </div>
                    ) : (
                      <div className="space-y-2">
                        <div className="flex flex-wrap gap-1 text-[10px] text-text-tertiary">
                          <RiskPill>{latestCheckpoint.phase}</RiskPill>
                          <RiskPill>{latestCheckpoint.status}</RiskPill>
                          <RiskPill>{formatTime(latestCheckpoint.createdAt)}</RiskPill>
                        </div>
                        <div className="rounded-md border border-border/60 bg-surface-0/75 p-3">
                          <div className="mb-1 text-[11px] font-medium text-text-tertiary">{copy.checkpointReason}</div>
                          <p className="text-xs leading-5 text-text-secondary">{latestCheckpoint.reason}</p>
                        </div>
                        <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-md border border-border/60 bg-surface-0/75 p-3 text-xs leading-5 text-text-secondary">
                          {latestCheckpoint.resumePrompt}
                        </pre>
                      </div>
                    )}
                  </section>
                </div>
              </div>
            </div>
          )}
        </section>
      </main>
    </div>
  );
}

export default TaskCenterPage;
