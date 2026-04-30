import {
  AlertTriangle,
  Bot,
  BrainCircuit,
  CheckCircle2,
  ChevronDown,
  ClipboardList,
  Cpu,
  Database,
  FolderOpen,
  Gauge,
  Loader2,
  Plus,
  Route,
  Scissors,
  ShieldCheck,
  Sparkles,
  Wrench,
  Zap,
} from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from '../../i18n';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  Conversation,
  ConversationMessage,
} from '../../types/conversation';
import type { ToolCallEvent } from '../../lib/useAgentStream';
import {
  findLatestPlanArtifact,
  findLatestVerificationArtifact,
  type VerificationOverallStatus,
} from '../../lib/taskArtifacts';
import { findVisibleSubagentRuns } from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import { SubagentCard } from './SubagentCard';

interface TokenUsage {
  promptTokens: number;
  totalTokens: number;
  contextWindow: number;
  completionTokens: number;
  thinkingTokens: number;
  isEstimated: boolean;
  source: 'live' | 'cached' | 'estimated';
}

interface SourceSelectionSummary {
  selectedCount: number;
  totalCount: number;
  loading: boolean;
}

interface RuntimeProfile {
  provider: string;
  model: string;
  contextWindow: number;
  reasoningEnabled: boolean;
  reasoningDetail: string;
  sourceAuthority: string;
  toolPolicy: string;
  memoryPolicy: string;
}

type EvidenceLevel = 'high' | 'medium' | 'low' | 'none';

interface ChatRunOverviewProps {
  conversationTitle?: string | null;
  collectionContext?: Conversation['collectionContext'] | null;
  sourceSummary: SourceSelectionSummary;
  isStreaming?: boolean;
  routeKind?: string | null;
  turnStatus?: string | null;
  evidenceLevel: EvidenceLevel;
  evidenceCount: number;
  tokenUsage?: TokenUsage | null;
  runtimeProfile?: RuntimeProfile | null;
  finishReason?: string | null;
  contextOverflow?: boolean;
  rateLimited?: boolean;
  lastCached?: boolean;
  isCompacting?: boolean;
  onCompact?: () => void;
  onStartNewChat?: () => void;
  messages: ConversationMessage[];
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
  taskEvents?: AgentTaskRunEvent[];
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

function formatRouteKind(routeKind: string): string {
  return routeKind
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/^./, (char) => char.toUpperCase());
}

function formatTurnStatus(
  status: string | null | undefined,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (status) {
    case 'success':
      return t('chat.investigationStatusReady');
    case 'cached':
      return t('chat.cached');
    case 'error':
      return t('chat.investigationStatusNeedsAttention');
    case 'running':
      return t('chat.investigationStatusInvestigating');
    case 'max_iterations':
      return t('chat.investigationStatusIncomplete');
    case 'cancelled':
      return t('chat.investigationStatusStopped');
    default:
      return t('chat.investigationStatusIdle');
  }
}

function evidenceTone(level: EvidenceLevel) {
  switch (level) {
    case 'high':
      return 'border-emerald-500/20 bg-emerald-500/10 text-emerald-300';
    case 'medium':
      return 'border-cyan-500/20 bg-cyan-500/10 text-cyan-300';
    case 'low':
      return 'border-amber-500/20 bg-amber-500/10 text-amber-300';
    case 'none':
    default:
      return 'border-border/70 bg-surface-1/70 text-text-secondary';
  }
}

function evidenceLabel(
  level: EvidenceLevel,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (level) {
    case 'high':
      return t('chat.investigationEvidenceHigh');
    case 'medium':
      return t('chat.investigationEvidenceMedium');
    case 'low':
      return t('chat.investigationEvidenceLow');
    case 'none':
    default:
      return t('chat.investigationEvidenceNone');
  }
}

function taskStatusLabel(status: string | null | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (status) {
    case 'queued':
      return t('chat.taskRunQueued');
    case 'running':
      return t('chat.taskRunRunning');
    case 'waiting_approval':
      return t('chat.taskRunWaitingApproval');
    case 'cancelling':
      return t('chat.taskRunCancelling');
    case 'cancelled':
      return t('chat.taskRunCancelled');
    case 'timed_out':
      return t('chat.taskRunTimedOut');
    case 'failed':
      return t('chat.taskRunFailed');
    case 'completed':
      return t('chat.taskRunCompleted');
    default:
      return status || t('chat.taskRunUnknown');
  }
}

