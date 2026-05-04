import { Bot, CheckCircle2, ChevronDown, ClipboardList, Loader2, Route, ShieldCheck } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from '../../i18n';
import type { AgentTaskRun, AgentTaskRunEvent, ConversationMessage } from '../../types/conversation';
import type { ToolCallEvent } from '../../lib/useAgentStream';
import {
  findLatestPlanArtifact,
  findLatestVerificationArtifact,
  type VerificationOverallStatus,
} from '../../lib/taskArtifacts';
import { findVisibleSubagentRuns } from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import { SubagentCard } from './SubagentCard';

interface TaskBoardProps {
  messages: ConversationMessage[];
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
  taskEvents?: AgentTaskRunEvent[];
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

export function TaskBoard({
  messages,
  toolCalls,
  taskRun,
  taskEvents = [],
}: TaskBoardProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
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

  if (!taskRun && !plan && !verification && subagents.length === 0) {
    return null;
  }

  const completed = plan?.steps.filter(step => step.status === 'completed').length ?? 0;
  const total = plan?.steps.length ?? 0;
  const verificationSummary = verification?.overallStatus ?? null;
  const runningSubagents = subagents.filter(run => run.status === 'running').length;
  const taskRunning = taskRun
    ? ['queued', 'running', 'waiting_approval', 'cancelling'].includes(taskRun.status)
    : false;
  const recentTaskEvents = taskEvents.slice(-5).reverse();

  return (
    <div
      data-testid="task-board"
      className="shrink-0 border-t border-border/60 bg-surface-0/90 px-3 py-2 backdrop-blur"
    >
      <details
        open={expanded}
        onToggle={(event) => setExpanded(event.currentTarget.open)}
        className="group rounded-lg border border-border/60 bg-surface-1/55"
      >
        <summary className="flex cursor-pointer list-none items-center gap-1.5 px-2 py-1.5 text-xs text-text-secondary [&::-webkit-details-marker]:hidden">
          {subagents.length > 0 && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-primary">
              <Bot className="h-3 w-3 text-accent" />
              <span className="truncate">
                {runningSubagents > 0
                  ? t('chat.helpersActive', {
                      active: runningSubagents,
                      total: subagents.length,
                    })
                  : t('chat.helpersCount', { count: subagents.length })}
              </span>
            </span>
          )}

          {taskRun && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-primary">
              {taskRunning
                ? <Loader2 className="h-3 w-3 animate-spin text-accent" />
                : <Route className="h-3 w-3 text-accent" />}
              <span className="truncate">{taskStatusLabel(taskRun.status, t)}</span>
              <span className="hidden text-text-tertiary sm:inline">
                {taskPhaseLabel(taskRun.phase, t)}
              </span>
            </span>
          )}

          {plan && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-primary">
              <ClipboardList className="h-3 w-3 text-accent" />
              <span className="truncate">{t('chat.planLabel')}</span>
              <span className="tabular-nums text-text-tertiary">{completed}/{total}</span>
            </span>
          )}

          {verification && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-primary">
              <ShieldCheck className="h-3 w-3 text-accent" />
              <span className="truncate">
                {verificationSummary
                  ? t('chat.verificationStatus', {
                    status: verificationStatusLabel(verificationSummary, t),
                  })
                  : t('chat.verificationLabel')}
              </span>
            </span>
          )}

          <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary">
            <CheckCircle2 className="h-3 w-3 text-text-tertiary" />
            <span className="truncate">{t('chat.taskBoard')}</span>
          </span>

          <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-text-tertiary transition-colors group-hover:text-text-secondary">
            {t('chat.taskBoardDetails')}
            <ChevronDown className="h-3.5 w-3.5 shrink-0 transition-transform group-open:rotate-180" />
          </span>
        </summary>

        <div className="grid max-h-[200px] gap-1.5 overflow-y-auto border-t border-border/60 px-2 pb-2 pt-1.5 md:grid-cols-2">
          {taskRun && (
            <div className="rounded-lg border border-border/70 bg-surface-1/70 px-2 py-1.5 md:col-span-2">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <Route className="h-3.5 w-3.5 text-accent" />
                    <span className="text-[10px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
                      {t('chat.taskRunLabel')}
                    </span>
                  </div>
                  <div className="mt-0.5 truncate text-xs font-medium text-text-primary">
                    {taskRun.title || t('chat.taskRunDefaultTitle')}
                  </div>
                  <div className="mt-0.5 flex flex-wrap gap-1 text-[11px] text-text-tertiary">
                    <span>{taskPhaseLabel(taskRun.phase, t)}</span>
                    {taskRun.routeKind && <span>{taskRun.routeKind}</span>}
                    {taskRun.summary && <span>{taskRun.summary}</span>}
                  </div>
                  {taskRun.errorMessage && (
                    <div className="mt-1 text-[11px] text-danger">{taskRun.errorMessage}</div>
                  )}
                  {recentTaskEvents.length > 0 && (
                    <ul className="mt-2 space-y-1">
                      {recentTaskEvents.map(event => (
                        <li key={event.id} className="flex items-center gap-1.5 text-[11px] text-text-tertiary">
                          <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent/70" />
                          <span className="truncate">{event.label}</span>
                          {event.status && (
                            <span className="shrink-0 text-text-tertiary/80">{event.status}</span>
                          )}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
                <div className="shrink-0 rounded-full border border-border/70 bg-surface-0/80 px-2 py-1 text-[11px] text-text-secondary">
                  {taskStatusLabel(taskRun.status, t)}
                </div>
              </div>
            </div>
          )}
          {subagents.length > 0 && (
            <div className="space-y-2 md:col-span-2">
              {subagents.map(run => (
                <SubagentCard
                  key={run.id}
                  run={run}
                  compact
                  defaultOpen={run.status === 'running' || subagents.length === 1}
                />
              ))}
            </div>
          )}
          {plan && <PlanPanel plan={plan} compact />}
          {verification && <VerificationPanel verification={verification} compact />}
        </div>
      </details>
    </div>
  );
}
