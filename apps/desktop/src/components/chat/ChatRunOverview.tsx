import { useMemo } from 'react';
import { useTranslation, type TranslationKey } from '../../i18n';
import { ProviderIcon } from '../../lib/providerIcons';

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
  prompts: 'bg-sky-400/70',
  conversation: 'bg-indigo-400/70',
  tools: 'bg-amber-400/70',
  toolResults: 'bg-orange-400/70',
  mcp: 'bg-fuchsia-400/70',
  memory: 'bg-emerald-400/70',
  sources: 'bg-teal-400/70',
  skills: 'bg-purple-400/70',
  thinking: 'bg-pink-400/70',
  estimated: 'bg-text-tertiary/60',
  other: 'bg-text-tertiary/50',
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
  const missTokens = safeTokenCount(
    useAverage ? usage.cacheAverageMissTokens : usage.cacheMissTokens,
  );
  const creationTokens = safeTokenCount(
    useAverage ? usage.cacheAverageCreationTokens : usage.cacheCreationTokens,
  );
  if (readTokens + missTokens + creationTokens <= 0) return null;

  const promptTokens = safeTokenCount(
    useAverage ? usage.cacheAveragePromptTokens : usage.promptTokens,
  );
  const denominator = missTokens > 0
    ? readTokens + missTokens
    : promptTokens >= readTokens
      ? promptTokens
      : readTokens + creationTokens;
  const hitPercent = denominator > 0
    ? Math.max(0, Math.min(100, Math.round((readTokens / denominator) * 100)))
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

function contextSegmentsForBar(usage: TokenUsage): ContextUsageSegment[] {
  const breakdown = usage.contextBreakdown;
  if (breakdown?.segments?.length) {
    const totals = new Map<string, number>();
    for (const segment of breakdown.segments) {
      const tokens = Math.max(0, Math.round(segment.tokens));
      if (tokens <= 0) continue;
      const key = segmentKey(segment.kind);
      totals.set(key, (totals.get(key) ?? 0) + tokens);
    }
    return Array.from(totals.entries()).map(([kind, tokens]) => ({ kind, tokens }));
  }
  const promptTokens = Math.max(0, usage.promptTokens);
  return promptTokens > 0 ? [{ kind: 'estimated', tokens: promptTokens }] : [];
}