function eventStatusLabel(status: string | null | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (status) {
    case 'queued':
    case 'running':
    case 'waiting_approval':
    case 'cancelling':
    case 'cancelled':
    case 'timed_out':
    case 'failed':
    case 'completed':
      return taskStatusLabel(status, t);
    case 'pending':
      return t('chat.verificationPending');
    case 'success':
      return t('common.success');
    case 'error':
      return t('common.error');
    case 'muted':
      return '';
    default:
      return status || '';
  }
}

function taskEventLabel(event: AgentTaskRunEvent, t: ReturnType<typeof useTranslation>['t']) {
  if (event.label.startsWith('Route selected: ')) {
    return t('chat.taskEventRouteSelectedWithRoute', {
      route: formatRouteKind(event.label.slice('Route selected: '.length).trim()),
    });
  }

  switch (event.label) {
    case 'Task queued':
      return t('chat.taskEventQueued');
    case 'Agent started':
      return t('chat.taskEventStarted');
    case 'Route selected':
      return t('chat.taskEventRouteSelected');
    case 'Generating answer':
      return t('chat.taskPhaseGenerating');
    case 'Reasoning':
      return t('chat.taskPhaseReasoning');
    case 'Finalizing answer':
      return t('chat.taskPhaseFinalizing');
    case 'Final answer produced':
      return t('chat.taskEventFinalAnswer');
    case 'Agent execution failed':
      return t('chat.taskEventFailed');
    case 'Conversation context compacted':
      return t('chat.taskEventCompacted');
    case 'Approval resolved':
      return t('chat.taskEventApprovalResolved');
    case 'Stop requested':
      return t('chat.taskEventStopRequested');
    case 'Stopped by user':
      return t('chat.taskEventStopped');
    default:
      return event.label;
  }
}

function taskPhaseLabel(phase: string | null | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (phase) {
    case 'queued':
      return t('chat.taskPhaseQueued');
    case 'initializing':
      return t('chat.taskPhaseInitializing');
    case 'routing':
      return t('chat.taskPhaseRouting');
    case 'reasoning':
      return t('chat.taskPhaseReasoning');
    case 'tooling':
      return t('chat.taskPhaseTooling');
    case 'approval':
      return t('chat.taskPhaseApproval');
    case 'generating':
      return t('chat.taskPhaseGenerating');
    case 'finalizing':
      return t('chat.taskPhaseFinalizing');
    case 'cancelling':
      return t('chat.taskPhaseCancelling');
    case 'done':
      return t('chat.taskPhaseDone');
    default:
      return phase || t('chat.taskRunUnknown');
  }
}

function verificationStatusLabel(
  status: VerificationOverallStatus,
  t: ReturnType<typeof useTranslation>['t'],
) {
  switch (status) {
    case 'passed':
      return t('chat.verificationPassed');
    case 'failed':
      return t('chat.verificationFailed');
    case 'partial':
      return t('chat.verificationPartial');
    case 'pending':
    default:
      return t('chat.verificationPending');
  }
}

