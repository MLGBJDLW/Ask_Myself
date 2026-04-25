import { AlertTriangle, BrainCircuit, ChevronDown, Clock3, Cpu, Gauge, Loader2, Plus, Scissors, ShieldCheck, Wrench, Zap } from 'lucide-react';
import { useTranslation } from '../../i18n';

interface TokenUsage {
  promptTokens: number;
  totalTokens: number;
  contextWindow: number;
  completionTokens: number;
  thinkingTokens: number;
  isEstimated: boolean;
  source: 'live' | 'cached' | 'estimated';
}

interface SourceSelectionSummary {
  selectedCount: number;
  totalCount: number;
  loading: boolean;
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

interface ContextCockpitProps {
  sourceSummary: SourceSelectionSummary;
  tokenUsage?: TokenUsage | null;
  runtimeProfile?: RuntimeProfile | null;
  finishReason?: string | null;
  contextOverflow?: boolean;
  rateLimited?: boolean;
  lastCached?: boolean;
  isStreaming?: boolean;
  isCompacting?: boolean;
  onCompact?: () => void;
  onStartNewChat?: () => void;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

export function ContextCockpit({
  sourceSummary,
  tokenUsage,
  runtimeProfile,
  finishReason,
  contextOverflow = false,
  rateLimited = false,
  lastCached = false,
  isStreaming = false,
  isCompacting = false,
  onCompact,
  onStartNewChat,
}: ContextCockpitProps) {
  const { t } = useTranslation();

  const usage = tokenUsage && tokenUsage.contextWindow > 0 ? tokenUsage : null;
  const usagePercent = usage ? Math.min(100, (usage.promptTokens / usage.contextWindow) * 100) : 0;
  const usagePercentRounded = Math.round(usagePercent);
  const scopeSummary = sourceSummary.loading
    ? t('common.loading')
    : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
      ? t('chat.allSources')
      : `${sourceSummary.selectedCount} / ${sourceSummary.totalCount}`;
  const canCompact = Boolean(onCompact);
  const canStartNewChat = Boolean(onStartNewChat);

  const usageSourceLabel = usage
    ? usage.source === 'live'
      ? t('chat.contextUsageLive')
      : usage.source === 'cached'
        ? t('chat.contextUsageCached')
        : t('chat.contextUsageEstimated')
    : t('chat.contextNoUsage');

  let riskTone = 'border-border/70 bg-surface-0/70 text-text-secondary';
  let riskChipTone = 'border-border/70 bg-surface-0/70 text-text-secondary';
  let riskIcon = ShieldCheck;
  let riskTitle = t('chat.contextHealthy');
  let riskAction = usage && usagePercent >= 80 ? t('chat.contextWatch') : '';
  let riskSummaryLabel = t('chat.contextHealthy');

  if (rateLimited) {
    riskTone = 'border-yellow-500/25 bg-yellow-500/10 text-yellow-700';
    riskChipTone = 'border-yellow-500/20 bg-yellow-500/10 text-yellow-700';
    riskIcon = Clock3;
    riskTitle = t('chat.contextRiskRateLimited');
    riskAction = t('chat.contextActionWaitRetry');
    riskSummaryLabel = t('chat.rateLimited');
  } else if (contextOverflow || usagePercent >= 95) {
    riskTone = 'border-red-500/25 bg-red-500/10 text-red-300';
    riskChipTone = 'border-red-500/20 bg-red-500/10 text-red-300';
    riskIcon = AlertTriangle;
    riskTitle = t('chat.contextRiskOverflow');
    riskAction = canCompact && canStartNewChat
      ? t('chat.contextActionCompactOrNew')
      : canCompact
        ? t('chat.contextActionCompactOnly')
        : canStartNewChat
          ? t('chat.contextActionNewChatOnly')
          : t('chat.contextWatch');
    riskSummaryLabel = t('chat.contextOverflow');
  } else if (finishReason === 'length') {
    riskTone = 'border-yellow-500/25 bg-yellow-500/10 text-yellow-700';
    riskChipTone = 'border-yellow-500/20 bg-yellow-500/10 text-yellow-700';
    riskIcon = AlertTriangle;
    riskTitle = t('chat.contextRiskTruncated');
    riskAction = t('chat.contextActionContinue');
    riskSummaryLabel = t('chat.truncated');
  } else if (finishReason === 'contentfilter') {
    riskTone = 'border-red-500/25 bg-red-500/10 text-red-300';
    riskChipTone = 'border-red-500/20 bg-red-500/10 text-red-300';
    riskIcon = AlertTriangle;
    riskTitle = t('chat.contentFiltered');
    riskSummaryLabel = t('chat.contentFiltered');
  } else if (isStreaming) {
    riskSummaryLabel = t('chat.thinking');
  }

  const RiskIcon = riskIcon;
  const showDetailActions = (contextOverflow || usagePercent >= 95) && (canCompact || canStartNewChat);
  const usageSummaryLabel = usage
    ? t('chat.tokenUsagePercent', { percent: usagePercentRounded })
    : t('chat.contextNoUsage');
  const modelLabel = runtimeProfile
    ? `${runtimeProfile.provider} / ${runtimeProfile.model}`
    : 'No model';
  const runtimeContextLabel = runtimeProfile?.contextWindow
    ? formatTokens(runtimeProfile.contextWindow)
    : usage
      ? formatTokens(usage.contextWindow)
      : '';

  return (
    <div className="shrink-0 border-b border-border/60 bg-surface-1/70 px-3 py-2 backdrop-blur">
      <details className="group rounded-xl border border-border/60 bg-surface-0/75">
        <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-sm text-text-secondary [&::-webkit-details-marker]:hidden">
          {(() => {
            const pct = usage && usage.contextWindow > 0
              ? usage.promptTokens / usage.contextWindow
              : 0;
            const colorClass = (pct >= 0.95 || contextOverflow)
              ? 'text-red-500 bg-red-500/10'
              : pct >= 0.8
              ? 'text-amber-400 bg-amber-400/10'
              : usage
              ? 'text-cyan-400 bg-cyan-400/10'
              : 'text-text-tertiary bg-surface-3';

            const chipBase = `inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-md ${colorClass}`;
            const IconComponent = isCompacting ? Loader2 : Gauge;
            const iconClass = `w-3 h-3${isCompacting ? ' animate-spin' : ''}`;
            const chipLabel = isCompacting ? t('chat.compacting') : usageSummaryLabel;

            if (canCompact) {
              return (
                <button
                  type="button"
                  onClick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (!isCompacting) onCompact?.();
                  }}
                  disabled={isCompacting}
                  title={isCompacting ? t('chat.compacting') : t('chat.compactNow')}
                  aria-label={isCompacting ? t('chat.compacting') : t('chat.compactNow')}
                  className={`${chipBase} transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-70`}
                >
                  <IconComponent className={iconClass} />
                  {chipLabel}
                </button>
              );
            }

            return (
              <span className={chipBase}>
                <IconComponent className={iconClass} />
                {chipLabel}
              </span>
            );
          })()}

