import { useTranslation, type TranslationKey } from '../../i18n';

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
  totalTokens: number;
  contextWindow: number;
  completionTokens: number;
  thinkingTokens: number;
  contextBreakdown?: ContextUsageBreakdown;
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

const CONTEXT_SEGMENT_COLOR: Record<string, string> = {
  prompts: 'var(--context-prompts)',
  systemCore: 'var(--context-prompts)',
  runtime: '#60a5fa',
  instructions: '#818cf8',
  persona: '#fb7185',
  routePlan: '#facc15',
  taskPlan: '#f97316',
  availableSkills: '#a78bfa',
  loadedSkills: '#8b5cf6',
  userMemory: '#34d399',
  projectMemory: '#22c55e',
  agentMemory: '#84cc16',
  preferences: '#f472b6',
  learnedSuccesses: '#14b8a6',
  scratchpad: '#eab308',
  sourceScope: '#38bdf8',
  collectionContext: '#06b6d4',
  conversationSummary: '#5eead4',
  conversation: 'var(--context-conversation)',
  thinking: '#c084fc',
  toolCalls: '#fb923c',
  toolResults: 'var(--context-tool-results)',
  tools: 'var(--context-tools)',
  mcp: 'var(--context-mcp)',
  overhead: 'var(--context-overhead)',
};

const CONTEXT_SEGMENT_LABEL_KEY: Record<string, TranslationKey> = {
  prompts: 'chat.contextBreakdownPrompts',
  systemCore: 'chat.contextBreakdownSystemCore',
  runtime: 'chat.contextBreakdownRuntime',
  instructions: 'chat.contextBreakdownInstructions',
  persona: 'chat.contextBreakdownPersona',
  routePlan: 'chat.contextBreakdownRoutePlan',
  taskPlan: 'chat.contextBreakdownTaskPlan',
  availableSkills: 'chat.contextBreakdownAvailableSkills',
  loadedSkills: 'chat.contextBreakdownLoadedSkills',
  userMemory: 'chat.contextBreakdownUserMemory',
  projectMemory: 'chat.contextBreakdownProjectMemory',
  agentMemory: 'chat.contextBreakdownAgentMemory',
  preferences: 'chat.contextBreakdownPreferences',
  learnedSuccesses: 'chat.contextBreakdownLearnedSuccesses',
  scratchpad: 'chat.contextBreakdownScratchpad',
  sourceScope: 'chat.contextBreakdownSourceScope',
  collectionContext: 'chat.contextBreakdownCollectionContext',
  conversationSummary: 'chat.contextBreakdownConversationSummary',
  conversation: 'chat.contextBreakdownConversation',
  thinking: 'chat.contextBreakdownThinking',
  toolCalls: 'chat.contextBreakdownToolCalls',
  toolResults: 'chat.contextBreakdownToolResults',
  tools: 'chat.contextBreakdownTools',
  mcp: 'chat.contextBreakdownMcp',
  overhead: 'chat.contextBreakdownOverhead',
};

interface RenderSegment {
  key: string;
  kind: string;
  tokens: number;
  sharePercent: number;
  color: string;
}

function contextSegmentsForBar(usage: TokenUsage | null): RenderSegment[] {
  const breakdown = usage?.contextBreakdown;
  if (!usage || !breakdown || usage.promptTokens <= 0 || !Array.isArray(breakdown.segments)) {
    return [];
  }

  const rawSegments = breakdown.segments
    .map((segment) => ({
      kind: segment.kind,
      tokens: Math.max(0, Math.round(segment.tokens)),
    }))
    .filter((segment) => segment.kind && segment.tokens > 0);
  if (rawSegments.length === 0) return [];

  const rawTotal = rawSegments.reduce((sum, segment) => sum + segment.tokens, 0);
  const basis = Math.max(1, rawTotal);
  return rawSegments.map((segment, index) => {
    const normalizedTokens = Math.max(1, Math.round((segment.tokens / basis) * usage.promptTokens));
    return {
      key: `${segment.kind}-${index}`,
      kind: segment.kind,
      tokens: normalizedTokens,
      sharePercent: Math.max(0, (segment.tokens / basis) * 100),
      color: CONTEXT_SEGMENT_COLOR[segment.kind] ?? 'var(--context-overhead)',
    };
  }).filter((segment) => segment.sharePercent > 0);
}

function contextSegmentLabel(t: ReturnType<typeof useTranslation>['t'], kind: string): string {
  const key = CONTEXT_SEGMENT_LABEL_KEY[kind];
  return key ? t(key) : kind;
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
  const fillGlow = isDanger
    ? 'shadow-[0_0_16px_rgba(248,113,113,0.42)]'
    : isWarning
      ? 'shadow-[0_0_14px_rgba(251,191,36,0.34)]'
      : 'shadow-[0_0_14px_rgba(20,184,166,0.34)]';
  const percentTone = isDanger
    ? 'text-red-300'
    : isWarning
      ? 'text-amber-300'
      : 'text-text-primary';
  const usageLabel = contextWindow > 0
    ? `${formatTokens(usedTokens)} / ${formatTokens(contextWindow)}`
    : t('chat.contextNoUsage');
  const contextSegments = contextSegmentsForBar(usage);

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
          {contextSegments.length > 0 ? (
            <div
              className={`chat-context-hud-fill absolute inset-y-0 left-0 flex overflow-hidden rounded-full ${fillGlow}`}
              style={{ width: fillWidth }}
            >
              {contextSegments.map((segment) => (
                <span
                  key={segment.key}
                  className="chat-context-hud-segment h-full shrink-0"
                  style={{
                    width: `${segment.sharePercent}%`,
                    minWidth: contextSegments.length > 1 ? 2 : undefined,
                    backgroundColor: segment.color,
                  }}
                  title={`${contextSegmentLabel(t, segment.kind)}: ${formatTokens(segment.tokens)} ${t('chat.tokensShort')}`}
                />
              ))}
            </div>
          ) : (
            <div
              className={`chat-context-hud-fill absolute inset-y-0 left-0 rounded-full ${fillTone}`}
              style={{ width: fillWidth }}
            />
          )}
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
      {contextSegments.length > 1 && (
        <div className="hidden h-3.5 items-center gap-x-2.5 gap-y-1 overflow-hidden pl-[7.5rem] pr-[9.5rem] text-[9px] leading-[14px] text-text-tertiary md:flex">
          <span className="shrink-0 font-medium text-text-secondary">
            {t('chat.contextBreakdownLegend')}
          </span>
          <div className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1 overflow-hidden">
            {contextSegments.map((segment) => (
              <span
                key={`legend-${segment.key}`}
                className="inline-flex min-w-0 max-w-[9rem] shrink-0 items-center gap-1.5 tabular-nums"
                title={`${contextSegmentLabel(t, segment.kind)}: ${formatTokens(segment.tokens)} ${t('chat.tokensShort')}`}
              >
                <span
                  className="h-1.5 w-1.5 shrink-0 rounded-[2px] ring-1 ring-border/50"
                  style={{ backgroundColor: segment.color }}
                />
                <span className="truncate">{contextSegmentLabel(t, segment.kind)}</span>
                <span className="text-text-tertiary/80">{formatTokens(segment.tokens)}</span>
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
