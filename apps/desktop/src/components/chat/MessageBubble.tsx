import React, { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { motion } from 'framer-motion';
import ReactMarkdown from 'react-markdown';
import { Check, CheckCircle2, ClipboardList, CornerDownRight, HelpCircle, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import {
  CitationContext,
  markdownComponents,
  markdownRemarkPlugins,
  preprocessCitations,
  preprocessFilePaths,
  rehypePlugins,
} from './markdownComponents';
import { preprocessChunkCitations, preprocessInlineCitations } from '../../lib/citationParser';
import type { CitationCardData } from '../../lib/citationParser';
import { isSteeringMessage } from '../../lib/chatMessageGuards';
import { buildEvidenceItemsFromContent } from '../../lib/evidenceItems';
import { MessageActions } from './MessageActions';
import { messageTimestamp } from '../../lib/relativeTime';
import type { ConversationMessage } from '../../types/conversation';
import { CitationChip } from './EvidenceCard';
import {
  extractProposedPlan,
  stripProposedPlanBlock,
  type ProposedPlanArtifact,
} from '../../lib/proposedPlan';
import { extractQuestionCards, stripQuestionCardsBlocks, type QuestionCard } from '../../lib/questionCards';

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
  onRetry?: (messageId?: string) => void;
  /** Always show timestamp (when gap > 5min) */
  alwaysShowTimestamp?: boolean;
  /** Called when a message is deleted */
  onDeleteMessage?: (messageId: string) => void;
  /** Called when a message is edited and re-sent */
  onEditAndResend?: (messageId: string, newContent: string) => void;
  /** Called when a proposed plan is approved for implementation */
  onApprovePlan?: (planMarkdown: string, sourceMessageId: string) => void;
}


/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

function ProposedPlanCard({
  plan,
  onApprove,
}: {
  plan: ProposedPlanArtifact;
  onApprove?: () => void;
}) {
  return (
    <div className="mb-3 overflow-hidden rounded-lg border border-accent/25 bg-surface-1/80">
      <div className="flex min-w-0 items-center gap-2 border-b border-border/50 px-3 py-2">
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-accent/30 bg-accent/10 text-accent">
          <ClipboardList className="h-3.5 w-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-semibold text-text-primary">Plan mode</div>
          <div className="truncate text-[11px] text-text-tertiary">Read-only proposal</div>
        </div>
        {onApprove && (
          <button
            type="button"
            onClick={onApprove}
            className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md bg-accent px-2.5 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
          >
            <CheckCircle2 className="h-3.5 w-3.5" />
            Approve and implement
          </button>
        )}
      </div>
      <div className="px-3 py-3">
        <h3 className="mb-2 text-sm font-semibold leading-5 text-text-primary">{plan.title}</h3>
        <div className="prose-chat text-sm">
          <ReactMarkdown
            remarkPlugins={markdownRemarkPlugins}
            rehypePlugins={rehypePlugins}
            components={markdownComponents}
            urlTransform={(url) => url}
          >
            {preprocessFilePaths(preprocessCitations(preprocessInlineCitations(preprocessChunkCitations(plan.markdown))))}
          </ReactMarkdown>
        </div>
      </div>
    </div>
  );
}


function QuestionCardsPanel({ cards }: { cards: QuestionCard[] }) {
  if (cards.length === 0) return null;
  return (
    <div className="mb-3 overflow-hidden rounded-lg border border-accent/20 bg-surface-1/75">
      <div className="flex items-center gap-2 border-b border-border/50 px-3 py-2">
        <span className="flex h-7 w-7 items-center justify-center rounded-md border border-accent/30 bg-accent/10 text-accent">
          <HelpCircle className="h-3.5 w-3.5" />
        </span>
        <div>
          <div className="text-xs font-semibold text-text-primary">Question cards</div>
          <div className="text-[11px] text-text-tertiary">Structured answers requested by the agent</div>
        </div>
      </div>
      <div className="grid gap-2 p-3">
        {cards.map((card, index) => (
          <div key={`${card.id}-${index}`} className="rounded-lg border border-border/60 bg-surface-0/70 p-3">
            <div className="mb-1 flex items-start justify-between gap-2">
              <h4 className="text-sm font-semibold text-text-primary">{card.title}</h4>
              <span className="rounded-full border border-border/60 px-2 py-0.5 text-[10px] uppercase tracking-[0.08em] text-text-tertiary">
                {card.type.replace('_', ' ')}
              </span>
            </div>
            <p className="text-sm text-text-primary">{card.question}</p>
            {card.why && <p className="mt-1 text-xs text-text-tertiary">Why: {card.why}</p>}
            {card.options && card.options.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1.5">
                {card.options.map((option) => (
                  <span key={option} className="rounded-md border border-border/60 bg-surface-2/70 px-2 py-1 text-xs text-text-secondary">
                    {option}
                  </span>
                ))}
              </div>
            )}
            {card.placeholder && <p className="mt-2 rounded-md bg-surface-2/50 px-2 py-1.5 text-xs text-text-tertiary">{card.placeholder}</p>}
          </div>
        ))}
      </div>
    </div>
  );
}

