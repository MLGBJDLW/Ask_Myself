import { useState, useEffect, useCallback, useRef } from 'react';
import { toast } from 'sonner';
import * as api from './api';
import { isOptimisticSteeringMessage, isSteeringMessage } from './chatMessageGuards';
import { hasPersistedResultAfterLatestUserMessage } from './streaming/chatVisibility';
import type {
  AgentCollaborationMode,
  AgentExecutionMode,
  AgentPowerMode,
  CustomOrchestrationOptions,
  MoaPresetId,
  OrchestrationProfile,
} from './api';
import { useAgentStream, useRunningConversationIds } from './useAgentStream';
import { streamStore } from './streamStore';
import { useTranslation } from '../i18n';
import type {
  AgentConfig,
  AgentTaskRun,
  Conversation,
  ConversationMessage,
  ConversationTurn,
  ArtifactPayload,
  ContextUsageBreakdown,
  ImageAttachment,
  UsageTotal,
  UsageSnapshot,
} from '../types/conversation';
import { appTimeMs } from './dateTime';
import { formatUserError } from './userError';
import {
  estimateJsonBytes,
  upsertBoundedConversationCache,
} from './boundedConversationCache';

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

const MAX_CACHED_CONVERSATIONS = 8;
const MAX_MESSAGE_CACHE_BYTES = 64 * 1024 * 1024;
const MAX_TURN_CACHE_BYTES = 16 * 1024 * 1024;
const MAX_TASK_RUN_CACHE_BYTES = 16 * 1024 * 1024;

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
  powerMode?: AgentPowerMode;
  collaborationMode?: AgentCollaborationMode;
  moaPreset?: MoaPresetId;
  orchestrationProfile?: OrchestrationProfile;
  customOrchestration?: CustomOrchestrationOptions | null;
  taskOrchestratorRunId?: string | null;
}

export interface UseChatSessionReturn {
  messages: ConversationMessage[];
  turns: ConversationTurn[];
  taskRun: AgentTaskRun | null;
  taskEvents: ReturnType<typeof useAgentStream>['taskEvents'];
  turnTiming: ReturnType<typeof useAgentStream>['turnTiming'];
  conversations: Conversation[];
  runningConversationIds: ReadonlySet<string>;
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
    contextBreakdown?: ContextUsageBreakdown;
    isEstimated: boolean;
    source: 'live' | 'provider' | 'normalized' | 'estimated';
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
  reloadMessages: (options?: {
    resetUsage?: boolean;
    conversationId?: string;
  }) => Promise<void>;
  applyCompactionUsage: (conversationId: string, promptTokens: number) => void;
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
  const [openedConversation, setOpenedConversation] = useState<Conversation | null>(null);
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
  const [usageSnapshot, setUsageSnapshot] = useState<UsageSnapshot | null>(null);

  // Internal conversation id used when the caller does not control routing.
  const [internalConversationId, setInternalConversationId] = useState<string | null>(null);

  // The effective active conversation id
  const activeId = externalConversationId ?? internalConversationId;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;

