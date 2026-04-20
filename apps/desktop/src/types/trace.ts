export interface TraceSummary {
  totalSessions: number;
  totalToolCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  avgIterationsPerSession: number;
  avgToolsPerSession: number;
  avgContextUsagePct: number;
  successRate: number;
  cacheHitRate: number;
  topTools: [string, number][];
  sessionsLast7Days: number;
  tokensLast7Days: number;
}

export interface AgentTrace {
  id: string;
  conversationId: string;
  startedAt: string;
  finishedAt: string | null;
  userMessagePreview: string;
  totalIterations: number;
  totalToolCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  peakContextUsagePct: number;
  toolsOffered: number;
  cacheHit: boolean;
  compactionCount: number;
  outcome: string;
  errorMessage: string | null;
  modelId: string;
}
