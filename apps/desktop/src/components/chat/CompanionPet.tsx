import { useEffect, useMemo, useState } from 'react';
import { Sparkles } from 'lucide-react';
import * as api from '../../lib/api';
import type { AgentTaskRun } from '../../types/conversation';
import { useTranslation, type TranslationKey } from '../../i18n';

interface CompanionPetProps {
  taskRun?: AgentTaskRun | null;
  onOpenTaskCenter: () => void;
}

const STATE_LABEL_KEYS: Record<api.CompanionState, TranslationKey> = {
  idle: 'chat.companionIdle',
  thinking: 'chat.companionThinking',
  searching: 'chat.companionSearching',
  browsing: 'chat.companionBrowsing',
  readingFiles: 'chat.companionReadingFiles',
  runningTool: 'chat.companionRunningTool',
  coding: 'chat.companionCoding',
  waitingForApproval: 'chat.companionWaitingForApproval',
  waitingForUser: 'chat.companionWaitingForUser',
  reviewing: 'chat.companionReviewing',
  succeeded: 'chat.companionSucceeded',
  failed: 'chat.companionFailed',
  cancelled: 'chat.companionCancelled',
  sleeping: 'chat.companionSleeping',
};

const ACTIVE_STATES = new Set<api.CompanionState>([
  'thinking',
  'searching',
  'browsing',
  'readingFiles',
  'runningTool',
  'coding',
  'reviewing',
]);
const COMPANION_STATES = new Set<api.CompanionState>(
  Object.keys(STATE_LABEL_KEYS) as api.CompanionState[],
);

function fallbackProjection(taskRun?: AgentTaskRun | null): api.CompanionProjection {
  if (!taskRun) {
    return { runId: '', state: 'idle', label: '', sourceEventSeq: null, terminal: false };
  }
  const status = taskRun.status.toLowerCase();
  if (status === 'completed' || status === 'succeeded') {
    return { runId: taskRun.id, state: 'succeeded', label: taskRun.summary || taskRun.title, terminal: true };
  }
  if (status === 'failed' || status === 'timed_out') {
    return { runId: taskRun.id, state: 'failed', label: taskRun.errorMessage || taskRun.title, terminal: true };
  }
  if (status === 'cancelled' || status === 'canceled') {
    return { runId: taskRun.id, state: 'cancelled', label: taskRun.title, terminal: true };
  }
  const phase = taskRun.phase.toLowerCase();
  const state: api.CompanionState = phase === 'approval'
    ? 'waitingForApproval'
    : phase === 'awaiting_user_input'
      ? 'waitingForUser'
      : phase === 'tooling'
        ? 'runningTool'
        : phase === 'accounting'
          ? 'reviewing'
          : 'thinking';
  return { runId: taskRun.id, state, label: taskRun.title, sourceEventSeq: null, terminal: false };
}

function normalizeProjection(
  value: unknown,
  taskRun?: AgentTaskRun | null,
): api.CompanionProjection {
  const fallback = fallbackProjection(taskRun);
  if (!value || typeof value !== 'object') return fallback;
  const candidate = value as Partial<api.CompanionProjection>;
  return {
    runId: typeof candidate.runId === 'string' ? candidate.runId : fallback.runId,
    state: typeof candidate.state === 'string' && COMPANION_STATES.has(candidate.state as api.CompanionState)
      ? candidate.state as api.CompanionState
      : fallback.state,
    label: typeof candidate.label === 'string' ? candidate.label : fallback.label,
    sourceEventSeq: typeof candidate.sourceEventSeq === 'number' ? candidate.sourceEventSeq : null,
    terminal: typeof candidate.terminal === 'boolean' ? candidate.terminal : fallback.terminal,
  };
}

export function CompanionPet({ taskRun, onOpenTaskCenter }: CompanionPetProps) {
  const { t } = useTranslation();
  const [projection, setProjection] = useState<api.CompanionProjection>(() => fallbackProjection(taskRun));

  useEffect(() => {
    let current = true;
    setProjection(fallbackProjection(taskRun));
    if (!taskRun?.id) return () => { current = false; };
    api.getCompanionProjection(taskRun.id)
      .then((next) => { if (current) setProjection(normalizeProjection(next, taskRun)); })
      .catch(() => { /* A pack or projection failure must never affect Agent Runtime. */ });
    return () => { current = false; };
  }, [taskRun?.id, taskRun?.updatedAt, taskRun]);

  const stateLabel = t(STATE_LABEL_KEYS[projection.state]);
  const detailLabel = projection.label.trim() || stateLabel;
  const animationClass = useMemo(() => {
    if (projection.state === 'waitingForApproval' || projection.state === 'waitingForUser') {
      return 'animate-pulse';
    }
    if (ACTIVE_STATES.has(projection.state)) return 'animate-bounce';
    return '';
  }, [projection.state]);
  const toneClass = projection.state === 'failed'
    ? 'border-danger/60 from-danger/35 to-danger/10'
    : projection.state === 'cancelled'
      ? 'border-warning/50 from-warning/30 to-warning/10'
      : projection.state === 'succeeded'
        ? 'border-success/50 from-success/35 to-success/10'
        : projection.state === 'waitingForApproval' || projection.state === 'waitingForUser'
          ? 'border-warning/50 from-warning/25 to-accent/15'
          : 'border-accent/50 from-accent/35 to-info/15';

  return (
    <div className="mx-auto hidden w-full max-w-4xl justify-end px-4 sm:flex" aria-live="polite">
      <button
        type="button"
        onClick={onOpenTaskCenter}
        className="group relative mb-1 rounded-2xl outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
        aria-label={detailLabel === stateLabel ? stateLabel : `${stateLabel}: ${detailLabel}`}
        title={stateLabel}
      >
        <span className="pointer-events-none absolute bottom-full right-0 z-10 mb-2 hidden max-w-64 rounded-lg border border-border bg-surface-2 px-3 py-2 text-left text-[11px] leading-4 text-text-secondary shadow-lg group-hover:block group-focus-visible:block">
          <span className="block font-medium text-text-primary">{stateLabel}</span>
          {detailLabel !== stateLabel && (
            <span className="mt-0.5 block line-clamp-2">{detailLabel}</span>
          )}
        </span>
        <span
          className={`relative flex h-12 w-12 items-center justify-center rounded-[18px] border bg-gradient-to-br shadow-sm transition-transform motion-reduce:animate-none ${toneClass} ${animationClass}`}
          data-companion-state={projection.state}
        >
          <span className="absolute -top-1 left-1.5 h-3 w-3 rotate-45 rounded-sm border-l border-t border-current bg-surface-1/80 text-accent" />
          <span className="absolute -top-1 right-1.5 h-3 w-3 rotate-45 rounded-sm border-l border-t border-current bg-surface-1/80 text-info" />
          <span className="relative h-7 w-8 rounded-[45%] bg-surface-0/85 shadow-inner">
            <span className="absolute left-1.5 top-2 h-1.5 w-1.5 rounded-full bg-text-primary" />
            <span className="absolute right-1.5 top-2 h-1.5 w-1.5 rounded-full bg-text-primary" />
            <span className={`absolute bottom-1.5 left-1/2 h-1 w-2 -translate-x-1/2 rounded-full ${
              projection.state === 'failed' ? 'bg-danger' : 'bg-accent'
            }`} />
          </span>
          {ACTIVE_STATES.has(projection.state) && (
            <Sparkles className="absolute -right-1 -top-1 h-3.5 w-3.5 text-accent motion-reduce:hidden" />
          )}
        </span>
      </button>
    </div>
  );
}
