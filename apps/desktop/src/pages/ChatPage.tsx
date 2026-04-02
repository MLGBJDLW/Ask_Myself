import { useCallback, useState, useEffect, useMemo, useRef } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { Settings, PanelLeftClose, PanelLeftOpen } from 'lucide-react';
import { motion } from 'framer-motion';
import { toast } from 'sonner';
import { Logo } from '../components/Logo';
import { SourceSelector, SystemPromptEditor, ChatSidebar, ChatMessages, ChatInput, ActiveExtensions, ContextCockpit, InvestigationHeader, TaskBoard } from '../components/chat';
import { useTranslation } from '../i18n';
import { EmptyState } from '../components/ui/EmptyState';
import { useChatSession } from '../lib/useChatSession';
import { undoableAction } from '../lib/undoToast';
import * as api from '../lib/api';
import type { AgentConfig, Conversation } from '../types/conversation';
import { extractChunkCitations } from '../lib/citationParser';

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatPage() {
  const { t } = useTranslation();
  const { conversationId } = useParams<{ conversationId?: string }>();
  const navigate = useNavigate();
  const location = useLocation();

  const onConversationCreated = useCallback(
    (id: string) => navigate(`/chat/${id}`, { replace: true }),
    [navigate],
  );

  const initialSourceIds = (
    (location.state as { sourceIds?: string[] } | null)?.sourceIds ?? []
  ).filter((value): value is string => typeof value === 'string' && value.length > 0);
  const initialCollectionContext = (
    (location.state as { collectionContext?: Conversation['collectionContext'] } | null)?.collectionContext
  ) ?? null;

  const chat = useChatSession({
    conversationId,
    onConversationCreated,
    systemPrompt: ((location.state as { systemPrompt?: string } | null)?.systemPrompt ?? '').trim(),
    initialSourceIds,
    initialCollectionContext,
  });

  const [agentConfigs, setAgentConfigs] = useState<AgentConfig[]>([]);
  useEffect(() => {
    api.listAgentConfigs().then(setAgentConfigs);
  }, []);
  const collectionContext = chat.activeConversation?.collectionContext ?? initialCollectionContext;

  const sentInitialRef = useRef<string | null>(null);
  const initialMessage = (
    (location.state as { initialMessage?: string } | null)?.initialMessage ?? ''
  ).trim();
  const initialSystemPrompt = (
    (location.state as { systemPrompt?: string } | null)?.systemPrompt ?? ''
  ).trim();
  const initialSourceScopeKey = initialSourceIds.join(',');
  const initialCollectionKey = collectionContext ? JSON.stringify(collectionContext) : '';

  // Accept one-off initial message forwarded from other pages.
  useEffect(() => {
    if (!initialMessage || chat.loadingConfig || !chat.agentConfig || chat.isStreaming) {
      return;
    }
    const key = `${location.key}:${initialMessage}:${initialSystemPrompt}:${initialSourceScopeKey}:${initialCollectionKey}`;
    if (sentInitialRef.current === key) {
      return;
    }
    sentInitialRef.current = key;
    void (async () => {
      if (conversationId && initialSourceIds.length > 0) {
        await api.setConversationSources(conversationId, initialSourceIds).catch(() => undefined);
      }
      if (conversationId && initialCollectionContext) {
        await api.updateConversationCollectionContext(conversationId, initialCollectionContext).catch(() => undefined);
      }
      await chat.send(initialMessage);
    })();

    const cleanPath = conversationId ? `/chat/${conversationId}` : '/chat';
    navigate(cleanPath, { replace: true, state: null });
  }, [
    initialMessage,
    initialSystemPrompt,
    initialCollectionContext,
    initialCollectionKey,
    initialSourceIds,
    initialSourceScopeKey,
    chat.loadingConfig,
    chat.agentConfig,
    chat.isStreaming,
    chat.send,
    location.key,
    conversationId,
    navigate,
  ]);

  /* ── Sidebar collapsed state ──────────────────────────────────────── */

  const SIDEBAR_STORAGE_KEY = 'chat-sidebar-collapsed';
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try { return localStorage.getItem(SIDEBAR_STORAGE_KEY) === 'true'; } catch { return false; }
  });

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((prev) => {
      const next = !prev;
      try { localStorage.setItem(SIDEBAR_STORAGE_KEY, String(next)); } catch { /* ignore */ }
      return next;
    });
  }, []);

  // Auto-collapse on narrow viewports
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)');
    const handler = (e: MediaQueryListEvent | MediaQueryList) => {
      if (e.matches) setSidebarCollapsed(true);
    };
    handler(mq);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Ctrl+B to toggle sidebar
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'b') {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleSidebar]);

  /* ── Handlers (navigation-aware wrappers) ───────────────────────── */

  const handleSelectConversation = useCallback(
    (id: string) => navigate(`/chat/${id}`),
    [navigate],
  );

  const handleNewConversation = useCallback(async () => {
    if (!chat.agentConfig) {
      navigate('/chat');
      chat.createNewConversation();
      return;
    }

    try {
      const conv = await api.createConversation(
        chat.agentConfig.provider,
        chat.agentConfig.model,
        chat.customSystemPrompt || undefined,
      );
      chat.setConversations((prev) => [conv, ...prev.filter((c) => c.id !== conv.id)]);
      navigate(`/chat/${conv.id}`);
    } catch (e) {
      toast.error(`${t('chat.createError')}: ${String(e)}`);
      navigate('/chat');
      chat.createNewConversation();
    }
  }, [
    chat.agentConfig,
    chat.customSystemPrompt,
    chat.setConversations,
    chat.createNewConversation,
    navigate,
    t,
  ]);

  const handleDeleteConversation = useCallback(
    (id: string) => {
      const prev = chat.conversations;
      const removed = prev.find((c) => c.id === id);
      chat.setConversations(prev.filter((c) => c.id !== id));
      if (chat.activeId === id) navigate('/chat');
      undoableAction({
        message: t('chat.conversation.deleted'),
        undoLabel: t('common.undo'),
        onConfirm: async () => {
          try {
            await api.deleteConversation(id);
          } catch (e) {
            toast.error(`${t('chat.deleteError')}: ${String(e)}`);
            if (removed) chat.setConversations((c) => [...c, removed]);
          }
        },
      });
      return () => { if (removed) chat.setConversations((c) => [...c, removed]); };
    },
    [chat.conversations, chat.setConversations, chat.activeId, navigate, t],
  );

  const handleDeleteBatch = useCallback(
    (ids: string[]) => {
      const prev = chat.conversations;
      const idSet = new Set(ids);
      const removed = prev.filter((c) => idSet.has(c.id));
      chat.setConversations(prev.filter((c) => !idSet.has(c.id)));
      if (chat.activeId && idSet.has(chat.activeId)) navigate('/chat');
      undoableAction({
        message: t('chat.conversation.deleted'),
        undoLabel: t('common.undo'),
        onConfirm: async () => {
          try {
            await api.deleteConversationsBatch(ids);
          } catch (e) {
            toast.error(`${t('chat.deleteError')}: ${String(e)}`);
            chat.setConversations((c) => [...c, ...removed]);
          }
        },
      });
    },
    [chat.conversations, chat.setConversations, chat.activeId, navigate, t],
  );

  const handleDeleteAll = useCallback(() => {
    const prev = chat.conversations;
    chat.setConversations([]);
    navigate('/chat');
    undoableAction({
      message: t('chat.conversation.deleted'),
      undoLabel: t('common.undo'),
      onConfirm: async () => {
        try {
          await api.deleteAllConversations();
        } catch (e) {
          toast.error(`${t('chat.deleteError')}: ${String(e)}`);
          chat.setConversations(prev);
        }
      },
    });
  }, [chat.conversations, chat.setConversations, navigate, t]);

  /* ── Suggestion prefill ─────────────────────────────────────────── */

  const [prefillText, setPrefillText] = useState<string>('');
  const prefillKey = useRef(0);
  const [sourceSummary, setSourceSummary] = useState({ selectedCount: 0, totalCount: 0, loading: true });
  const handleSuggestionClick = useCallback((text: string) => {
    prefillKey.current += 1;
    setPrefillText(text);
  }, []);

  const handleCompactConversation = useCallback(async () => {
    if (!chat.activeId) return;
    try {
      await api.compactConversation(chat.activeId);
      await chat.reloadMessages();
    } catch (e) {
      toast.error(String(e));
    }
  }, [chat.activeId, chat.reloadMessages]);

  const latestTurn = useMemo(
    () => (chat.turns.length > 0 ? chat.turns[chat.turns.length - 1] : null),
    [chat.turns],
  );

  const latestAnswerEvidence = useMemo(() => {
    const latestAssistant = [...chat.messages]
      .reverse()
      .find((message) => message.role === 'assistant' && message.content.trim().length > 0);

    if (!latestAssistant) {
      return { level: 'none' as const, count: 0 };
    }

    const chunkCitationCount = extractChunkCitations(latestAssistant.content).length;
    const documentCitationCount = (latestAssistant.content.match(/\[(doc|file|url):/g) ?? []).length;
    const totalCitations = chunkCitationCount + documentCitationCount;

    if (totalCitations >= 3) {
      return { level: 'high' as const, count: totalCitations };
    }
    if (totalCitations >= 1) {
      return { level: 'medium' as const, count: totalCitations };
    }
    return { level: 'low' as const, count: 0 };
  }, [chat.messages]);

  /* ── No provider configured ─────────────────────────────────────── */
  if (!chat.loadingConfig && !chat.agentConfig) {
    return (
      <div className="flex items-center justify-center h-full">
        <EmptyState
          icon={<><Logo size={48} className="mx-auto mb-2" /><Settings className="h-8 w-8" /></>}
          title={t('chat.noProvider')}
          description={t('chat.noProviderDesc')}
          action={{
            label: t('chat.configureProvider'),
            onClick: () => navigate('/settings'),
          }}
        />
      </div>
    );
  }

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div className="flex h-full min-h-0">
      {/* Sidebar */}
      <motion.div
        initial={false}
        animate={{ width: sidebarCollapsed ? 0 : 'clamp(200px, 20vw, 260px)' }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="shrink-0 overflow-hidden h-full min-h-0"
      >
        <div className="w-[clamp(200px,20vw,260px)] h-full min-h-0">
          <ChatSidebar
            conversations={chat.conversations}
            activeId={chat.activeId}
            onSelect={handleSelectConversation}
            onNew={handleNewConversation}
            onDelete={handleDeleteConversation}
            onRename={chat.renameConversation}
            onDeleteBatch={handleDeleteBatch}
            onDeleteAll={handleDeleteAll}
          />
        </div>
      </motion.div>

      {/* Main chat area */}
      <div className="flex-1 flex flex-col min-w-0 min-h-0 relative">
        {!chat.activeId && (
          <div className="absolute top-2 left-2 z-20">
            <button
              type="button"
              onClick={toggleSidebar}
              className="p-1.5 rounded-md bg-surface-2/80 backdrop-blur border border-border/50
                text-text-tertiary hover:text-text-primary hover:bg-surface-3
                transition-colors cursor-pointer"
              title={t('chat.toggleSidebar')}
              aria-label={t('chat.toggleSidebar')}
            >
              {sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
            </button>
          </div>
        )}
        {!chat.activeId && !chat.isStreaming ? (
          <div className="flex-1 flex items-center justify-center">
            <EmptyState
              icon={<Logo size={64} />}
              title={t('chat.noConversations')}
              description={t('chat.noConversationsDesc')}
              action={{
                label: t('chat.newChat'),
                onClick: handleNewConversation,
              }}
            />
          </div>
        ) : (
          <>
            {chat.activeId && (
              <div className="sticky top-0 z-10 shrink-0 border-b border-border bg-surface-1/90 backdrop-blur px-3 py-2">
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={toggleSidebar}
                    className="p-1.5 rounded-md bg-surface-2/80 border border-border/50
                      text-text-tertiary hover:text-text-primary hover:bg-surface-3
                      transition-colors cursor-pointer"
                    title={t('chat.toggleSidebar')}
                    aria-label={t('chat.toggleSidebar')}
                  >
                    {sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
                  </button>
                  {chat.agentConfig && agentConfigs.length > 0 && (
                    <div className="relative">
                      <select
                        className="text-[10px] text-text-tertiary bg-surface-3 pl-1.5 pr-4 py-0.5 rounded-md cursor-pointer border-none outline-none max-w-[200px] truncate appearance-none"
                        value={chat.agentConfig.id}
                        onChange={async (e) => {
                          const selected = agentConfigs.find(c => c.id === e.target.value);
                          if (selected) await chat.switchAgentConfig(selected);
                        }}
                        title={`${chat.agentConfig.provider} / ${chat.agentConfig.model}`}
                      >
                        {agentConfigs.map(c => (
                          <option key={c.id} value={c.id}>
                            {c.name || `${c.provider}/${c.model}`}
                          </option>
                        ))}
                      </select>
                      <span className="pointer-events-none absolute right-1 top-1/2 -translate-y-1/2 text-[8px] text-text-tertiary">▾</span>
                    </div>
                  )}
                  <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                    <SourceSelector conversationId={chat.activeId} onStateChange={setSourceSummary} />
                    <SystemPromptEditor
                      conversationId={chat.activeId}
                      systemPrompt={chat.customSystemPrompt}
                      onSaved={(newPrompt) => chat.setCustomSystemPrompt(newPrompt)}
                    />
                    <ActiveExtensions conversationId={chat.activeId ?? undefined} />
                  </div>
                </div>
              </div>
            )}
            {chat.activeId && (
              <InvestigationHeader
                conversationTitle={chat.activeConversation?.title ?? null}
                collectionContext={collectionContext}
                sourceSummary={sourceSummary}
                isStreaming={chat.isStreaming}
                routeKind={latestTurn?.routeKind ?? null}
                turnStatus={latestTurn?.status ?? null}
                evidenceLevel={latestAnswerEvidence.level}
                evidenceCount={latestAnswerEvidence.count}
              />
            )}
            {chat.activeId && (
              <ContextCockpit
                sourceSummary={sourceSummary}
                tokenUsage={chat.tokenUsage}
                finishReason={chat.finishReason}
                contextOverflow={chat.contextOverflow}
                rateLimited={chat.rateLimited}
                lastCached={chat.lastCached}
                isStreaming={chat.isStreaming}
                onCompact={handleCompactConversation}
                onStartNewChat={handleNewConversation}
              />
            )}
            {chat.activeId && (
              <TaskBoard
                messages={chat.messages}
                toolCalls={chat.toolCalls}
              />
            )}
            <ChatMessages
              messages={chat.messages}
              turns={chat.turns}
              streamText={chat.streamText}
              streamRounds={chat.streamRounds}
              traceEvents={chat.traceEvents}
              thinkingText={chat.thinkingText}
              isThinking={chat.isThinking}
              toolCalls={chat.toolCalls}
              isStreaming={chat.isStreaming}
              error={chat.error}
              onRetry={chat.retry}
              onDismissError={() => chat.clearError()}
              onDeleteMessage={chat.deleteMessage}
              onEditAndResend={chat.editAndResend}
              loadingMsgs={chat.loadingMsgs}
              lastCached={chat.lastCached}
              onSuggestionClick={handleSuggestionClick}
            />
            <ChatInput
              onSend={chat.send}
              onStop={chat.stop}
              isStreaming={chat.isStreaming}
              disabled={!chat.agentConfig || chat.loadingMsgs}
              conversationId={chat.activeId ?? undefined}
              prefillText={prefillText}
              onRestoreCheckpoint={chat.activeId ? async () => {
                await chat.reloadMessages();
              } : undefined}
            />
          </>
        )}
      </div>
    </div>
  );
}

export default ChatPage;
