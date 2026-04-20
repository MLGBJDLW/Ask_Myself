import { Bot, CheckCircle2, ChevronDown, ClipboardList, ShieldCheck } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from '../../i18n';
import type { ConversationMessage } from '../../types/conversation';
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
}: TaskBoardProps) {
  const { t } = useTranslation();
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

  if (!plan && !verification && subagents.length === 0) {
    return null;
  }

  const completed = plan?.steps.filter(step => step.status === 'completed').length ?? 0;
  const total = plan?.steps.length ?? 0;
  const verificationSummary = verification?.overallStatus ?? null;
  const runningSubagents = subagents.filter(run => run.status === 'running').length;

  return (
    <div className="shrink-0 border-b border-border/60 bg-surface-1/70 px-2 py-1.5 backdrop-blur">
      <details className="group rounded-lg border border-border/60 bg-surface-0/75">
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
          {subagents.length > 0 && (
            <div className="space-y-2 md:col-span-2">
              {subagents.map(run => (
                <SubagentCard
                  key={run.id}
                  run={run}
                  compact
                  defaultOpen={run.status === 'running'}
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
