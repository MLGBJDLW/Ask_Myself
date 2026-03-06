import { AlertTriangle, CheckCircle2, Circle, ClipboardList, Loader2, ShieldCheck, XCircle } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { PlanArtifact, PlanStepArtifact, VerificationArtifact, VerificationCheckArtifact } from '../../lib/taskArtifacts';

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

function PlanStepRow({ step }: { step: PlanStepArtifact }) {
  let icon = <Circle className="h-4 w-4 text-text-tertiary" />;
  let tone = 'text-text-secondary';

  if (step.status === 'completed') {
    icon = <CheckCircle2 className="h-4 w-4 text-success" />;
    tone = 'text-text-primary';
  } else if (step.status === 'in_progress') {
    icon = <Loader2 className="h-4 w-4 animate-spin text-accent" />;
    tone = 'text-text-primary';
  }

  return (
    <li className="flex items-start gap-2.5">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0">
        <div className={`text-sm ${tone}`}>{step.title}</div>
        {step.notes && (
          <div className="mt-0.5 text-[12px] text-text-tertiary">{step.notes}</div>
        )}
      </div>
    </li>
  );
}

function VerificationRow({ check }: { check: VerificationCheckArtifact }) {
  let icon = <Circle className="h-4 w-4 text-text-tertiary" />;
  let tone = 'text-text-secondary';

  if (check.status === 'passed') {
    icon = <CheckCircle2 className="h-4 w-4 text-success" />;
    tone = 'text-text-primary';
  } else if (check.status === 'failed') {
    icon = <XCircle className="h-4 w-4 text-danger" />;
    tone = 'text-text-primary';
  } else if (check.status === 'skipped') {
    icon = <AlertTriangle className="h-4 w-4 text-warning" />;
  }

  return (
    <li className="flex items-start gap-2.5">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0">
        <div className={`text-sm ${tone}`}>{check.name}</div>
        {check.details && (
          <div className="mt-0.5 text-[12px] text-text-tertiary">{check.details}</div>
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
    <div className={`rounded-xl border border-border/70 bg-surface-1/70 ${compact ? 'px-3 py-2.5' : 'px-3.5 py-3'}`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <ClipboardList className="h-4 w-4 text-accent" />
            <span className="text-[11px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.planLabel')}
            </span>
          </div>
          <div className="mt-1 text-sm font-medium text-text-primary">
            {plan.title || t('chat.planDefaultTitle')}
          </div>
          {plan.explanation && (
            <div className="mt-1 text-[12px] text-text-secondary">{plan.explanation}</div>
          )}
        </div>
        <div className="shrink-0 rounded-full border border-border/70 bg-surface-0/80 px-2 py-1 text-[11px] tabular-nums text-text-secondary">
          {counts.completed}/{counts.total}
        </div>
      </div>

      <div className="mt-3">
        <div className="h-2 rounded-full bg-surface-0">
          <div
            className="h-full rounded-full bg-accent transition-[width] duration-300"
            style={{ width: `${percent}%` }}
          />
        </div>
        <div className="mt-1.5 text-[11px] text-text-tertiary">
          {progressBits.join(', ')}
        </div>
      </div>

      <ul className="mt-3 space-y-2">
        {plan.steps.map((step, index) => (
          <PlanStepRow key={step.id || `${step.title}-${index}`} step={step} />
        ))}
      </ul>
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
    <div className={`rounded-xl border border-border/70 bg-surface-1/70 ${compact ? 'px-3 py-2.5' : 'px-3.5 py-3'}`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-accent" />
            <span className="text-[11px] font-medium uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.verificationLabel')}
            </span>
          </div>
          <div className="mt-1 text-sm font-medium text-text-primary">
            {verification.summary || t('chat.verificationDefaultSummary')}
          </div>
        </div>
        <div className={`shrink-0 rounded-full border px-2 py-1 text-[11px] font-medium ${overallTone}`}>
          {overallLabel}
        </div>
      </div>

      <div className="mt-3 flex flex-wrap gap-1.5 text-[11px] text-text-tertiary">
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

      <ul className="mt-3 space-y-2">
        {verification.checks.map((check, index) => (
          <VerificationRow key={`${check.name}-${index}`} check={check} />
        ))}
      </ul>
    </div>
  );
}
