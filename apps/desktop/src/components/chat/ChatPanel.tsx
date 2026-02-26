import { useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { X, Plus, Settings, Maximize2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { ChatMessages } from './ChatMessages';
import { ChatInput } from './ChatInput';
import { EmptyState } from '../ui/EmptyState';
import { useChatSession } from '../../lib/useChatSession';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ChatPanelProps {
  /** Initial message to send when panel opens (e.g., search query) */
  initialMessage?: string;
  /** Called when user wants to close the panel */
  onClose: () => void;
  /** Additional class names */
  className?: string;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatPanel({ initialMessage, onClose, className }: ChatPanelProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const chat = useChatSession();

  // Track the last initialMessage we auto-sent, to avoid re-sending
  const sentInitialRef = useRef<string | null>(null);

  /* ── Auto-send initialMessage ───────────────────────────────────── */
  useEffect(() => {
    if (
      initialMessage &&
      initialMessage.trim() &&
      chat.agentConfig &&
      !chat.loadingConfig &&
      sentInitialRef.current !== initialMessage
    ) {
      sentInitialRef.current = initialMessage;
      chat.send(initialMessage);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialMessage, chat.agentConfig, chat.loadingConfig]);

  const handleNewChat = useCallback(() => {
    chat.createNewConversation();
    sentInitialRef.current = null;
  }, [chat.createNewConversation]);

  /* ── No provider configured ─────────────────────────────────────── */
  if (!chat.loadingConfig && !chat.agentConfig) {
    return (
      <div className={`flex flex-col h-full ${className ?? ''}`}>
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 bg-surface-2 border-b border-border">
          <span className="text-sm font-semibold text-text-primary">
            {t('chat.aiAssistant')}
          </span>
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-tertiary hover:bg-surface-3 hover:text-text-secondary transition-colors cursor-pointer"
            aria-label={t('chat.closePanel')}
          >
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 flex items-center justify-center p-4">
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
      </div>
    );
  }

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div className={`flex flex-col h-full ${className ?? ''}`}>
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-surface-2 border-b border-border">
        <span className="text-sm font-semibold text-text-primary">
          {t('chat.aiAssistant')}
        </span>
        <div className="flex items-center gap-1">
          <button
            onClick={handleNewChat}
            className="rounded-md px-2 py-1.5 text-xs font-medium text-text-tertiary hover:bg-surface-3 hover:text-text-secondary transition-colors cursor-pointer flex items-center gap-1"
            aria-label={t('chat.newChatShort')}
          >
            <Plus size={13} />
            {t('chat.newChatShort')}
          </button>
          {chat.activeId && (
            <button
              onClick={() => { navigate(`/chat/${chat.activeId}`); onClose(); }}
              className="rounded-md px-2 py-1.5 text-xs font-medium text-text-tertiary hover:bg-surface-3 hover:text-text-secondary transition-colors cursor-pointer flex items-center gap-1"
              title={t('chat.openFullChat')}
            >
              <Maximize2 size={13} />
            </button>
          )}
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-tertiary hover:bg-surface-3 hover:text-text-secondary transition-colors cursor-pointer"
            aria-label={t('chat.closePanel')}
          >
            <X size={16} />
          </button>
        </div>
      </div>

      {/* Messages area */}
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
      />

      {/* Input */}
      <ChatInput
        onSend={chat.send}
        onStop={chat.stop}
        isStreaming={chat.isStreaming}
        disabled={!chat.agentConfig || chat.loadingConfig}
      />
    </div>
  );
}