function MessageBubbleInner({ msg, chunkIds, queryText, citationLookup, isLastAssistant, lastCached, onRetry, alwaysShowTimestamp, onDeleteMessage, onEditAndResend, onApprovePlan }: MessageBubbleProps) {
  const { t } = useTranslation();
  const isUser = msg.role === 'user';
  const [isEditing, setIsEditing] = useState(false);
  const [editText, setEditText] = useState(msg.content);
  const editRef = useRef<HTMLTextAreaElement>(null);
  const proposedPlan = useMemo(
    () => (isUser ? null : extractProposedPlan(msg.artifacts, msg.content)),
    [isUser, msg.artifacts, msg.content],
  );
  const questionCards = useMemo(() => (isUser ? [] : extractQuestionCards(msg.content)), [isUser, msg.content]);
  const contentWithoutQuestionCards = questionCards.length > 0 ? stripQuestionCardsBlocks(msg.content) : msg.content;
  const visibleContent = proposedPlan ? stripProposedPlanBlock(contentWithoutQuestionCards) : contentWithoutQuestionCards;
  const actionText = proposedPlan
    ? [visibleContent, proposedPlan.markdown].filter(Boolean).join('\n\n')
    : msg.content;

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
    return buildEvidenceItemsFromContent(
      visibleContent,
      citationLookup,
      (index) => t('chat.evidenceSourceLabel', { index: String(index) }),
      { dedupeChunks: true, sortByFrequency: true },
    ).map((item) => ({
      chunkId: item.chunkId,
      displayText: item.displayText,
    }));
  }, [citationLookup, isUser, visibleContent, t]);

  const timestamp = messageTimestamp(msg.createdAt, t);
  const steering = isSteeringMessage(msg);
  const ariaLabel = steering
    ? t('chat.steeringMessage')
    : isUser
      ? t('chat.userMessage')
      : t('chat.assistantResponse');

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className={`flex ${isUser ? 'justify-end' : 'justify-start'} mb-3`}
    >
      <div
        className={`group flex flex-col ${
          isUser ? 'max-w-[80%]' : 'w-full min-w-0'
        }`}
      >
        <div
          aria-label={ariaLabel}
          className={`relative text-sm leading-relaxed
            ${isUser
              ? 'rounded-lg bg-accent/20 px-3.5 py-2.5 text-text-primary'
              : 'bg-transparent px-0 py-0 text-text-primary'
            }`}
        >
          {msg.tokenCount > 0 && !isEditing && (
            <span
              className="absolute bottom-0.5 right-2 text-[9px] text-text-tertiary/0 group-hover:text-text-tertiary/60 transition-colors tabular-nums select-none"
              title={`${msg.tokenCount.toLocaleString()} ${t('chat.tokensLabel')}`}
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
              {steering && (
                <span className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-[0.12em] text-accent/80">
                  <CornerDownRight className="h-3 w-3" />
                  {t('chat.steeringLabel')}
                </span>
              )}
              <span className="whitespace-pre-wrap">{visibleContent}</span>
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

              {questionCards.length > 0 && <QuestionCardsPanel cards={questionCards} />}

              {proposedPlan && (
                <ProposedPlanCard
                  plan={proposedPlan}
                  onApprove={
                    onApprovePlan
                      ? () => onApprovePlan(proposedPlan.markdown, msg.id)
                      : undefined
                  }
                />
              )}

              {visibleContent.length > 0 && (
                <div className="prose-chat">
                  <CitationContext.Provider value={citationLookup ?? { getCard: () => undefined }}>
                    <ReactMarkdown
                      remarkPlugins={markdownRemarkPlugins}
                      rehypePlugins={rehypePlugins}
                      components={markdownComponents}
                      urlTransform={(url) => url}
                    >
                      {preprocessFilePaths(preprocessCitations(preprocessInlineCitations(preprocessChunkCitations(visibleContent))))}
                    </ReactMarkdown>
                  </CitationContext.Provider>
                </div>
              )}
            </>
          )}
        </div>
        {!isEditing && (
          <MessageActions
            text={actionText}
            showFeedback={!isUser}
            chunkIds={chunkIds}
            queryText={queryText}
            showRetry={Boolean(onRetry && ((isUser && !steering) || isLastAssistant))}
            onRetry={
              onRetry
                ? () => {
                    void onRetry(isUser ? msg.id : undefined);
                  }
                : undefined
            }
            isUser={isUser}
            messageId={msg.id}
            conversationId={msg.conversationId}
            onEdit={isUser && onEditAndResend ? handleStartEdit : undefined}
            onDelete={onDeleteMessage}
            align={isUser ? 'end' : 'start'}
          />
        )}
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