export function ChatRunOverview({
  isStreaming,
  tokenUsage,
  runtimeProfile,
  finishReason,
  contextOverflow = false,
  isCompacting = false,
}: ChatRunOverviewProps) {
  const { t } = useTranslation();

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

  const segments = useMemo(() => (usage ? contextSegmentsForBar(usage) : []), [usage]);
  const totalSegmentTokens = segments.reduce((sum, segment) => sum + segment.tokens, 0);
  const barFillPercent = usage ? Math.min(100, usagePercent) : 0;

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

  const statusTone = contextRisk === 'danger'
    ? 'border-red-500/30 bg-red-500/10 text-red-300'
    : contextRisk === 'warning'
      ? 'border-amber-400/30 bg-amber-400/10 text-amber-300'
      : isStreaming
        ? 'border-accent/30 bg-accent/10 text-accent'
        : 'border-border/60 bg-surface-0/70 text-text-secondary';

  const usageSourceLabel = usage
    ? usage.source === 'live'
      ? t('chat.contextUsageLive')
      : usage.source === 'cached'
        ? t('chat.contextUsageCached')
        : t('chat.contextUsageEstimated')
    : t('chat.contextNoUsage');

  const modelLabel = runtimeProfile
    ? `${runtimeProfile.provider} / ${runtimeProfile.model}`
    : t('chat.contextNoModel');
  const modelTitle = runtimeProfile?.contextWindow
    ? `${t('chat.contextRuntimeModel')}: ${modelLabel} · ${t('chat.contextWindowValue', {
      value: formatTokens(runtimeProfile.contextWindow),
    })}`
    : `${t('chat.contextRuntimeModel')}: ${modelLabel}`;
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
    : '';
  const cacheTitle = cacheStats
    ? `${t('chat.providerCache')}: ${cacheDetailLabel}${cacheStats.hitPercent == null ? '' : ` (${cacheStats.hitPercent}%)`}${cacheStats.sampleCount > 0 ? ` · ${t('chat.cacheAverageTurns', { count: String(cacheStats.sampleCount) })}` : ''}`
    : '';
  const cacheTone = cacheStats?.readTokens
    ? 'border-accent/35 bg-accent/10 text-accent'
    : cacheStats?.creationTokens
      ? 'border-amber-300/30 bg-amber-300/10 text-amber-300'
      : 'border-border/60 bg-surface-2/60 text-text-tertiary';
  const cacheValueLabel = `${cacheStats?.hitPercent ?? 0}%`;

  if (!usage && !runtimeProfile && !isStreaming) {
    return null;
  }

  return (
    <div
      className="shrink-0 border-b border-border/60 bg-surface-1/90 px-4 py-2 backdrop-blur"
      data-testid="chat-run-overview"
    >
      <div className="mx-auto grid w-full max-w-5xl grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 text-[11px] text-text-tertiary sm:gap-3">
        <span
          className="relative inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-surface-0/75 shadow-[0_1px_0_rgba(255,255,255,0.04)]"
          title={modelTitle}
          aria-label={modelTitle}
          data-testid="chat-run-model-anchor"
        >
          <ProviderIcon
            provider={modelProvider}
            providerId={modelProvider}
            label={modelLabel}
            size="md"
            className="rounded-lg bg-transparent"
          />
          <span
            className={`absolute -right-0.5 -top-0.5 h-2.5 w-2.5 rounded-full border border-surface-0 ${
              isStreaming || isCompacting ? 'animate-pulse bg-accent' : 'bg-text-tertiary'
            }`}
            aria-hidden="true"
          />
        </span>

        <div className="min-w-0">
          {usage ? (
            <>
              <div className="mb-1 hidden items-center justify-between gap-2 sm:flex">
                <div className="flex min-w-0 items-center gap-1.5">
                  <span className="shrink-0 font-medium text-text-secondary">{t('chat.contextBudgetLabel')}</span>
                  <span className="truncate tabular-nums">
                    {t('chat.tokenUsage', {
                      used: formatTokens(usage.promptTokens),
                      total: formatTokens(usage.contextWindow),
                    })}
                  </span>
                </div>
                <div className="shrink-0 text-right tabular-nums">
                  <span className={contextRisk === 'danger'
                    ? 'font-semibold text-red-300'
                    : contextRisk === 'warning'
                      ? 'font-semibold text-amber-300'
                      : 'font-semibold text-text-secondary'}>
                    {t('chat.tokenUsagePercent', { percent: usagePercentRounded })}
                  </span>
                  <span className="hidden pl-2 text-text-tertiary sm:inline">{usageSourceLabel}</span>
                </div>
              </div>
              <div
                className="h-1.5 overflow-hidden rounded-full bg-surface-3/80"
                title={`${t('chat.contextBudgetLabel')}: ${t('chat.tokenUsage', {
                  used: formatTokens(usage.promptTokens),
                  total: formatTokens(usage.contextWindow),
                })} · ${t('chat.tokenUsagePercent', { percent: usagePercentRounded })}`}
              >
                <div className="flex h-full" style={{ width: `${barFillPercent}%` }}>
                  {segments.length > 0 ? (
                    segments.map((segment, index) => {
                      const width = totalSegmentTokens > 0
                        ? Math.max(2, (segment.tokens / totalSegmentTokens) * 100)
                        : 100;
                      return (
                        <div
                          key={`${segment.kind}-${index}`}
                          className={CONTEXT_SEGMENT_COLOR[segment.kind] ?? CONTEXT_SEGMENT_COLOR.other}
                          style={{ width: `${width}%` }}
                          title={`${t(SEGMENT_LABEL_KEYS[segment.kind] ?? SEGMENT_LABEL_KEYS.other)} · ${formatTokens(segment.tokens)}`}
                        />
                      );
                    })
                  ) : (
                    <div className="h-full w-full bg-text-tertiary/60" />
                  )}
                </div>
              </div>
              {segments.length > 1 && (
                <div className="mt-1 hidden flex-wrap gap-x-2 gap-y-1 md:flex">
                  {segments.slice(0, 5).map((segment, index) => (
                    <span key={`${segment.kind}-legend-${index}`} className="inline-flex items-center gap-1">
                      <span className={`h-1.5 w-1.5 rounded-full ${CONTEXT_SEGMENT_COLOR[segment.kind] ?? CONTEXT_SEGMENT_COLOR.other}`} />
                      <span className="truncate">
                        {t(SEGMENT_LABEL_KEYS[segment.kind] ?? SEGMENT_LABEL_KEYS.other)} {formatTokens(segment.tokens)}
                      </span>
                    </span>
                  ))}
                  {segments.length > 5 && <span>+{segments.length - 5}</span>}
                </div>
              )}
            </>
          ) : (
            <div className="h-1.5 overflow-hidden rounded-full bg-surface-3/70" title={statusLabel}>
              <div className={`h-full w-1/3 rounded-full ${isStreaming || isCompacting ? 'animate-pulse bg-accent/70' : 'bg-text-tertiary/50'}`} />
            </div>
          )}
        </div>

        <div className="flex min-w-0 shrink-0 items-center justify-end gap-2">
          {cacheStats && (
            <span
              className={`inline-flex h-7 min-w-12 items-center justify-center rounded-lg border px-2 text-[11px] font-semibold tabular-nums ${cacheTone}`}
              title={cacheTitle}
              aria-label={cacheTitle}
              data-testid="chat-run-cache-hit"
            >
              {cacheValueLabel}
            </span>
          )}
          <span
            className={`inline-flex h-7 w-7 items-center justify-center gap-1.5 rounded-lg border px-0 sm:w-auto sm:max-w-[8rem] sm:px-2 ${statusTone}`}
            title={statusLabel}
            aria-label={statusLabel}
          >
            <span className={`h-1.5 w-1.5 rounded-full ${isStreaming || isCompacting ? 'animate-pulse bg-current' : 'bg-current opacity-70'}`} />
            <span className="hidden truncate text-[11px] font-medium sm:block">{statusLabel}</span>
          </span>
        </div>
      </div>
    </div>
  );
}
