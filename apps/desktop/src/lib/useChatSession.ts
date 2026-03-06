import { useState, useEffect, useCallback, useRef } from 'react';
import { toast } from 'sonner';
import * as api from './api';
import { useAgentStream, UsageTotal } from './useAgentStream';
import { useTranslation } from '../i18n';
import type { Conversation, ConversationMessage, AgentConfig, ImageAttachment } from '../types/conversation';
import { appTimeMs } from './dateTime';

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
  lastPromptTokens: number;
  updatedAt: number;
}

function sanitizeNumber(input: unknown, fallback = 0): number {
  if (typeof input !== 'number' || !Number.isFinite(input)) return fallback;
  return Math.max(0, Math.round(input));
}

function normalizeUsage(usage: UsageTotal): UsageTotal {
  const promptTokens = sanitizeNumber(usage.promptTokens);
  const completionTokens = sanitizeNumber(usage.completionTokens);
  const totalTokens = sanitizeNumber(usage.totalTokens, promptTokens + completionTokens);
  const thinkingTokens = sanitizeNumber(usage.thinkingTokens ?? 0);
  const lastPromptTokens = sanitizeNumber(usage.lastPromptTokens ?? promptTokens, promptTokens);
  return {
    promptTokens,
    completionTokens,
    totalTokens,
    thinkingTokens,
    lastPromptTokens,
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
      const lastPromptTokens = sanitizeNumber(row.lastPromptTokens ?? promptTokens, promptTokens);
      const updatedAt = sanitizeNumber(row.updatedAt ?? Date.now(), Date.now());
      next[conversationId] = {
        promptTokens,
        completionTokens,
        totalTokens,
        thinkingTokens,
        lastPromptTokens,
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
}

export interface UseChatSessionReturn {
  messages: ConversationMessage[];
  conversations: Conversation[];
  setConversations: React.Dispatch<React.SetStateAction<Conversation[]>>;
  isStreaming: boolean;
  streamText: string;
  streamRounds: ReturnType<typeof useAgentStream>['streamRounds'];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ReturnType<typeof useAgentStream>['toolCalls'];
  loadingMsgs: boolean;
  loadingConfig: boolean;
  agentConfig: AgentConfig | null;
  contextWindow: number;
  lastUsage: UsageTotal | null;
  tokenUsage: {
    promptTokens: number;
    totalTokens: number;
    contextWindow: number;
    completionTokens: number;
    thinkingTokens: number;
    isEstimated: boolean;
    source: 'live' | 'cached' | 'estimated';
  } | null;
  lastCached: boolean;
  finishReason: string | null;
  contextOverflow: boolean;
  rateLimited: boolean;
  send: (content: string, images?: ImageAttachment[]) => Promise<void>;
  stop: () => void;
  deleteConversation: (id: string) => Promise<void>;
  deleteConversationsBatch: (ids: string[]) => Promise<void>;
  deleteAllConversations: () => Promise<void>;
  renameConversation: (id: string, title: string) => Promise<void>;
  setActiveConversation: (id: string) => void;
  createNewConversation: () => void;
  activeId: string | null;
  customSystemPrompt: string;
  setCustomSystemPrompt: (prompt: string) => void;
  error: string | null;
  retry: () => Promise<void>;
  clearError: () => void;
  loadConversations: () => Promise<void>;
  reloadMessages: () => Promise<void>;
  deleteMessage: (messageId: string) => void;
  editAndResend: (messageId: string, newContent: string) => Promise<void>;
}

/* ------------------------------------------------------------------ */
/*  Hook                                                               */
/* ------------------------------------------------------------------ */

export function useChatSession(options: UseChatSessionOptions = {}): UseChatSessionReturn {
  const { conversationId: externalConversationId, onConversationCreated, systemPrompt: externalSystemPrompt } = options;

  const { t } = useTranslation();

  /* ── State ──────────────────────────────────────────────────────── */
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
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
  const lastUserMessageRef = useRef<{ content: string; attachments?: ImageAttachment[] } | null>(null);
  const usageConversationRef = useRef<string | null>(null);
  const usageCacheRef = useRef<Record<string, StoredUsageEntry>>(readUsageCache());

  const {
    send: streamSend,
    stop: streamStop,
    isStreaming,
    streamText,
    streamRounds,
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
    reset,
  } = useAgentStream();

  const setUsageCacheForConversation = useCallback((conversationId: string, usage: UsageTotal) => {
    const normalized = normalizeUsage(usage);
    usageCacheRef.current = {
      ...usageCacheRef.current,
      [conversationId]: {
        promptTokens: normalized.promptTokens,
        completionTokens: normalized.completionTokens,
        totalTokens: normalized.totalTokens,
        thinkingTokens: normalized.thinkingTokens ?? 0,
        lastPromptTokens: normalized.lastPromptTokens ?? normalized.promptTokens,
        updatedAt: Date.now(),
      },
    };
    writeUsageCache(usageCacheRef.current);
    setCachedUsage(normalized);
  }, []);

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
      toast.error(`${t('chat.loadError')}: ${String(e)}`);
    } finally {
      setLoadingConvos(false);
    }
  }, [t]);

  /* ── Load default agent config ──────────────────────────────────── */
  const loadDefaultConfig = useCallback(async () => {
    try {
      const configs = await api.listAgentConfigs();
      const def = configs.find((c) => c.isDefault) ?? configs[0] ?? null;
      setAgentConfig(def);
      if (def) {
        const cw = def.contextWindow ?? await api.getModelContextWindow(def.model).catch(() => 0);
        setDefaultContextWindow(cw);
        setContextWindow(cw);
      } else {
        setDefaultContextWindow(0);
        setContextWindow(0);
      }
    } catch {
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
      setMessages([]);
      setCachedUsage(null);
      setContextWindow(defaultContextWindow);
      return;
    }
    let cancelled = false;
    setLoadingMsgs(true);
    reset();
    const restored = usageCacheRef.current[activeId];
    setCachedUsage(
      restored
        ? {
            promptTokens: restored.promptTokens,
            completionTokens: restored.completionTokens,
            totalTokens: restored.totalTokens,
            thinkingTokens: restored.thinkingTokens,
            lastPromptTokens: restored.lastPromptTokens,
          }
        : null,
    );

    void (async () => {
      try {
        const [conv, msgs] = await api.getConversation(activeId);
        if (cancelled) return;
        setMessages(msgs);
        setCustomSystemPrompt(conv.systemPrompt ?? '');
        const cw = await api.getModelContextWindow(conv.model).catch(() => 0);
        if (!cancelled) {
          setContextWindow(cw || defaultContextWindow);
        }
      } catch {
        if (!cancelled) {
          setMessages([]);
          setContextWindow(defaultContextWindow);
        }
      } finally {
        if (!cancelled) setLoadingMsgs(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeId, reset, defaultContextWindow]);

  /* ── Reload messages when streaming completes ───────────────────── */
  useEffect(() => {
    let cancelled = false;
    if (!isStreaming && activeId && messages.length > 0) {
      // Re-fetch messages after agent is done.
      api.getConversation(activeId).then(([, msgs]) => {
        if (!cancelled) setMessages(msgs);
      }).catch((e) => {
        console.error('Failed to refresh messages after streaming:', e);
      });
      // Also refresh conversation list (updatedAt changes)
      loadConversations();

      // Auto-title: generate title from first user message if untitled
      const conv = conversations.find((c) => c.id === activeId);
      if (conv && !conv.title) {
        const firstUserMsg = messages.find((m) => m.role === 'user');
        if (firstUserMsg) {
          const title = generateTitle(firstUserMsg.content);
          if (title) {
            api.renameConversation(activeId, title)
              .then(() => {
                if (!cancelled) {
                  setConversations((prev) =>
                    prev.map((c) => (c.id === activeId ? { ...c, title } : c)),
                  );
                }
              })
              .catch((e) => {
                // Auto-title is cosmetic; log but don't interrupt user
                console.error('Failed to auto-title conversation:', e);
              });
          }
        }
      }
    }
    return () => { cancelled = true; };
    // Only trigger on isStreaming becoming false
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isStreaming]);

  /* ── Sync stream errors to chatError ────────────────────────────── */
  useEffect(() => {
    if (streamError) {
      setChatError(streamError);
      toast.error(streamError);
    }
  }, [streamError]);

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
    setMessages([]);
    setCustomSystemPrompt('');
    setCachedUsage(null);
    setContextWindow(defaultContextWindow);
    usageConversationRef.current = null;
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
        if (activeId === id) {
          setInternalConversationId(null);
          setMessages([]);
          setCachedUsage(null);
          setContextWindow(defaultContextWindow);
          usageConversationRef.current = null;
        }
      } catch (e) {
        toast.error(`${t('chat.deleteError')}: ${String(e)}`);
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
        if (activeId && idSet.has(activeId)) {
          setInternalConversationId(null);
          setMessages([]);
          setCachedUsage(null);
          setContextWindow(defaultContextWindow);
          usageConversationRef.current = null;
        }
      } catch (e) {
        toast.error(`${t('chat.deleteError')}: ${String(e)}`);
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
      setMessages([]);
      setCachedUsage(null);
      setContextWindow(defaultContextWindow);
      usageConversationRef.current = null;
    } catch (e) {
      toast.error(`${t('chat.deleteError')}: ${String(e)}`);
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
        toast.error(`${t('chat.renameError')}: ${String(e)}`);
      }
    },
    [t],
  );

  const send = useCallback(
    async (content: string, attachments?: ImageAttachment[]) => {
      if (!agentConfig) {
        toast.error(t('chat.noConfigError'));
        return;
      }

      // Clear previous error
      setChatError(null);

      // Track for retry
      lastUserMessageRef.current = { content, attachments };

      let convId = activeId;

      // Auto-create conversation if none active
      if (!convId) {
        try {
          const conv = await api.createConversation(
            agentConfig.provider,
            agentConfig.model,
            customSystemPrompt || undefined,
          );
          convId = conv.id;
          setConversations((prev) => [conv, ...prev]);
          setInternalConversationId(convId);
          onConversationCreated?.(convId);
        } catch (e) {
          toast.error(`${t('chat.createError')}: ${String(e)}`);
          return;
        }
      }

      // Add optimistic user message
      const optimisticMsg: ConversationMessage = {
        id: `temp-${Date.now()}`,
        conversationId: convId,
        role: 'user',
        content,
        toolCallId: null,
        toolCalls: [],
        artifacts: null,
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: messages.length,
        thinking: null,
        imageAttachments: attachments ?? null,
      };
      setMessages((prev) => [...prev, optimisticMsg]);
      usageConversationRef.current = convId;

      await streamSend(convId, content, attachments);
    },
    [activeId, agentConfig, customSystemPrompt, messages.length, streamSend, onConversationCreated, t],
  );

  const stop = useCallback(() => {
    if (activeId) {
      streamStop(activeId);
    }
  }, [activeId, streamStop]);

  const retry = useCallback(async () => {
    if (!lastUserMessageRef.current || !activeId) return;

    // Remove the last user message and any subsequent assistant messages from local state
    const lastUserIdx = messages.map(m => m.role).lastIndexOf('user');
    if (lastUserIdx >= 0) {
      setMessages((prev) => prev.slice(0, lastUserIdx));
    }

    setChatError(null);

    const { content, attachments } = lastUserMessageRef.current;

    // Re-add optimistic user message
    const optimisticMsg: ConversationMessage = {
      id: `temp-${Date.now()}`,
      conversationId: activeId,
      role: 'user',
      content,
      toolCallId: null,
      toolCalls: [],
      artifacts: null,
      tokenCount: 0,
      createdAt: new Date().toISOString(),
      sortOrder: messages.length,
      thinking: null,
      imageAttachments: attachments ?? null,
    };
    setMessages((prev) => [...prev, optimisticMsg]);
    usageConversationRef.current = activeId;

    await streamSend(activeId, content, attachments);
  }, [activeId, messages, streamSend]);

  /* ── Delete single message (optimistic, local only) ─────────────── */
  const deleteMessage = useCallback((messageId: string) => {
    setMessages((prev) => prev.filter((m) => m.id !== messageId));
  }, []);

  /* ── Edit and re-send ───────────────────────────────────────────── */
  const editAndResend = useCallback(async (messageId: string, newContent: string) => {
    if (!activeId) return;

    const msgIndex = messages.findIndex((m) => m.id === messageId);
    if (msgIndex < 0) return;

    // Remove messages from this point onwards (optimistic)
    setMessages((prev) => prev.slice(0, msgIndex));
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
    setMessages((prev) => [...prev, optimisticMsg]);
    usageConversationRef.current = activeId;

    await streamSend(activeId, newContent);
  }, [activeId, messages, streamSend]);

  /* ── Reload messages (e.g. after compaction) ────────────────────── */
  const reloadMessages = useCallback(async () => {
    if (!activeId) return;
    try {
      const [, msgs] = await api.getConversation(activeId);
      setMessages(msgs);
    } catch { /* ignore */ }
  }, [activeId]);

  /* ── Computed ────────────────────────────────────────────────────── */

  const usageForView = lastUsage ? normalizeUsage(lastUsage) : (cachedUsage ? normalizeUsage(cachedUsage) : null);
  const estimatedPromptTokens = messages.reduce((sum, msg) => {
    if (!Number.isFinite(msg.tokenCount) || msg.tokenCount <= 0) return sum;
    return sum + msg.tokenCount;
  }, 0);

  const tokenUsage = contextWindow > 0
    ? (usageForView
      ? {
          promptTokens: usageForView.lastPromptTokens ?? usageForView.promptTokens,
          totalTokens: usageForView.totalTokens,
          contextWindow,
          completionTokens: usageForView.completionTokens,
          thinkingTokens: usageForView.thinkingTokens ?? 0,
          isEstimated: false,
          source: (lastUsage ? 'live' : 'cached') as 'live' | 'cached',
        }
      : (estimatedPromptTokens > 0
        ? {
            promptTokens: estimatedPromptTokens,
            totalTokens: estimatedPromptTokens,
            contextWindow,
            completionTokens: 0,
            thinkingTokens: 0,
            isEstimated: true,
            source: 'estimated' as const,
          }
        : null))
    : null;

  return {
    messages,
    conversations,
    setConversations,
    isStreaming,
    streamText,
    streamRounds,
    thinkingText,
    isThinking,
    toolCalls,
    loadingMsgs,
    loadingConfig: loadingConfig || loadingConvos,
    agentConfig,
    contextWindow,
    lastUsage,
    tokenUsage,
    lastCached,
    finishReason,
    contextOverflow,
    rateLimited,
    send,
    stop,
    deleteConversation,
    deleteConversationsBatch,
    deleteAllConversations,
    renameConversation,
    setActiveConversation,
    createNewConversation,
    activeId,
    customSystemPrompt,
    setCustomSystemPrompt,
    error: chatError,
    retry,
    clearError: () => setChatError(null),
    loadConversations,
    reloadMessages,
    deleteMessage,
    editAndResend,
  };
}
