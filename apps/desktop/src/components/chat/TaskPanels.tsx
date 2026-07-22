import { AlertTriangle, CheckCircle2, ChevronDown, Circle, ClipboardList, GitBranch, Loader2, ShieldCheck, Target, XCircle } from 'lucide-react';
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import { useTranslation } from '../../i18n';
import type {
  PlanArtifact,
  PlanStepArtifact,
  SubtaskRunArtifact,
  VerificationArtifact,
  VerificationCheckArtifact,
} from '../../lib/taskArtifacts';
import type { ActiveGoalContext } from '../../lib/goalContext';

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

function getCurrentPlanStep(plan: PlanArtifact): PlanStepArtifact | null {
  return (
    plan.steps.find(step => step.status === 'in_progress')
    ?? plan.steps.find(step => step.status === 'pending')
    ?? plan.steps[plan.steps.length - 1]
    ?? null
  );
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

function PlanProgressStepRow({ step }: { step: PlanStepArtifact }) {
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
    <li className="grid grid-cols-[14px_minmax(0,1fr)] items-start gap-1.5">
      <span className="mt-1 shrink-0">{icon}</span>
      <span className={`min-w-0 break-words text-xs leading-5 ${tone}`}>
        {step.title}
      </span>
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

const PLAN_DRAG_THRESHOLD_PX = 4;
const PLAN_SNAP_BACK_DURATION_MS = 320;

interface PlanDragState {
  pointerId: number;
  startClientX: number;
  startClientY: number;
  startOffsetX: number;
  startOffsetY: number;
  baseLeft: number;
  baseTop: number;
  width: number;
  height: number;
  nextOffsetX: number;
  nextOffsetY: number;
  moved: boolean;
  frameId: number | null;
}

function currentPlanTranslation(element: HTMLElement): { x: number; y: number } {
  const transform = window.getComputedStyle(element).transform;
  if (!transform || transform === 'none') return { x: 0, y: 0 };

  try {
    const matrix = new DOMMatrixReadOnly(transform);
    return { x: matrix.m41, y: matrix.m42 };
  } catch {
    return { x: 0, y: 0 };
  }
}

export function PlanProgressPanel({
  plan,
  goal = null,
  subtasks = [],
}: {
  plan?: PlanArtifact | null;
  goal?: ActiveGoalContext | null;
  subtasks?: SubtaskRunArtifact[];
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const dragStateRef = useRef<PlanDragState | null>(null);
  const snapAnimationRef = useRef<Animation | null>(null);
  const suppressClickRef = useRef(false);
  const counts = plan
    ? getPlanCounts(plan)
    : { total: 0, completed: 0, inProgress: 0, pending: 0 };
  const current = plan ? getCurrentPlanStep(plan) : null;
  const percent = counts.total > 0 ? Math.round((counts.completed / counts.total) * 100) : 0;
  const subtaskCounts = getSubtaskCounts(subtasks);
  let currentIcon = goal
    ? <Target className="h-3 w-3 text-accent" />
    : <Circle className="h-3 w-3 text-text-tertiary" />;
  if (goal?.status === 'blocked') {
    currentIcon = <AlertTriangle className="h-3 w-3 text-warning" />;
  } else if (goal?.status === 'active') {
    currentIcon = <Loader2 className="h-3 w-3 animate-spin text-accent motion-reduce:animate-none" />;
  } else if (current?.status === 'completed') {
    currentIcon = <CheckCircle2 className="h-3 w-3 text-success" />;
  } else if (current?.status === 'in_progress') {
    currentIcon = <Loader2 className="h-3 w-3 animate-spin text-accent" />;
  }

  const applyDragFrame = useCallback(() => {
    const element = panelRef.current;
    const dragState = dragStateRef.current;
    if (!element || !dragState) return;
    dragState.frameId = null;
    element.style.transform = `translate3d(${dragState.nextOffsetX}px, ${dragState.nextOffsetY}px, 0)`;
  }, []);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!event.isPrimary || event.button !== 0) return;
    const element = panelRef.current;
    if (!element) return;

    const translation = currentPlanTranslation(element);
    snapAnimationRef.current?.cancel();
    snapAnimationRef.current = null;
    element.style.transform = `translate3d(${translation.x}px, ${translation.y}px, 0)`;
    element.style.willChange = 'transform';
    element.dataset.dragging = 'true';

    const rect = element.getBoundingClientRect();
    dragStateRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startOffsetX: translation.x,
      startOffsetY: translation.y,
      baseLeft: rect.left - translation.x,
      baseTop: rect.top - translation.y,
      width: rect.width,
      height: rect.height,
      nextOffsetX: translation.x,
      nextOffsetY: translation.y,
      moved: false,
      frameId: null,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }, []);

  const handlePointerMove = useCallback((event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    const deltaX = event.clientX - dragState.startClientX;
    const deltaY = event.clientY - dragState.startClientY;
    if (!dragState.moved && Math.hypot(deltaX, deltaY) < PLAN_DRAG_THRESHOLD_PX) return;

    if (!dragState.moved) {
      dragState.moved = true;
      setOpen(false);
    }

    event.preventDefault();
    const viewportPadding = 8;
    const minX = viewportPadding - dragState.baseLeft;
    const maxX = window.innerWidth - viewportPadding - dragState.baseLeft - dragState.width;
    const minY = viewportPadding - dragState.baseTop;
    const maxY = window.innerHeight - viewportPadding - dragState.baseTop - dragState.height;
    dragState.nextOffsetX = Math.min(maxX, Math.max(minX, dragState.startOffsetX + deltaX));
    dragState.nextOffsetY = Math.min(maxY, Math.max(minY, dragState.startOffsetY + deltaY));

    if (dragState.frameId === null) {
      dragState.frameId = window.requestAnimationFrame(applyDragFrame);
    }
  }, [applyDragFrame]);

  const finishDrag = useCallback((event: ReactPointerEvent<HTMLButtonElement>) => {
    const element = panelRef.current;
    const dragState = dragStateRef.current;
    if (!element || !dragState || dragState.pointerId !== event.pointerId) return;

    if (dragState.frameId !== null) {
      window.cancelAnimationFrame(dragState.frameId);
      dragState.frameId = null;
      applyDragFrame();
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }

    dragStateRef.current = null;
    delete element.dataset.dragging;
    if (!dragState.moved) {
      element.style.transform = '';
      element.style.willChange = '';
      return;
    }

    suppressClickRef.current = true;
    window.setTimeout(() => {
      suppressClickRef.current = false;
    }, 0);

    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reduceMotion || typeof element.animate !== 'function') {
      element.style.transform = '';
      element.style.willChange = '';
      return;
    }

    const animation = element.animate(
      [
        { transform: `translate3d(${dragState.nextOffsetX}px, ${dragState.nextOffsetY}px, 0)` },
        { transform: 'translate3d(0, 0, 0)' },
      ],
      {
        duration: PLAN_SNAP_BACK_DURATION_MS,
        easing: 'cubic-bezier(0.22, 1, 0.36, 1)',
        fill: 'forwards',
      },
    );
    snapAnimationRef.current = animation;
    animation.onfinish = () => {
      if (snapAnimationRef.current !== animation) return;
      snapAnimationRef.current = null;
      element.style.transform = '';
      element.style.willChange = '';
      animation.cancel();
    };
  }, [applyDragFrame]);

  const toggleOpen = useCallback(() => {
    if (suppressClickRef.current) return;
    setOpen((value) => !value);
  }, []);

  useEffect(() => () => {
    const dragState = dragStateRef.current;
    if (dragState?.frameId != null) {
      window.cancelAnimationFrame(dragState.frameId);
    }
    snapAnimationRef.current?.cancel();
  }, []);

  return (
    <div
      ref={panelRef}
      className="pointer-events-auto relative ml-auto h-10 w-full"
      data-open={open}
      data-goal-active={goal ? 'true' : 'false'}
      data-testid="plan-progress-panel"
    >
      <button
        type="button"
        data-testid="task-board-collapsed"
        className="chat-plan-capsule-collapsed ml-auto flex max-w-full touch-none select-none items-center gap-1.5 rounded-full border border-border/70 bg-surface-1 px-3 py-2 text-left shadow-lg shadow-black/10 cursor-grab hover:bg-surface-2 active:cursor-grabbing"
        aria-expanded={open}
        aria-hidden={open}
        tabIndex={open ? -1 : 0}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={finishDrag}
        onClick={toggleOpen}
      >
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-accent/20 bg-accent/10">
          {currentIcon}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.12em] text-accent">
              {goal ? t('chat.goalStatusTitle') : t('chat.planLabel')}
            </span>
            {(goal?.objective || current?.title) && (
              <span className="max-w-44 truncate text-xs font-medium text-text-primary">
                {goal?.objective ?? current?.title}
              </span>
            )}
          </div>
        </div>
        {plan && (
          <span
            data-testid="task-board-progress"
            className="shrink-0 text-[11px] tabular-nums text-text-tertiary"
          >
            {counts.completed}/{counts.total}
          </span>
        )}
        {subtaskCounts.running > 0 && (
          <span
            className="inline-flex shrink-0 items-center gap-1 rounded-full bg-accent/10 px-1.5 py-0.5 text-[10px] text-accent"
            title={t('chat.subtasksRunningCount', { count: String(subtaskCounts.running) })}
          >
            <GitBranch className="h-2.5 w-2.5" />
            {subtaskCounts.running}
          </span>
        )}
        <ChevronDown className={`h-3.5 w-3.5 shrink-0 text-text-tertiary transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      <div
        data-testid="task-board-expanded"
        className="chat-plan-capsule-expanded absolute right-0 top-0 w-full rounded-2xl border border-border/70 bg-surface-1 px-2.5 py-2 shadow-lg shadow-black/10"
        aria-hidden={!open}
      >
        <button
          type="button"
          className="flex w-full touch-none select-none items-center gap-2 rounded-xl px-1 py-1 text-left transition-colors cursor-grab hover:bg-surface-2/70 active:cursor-grabbing"
          aria-expanded={open}
          tabIndex={open ? 0 : -1}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={finishDrag}
          onPointerCancel={finishDrag}
          onClick={toggleOpen}
        >
          <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-accent/20 bg-accent/10">
            {currentIcon}
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.12em] text-accent">
                {goal ? t('chat.goalStatusTitle') : t('chat.planLabel')}
              </span>
            </div>
            <div className="mt-0.5 truncate text-xs font-semibold text-text-primary">
              {goal?.objective ?? current?.title ?? plan?.title ?? ''}
            </div>
            {current?.notes && (
              <div className="mt-0.5 truncate text-[11px] text-text-tertiary">{current.notes}</div>
            )}
          </div>
          {plan && (
            <span className="shrink-0 text-[11px] tabular-nums text-text-tertiary">
              {counts.completed}/{counts.total}
            </span>
          )}
          <ChevronDown className="h-3.5 w-3.5 shrink-0 rotate-180 text-text-tertiary transition-transform" />
        </button>

        {plan && (
          <>
            <div className="mx-1 mt-1 h-1 rounded-full bg-surface-0">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-300"
                style={{ width: `${percent}%` }}
              />
            </div>
            <ol className="mt-2 max-h-40 space-y-1 overflow-y-auto px-1 pr-1.5">
              {plan.steps.map((step, index) => (
                <PlanProgressStepRow key={step.id || `${step.title}-${index}`} step={step} />
              ))}
            </ol>
          </>
        )}
        {subtasks.length > 0 && (
          <section
            data-testid="plan-subagent-status"
            className="mx-1 mt-2 border-t border-border/50 pt-2"
          >
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-1.5">
                <GitBranch className="h-3 w-3 shrink-0 text-accent" />
                <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-text-tertiary">
                  {t('chat.subtasksLabel')}
                </span>
                {(subtaskCounts.running > 0 || subtaskCounts.queued > 0) && (
                  <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-accent motion-reduce:animate-none" />
                )}
              </div>
              <span className="text-[10px] tabular-nums text-text-tertiary">
                {subtaskCounts.completed}/{subtaskCounts.total}
              </span>
            </div>
            <ul className="mt-1.5 max-h-32 space-y-1.5 overflow-y-auto pr-1">
              {subtasks.map((subtask, index) => (
                <SubtaskRow key={subtask.id || `${subtask.label}-${index}`} subtask={subtask} />
              ))}
            </ul>
          </section>
        )}
      </div>
    </div>
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
