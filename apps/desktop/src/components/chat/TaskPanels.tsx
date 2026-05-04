import { AlertTriangle, CheckCircle2, Circle, ClipboardList, GitBranch, Loader2, ShieldCheck, XCircle } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from '../../i18n';
import type {
  PlanArtifact,
  PlanStepArtifact,
  SubtaskRunArtifact,
  VerificationArtifact,
  VerificationCheckArtifact,
} from '../../lib/taskArtifacts';

function derivePlanCounts(plan: PlanArtifact) {
  const total = plan.steps.length;
  const completed = plan.steps.filter(step => step.status === 'completed').length;
  const inProgress = plan.steps.filter(step => step.status === 'in_progress').length;
  const pending = total - completed - inProgress;
  return {
    total,
    completed,
    inProgress,
    pending,
  };
}

function getPlanCounts(plan: PlanArtifact) {
  const derived = derivePlanCounts(plan);
  return {
    total: plan.counts?.total ?? derived.total,
    completed: plan.counts?.completed ?? derived.completed,
    inProgress: plan.counts?.inProgress ?? derived.inProgress,
    pending: plan.counts?.pending ?? derived.pending,
  };
}

function deriveVerificationCounts(verification: VerificationArtifact) {
  const total = verification.checks.length;
  const passed = verification.checks.filter(check => check.status === 'passed').length;
  const failed = verification.checks.filter(check => check.status === 'failed').length;
  const pending = verification.checks.filter(check => check.status === 'pending').length;
  const skipped = verification.checks.filter(check => check.status === 'skipped').length;
  return {
    total,
    passed,
    failed,
    pending,
    skipped,
  };
}

function getVerificationCounts(verification: VerificationArtifact) {
  const derived = deriveVerificationCounts(verification);
  return {
    total: verification.counts?.total ?? derived.total,
    passed: verification.counts?.passed ?? derived.passed,
    failed: verification.counts?.failed ?? derived.failed,
    pending: verification.counts?.pending ?? derived.pending,
    skipped: verification.counts?.skipped ?? derived.skipped,
  };
}

function getSubtaskCounts(subtasks: SubtaskRunArtifact[]) {
  return {
    total: subtasks.length,
    completed: subtasks.filter(subtask => subtask.status === 'completed').length,
    failed: subtasks.filter(subtask => subtask.status === 'failed').length,
    running: subtasks.filter(subtask => subtask.status === 'running').length,
    queued: subtasks.filter(subtask => subtask.status === 'queued').length,
  };
}

function subtaskStatusLabel(status: string, t: ReturnType<typeof useTranslation>['t']) {
  switch (status) {
    case 'queued':
      return t('chat.taskRunQueued');
    case 'running':
      return t('chat.taskRunRunning');
    case 'failed':
      return t('chat.taskRunFailed');
    case 'completed':
      return t('chat.taskRunCompleted');
    default:
      return status;
  }
}

function PlanStepRow({ step }: { step: PlanStepArtifact }) {
  let icon = <Circle className="h-3 w-3 text-text-tertiary" />;
  let tone = 'text-text-secondary';

  if (step.status === 'completed') {
    icon = <CheckCircle2 className="h-3 w-3 text-success" />;
    tone = 'text-text-primary';
  } else if (step.status === 'in_progress') {
    icon = <Loader2 className="h-3 w-3 animate-spin text-accent" />;
    tone = 'text-text-primary';
  }

  return (
    <li className="flex items-start gap-1.5">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0">
        <div className={`text-xs ${tone}`}>{step.title}</div>
        {step.notes && (
          <div className="mt-0.5 text-[11px] text-text-tertiary">{step.notes}</div>
        )}
      </div>
    </li>
  );
}

function VerificationRow({ check }: { check: VerificationCheckArtifact }) {
  let icon = <Circle className="h-3 w-3 text-text-tertiary" />;
  let tone = 'text-text-secondary';

  if (check.status === 'passed') {
    icon = <CheckCircle2 className="h-3 w-3 text-success" />;
    tone = 'text-text-primary';
  } else if (check.status === 'failed') {
    icon = <XCircle className="h-3 w-3 text-danger" />;
    tone = 'text-text-primary';
  } else if (check.status === 'skipped') {
    icon = <AlertTriangle className="h-3 w-3 text-warning" />;
  }

  return (
    <li className="flex items-start gap-1.5">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0">
        <div className={`text-xs ${tone}`}>{check.name}</div>
        {check.details && (
          <div className="mt-0.5 text-[11px] text-text-tertiary">{check.details}</div>
        )}
      </div>
    </li>
  );
}

