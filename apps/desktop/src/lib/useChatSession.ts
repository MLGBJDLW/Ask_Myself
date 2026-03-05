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
  tokenUsage: { promptTokens: number; totalTokens: number; contextWindow: number; completionTokens: number; thinkingTokens: number } | null;
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
  const [contextWindow, setContextWindow] = useState<number>(0);
  const [chatError, setChatError] = useState<string | null>(null);

  // Internal conversation id used when the caller does not control routing.
  const [internalConversationId, setInternalConversationId] = useState<string | null>(null);

  // The effective active conversation id
  const activeId = externalConversationId ?? internalConversationId;

  // Track last user message for retry
  const lastUserMessageRef = useRef<{ content: string; attachments?: ImageAttachment[] } | null>(null);

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
        setContextWindow(cw);
      }
    } catch {
      setAgentConfig(null);
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
      return;
    }
    let cancelled = false;
    setLoadingMsgs(true);
    reset();
    api
      .getConversation(activeId)
      .then(([conv, msgs]) => {
        if (!cancelled) {
          setMessages(msgs);
          setCustomSystemPrompt(conv.systemPrompt ?? '');
        }
      })
      .catch(() => {
        if (!cancelled) setMessages([]);
      })
      .finally(() => {
        if (!cancelled) setLoadingMsgs(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeId, reset]);

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
    reset();
    setChatError(null);
    lastUserMessageRef.current = null;
  }, [reset]);

  const deleteConversation = useCallback(
    async (id: string) => {
      try {
        await api.deleteConversation(id);
        setConversations((prev) => prev.filter((c) => c.id !== id));
        if (activeId === id) {
          setInternalConversationId(null);
          setMessages([]);
        }
      } catch (e) {
        toast.error(`${t('chat.deleteError')}: ${String(e)}`);
      }
    },
    [activeId, t],
  );

  const deleteConversationsBatch = useCallback(
    async (ids: string[]) => {
      try {
        await api.deleteConversationsBatch(ids);
        const idSet = new Set(ids);
        setConversations((prev) => prev.filter((c) => !idSet.has(c.id)));
        if (activeId && idSet.has(activeId)) {
          setInternalConversationId(null);
          setMessages([]);
        }
      } catch (e) {
        toast.error(`${t('chat.deleteError')}: ${String(e)}`);
      }
    },
    [activeId, t],
  );

  const deleteAllConversations = useCallback(async () => {
    try {
      await api.deleteAllConversations();
      setConversations([]);
      setInternalConversationId(null);
      setMessages([]);
    } catch (e) {
      toast.error(`${t('chat.deleteError')}: ${String(e)}`);
    }
  }, [t]);

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
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: messages.length,
        thinking: null,
        imageAttachments: attachments ?? null,
      };
      setMessages((prev) => [...prev, optimisticMsg]);

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
      tokenCount: 0,
      createdAt: new Date().toISOString(),
      sortOrder: messages.length,
      thinking: null,
      imageAttachments: attachments ?? null,
    };
    setMessages((prev) => [...prev, optimisticMsg]);

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
      tokenCount: 0,
      createdAt: new Date().toISOString(),
      sortOrder: msgIndex,
      thinking: null,
      imageAttachments: null,
    };
    setMessages((prev) => [...prev, optimisticMsg]);

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

  const tokenUsage = lastUsage && contextWindow > 0
    ? {
        promptTokens: lastUsage.lastPromptTokens ?? lastUsage.promptTokens,
        totalTokens: lastUsage.totalTokens,
        contextWindow,
        completionTokens: lastUsage.completionTokens,
        thinkingTokens: lastUsage.thinkingTokens ?? 0,
      }
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
