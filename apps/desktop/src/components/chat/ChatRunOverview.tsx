import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import { Target } from 'lucide-react';
import { useTranslation, type TranslationKey } from '../../i18n';
import { ProviderIcon } from '../../lib/providerIcons';
import type { ActiveGoalContext } from '../../lib/goalContext';

interface ContextUsageSegment {
  kind: string;
  tokens: number;
}

interface ContextUsageBreakdown {
  totalTokens: number;
  segments: ContextUsageSegment[];
}

interface TokenUsage {
  promptTokens: number;
  aggregatePromptTokens?: number;
  totalTokens: number;
  contextWindow: number;
  completionTokens: number;
  thinkingTokens: number;
  cacheReadTokens?: number;
  cacheMissTokens?: number;
  cacheCreationTokens?: number;
  cacheAveragePromptTokens?: number;
  cacheAverageReadTokens?: number;
  cacheAverageMissTokens?: number;
  cacheAverageCreationTokens?: number;
  cacheAverageSampleCount?: number;
  contextBreakdown?: ContextUsageBreakdown;
  isEstimated: boolean;
  source: 'live' | 'cached' | 'estimated';
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

interface CacheUsageStats {
  readTokens: number;
  missTokens: number;
  creationTokens: number;
  hitPercent: number | null;
  sampleCount: number;
}

interface ChatRunOverviewProps {
  isStreaming: boolean;
  tokenUsage?: TokenUsage | null;
  runtimeProfile?: RuntimeProfile | null;
  finishReason?: string | null;
  contextOverflow?: boolean;
  isCompacting?: boolean;
  activeGoalContext?: ActiveGoalContext | null;
}

const SEGMENT_LABEL_KEYS: Record<string, TranslationKey> = {
  prompts: 'chat.contextSegmentPrompts',
  conversation: 'chat.contextSegmentConversation',
  tools: 'chat.contextSegmentTools',
  toolResults: 'chat.contextSegmentToolResults',
  mcp: 'chat.contextSegmentMcp',
  memory: 'chat.contextSegmentMemory',
  sources: 'chat.contextSegmentSources',
  skills: 'chat.contextSegmentSkills',
  thinking: 'chat.contextSegmentThinking',
  estimated: 'chat.contextSegmentEstimated',
  other: 'chat.contextSegmentOther',
};

const CONTEXT_SEGMENT_COLOR: Record<string, string> = {
  prompts: 'bg-sky-400/80',
  conversation: 'bg-indigo-400/80',
  tools: 'bg-amber-400/80',
  toolResults: 'bg-orange-400/80',
  mcp: 'bg-fuchsia-400/80',
  memory: 'bg-emerald-400/80',
  sources: 'bg-teal-400/80',
  skills: 'bg-purple-400/80',
  thinking: 'bg-pink-400/80',
  estimated: 'bg-text-tertiary/65',
  other: 'bg-text-tertiary/55',
};

const SEGMENT_KIND_ALIASES: Record<string, string> = {
  systemcore: 'prompts',
  runtime: 'prompts',
  instructions: 'prompts',
  persona: 'prompts',
  routeplan: 'prompts',
  taskplan: 'prompts',
  scratchpad: 'prompts',
  overhead: 'prompts',
  availableskills: 'skills',
  loadedskills: 'skills',
  usermemory: 'memory',
  projectmemory: 'memory',
  agentmemory: 'memory',
  preferences: 'memory',
  learnedsuccesses: 'memory',
  sourcescope: 'sources',
  collectioncontext: 'sources',
  conversationsummary: 'conversation',
  toolcalls: 'tools',
  toolresults: 'toolResults',
};

const RING_RADIUS = 12;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;
const DRAG_THRESHOLD_PX = 4;
const SNAP_BACK_DURATION_MS = 320;

interface HudDragState {
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

function currentTranslation(element: HTMLElement): { x: number; y: number } {
  const transform = window.getComputedStyle(element).transform;
  if (!transform || transform === 'none') return { x: 0, y: 0 };

  try {
    const matrix = new DOMMatrixReadOnly(transform);
    return { x: matrix.m41, y: matrix.m42 };
  } catch {
    return { x: 0, y: 0 };
  }
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

function safeTokenCount(value: number | undefined): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 0;
  return Math.max(0, Math.round(value));
}

function cacheUsageStats(usage: TokenUsage | null): CacheUsageStats | null {
  if (!usage || usage.isEstimated) return null;

  const sampleCount = safeTokenCount(usage.cacheAverageSampleCount);
  const useAverage = sampleCount > 0;
  const readTokens = safeTokenCount(
    useAverage ? usage.cacheAverageReadTokens : usage.cacheReadTokens,
  );
  const promptTokens = safeTokenCount(
    useAverage ? usage.cacheAveragePromptTokens : usage.promptTokens,
  );
  let missTokens = safeTokenCount(
    useAverage ? usage.cacheAverageMissTokens : usage.cacheMissTokens,
  );
  if (missTokens <= 0 && readTokens > 0 && promptTokens > readTokens) {
    missTokens = promptTokens - readTokens;
  }
  const creationTokens = safeTokenCount(
    useAverage ? usage.cacheAverageCreationTokens : usage.cacheCreationTokens,
  );
  if (readTokens + missTokens + creationTokens <= 0) return null;

  const denominator = missTokens > 0
    ? readTokens + missTokens
    : promptTokens >= readTokens
      ? promptTokens
      : readTokens + creationTokens;
  const hitPercent = denominator > 0
    ? Math.max(0, Math.min(100, (readTokens / denominator) * 100))
    : null;

  return {
    readTokens,
    missTokens,
    creationTokens,
    hitPercent,
    sampleCount,
  };
}

function segmentKey(kind: string): string {
  const trimmed = kind.trim();
  if (trimmed in SEGMENT_LABEL_KEYS) return trimmed;
  const normalized = trimmed.replace(/[\s_-]+/g, '').toLowerCase();
  const aliased = SEGMENT_KIND_ALIASES[normalized];
  if (aliased && aliased in SEGMENT_LABEL_KEYS) return aliased;
  return 'other';
}

function segmentLabel(kind: string, t: ReturnType<typeof useTranslation>['t']): string {
  return t(SEGMENT_LABEL_KEYS[kind] ?? SEGMENT_LABEL_KEYS.other);
}

function contextSegments(usage: TokenUsage): ContextUsageSegment[] {
  const breakdown = usage.contextBreakdown;
  if (breakdown?.segments?.length) {
    const totals = new Map<string, number>();
    for (const segment of breakdown.segments) {
      const tokens = safeTokenCount(segment.tokens);
      if (tokens <= 0) continue;
      const key = segmentKey(segment.kind);
      totals.set(key, (totals.get(key) ?? 0) + tokens);
    }
    return Array.from(totals.entries()).map(([kind, tokens]) => ({ kind, tokens }));
  }
  const promptTokens = safeTokenCount(usage.promptTokens);
  return promptTokens > 0 ? [{ kind: 'estimated', tokens: promptTokens }] : [];
}

export function ChatRunOverview({
  isStreaming,
  tokenUsage,
  runtimeProfile,
  finishReason,
  contextOverflow = false,
  isCompacting = false,
  activeGoalContext = null,
}: ChatRunOverviewProps) {
  const { t } = useTranslation();
  const overviewRef = useRef<HTMLDivElement>(null);
  const pointerInsideRef = useRef(false);
  const suppressHoverRef = useRef(false);
  const suppressClickRef = useRef(false);
  const dragStateRef = useRef<HudDragState | null>(null);
  const snapAnimationRef = useRef<Animation | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);

