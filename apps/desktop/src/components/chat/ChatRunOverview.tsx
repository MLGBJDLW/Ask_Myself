import {
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation, type TranslationKey } from '../../i18n';
import { ProviderIcon } from '../../lib/providerIcons';
import type { TurnTiming } from '../../lib/streaming/protocol';
import { formatTimingLatency, useElapsedTime } from '../../lib/useElapsedTime';

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
  contextBreakdown?: ContextUsageBreakdown;
  isEstimated: boolean;
  source: 'live' | 'provider' | 'normalized' | 'estimated';
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
}

interface ChatRunOverviewProps {
  isStreaming: boolean;
  tokenUsage?: TokenUsage | null;
  runtimeProfile?: RuntimeProfile | null;
  finishReason?: string | null;
  contextOverflow?: boolean;
  isCompacting?: boolean;
  turnTiming?: TurnTiming | null;
  taskPhase?: string | null;
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

const TASK_PHASE_KEYS: Record<string, TranslationKey> = {
  queued: 'chat.taskPhaseQueued',
  initializing: 'chat.taskPhaseInitializing',
  routing: 'chat.taskPhaseRouting',
  reasoning: 'chat.taskPhaseReasoning',
  tooling: 'chat.taskPhaseTooling',
  approval: 'chat.taskPhaseApproval',
  generating: 'chat.taskPhaseGenerating',
  finalizing: 'chat.taskPhaseFinalizing',
  cancelling: 'chat.taskPhaseCancelling',
  done: 'chat.taskPhaseDone',
};

const CONTEXT_SEGMENT_COLOR: Record<string, string> = {
  prompts: 'bg-[var(--context-prompts)]',
  conversation: 'bg-[var(--context-conversation)]',
  tools: 'bg-[var(--context-tools)]',
  toolResults: 'bg-[var(--context-tool-results)]',
  mcp: 'bg-[var(--context-mcp)]',
  memory: 'bg-[var(--context-conversation)]',
  sources: 'bg-[var(--context-prompts)]',
  skills: 'bg-[var(--context-tools)]',
  thinking: 'bg-[var(--context-overhead)]',
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

  const readTokens = safeTokenCount(usage.cacheReadTokens);
  const promptTokens = safeTokenCount(usage.promptTokens);
  let missTokens = safeTokenCount(usage.cacheMissTokens);
  if (missTokens <= 0 && readTokens > 0 && promptTokens > readTokens) {
    missTokens = promptTokens - readTokens;
  }
  const creationTokens = safeTokenCount(usage.cacheCreationTokens);
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
  turnTiming,
  taskPhase,
}: ChatRunOverviewProps) {
  const { t } = useTranslation();
  const overviewRef = useRef<HTMLDivElement>(null);
  const pointerInsideRef = useRef(false);
  const suppressHoverRef = useRef(false);
  const suppressHoverTimerRef = useRef<number | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const elapsedLabel = useElapsedTime(turnTiming, isStreaming, 3000);
  const wallLabel = useElapsedTime(turnTiming, isStreaming);
  const ttfeLabel = turnTiming
    ? formatTimingLatency(turnTiming.startedAtEpochMs, turnTiming.firstEventAtEpochMs)
    : null;
  const ttfvLabel = turnTiming
    ? formatTimingLatency(turnTiming.startedAtEpochMs, turnTiming.firstVisibleOutputAtEpochMs)
    : null;
  const phaseKey = taskPhase ? TASK_PHASE_KEYS[taskPhase] : undefined;
  const phaseLabel = phaseKey
    ? t(phaseKey)
    : isCompacting
      ? t('chat.compacting')
      : t('chat.thinking');

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
              : usage.source === 'estimated'
                ? t('chat.contextUsageEstimated')
                : t('chat.contextUsageCached')
            : t('chat.contextNoUsage');

  const usageSourceLabel = usage
    ? usage.source === 'live'
      ? t('chat.contextUsageLive')
      : usage.source === 'estimated'
        ? t('chat.contextUsageEstimated')
        : t('chat.contextUsageCached')
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
  const cacheSampleLabel = cacheDetailLabel;

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
  const triggerLabel = contextTriggerLabel;
  const ringDashOffset = RING_CIRCUMFERENCE * (1 - usagePercent / 100);

  useEffect(() => {
    if (!detailsOpen) return undefined;

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      suppressHoverRef.current = pointerInsideRef.current;
      if (suppressHoverTimerRef.current !== null) {
        window.clearTimeout(suppressHoverTimerRef.current);
      }
      suppressHoverTimerRef.current = window.setTimeout(() => {
        suppressHoverRef.current = false;
        suppressHoverTimerRef.current = null;
      }, 120);
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
    if (suppressHoverTimerRef.current !== null) {
      window.clearTimeout(suppressHoverTimerRef.current);
    }
  }, []);

  if (!usage && !runtimeProfile && !isStreaming && !isCompacting && !contextOverflow && !turnTiming) {
    return null;
  }

  return (
    <div
      ref={overviewRef}
      className="relative z-30 ml-auto flex h-8 shrink-0 items-center"
      data-testid="chat-run-overview"
      onMouseEnter={() => {
        pointerInsideRef.current = true;
        // A fresh pointer entry is an explicit request to reopen, even when an
        // earlier Escape briefly suppressed hover while the pointer remained
        // inside the old popover geometry.
        suppressHoverRef.current = false;
        if (suppressHoverTimerRef.current !== null) {
          window.clearTimeout(suppressHoverTimerRef.current);
          suppressHoverTimerRef.current = null;
        }
        setDetailsOpen(true);
      }}
      onMouseLeave={() => {
        pointerInsideRef.current = false;
        suppressHoverRef.current = false;
        if (suppressHoverTimerRef.current !== null) {
          window.clearTimeout(suppressHoverTimerRef.current);
          suppressHoverTimerRef.current = null;
        }
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
      {elapsedLabel ? (
        <span
          data-testid="chat-turn-elapsed"
          className="mr-1.5 inline-flex h-6 items-center rounded-full border border-accent/25 bg-accent/8 px-2 text-[10px] font-medium tabular-nums text-text-secondary"
        >
          {phaseLabel} · {elapsedLabel}
        </span>
      ) : null}
      {cacheStats?.hitPercent != null ? (
        <span
          data-testid="chat-run-cache-hit-summary"
          className="mr-1.5 inline-flex h-6 max-w-[10rem] items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/80 px-2 text-[10px] text-text-secondary"
          title={`${t('chat.contextHudAverageCache')}: ${cacheStats.hitPercent.toFixed(1)}%`}
        >
          <span className="truncate">{t('chat.contextHudAverageCache')}</span>
          <span className="shrink-0 font-semibold tabular-nums text-text-primary">
            {cacheStats.hitPercent.toFixed(1)}%
          </span>
        </span>
      ) : null}
      <button
        type="button"
        className="relative flex h-8 w-8 cursor-pointer select-none items-center justify-center rounded-full outline-none transition-colors hover:bg-surface-2/80 focus-visible:bg-surface-2 focus-visible:ring-2 focus-visible:ring-accent/35"
        aria-label={triggerLabel}
        aria-describedby={detailsOpen ? 'chat-context-details' : undefined}
        aria-expanded={detailsOpen}
        aria-controls={detailsOpen ? 'chat-context-details' : undefined}
        data-testid="chat-context-trigger"
        onClick={() => {
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
          {turnTiming ? (
            <div
              data-testid="chat-turn-timing-metrics"
              className="col-span-2 rounded-lg border border-border/55 bg-surface-1/75 px-2.5 py-2 text-[10px] tabular-nums text-text-tertiary"
            >
              {t('chat.timeToFirstEvent')} {ttfeLabel ?? '—'} ·{' '}
              {t('chat.timeToFirstVisibleOutput')} {ttfvLabel ?? '—'} ·{' '}
              {t('chat.wallTime')} {wallLabel ?? '—'}
            </div>
          ) : null}
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
