import { useRef, useEffect, useLayoutEffect, useMemo, useState, useCallback } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { MessageCircle, ChevronDown, AlertCircle, RotateCcw, X, Search, FileText, Link2, HelpCircle } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useTranslation } from '../../i18n';
import { useTypewriter } from '../../lib/useTypewriter';
import { hasTimeGap } from '../../lib/relativeTime';
import { preprocessChunkCitations, buildCitationMap } from '../../lib/citationParser';
import type { CitationCardData } from '../../lib/citationParser';
import type { StreamRoundEvent, ToolCallEvent } from '../../lib/useAgentStream';
import { ToolCallCard } from './ToolCallCard';
import { ThinkingBlock } from './ThinkingBlock';
import type { ThinkingSection } from './ThinkingBlock';
import { markdownComponents, preprocessFilePaths, preprocessCitations, CitationContext } from './markdownComponents';
import { MessageBubble } from './MessageBubble';
import { Skeleton } from '../ui/Skeleton';
import type { ConversationMessage } from '../../types/conversation';

interface ChatMessagesProps {
  messages: ConversationMessage[];
  streamText: string;
  streamRounds: StreamRoundEvent[];
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
  lastCached?: boolean;
  onSuggestionClick?: (text: string) => void;
}

const SUGGESTIONS: { icon: typeof Search; labelKey: keyof import('../../i18n').TranslationKeys; promptKey: keyof import('../../i18n').TranslationKeys }[] = [
  { icon: Search, labelKey: 'chat.suggestions.search', promptKey: 'chat.suggestions.search.prompt' },
  { icon: FileText, labelKey: 'chat.suggestions.summarize', promptKey: 'chat.suggestions.summarize.prompt' },
  { icon: Link2, labelKey: 'chat.suggestions.connections', promptKey: 'chat.suggestions.connections.prompt' },
  { icon: HelpCircle, labelKey: 'chat.suggestions.question', promptKey: 'chat.suggestions.question.prompt' },
];

const INSTANT_TRANSITION = { duration: 0 };
const NEAR_BOTTOM_THRESHOLD = 96;
const FOLLOW_RELEASE_THRESHOLD = 160;

function normalizeThinking(content: string): string {
  return content.replace(/\r\n/g, '\n').trim();
}

function hasTraceToolCalls<T extends { toolName?: string; name?: string }>(toolCalls: T[]): boolean {
  return toolCalls.some(tc => (tc.toolName ?? tc.name) !== 'update_plan');
}

