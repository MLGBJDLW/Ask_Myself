import { useState, useEffect, useCallback, useRef } from 'react';
import { toast } from 'sonner';
import * as api from './api';
import { isOptimisticSteeringMessage, isSteeringMessage } from './chatMessageGuards';
import { hasPersistedResultAfterLatestUserMessage } from './streaming/chatVisibility';
import type { AgentExecutionMode } from './api';
import { useAgentStream, type ContextUsageBreakdown, type UsageTotal } from './useAgentStream';
import { streamStore } from './streamStore';
import { useTranslation } from '../i18n';
import type {
  AgentConfig,
  AgentTaskRun,
  Conversation,
  ConversationMessage,
  ConversationTurn,
  ArtifactPayload,
  ImageAttachment,
} from '../types/conversation';
import { appTimeMs } from './dateTime';
import { formatUserError } from './userError';

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function generateTitle(message: string): string {
  const trimmed = message.trim();
  if (!trimmed) return '';
  if (trimmed.length <= 50) return trimmed;
  const truncated = trimmed.slice(0, 50);
  const lastSpace = truncated.lastIndexOf(' ');
  if (lastSpace > 20) {
    return truncated.slice(0, lastSpace) + '...';
  }
  return truncated + '...';
}

const USAGE_CACHE_KEY = 'chat-token-usage-v1';

interface StoredUsageEntry {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  thinkingTokens: number;
  cacheReadTokens?: number;
  cacheMissTokens?: number;
  cacheCreationTokens?: number;
  cacheSampleCount?: number;
  cachePromptTotal?: number;
  cacheReadTotal?: number;
  cacheMissTotal?: number;
  cacheCreationTotal?: number;
  lastPromptTokens: number;
  contextBreakdown?: ContextUsageBreakdown;
  updatedAt: number;
}

function sanitizeNumber(input: unknown, fallback = 0): number {
  if (typeof input !== 'number' || !Number.isFinite(input)) return fallback;
  return Math.max(0, Math.round(input));
}

function averageTokens(total: number | undefined, sampleCount: number): number | undefined {
  if (!Number.isFinite(total) || sampleCount <= 0) return undefined;
  return Math.max(0, Math.round((total ?? 0) / sampleCount));
}

function sanitizeContextBreakdown(input: unknown): ContextUsageBreakdown | undefined {
  if (!input || typeof input !== 'object') return undefined;
  const record = input as Record<string, unknown>;
  if (!Array.isArray(record.segments)) return undefined;
  const segments = record.segments
    .filter((segment): segment is Record<string, unknown> => Boolean(segment) && typeof segment === 'object')
    .map((segment) => ({
      kind: typeof segment.kind === 'string' ? segment.kind : '',
      tokens: sanitizeNumber(segment.tokens),
    }))
    .filter((segment) => segment.kind.length > 0 && segment.tokens > 0);
  if (segments.length === 0) return undefined;
  const segmentTotal = segments.reduce((sum, segment) => sum + segment.tokens, 0);
  return {
    totalTokens: sanitizeNumber(record.totalTokens, segmentTotal),
    segments,
  };
}

function buildFallbackContextBreakdown(
  messages: ConversationMessage[],
  promptTokens: number,
): ContextUsageBreakdown | undefined {
  const totalTokens = sanitizeNumber(promptTokens);
  if (totalTokens <= 0) return undefined;

  const tokenSum = (roles: ConversationMessage['role'][]) => messages.reduce((sum, message) => {
    if (!roles.includes(message.role)) return sum;
    return sum + sanitizeNumber(message.tokenCount);
  }, 0);
  const conversationTokens = tokenSum(['user', 'assistant']);
  const thinkingTokens = messages.reduce((sum, message) => {
    if (message.role !== 'assistant' || !message.thinking) return sum;
    return sum + Math.max(1, Math.round(message.thinking.length / 4));
  }, 0);
  const toolResultTokens = tokenSum(['tool']);
  const toolCalls = messages.flatMap(message => message.toolCalls ?? []);
  const hasMcpToolCall = toolCalls.some(call => call.name === 'mcp_tool' || call.name.startsWith('mcp__'));
  const hasBuiltinToolCall = toolCalls.some(call => call.name !== 'mcp_tool' && !call.name.startsWith('mcp__'));

  const promptsFloor = Math.max(1, Math.round(totalTokens * 0.18));
  const mcpTokens = hasMcpToolCall ? Math.max(1, Math.round(totalTokens * 0.06)) : 0;
  const toolTokens = hasBuiltinToolCall ? Math.max(1, Math.round(totalTokens * 0.04)) : 0;
  const knownTokens = conversationTokens + thinkingTokens + toolResultTokens + mcpTokens + toolTokens;
  const promptSegmentTokens = Math.max(promptsFloor, totalTokens - knownTokens);
  const rawSegments = [
    { kind: 'prompts', tokens: promptSegmentTokens },
    { kind: 'conversation', tokens: conversationTokens },
    { kind: 'thinking', tokens: thinkingTokens },
    { kind: 'toolResults', tokens: toolResultTokens },
    { kind: 'tools', tokens: toolTokens },
    { kind: 'mcp', tokens: mcpTokens },
  ].filter(segment => segment.tokens > 0);

  const rawTotal = rawSegments.reduce((sum, segment) => sum + segment.tokens, 0);
  if (rawTotal <= 0) return undefined;

  let scaledTotal = 0;
  const segments = rawSegments.map(segment => {
    const tokens = Math.max(1, Math.round((segment.tokens / rawTotal) * totalTokens));
    scaledTotal += tokens;
    return { kind: segment.kind, tokens };
  });
  const largest = segments.reduce((maxIndex, segment, index) => (
    segment.tokens > segments[maxIndex].tokens ? index : maxIndex
  ), 0);
  segments[largest] = {
    ...segments[largest],
    tokens: Math.max(1, segments[largest].tokens + totalTokens - scaledTotal),
  };

  return { totalTokens, segments };
}

/**
 * Merge imageAttachments from the prior in-memory message list onto a fresh
 * backend response. Backend rows that already include imageAttachments win
 * (Tier B persistence). For rows that lack them, we fall back to:
 *   1) the same message id in prior state, or
 *   2) an optimistic `temp-*` user message with matching content (handles the
 *      id swap after the backend assigns a permanent id).
 * This is a safety net — once all historical rows have been persisted via
 * Tier B, this merge becomes a no-op in practice.
 */
function mergeImageAttachments(
  prev: ConversationMessage[],
  next: ConversationMessage[],
): ConversationMessage[] {
  const prevById = new Map(prev.map((m) => [m.id, m]));
  const prevOptimisticWithImages = prev.filter(
    (m) =>
      m.id.startsWith('temp-') &&
      m.role === 'user' &&
      m.imageAttachments &&
      m.imageAttachments.length > 0,
  );
  return next.map((m) => {
    if (m.imageAttachments && m.imageAttachments.length > 0) return m;
    const existing = prevById.get(m.id);
    if (existing?.imageAttachments && existing.imageAttachments.length > 0) {
      return { ...m, imageAttachments: existing.imageAttachments };
    }
    if (m.role === 'user') {
      const opt = prevOptimisticWithImages.find((o) => o.content === m.content);
      if (opt) return { ...m, imageAttachments: opt.imageAttachments };
    }
    return m;
  });
}


function isNoRunningAgentError(error: unknown): boolean {
  const message = String(error ?? '');
  return (
    /no running agent/i.test(message) ||
    /no conversation running/i.test(message) ||
    /no longer accepting steering/i.test(message)
  );
}

function taskRunCanResumeStream(run: AgentTaskRun): boolean {
  return ['queued', 'running', 'waiting_approval', 'cancelling'].includes(run.status);
}

function streamHasVisiblePreview(conversationId: string): boolean {
  const stream = streamStore.getStream(conversationId);
  return Boolean(stream && (
    stream.isStreaming ||
    stream.streamRounds.length > 0 ||
    stream.traceEvents.length > 0 ||
    stream.streamText.length > 0
  ));
}