export function ChatRunOverview({
  conversationTitle,
  collectionContext,
  sourceSummary,
  isStreaming = false,
  routeKind,
  turnStatus,
  evidenceLevel,
  evidenceCount,
  tokenUsage,
  runtimeProfile,
  finishReason,
  contextOverflow = false,
  rateLimited = false,
  lastCached = false,
  isCompacting = false,
  onCompact,
  onStartNewChat,
  messages,
  toolCalls,
  taskRun,
  taskEvents = [],
}: ChatRunOverviewProps) {
  const { t } = useTranslation();
  const shouldStartExpanded =
    Boolean(contextOverflow || rateLimited || taskRun?.status === 'waiting_approval');
  const [expanded, setExpanded] = useState(shouldStartExpanded);

  const plan = useMemo(
    () => findLatestPlanArtifact(messages, toolCalls),
    [messages, toolCalls],
  );
  const verification = useMemo(
    () => findLatestVerificationArtifact(messages, toolCalls),
    [messages, toolCalls],
  );
  const subagents = useMemo(
    () => findVisibleSubagentRuns(messages, toolCalls, 4),
    [messages, toolCalls],
  );

  const usage = tokenUsage && tokenUsage.contextWindow > 0 ? tokenUsage : null;
  const usagePercent = usage ? Math.min(100, (usage.promptTokens / usage.contextWindow) * 100) : 0;
  const usagePercentRounded = Math.round(usagePercent);
  const usageSourceLabel = usage
    ? usage.source === 'live'
      ? t('chat.contextUsageLive')
      : usage.source === 'cached'
        ? t('chat.contextUsageCached')
        : t('chat.contextUsageEstimated')
    : t('chat.contextNoUsage');

  const scopeLabel = sourceSummary.loading
    ? t('common.loading')
    : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
      ? t('chat.allSources')
      : `${sourceSummary.selectedCount} / ${sourceSummary.totalCount}`;
  const scopeHint = sourceSummary.loading
    ? t('common.loading')
    : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
      ? t('chat.investigationScopeAllHint')
      : t('chat.investigationScopeSelectedHint');

  const routeLabel = routeKind
    ? formatRouteKind(routeKind)
    : (isStreaming ? t('chat.investigationRouteLiveDefault') : t('chat.investigationRouteIdle'));
  const title = collectionContext?.title || conversationTitle || t('chat.investigationNew');
  const contextSummary = collectionContext?.description?.trim()
    || collectionContext?.queryText?.trim()
    || t('chat.investigationDefaultSummary');
  const statusLabel = taskRun
    ? taskStatusLabel(taskRun.status, t)
    : isStreaming
      ? t('chat.investigationStatusInvestigating')
      : formatTurnStatus(turnStatus, t);
  const taskRunning = taskRun
    ? ['queued', 'running', 'waiting_approval', 'cancelling'].includes(taskRun.status)
    : isStreaming;
  const recentTaskEvents = taskEvents.slice(-5).reverse();
  const runningSubagents = subagents.filter(run => run.status === 'running').length;
  const planCompleted = plan?.steps.filter(step => step.status === 'completed').length ?? 0;
  const planTotal = plan?.steps.length ?? 0;
  const verificationSummary = verification?.overallStatus ?? null;
  const modelLabel = runtimeProfile
    ? `${runtimeProfile.provider} / ${runtimeProfile.model}`
    : t('chat.contextNoModel');
  const runtimeContextLabel = runtimeProfile?.contextWindow
    ? t('chat.contextWindowValue', { value: formatTokens(runtimeProfile.contextWindow) })
    : usage
      ? t('chat.contextWindowValue', { value: formatTokens(usage.contextWindow) })
      : t('chat.contextWindowPending');
  const canCompact = Boolean(onCompact);
  const canStartNewChat = Boolean(onStartNewChat);
  const showContextActions = (contextOverflow || usagePercent >= 95) && (canCompact || canStartNewChat);

  const riskLabel = rateLimited
    ? t('chat.rateLimited')
    : contextOverflow || usagePercent >= 95
      ? t('chat.contextOverflow')
      : finishReason === 'length'
        ? t('chat.truncated')
        : finishReason === 'contentfilter'
          ? t('chat.contentFiltered')
          : isStreaming
            ? t('chat.thinking')
            : t('chat.contextHealthy');
  const riskTone = rateLimited || finishReason === 'length'
    ? 'border-amber-500/25 bg-amber-500/10 text-amber-300'
    : contextOverflow || usagePercent >= 95 || finishReason === 'contentfilter'
      ? 'border-red-500/25 bg-red-500/10 text-red-300'
      : 'border-border/70 bg-surface-1/70 text-text-secondary';
  const RiskIcon = rateLimited || contextOverflow || usagePercent >= 95 || finishReason
    ? AlertTriangle
    : ShieldCheck;

  return (
    <div className="shrink-0 border-b border-border/60 bg-surface-1/80 px-3 py-2.5 backdrop-blur">
      <details
        open={expanded}
        onToggle={(event) => setExpanded(event.currentTarget.open)}
        className="group rounded-xl border border-border/70 bg-surface-0/85"
      >
        <summary className="flex cursor-pointer list-none flex-wrap items-center gap-2 px-3 py-2.5 text-sm [&::-webkit-details-marker]:hidden">
          <div className="flex min-w-[180px] flex-1 items-center gap-2">
            <span className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border ${taskRunning ? 'border-accent/25 bg-accent/10' : 'border-border/70 bg-surface-1'}`}>
              {taskRunning
                ? <Loader2 className="h-4 w-4 animate-spin text-accent" />
                : <Sparkles className="h-4 w-4 text-accent" />}
            </span>
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold text-text-primary">{title}</div>
              <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px] text-text-tertiary">
                <span className="truncate">{statusLabel}</span>
                <span className="text-text-tertiary/70">/</span>
                <span className="truncate">{taskRun ? taskPhaseLabel(taskRun.phase, t) : routeLabel}</span>
              </div>
            </div>
          </div>

          <span className={`inline-flex max-w-[180px] items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] ${evidenceTone(evidenceLevel)}`}>
            <ShieldCheck className="h-3 w-3 shrink-0" />
            <span className="truncate">{evidenceLabel(evidenceLevel, t)}</span>
          </span>

          <span className="inline-flex max-w-[170px] items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary">
            <FolderOpen className="h-3 w-3 shrink-0 text-text-tertiary" />
            <span className="truncate">{scopeLabel}</span>
          </span>

          <span className="hidden max-w-[220px] items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary lg:inline-flex">
            <Cpu className="h-3 w-3 shrink-0 text-text-tertiary" />
            <span className="truncate">{modelLabel}</span>
          </span>

          <span className={`inline-flex max-w-[170px] items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] ${riskTone}`}>
            <RiskIcon className="h-3 w-3 shrink-0" />
            <span className="truncate">{riskLabel}</span>
          </span>

          <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-text-tertiary transition-colors group-hover:text-text-secondary">
            {expanded ? t('common.collapse') : t('common.expand')}
            <ChevronDown className="h-3.5 w-3.5 shrink-0 transition-transform group-open:rotate-180" />
          </span>
        </summary>

        <div className="border-t border-border/60 px-3 pb-3 pt-2.5">
          <div className="grid gap-2 xl:grid-cols-[1.15fr_1fr]">
            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                    <Route className="h-3.5 w-3.5" />
                    {t('chat.taskRunLabel')}
                  </div>
                  <div className="mt-1 truncate text-sm font-medium text-text-primary">
                    {taskRun?.title || title}
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1.5 text-[11px] text-text-tertiary">
                    <span>{taskRun ? taskPhaseLabel(taskRun.phase, t) : routeLabel}</span>
                    {taskRun && routeLabel && <span>{routeLabel}</span>}
                    {taskRun?.summary && <span>{taskRun.summary}</span>}
                  </div>
                </div>
                <span className="shrink-0 rounded-full border border-border/70 bg-surface-0/80 px-2 py-1 text-[11px] text-text-secondary">
                  {statusLabel}
                </span>
              </div>
              {taskRun?.errorMessage && (
                <div className="mt-2 rounded-md border border-red-500/20 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-300">
                  {taskRun.errorMessage}
                </div>
              )}
              {recentTaskEvents.length > 0 && (
                <ul className="mt-2 space-y-1">
                  {recentTaskEvents.map(event => {
                    const translatedStatus = eventStatusLabel(event.status, t);
                    return (
                      <li key={event.id} className="flex items-center gap-1.5 text-[11px] text-text-tertiary">
                        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent/70" />
                        <span className="truncate">{taskEventLabel(event, t)}</span>
                        {translatedStatus && (
                          <span className="shrink-0 text-text-tertiary/80">{translatedStatus}</span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
            </section>

            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Database className="h-3.5 w-3.5" />
                {t('chat.investigationLabel')}
              </div>
              <p className="mt-1 line-clamp-2 text-sm text-text-secondary">
                {contextSummary}
              </p>
              <div className="mt-2 grid gap-1.5 sm:grid-cols-3">
                <span className="rounded-md border border-border/60 bg-surface-0/70 px-2 py-1.5 text-[11px] text-text-secondary">
                  {t('chat.investigationRouteLabel')}: {routeLabel}
                </span>
                <span className="rounded-md border border-border/60 bg-surface-0/70 px-2 py-1.5 text-[11px] text-text-secondary">
                  {t('chat.answerEvidence')}: {evidenceCount > 0 ? evidenceCount : t('chat.investigationSupportingNone')}
                </span>
                <span className="rounded-md border border-border/60 bg-surface-0/70 px-2 py-1.5 text-[11px] text-text-secondary">
                  {t('chat.contextScopeLabel')}: {scopeHint}
                </span>
              </div>
            </section>
          </div>

          <div className="mt-2 grid gap-2 lg:grid-cols-3">
            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Gauge className="h-3.5 w-3.5" />
                {t('chat.contextBudgetLabel')}
              </div>
              {usage ? (
                <>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium tabular-nums text-text-primary">
                      {t('chat.tokenUsagePercent', { percent: usagePercentRounded })}
                    </span>
                    <span className="text-[11px] text-text-tertiary">{usageSourceLabel}</span>
                  </div>
                  <div className="mt-1 text-[11px] tabular-nums text-text-secondary">
                    {t('chat.tokenUsage', {
                      used: formatTokens(usage.promptTokens),
                      total: formatTokens(usage.contextWindow),
                    })}
                  </div>
                </>
              ) : (
                <div className="text-sm text-text-secondary">{lastCached ? t('chat.cached') : usageSourceLabel}</div>
              )}
              {showContextActions && (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {canCompact && (
                    <button
                      type="button"
                      onClick={() => onCompact?.()}
                      disabled={isCompacting}
                      className="inline-flex items-center gap-1 rounded-md border border-border/60 bg-surface-0/80 px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      {isCompacting ? <Loader2 className="h-3 w-3 animate-spin" /> : <Scissors className="h-3 w-3" />}
                      {isCompacting ? t('chat.compacting') : t('chat.compact')}
                    </button>
                  )}
                  {canStartNewChat && (
                    <button
                      type="button"
                      onClick={() => onStartNewChat?.()}
                      className="inline-flex items-center gap-1 rounded-md border border-border/60 bg-surface-0/80 px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-surface-2"
                    >
                      <Plus className="h-3 w-3" />
                      {t('chat.startNewChat')}
                    </button>
                  )}
                </div>
              )}
            </section>

            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Cpu className="h-3.5 w-3.5" />
                {t('chat.contextRuntimeModel')}
              </div>
              <div className="truncate text-sm font-medium text-text-primary" title={modelLabel}>
                {modelLabel}
              </div>
              <div className="mt-1 text-[11px] text-text-secondary">{runtimeContextLabel}</div>
            </section>

            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <BrainCircuit className="h-3.5 w-3.5" />
                {t('chat.contextReasoningLabel')}
              </div>
              <div className="text-sm text-text-primary">
                {runtimeProfile?.reasoningEnabled
                  ? t('chat.contextReasoningEnabled')
                  : t('chat.contextReasoningDisabled')}
              </div>
              <div className="mt-1 truncate text-[11px] text-text-secondary" title={runtimeProfile?.reasoningDetail}>
                {runtimeProfile?.reasoningDetail ?? t('chat.contextReasoningOff')}
              </div>
            </section>
          </div>

          <div className="mt-2 grid gap-2 lg:grid-cols-2">
            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Wrench className="h-3.5 w-3.5" />
                {t('chat.contextToolPolicyLabel')}
              </div>
              <div className="text-sm text-text-primary">
                {runtimeProfile?.sourceAuthority ?? t('chat.contextDefaultSourceAuthority')}
              </div>
              <div className="mt-1 text-[11px] text-text-secondary">
                {runtimeProfile?.toolPolicy ?? t('chat.contextDefaultToolPolicy')}
              </div>
            </section>

            <section className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Zap className="h-3.5 w-3.5" />
                {t('chat.contextStatusLabel')}
              </div>
              <div className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] ${riskTone}`}>
                <RiskIcon className="h-3 w-3" />
                {riskLabel}
              </div>
            </section>
          </div>

          {(subagents.length > 0 || plan || verification) && (
            <div className="mt-2 grid max-h-[240px] gap-2 overflow-y-auto lg:grid-cols-2">
              {subagents.length > 0 && (
                <section className="space-y-2 rounded-lg border border-border/60 bg-surface-1/60 px-2 py-2 lg:col-span-2">
                  <div className="flex items-center gap-1.5 px-1 text-[11px] font-medium text-text-tertiary">
                    <Bot className="h-3.5 w-3.5" />
                    {runningSubagents > 0
                      ? t('chat.helpersActive', { active: runningSubagents, total: subagents.length })
                      : t('chat.helpersCount', { count: subagents.length })}
                  </div>
                  {subagents.map(run => (
                    <SubagentCard
                      key={run.id}
                      run={run}
                      compact
                      defaultOpen={run.status === 'running'}
                    />
                  ))}
                </section>
              )}
              {plan && (
                <div className="min-w-0">
                  <div className="mb-1 flex items-center gap-1.5 px-1 text-[11px] text-text-tertiary">
                    <ClipboardList className="h-3.5 w-3.5" />
                    {t('chat.planStepsCompleted', { completed: planCompleted, total: planTotal })}
                  </div>
                  <PlanPanel plan={plan} compact />
                </div>
              )}
              {verification && (
                <div className="min-w-0">
                  <div className="mb-1 flex items-center gap-1.5 px-1 text-[11px] text-text-tertiary">
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    {verificationSummary
                      ? t('chat.verificationStatus', {
                        status: verificationStatusLabel(verificationSummary, t),
                      })
                      : t('chat.verificationLabel')}
                  </div>
                  <VerificationPanel verification={verification} compact />
                </div>
              )}
            </div>
          )}
        </div>
      </details>
    </div>
  );
}