  const usage = tokenUsage && tokenUsage.contextWindow > 0 ? tokenUsage : null;
  const cacheStats = cacheUsageStats(usage);
  const usagePercent = usage
    ? Math.min(100, Math.max(0, (usage.promptTokens / usage.contextWindow) * 100))
    : 0;
  const usagePercentRounded = Math.round(usagePercent);
  const contextRisk = contextOverflow || usagePercent >= 95
    ? 'danger'
    : usagePercent >= 80
      ? 'warning'
      : 'ok';

  const segments = useMemo(
    () => (detailsOpen && usage ? contextSegments(usage) : []),
    [detailsOpen, usage],
  );
  const totalSegmentTokens = segments.reduce((sum, segment) => sum + segment.tokens, 0);

  const statusLabel = isCompacting
    ? t('chat.compacting')
    : isStreaming
      ? t('chat.thinking')
      : finishReason === 'length'
        ? t('chat.truncated')
        : contextOverflow
          ? t('chat.contextOverflow')
          : usage
            ? usage.source === 'live'
              ? t('chat.contextUsageLive')
              : usage.source === 'cached'
                ? t('chat.contextUsageCached')
                : t('chat.contextUsageEstimated')
            : t('chat.contextNoUsage');

  const usageSourceLabel = usage
    ? usage.source === 'live'
      ? t('chat.contextUsageLive')
      : usage.source === 'cached'
        ? t('chat.contextUsageCached')
        : t('chat.contextUsageEstimated')
    : t('chat.contextNoUsage');