function insertMessagesByCreatedAt(
  messages: ConversationMessage[],
  inserts: ConversationMessage[],
): ConversationMessage[] {
  const ordered = [...messages];
  for (const insert of inserts) {
    const insertTime = appTimeMs(insert.createdAt);
    const insertAt = ordered.findIndex((message) => appTimeMs(message.createdAt) > insertTime);
    ordered.splice(insertAt === -1 ? ordered.length : insertAt, 0, insert);
  }
  return ordered.map((message, index) => ({ ...message, sortOrder: index }));
}

function optimisticSteeringIsPending(message: ConversationMessage): boolean {
  const artifacts = message.artifacts;
  if (!artifacts || Array.isArray(artifacts) || typeof artifacts !== 'object') {
    return true;
  }
  return (artifacts as Record<string, unknown>).delivery !== 'accepted';
}

function mergeLocalMessageState(
  prev: ConversationMessage[],
  next: ConversationMessage[],
): ConversationMessage[] {
  const merged = mergeImageAttachments(prev, next);
  const nextUserContent = new Set(
    merged.filter((m) => m.role === 'user').map((m) => m.content.trim()),
  );
  const preservedSteering = prev.filter(
    (m) =>
      isOptimisticSteeringMessage(m) &&
      optimisticSteeringIsPending(m) &&
      !nextUserContent.has(m.content.trim()),
  );

  if (preservedSteering.length === 0) {
    return merged;
  }

  return insertMessagesByCreatedAt(merged, preservedSteering);
}

function normalizeUsage(usage: UsageTotal): UsageTotal {
  const promptTokens = sanitizeNumber(usage.promptTokens);
  const completionTokens = sanitizeNumber(usage.completionTokens);
  const totalTokens = sanitizeNumber(usage.totalTokens, promptTokens + completionTokens);
  const thinkingTokens = sanitizeNumber(usage.thinkingTokens ?? 0);
  const cacheReadTokens = sanitizeNumber(usage.cacheReadTokens ?? 0);
  const cacheMissTokens = sanitizeNumber(usage.cacheMissTokens ?? 0);
  const cacheCreationTokens = sanitizeNumber(usage.cacheCreationTokens ?? 0);
  const cacheAverageSampleCount = sanitizeNumber(usage.cacheAverageSampleCount ?? 0);
  const cacheAveragePromptTokens = averageTokens(
    sanitizeNumber(usage.cacheAveragePromptTokens ?? 0),
    cacheAverageSampleCount > 0 ? 1 : 0,
  );
  const cacheAverageReadTokens = averageTokens(
    sanitizeNumber(usage.cacheAverageReadTokens ?? 0),
    cacheAverageSampleCount > 0 ? 1 : 0,
  );
  const cacheAverageMissTokens = averageTokens(
    sanitizeNumber(usage.cacheAverageMissTokens ?? 0),
    cacheAverageSampleCount > 0 ? 1 : 0,
  );
  const cacheAverageCreationTokens = averageTokens(
    sanitizeNumber(usage.cacheAverageCreationTokens ?? 0),
    cacheAverageSampleCount > 0 ? 1 : 0,
  );
  const lastPromptTokens = sanitizeNumber(usage.lastPromptTokens ?? promptTokens, promptTokens);
  const contextBreakdown = sanitizeContextBreakdown(usage.contextBreakdown);
  return {
    promptTokens,
    completionTokens,
    totalTokens,
    thinkingTokens,
    cacheReadTokens,
    cacheMissTokens,
    cacheCreationTokens,
    cacheAveragePromptTokens,
    cacheAverageReadTokens,
    cacheAverageMissTokens,
    cacheAverageCreationTokens,
    cacheAverageSampleCount: cacheAverageSampleCount > 0 ? cacheAverageSampleCount : undefined,
    lastPromptTokens,
    contextBreakdown,
  };
}

function usageAverageFromStoredEntry(entry: StoredUsageEntry): Partial<UsageTotal> {
  const sampleCount = sanitizeNumber(entry.cacheSampleCount ?? 0);
  if (sampleCount <= 0) return {};
  return {
    cacheAveragePromptTokens: averageTokens(entry.cachePromptTotal, sampleCount) ?? 0,
    cacheAverageReadTokens: averageTokens(entry.cacheReadTotal, sampleCount) ?? 0,
    cacheAverageMissTokens: averageTokens(entry.cacheMissTotal, sampleCount) ?? 0,
    cacheAverageCreationTokens: averageTokens(entry.cacheCreationTotal, sampleCount) ?? 0,
    cacheAverageSampleCount: sampleCount,
  };
}

function storedEntryToUsage(entry: StoredUsageEntry): UsageTotal {
  return {
    promptTokens: entry.promptTokens,
    completionTokens: entry.completionTokens,
    totalTokens: entry.totalTokens,
    thinkingTokens: entry.thinkingTokens,
    cacheReadTokens: entry.cacheReadTokens,
    cacheMissTokens: entry.cacheMissTokens,
    cacheCreationTokens: entry.cacheCreationTokens,
    lastPromptTokens: entry.lastPromptTokens,
    contextBreakdown: entry.contextBreakdown,
    ...usageAverageFromStoredEntry(entry),
  };
}

function readUsageCache(): Record<string, StoredUsageEntry> {
  try {
    const raw = localStorage.getItem(USAGE_CACHE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== 'object') return {};
    const entries = Object.entries(parsed as Record<string, unknown>);
    const next: Record<string, StoredUsageEntry> = {};
    for (const [conversationId, value] of entries) {
      if (!conversationId || !value || typeof value !== 'object') continue;
      const row = value as Record<string, unknown>;
      const promptTokens = sanitizeNumber(row.promptTokens);
      const completionTokens = sanitizeNumber(row.completionTokens);
      const totalTokens = sanitizeNumber(row.totalTokens, promptTokens + completionTokens);
      const thinkingTokens = sanitizeNumber(row.thinkingTokens ?? 0);
      const cacheReadTokens = sanitizeNumber(row.cacheReadTokens ?? 0);
      const cacheMissTokens = sanitizeNumber(row.cacheMissTokens ?? 0);
      const cacheCreationTokens = sanitizeNumber(row.cacheCreationTokens ?? 0);
      const cacheSampleCount = sanitizeNumber(row.cacheSampleCount ?? 0);
      const cachePromptTotal = sanitizeNumber(row.cachePromptTotal ?? 0);
      const cacheReadTotal = sanitizeNumber(row.cacheReadTotal ?? 0);
      const cacheMissTotal = sanitizeNumber(row.cacheMissTotal ?? 0);
      const cacheCreationTotal = sanitizeNumber(row.cacheCreationTotal ?? 0);
      const lastPromptTokens = sanitizeNumber(row.lastPromptTokens ?? promptTokens, promptTokens);
      const contextBreakdown = sanitizeContextBreakdown(row.contextBreakdown);
      const updatedAt = sanitizeNumber(row.updatedAt ?? Date.now(), Date.now());
      next[conversationId] = {
        promptTokens,
        completionTokens,
        totalTokens,
        thinkingTokens,
        cacheReadTokens,
        cacheMissTokens,
        cacheCreationTokens,
        cacheSampleCount,
        cachePromptTotal,
        cacheReadTotal,
        cacheMissTotal,
        cacheCreationTotal,
        lastPromptTokens,
        contextBreakdown,
        updatedAt,
      };
    }
    return next;
  } catch {
    return {};
  }
}