          {(usage || lastCached) && (
          <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary">
            <Zap className="h-3 w-3 text-text-tertiary" />
            <span className="truncate">{lastCached && !usage ? t('chat.cached') : usageSourceLabel}</span>
          </span>
        )}

          <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary">
            <Cpu className="h-3 w-3 text-text-tertiary" />
            <span className="truncate">{modelLabel}</span>
          </span>

          <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-border/60 bg-surface-1/70 px-2 py-1 text-[11px] text-text-secondary">
            <span className="truncate">{t('chat.knowledgeSources')}: {scopeSummary}</span>
          </span>

          <span className={`inline-flex min-w-0 items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] ${riskChipTone}`}>
            <RiskIcon className="h-3 w-3 shrink-0" />
            <span className="truncate">{riskSummaryLabel}</span>
          </span>

          <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-text-tertiary transition-colors group-hover:text-text-secondary">
            {t('common.expand')}
            <ChevronDown className="h-3.5 w-3.5 shrink-0 transition-transform group-open:rotate-180" />
          </span>
        </summary>

        <div className="border-t border-border/60 px-3 pb-3 pt-2.5">
          <div className="grid gap-2 md:grid-cols-2">
            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Gauge className="h-3.5 w-3.5" />
                {t('chat.contextBudgetLabel')}
              </div>
              {usage ? (
                <>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium tabular-nums text-text-primary">
                      {t('chat.tokenUsagePercent', { percent: usagePercentRounded })}
                    </span>
                    <span className="text-[11px] text-text-tertiary">{usageSourceLabel}</span>
                  </div>
                  <div className="mt-1 text-[11px] tabular-nums text-text-secondary">
                    {t('chat.tokenUsage', {
                      used: formatTokens(usage.promptTokens),
                      total: formatTokens(usage.contextWindow),
                    })}
                  </div>
                </>
              ) : (
                <div className="text-sm text-text-secondary">{usageSourceLabel}</div>
              )}
            </div>

            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Cpu className="h-3.5 w-3.5" />
                Runtime model
              </div>
              <div className="truncate text-sm font-medium text-text-primary" title={modelLabel}>
                {modelLabel}
              </div>
              <div className="mt-1 text-[11px] text-text-secondary">
                {runtimeContextLabel ? `Context ${runtimeContextLabel}` : 'Context window not resolved yet'}
              </div>
            </div>

            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <BrainCircuit className="h-3.5 w-3.5" />
                Reasoning
              </div>
              <div className="text-sm text-text-primary">
                {runtimeProfile?.reasoningEnabled ? 'enabled' : 'disabled'}
              </div>
              <div className="mt-1 truncate text-[11px] text-text-secondary" title={runtimeProfile?.reasoningDetail}>
                {runtimeProfile?.reasoningDetail ?? 'off'}
              </div>
            </div>

            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 text-[11px] font-medium text-text-tertiary">
                {t('chat.contextStatusLabel')}
              </div>
              <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-text-secondary">
                {isStreaming && (
                  <span className="rounded-full bg-surface-2 px-2 py-1">{t('chat.thinking')}</span>
                )}
                {finishReason === 'length' && !isStreaming && (
                  <span className="rounded-full bg-yellow-500/10 px-2 py-1 text-yellow-700">{t('chat.truncated')}</span>
                )}
                {finishReason === 'contentfilter' && !isStreaming && (
                  <span className="rounded-full bg-red-500/10 px-2 py-1 text-red-300">{t('chat.contentFiltered')}</span>
                )}
                {contextOverflow && !isStreaming && (
                  <span className="rounded-full bg-red-500/10 px-2 py-1 text-red-300">{t('chat.contextOverflow')}</span>
                )}
                {rateLimited && !isStreaming && (
                  <span className="rounded-full bg-yellow-500/10 px-2 py-1 text-yellow-700">{t('chat.rateLimited')}</span>
                )}
                {!isStreaming && !finishReason && !contextOverflow && !rateLimited && (
                  <span className="rounded-full bg-surface-2 px-2 py-1">{t('chat.contextHealthy')}</span>
                )}
              </div>
            </div>

            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <ShieldCheck className="h-3.5 w-3.5" />
                {t('chat.knowledgeSources')}
              </div>
              <div className="text-sm text-text-primary">{scopeSummary}</div>
              <div className="mt-1 text-[11px] text-text-secondary">
                {sourceSummary.loading
                  ? t('common.loading')
                  : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
                    ? t('chat.allSources')
                    : t('chat.contextScopeSelected', {
                        selected: sourceSummary.selectedCount,
                        total: sourceSummary.totalCount,
                      })}
              </div>
            </div>

