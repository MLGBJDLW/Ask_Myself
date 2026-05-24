import { useCallback, useState, useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { Network, Settings, PanelLeftClose, PanelLeftOpen, UserRound, X } from 'lucide-react';
import { motion } from 'framer-motion';
import { toast } from 'sonner';
import { Logo } from '../components/Logo';
import { SourceSelector, SystemPromptEditor, ChatSidebar, ChatInput, ActiveExtensions, ChatRunOverview, TaskBoard } from '../components/chat';
import { ApprovalDialog } from '../components/chat/ApprovalDialog';
import { ChatMessages } from '../features/chat';
import { useApprovalQueue } from '../lib/useApprovalQueue';
import { useTranslation } from '../i18n';
import { EmptyState } from '../components/ui/EmptyState';
import { Button } from '../components/ui/Button';
import { useChatSession } from '../lib/useChatSession';
import { useResizablePanel } from '../lib/useResizablePanel';
import { undoableAction } from '../lib/undoToast';
import * as api from '../lib/api';
import type { AgentConfig, Conversation, ImageAttachment } from '../types/conversation';
import { formatUserError } from '../lib/userError';
import {
  GRAPH_AGENT_CONTEXT_EVENT,
  buildGraphCollectionContext,
  clearGraphAgentContext,
  readGraphAgentContext,
  type GraphAgentContext,
} from '../lib/knowledgeGraphAgent';

function personaExists(personas: api.PersonaProfile[], id: string): boolean {
  return personas.some((persona) => persona.id === id && persona.enabled !== false);
}

function suggestPersonaId(message: string, personas: api.PersonaProfile[]): string | null {
  const text = message.toLowerCase();
  const matches = (terms: string[]) => terms.some((term) => text.includes(term.toLowerCase()));

  if (
    personaExists(personas, 'speaker') &&
    matches(['ppt', 'pptx', 'powerpoint', 'slide', 'slides', 'deck', 'presentation', 'pitch', '演讲', '讲稿', '幻灯', '汇报', '路演'])
  ) {
    return 'speaker';
  }
  if (
    personaExists(personas, 'researcher') &&
    matches(['research', 'investigate', 'citation', 'citations', 'evidence', 'source', 'sources', 'paper', '调研', '研究', '证据', '引用', '论文', '深入了解'])
  ) {
    return 'researcher';
  }
  if (
    personaExists(personas, 'editor') &&
    matches(['edit', 'rewrite', 'polish', 'proofread', 'copyedit', '润色', '改写', '校对', '编辑', '修改文案'])
  ) {
    return 'editor';
  }
  if (
    personaExists(personas, 'novelist') &&
    matches(['novel', 'fiction', 'story', 'scene', 'character', 'plot', '小说', '故事', '角色', '剧情', '场景'])
  ) {
    return 'novelist';
  }

  return null;
}

const CHAT_SIDEBAR_WIDTH_KEY = 'chat-sidebar-width';
const CHAT_SIDEBAR_MIN_WIDTH = 200;
const CHAT_SIDEBAR_MAX_WIDTH = 420;

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

  // Source scope forwarded from route state, applied when the first send
  // auto-creates a conversation.
  const currentSourceIdsRef = useRef<string[]>(initialSourceIds);
  useEffect(() => {
    currentSourceIdsRef.current = initialSourceIds;
  }, [initialSourceIds]);
  const getCurrentSourceScope = useCallback(
    () => currentSourceIdsRef.current,
    [],
  );
  const handleSourceSelectionChange = useCallback((ids: string[]) => {
    currentSourceIdsRef.current = ids;
  }, []);

  const [activePersonaId, setActivePersonaId] = useState('default');
  const [personas, setPersonas] = useState<api.PersonaProfile[]>([]);
  useEffect(() => {
    api.listPersonas()
      .then((items) => setPersonas(Array.isArray(items) ? items : []))
      .catch(() => setPersonas([]));
  }, []);
  const chat = useChatSession({
    conversationId,
    onConversationCreated,
    systemPrompt: ((location.state as { systemPrompt?: string } | null)?.systemPrompt ?? '').trim(),
    initialSourceIds,
    getCurrentSourceScope,
    initialCollectionContext,
    activePersonaId,
  });
  const [pendingGraphContext, setPendingGraphContext] = useState<GraphAgentContext | null>(
    () => readGraphAgentContext(),
  );

  useEffect(() => {
    const syncGraphContext = () => setPendingGraphContext(readGraphAgentContext());
    window.addEventListener(GRAPH_AGENT_CONTEXT_EVENT, syncGraphContext as EventListener);
    window.addEventListener('storage', syncGraphContext);
    return () => {
      window.removeEventListener(GRAPH_AGENT_CONTEXT_EVENT, syncGraphContext as EventListener);
      window.removeEventListener('storage', syncGraphContext);
    };
  }, []);

  useEffect(() => {
    if (!chat.activeId) return;
    const next = chat.activeConversation?.personaId || 'default';
    setActivePersonaId((current) => (current === next ? current : next));
  }, [chat.activeId, chat.activeConversation?.personaId]);

  const setPersona = useCallback((id: string) => {
    setActivePersonaId(id);
    if (chat.activeId) {
      void api.updateConversationPersona(chat.activeId, id)
        .then((updated) => {
          chat.setConversations((prev) =>
            prev.map((conv) => (conv.id === updated.id ? updated : conv)),
          );
        })
        .catch((error) => toast.error(formatUserError(t('settings.personas'), error)));
    }
  }, [chat.activeId, chat.setConversations, t]);

  const handleChatSend = useCallback(
    async (content: string, attachments?: ImageAttachment[]) => {
      const suggestedPersonaId =
        activePersonaId === 'default' ? suggestPersonaId(content, personas) : null;
      const personaForSend = suggestedPersonaId ?? activePersonaId;
      if (suggestedPersonaId && suggestedPersonaId !== activePersonaId) {
        setPersona(suggestedPersonaId);
      }
      const graphContext = pendingGraphContext;
      if (graphContext?.sourceId) {
        currentSourceIdsRef.current = [graphContext.sourceId];
      }
      await chat.send(
        content,
        attachments,
        personaForSend,
        graphContext
          ? {
              collectionContext: buildGraphCollectionContext(graphContext),
              sourceIds: graphContext.sourceId ? [graphContext.sourceId] : [],
              userArtifacts: {
                kind: 'graphAgentContext',
                graphContext,
              },
            }
          : undefined,
      );
      if (graphContext) {
        clearGraphAgentContext();
        setPendingGraphContext(null);
      }
    },
    [activePersonaId, chat.send, pendingGraphContext, personas, setPersona],
  );

  const handleClearGraphContext = useCallback(() => {
    clearGraphAgentContext();
    setPendingGraphContext(null);
  }, []);

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
  const {
    size: chatSidebarWidth,
    setSize: setChatSidebarWidth,
    startResize: startChatSidebarResize,
    isResizing: isChatSidebarResizing,
  } = useResizablePanel({
    storageKey: CHAT_SIDEBAR_WIDTH_KEY,
    defaultSize: 240,
    minSize: CHAT_SIDEBAR_MIN_WIDTH,
    maxSize: CHAT_SIDEBAR_MAX_WIDTH,
  });

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((prev) => {
      const next = !prev;
      try { localStorage.setItem(SIDEBAR_STORAGE_KEY, String(next)); } catch { /* ignore */ }
      return next;
    });
  }, []);

  const handleChatSidebarResizeKey = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      setChatSidebarWidth(chatSidebarWidth - 12);
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      setChatSidebarWidth(chatSidebarWidth + 12);
    } else if (event.key === 'Home') {
      event.preventDefault();
      setChatSidebarWidth(CHAT_SIDEBAR_MIN_WIDTH);
    } else if (event.key === 'End') {
      event.preventDefault();
      setChatSidebarWidth(CHAT_SIDEBAR_MAX_WIDTH);
    }
  }, [chatSidebarWidth, setChatSidebarWidth]);

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

  const handleNewConversation = useCallback(async (projectId?: string | null) => {
    // Defensive guard: if a React SyntheticEvent / DOM node leaks in as projectId
    // (e.g. onClick={handler} passes MouseEvent), drop it to avoid circular JSON.
    if (projectId != null && typeof projectId !== 'string') {
      if (import.meta.env.DEV) {
        // eslint-disable-next-line no-console
        console.warn('[ChatPage] handleNewConversation received non-string projectId; ignoring.', projectId);
      }
      projectId = null;
    }
    if (!chat.agentConfig) {
      navigate('/chat');
      chat.createNewConversation();
      return;
    }

    try {
      // If creating within a project, fetch project's system prompt as default
      let systemPrompt = chat.customSystemPrompt || undefined;
      if (projectId && !chat.customSystemPrompt) {
        try {
          const proj = await api.getProject(projectId);
          if (proj.systemPrompt) systemPrompt = proj.systemPrompt;
        } catch { /* ignore, use default */ }
      }

      const conv = await api.createConversation(
        chat.agentConfig.provider,
        chat.agentConfig.model,
        systemPrompt,
        projectId ?? undefined,
        'default',
      );
      chat.setConversations((prev) => [conv, ...prev.filter((c) => c.id !== conv.id)]);
      navigate(`/chat/${conv.id}`);
    } catch (e) {
      toast.error(formatUserError(t('chat.createError'), e));
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

  const handleCheckpointBranch = useCallback((conversation: Conversation) => {
    chat.setConversations((prev) => [conversation, ...prev.filter((c) => c.id !== conversation.id)]);
    navigate(`/chat/${conversation.id}`);
  }, [chat.setConversations, navigate]);

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
            toast.error(formatUserError(t('chat.deleteError'), e));
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
            toast.error(formatUserError(t('chat.deleteError'), e));
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
          toast.error(formatUserError(t('chat.deleteError'), e));
          chat.setConversations(prev);
        }
      },
    });
  }, [chat.conversations, chat.setConversations, navigate, t]);

  /* ── Suggestion prefill ─────────────────────────────────────────── */

  const [prefillText, setPrefillText] = useState<string>('');
  const prefillKey = useRef(0);
  const handleSuggestionClick = useCallback((text: string) => {
    prefillKey.current += 1;
    setPrefillText(text);
  }, []);

  const [isCompacting, setIsCompacting] = useState(false);
  const handleCompactConversation = useCallback(async () => {
    if (!chat.activeId) return;
    if (isCompacting) return;
    setIsCompacting(true);
    try {
      await api.compactConversation(chat.activeId);
      await chat.reloadMessages({ resetUsage: true });
    } catch (e) {
      toast.error(formatUserError(t('chat.compact'), e));
    } finally {
      setIsCompacting(false);
    }
  }, [chat.activeId, chat.reloadMessages, isCompacting, t]);

  const pendingChatAction = (
    location.state as { pendingChatAction?: string } | null
  )?.pendingChatAction;

  useEffect(() => {
    if (pendingChatAction !== 'compact') return;
    if (!chat.activeId || chat.loadingMsgs || isCompacting) return;

    navigate(location.pathname, { replace: true, state: null });
    void handleCompactConversation();
  }, [
    chat.activeId,
    chat.loadingMsgs,
    handleCompactConversation,
    isCompacting,
    location.pathname,
    navigate,
    pendingChatAction,
  ]);

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
        animate={{ width: sidebarCollapsed ? 0 : chatSidebarWidth }}
        transition={isChatSidebarResizing ? { duration: 0 } : { duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="relative shrink-0 overflow-hidden h-full min-h-0"
      >
        <div className="h-full min-h-0" style={{ width: chatSidebarWidth }}>
          <ChatSidebar
            conversations={chat.conversations}
            activeId={chat.activeId}
            onSelect={handleSelectConversation}
            onNew={handleNewConversation}
            onDelete={handleDeleteConversation}
            onRename={chat.renameConversation}
            onDeleteBatch={handleDeleteBatch}
            onDeleteAll={handleDeleteAll}
            onConversationMoved={chat.loadConversations}
          />
        </div>
        {!sidebarCollapsed && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-valuemin={CHAT_SIDEBAR_MIN_WIDTH}
            aria-valuemax={CHAT_SIDEBAR_MAX_WIDTH}
            aria-valuenow={chatSidebarWidth}
            tabIndex={0}
            onPointerDown={startChatSidebarResize}
            onKeyDown={handleChatSidebarResizeKey}
            className="absolute right-0 top-0 h-full w-2 translate-x-1 cursor-col-resize touch-none
              bg-transparent outline-none transition-colors hover:bg-accent/25 focus-visible:bg-accent/35"
            title={t('nav.resizeSidebar')}
          />
        )}
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
        {!chat.activeId && !chat.isStreaming && !pendingGraphContext ? (
          <div className="flex-1 flex items-center justify-center">
            <EmptyState
              icon={<Logo size={64} />}
              title={t('chat.noConversations')}
              description={t('chat.noConversationsDesc')}
              action={{
                label: t('chat.newChat'),
                onClick: () => handleNewConversation(),
              }}
            />
          </div>
        ) : (
          <>
            {chat.activeId && (
              <div className="sticky top-0 z-10 shrink-0 border-b border-border/60 bg-surface-1/80 px-3 py-1.5 backdrop-blur">
                <div className="flex min-h-8 items-center gap-1.5">
                  <button
                    type="button"
                    onClick={toggleSidebar}
                    className="flex h-7 w-7 items-center justify-center rounded-md border border-border/50 bg-surface-2/70
                      text-text-tertiary hover:text-text-primary hover:bg-surface-3
                      transition-colors cursor-pointer"
                    title={t('chat.toggleSidebar')}
                    aria-label={t('chat.toggleSidebar')}
                  >
                    {sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
                  </button>
                  {chat.agentConfig && agentConfigs.length > 0 && (
                    <div className="relative min-w-[140px]">
                      <select
                        className="h-7 w-full max-w-[210px] cursor-pointer appearance-none rounded-md border border-border/70 bg-surface-2/80 pl-2 pr-6 text-xs text-text-secondary outline-none transition-colors hover:border-border-hover focus:border-accent"
                        value={chat.agentConfig.id}
                        aria-label={t('settings.defaultModel')}
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
                      <span className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-text-tertiary">▾</span>
                    </div>
                  )}
                  {personas.length > 0 && (
                    <div className="relative min-w-[132px]">
                      <UserRound className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text-tertiary" />
                      <select
                        className="h-7 w-full max-w-[178px] cursor-pointer appearance-none rounded-md border border-border/70 bg-surface-2/80 pl-7 pr-6 text-xs text-text-secondary outline-none transition-colors hover:border-border-hover focus:border-accent"
                        value={activePersonaId}
                        aria-label={t('settings.personas')}
                        onChange={(e) => setPersona(e.target.value)}
                        title={`Persona: ${personas.find((p) => p.id === activePersonaId)?.name ?? activePersonaId}`}
                      >
                        {personas.map((persona) => (
                          <option key={persona.id} value={persona.id}>
                            {persona.name}
                          </option>
                        ))}
                      </select>
                      <span className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-text-tertiary">▾</span>
                    </div>
                  )}
                  <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                    <SourceSelector
                      conversationId={chat.activeId}
                      initialSelectedIds={initialSourceIds}
                      onSelectionChange={handleSourceSelectionChange}
                    />
                    <SystemPromptEditor
                      conversationId={chat.activeId}
                      systemPrompt={chat.customSystemPrompt}
                      onSaved={(newPrompt) => chat.setCustomSystemPrompt(newPrompt)}
                    />
                    <ActiveExtensions
                      conversationId={chat.activeId ?? undefined}
                    />
                  </div>
                </div>
              </div>
            )}
            {chat.activeId && (
              <ChatRunOverview
                isStreaming={chat.isStreaming}
                tokenUsage={chat.tokenUsage}
                runtimeProfile={chat.runtimeProfile}
                finishReason={chat.finishReason}
                contextOverflow={chat.contextOverflow}
                isCompacting={isCompacting}
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
              taskRun={chat.taskRun}
              isStreaming={chat.isStreaming}
              error={chat.error}
              onRetry={chat.retry}
              onDismissError={() => chat.clearError()}
              onDeleteMessage={chat.deleteMessage}
              onEditAndResend={chat.editAndResend}
              loadingMsgs={chat.loadingMsgs}
              lastCached={chat.lastCached}
              onSuggestionClick={handleSuggestionClick}
              isCompacting={isCompacting}
            />
            <TaskBoard
              messages={chat.messages}
              toolCalls={chat.toolCalls}
              taskRun={chat.taskRun}
            />
            {pendingGraphContext && (
              <div className="mx-4 mb-2 rounded-md border border-accent/25 bg-accent/10 px-3 py-2">
                <div className="flex min-w-0 items-center gap-2">
                  <Network className="h-4 w-4 shrink-0 text-accent" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-medium text-text-primary">
                      {t('chat.graphContextFromKnowledge', { name: pendingGraphContext.node.label })}
                    </div>
                    <div className="mt-0.5 truncate text-[11px] text-text-tertiary">
                      {[
                        pendingGraphContext.sourceLabel,
                        pendingGraphContext.pathPrefix,
                        t('chat.graphContextStats', {
                          nodes: String(new Set([
                            pendingGraphContext.node.id,
                            ...pendingGraphContext.edges.flatMap((edge) => [edge.source, edge.target]),
                          ]).size),
                          documents: String(pendingGraphContext.documents.length),
                          saved: String(pendingGraphContext.tokenEstimate.savedPctEstimate),
                        }),
                      ].filter(Boolean).join(' · ')}
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    iconOnly
                    icon={<X size={14} />}
                    aria-label={t('chat.removeGraphContext')}
                    title={t('chat.removeGraphContext')}
                    onClick={handleClearGraphContext}
                  />
                </div>
              </div>
            )}
            <ChatInput
              onSend={handleChatSend}
              onStop={chat.stop}
              isStreaming={chat.isStreaming}
              disabled={!chat.agentConfig || chat.loadingMsgs || isCompacting}
              conversationId={chat.activeId ?? undefined}
              prefillText={prefillText}
              onCompact={chat.activeId ? handleCompactConversation : undefined}
              isCompacting={isCompacting}
              onRestoreCheckpoint={chat.activeId ? async () => {
                await chat.reloadMessages();
              } : undefined}
              onBranchCheckpoint={handleCheckpointBranch}
            />
            {chat.activeId && <ApprovalDialogMount conversationId={chat.activeId} />}
          </>
        )}
      </div>
    </div>
  );
}

export default ChatPage;

/**
 * Small wrapper that subscribes to the approval queue for the active
 * conversation and renders the modal dialog for the head request.
 */
function ApprovalDialogMount({ conversationId }: { conversationId: string }) {
  const { current, onResolved } = useApprovalQueue(conversationId);
  return <ApprovalDialog request={current} onResolved={onResolved} />;
}