function SubtaskRow({ subtask }: { subtask: SubtaskRunArtifact }) {
  const { t } = useTranslation();
  let icon = <Circle className="h-3 w-3 text-text-tertiary" />;
  let tone = 'text-text-secondary';

  if (subtask.status === 'completed') {
    icon = <CheckCircle2 className="h-3 w-3 text-success" />;
    tone = 'text-text-primary';
  } else if (subtask.status === 'running') {
    icon = <Loader2 className="h-3 w-3 animate-spin text-accent" />;
    tone = 'text-text-primary';
  } else if (subtask.status === 'failed') {
    icon = <XCircle className="h-3 w-3 text-danger" />;
    tone = 'text-text-primary';
  }

  return (
    <li className="flex items-start gap-1.5">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0 flex-1">
        <div className={`text-xs ${tone}`}>{subtask.task || subtask.label}</div>
        <div className="mt-0.5 flex flex-wrap items-center gap-1 text-[10px] text-text-tertiary">
          {subtask.role && <span>{subtask.role}</span>}
          <span>{subtaskStatusLabel(subtask.status, t)}</span>
          {subtask.tokenBudget != null && (
            <span>{t('chat.subtasksTokenBudget', { count: subtask.tokenBudget.toLocaleString() })}</span>
          )}
        </div>
        {(subtask.errorMessage || subtask.result) && (
          <div className="mt-0.5 line-clamp-2 text-[11px] text-text-tertiary">
            {subtask.errorMessage || subtask.result}
          </div>
        )}
      </div>
    </li>
  );
}