            <div className="rounded-lg border border-border/60 bg-surface-1/60 px-3 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                <Wrench className="h-3.5 w-3.5" />
                Tool policy
              </div>
              <div className="text-sm text-text-primary">
                {runtimeProfile?.sourceAuthority ?? 'KB evidence only'}
              </div>
              <div className="mt-1 text-[11px] text-text-secondary">
                {runtimeProfile?.toolPolicy ?? 'read/search allowed; mutation asks'}
              </div>
            </div>
          </div>

          <div className={`mt-2 rounded-lg border px-3 py-2.5 ${riskTone}`}>
            <div className="flex flex-wrap items-center gap-2">
              <RiskIcon className="h-4 w-4 shrink-0" />
              <span className="text-sm font-medium">{riskTitle}</span>
              {riskAction && <span className="text-sm opacity-90">{riskAction}</span>}
              {showDetailActions && (
                <div className="ml-auto flex flex-wrap gap-1.5">
                  {canCompact && (
                    <button
                      type="button"
                      onClick={onCompact}
                      className="inline-flex items-center gap-1 rounded-md bg-black/10 px-2 py-1 text-[11px] font-medium transition-colors hover:bg-black/15"
                    >
                      <Scissors className="h-3 w-3" />
                      {t('chat.compact')}
                    </button>
                  )}
                  {canStartNewChat && (
                    <button
                      type="button"
                      onClick={() => onStartNewChat?.()}
                      className="inline-flex items-center gap-1 rounded-md bg-black/10 px-2 py-1 text-[11px] font-medium transition-colors hover:bg-black/15"
                    >
                      <Plus className="h-3 w-3" />
                      {t('chat.startNewChat')}
                    </button>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      </details>
    </div>
  );
}
