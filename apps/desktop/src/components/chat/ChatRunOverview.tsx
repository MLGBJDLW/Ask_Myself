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

interface RuntimeProfile {
  contextWindow: number;
}

interface ChatRunOverviewProps {
  isStreaming?: boolean;
  tokenUsage?: TokenUsage | null;
  runtimeProfile?: RuntimeProfile | null;
  finishReason?: string | null;
  contextOverflow?: boolean;
  isCompacting?: boolean;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

export function ChatRunOverview({
  isStreaming = false,
  tokenUsage,
  runtimeProfile,
  finishReason,
  contextOverflow = false,
  isCompacting = false,
}: ChatRunOverviewProps) {
  const { t } = useTranslation();
  const usage = tokenUsage && tokenUsage.contextWindow > 0 ? tokenUsage : null;
  const contextWindow = usage?.contextWindow || runtimeProfile?.contextWindow || 0;
  const usedTokens = usage?.promptTokens ?? 0;
  const usagePercent = contextWindow > 0
    ? Math.min(100, (usedTokens / contextWindow) * 100)
    : 0;
  const usagePercentRounded = Math.round(usagePercent);
  const fillWidth = `${Math.max(usage ? 1 : 0, usagePercent)}%`;
  const isDanger = contextOverflow || usagePercent >= 95 || finishReason === 'contentfilter';
  const isWarning = !isDanger && (usagePercent >= 80 || finishReason === 'length');
  const fillTone = isDanger
    ? 'bg-red-400 shadow-[0_0_16px_rgba(248,113,113,0.42)]'
    : isWarning
      ? 'bg-amber-300 shadow-[0_0_14px_rgba(251,191,36,0.34)]'
      : 'bg-accent shadow-[0_0_14px_rgba(20,184,166,0.34)]';
  const percentTone = isDanger
    ? 'text-red-300'
    : isWarning
      ? 'text-amber-300'
      : 'text-text-primary';
  const usageLabel = contextWindow > 0
    ? `${formatTokens(usedTokens)} / ${formatTokens(contextWindow)}`
    : t('chat.contextNoUsage');

  return (
    <div className="shrink-0 border-b border-border/45 bg-surface-1/70 px-3 py-1 backdrop-blur">
      <div className="flex h-6 items-center gap-2 text-[11px]">
        <div className="hidden w-28 shrink-0 items-center gap-1.5 text-text-tertiary sm:flex">
          <span className={`h-1.5 w-1.5 rounded-full ${isDanger ? 'bg-red-400' : isWarning ? 'bg-amber-300' : 'bg-accent'} ${isStreaming || isCompacting ? 'animate-pulse' : ''}`} />
          <span className="truncate font-medium uppercase tracking-[0.12em]">
            {t('chat.contextBudgetLabel')}
          </span>
        </div>

        <div
          className="chat-context-hud relative h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-3/80 ring-1 ring-border/50"
          data-active={isStreaming || isCompacting || Boolean(usage)}
        >
          <div
            className={`chat-context-hud-fill absolute inset-y-0 left-0 rounded-full ${fillTone}`}
            style={{ width: fillWidth }}
          />
          <span className="absolute inset-y-[-2px] left-[80%] w-px bg-amber-300/45" />
          <span className="absolute inset-y-[-2px] left-[95%] w-px bg-red-300/50" />
        </div>

        <div className="flex w-[9.5rem] shrink-0 items-baseline justify-end gap-2 tabular-nums">
          <span className={`text-xs font-semibold ${percentTone}`}>
            {contextWindow > 0 ? t('chat.tokenUsagePercent', { percent: usagePercentRounded }) : '0%'}
          </span>
          <span className="hidden max-w-[6.5rem] truncate text-[10px] text-text-tertiary md:inline">
            {usageLabel}
          </span>
        </div>
      </div>
    </div>
  );
}
