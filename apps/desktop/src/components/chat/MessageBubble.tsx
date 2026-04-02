import React, { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { motion } from 'framer-motion';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { rehypePlugins } from './markdownComponents';
import { Check, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { markdownComponents, preprocessFilePaths, preprocessCitations, CitationContext } from './markdownComponents';
import { extractChunkCitations, preprocessChunkCitations, preprocessInlineCitations } from '../../lib/citationParser';
import type { CitationCardData } from '../../lib/citationParser';
import { MessageActions } from './MessageActions';
import { messageTimestamp } from '../../lib/relativeTime';
import type { ConversationMessage } from '../../types/conversation';
import { CitationChip } from './EvidenceCard';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

export interface MessageBubbleProps {
  msg: ConversationMessage;
  chunkIds?: string[];
  queryText?: string;
  /** Citation data lookup for rendering inline citations */
  citationLookup?: { getCard(chunkId: string): CitationCardData | undefined };
  /** Show retry button on this message */
  isLastAssistant?: boolean;
  /** Whether the last response came from cache */
  lastCached?: boolean;
  /** Called when retry is clicked */
  onRetry?: () => void;
  /** Always show timestamp (when gap > 5min) */
  alwaysShowTimestamp?: boolean;
  /** Called when a message is deleted */
  onDeleteMessage?: (messageId: string) => void;
  /** Called when a message is edited and re-sent */
  onEditAndResend?: (messageId: string, newContent: string) => void;
}

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

function MessageBubbleInner({ msg, chunkIds, queryText, citationLookup, isLastAssistant, lastCached, onRetry, alwaysShowTimestamp, onDeleteMessage, onEditAndResend }: MessageBubbleProps) {
  const { t } = useTranslation();
  const isUser = msg.role === 'user';
  const [isEditing, setIsEditing] = useState(false);
  const [editText, setEditText] = useState(msg.content);
  const editRef = useRef<HTMLTextAreaElement>(null);

  // Focus textarea when entering edit mode
  useEffect(() => {
    if (isEditing && editRef.current) {
      editRef.current.focus();
      editRef.current.setSelectionRange(editRef.current.value.length, editRef.current.value.length);
    }
  }, [isEditing]);

  const handleStartEdit = useCallback(() => {
    setEditText(msg.content);
    setIsEditing(true);
  }, [msg.content]);

  const handleCancelEdit = useCallback(() => {
    setIsEditing(false);
    setEditText(msg.content);
  }, [msg.content]);

  const handleSaveEdit = useCallback(() => {
    const trimmed = editText.trim();
    if (!trimmed || trimmed === msg.content) {
      handleCancelEdit();
      return;
    }
    onEditAndResend?.(msg.id, trimmed);
    setIsEditing(false);
  }, [editText, msg.content, msg.id, onEditAndResend, handleCancelEdit]);

  const handleEditKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSaveEdit();
    }
    if (e.key === 'Escape') {
      handleCancelEdit();
    }
  }, [handleSaveEdit, handleCancelEdit]);

  if (msg.role === 'tool' || msg.role === 'system') return null;

  const evidenceItems = useMemo(() => {
    if (isUser) return [];

    const parsed = extractChunkCitations(msg.content);
    const grouped = new Map<string, {
      chunkId: string;
      card?: CitationCardData;
      count: number;
      displayText?: string;
    }>();
    const seenChunks = new Set<string>();

    const addEvidence = (chunkId: string, displayText?: string) => {
      if (seenChunks.has(chunkId)) return;
      seenChunks.add(chunkId);

      const card = citationLookup?.getCard(chunkId);
      const groupKey =
        card?.documentPath?.trim()
        || card?.documentTitle?.trim()
        || chunkId;
      const existing = grouped.get(groupKey);
      if (existing) {
        existing.count += 1;
        if (!existing.card && card) existing.card = card;
        if (!existing.displayText && displayText) existing.displayText = displayText;
        return;
      }

      grouped.set(groupKey, {
        chunkId,
        card: card ?? undefined,
        count: 1,
        displayText,
      });
    };

    for (const entry of parsed) {
      addEvidence(entry.chunkId, entry.displayText);
    }

    for (const chunkId of chunkIds ?? []) {
      addEvidence(chunkId);
    }

    return Array.from(grouped.values())
      .sort((a, b) => {
        if (b.count !== a.count) return b.count - a.count;
        const aLabel = a.card?.documentTitle || a.card?.documentPath || a.displayText || a.chunkId;
        const bLabel = b.card?.documentTitle || b.card?.documentPath || b.displayText || b.chunkId;
        return aLabel.localeCompare(bLabel);
      })
      .map((item, index) => {
        const baseLabel =
          item.card?.documentTitle
          || (item.card?.documentPath ? basename(item.card.documentPath) : '')
          || item.displayText
          || t('chat.evidenceSourceLabel', { index: String(index + 1) });
        return {
          chunkId: item.chunkId,
          displayText: item.count > 1 ? `${baseLabel} ×${item.count}` : baseLabel,
        };
      });
  }, [chunkIds, citationLookup, isUser, msg.content, t]);

  const timestamp = messageTimestamp(msg.createdAt, t);
  const ariaLabel = isUser ? t('chat.userMessage') : t('chat.assistantResponse');

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className={`flex ${isUser ? 'justify-end' : 'justify-start'} mb-3`}
    >
      <div className="group flex flex-col max-w-[80%]">
        <div
          aria-label={ariaLabel}
          className={`relative rounded-lg px-3.5 py-2.5 text-sm leading-relaxed
            ${isUser
              ? 'bg-accent/20 text-text-primary'
              : 'bg-surface-2 text-text-primary'
            }`}
        >
          {!isEditing && (
            <MessageActions
              text={msg.content}
              showFeedback={!isUser}
              chunkIds={chunkIds}
              queryText={queryText}
              isLastAssistant={isLastAssistant}
              onRetry={onRetry}
              isUser={isUser}
              messageId={msg.id}
              onEdit={isUser && onEditAndResend ? handleStartEdit : undefined}
              onDelete={onDeleteMessage}
            />
          )}
          {msg.tokenCount > 0 && !isEditing && (
            <span
              className="absolute bottom-0.5 right-2 text-[9px] text-text-tertiary/0 group-hover:text-text-tertiary/60 transition-colors tabular-nums select-none"
              title={`${msg.tokenCount.toLocaleString()} tokens`}
            >
              {msg.tokenCount.toLocaleString()} {t('chat.tokensShort')}
            </span>
          )}
          {isLastAssistant && lastCached && !isEditing && (
            <span
              className="absolute top-1.5 right-2 rounded-full border border-border/50 bg-surface-1/70 px-1.5 py-0.5 text-[9px] uppercase tracking-[0.1em] text-text-tertiary/70 select-none"
              title={t('chat.cached')}
            >
              {t('chat.cached')}
            </span>
          )}
          {isEditing ? (
            <div>
              <textarea
                ref={editRef}
                value={editText}
                onChange={(e) => setEditText(e.target.value)}
                onKeyDown={handleEditKeyDown}
                aria-label={t('chat.editing')}
                className="w-full min-h-[60px] rounded-md border border-border bg-surface-0 px-2.5 py-2 text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-1 focus:ring-accent resize-y"
                rows={Math.min(editText.split('\n').length + 1, 8)}
              />
              <div className="flex items-center gap-1.5 mt-1.5">
                <button
                  type="button"
                  onClick={handleSaveEdit}
                  className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-accent text-on-accent hover:bg-accent/90 transition-colors cursor-pointer"
                >
                  <Check className="h-3 w-3" />
                  {t('chat.save')}
                </button>
                <button
                  type="button"
                  onClick={handleCancelEdit}
                  className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-surface-2 text-text-tertiary hover:text-text-primary transition-colors cursor-pointer"
                >
                  <X className="h-3 w-3" />
                  {t('chat.cancel')}
                </button>
              </div>
            </div>
          ) : isUser ? (
            <>
              <span className="whitespace-pre-wrap">{msg.content}</span>
              {msg.imageAttachments && msg.imageAttachments.length > 0 && (
                <div className="flex flex-wrap gap-1.5 mt-1.5">
                  {msg.imageAttachments.map((att, i) => (
                    <img
                      key={i}
                      src={`data:${att.mediaType};base64,${att.base64Data}`}
                      alt={att.originalName}
                      className="max-w-[200px] max-h-[200px] object-contain rounded-md border border-border"
                    />
                  ))}
                </div>
              )}
            </>
          ) : (
            <>
              <div className="mb-2 flex items-center gap-2">
                <span className="rounded-full bg-surface-3 px-2 py-1 text-[10px] font-medium uppercase tracking-[0.14em] text-text-tertiary">
                  {t('chat.conclusion')}
                </span>
                {evidenceItems.length > 0 && (
                  <span className="text-[11px] text-text-tertiary">
                    {t('chat.answerEvidenceSummary', { count: String(evidenceItems.length) })}
                  </span>
                )}
              </div>

              {evidenceItems.length > 0 && (
                <div className="mb-3 rounded-xl border border-border/70 bg-surface-1/70 px-2.5 py-2">
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="text-[11px] font-medium text-text-secondary">{t('chat.answerEvidence')}</span>
                    <span className="text-[10px] text-text-tertiary">
                      {t('chat.answerEvidenceSummary', { count: String(evidenceItems.length) })}
                    </span>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {evidenceItems.map((item) => (
                      <CitationChip
                        key={item.chunkId}
                        chunkId={item.chunkId}
                        displayText={item.displayText}
                        card={citationLookup?.getCard(item.chunkId)}
                      />
                    ))}
                  </div>
                </div>
              )}

              <div className="prose-chat">
                <CitationContext.Provider value={citationLookup ?? { getCard: () => undefined }}>
                  <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={rehypePlugins} components={markdownComponents}>
                    {preprocessFilePaths(preprocessCitations(preprocessInlineCitations(preprocessChunkCitations(msg.content))))}
                  </ReactMarkdown>
                </CitationContext.Provider>
              </div>
            </>
          )}
        </div>
        {/* Timestamp */}
        <span
          className={`text-[10px] text-text-tertiary mt-1 select-none transition-opacity duration-200
            ${isUser ? 'self-end pr-1' : 'self-start pl-1'}
            ${alwaysShowTimestamp ? 'opacity-60' : 'opacity-0 group-hover:opacity-60'}`}
        >
          {timestamp}
        </span>
      </div>
    </motion.div>
  );
}

export const MessageBubble = React.memo(MessageBubbleInner);
