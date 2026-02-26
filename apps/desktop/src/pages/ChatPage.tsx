import { useCallback, useState, useEffect } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { Settings, AlertTriangle, PanelLeftClose, PanelLeftOpen, Plus } from 'lucide-react';
import { motion } from 'framer-motion';
import { Logo } from '../components/Logo';
import { SourceSelector, SystemPromptEditor, ChatSidebar, ChatMessages, ChatInput } from '../components/chat';
import { useTranslation } from '../i18n';
import { EmptyState } from '../components/ui/EmptyState';
import { useChatSession } from '../lib/useChatSession';

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatPage() {
  const { t } = useTranslation();
  const { conversationId } = useParams<{ conversationId?: string }>();
  const navigate = useNavigate();

  const onConversationCreated = useCallback(
    (id: string) => navigate(`/chat/${id}`, { replace: true }),
    [navigate],
  );

  const chat = useChatSession({
    conversationId,
    onConversationCreated,
  });

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

  const handleNewConversation = useCallback(() => {
    navigate('/chat');
    chat.createNewConversation();
  }, [navigate, chat.createNewConversation]);

  const handleDeleteConversation = useCallback(
    async (id: string) => {
      await chat.deleteConversation(id);
      if (chat.activeId === id) {
        navigate('/chat');
      }
    },
    [chat.deleteConversation, chat.activeId, navigate],
  );

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
    <div className="flex h-full">
      {/* Sidebar */}
      <motion.div
        initial={false}
        animate={{ width: sidebarCollapsed ? 0 : 'clamp(200px, 20vw, 260px)' }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="shrink-0 overflow-hidden"
      >
        <div className="w-[clamp(200px,20vw,260px)] h-full">
          <ChatSidebar
            conversations={chat.conversations}
            activeId={chat.activeId}
            onSelect={handleSelectConversation}
            onNew={handleNewConversation}
            onDelete={handleDeleteConversation}
            onRename={chat.renameConversation}
          />
        </div>
      </motion.div>

      {/* Main chat area */}
      <div className="flex-1 flex flex-col min-w-0 relative">
        {/* Sidebar toggle */}
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
              <div className="shrink-0 border-b border-border px-4 py-2 flex items-center gap-2">
                <SourceSelector conversationId={chat.activeId} />
                <SystemPromptEditor
                  conversationId={chat.activeId}
                  systemPrompt={chat.customSystemPrompt}
                  onSaved={(newPrompt) => chat.setCustomSystemPrompt(newPrompt)}
                />
              </div>
            )}
            <ChatMessages
              messages={chat.messages}
              streamText={chat.streamText}
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
            />
            {chat.lastUsage && chat.contextWindow > 0 && (() => {
              const pct = (chat.lastUsage.promptTokens / chat.contextWindow) * 100;
              if (pct < 80) return null;
              const isRed = pct > 95;
              return (
                <div className={`mx-3 mb-2 px-3 py-2 rounded-lg text-xs flex items-center gap-2 ${
                  isRed ? 'bg-red-500/10 text-red-400 border border-red-500/20' : 'bg-yellow-500/10 text-yellow-500 border border-yellow-500/20'
                }`}>
                  <AlertTriangle size={14} />
                  <span className="flex-1">
                    {isRed
                      ? t('chat.contextNearlyFull', { percent: Math.round(pct) })
                      : t('chat.contextFillingUp', { percent: Math.round(pct) })
                    }
                  </span>
                  <button
                    type="button"
                    onClick={handleNewConversation}
                    className={`inline-flex items-center gap-1 px-2 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer shrink-0 ${
                      isRed
                        ? 'bg-red-500/20 text-red-300 hover:bg-red-500/30'
                        : 'bg-yellow-500/20 text-yellow-600 hover:bg-yellow-500/30'
                    }`}
                  >
                    <Plus size={12} />
                    {t('chat.startNewChat')}
                  </button>
                </div>
              );
            })()}
            <ChatInput
              onSend={chat.send}
              onStop={chat.stop}
              isStreaming={chat.isStreaming}
              disabled={!chat.agentConfig || chat.loadingMsgs}
              tokenUsage={chat.tokenUsage}
            />
          </>
        )}
      </div>
    </div>
  );
}

export default ChatPage;
