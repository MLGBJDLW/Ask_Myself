import { useCallback, useState } from 'react';
import { Copy, Check, ThumbsUp, ThumbsDown, RotateCcw, Pencil, Trash2, Volume2, Loader2, Pause, Play, RefreshCw } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { useSpeechPlayback } from '../../features/voice/SpeechPlaybackProvider';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

type FeedbackState = 'up' | 'down' | null;

export interface MessageActionsProps {
  text: string;
  showFeedback: boolean;
  chunkIds?: string[];
  queryText?: string;
  /** Show retry button for assistant retry or user resend. */
  showRetry?: boolean;
  /** Called when retry is clicked */
  onRetry?: () => void;
  /** Whether the message is from user (enables edit button) */
  isUser?: boolean;
  /** Message id for edit/delete + message-level feedback */
  messageId?: string;
  /** Conversation id (required for message-level feedback) */
  conversationId?: string;
  /** Called when edit is clicked */
  onEdit?: () => void;
  /** Called when delete is confirmed */
  onDelete?: (messageId: string) => void;
  /** Align actions with the owning message. */
  align?: 'start' | 'end';
  /** Offer unified speech playback for a final assistant reply. */
  showSpeech?: boolean;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function MessageActions({ text, showFeedback, chunkIds = [], queryText = '', showRetry, onRetry, isUser, messageId, conversationId, onEdit, onDelete, align = 'start', showSpeech = false }: MessageActionsProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [feedback, setFeedback] = useState<FeedbackState>(null);
  const [submitting, setSubmitting] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const speech = useSpeechPlayback();
  const speechState = messageId && speech.state.status !== 'idle' && speech.state.messageId === messageId
    ? speech.state
    : null;

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Silently fail if clipboard access is denied
    }
  }, [text]);

  const handleFeedback = useCallback(async (type: 'up' | 'down') => {
    const clearing = feedback === type;
    const nextFeedback: FeedbackState = clearing ? null : type;
    const rating = clearing ? 0 : type === 'up' ? 1 : -1;

    // Always persist message-level signal (backend no-ops if messageId/conversationId missing).
    setSubmitting(true);
    try {
      const tasks: Promise<unknown>[] = [];
      if (messageId && conversationId) {
        tasks.push(api.setMessageFeedback(messageId, conversationId, rating));
      }
      // Preserve backward-compat chunk-level path when evidence is present.
      if (!clearing && chunkIds.length > 0 && queryText) {
        const action = type === 'up' ? 'upvote' : 'downvote';
        tasks.push(...chunkIds.map((id) => api.addFeedback(id, queryText, action)));
      }
      if (tasks.length > 0) {
        await Promise.allSettled(tasks);
      }
      setFeedback(nextFeedback);
    } catch {
      // Silently fail — feedback is best-effort
    } finally {
      setSubmitting(false);
    }
  }, [feedback, chunkIds, queryText, messageId, conversationId]);

  const handleDelete = useCallback(() => {
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      // Auto-reset after 3 seconds
      setTimeout(() => setConfirmingDelete(false), 3000);
      return;
    }
    if (messageId && onDelete) {
      onDelete(messageId);
    }
    setConfirmingDelete(false);
  }, [confirmingDelete, messageId, onDelete]);

  const actionBtn =
    'flex h-7 w-7 items-center justify-center rounded-md text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-primary disabled:pointer-events-none disabled:opacity-45';

  return (
    <div className={`mt-1 flex items-center gap-0.5 opacity-70 transition-opacity duration-150 group-hover:opacity-100 focus-within:opacity-100 ${align === 'end' ? 'justify-end' : 'justify-start'}`}>
      <button
        type="button"
        onClick={handleCopy}
        title={copied ? t('chat.copied') : t('chat.copyMessage')}
        aria-label={copied ? t('chat.copied') : t('chat.copyMessage')}
        className={actionBtn}
      >
        {copied ? (
          <Check className="h-3.5 w-3.5 text-success" />
        ) : (
          <Copy className="h-3.5 w-3.5" />
        )}
      </button>
      {showSpeech && messageId && text.trim() && (
        <button
          type="button"
          onClick={() => {
            if (speechState?.status === 'playing') speech.pause();
            else if (speechState?.status === 'paused') void speech.resume();
            else void speech.speakMessage(messageId, text);
          }}
          disabled={speechState?.status === 'synthesizing'}
          title={speechState?.status === 'playing'
            ? 'Pause speech'
            : speechState?.status === 'paused'
              ? 'Resume speech'
              : speechState?.status === 'error'
                ? `${speechState.error} Retry`
                : 'Read this reply'}
          aria-label={speechState?.status === 'playing' ? 'Pause speech' : speechState?.status === 'paused' ? 'Resume speech' : 'Read this reply'}
          className={`${actionBtn} ${speechState?.status === 'playing' ? 'text-accent' : ''} ${speechState?.status === 'error' ? 'text-danger' : ''}`}
        >
          {speechState?.status === 'synthesizing' ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
            : speechState?.status === 'playing' ? <Pause className="h-3.5 w-3.5" />
              : speechState?.status === 'paused' ? <Play className="h-3.5 w-3.5" />
                : speechState?.status === 'error' ? <RefreshCw className="h-3.5 w-3.5" />
                  : <Volume2 className="h-3.5 w-3.5" />}
        </button>
      )}
      {isUser && onEdit && (
        <button
          type="button"
          onClick={onEdit}
          title={t('chat.edit')}
          aria-label={t('chat.edit')}
          className={actionBtn}
        >
          <Pencil className="h-3.5 w-3.5" />
        </button>
      )}
      {showRetry && onRetry && (
        <button
          type="button"
          onClick={onRetry}
          title={t('chat.retry')}
          aria-label={t('chat.retry')}
          className={actionBtn}
        >
          <RotateCcw className="h-3.5 w-3.5" />
        </button>
      )}
      {showFeedback && (
        <>
          <button
            type="button"
            onClick={() => handleFeedback('up')}
            disabled={submitting}
            title={t('chat.feedbackGood')}
            aria-label={t('chat.feedbackGood')}
            aria-pressed={feedback === 'up'}
            className={`${actionBtn} ${feedback === 'up' ? 'text-success' : ''} ${submitting ? 'opacity-50 pointer-events-none' : ''}`}
          >
            <ThumbsUp className="h-3.5 w-3.5" fill={feedback === 'up' ? 'currentColor' : 'none'} />
          </button>
          <button
            type="button"
            onClick={() => handleFeedback('down')}
            disabled={submitting}
            title={t('chat.feedbackBad')}
            aria-label={t('chat.feedbackBad')}
            aria-pressed={feedback === 'down'}
            className={`${actionBtn} ${feedback === 'down' ? 'text-danger' : ''} ${submitting ? 'opacity-50 pointer-events-none' : ''}`}
          >
            <ThumbsDown className="h-3.5 w-3.5" fill={feedback === 'down' ? 'currentColor' : 'none'} />
          </button>
        </>
      )}
      {messageId && onDelete && (
        <button
          type="button"
          onClick={handleDelete}
          title={confirmingDelete ? t('chat.confirmDelete') : t('chat.delete')}
          aria-label={confirmingDelete ? t('chat.confirmDelete') : t('chat.delete')}
          className={`${actionBtn} ${confirmingDelete ? 'w-auto px-2 text-danger bg-danger/10' : ''}`}
        >
          {confirmingDelete ? (
            <span className="text-[10px] font-medium px-0.5">{t('chat.confirmDelete')}</span>
          ) : (
            <Trash2 className="h-3.5 w-3.5" />
          )}
        </button>
      )}
    </div>
  );
}