function writeUsageCache(cache: Record<string, StoredUsageEntry>) {
  try {
    localStorage.setItem(USAGE_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // ignore localStorage failures
  }
}

async function resolveContextWindowForConfig(config: AgentConfig | null): Promise<number> {
  if (!config) return 0;
  if (config.contextWindow && config.contextWindow > 0) return config.contextWindow;
  return api.getModelContextWindow(config.model).catch(() => 0);
}

function findConfigForConversation(
  configs: AgentConfig[],
  conversation: Conversation,
  fallback: AgentConfig | null,
): AgentConfig | null {
  return (
    configs.find(
      (config) =>
        config.provider === conversation.provider &&
        config.model === conversation.model &&
        config.isDefault,
    ) ??
    configs.find(
      (config) =>
        config.provider === conversation.provider &&
        config.model === conversation.model,
    ) ??
    fallback
  );
}

function buildRuntimeProfile(
  config: AgentConfig | null,
  conversation: Conversation | null,
  contextWindow: number,
  t: ReturnType<typeof useTranslation>['t'],
): RuntimeProfile | null {
  const provider = conversation?.provider ?? config?.provider ?? '';
  const model = conversation?.model ?? config?.model ?? '';
  if (!provider || !model) return null;

  const reasoningEnabled = Boolean(
    config?.reasoningEnabled || config?.thinkingBudget || config?.reasoningEffort,
  );
  const reasoningDetail = !reasoningEnabled
    ? t('chat.contextReasoningOff')
    : config?.reasoningEffort
      ? t('chat.contextReasoningEffort', { effort: config.reasoningEffort })
      : config?.thinkingBudget
        ? t('chat.contextThinkingBudget', { tokens: config.thinkingBudget })
        : t('chat.contextReasoningOn');

  return {
    provider,
    model,
    contextWindow,
    reasoningEnabled,
    reasoningDetail,
    sourceAuthority: t('chat.contextDefaultSourceAuthority'),
    toolPolicy: t('chat.contextDefaultToolPolicy'),
    memoryPolicy: t('chat.contextDefaultMemoryPolicy'),
  };
}

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

export interface UseChatSessionOptions {
  /** Active conversation id (externally controlled, e.g. from URL params) */
  conversationId?: string;
  /** Called when a new conversation is auto-created */
  onConversationCreated?: (id: string) => void;
  /** Optional custom system prompt to use when creating a conversation */
  systemPrompt?: string;
  /** Optional source scope to apply when creating a new conversation */
  initialSourceIds?: string[];
  /**
   * Optional callback returning the *current* source scope at send-time.
   * When provided and non-empty, this takes precedence over `initialSourceIds`
   * during auto-create. Use this to capture live user selections from a
   * SourceSelector that is rendered before the conversation exists.
   */
  getCurrentSourceScope?: () => string[] | null | undefined;
  /** Optional collection context to persist on the conversation */
  initialCollectionContext?: Conversation['collectionContext'];
  /** UI-selected persona to inject for the next agent turn */
  activePersonaId?: string | null;
}

export interface RuntimeProfile {
  provider: string;
  model: string;
  contextWindow: number;
  reasoningEnabled: boolean;
  reasoningDetail: string;
  sourceAuthority: string;
  toolPolicy: string;
  memoryPolicy: string;
}

export interface ChatSendOptions {
  collectionContext?: Conversation['collectionContext'];
  sourceIds?: string[];
  userArtifacts?: ArtifactPayload | null;
  skillIds?: string[];
  executionMode?: AgentExecutionMode;
  taskOrchestratorRunId?: string | null;
}

export interface UseChatSessionReturn {
  messages: ConversationMessage[];
  turns: ConversationTurn[];
  taskRun: AgentTaskRun | null;
  taskEvents: ReturnType<typeof useAgentStream>['taskEvents'];
  conversations: Conversation[];
  setConversations: React.Dispatch<React.SetStateAction<Conversation[]>>;
  isStreaming: boolean;
  streamText: string;
  streamRounds: ReturnType<typeof useAgentStream>['streamRounds'];
  traceEvents: ReturnType<typeof useAgentStream>['traceEvents'];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ReturnType<typeof useAgentStream>['toolCalls'];
  loadingMsgs: boolean;
  loadingConfig: boolean;
  agentConfig: AgentConfig | null;
  contextWindow: number;
  runtimeProfile: RuntimeProfile | null;
  lastUsage: UsageTotal | null;
  tokenUsage: {
    promptTokens: number;
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
  } | null;
  lastCached: boolean;
  finishReason: string | null;
  contextOverflow: boolean;
  rateLimited: boolean;
  send: (
    content: string,
    images?: ImageAttachment[],
    personaOverrideId?: string | null,
    options?: ChatSendOptions,
  ) => Promise<void>;
  stop: () => void;
  deleteConversation: (id: string) => Promise<void>;
  deleteConversationsBatch: (ids: string[]) => Promise<void>;
  deleteAllConversations: () => Promise<void>;
  renameConversation: (id: string, title: string) => Promise<void>;
  setActiveConversation: (id: string) => void;
  createNewConversation: () => void;
  activeId: string | null;
  activeConversation: Conversation | null;
  customSystemPrompt: string;
  setCustomSystemPrompt: (prompt: string) => void;
  error: string | null;
  retry: (messageId?: string) => Promise<void>;
  clearError: () => void;
  loadConversations: () => Promise<void>;
  reloadMessages: (options?: { resetUsage?: boolean }) => Promise<void>;
  deleteMessage: (messageId: string) => void;
  editAndResend: (messageId: string, newContent: string) => Promise<void>;
  switchAgentConfig: (config: AgentConfig) => Promise<void>;
}

/* ------------------------------------------------------------------ */
/*  Hook                                                               */
/* ------------------------------------------------------------------ */

export function useChatSession(options: UseChatSessionOptions = {}): UseChatSessionReturn {
  const {
    conversationId: externalConversationId,
    onConversationCreated,
    systemPrompt: externalSystemPrompt,
    initialSourceIds = [],
    getCurrentSourceScope,
    initialCollectionContext = null,
    activePersonaId = null,
  } = options;

  const { t } = useTranslation();

  // Ref-wrap the callback so it can be read at send-time without being part
  // of the `send` dependency array (which would force consumers to memoize).
  const getCurrentSourceScopeRef = useRef(getCurrentSourceScope);
  useEffect(() => {
    getCurrentSourceScopeRef.current = getCurrentSourceScope;
  }, [getCurrentSourceScope]);

  /* ── State ──────────────────────────────────────────────────────── */
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [messageCache, setMessageCache] = useState<Record<string, ConversationMessage[]>>({});
  const [turnCache, setTurnCache] = useState<Record<string, ConversationTurn[]>>({});
  const [taskRunCache, setTaskRunCache] = useState<Record<string, AgentTaskRun[]>>({});
  const [agentConfig, setAgentConfig] = useState<AgentConfig | null>(null);
  const [customSystemPrompt, setCustomSystemPrompt] = useState<string>(externalSystemPrompt ?? '');
  const [loadingConfig, setLoadingConfig] = useState(true);
  const [loadingConvos, setLoadingConvos] = useState(true);
  const [loadingMsgs, setLoadingMsgs] = useState(false);
  const [defaultContextWindow, setDefaultContextWindow] = useState<number>(0);
  const [contextWindow, setContextWindow] = useState<number>(0);
  const [chatError, setChatError] = useState<string | null>(null);
  const [cachedUsage, setCachedUsage] = useState<UsageTotal | null>(null);

  // Internal conversation id used when the caller does not control routing.
  const [internalConversationId, setInternalConversationId] = useState<string | null>(null);

  // The effective active conversation id
  const activeId = externalConversationId ?? internalConversationId;

  // Track last user message for retry
  const lastUserMessageRef = useRef<{
    content: string;
    attachments?: ImageAttachment[];
    personaId?: string | null;
    options?: ChatSendOptions;
  } | null>(null);
  const usageConversationRef = useRef<string | null>(null);
  const usageCacheRef = useRef<Record<string, StoredUsageEntry>>(readUsageCache());
  const lastUsageRef = useRef<UsageTotal | null>(null);
  const recordedUsageSampleRef = useRef<string | null>(null);
  const pendingStreamConversationRef = useRef<string | null>(null);
  const streamingConversationRef = useRef<string | null>(null);
  const systemPromptCacheRef = useRef<Record<string, string>>({});
  const contextWindowCacheRef = useRef<Record<string, number>>({});
  const agentConfigsRef = useRef<AgentConfig[]>([]);
  const activeAgentConfigRef = useRef<AgentConfig | null>(null);
  const defaultAgentConfigRef = useRef<AgentConfig | null>(null);
  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  const messageCacheRef = useRef(messageCache);
  messageCacheRef.current = messageCache;
  activeAgentConfigRef.current = agentConfig;

  const messages = activeId ? (messageCache[activeId] ?? []) : [];
  const turns = activeId ? (turnCache[activeId] ?? []) : [];
  const taskRuns = activeId ? (taskRunCache[activeId] ?? []) : [];

  const setMessagesForConversation = useCallback((
    conversationId: string,
    updater: ConversationMessage[] | ((prev: ConversationMessage[]) => ConversationMessage[]),
  ) => {
    setMessageCache(prev => {
      const current = prev[conversationId] ?? [];
      const nextMessages = typeof updater === 'function'
        ? (updater as (prev: ConversationMessage[]) => ConversationMessage[])(current)
        : updater;
      return {
        ...prev,
        [conversationId]: nextMessages,
      };
    });
  }, []);

  const setTurnsForConversation = useCallback((
    conversationId: string,
    updater: ConversationTurn[] | ((prev: ConversationTurn[]) => ConversationTurn[]),
  ) => {
    setTurnCache(prev => {
      const current = prev[conversationId] ?? [];
      const nextTurns = typeof updater === 'function'
        ? (updater as (prev: ConversationTurn[]) => ConversationTurn[])(current)
        : updater;
      return {
        ...prev,
        [conversationId]: nextTurns,
      };
    });
  }, []);

  const setTaskRunsForConversation = useCallback((
    conversationId: string,
    updater: AgentTaskRun[] | ((prev: AgentTaskRun[]) => AgentTaskRun[]),
  ) => {
    setTaskRunCache(prev => {
      const current = prev[conversationId] ?? [];
      const nextRuns = typeof updater === 'function'
        ? (updater as (prev: AgentTaskRun[]) => AgentTaskRun[])(current)
        : updater;
      return {
        ...prev,
        [conversationId]: nextRuns,
      };
    });
  }, []);

  const {
    send: streamSend,
    stop: streamStop,
    isStreaming,
    streamText,
    streamRounds,
    traceEvents,
    thinkingText,
    isThinking,
    toolCalls,
    error: streamError,
    lastUsage,
    lastCached,
    finishReason,
    contextOverflow,
    rateLimited,
    autoCompacted,
    taskRun: streamTaskRun,
    taskEvents: streamTaskEvents,
    reset,
  } = useAgentStream(activeId);

  useEffect(() => {
    lastUsageRef.current = lastUsage;
  }, [lastUsage]);

  // Reconnect to in-progress or just-completed stream from global store
  // (runs during render so scoping computed values below see the correct ref)
  if (activeId && !streamingConversationRef.current) {
    const storeStream = streamStore.getStream(activeId);
    if (storeStream && (
      storeStream.isStreaming
      || storeStream.streamRounds.length > 0
      || storeStream.traceEvents.length > 0
      || storeStream.streamText.length > 0
    )) {
      streamingConversationRef.current = activeId;
      usageConversationRef.current = activeId;
    }
  }

  const setUsageCacheForConversation = useCallback((conversationId: string, usage: UsageTotal) => {
    const normalized = normalizeUsage(usage);
    const previous = usageCacheRef.current[conversationId];
    const nextEntry: StoredUsageEntry = {
      promptTokens: normalized.promptTokens,
      completionTokens: normalized.completionTokens,
      totalTokens: normalized.totalTokens,
      thinkingTokens: normalized.thinkingTokens ?? 0,
      cacheReadTokens: normalized.cacheReadTokens ?? 0,
      cacheMissTokens: normalized.cacheMissTokens ?? 0,
      cacheCreationTokens: normalized.cacheCreationTokens ?? 0,
      cacheSampleCount: previous?.cacheSampleCount ?? 0,
      cachePromptTotal: previous?.cachePromptTotal ?? 0,
      cacheReadTotal: previous?.cacheReadTotal ?? 0,
      cacheMissTotal: previous?.cacheMissTotal ?? 0,
      cacheCreationTotal: previous?.cacheCreationTotal ?? 0,
      lastPromptTokens: normalized.lastPromptTokens ?? normalized.promptTokens,
      contextBreakdown: normalized.contextBreakdown,
      updatedAt: Date.now(),
    };
    usageCacheRef.current = {
      ...usageCacheRef.current,
      [conversationId]: nextEntry,
    };
    writeUsageCache(usageCacheRef.current);
    setCachedUsage(storedEntryToUsage(nextEntry));
  }, []);

  const recordUsageCacheSampleForConversation = useCallback((conversationId: string, usage: UsageTotal) => {
    const normalized = normalizeUsage(usage);
    const readTokens = normalized.cacheReadTokens ?? 0;
    const missTokens = normalized.cacheMissTokens ?? 0;
    const creationTokens = normalized.cacheCreationTokens ?? 0;
    if (readTokens + missTokens + creationTokens <= 0) return;

    const promptTokens = normalized.lastPromptTokens ?? normalized.promptTokens;
    const signature = [
      conversationId,
      normalized.promptTokens,
      normalized.completionTokens,
      normalized.totalTokens,
      normalized.thinkingTokens ?? 0,
      promptTokens,
      readTokens,
      missTokens,
      creationTokens,
    ].join(':');
    if (recordedUsageSampleRef.current === signature) return;
    recordedUsageSampleRef.current = signature;

    const previous = usageCacheRef.current[conversationId];
    const nextEntry: StoredUsageEntry = {
      promptTokens: normalized.promptTokens,
      completionTokens: normalized.completionTokens,
      totalTokens: normalized.totalTokens,
      thinkingTokens: normalized.thinkingTokens ?? 0,
      cacheReadTokens: readTokens,
      cacheMissTokens: missTokens,
      cacheCreationTokens: creationTokens,
      cacheSampleCount: (previous?.cacheSampleCount ?? 0) + 1,
      cachePromptTotal: (previous?.cachePromptTotal ?? 0) + promptTokens,
      cacheReadTotal: (previous?.cacheReadTotal ?? 0) + readTokens,
      cacheMissTotal: (previous?.cacheMissTotal ?? 0) + missTokens,
      cacheCreationTotal: (previous?.cacheCreationTotal ?? 0) + creationTokens,
      lastPromptTokens: promptTokens,
      contextBreakdown: normalized.contextBreakdown,
      updatedAt: Date.now(),
    };

    usageCacheRef.current = {
      ...usageCacheRef.current,
      [conversationId]: nextEntry,
    };
    writeUsageCache(usageCacheRef.current);
    if (activeId === conversationId) {
      setCachedUsage(storedEntryToUsage(nextEntry));
    }
  }, [activeId]);

  const deleteUsageCacheForConversations = useCallback((conversationIds: string[]) => {
    if (conversationIds.length === 0) return;
    const next = { ...usageCacheRef.current };
    let changed = false;
    for (const id of conversationIds) {
      if (id in next) {
        delete next[id];
        changed = true;
      }
    }
    if (changed) {
      usageCacheRef.current = next;
      writeUsageCache(next);
    }
  }, []);

  /* ── Load conversations ─────────────────────────────────────────── */
  const loadConversations = useCallback(async () => {
    try {
      const list = await api.listConversations();
      list.sort((a, b) => appTimeMs(b.updatedAt) - appTimeMs(a.updatedAt));
      setConversations(list);
    } catch (e) {
      toast.error(formatUserError(t('chat.loadError'), e));
    } finally {
      setLoadingConvos(false);
    }
  }, [t]);

  /* ── Switch agent config (called from UI model selector) ─────── */
  const switchAgentConfig = useCallback(async (config: AgentConfig) => {
    activeAgentConfigRef.current = config;
    setAgentConfig(config);
    defaultAgentConfigRef.current = config;
    agentConfigsRef.current = agentConfigsRef.current.map((candidate) => ({
      ...candidate,
      isDefault: candidate.id === config.id,
    }));

    await api.setDefaultAgentConfig(config.id);
    let updatedSystemPrompt = customSystemPrompt;
    if (activeId) {
      const updatedConversation = await api.updateConversationModel(activeId, config.provider, config.model);
      updatedSystemPrompt = updatedConversation.systemPrompt ?? '';
      setConversations((prev) =>
        prev.map((conversation) =>
          conversation.id === activeId
            ? { ...conversation, ...updatedConversation }
            : conversation,
        ),
      );
    }
    const cw = await resolveContextWindowForConfig(config);
    setDefaultContextWindow(cw);
    setContextWindow(cw);
    if (activeId) {
      contextWindowCacheRef.current = {
        ...contextWindowCacheRef.current,
        [activeId]: cw,
      };
      systemPromptCacheRef.current = {
        ...systemPromptCacheRef.current,
        [activeId]: updatedSystemPrompt,
      };
    }
  }, [activeId, customSystemPrompt]);

  /* ── Load default agent config ──────────────────────────────────── */
  const loadDefaultConfig = useCallback(async () => {
    try {
      const configs = await api.listAgentConfigs();
      const def = configs.find((c) => c.isDefault) ?? configs[0] ?? null;
      agentConfigsRef.current = configs;
      defaultAgentConfigRef.current = def;
      setAgentConfig(def);
      if (def) {
        const cw = await resolveContextWindowForConfig(def);
        setDefaultContextWindow(cw);
        setContextWindow(cw);
      } else {
        setDefaultContextWindow(0);
        setContextWindow(0);
      }
    } catch {
      agentConfigsRef.current = [];
      defaultAgentConfigRef.current = null;
      setAgentConfig(null);
      setDefaultContextWindow(0);
      setContextWindow(0);
    } finally {
      setLoadingConfig(false);
    }
  }, []);

  useEffect(() => {
    loadConversations();
    loadDefaultConfig();
  }, [loadConversations, loadDefaultConfig]);

  /* ── Load messages when conversation changes ────────────────────── */
  useEffect(() => {
    if (!activeId) {
      setCachedUsage(null);
      setCustomSystemPrompt(externalSystemPrompt ?? '');
      setAgentConfig(defaultAgentConfigRef.current);
      setContextWindow(defaultContextWindow);
      setLoadingMsgs(false);
      return;
    }

    setCustomSystemPrompt(systemPromptCacheRef.current[activeId] ?? '');
    setContextWindow(contextWindowCacheRef.current[activeId] ?? defaultContextWindow);

    const isPendingStreamConversation = pendingStreamConversationRef.current === activeId;
    const isActiveStreamingConversation =
      streamingConversationRef.current === activeId && isStreaming;
    const justFinishedStreaming =
      streamingConversationRef.current === activeId && !isStreaming;
    if (isPendingStreamConversation || isActiveStreamingConversation || justFinishedStreaming) {
      setLoadingMsgs(false);
      return;
    }
    let cancelled = false;
    setLoadingMsgs(true);
    const restored = usageCacheRef.current[activeId];
    setCachedUsage(restored ? storedEntryToUsage(restored) : null);

    void (async () => {
      try {
        const [[conv, msgs], conversationTurns, agentTaskRuns] = await Promise.all([
          api.getConversation(activeId),
          api.getConversationTurns(activeId),
          api.getAgentTaskRuns(activeId),
        ]);
        if (cancelled) return;
        // Safety net (also covers pre-Tier-B persisted rows): preserve any
        // imageAttachments present in prior in-memory state when the backend
        // response lacks them (e.g. optimistic temp-* ids or legacy rows).
        setMessagesForConversation(activeId, (prev) => mergeLocalMessageState(prev, msgs));
        setTurnsForConversation(activeId, conversationTurns);
        setTaskRunsForConversation(activeId, agentTaskRuns);
        const resumableRun = [...agentTaskRuns].reverse().find(taskRunCanResumeStream);
        if (resumableRun && !streamHasVisiblePreview(activeId)) {
          Promise.all([
            api.getAgentTaskRunEvents(resumableRun.id).catch(() => []),
            api.getAgentRunEvents(resumableRun.id).catch(() => []),
          ])
            .then(([taskEvents, runEvents]) => {
              if (cancelled) return;
              streamStore.restoreFromHistoricalEvents(activeId, resumableRun, taskEvents, runEvents);
              const restoredStream = streamStore.getStream(activeId);
              if (restoredStream?.isStreaming) {
                streamingConversationRef.current = activeId;
                usageConversationRef.current = activeId;
              }
            })
            .catch(() => undefined);
        }
        setConversations((prev) => {
          const existing = prev.find((item) => item.id === conv.id);
          if (existing) {
            return prev.map((item) => (item.id === conv.id ? { ...item, ...conv } : item));
          }
          return [conv, ...prev];
        });
        systemPromptCacheRef.current = {
          ...systemPromptCacheRef.current,
          [activeId]: conv.systemPrompt ?? '',
        };
        setCustomSystemPrompt(conv.systemPrompt ?? '');
        const selectedConfig = findConfigForConversation(
          agentConfigsRef.current,
          conv,
          defaultAgentConfigRef.current,
        );
        const cw = await resolveContextWindowForConfig(selectedConfig);
        if (!cancelled) {
          const resolvedContextWindow = cw || defaultContextWindow;
          if (selectedConfig) {
            setAgentConfig(selectedConfig);
          }
          contextWindowCacheRef.current = {
            ...contextWindowCacheRef.current,
            [activeId]: resolvedContextWindow,
          };
          setContextWindow(resolvedContextWindow);
        }
      } catch {
        if (!cancelled) {
          setContextWindow(defaultContextWindow);
        }
      } finally {
        if (!cancelled) setLoadingMsgs(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeId, defaultContextWindow, externalSystemPrompt, isStreaming, setMessagesForConversation, setTaskRunsForConversation, setTurnsForConversation]);

  /* ── Reload messages when streaming completes ───────────────────── */
  useEffect(() => {
    let cancelled = false;
    const completedConversationId = !isStreaming ? streamingConversationRef.current : null;
    if (completedConversationId) {
      const completedUsage = lastUsageRef.current;
      if (completedUsage && usageConversationRef.current === completedConversationId) {
        recordUsageCacheSampleForConversation(completedConversationId, completedUsage);
      }
      // Re-fetch messages after agent is done.
      Promise.all([
        api.getConversation(completedConversationId),
        api.getConversationTurns(completedConversationId),
        api.getAgentTaskRuns(completedConversationId),
      ]).then(([[conv, msgs], conversationTurns, agentTaskRuns]) => {
        if (!cancelled) {
          // Safety net (also covers pre-Tier-B persisted rows): preserve any
          // imageAttachments present in prior in-memory state when the backend
          // response lacks them (e.g. optimistic temp-* ids or legacy rows).
          setMessagesForConversation(completedConversationId, (prev) =>
            mergeLocalMessageState(prev, msgs),
          );
          setTurnsForConversation(completedConversationId, conversationTurns);
          setTaskRunsForConversation(completedConversationId, agentTaskRuns);
          setConversations((prev) => {
            const existing = prev.find((item) => item.id === conv.id);
            if (existing) {
              return prev.map((item) => (item.id === conv.id ? { ...item, ...conv } : item));
            }
            return [conv, ...prev];
          });
          systemPromptCacheRef.current = {
            ...systemPromptCacheRef.current,
            [completedConversationId]: conv.systemPrompt ?? '',
          };
          if (activeId === completedConversationId) {
            setCustomSystemPrompt(conv.systemPrompt ?? '');
          }
          if (msgs.some(msg => msg.role === 'assistant' || msg.role === 'tool')) {
            requestAnimationFrame(() => streamStore.clearPreview(completedConversationId));
          }
        }
      }).catch((e) => {
        console.error('Failed to refresh messages after streaming:', e);
      });
      // Also refresh conversation list (updatedAt changes)
      loadConversations();

      // Auto-title: request LLM-generated title. Show a truncated placeholder
      // in local React state for responsiveness, but do NOT persist it — the
      // DB stays empty until generateTitle() returns, and `loadConversations()`
      // picks up the final LLM title.
      const conv = conversationsRef.current.find((c) => c.id === completedConversationId);
      if (conv && !conv.title) {
        const firstUserMsg = (messageCacheRef.current[completedConversationId] ?? []).find((m) => m.role === 'user');
        if (firstUserMsg) {
          const placeholder = generateTitle(firstUserMsg.content);
          if (placeholder && !cancelled) {
            // Optimistic UI only — purely local state, not persisted.
            setConversations((prev) =>
              prev.map((c) => (c.id === completedConversationId ? { ...c, title: placeholder } : c)),
            );
          }
          // Request LLM-generated title in background; DB is written once here.
          api.generateTitle(completedConversationId)
            .then((llmTitle) => {
              if (!cancelled && llmTitle) {
                setConversations((prev) =>
                  prev.map((c) => (c.id === completedConversationId ? { ...c, title: llmTitle } : c)),
                );
              }
            })
            .catch((e) => {
              // LLM title failed; placeholder remains in local state only.
              console.error('LLM title generation failed, keeping placeholder:', e);
              toast.warning(t('chat.smartTitleGenerationFailed', { message: String(e) }));
            });
        }
      }
    }
    return () => { cancelled = true; };
  }, [activeId, isStreaming, loadConversations, recordUsageCacheSampleForConversation, setMessagesForConversation, setTaskRunsForConversation, setTurnsForConversation, t]);

  /* ── Sync stream errors to chatError ────────────────────────────── */
  useEffect(() => {
    if (streamError) {
      setChatError(streamError);
      toast.error(streamError);
    }
  }, [streamError]);

  useEffect(() => {
    if (isStreaming) {
      pendingStreamConversationRef.current = null;
      return;
    }
    if (streamText.trim().length > 0) return;
    if (streamRounds.length > 0) return;
    if (traceEvents.length > 0) return;
    if (thinkingText.trim().length > 0) return;
    if (isThinking) return;
    if (toolCalls.length > 0) return;
    pendingStreamConversationRef.current = null;
    streamingConversationRef.current = null;
  }, [isStreaming, isThinking, streamRounds.length, streamText, thinkingText, toolCalls.length, traceEvents.length]);

  useEffect(() => {
    if (!activeId || !lastUsage) return;
    if (usageConversationRef.current !== activeId) return;
    setUsageCacheForConversation(activeId, lastUsage);
  }, [activeId, lastUsage, setUsageCacheForConversation]);

  /* ── Handle auto-compacted notification ──────────────────────────── */
  useEffect(() => {
    if (autoCompacted) {
      toast.info(t('chat.autoCompacted'));
    }
  }, [autoCompacted, t]);

  /* ── Handlers ───────────────────────────────────────────────────── */

  const setActiveConversation = useCallback((id: string) => {
    // When route-controlled, the caller handles navigation.
    // In uncontrolled mode, we keep the active id locally.
    setInternalConversationId(id);
  }, []);

  const createNewConversation = useCallback(() => {
    setInternalConversationId(null);
    setCustomSystemPrompt('');
    setCachedUsage(null);
    setAgentConfig(defaultAgentConfigRef.current);
    setContextWindow(defaultContextWindow);
    usageConversationRef.current = null;
    pendingStreamConversationRef.current = null;
    streamingConversationRef.current = null;
    reset();
    setChatError(null);
    lastUserMessageRef.current = null;
  }, [defaultContextWindow, reset]);

  const deleteConversation = useCallback(
    async (id: string) => {
      try {
        await api.deleteConversation(id);
        deleteUsageCacheForConversations([id]);
        setConversations((prev) => prev.filter((c) => c.id !== id));
        setMessageCache(prev => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
        setTurnCache(prev => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
        setTaskRunCache(prev => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
        delete systemPromptCacheRef.current[id];
        delete contextWindowCacheRef.current[id];
        if (activeId === id) {
          setInternalConversationId(null);
          setCachedUsage(null);
          setAgentConfig(defaultAgentConfigRef.current);
          setContextWindow(defaultContextWindow);
          usageConversationRef.current = null;
          pendingStreamConversationRef.current = null;
          streamingConversationRef.current = null;
        }
      } catch (e) {
        toast.error(formatUserError(t('chat.deleteError'), e));
      }
    },
    [activeId, defaultContextWindow, deleteUsageCacheForConversations, t],
  );

  const deleteConversationsBatch = useCallback(
    async (ids: string[]) => {
      try {
        await api.deleteConversationsBatch(ids);
        deleteUsageCacheForConversations(ids);
        const idSet = new Set(ids);
        setConversations((prev) => prev.filter((c) => !idSet.has(c.id)));
        setMessageCache(prev => {
          const next = { ...prev };
          for (const id of ids) {
            delete next[id];
            delete systemPromptCacheRef.current[id];
            delete contextWindowCacheRef.current[id];
          }
          return next;
        });
        setTurnCache(prev => {
          const next = { ...prev };
          for (const id of ids) delete next[id];
          return next;
        });
        setTaskRunCache(prev => {
          const next = { ...prev };
          for (const id of ids) delete next[id];
          return next;
        });
        if (activeId && idSet.has(activeId)) {
          setInternalConversationId(null);
          setCachedUsage(null);
          setAgentConfig(defaultAgentConfigRef.current);
          setContextWindow(defaultContextWindow);
          usageConversationRef.current = null;
          pendingStreamConversationRef.current = null;
          streamingConversationRef.current = null;
        }
      } catch (e) {
        toast.error(formatUserError(t('chat.deleteError'), e));
      }
    },
    [activeId, defaultContextWindow, deleteUsageCacheForConversations, t],
  );

  const deleteAllConversations = useCallback(async () => {
    try {
      await api.deleteAllConversations();
      usageCacheRef.current = {};
      writeUsageCache({});
      setConversations([]);
      setInternalConversationId(null);
      setMessageCache({});
      setTurnCache({});
      setTaskRunCache({});
      systemPromptCacheRef.current = {};
      contextWindowCacheRef.current = {};
      setCachedUsage(null);
      setAgentConfig(defaultAgentConfigRef.current);
      setContextWindow(defaultContextWindow);
      usageConversationRef.current = null;
      pendingStreamConversationRef.current = null;
      streamingConversationRef.current = null;
    } catch (e) {
      toast.error(formatUserError(t('chat.deleteError'), e));
    }
  }, [defaultContextWindow, t]);

  const renameConversation = useCallback(
    async (id: string, title: string) => {
      try {
        await api.renameConversation(id, title);
        setConversations((prev) =>
          prev.map((c) => (c.id === id ? { ...c, title } : c)),
        );
      } catch (e) {
        toast.error(formatUserError(t('chat.renameError'), e));
      }
    },
    [t],
  );

  const setCustomSystemPromptForActiveConversation = useCallback((prompt: string) => {
    setCustomSystemPrompt(prompt);
    if (!activeId) return;
    systemPromptCacheRef.current = {
      ...systemPromptCacheRef.current,
      [activeId]: prompt,
    };
  }, [activeId]);

  const send = useCallback(
    async (
      content: string,
      attachments?: ImageAttachment[],
      personaOverrideId?: string | null,
      options?: ChatSendOptions,
    ) => {
      const configForSend = activeAgentConfigRef.current;
      if (!configForSend) {
        toast.error(t('chat.noConfigError'));
        return;
      }
      const personaForSend = personaOverrideId ?? activePersonaId;
      const sourceIdsForSend = options?.sourceIds?.filter((id) => id.trim().length > 0) ?? [];
      const collectionContextForSend =
        options && 'collectionContext' in options
          ? options.collectionContext ?? null
          : initialCollectionContext;

      // Clear previous error
      setChatError(null);

      // Track for retry
      lastUserMessageRef.current = {
        content,
        attachments,
        personaId: personaForSend,
        options,
      };

      let convId = activeId;

      const liveStream = convId ? streamStore.getStream(convId) : undefined;
      if (convId && streamingConversationRef.current === convId && liveStream?.isStreaming) {
        const steeringConversationId = convId;
        if (attachments && attachments.length > 0) {
          toast.error(t('chat.attachmentWhileRunning'));
          return;
        }

        const currentMessages = messageCache[steeringConversationId] ?? [];
        const optimisticId = `temp-steer-${Date.now()}`;
        const optimisticMsg: ConversationMessage = {
          id: optimisticId,
          conversationId: steeringConversationId,
          role: 'user',
          content,
          toolCallId: null,
          toolCalls: [],
          artifacts: { kind: 'steering', delivery: 'pending' },
          tokenCount: 0,
          createdAt: new Date().toISOString(),
          sortOrder: currentMessages.length,
          thinking: null,
          imageAttachments: null,
        };
        setMessagesForConversation(steeringConversationId, (prev) => [...prev, optimisticMsg]);
        recordedUsageSampleRef.current = null;
        usageConversationRef.current = steeringConversationId;
        streamingConversationRef.current = steeringConversationId;

        try {
          await api.agentSteer(steeringConversationId, content);
          setMessagesForConversation(steeringConversationId, (prev) =>
            prev.map((m) =>
              m.id === optimisticId
                ? { ...m, artifacts: { kind: 'steering', delivery: 'accepted' } }
                : m,
            ),
          );
        } catch (e) {
          setMessagesForConversation(steeringConversationId, (prev) =>
            prev.filter((m) => m.id !== optimisticId),
          );
          if (isNoRunningAgentError(e)) {
            streamStore.clearStream(steeringConversationId);
            if (usageConversationRef.current === steeringConversationId) {
              usageConversationRef.current = null;
            }
            if (pendingStreamConversationRef.current === steeringConversationId) {
              pendingStreamConversationRef.current = null;
            }
            if (streamingConversationRef.current === steeringConversationId) {
              streamingConversationRef.current = null;
            }
            setChatError(null);
            await send(content, attachments, personaOverrideId, options);
            return;
          }
          const message = String(e);
          setChatError(message);
          toast.error(message);
        }
        return;
      }

      // Auto-create conversation if none active
      if (!convId) {
        try {
          const conv = collectionContextForSend
            ? await api.createConversationWithContext(
              configForSend.provider,
              configForSend.model,
              customSystemPrompt || undefined,
              collectionContextForSend,
              undefined,
              personaForSend,
            )
            : await api.createConversation(
            configForSend.provider,
            configForSend.model,
            customSystemPrompt || undefined,
            undefined,
            personaForSend,
          );
          convId = conv.id;
          // Resolve the source scope to seed the new conversation with.
          // Priority: explicit send options > live selection > initialSourceIds.
          const liveScope = getCurrentSourceScopeRef.current?.();
          const scopeToApply =
            sourceIdsForSend.length > 0
              ? sourceIdsForSend
              : liveScope && liveScope.length > 0
                ? liveScope
                : initialSourceIds;
          if (scopeToApply.length > 0) {
            await api.setConversationSources(convId, scopeToApply);
          }
          const nextConversation = collectionContextForSend
            ? { ...conv, collectionContext: collectionContextForSend }
            : conv;
          setConversations((prev) => [nextConversation, ...prev]);
          setInternalConversationId(convId);
          onConversationCreated?.(convId);
        } catch (e) {
          toast.error(formatUserError(t('chat.createError'), e));
          return;
        }
      } else {
        if (sourceIdsForSend.length > 0) {
          await api.setConversationSources(convId, sourceIdsForSend).catch(() => undefined);
        }
        if (options && 'collectionContext' in options) {
          await api.updateConversationCollectionContext(convId, collectionContextForSend)
            .then(() => {
              setConversations((prev) =>
                prev.map((conv) =>
                  conv.id === convId ? { ...conv, collectionContext: collectionContextForSend } : conv,
                ),
              );
            })
            .catch(() => undefined);
        }
      }

      const currentMessages = messageCache[convId] ?? [];

      // Add optimistic user message
      const optimisticMsg: ConversationMessage = {
        id: `temp-${Date.now()}`,
        conversationId: convId,
        role: 'user',
        content,
        toolCallId: null,
        toolCalls: [],
        artifacts: options?.userArtifacts ?? null,
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: currentMessages.length,
        thinking: null,
        imageAttachments: attachments ?? null,
      };
      setMessagesForConversation(convId, (prev) => [...prev, optimisticMsg]);
      recordedUsageSampleRef.current = null;
      usageConversationRef.current = convId;
      pendingStreamConversationRef.current = convId;
      streamingConversationRef.current = convId;

      await streamSend(
        convId,
        content,
        attachments,
        configForSend.id,
        personaForSend,
        options?.skillIds,
        options?.executionMode,
        options?.userArtifacts,
        options?.taskOrchestratorRunId,
      );
    },
    [activeId, activePersonaId, customSystemPrompt, initialCollectionContext, initialSourceIds, messageCache, streamSend, onConversationCreated, setMessagesForConversation, t],
  );

  const stop = useCallback(() => {
    const targetConversationId =
      streamingConversationRef.current ?? pendingStreamConversationRef.current ?? activeId;
    if (targetConversationId) {
      streamStop(targetConversationId);
    }
  }, [activeId, streamStop]);

  const retry = useCallback(async (messageId?: string) => {
    if (!activeId || streamStore.getStream(activeId)?.isStreaming) return;

    const messageIndexById = new Map(messages.map((message, index) => [message.id, index]));
    const explicitUserIdx = messageId
      ? messages.findIndex((message) => message.id === messageId && message.role === 'user')
      : -1;
    const lastTurn = turns.length > 0 ? turns[turns.length - 1] : null;
    const turnUserIdx =
      !messageId && lastTurn
        ? (messageIndexById.get(lastTurn.userMessageId) ?? -1)
        : -1;
    const fallbackUserIdx = !messageId
      ? (() => {
          const lastAssistantIdx = messages.map((message) => message.role).lastIndexOf('assistant');
          const searchUntil = lastAssistantIdx >= 0 ? lastAssistantIdx : messages.length;
          for (let idx = searchUntil - 1; idx >= 0; idx -= 1) {
            if (messages[idx].role === 'user' && !isSteeringMessage(messages[idx])) {
              return idx;
            }
          }
          return -1;
        })()
      : -1;
    const targetUserIdx = explicitUserIdx >= 0
      ? explicitUserIdx
      : turnUserIdx >= 0
        ? turnUserIdx
        : fallbackUserIdx;
    if (targetUserIdx < 0) return;

    const targetMessage = messages[targetUserIdx];
    if (!targetMessage || targetMessage.role !== 'user' || isSteeringMessage(targetMessage)) return;

    const fallbackRetry = lastUserMessageRef.current;
    const content = targetMessage.content;
    const attachments =
      fallbackRetry && fallbackRetry.content === content
        ? fallbackRetry.attachments
        : targetMessage.imageAttachments ?? undefined;
    const personaId =
      fallbackRetry && fallbackRetry.content === content
        ? fallbackRetry.personaId
        : activePersonaId;
    const options =
      fallbackRetry && fallbackRetry.content === content
        ? fallbackRetry.options
        : {
            userArtifacts: targetMessage.artifacts ?? null,
          };

    // Re-add optimistic user message
    const optimisticMsg: ConversationMessage = {
      id: `temp-${Date.now()}`,
      conversationId: activeId,
      role: 'user',
      content,
      toolCallId: null,
      toolCalls: [],
      artifacts: options?.userArtifacts ?? null,
      tokenCount: 0,
      createdAt: new Date().toISOString(),
      sortOrder: targetUserIdx,
      thinking: null,
      imageAttachments: attachments ?? null,
    };

    // Remove the target user message and every local message after it.
    setMessagesForConversation(activeId, (prev) => prev.slice(0, targetUserIdx));
    setTurnsForConversation(activeId, (prev) =>
      prev.filter((turn) => {
        const userIdx = messageIndexById.get(turn.userMessageId);
        return userIdx != null && userIdx < targetUserIdx;
      }),
    );
    setChatError(null);
    lastUserMessageRef.current = { content, attachments, personaId, options };

    setMessagesForConversation(activeId, (prev) => [...prev, optimisticMsg]);
    recordedUsageSampleRef.current = null;
    usageConversationRef.current = activeId;
    pendingStreamConversationRef.current = activeId;
    streamingConversationRef.current = activeId;

    await streamSend(
      activeId,
      content,
      attachments,
      activeAgentConfigRef.current?.id ?? null,
      personaId ?? activePersonaId,
      options?.skillIds,
      options?.executionMode,
      options?.userArtifacts,
      null,
    );
  }, [activeId, activePersonaId, messages, setMessagesForConversation, setTurnsForConversation, streamSend, turns]);

  /* ── Delete single message (optimistic, local only) ─────────────── */
  const deleteMessage = useCallback((messageId: string) => {
    if (!activeId) return;
    setMessagesForConversation(activeId, (prev) => prev.filter((m) => m.id !== messageId));
  }, [activeId, setMessagesForConversation]);

  /* ── Edit and re-send ───────────────────────────────────────────── */
  const editAndResend = useCallback(async (messageId: string, newContent: string) => {
    if (!activeId) return;

    const msgIndex = messages.findIndex((m) => m.id === messageId);
    if (msgIndex < 0) return;

    // Remove messages from this point onwards (optimistic)
    setMessagesForConversation(activeId, (prev) => prev.slice(0, msgIndex));
    setChatError(null);

    // Track for retry
    lastUserMessageRef.current = { content: newContent };

    // Add optimistic user message and stream
    const optimisticMsg: ConversationMessage = {
      id: `temp-${Date.now()}`,
      conversationId: activeId,
      role: 'user',
      content: newContent,
      toolCallId: null,
      toolCalls: [],
      artifacts: null,
      tokenCount: 0,
      createdAt: new Date().toISOString(),
      sortOrder: msgIndex,
      thinking: null,
      imageAttachments: null,
    };
    setMessagesForConversation(activeId, (prev) => [...prev, optimisticMsg]);
    recordedUsageSampleRef.current = null;
    usageConversationRef.current = activeId;
    pendingStreamConversationRef.current = activeId;
    streamingConversationRef.current = activeId;

    await streamSend(activeId, newContent, undefined, activeAgentConfigRef.current?.id ?? null);
  }, [activeId, messages, setMessagesForConversation, streamSend]);

  /* ── Reload messages (e.g. after compaction) ────────────────────── */
  const reloadMessages = useCallback(async (options?: { resetUsage?: boolean }) => {
    if (!activeId) return;
    if (options?.resetUsage) {
      if (activeId in usageCacheRef.current) {
        const next = { ...usageCacheRef.current };
        delete next[activeId];
        usageCacheRef.current = next;
        writeUsageCache(next);
      }
      setCachedUsage(null);
      if (usageConversationRef.current === activeId) {
        usageConversationRef.current = null;
      }
    }
    try {
      const [[, msgs], conversationTurns, agentTaskRuns] = await Promise.all([
        api.getConversation(activeId),
        api.getConversationTurns(activeId),
        api.getAgentTaskRuns(activeId),
      ]);
      setMessagesForConversation(activeId, (prev) => mergeLocalMessageState(prev, msgs));
      setTurnsForConversation(activeId, conversationTurns);
      setTaskRunsForConversation(activeId, agentTaskRuns);
    } catch { /* ignore */ }
  }, [activeId, setMessagesForConversation, setTaskRunsForConversation, setTurnsForConversation]);

  /* ── Computed ────────────────────────────────────────────────────── */

  const isViewingStreamingConversation =
    activeId != null && streamingConversationRef.current === activeId;
  const hasPersistedStreamResult = hasPersistedResultAfterLatestUserMessage(messages);
  const shouldShowLivePreview =
    isViewingStreamingConversation && (isStreaming || !hasPersistedStreamResult);
  const activeConversation = activeId
    ? conversations.find((conversation) => conversation.id === activeId) ?? null
    : null;
  const activeTurns = turns;
  const activeIsStreaming = shouldShowLivePreview && isStreaming;
  const activeStreamText = shouldShowLivePreview ? streamText : '';
  const activeStreamRounds = shouldShowLivePreview ? streamRounds : [];
  const activeTraceEvents = shouldShowLivePreview ? traceEvents : [];
  const activeThinkingText = shouldShowLivePreview ? thinkingText : '';
  const activeIsThinking = shouldShowLivePreview ? isThinking : false;
  const activeToolCalls = shouldShowLivePreview ? toolCalls : [];
  const latestPersistedTaskRun = taskRuns.length > 0 ? taskRuns[taskRuns.length - 1] : null;
  const activeTaskRun = isViewingStreamingConversation
    ? (streamTaskRun ?? latestPersistedTaskRun)
    : latestPersistedTaskRun;
  const activeTaskEvents = shouldShowLivePreview ? streamTaskEvents : [];
  const scopedLastUsage = usageConversationRef.current === activeId ? lastUsage : null;
  const scopedLastCached = usageConversationRef.current === activeId ? lastCached : false;
  const scopedFinishReason = usageConversationRef.current === activeId ? finishReason : null;
  const scopedContextOverflow = usageConversationRef.current === activeId ? contextOverflow : false;
  const scopedRateLimited = usageConversationRef.current === activeId ? rateLimited : false;
  const scopedError = usageConversationRef.current === activeId ? chatError : null;

  const usageForView = scopedLastUsage ? normalizeUsage(scopedLastUsage) : (cachedUsage ? normalizeUsage(cachedUsage) : null);
  const usageAverageForView = cachedUsage ? normalizeUsage(cachedUsage) : null;
  const estimatedPromptTokens = messages.reduce((sum, msg) => {
    if (!Number.isFinite(msg.tokenCount) || msg.tokenCount <= 0) return sum;
    return sum + msg.tokenCount;
  }, 0);

  const tokenUsage = contextWindow > 0
    ? (usageForView
      ? (() => {
          const promptTokens = usageForView.lastPromptTokens ?? usageForView.promptTokens;
          return {
            promptTokens,
            aggregatePromptTokens: usageForView.promptTokens,
            totalTokens: usageForView.totalTokens,
            contextWindow,
            completionTokens: usageForView.completionTokens,
            thinkingTokens: usageForView.thinkingTokens ?? 0,
            cacheReadTokens: usageForView.cacheReadTokens ?? 0,
            cacheMissTokens: usageForView.cacheMissTokens ?? 0,
            cacheCreationTokens: usageForView.cacheCreationTokens ?? 0,
            cacheAveragePromptTokens:
              usageForView.cacheAveragePromptTokens ?? usageAverageForView?.cacheAveragePromptTokens,
            cacheAverageReadTokens:
              usageForView.cacheAverageReadTokens ?? usageAverageForView?.cacheAverageReadTokens,
            cacheAverageMissTokens:
              usageForView.cacheAverageMissTokens ?? usageAverageForView?.cacheAverageMissTokens,
            cacheAverageCreationTokens:
              usageForView.cacheAverageCreationTokens ?? usageAverageForView?.cacheAverageCreationTokens,
            cacheAverageSampleCount:
              usageForView.cacheAverageSampleCount ?? usageAverageForView?.cacheAverageSampleCount,
            contextBreakdown: usageForView.contextBreakdown ?? buildFallbackContextBreakdown(messages, promptTokens),
            isEstimated: false,
            source: (scopedLastUsage ? 'live' : 'cached') as 'live' | 'cached',
          };
        })()
      : (estimatedPromptTokens > 0
        ? {
            promptTokens: estimatedPromptTokens,
            totalTokens: estimatedPromptTokens,
            contextWindow,
            completionTokens: 0,
            thinkingTokens: 0,
            cacheReadTokens: 0,
            cacheMissTokens: 0,
            cacheCreationTokens: 0,
            cacheAveragePromptTokens: 0,
            cacheAverageReadTokens: 0,
            cacheAverageMissTokens: 0,
            cacheAverageCreationTokens: 0,
            cacheAverageSampleCount: 0,
            contextBreakdown: buildFallbackContextBreakdown(messages, estimatedPromptTokens),
            isEstimated: true,
            source: 'estimated' as const,
          }
        : null))
    : null;

  const runtimeProfile = buildRuntimeProfile(agentConfig, activeConversation, contextWindow, t);

  return {
    messages,
    turns: activeTurns,
    taskRun: activeTaskRun,
    taskEvents: activeTaskEvents,
    conversations,
    setConversations,
    isStreaming: activeIsStreaming,
    streamText: activeStreamText,
    streamRounds: activeStreamRounds,
    traceEvents: activeTraceEvents,
    thinkingText: activeThinkingText,
    isThinking: activeIsThinking,
    toolCalls: activeToolCalls,
    loadingMsgs,
    loadingConfig: loadingConfig || loadingConvos,
    agentConfig,
    contextWindow,
    runtimeProfile,
    lastUsage: scopedLastUsage,
    tokenUsage,
    lastCached: scopedLastCached,
    finishReason: scopedFinishReason,
    contextOverflow: scopedContextOverflow,
    rateLimited: scopedRateLimited,
    send,
    stop,
    deleteConversation,
    deleteConversationsBatch,
    deleteAllConversations,
    renameConversation,
    setActiveConversation,
    createNewConversation,
    activeId,
    activeConversation,
    customSystemPrompt,
    setCustomSystemPrompt: setCustomSystemPromptForActiveConversation,
    error: scopedError,
    retry,
    clearError: () => setChatError(null),
    loadConversations,
    reloadMessages,
    deleteMessage,
    editAndResend,
    switchAgentConfig,
  };
}
