import { useState, useEffect, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { Settings, MessageCircle } from 'lucide-react';
import { SourceSelector } from '../components/chat/SourceSelector';
import { SystemPromptEditor } from '../components/chat/SystemPromptEditor';
import { toast } from 'sonner';
import * as api from '../lib/api';
import { useAgentStream } from '../lib/useAgentStream';
import { useTranslation } from '../i18n';
import { EmptyState } from '../components/ui/EmptyState';
import { ChatSidebar } from '../components/chat/ChatSidebar';
import { ChatMessages } from '../components/chat/ChatMessages';
import { ChatInput } from '../components/chat/ChatInput';
import type { Conversation, ConversationMessage, AgentConfig } from '../types/conversation';

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
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatPage() {
  const { t } = useTranslation();
  const { conversationId } = useParams<{ conversationId?: string }>();
  const navigate = useNavigate();

  /* ── State ──────────────────────────────────────────────────────── */
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [defaultConfig, setDefaultConfig] = useState<AgentConfig | null>(null);
  const [customSystemPrompt, setCustomSystemPrompt] = useState<string>('');
  const [loadingConvos, setLoadingConvos] = useState(true);
  const [loadingMsgs, setLoadingMsgs] = useState(false);

  const { send, stop, isStreaming, streamText, toolCalls, error, reset } = useAgentStream();

  const activeId = conversationId ?? null;

  /* ── Load conversations ─────────────────────────────────────────── */
  const loadConversations = useCallback(async () => {
    try {
      const list = await api.listConversations();
      // Sort by updatedAt desc
      list.sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime());
      setConversations(list);
    } catch (e) {
      toast.error(`${t('chat.loadError')}: ${String(e)}`);
    } finally {
      setLoadingConvos(false);
    }
  }, []);

  /* ── Load default agent config ──────────────────────────────────── */
  const loadDefaultConfig = useCallback(async () => {
    try {
      const configs = await api.listAgentConfigs();
      const def = configs.find((c) => c.isDefault) ?? configs[0] ?? null;
      setDefaultConfig(def);
    } catch {
      setDefaultConfig(null);
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
    if (!isStreaming && activeId && messages.length > 0) {
      // Re-fetch messages after agent is done
      api.getConversation(activeId).then(([, msgs]) => {
        setMessages(msgs);
      }).catch(() => {});
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
                setConversations((prev) =>
                  prev.map((c) => (c.id === activeId ? { ...c, title } : c)),
                );
              })
              .catch(() => {});
          }
        }
      }
    }
    // Only trigger on isStreaming becoming false
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isStreaming]);

  /* ── Show error toast ───────────────────────────────────────────── */
  useEffect(() => {
    if (error) toast.error(error);
  }, [error]);

  /* ── Handlers ───────────────────────────────────────────────────── */

  const handleSelectConversation = useCallback(
    (id: string) => {
      navigate(`/chat/${id}`);
    },
    [navigate],
  );

  const handleNewConversation = useCallback(() => {
    navigate('/chat');
    setMessages([]);
    setCustomSystemPrompt('');
    reset();
  }, [navigate, reset]);

  const handleDeleteConversation = useCallback(
    async (id: string) => {
      try {
        await api.deleteConversation(id);
        setConversations((prev) => prev.filter((c) => c.id !== id));
        if (activeId === id) {
          navigate('/chat');
          setMessages([]);
        }
      } catch (e) {
        toast.error(`${t('chat.deleteError')}: ${String(e)}`);
      }
    },
    [activeId, navigate],
  );

  const handleRenameConversation = useCallback(
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
    [],
  );

  const handleSendMessage = useCallback(
    async (message: string) => {
      if (!defaultConfig) return;

      let convId = activeId;

      // Auto-create conversation if none active
      if (!convId) {
        try {
          const conv = await api.createConversation(
            defaultConfig.provider,
            defaultConfig.model,
            customSystemPrompt || undefined,
          );
          convId = conv.id;
          setConversations((prev) => [conv, ...prev]);
          navigate(`/chat/${conv.id}`, { replace: true });
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
        content: message,
        toolCallId: null,
        toolCalls: [],
        tokenCount: 0,
        createdAt: new Date().toISOString(),
        sortOrder: messages.length,
      };
      setMessages((prev) => [...prev, optimisticMsg]);

      await send(convId, message);
    },
    [activeId, defaultConfig, customSystemPrompt, messages.length, navigate, send],
  );

  const handleStop = useCallback(() => {
    if (activeId) {
      stop(activeId);
    }
  }, [activeId, stop]);

  /* ── No provider configured ─────────────────────────────────────── */
  if (!loadingConvos && !defaultConfig) {
    return (
      <div className="flex items-center justify-center h-full">
        <EmptyState
          icon={<Settings className="h-8 w-8" />}
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
      <div className="w-[260px] shrink-0">
        <ChatSidebar
          conversations={conversations}
          activeId={activeId}
          onSelect={handleSelectConversation}
          onNew={handleNewConversation}
          onDelete={handleDeleteConversation}
          onRename={handleRenameConversation}
        />
      </div>

      {/* Main chat area */}
      <div className="flex-1 flex flex-col min-w-0">
        {!activeId && !isStreaming ? (
          <div className="flex-1 flex items-center justify-center">
            <EmptyState
              icon={<MessageCircle className="h-8 w-8" />}
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
            {activeId && (
              <div className="shrink-0 border-b border-border px-4 py-2 flex items-center gap-2">
                <SourceSelector conversationId={activeId} />
                <SystemPromptEditor
                  conversationId={activeId}
                  systemPrompt={customSystemPrompt}
                  onSaved={(newPrompt) => setCustomSystemPrompt(newPrompt)}
                />
              </div>
            )}
            <ChatMessages
              messages={messages}
              streamText={streamText}
              toolCalls={toolCalls}
              isStreaming={isStreaming}
            />
            <ChatInput
              onSend={handleSendMessage}
              onStop={handleStop}
              isStreaming={isStreaming}
              disabled={!defaultConfig || loadingMsgs}
            />
          </>
        )}
      </div>
    </div>
  );
}

export default ChatPage;