  const statusTone = contextRisk === 'danger'
    ? 'border-danger/30 bg-danger/10 text-text-primary'
    : contextRisk === 'warning'
      ? 'border-warning/35 bg-warning/10 text-text-primary'
      : isStreaming || isCompacting
        ? 'border-accent/30 bg-accent/10 text-text-primary'
        : 'border-border/60 bg-surface-2/70 text-text-secondary';
  const showStatusPill = isStreaming
    || isCompacting
    || contextOverflow
    || finishReason === 'length';
  const ringTone = contextRisk === 'danger'
    ? 'stroke-danger'
    : contextRisk === 'warning'
      ? 'stroke-warning'
      : 'stroke-accent';
  const valueTone = contextRisk === 'danger'
    ? 'text-danger'
    : 'text-text-primary';
  const statusDotTone = contextRisk === 'danger'
    ? 'bg-danger'
    : contextRisk === 'warning'
      ? 'bg-warning'
      : isStreaming || isCompacting
        ? 'bg-accent'
        : 'bg-text-tertiary';

  const modelLabel = runtimeProfile
    ? `${runtimeProfile.provider} / ${runtimeProfile.model}`
    : t('chat.contextNoModel');
  const modelProvider = runtimeProfile?.provider || 'custom';

  const cacheDetailLabel = cacheStats
    ? cacheStats.missTokens > 0
      ? t('chat.cacheReadMissWrite', {
        read: formatTokens(cacheStats.readTokens),
        miss: formatTokens(cacheStats.missTokens),
        write: formatTokens(cacheStats.creationTokens),
      })
      : cacheStats.creationTokens > 0
        ? t('chat.cacheReadWrite', {
          read: formatTokens(cacheStats.readTokens),
          write: formatTokens(cacheStats.creationTokens),
        })
        : t('chat.cacheRead', { read: formatTokens(cacheStats.readTokens) })
    : t('chat.contextHudNoCacheSamples');
  const cacheSampleLabel = cacheStats && cacheStats.sampleCount > 0
    ? t('chat.cacheAverageTurns', { count: String(cacheStats.sampleCount) })
    : cacheDetailLabel;

  const tokenUsageLabel = usage
    ? t('chat.tokenUsage', {
      used: formatTokens(usage.promptTokens),
      total: formatTokens(usage.contextWindow),
    })
    : t('chat.contextNoUsage');
  const percentLabel = usage
    ? t('chat.tokenUsagePercent', { percent: usagePercentRounded })
    : t('chat.contextNoUsage');
  const contextTriggerLabel = usage
    ? `${t('chat.contextBudgetLabel')}: ${tokenUsageLabel} · ${percentLabel} · ${statusLabel}`
    : `${t('chat.contextBudgetLabel')}: ${statusLabel}`;
  const goalTriggerLabel = activeGoalContext
    ? `${t('chat.goalStatusTitle')}: ${activeGoalContext.objective}`
    : null;
  const triggerLabel = goalTriggerLabel
    ? `${goalTriggerLabel}; ${contextTriggerLabel}`
    : contextTriggerLabel;
  const ringDashOffset = RING_CIRCUMFERENCE * (1 - usagePercent / 100);