export function PlanPanel({
  plan,
  compact = false,
}: {
  plan: PlanArtifact;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const [showAll, setShowAll] = useState(false);
  const counts = getPlanCounts(plan);
  const percent = counts.total > 0 ? Math.round((counts.completed / counts.total) * 100) : 0;
  const progressBits = [t('chat.planPercentComplete', { percent: String(percent) })];
  if (counts.inProgress) {
    progressBits.push(t('chat.planInProgressCount', { count: String(counts.inProgress) }));
  }
  if (counts.pending) {
    progressBits.push(t('chat.planPendingCount', { count: String(counts.pending) }));
  }

  return (
    <div className={`rounded-lg border border-border/70 bg-surface-1/70 ${compact ? 'px-2 py-1.5' : 'px-3 py-2.5'}`}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <ClipboardList className="h-3.5 w-3.5 text-accent" />
            <span className="text-[10px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.planLabel')}
            </span>
          </div>
          <div className="mt-0.5 text-xs font-medium text-text-primary">
            {plan.title || t('chat.planDefaultTitle')}
          </div>
          {plan.explanation && (
            <div className="mt-0.5 text-[11px] text-text-secondary">{plan.explanation}</div>
          )}
        </div>
        <div className="shrink-0 rounded-full border border-border/70 bg-surface-0/80 px-2 py-1 text-[11px] tabular-nums text-text-secondary">
          {counts.completed}/{counts.total}
        </div>
      </div>

      <div className="mt-2">
        <div className="h-1.5 rounded-full bg-surface-0">
          <div
            className="h-full rounded-full bg-accent transition-[width] duration-300"
            style={{ width: `${percent}%` }}
          />
        </div>
        <div className="mt-1 text-[10px] text-text-tertiary">
          {progressBits.join(', ')}
        </div>
      </div>

      <ul className="mt-2 max-h-[150px] space-y-1.5 overflow-y-auto">
        {(showAll ? plan.steps : plan.steps.slice(0, 3)).map((step, index) => (
          <PlanStepRow key={step.id || `${step.title}-${index}`} step={step} />
        ))}
      </ul>
      {plan.steps.length > 3 && (
        <button
          type="button"
          className="mt-1 text-[11px] text-accent hover:underline"
          onClick={() => setShowAll(prev => !prev)}
        >
          {showAll ? t('chat.showLess') : t('chat.showAllSteps', { count: String(plan.steps.length) })}
        </button>
      )}
    </div>
  );
}

export function SubtaskPanel({
  subtasks,
  compact = false,
}: {
  subtasks: SubtaskRunArtifact[];
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const [showAll, setShowAll] = useState(false);
  const counts = getSubtaskCounts(subtasks);

  if (subtasks.length === 0) {
    return null;
  }

  return (
    <div className={`rounded-lg border border-border/70 bg-surface-1/70 ${compact ? 'px-2 py-1.5' : 'px-3 py-2.5'}`}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <GitBranch className="h-3.5 w-3.5 text-accent" />
            <span className="text-[10px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.subtasksLabel')}
            </span>
          </div>
          <div className="mt-0.5 text-xs font-medium text-text-primary">
            {t('chat.subtasksDefaultSummary')}
          </div>
        </div>
        <div className="shrink-0 rounded-full border border-border/70 bg-surface-0/80 px-2 py-1 text-[11px] tabular-nums text-text-secondary">
          {counts.completed}/{counts.total}
        </div>
      </div>

      <div className="mt-2 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
        <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
          {t('chat.subtasksCompletedCount', { count: String(counts.completed) })}
        </span>
        {counts.running > 0 && (
          <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
            {t('chat.subtasksRunningCount', { count: String(counts.running) })}
          </span>
        )}
        {counts.queued > 0 && (
          <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
            {t('chat.subtasksQueuedCount', { count: String(counts.queued) })}
          </span>
        )}
        {counts.failed > 0 && (
          <span className="rounded-full border border-danger/30 bg-danger/10 px-2 py-1 text-danger">
            {t('chat.subtasksFailedCount', { count: String(counts.failed) })}
          </span>
        )}
      </div>

      <ul className="mt-2 max-h-[150px] space-y-1.5 overflow-y-auto">
        {(showAll ? subtasks : subtasks.slice(0, 3)).map((subtask, index) => (
          <SubtaskRow key={subtask.id || `${subtask.label}-${index}`} subtask={subtask} />
        ))}
      </ul>
      {subtasks.length > 3 && (
        <button
          type="button"
          className="mt-1 text-[11px] text-accent hover:underline"
          onClick={() => setShowAll(prev => !prev)}
        >
          {showAll ? t('chat.showLess') : t('chat.showAllSubtasks', { count: String(subtasks.length) })}
        </button>
      )}
    </div>
  );
}

export function VerificationPanel({
  verification,
  compact = false,
}: {
  verification: VerificationArtifact;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const [showAll, setShowAll] = useState(false);
  const counts = getVerificationCounts(verification);
  const overall = verification.overallStatus
    ?? (counts.failed > 0 ? 'failed' : counts.passed > 0 && counts.pending > 0 ? 'partial' : counts.passed > 0 ? 'passed' : 'pending');

  const overallTone =
    overall === 'passed'
      ? 'border-success/20 bg-success/10 text-success'
      : overall === 'failed'
        ? 'border-danger/20 bg-danger/10 text-danger'
        : overall === 'partial'
          ? 'border-warning/20 bg-warning/10 text-warning'
          : 'border-border/70 bg-surface-0/80 text-text-secondary';

  const overallLabel =
    overall === 'passed'
      ? t('chat.verificationPassed')
      : overall === 'failed'
        ? t('chat.verificationFailed')
        : overall === 'partial'
          ? t('chat.verificationPartial')
          : t('chat.verificationPending');

  return (
    <div className={`rounded-lg border border-border/70 bg-surface-1/70 ${compact ? 'px-2 py-1.5' : 'px-3 py-2.5'}`}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <ShieldCheck className="h-3.5 w-3.5 text-accent" />
            <span className="text-[10px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.verificationLabel')}
            </span>
          </div>
          <div className="mt-0.5 text-xs font-medium text-text-primary">
            {verification.summary || t('chat.verificationDefaultSummary')}
          </div>
        </div>
        <div className={`shrink-0 rounded-full border px-2 py-1 text-[11px] font-medium ${overallTone}`}>
          {overallLabel}
        </div>
      </div>

      <div className="mt-2 flex flex-wrap gap-1 text-[10px] text-text-tertiary">
        <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
          {t('chat.verificationPassedCount', { count: String(counts.passed) })}
        </span>
        <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
          {t('chat.verificationFailedCount', { count: String(counts.failed) })}
        </span>
        <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
          {t('chat.verificationPendingCount', { count: String(counts.pending) })}
        </span>
        {counts.skipped > 0 && (
          <span className="rounded-full border border-border/70 bg-surface-0/80 px-2 py-1">
            {t('chat.verificationSkippedCount', { count: String(counts.skipped) })}
          </span>
        )}
      </div>

      <ul className="mt-2 max-h-[150px] space-y-1.5 overflow-y-auto">
        {(showAll ? verification.checks : verification.checks.slice(0, 3)).map((check, index) => (
          <VerificationRow key={`${check.name}-${index}`} check={check} />
        ))}
      </ul>
      {verification.checks.length > 3 && (
        <button
          type="button"
          className="mt-1 text-[11px] text-accent hover:underline"
          onClick={() => setShowAll(prev => !prev)}
        >
          {showAll ? t('chat.showLess') : t('chat.showAllChecks', { count: String(verification.checks.length) })}
        </button>
      )}
    </div>
  );
}