export function ChatMessages({
  messages,
  streamText,
  streamRounds,
  thinkingText,
  isThinking,
  toolCalls,
  isStreaming,
  error,
  onRetry,
  onDismissError,
  onDeleteMessage,
  onEditAndResend,
  loadingMsgs,
  lastCached,
  onSuggestionClick,
}: ChatMessagesProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const autoScrollFrameRef = useRef<number | null>(null);
  const shouldAutoFollowRef = useRef(true);
  const [isNearBottom, setIsNearBottom] = useState(true);
  const [hasOverflow, setHasOverflow] = useState(false);
  const [unreadCount, setUnreadCount] = useState(0);
  const prevMsgCountRef = useRef(messages.length);

  const chunkIdCacheRef = useRef<Map<string, string[]>>(new Map());
  const pendingChunkIdsRef = useRef<string[]>([]);

  useEffect(() => {
    const ids: string[] = [];
    for (const tc of toolCalls) {
      if (tc.status !== 'done' || !tc.artifacts) continue;
      const arr = Array.isArray(tc.artifacts) ? tc.artifacts : Object.values(tc.artifacts);
      for (const item of arr) {
        if (item && typeof item === 'object' && 'chunkId' in (item as Record<string, unknown>)) {
          ids.push((item as Record<string, unknown>).chunkId as string);
        }
      }
    }
    if (ids.length > 0) {
      pendingChunkIdsRef.current = ids;
    }
  }, [toolCalls]);

  const prevMessagesLenRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length > prevMessagesLenRef.current && pendingChunkIdsRef.current.length > 0) {
      for (let i = messages.length - 1; i >= 0; i -= 1) {
        if (messages[i].role === 'assistant') {
          chunkIdCacheRef.current.set(messages[i].id, [...pendingChunkIdsRef.current]);
          pendingChunkIdsRef.current = [];
          break;
        }
      }
    }
    prevMessagesLenRef.current = messages.length;
  }, [messages]);

  const typewriterText = useTypewriter(streamText, isStreaming, { charsPerTick: 5, intervalMs: 30 });
  const displayedText = shouldReduceMotion ? streamText : typewriterText;

  const [debouncedMarkdown, setDebouncedMarkdown] = useState('');
  useEffect(() => {
    if (shouldReduceMotion || !isStreaming) {
      setDebouncedMarkdown(displayedText);
      return;
    }
    const timer = setTimeout(() => setDebouncedMarkdown(displayedText), 100);
    return () => clearTimeout(timer);
  }, [displayedText, isStreaming, shouldReduceMotion]);

  const remarkPlugins = useMemo(() => [remarkGfm], []);

  const processedMarkdown = useMemo(
    () => preprocessFilePaths(preprocessCitations(preprocessChunkCitations(debouncedMarkdown))),
    [debouncedMarkdown],
  );

  const preprocessStreamingMarkdown = useCallback(
    (content: string) => preprocessFilePaths(preprocessCitations(preprocessChunkCitations(content))),
    [],
  );

  const streamingCitationLookup = useMemo(() => {
    const map = buildCitationMap(toolCalls);
    return { getCard: (id: string) => map.get(id) };
  }, [toolCalls]);

  const messageToolCalls = useMemo(() => {
    const map = new Map<number, ConversationMessage[]>();
    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role !== 'assistant' || msg.toolCalls.length === 0) continue;
      const toolResults: ConversationMessage[] = [];
      for (let j = i + 1; j < messages.length; j += 1) {
        if (messages[j].role !== 'tool') break;
        toolResults.push(messages[j]);
      }
      map.set(i, toolResults);
    }
    return map;
  }, [messages]);

  const messageCitationLookups = useMemo(() => {
    const map = new Map<number, { getCard: (id: string) => CitationCardData | undefined }>();
    for (const [idx, toolResults] of messageToolCalls.entries()) {
      const citationMap = buildCitationMap(
        toolResults.map(result => ({ artifacts: result.artifacts })),
      );
      map.set(idx, { getCard: (id: string) => citationMap.get(id) });
    }
    return map;
  }, [messageToolCalls]);

  const messageThinkingText = useMemo(() => {
    const map = new Map<number, string>();
    let lastUserIdx = -1;

    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role === 'user') {
        lastUserIdx = i;
        continue;
      }
      if (msg.role !== 'assistant' || !msg.thinking) continue;

      let renderableThinking = normalizeThinking(msg.thinking);
      if (msg.toolCalls.length === 0) {
        const priorToolRoundThinking: string[] = [];
        for (let j = lastUserIdx + 1; j < i; j += 1) {
          const prev = messages[j];
          if (prev.role !== 'assistant' || !prev.thinking || prev.toolCalls.length === 0) continue;
          const segment = normalizeThinking(prev.thinking);
          if (segment) {
            priorToolRoundThinking.push(segment);
          }
        }

        const knownPrefix = priorToolRoundThinking.join('\n').trim();
        if (knownPrefix && renderableThinking.startsWith(knownPrefix)) {
          renderableThinking = renderableThinking.slice(knownPrefix.length).replace(/^\s+/, '');
        }
      }

      if (renderableThinking) {
        map.set(i, renderableThinking);
      }
    }

    return map;
  }, [messages]);

  const messageTraceGroups = useMemo(() => {
    const map = new Map<number, { type: 'anchor'; sections: ThinkingSection[] } | { type: 'member' }>();
    let currentGroup: number[] = [];

    const flushGroup = () => {
      if (currentGroup.length === 0) return;

      const tracedIndexes = currentGroup.filter((idx) => {
        const msg = messages[idx];
        return messageThinkingText.has(idx) || hasTraceToolCalls(msg.toolCalls);
      });
      if (tracedIndexes.length === 0) {
        currentGroup = [];
        return;
      }

      const sections: ThinkingSection[] = tracedIndexes.map((idx) => {
        const msg = messages[idx];
        const thinking = messageThinkingText.get(idx) ?? '';
        const renderedToolCalls = msg.toolCalls
          .filter(tc => tc.name !== 'update_plan')
          .map((tc, toolIdx) => {
            const toolResult = messageToolCalls.get(idx)?.find(
              (tr) => tr.toolCallId === tc.id,
            );
            return (
              <ToolCallCard
                key={`persisted-trace-${msg.id}-${tc.id || tc.name || toolIdx}`}
                toolName={tc.name || 'unknown_tool'}
                arguments={tc.arguments || ''}
                status={toolResult ? 'done' : 'running'}
                content={toolResult?.content}
                artifacts={toolResult?.artifacts ?? undefined}
                compact
              />
            );
          });

        return {
          text: thinking,
          toolCallCards: renderedToolCalls.length > 0 ? (
            <div className="mt-1 space-y-1">
              {renderedToolCalls}
            </div>
          ) : undefined,
        };
      });

      const anchorIdx = tracedIndexes[0];
      map.set(anchorIdx, { type: 'anchor', sections });
      for (const idx of tracedIndexes.slice(1)) {
        map.set(idx, { type: 'member' });
      }
      currentGroup = [];
    };

    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role === 'user') {
        flushGroup();
        continue;
      }
      if (msg.role === 'assistant') {
        currentGroup.push(i);
      }
    }
    flushGroup();

    return map;
  }, [messageThinkingText, messageToolCalls, messages]);

  const streamTraceSections = useMemo<ThinkingSection[]>(() => {
    const sections: ThinkingSection[] = [];

    for (const round of streamRounds) {
      const traceToolCalls = round.toolCalls.filter(tc => tc.toolName !== 'update_plan');
      if (!round.thinking && traceToolCalls.length === 0) continue;

      sections.push({
        text: round.thinking || '',
        toolCallCards: traceToolCalls.length > 0 ? (
          <div className="mt-1 space-y-1">
            {traceToolCalls.map((tc, toolIdx) => (
              <ToolCallCard
                key={`stream-trace-${round.id}-${tc.callId || 'tool-call'}-${toolIdx}`}
                toolName={tc.toolName}
                arguments={tc.arguments}
                status={tc.status}
                content={tc.content}
                isError={tc.isError}
                artifacts={tc.artifacts}
                compact
              />
            ))}
          </div>
        ) : undefined,
      });
    }

    if (thinkingText.trim().length > 0) {
      sections.push({ text: thinkingText });
    }

    return sections;
  }, [streamRounds, thinkingText]);

  const streamTraceActive = useMemo(() => {
    if (!isStreaming) return false;
    if (isThinking || thinkingText.trim().length > 0) return true;
    return streamRounds.some(round =>
      round.toolCalls.some(tc => tc.toolName !== 'update_plan' && tc.status === 'running'));
  }, [isStreaming, isThinking, streamRounds, thinkingText]);

  const getScrollMetrics = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el) {
      return { distanceFromBottom: 0, nearBottom: true, overflow: false };
    }
    const distanceFromBottom = Math.max(0, el.scrollHeight - el.scrollTop - el.clientHeight);
    return {
      distanceFromBottom,
      nearBottom: distanceFromBottom <= NEAR_BOTTOM_THRESHOLD,
      overflow: el.scrollHeight > el.clientHeight + 8,
    };
  }, []);

  const scrollToContainerBottom = useCallback((behavior: ScrollBehavior) => {
    const el = scrollContainerRef.current;
    if (!el) return;

    if (autoScrollFrameRef.current != null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
    }

    autoScrollFrameRef.current = requestAnimationFrame(() => {
      el.scrollTo({ top: el.scrollHeight, behavior });
      setHasOverflow(el.scrollHeight > el.clientHeight + 8);
      setIsNearBottom(true);
      setUnreadCount(0);
      autoScrollFrameRef.current = null;
    });
  }, []);

  useEffect(() => () => {
    if (autoScrollFrameRef.current != null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
    }
  }, []);

  const handleScroll = useCallback(() => {
    const { distanceFromBottom, nearBottom, overflow } = getScrollMetrics();
    setHasOverflow(overflow);
    setIsNearBottom(!overflow || nearBottom);

    if (!overflow || nearBottom) {
      shouldAutoFollowRef.current = true;
      setUnreadCount(0);
      return;
    }

    if (distanceFromBottom > FOLLOW_RELEASE_THRESHOLD) {
      shouldAutoFollowRef.current = false;
    }
  }, [getScrollMetrics]);

  useEffect(() => {
    const newCount = messages.length - prevMsgCountRef.current;
    if (newCount > 0 && hasOverflow && !shouldAutoFollowRef.current) {
      setUnreadCount((count) => count + newCount);
    }
    prevMsgCountRef.current = messages.length;
  }, [messages.length, hasOverflow]);

  useLayoutEffect(() => {
    const { nearBottom, overflow } = getScrollMetrics();
    setHasOverflow(overflow);
    if (!overflow) {
      shouldAutoFollowRef.current = true;
      setIsNearBottom(true);
      setUnreadCount(0);
      return;
    }

    if (!shouldAutoFollowRef.current) {
      setIsNearBottom(nearBottom);
      return;
    }

    scrollToContainerBottom('auto');
  }, [messages, streamText, streamRounds, toolCalls, getScrollMetrics, scrollToContainerBottom]);

  const scrollToBottom = useCallback(() => {
    shouldAutoFollowRef.current = true;
    scrollToContainerBottom(shouldReduceMotion ? 'auto' : 'smooth');
  }, [scrollToContainerBottom, shouldReduceMotion]);

  const lastAssistantIdx = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      if (messages[i].role === 'assistant') return i;
    }
    return -1;
  }, [messages]);

  const lastRenderableMessageRole = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      const msg = messages[i];
      if (msg.role === 'tool' || msg.role === 'system') continue;
      if (msg.role === 'assistant' && msg.content.trim().length === 0) continue;
      return msg.role;
    }
    return null;
  }, [messages]);

  const shouldRenderStreamRounds = streamRounds.length > 0;
  const shouldShowStreamingText = isStreaming
    || (
      streamText.trim().length > 0
      && (lastRenderableMessageRole == null || lastRenderableMessageRole === 'user')
    );

  if (messages.length === 0 && !isStreaming && !loadingMsgs) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center max-w-md w-full px-4">
          <div className="p-4 rounded-2xl bg-surface-2 text-text-tertiary inline-block mb-4">
            <MessageCircle className="h-8 w-8" />
          </div>
          <p className="text-sm text-text-tertiary mb-6">{t('chat.placeholder')}</p>
          {onSuggestionClick && (
            <div className="grid grid-cols-2 gap-3">
              {SUGGESTIONS.map((s, i) => {
                const Icon = s.icon;
                const prompt = t(s.promptKey);
                return (
                  <motion.button
                    key={s.labelKey}
                    type="button"
                    initial={shouldReduceMotion ? false : { opacity: 0, y: 12 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={shouldReduceMotion ? INSTANT_TRANSITION : { delay: i * 0.07, duration: 0.3, ease: 'easeOut' }}
                    onClick={() => onSuggestionClick(prompt)}
                    className="bg-surface-1 hover:bg-surface-2 border border-border rounded-lg p-4 cursor-pointer transition-colors text-left"
                  >
                    <Icon className="h-4 w-4 text-accent mb-2" />
                    <p className="text-sm font-medium text-text-primary mb-1">{t(s.labelKey)}</p>
                    <p className="text-xs text-text-tertiary truncate">{prompt}</p>
                  </motion.button>
                );
              })}
            </div>
          )}
        </div>
      </div>
    );
  }

  if (loadingMsgs) {
    return (
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-48" />
          </div>
        </div>
        <div className="flex justify-start">
          <div className="max-w-[80%] rounded-lg bg-surface-2 px-3.5 py-2.5 space-y-2">
            <Skeleton className="h-4 w-64" />
            <Skeleton className="h-4 w-56" />
            <Skeleton className="h-4 w-40" />
          </div>
        </div>
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-36" />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      ref={scrollContainerRef}
      onScroll={handleScroll}
      data-chat-scroll-root="true"
      className="flex-1 overflow-y-auto px-4 py-4 relative"
      role="log"
      aria-live="polite"
      aria-label={t('chat.messageArea')}
    >
      <AnimatePresence initial={false}>
        {messages.map((msg, idx) => {
          if (msg.role === 'tool' || msg.role === 'system') return null;

          const queryText = msg.role === 'assistant'
            ? (messages.slice(0, idx).reverse().find((m) => m.role === 'user')?.content ?? '')
            : '';
          const chunkIds = chunkIdCacheRef.current.get(msg.id) ?? [];
          const traceGroup = msg.role === 'assistant' ? messageTraceGroups.get(idx) : undefined;
          const hasRenderableAssistantContent =
            msg.role !== 'assistant' || msg.content.trim().length > 0;

          return (
            <div key={msg.id}>
              {traceGroup?.type === 'anchor' && (
                <div className="flex justify-start mb-1">
                  <div className="max-w-[80%]">
                    <ThinkingBlock
                      content=""
                      sections={traceGroup.sections}
                      isStreaming={false}
                      defaultExpanded
                      collapseOnFinish={false}
                    />
                  </div>
                </div>
              )}

              {hasRenderableAssistantContent && (
                <MessageBubble
                  msg={msg}
                  chunkIds={chunkIds}
                  queryText={queryText}
                  citationLookup={messageCitationLookups.get(idx)}
                  isLastAssistant={idx === lastAssistantIdx && !isStreaming}
                  lastCached={idx === lastAssistantIdx ? lastCached : undefined}
                  onRetry={onRetry}
                  alwaysShowTimestamp={(() => {
                    for (let p = idx - 1; p >= 0; p -= 1) {
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
            </div>
          );
        })}
      </AnimatePresence>

      {streamTraceSections.length > 0 && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          layout={!shouldReduceMotion}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          className="flex justify-start mb-3"
        >
          <div className="max-w-[80%]">
            <ThinkingBlock
              content=""
              sections={streamTraceSections}
              isStreaming={streamTraceActive}
              defaultExpanded
              collapseOnFinish={false}
            />
          </div>
        </motion.div>
      )}

      {shouldRenderStreamRounds && streamRounds
        .filter(round => round.reply.trim().length > 0)
        .map((round) => (
          <motion.div
            key={`reply-${round.id}`}
            initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            layout={!shouldReduceMotion}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
            className="flex justify-start mb-4"
          >
            <div className="max-w-[80%] rounded-lg px-3.5 py-2.5 text-sm leading-relaxed bg-surface-2 text-text-primary">
              <div className="prose-chat">
                <CitationContext.Provider value={streamingCitationLookup}>
                  <ReactMarkdown remarkPlugins={remarkPlugins} components={markdownComponents}>
                    {preprocessStreamingMarkdown(round.reply)}
                  </ReactMarkdown>
                </CitationContext.Provider>
              </div>
            </div>
          </motion.div>
        ))}

      {shouldShowStreamingText && streamText.trim().length > 0 && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          className="flex justify-start mb-3"
        >
          <div
            className="relative max-w-[80%] rounded-lg px-3.5 py-2.5 pr-6 text-sm leading-relaxed bg-surface-2 text-text-primary"
            style={streamText.length > 2000 ? { willChange: 'contents' } : undefined}
          >
            <div className="prose-chat">
              <CitationContext.Provider value={streamingCitationLookup}>
                <ReactMarkdown remarkPlugins={remarkPlugins} components={markdownComponents}>
                  {processedMarkdown}
                </ReactMarkdown>
              </CitationContext.Provider>
            </div>
            <span className={`streaming-caret-overlay ${shouldReduceMotion ? '' : 'animate-pulse'}`} />
          </div>
        </motion.div>
      )}

      {isStreaming && !streamText && streamRounds.length === 0 && toolCalls.length === 0 && !thinkingText && !isThinking && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          className="flex justify-start mb-3"
        >
          <div className="rounded-lg px-3.5 py-2.5 bg-surface-2" role="status" aria-label={t('chat.thinking')}>
            <div className="flex items-center gap-2 text-sm text-text-tertiary">
              <div className="flex gap-1">
                <span className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? '' : 'animate-bounce'}`} style={{ animationDelay: '0ms' }} />
                <span className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? '' : 'animate-bounce'}`} style={{ animationDelay: '150ms' }} />
                <span className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? '' : 'animate-bounce'}`} style={{ animationDelay: '300ms' }} />
              </div>
              {t('chat.thinking')}
            </div>
          </div>
        </motion.div>
      )}

      {error && !isStreaming && (
        <motion.div
          initial={shouldReduceMotion ? false : { opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={shouldReduceMotion ? { opacity: 0, y: 0 } : { opacity: 0, y: 8 }}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
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

      <AnimatePresence>
        {hasOverflow && !isNearBottom && (
          <motion.button
            initial={shouldReduceMotion ? false : { opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={shouldReduceMotion ? { opacity: 0, y: 0 } : { opacity: 0, y: 12 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.18, ease: 'easeOut' }}
            type="button"
            onClick={scrollToBottom}
            title={t('chat.scrollToBottom')}
            className="sticky bottom-3 left-1/2 -translate-x-1/2 mx-auto flex items-center gap-1.5 rounded-full bg-surface-3 hover:bg-surface-4 text-text-primary shadow-md px-3 py-2 transition-colors cursor-pointer z-10"
          >
            <ChevronDown className="h-4 w-4" />
            {unreadCount > 0 && (
              <span className="text-xs font-medium tabular-nums">{unreadCount}</span>
            )}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