  const applyDragFrame = useCallback(() => {
    const element = overviewRef.current;
    const dragState = dragStateRef.current;
    if (!element || !dragState) return;
    dragState.frameId = null;
    element.style.transform = `translate3d(${dragState.nextOffsetX}px, ${dragState.nextOffsetY}px, 0)`;
  }, []);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!event.isPrimary || event.button !== 0) return;
    const element = overviewRef.current;
    if (!element) return;

    const translation = currentTranslation(element);
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
    if (!dragState.moved && Math.hypot(deltaX, deltaY) < DRAG_THRESHOLD_PX) return;

    if (!dragState.moved) {
      dragState.moved = true;
      suppressHoverRef.current = true;
      setDetailsOpen(false);
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
    const element = overviewRef.current;
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

    const startTransform = `translate3d(${dragState.nextOffsetX}px, ${dragState.nextOffsetY}px, 0)`;
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reduceMotion || typeof element.animate !== 'function') {
      element.style.transform = '';
      element.style.willChange = '';
      return;
    }

    const animation = element.animate(
      [
        { transform: startTransform },
        { transform: 'translate3d(0, 0, 0)' },
      ],
      {
        duration: SNAP_BACK_DURATION_MS,
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

  useEffect(() => {
    if (!detailsOpen) return undefined;

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      suppressHoverRef.current = pointerInsideRef.current;
      setDetailsOpen(false);

      const activeElement = document.activeElement;
      if (activeElement instanceof HTMLElement && overviewRef.current?.contains(activeElement)) {
        activeElement.blur();
      }
    };

    document.addEventListener('keydown', handleEscape, true);
    return () => document.removeEventListener('keydown', handleEscape, true);
  }, [detailsOpen]);

  useEffect(() => () => {
    const dragState = dragStateRef.current;
    if (dragState?.frameId !== null && dragState?.frameId !== undefined) {
      window.cancelAnimationFrame(dragState.frameId);
    }
    snapAnimationRef.current?.cancel();
  }, []);

  if (!usage && !runtimeProfile && !isStreaming && !isCompacting && !contextOverflow && !activeGoalContext) {
    return null;
  }

  return (
    <div
      ref={overviewRef}
      className="relative z-30 ml-auto flex h-8 shrink-0 items-center"
      data-testid="chat-run-overview"
      onMouseEnter={() => {
        pointerInsideRef.current = true;
        if (!suppressHoverRef.current && !dragStateRef.current) setDetailsOpen(true);
      }}
      onMouseLeave={() => {
        pointerInsideRef.current = false;
        suppressHoverRef.current = false;
        if (!overviewRef.current?.contains(document.activeElement)) setDetailsOpen(false);
      }}
      onFocusCapture={() => {
        suppressHoverRef.current = false;
        setDetailsOpen(true);
      }}
      onBlurCapture={(event) => {
        const nextTarget = event.relatedTarget;
        if (nextTarget instanceof Node && event.currentTarget.contains(nextTarget)) return;
        if (!pointerInsideRef.current || suppressHoverRef.current) setDetailsOpen(false);
      }}
    >
      <button
        type="button"
        className={`relative flex h-8 touch-none select-none items-center rounded-full outline-none transition-colors hover:bg-surface-2/80 focus-visible:bg-surface-2 focus-visible:ring-2 focus-visible:ring-accent/35 active:cursor-grabbing ${
          activeGoalContext
            ? 'max-w-[min(16rem,45vw)] cursor-grab gap-1 border border-accent/25 bg-accent/10 pr-2.5 text-left'
            : 'w-8 cursor-grab justify-center'
        }`}
        aria-label={triggerLabel}
        aria-describedby={detailsOpen ? 'chat-context-details' : undefined}
        aria-expanded={detailsOpen}
        aria-controls={detailsOpen ? 'chat-context-details' : undefined}
        data-testid="chat-context-trigger"
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={finishDrag}
        onClick={(event) => {
          if (suppressClickRef.current) {
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          suppressHoverRef.current = false;
          setDetailsOpen(true);
        }}
      >
        <span className="relative flex h-8 w-8 shrink-0 items-center justify-center">
          <svg className="h-8 w-8 -rotate-90" viewBox="0 0 32 32" aria-hidden="true">
          <circle
            cx="16"
            cy="16"
            r={RING_RADIUS}
            fill="none"
            strokeWidth="2.5"
            className="stroke-surface-3"
          />
          {usage ? (
            <circle
              cx="16"
              cy="16"
              r={RING_RADIUS}
              fill="none"
              strokeWidth="2.5"
              strokeLinecap="round"
              className={`transition-[stroke-dashoffset] duration-500 ease-out motion-reduce:transition-none ${ringTone}`}
              style={{
                strokeDasharray: RING_CIRCUMFERENCE,
                strokeDashoffset: ringDashOffset,
              }}
            />
          ) : (
            <circle
              cx="16"
              cy="16"
              r={RING_RADIUS}
              fill="none"
              strokeWidth="2.5"
              strokeDasharray="2 3"
              className="stroke-text-tertiary/55"
            />
          )}
          </svg>
          <span className={`absolute inset-0 flex items-center justify-center text-[8px] font-semibold tabular-nums ${valueTone}`}>
            {usage ? `${usagePercentRounded}%` : '—'}
          </span>
          {(isStreaming || isCompacting) && (
            <span
              className="absolute right-0 top-0 h-2 w-2 animate-pulse rounded-full border border-surface-1 bg-accent motion-reduce:animate-none"
              aria-hidden="true"
            />
          )}
        </span>
        {activeGoalContext && (
          <span
            className="flex min-w-0 items-center gap-1.5 text-[10px] font-semibold text-text-primary"
            data-testid="chat-context-goal-summary"
          >
            <Target className="h-3.5 w-3.5 shrink-0 text-accent" aria-hidden="true" />
            <span className="shrink-0 text-accent">{t('chat.goalStatusTitle')}</span>
            <span className="truncate">{activeGoalContext.objective}</span>
          </span>
        )}
      </button>

      <div
        className={`absolute bottom-full right-0 w-[min(19rem,calc(100vw-2rem))] pb-2 ${
          detailsOpen ? 'visible pointer-events-auto' : 'invisible pointer-events-none'
        }`}
      >
        {detailsOpen && (
        <div
          id="chat-context-details"
          role="tooltip"
          data-testid="chat-context-details"
          data-state="open"
          className="chat-run-overview-details relative rounded-xl border border-border/75 bg-surface-0 p-3 shadow-[0_12px_32px_rgba(0,0,0,0.24)] ring-1 ring-white/[0.04]"
        >
          <span
            aria-hidden="true"
            className="absolute -bottom-1 right-3 h-2 w-2 rotate-45 border-b border-r border-border/75 bg-surface-0"
          />

          <div className="flex min-w-0 items-center gap-2">
          <ProviderIcon
            provider={modelProvider}
            providerId={modelProvider}
            label={modelLabel}
            size="sm"
            className="border border-border/55 bg-surface-1"
          />
          <div className="min-w-0 flex-1">
            <div className="truncate text-xs font-semibold text-text-primary">{modelLabel}</div>
            <div className="mt-0.5 truncate text-[10px] text-text-tertiary">{usageSourceLabel}</div>
          </div>
          {showStatusPill && (
            <span className={`inline-flex max-w-24 shrink-0 items-center gap-1 rounded-full border px-1.5 py-0.5 text-[9px] font-medium ${statusTone}`}>
              <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${statusDotTone} ${isStreaming || isCompacting ? 'animate-pulse motion-reduce:animate-none' : 'opacity-70'}`} />
              <span className="truncate">{statusLabel}</span>
            </span>
          )}
        </div>

        {activeGoalContext && (
          <div
            className="mt-3 flex min-w-0 items-start gap-2 rounded-lg border border-accent/25 bg-accent/10 px-2.5 py-2"
            data-testid="chat-context-goal"
          >
            <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-accent/15 text-accent">
              <Target className="h-3.5 w-3.5" aria-hidden="true" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-[10px] font-semibold uppercase tracking-[0.1em] text-accent">
                {activeGoalContext.status === 'active'
                  ? t('chat.goalStatusActive')
                  : t('chat.goalStatusTitle')}
              </span>
              <span className="mt-0.5 block line-clamp-2 text-xs leading-4 text-text-primary">
                {activeGoalContext.objective}
              </span>
            </span>
          </div>
        )}

        <div className="mt-3">
          <div className="flex items-end justify-between gap-3">
            <div>
              <div className="text-[10px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
                {t('chat.contextBudgetLabel')}
              </div>
              <div className="mt-0.5 text-xs tabular-nums text-text-secondary">{tokenUsageLabel}</div>
            </div>
            <div className={`text-sm font-semibold tabular-nums ${valueTone}`}>{percentLabel}</div>
          </div>

          <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3/80">
            <div className="flex h-full" style={{ width: `${usagePercent}%` }}>
              {segments.length > 0 ? (
                segments.map((segment, index) => {
                  const width = totalSegmentTokens > 0
                    ? Math.max(3, (segment.tokens / totalSegmentTokens) * 100)
                    : 100;
                  return (
                    <span
                      key={`${segment.kind}-bar-${index}`}
                      className={CONTEXT_SEGMENT_COLOR[segment.kind] ?? CONTEXT_SEGMENT_COLOR.other}
                      style={{ width: `${width}%` }}
                    />
                  );
                })
              ) : (
                <span className="h-full w-full bg-text-tertiary/55" />
              )}
            </div>
          </div>
        </div>

        <div className="mt-3 grid grid-cols-2 gap-2">
          <div className="rounded-lg border border-border/55 bg-surface-1/75 px-2.5 py-2">
            <div className="text-[10px] text-text-tertiary">{t('chat.contextHudPromptUsage')}</div>
            <div className="mt-0.5 flex items-baseline gap-1.5">
              <span className="text-sm font-semibold tabular-nums text-text-primary">
                {usage ? formatTokens(usage.promptTokens) : '—'}
              </span>
              {usage && (
                <span className="text-[10px] tabular-nums text-text-tertiary">{usagePercentRounded}%</span>
              )}
            </div>
          </div>
          <div className="rounded-lg border border-border/55 bg-surface-1/75 px-2.5 py-2">
            <div className="text-[10px] text-text-tertiary">{t('chat.contextHudAverageCache')}</div>
            <div
              className="mt-0.5 text-sm font-semibold tabular-nums text-text-primary"
              data-testid="chat-run-cache-hit"
            >
              {cacheStats?.hitPercent == null ? '—' : `${cacheStats.hitPercent.toFixed(1)}%`}
            </div>
            <div className="mt-0.5 truncate text-[9px] text-text-tertiary" title={cacheDetailLabel}>
              {cacheSampleLabel}
            </div>
          </div>
        </div>

        {segments.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1" data-testid="chat-context-segments">
            {segments.slice(0, 6).map((segment, index) => (
              <span
                key={`${segment.kind}-detail-${index}`}
                className="inline-flex items-center gap-1 rounded-md border border-border/45 bg-surface-1/60 px-1.5 py-0.5 text-[9px] text-text-tertiary"
              >
                <span className={`h-1.5 w-1.5 rounded-full ${CONTEXT_SEGMENT_COLOR[segment.kind] ?? CONTEXT_SEGMENT_COLOR.other}`} />
                <span>
                  {segmentLabel(segment.kind, t)}{' '}
                  <span className="tabular-nums text-text-secondary">{formatTokens(segment.tokens)}</span>
                </span>
              </span>
            ))}
            {segments.length > 6 && (
              <span className="px-1 py-0.5 text-[9px] text-text-tertiary">+{segments.length - 6}</span>
            )}
          </div>
        )}

        {usage && (
          <div className="mt-2 border-t border-border/45 pt-2 text-[9px] tabular-nums text-text-tertiary">
            {t('chat.tokenIO', {
              input: formatTokens(usage.promptTokens),
              output: formatTokens(usage.completionTokens),
            })}
          </div>
        )}
        </div>
        )}
      </div>
    </div>
  );
}
