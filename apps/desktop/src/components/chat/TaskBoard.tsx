import { ChevronDown, ClipboardList } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from '../../i18n';
import type { AgentTaskRun, ConversationMessage } from '../../types/conversation';
import type { ToolCallEvent } from '../../lib/useAgentStream';
import {
  findLatestPlanArtifact,
  findLatestSubtaskArtifacts,
  findLatestVerificationArtifact,
} from '../../lib/taskArtifacts';
import { PlanPanel, SubtaskPanel, VerificationPanel } from './TaskPanels';

interface TaskBoardProps {
  messages: ConversationMessage[];
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
}

export function TaskBoard({
  messages,
  toolCalls,
  taskRun,
}: TaskBoardProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const plan = useMemo(
    () => findLatestPlanArtifact(messages, toolCalls, taskRun?.plan),
    [messages, toolCalls, taskRun?.plan],
  );
  const verification = useMemo(
    () => findLatestVerificationArtifact(messages, toolCalls, taskRun?.artifacts),
    [messages, toolCalls, taskRun?.artifacts],
  );
  const subtasks = useMemo(
    () => findLatestSubtaskArtifacts(messages, toolCalls, taskRun?.artifacts),
    [messages, toolCalls, taskRun?.artifacts],
  );

  if (!plan && !verification && subtasks.length === 0) {
    return null;
  }

  const completed = plan?.steps.filter(step => step.status === 'completed').length ?? 0;
  const total = plan?.steps.length ?? 0;

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
          <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-primary">
            <ClipboardList className="h-3 w-3 text-accent" />
            <span className="truncate">{t('chat.taskBoard')}</span>
            {plan && (
              <span className="tabular-nums text-text-tertiary">{completed}/{total}</span>
            )}
          </span>

          <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-text-tertiary transition-colors group-hover:text-text-secondary">
            {t('chat.taskBoardDetails')}
            <ChevronDown className="h-3.5 w-3.5 shrink-0 transition-transform group-open:rotate-180" />
          </span>
        </summary>

        <div className="max-h-[260px] space-y-2 overflow-y-auto border-t border-border/60 px-2 pb-2 pt-1.5">
          {plan && <PlanPanel plan={plan} compact />}
          {subtasks.length > 0 && <SubtaskPanel subtasks={subtasks} compact />}
          {verification && <VerificationPanel verification={verification} compact />}
        </div>
      </details>
    </div>
  );
}
