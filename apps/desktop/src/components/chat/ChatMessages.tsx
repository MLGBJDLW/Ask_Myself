import { useRef, useEffect, useMemo, useState, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { MessageCircle, ChevronDown, AlertCircle, RotateCcw, X } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useTranslation } from '../../i18n';
import { useTypewriter } from '../../lib/useTypewriter';
import { hasTimeGap } from '../../lib/relativeTime';
import { preprocessChunkCitations, buildCitationMap } from '../../lib/citationParser';
import { ToolCallCard } from './ToolCallCard';
import { ThinkingBlock } from './ThinkingBlock';
import { markdownComponents, preprocessFilePaths, preprocessCitations, CitationContext } from './markdownComponents';
import { MessageBubble } from './MessageBubble';
import { Skeleton } from '../ui/Skeleton';
import type { ConversationMessage } from '../../types/conversation';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
}

interface ChatMessagesProps {
  messages: ConversationMessage[];
  streamText: string;
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  isStreaming: boolean;
  error?: string | null;
  onRetry?: () => void;
  onDismissError?: () => void;
  onDeleteMessage?: (messageId: string) => void;
  onEditAndResend?: (messageId: string, newContent: string) => void;
  loadingMsgs?: boolean;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatMessages({ messages, streamText, thinkingText, isThinking, toolCalls, isStreaming, error, onRetry, onDismissError, onDeleteMessage, onEditAndResend, loadingMsgs }: ChatMessagesProps) {
  const { t } = useTranslation();
  const bottomRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const [isNearBottom, setIsNearBottom] = useState(true);

  // ── Feedback: chunk-ID tracking ───────────────────────────────────────
  const chunkIdCacheRef = useRef<Map<string, string[]>>(new Map());
  const pendingChunkIdsRef = useRef<string[]>([]);

  // Collect chunk IDs from streaming tool-call artifacts
  useEffect(() => {
    const ids: string[] = [];
    for (const tc of toolCalls) {
      if (tc.status === 'done' && tc.artifacts) {
        const arr = Array.isArray(tc.artifacts) ? tc.artifacts : Object.values(tc.artifacts);
        for (const item of arr) {
          if (item && typeof item === 'object' && 'chunkId' in (item as Record<string, unknown>)) {
            ids.push((item as Record<string, unknown>).chunkId as string);
          }
        }
      }
    }
    if (ids.length > 0) {
      pendingChunkIdsRef.current = ids;
    }
  }, [toolCalls]);

  // When streaming ends and a new assistant message appears, persist chunk IDs
  const prevMessagesLenRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length > prevMessagesLenRef.current && pendingChunkIdsRef.current.length > 0) {
      for (let i = messages.length - 1; i >= 0; i--) {
        if (messages[i].role === 'assistant') {
          chunkIdCacheRef.current.set(messages[i].id, [...pendingChunkIdsRef.current]);
          pendingChunkIdsRef.current = [];
          break;
        }
      }
    }
    prevMessagesLenRef.current = messages.length;
  }, [messages]);

  // Typewriter: gradually reveal streamed text for a smooth typing feel
  const displayedText = useTypewriter(streamText, isStreaming, { charsPerTick: 5, intervalMs: 30 });
  const isRevealing = isStreaming || displayedText.length < streamText.length;
  const streamingThinkingContent = thinkingText || t('chat.thinking');

  // Debounce displayed text for markdown rendering (~100ms) to avoid re-parsing on every tick
  const [debouncedMarkdown, setDebouncedMarkdown] = useState('');
  useEffect(() => {
    if (!isStreaming) {
      setDebouncedMarkdown(displayedText);
      return;
    }
    const timer = setTimeout(() => setDebouncedMarkdown(displayedText), 100);
    return () => clearTimeout(timer);
  }, [displayedText, isStreaming]);

  // Memoize remark plugins array to avoid re-creating on each render
  const remarkPlugins = useMemo(() => [remarkGfm], []);

  // Pre-process markdown content (memoized on debounced text)
  const processedMarkdown = useMemo(
    () => preprocessFilePaths(preprocessCitations(preprocessChunkCitations(debouncedMarkdown))),
    [debouncedMarkdown],
  );

  // Build citation lookup map from streaming tool call artifacts
  const streamingCitationLookup = useMemo(() => {
    const map = buildCitationMap(toolCalls);
    return { getCard: (id: string) => map.get(id) };
  }, [toolCalls]);

  // Build tool call map from messages for completed tool calls
  const messageToolCalls = useMemo(() => {
    const map = new Map<number, ConversationMessage[]>();
    for (let i = 0; i < messages.length; i++) {
      const msg = messages[i];
      if (msg.role === 'assistant' && msg.toolCalls.length > 0) {
        const toolResults: ConversationMessage[] = [];
        for (let j = i + 1; j < messages.length; j++) {
          if (messages[j].role === 'tool') {
            toolResults.push(messages[j]);
          } else {
            break;
          }
        }
        map.set(i, toolResults);
      }
    }
    return map;
  }, [messages]);

  // ── Smart auto-scroll ─────────────────────────────────────────────
  const NEAR_BOTTOM_THRESHOLD = 100;

  const checkNearBottom = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_THRESHOLD;
  }, []);

  const handleScroll = useCallback(() => {
    setIsNearBottom(checkNearBottom());
  }, [checkNearBottom]);

  // Auto-scroll only when near bottom
  useEffect(() => {
    if (isNearBottom) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
    }
  }, [messages, streamText, toolCalls, isNearBottom]);

  const scrollToBottom = useCallback(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
    setIsNearBottom(true);
  }, []);

  // Find the last assistant message index for retry button
  // NOTE: This useMemo MUST be before early returns to satisfy React's Rules of Hooks.
  const lastAssistantIdx = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'assistant') return i;
    }
    return -1;
  }, [messages]);

  // Empty state
  if (messages.length === 0 && !isStreaming && !loadingMsgs) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center">
          <div className="p-4 rounded-2xl bg-surface-2 text-text-tertiary inline-block mb-4">
            <MessageCircle className="h-8 w-8" />
          </div>
          <p className="text-sm text-text-tertiary">{t('chat.placeholder')}</p>
        </div>
      </div>
    );
  }

  // Loading skeleton
  if (loadingMsgs) {
    return (
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
        {/* User skeleton */}
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-48" />
          </div>
        </div>
        {/* Assistant skeleton */}
        <div className="flex justify-start">
          <div className="max-w-[80%] rounded-lg bg-surface-2 px-3.5 py-2.5 space-y-2">
            <Skeleton className="h-4 w-64" />
            <Skeleton className="h-4 w-56" />
            <Skeleton className="h-4 w-40" />
          </div>
        </div>
        {/* User skeleton 2 */}
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-36" />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div ref={scrollContainerRef} onScroll={handleScroll} className="flex-1 overflow-y-auto px-4 py-4 relative" role="log" aria-live="polite" aria-label={t('chat.messageArea')}>
      <AnimatePresence initial={false}>
        {messages.map((msg, idx) => {
          // Skip tool result messages (rendered inline with tool calls)
          if (msg.role === 'tool' || msg.role === 'system') return null;

          // Resolve feedback context for assistant messages
          const queryText = msg.role === 'assistant'
            ? (messages.slice(0, idx).reverse().find((m) => m.role === 'user')?.content ?? '')
            : '';
          const chunkIds = chunkIdCacheRef.current.get(msg.id) ?? [];
          const hasRenderableAssistantContent =
            msg.role !== 'assistant' || msg.content.trim().length > 0;

          return (
            <div key={msg.id}>
              {/* Persisted thinking block for assistant messages */}
              {msg.role === 'assistant' && msg.thinking && (
                <div className="flex justify-start mb-3">
                  <div className="max-w-[80%]">
                    <ThinkingBlock content={msg.thinking} isStreaming={false} />
                  </div>
                </div>
              )}

              {hasRenderableAssistantContent && (
                <MessageBubble
                  msg={msg}
                  chunkIds={chunkIds}
                  queryText={queryText}
                  isLastAssistant={idx === lastAssistantIdx && !isStreaming}
                  onRetry={onRetry}
                  alwaysShowTimestamp={(() => {
                    // Find previous visible message
                    for (let p = idx - 1; p >= 0; p--) {
                      const prev = messages[p];
                      if (prev.role !== 'tool' && prev.role !== 'system') {
                        return hasTimeGap(prev.createdAt, msg.createdAt);
                      }
                    }
                    return false;
                  })()}
                  onDeleteMessage={onDeleteMessage}
                  onEditAndResend={onEditAndResend}
                />
              )}

              {/* Show tool call cards after assistant messages with tool calls */}
              {msg.role === 'assistant' && msg.toolCalls.length > 0 && (
                <div className="mb-3">
                  {msg.toolCalls.map((tc, toolIdx) => {
                    const toolResult = messageToolCalls.get(idx)?.find(
                      (tr) => tr.toolCallId === tc.id,
                    );
                    return (
                      <ToolCallCard
                        key={`${msg.id}-tool-${tc.id || tc.name || toolIdx}`}
                        toolName={tc.name || 'unknown_tool'}
                        arguments={tc.arguments || ''}
                        status={toolResult ? 'done' : 'running'}
                        content={toolResult?.content}
                      />
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </AnimatePresence>

      {/* Streaming thinking block */}
      {isStreaming && (thinkingText || isThinking) && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="flex justify-start mb-3"
        >
          <div className="max-w-[80%]">
            <ThinkingBlock content={streamingThinkingContent} isStreaming={isThinking} />
          </div>
        </motion.div>
      )}

      {/* Streaming tool calls */}
      {isStreaming && toolCalls.length > 0 && (
        <div className="mb-3">
          {toolCalls.map((tc, toolIdx) => (
            <ToolCallCard
              key={`${tc.callId || 'tool-call'}-${toolIdx}`}
              toolName={tc.toolName}
              arguments={tc.arguments}
              status={tc.status}
              content={tc.content}
              isError={tc.isError}
              artifacts={tc.artifacts}
            />
          ))}
        </div>
      )}

      {/* Streaming text */}
      {isStreaming && streamText && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="flex justify-start mb-3"
        >
          <div
            className="max-w-[80%] rounded-lg px-3.5 py-2.5 text-sm leading-relaxed bg-surface-2 text-text-primary"
            style={streamText.length > 2000 ? { willChange: 'contents' } : undefined}
          >
            <div className="prose-chat">
              <CitationContext.Provider value={streamingCitationLookup}>
                <ReactMarkdown remarkPlugins={remarkPlugins} components={markdownComponents}>
                  {processedMarkdown}
                </ReactMarkdown>
              </CitationContext.Provider>
              {isRevealing && (
                <span className="inline-block w-1.5 h-4 bg-accent animate-pulse ml-0.5 align-text-bottom rounded-sm" />
              )}
            </div>
          </div>
        </motion.div>
      )}

      {/* Thinking indicator */}
      {isStreaming && !streamText && toolCalls.length === 0 && !thinkingText && !isThinking && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="flex justify-start mb-3"
        >
          <div className="rounded-lg px-3.5 py-2.5 bg-surface-2" role="status" aria-label={t('chat.thinking')}>
            <div className="flex items-center gap-2 text-sm text-text-tertiary">
              <div className="flex gap-1">
                <span className="w-1.5 h-1.5 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '0ms' }} />
                <span className="w-1.5 h-1.5 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '150ms' }} />
                <span className="w-1.5 h-1.5 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '300ms' }} />
              </div>
              {t('chat.thinking')}
            </div>
          </div>
        </motion.div>
      )}

      {/* Inline error state */}
      {error && !isStreaming && (
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 8 }}
          className="flex justify-start mb-3"
        >
          <div className="max-w-[80%] rounded-lg px-3.5 py-2.5 bg-red-500/10 border border-red-500/20 text-sm">
            <div className="flex items-start gap-2">
              <AlertCircle className="h-4 w-4 text-red-400 mt-0.5 shrink-0" />
              <div className="flex-1 min-w-0">
                <p className="text-red-400 font-medium text-xs mb-1">{t('chat.errorOccurred')}</p>
                <p className="text-red-300/80 text-xs break-words">{error}</p>
                <div className="flex items-center gap-2 mt-2">
                  {onRetry && (
                    <button
                      type="button"
                      onClick={onRetry}
                      className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-red-500/20 text-red-300 hover:bg-red-500/30 transition-colors cursor-pointer"
                    >
                      <RotateCcw className="h-3 w-3" />
                      {t('chat.retry')}
                    </button>
                  )}
                  {onDismissError && (
                    <button
                      type="button"
                      onClick={onDismissError}
                      className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-surface-2 text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer"
                    >
                      <X className="h-3 w-3" />
                      {t('chat.dismiss')}
                    </button>
                  )}
                </div>
              </div>
            </div>
          </div>
        </motion.div>
      )}

      <div ref={bottomRef} />

      {/* Scroll-to-bottom floating button */}
      <AnimatePresence>
        {!isNearBottom && (
          <motion.button
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            transition={{ duration: 0.15 }}
            type="button"
            onClick={scrollToBottom}
            title={t('chat.scrollToBottom')}
            className="sticky bottom-3 left-1/2 -translate-x-1/2 mx-auto block p-2 rounded-full bg-surface-2 border border-border shadow-lg text-text-tertiary hover:text-text-primary hover:bg-surface-3 transition-colors cursor-pointer z-10"
          >
            <ChevronDown className="h-4 w-4" />
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