  // Track last user message for retry
  const lastUserMessageRef = useRef<{
    content: string;
    attachments?: ImageAttachment[];
    personaId?: string | null;
    options?: ChatSendOptions;
  } | null>(null);
  const knownStreamConversationsRef = useRef<Set<string>>(new Set());
  const suppressedLiveUsageRef = useRef<Set<string>>(new Set());
  const compactionUsageRef = useRef<Map<string, UsageSnapshot>>(new Map());
  const autoTitleInFlightRef = useRef<Set<string>>(new Set());
  const systemPromptCacheRef = useRef<Record<string, string>>({});
  const contextWindowCacheRef = useRef<Record<string, number>>({});
  const messageCacheRecencyRef = useRef<Map<string, number>>(new Map());
  const turnCacheRecencyRef = useRef<Map<string, number>>(new Map());
  const taskRunCacheRecencyRef = useRef<Map<string, number>>(new Map());
  const cacheClockRef = useRef(0);
  const agentConfigsRef = useRef<AgentConfig[]>([]);
  const activeAgentConfigRef = useRef<AgentConfig | null>(null);
  const defaultAgentConfigRef = useRef<AgentConfig | null>(null);
  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  const openedConversationRef = useRef(openedConversation);
  openedConversationRef.current = openedConversation;
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
      return upsertBoundedConversationCache(prev, conversationId, nextMessages, {
        maxEntries: MAX_CACHED_CONVERSATIONS,
        maxBytes: MAX_MESSAGE_CACHE_BYTES,
        estimateBytes: estimateJsonBytes,
        recency: messageCacheRecencyRef.current,
        protectedKeys: [
          ...(activeIdRef.current ? [activeIdRef.current] : []),
          ...streamStore.getRunningConversationIds(),
        ],
        tick: ++cacheClockRef.current,
      });
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
      return upsertBoundedConversationCache(prev, conversationId, nextTurns, {
        maxEntries: MAX_CACHED_CONVERSATIONS,
        maxBytes: MAX_TURN_CACHE_BYTES,
        estimateBytes: estimateJsonBytes,
        recency: turnCacheRecencyRef.current,
        protectedKeys: [
          ...(activeIdRef.current ? [activeIdRef.current] : []),
          ...streamStore.getRunningConversationIds(),
        ],
        tick: ++cacheClockRef.current,
      });
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
      return upsertBoundedConversationCache(prev, conversationId, nextRuns, {
        maxEntries: MAX_CACHED_CONVERSATIONS,
        maxBytes: MAX_TASK_RUN_CACHE_BYTES,
        estimateBytes: estimateJsonBytes,
        recency: taskRunCacheRecencyRef.current,
        protectedKeys: [
          ...(activeIdRef.current ? [activeIdRef.current] : []),
          ...streamStore.getRunningConversationIds(),
        ],
        tick: ++cacheClockRef.current,
      });
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
    turnTiming: streamTurnTiming,
  } = useAgentStream(activeId);
  const runningConversationIds = useRunningConversationIds();

  useEffect(() => {
    for (const conversationId of runningConversationIds) {
      knownStreamConversationsRef.current.add(conversationId);
    }
  }, [runningConversationIds]);

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
      setOpenedConversation(null);
      setUsageSnapshot(null);
      setCustomSystemPrompt(externalSystemPrompt ?? '');
      setAgentConfig(defaultAgentConfigRef.current);
      setContextWindow(defaultContextWindow);
      setLoadingMsgs(false);
      return;
    }

    setOpenedConversation(
      conversationsRef.current.find((conversation) => conversation.id === activeId) ?? null,
    );
    setCustomSystemPrompt(systemPromptCacheRef.current[activeId] ?? '');
    setContextWindow(contextWindowCacheRef.current[activeId] ?? defaultContextWindow);
    setUsageSnapshot(null);

    if (isStreaming) {
      setLoadingMsgs(false);
      return;
    }
    let cancelled = false;
    setLoadingMsgs(true);
    void (async () => {
      try {
        const [[conv, msgs], conversationTurns, agentTaskRuns, durableUsage] = await Promise.all([
          api.getConversation(activeId),
          api.getConversationTurns(activeId),
          api.getAgentTaskRuns(activeId),
          api.getConversationUsageSnapshot(activeId),
        ]);
        if (cancelled) return;
        setOpenedConversation(conv);
        // Safety net (also covers pre-Tier-B persisted rows): preserve any
        // imageAttachments present in prior in-memory state when the backend
        // response lacks them (e.g. optimistic temp-* ids or legacy rows).
        setMessagesForConversation(activeId, (prev) => mergeLocalMessageState(prev, msgs));
        setTurnsForConversation(activeId, conversationTurns);
        setTaskRunsForConversation(activeId, agentTaskRuns);
        setUsageSnapshot(compactionUsageRef.current.get(activeId) ?? durableUsage);
        const resumableRun = [...agentTaskRuns].reverse().find(taskRunCanResumeStream);
        if (resumableRun && !streamHasVisiblePreview(activeId)) {
          Promise.all([
            api.getAgentTaskRunEvents(resumableRun.id).catch(() => []),
            api.getAgentRunEvents(resumableRun.id).catch(() => []),
          ])
            .then(([taskEvents, runEvents]) => {
              if (cancelled) return;
              streamStore.restoreFromRunEvents(activeId, resumableRun, runEvents, taskEvents);
              const restoredStream = streamStore.getStream(activeId);
              if (restoredStream?.isStreaming) {
                knownStreamConversationsRef.current.add(activeId);
              }
            })
            .catch(() => undefined);
        }
        setConversations((prev) => {
          if (conv.archivedAt) {
            return prev.filter((item) => item.id !== conv.id);
          }
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
    const completedConversationId = activeId
      && !isStreaming
      && knownStreamConversationsRef.current.has(activeId)
      ? activeId
      : null;
    if (completedConversationId) {
      knownStreamConversationsRef.current.delete(completedConversationId);
      // Re-fetch messages after agent is done.
      const refreshConversationPromise = Promise.all([
        api.getConversation(completedConversationId),
        api.getConversationTurns(completedConversationId),
        api.getAgentTaskRuns(completedConversationId),
        api.getConversationUsageSnapshot(completedConversationId),
      ]).then(([[conv, msgs], conversationTurns, agentTaskRuns, durableUsage]) => {
        if (!cancelled) {
          // Safety net (also covers pre-Tier-B persisted rows): preserve any
          // imageAttachments present in prior in-memory state when the backend
          // response lacks them (e.g. optimistic temp-* ids or legacy rows).
          setMessagesForConversation(completedConversationId, (prev) =>
            mergeLocalMessageState(prev, msgs),
          );
          setTurnsForConversation(completedConversationId, conversationTurns);
          setTaskRunsForConversation(completedConversationId, agentTaskRuns);
          if (activeId === completedConversationId) {
            setOpenedConversation(conv);
            setUsageSnapshot(
              compactionUsageRef.current.get(completedConversationId) ?? durableUsage,
            );
          }
          setConversations((prev) => {
            if (conv.archivedAt) {
              return prev.filter((item) => item.id !== conv.id);
            }
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
      const refreshListPromise = loadConversations();

      // Auto-title runs after every completed turn while the title remains
      // auto-managed. This lets the opening placeholder mature as the topic
      // becomes clearer. The backend atomically refuses to overwrite a manual
      // rename that happens while generation is in flight.
      const conv = conversationsRef.current.find((c) => c.id === completedConversationId);
      if (
        conv
        && conv.titleIsAuto !== false
        && !autoTitleInFlightRef.current.has(completedConversationId)
      ) {
        const firstUserMsg = (messageCacheRef.current[completedConversationId] ?? []).find((m) => m.role === 'user');
        if (!conv.title && firstUserMsg) {
          const placeholder = generateTitle(firstUserMsg.content);
          if (placeholder && !cancelled) {
            setConversations((prev) =>
              prev.map((c) => (c.id === completedConversationId ? { ...c, title: placeholder } : c)),
            );
          }
        }
        autoTitleInFlightRef.current.add(completedConversationId);
        // Both refresh paths can carry the pre-generation title. Wait for them
        // before generating so a late stale response cannot overwrite the new
        // title in local state.
        Promise.allSettled([refreshConversationPromise, refreshListPromise])
          .then(() => api.generateTitle(completedConversationId))
          .then((llmTitle) => {
            if (llmTitle) {
              setConversations((prev) =>
                prev.map((c) => (c.id === completedConversationId ? { ...c, title: llmTitle } : c)),
              );
            }
          })
          .catch((e) => {
            console.error('Automatic title generation failed:', e);
            toast.warning(t('chat.smartTitleGenerationFailed', { message: String(e) }));
          })
          .finally(() => {
            autoTitleInFlightRef.current.delete(completedConversationId);
          });
      }
    }
    return () => { cancelled = true; };
  }, [activeId, isStreaming, loadConversations, setMessagesForConversation, setTaskRunsForConversation, setTurnsForConversation, t]);

  /* ── Sync stream errors to chatError ────────────────────────────── */
  useEffect(() => {
    if (streamError) {
      setChatError(streamError);
      toast.error(streamError);
    }
  }, [streamError]);

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
    setUsageSnapshot(null);
    setAgentConfig(defaultAgentConfigRef.current);
    setContextWindow(defaultContextWindow);
    setChatError(null);
    lastUserMessageRef.current = null;
  }, [defaultContextWindow]);

  const deleteConversation = useCallback(
    async (id: string) => {
      try {
        await api.deleteConversation(id);
        setConversations((prev) => prev.filter((c) => c.id !== id));
        setMessageCache(prev => {
          const next = { ...prev };
          delete next[id];
          messageCacheRecencyRef.current.delete(id);
          return next;
        });
        setTurnCache(prev => {
          const next = { ...prev };
          delete next[id];
          turnCacheRecencyRef.current.delete(id);
          return next;
        });
        setTaskRunCache(prev => {
          const next = { ...prev };
          delete next[id];
          taskRunCacheRecencyRef.current.delete(id);
          return next;
        });
        delete systemPromptCacheRef.current[id];
        delete contextWindowCacheRef.current[id];
        if (activeId === id) {
          setInternalConversationId(null);
          setUsageSnapshot(null);
          setAgentConfig(defaultAgentConfigRef.current);
          setContextWindow(defaultContextWindow);
        }
      } catch (e) {
        toast.error(formatUserError(t('chat.deleteError'), e));
      }
    },
    [activeId, defaultContextWindow, t],
  );

  const deleteConversationsBatch = useCallback(
    async (ids: string[]) => {
      try {
        await api.deleteConversationsBatch(ids);
        const idSet = new Set(ids);
        setConversations((prev) => prev.filter((c) => !idSet.has(c.id)));
        setMessageCache(prev => {
          const next = { ...prev };
          for (const id of ids) {
            delete next[id];
            messageCacheRecencyRef.current.delete(id);
            delete systemPromptCacheRef.current[id];
            delete contextWindowCacheRef.current[id];
          }
          return next;
        });
        setTurnCache(prev => {
          const next = { ...prev };
          for (const id of ids) {
            delete next[id];
            turnCacheRecencyRef.current.delete(id);
          }
          return next;
        });
        setTaskRunCache(prev => {
          const next = { ...prev };
          for (const id of ids) {
            delete next[id];
            taskRunCacheRecencyRef.current.delete(id);
          }
          return next;
        });
        if (activeId && idSet.has(activeId)) {
          setInternalConversationId(null);
          setUsageSnapshot(null);
          setAgentConfig(defaultAgentConfigRef.current);
          setContextWindow(defaultContextWindow);
        }
      } catch (e) {
        toast.error(formatUserError(t('chat.deleteError'), e));
      }
    },
    [activeId, defaultContextWindow, t],
  );

  const deleteAllConversations = useCallback(async () => {
    try {
      await api.deleteAllConversations();
      setConversations([]);
      setInternalConversationId(null);
      setMessageCache({});
      setTurnCache({});
      setTaskRunCache({});
      messageCacheRecencyRef.current.clear();
      turnCacheRecencyRef.current.clear();
      taskRunCacheRecencyRef.current.clear();
      systemPromptCacheRef.current = {};
      contextWindowCacheRef.current = {};
      setUsageSnapshot(null);
      setAgentConfig(defaultAgentConfigRef.current);
      setContextWindow(defaultContextWindow);
    } catch (e) {
      toast.error(formatUserError(t('chat.deleteError'), e));
    }
  }, [defaultContextWindow, t]);

  const renameConversation = useCallback(
    async (id: string, title: string) => {
      try {
        await api.renameConversation(id, title);
        setConversations((prev) =>
          prev.map((c) => (c.id === id ? { ...c, title, titleIsAuto: false } : c)),
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
      const conversationForSend = activeId
        ? conversationsRef.current.find((conversation) => conversation.id === activeId)
          ?? (openedConversationRef.current?.id === activeId ? openedConversationRef.current : null)
        : null;
      if (conversationForSend?.archivedAt) {
        toast.error(t('chat.archivedReadOnlyError'));
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
      if (convId && liveStream?.isStreaming) {
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
        knownStreamConversationsRef.current.add(steeringConversationId);

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
            knownStreamConversationsRef.current.delete(steeringConversationId);
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
      knownStreamConversationsRef.current.add(convId);
      suppressedLiveUsageRef.current.delete(convId);
      compactionUsageRef.current.delete(convId);

      await streamSend(
        convId,
        content,
        attachments,
        configForSend.id,
        personaForSend,
        options?.skillIds,
        options?.executionMode,
        options?.powerMode,
        options?.collaborationMode,
        options?.moaPreset,
        options?.orchestrationProfile,
        options?.customOrchestration,
        options?.userArtifacts,
        options?.taskOrchestratorRunId,
      );
    },
    [activeId, activePersonaId, customSystemPrompt, initialCollectionContext, initialSourceIds, messageCache, streamSend, onConversationCreated, setMessagesForConversation, t],
  );

  const stop = useCallback(() => {
    if (activeId) {
      streamStop(activeId);
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
    knownStreamConversationsRef.current.add(activeId);
    suppressedLiveUsageRef.current.delete(activeId);
    compactionUsageRef.current.delete(activeId);

    await streamSend(
      activeId,
      content,
      attachments,
      activeAgentConfigRef.current?.id ?? null,
      personaId ?? activePersonaId,
      options?.skillIds,
      options?.executionMode,
      options?.powerMode,
      options?.collaborationMode,
      options?.moaPreset,
      options?.orchestrationProfile,
      options?.customOrchestration,
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
    knownStreamConversationsRef.current.add(activeId);
    suppressedLiveUsageRef.current.delete(activeId);
    compactionUsageRef.current.delete(activeId);

    await streamSend(activeId, newContent, undefined, activeAgentConfigRef.current?.id ?? null);
  }, [activeId, messages, setMessagesForConversation, streamSend]);

  /* ── Reload messages (e.g. after compaction) ────────────────────── */
  const reloadMessages = useCallback(async (options?: {
    resetUsage?: boolean;
    conversationId?: string;
  }) => {
    const targetConversationId = options?.conversationId ?? activeId;
    if (!targetConversationId) return;
    if (options?.resetUsage) {
      if (activeIdRef.current === targetConversationId) {
        setUsageSnapshot(null);
      }
      suppressedLiveUsageRef.current.add(targetConversationId);
    }
    try {
      const [[, msgs], conversationTurns, agentTaskRuns, durableUsage] = await Promise.all([
        api.getConversation(targetConversationId),
        api.getConversationTurns(targetConversationId),
        api.getAgentTaskRuns(targetConversationId),
        options?.resetUsage
          ? Promise.resolve(null)
          : api.getConversationUsageSnapshot(targetConversationId),
      ]);
      setMessagesForConversation(targetConversationId, (prev) => mergeLocalMessageState(prev, msgs));
      setTurnsForConversation(targetConversationId, conversationTurns);
      setTaskRunsForConversation(targetConversationId, agentTaskRuns);
      if (activeIdRef.current === targetConversationId) {
        setUsageSnapshot(
          compactionUsageRef.current.get(targetConversationId) ?? durableUsage,
        );
      }
    } catch { /* ignore */ }
  }, [activeId, setMessagesForConversation, setTaskRunsForConversation, setTurnsForConversation]);

  const applyCompactionUsage = useCallback((
    conversationId: string,
    promptTokens: number,
  ) => {
    suppressedLiveUsageRef.current.add(conversationId);
    if (activeIdRef.current !== conversationId) return;
    const snapshot: UsageSnapshot = {
      source: 'estimated',
      promptTokens,
      completionTokens: 0,
      totalTokens: promptTokens,
      thinkingTokens: 0,
      cacheReadTokens: 0,
      cacheMissTokens: promptTokens,
      cacheCreationTokens: 0,
      lastPromptTokens: promptTokens,
      providerRaw: {
        kind: 'contextCompactionProjection',
      },
    };
    compactionUsageRef.current.set(conversationId, snapshot);
    setUsageSnapshot(snapshot);
  }, []);

  /* ── Computed ────────────────────────────────────────────────────── */

  const isViewingStreamingConversation =
    activeId != null && streamHasVisiblePreview(activeId);
  const hasPersistedStreamResult = hasPersistedResultAfterLatestUserMessage(messages);
  const shouldShowLivePreview =
    isViewingStreamingConversation && (isStreaming || !hasPersistedStreamResult);
  const activeConversation = activeId
    ? conversations.find((conversation) => conversation.id === activeId)
      ?? (openedConversation?.id === activeId ? openedConversation : null)
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
  const activeTurnTiming = shouldShowLivePreview
    ? streamTurnTiming
    : activeTaskRun
      ? (() => {
          const startedAt = Date.parse(activeTaskRun.startedAt ?? activeTaskRun.createdAt);
          const finishedAt = activeTaskRun.finishedAt ? Date.parse(activeTaskRun.finishedAt) : null;
          if (!Number.isFinite(startedAt)) return null;
          return {
            startedAtEpochMs: startedAt,
            startedAtMonotonicMs: null,
            firstEventAtEpochMs: null,
            firstVisibleOutputAtEpochMs: null,
            finishedAtEpochMs: finishedAt != null && Number.isFinite(finishedAt) ? finishedAt : null,
            finishedAtMonotonicMs: null,
          };
        })()
      : null;
  const liveUsageSuppressed = activeId ? suppressedLiveUsageRef.current.has(activeId) : false;
  const scopedLastUsage = activeId && !liveUsageSuppressed ? lastUsage : null;
  const scopedLastCached = activeId && !liveUsageSuppressed ? lastCached : false;
  const scopedFinishReason = activeId && !liveUsageSuppressed ? finishReason : null;
  const scopedContextOverflow = activeId && !liveUsageSuppressed ? contextOverflow : false;
  const scopedRateLimited = activeId && !liveUsageSuppressed ? rateLimited : false;
  const scopedError = activeId ? chatError : null;

  // Streaming usage describes the in-flight run. Once the run is durable,
  // prefer the backend conversation snapshot so cache and token totals remain
  // aggregated across completed turns instead of falling back to only the
  // latest run kept by the stream store.
  const isUsingLiveUsage = (shouldShowLivePreview || usageSnapshot == null) && scopedLastUsage != null;
  const usageForView = isUsingLiveUsage ? scopedLastUsage : usageSnapshot ?? scopedLastUsage;

  const tokenUsage = contextWindow > 0
    ? (usageForView
      ? (() => {
          const promptTokens = usageForView.lastPromptTokens ?? usageForView.promptTokens;
          const source = isUsingLiveUsage ? 'live' as const : usageSnapshot?.source ?? 'normalized';
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
            contextBreakdown: usageForView.contextBreakdown,
            isEstimated: source === 'estimated',
            source,
          };
        })()
      : null)
    : null;

  const runtimeProfile = buildRuntimeProfile(agentConfig, activeConversation, contextWindow, t);

  return {
    messages,
    turns: activeTurns,
    taskRun: activeTaskRun,
    taskEvents: activeTaskEvents,
    turnTiming: activeTurnTiming,
    conversations,
    runningConversationIds,
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
    applyCompactionUsage,
    deleteMessage,
    editAndResend,
    switchAgentConfig,
  };
}
