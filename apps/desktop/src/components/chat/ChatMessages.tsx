import { useRef, useEffect, useMemo, useCallback, useState, type ComponentPropsWithoutRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { MessageCircle, Copy, Check, ThumbsUp, ThumbsDown } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { open } from '@tauri-apps/plugin-shell';
import { useTranslation } from '../../i18n';
import { useTypewriter } from '../../lib/useTypewriter';
import { ToolCallCard } from './ToolCallCard';
import { ThinkingBlock } from './ThinkingBlock';
import { FileBadge } from '../ui/FileBadge';
import * as api from '../../lib/api';
import type { ConversationMessage } from '../../types/conversation';

/* ------------------------------------------------------------------ */
/*  File-path detection constants                                      */
/* ------------------------------------------------------------------ */

const FILE_EXT =
  'md|markdown|txt|log|pdf|docx|xlsx|xls|pptx|ts|tsx|js|jsx|rs|' +
  'json|toml|yaml|yml|css|scss|sass|less|html|py|go|java|c|cpp|' +
  'h|hpp|sh|bat|sql|xml|csv';

const FILE_PATH_REGEX = new RegExp(
  `^(?:[A-Za-z]:[\\\\/]|\\.{1,2}[\\\\/]|\\/|[\\w.-]+[\\\\/])?[\\w .,()\\\\/~\\-\\u4e00-\\u9fff]*\\.(?:${FILE_EXT})$`,
  'i',
);

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
}

/* ------------------------------------------------------------------ */
/*  Markdown component overrides                                       */
/* ------------------------------------------------------------------ */

/** Open links in the system browser via Tauri shell */
function MarkdownLink({ href, children, ...rest }: ComponentPropsWithoutRef<'a'>) {
  const handleClick = useCallback(
    (e: React.MouseEvent<HTMLAnchorElement>) => {
      e.preventDefault();
      if (href) open(href);
    },
    [href],
  );
  return (
    <a
      {...rest}
      href={href}
      onClick={handleClick}
      className="text-accent hover:text-accent-hover underline underline-offset-2"
    >
      {children}
    </a>
  );
}

/**
 * Pre-process AI citations like [source: D:\path\to\file.docx]
 * into backtick-wrapped paths so the `code` component renders them as FileBadge.
 */
function preprocessCitations(content: string): string {
  return content.replace(/\[source:\s*([^\]]+)\]/gi, (_match, path: string) => `\`${path.trim()}\``);
}

/**
 * Detects bare file paths in markdown prose and wraps them in backticks
 * so they get rendered as FileBadge components by the code component.
 * Uses a 3-phase protect→match→restore approach to avoid breaking
 * existing markdown constructs.
 */
function preprocessFilePaths(content: string): string {
  // Phase 1: Protect constructs that must not be modified
  const saved: string[] = [];
  const protect = (m: string) => {
    saved.push(m);
    return `\x00${saved.length - 1}\x00`;
  };

  let s = content
    .replace(/```[\s\S]*?```/g, protect)                // fenced code blocks
    .replace(/`[^`\n]+`/g, protect)                      // inline code (already wrapped)
    .replace(/!\[[^\]]*\]\([^)]*\)/g, protect)           // image links
    .replace(/\[[^\]]*\]\([^)]*\)/g, protect)            // markdown links
    .replace(/\[[^\]]*\]\[[^\]]*\]/g, protect)           // reference links
    .replace(/(?:https?|ftp):\/\/[^\s)>\]]+/gi, protect); // URLs

  // Phase 2: Wrap bare file paths in backticks
  const withSep =
    `(?:[A-Za-z]:[/\\\\]|\\.{1,2}[/\\\\]|[\\w\\-][\\w.\\-]*[/\\\\])` +
    `(?:[\\w .,()/\\\\~\\-\\u4e00-\\u9fff])*` +
    `\\.(?:${FILE_EXT})`;

  const bare = `[\\w][\\w.\\-]*\\.(?:${FILE_EXT})`;

  const filePathRx = new RegExp(
    `(?<![\\w\`/\\\\])(?:${withSep}|${bare})(?![\\w/\\\\]|\\.\\w)`,
    'gi',
  );

  s = s.replace(filePathRx, '`$&`');

  // Phase 3: Restore protected constructs
  return s.replace(/\x00(\d+)\x00/g, (_, i) => saved[+i]);
}

/** Shared markdown component map for ReactMarkdown */
const markdownComponents: Record<string, React.ComponentType<ComponentPropsWithoutRef<any>>> = {
  a: MarkdownLink,
  pre({ children, ...rest }: ComponentPropsWithoutRef<'pre'>) {
    return (
      <pre
        {...rest}
        className="bg-surface-0 border border-border rounded-md px-3 py-2 my-2 text-xs overflow-x-auto"
      >
        {children}
      </pre>
    );
  },
  code({ children, className, ...rest }: ComponentPropsWithoutRef<'code'> & { className?: string }) {
    // If wrapped in <pre> it's a fenced code block – className contains language
    const isBlock = className?.startsWith('language-');
    if (isBlock) {
      return (
        <code {...rest} className={className}>
          {children}
        </code>
      );
    }
    // Detect file paths in inline code and render as FileBadge
    const text = typeof children === 'string' ? children : Array.isArray(children) ? children.join('') : '';
    if (
      typeof text === 'string' &&
      text.length > 0 &&
      FILE_PATH_REGEX.test(text)
    ) {
      return <FileBadge path={text} />;
    }
    return (
      <code
        {...rest}
        className="bg-surface-0 border border-border rounded px-1 py-0.5 text-xs"
      >
        {children}
      </code>
    );
  },
  h1({ children, ...r }: ComponentPropsWithoutRef<'h1'>) {
    return <h1 {...r} className="text-xl font-bold mt-4 mb-2">{children}</h1>;
  },
  h2({ children, ...r }: ComponentPropsWithoutRef<'h2'>) {
    return <h2 {...r} className="text-lg font-semibold mt-3 mb-1.5">{children}</h2>;
  },
  h3({ children, ...r }: ComponentPropsWithoutRef<'h3'>) {
    return <h3 {...r} className="text-base font-semibold mt-3 mb-1">{children}</h3>;
  },
  h4({ children, ...r }: ComponentPropsWithoutRef<'h4'>) {
    return <h4 {...r} className="text-sm font-semibold mt-2 mb-1">{children}</h4>;
  },
  ul({ children, ...r }: ComponentPropsWithoutRef<'ul'>) {
    return <ul {...r} className="list-disc list-inside my-1.5 space-y-0.5">{children}</ul>;
  },
  ol({ children, ...r }: ComponentPropsWithoutRef<'ol'>) {
    return <ol {...r} className="list-decimal list-inside my-1.5 space-y-0.5">{children}</ol>;
  },
  li({ children, ...r }: ComponentPropsWithoutRef<'li'>) {
    return <li {...r} className="leading-relaxed">{children}</li>;
  },
  blockquote({ children, ...r }: ComponentPropsWithoutRef<'blockquote'>) {
    return (
      <blockquote
        {...r}
        className="border-l-2 border-accent/40 pl-3 my-2 text-text-secondary italic"
      >
        {children}
      </blockquote>
    );
  },
  table({ children, ...r }: ComponentPropsWithoutRef<'table'>) {
    return (
      <div className="overflow-x-auto my-2">
        <table {...r} className="min-w-full text-xs border border-border rounded-md">
          {children}
        </table>
      </div>
    );
  },
  thead({ children, ...r }: ComponentPropsWithoutRef<'thead'>) {
    return <thead {...r} className="bg-surface-3">{children}</thead>;
  },
  th({ children, ...r }: ComponentPropsWithoutRef<'th'>) {
    return (
      <th {...r} className="px-2 py-1 text-left font-medium border-b border-border">
        {children}
      </th>
    );
  },
  td({ children, ...r }: ComponentPropsWithoutRef<'td'>) {
    return (
      <td {...r} className="px-2 py-1 border-b border-border">
        {children}
      </td>
    );
  },
  tr({ children, ...r }: ComponentPropsWithoutRef<'tr'>) {
    return <tr {...r} className="even:bg-surface-0/30">{children}</tr>;
  },
  hr(r: ComponentPropsWithoutRef<'hr'>) {
    return <hr {...r} className="border-border my-3" />;
  },
  p({ children, ...r }: ComponentPropsWithoutRef<'p'>) {
    return <p {...r} className="my-1.5 leading-relaxed">{children}</p>;
  },
  strong({ children, ...r }: ComponentPropsWithoutRef<'strong'>) {
    return <strong {...r} className="font-semibold">{children}</strong>;
  },
};

/* ------------------------------------------------------------------ */
/*  Message bubble                                                     */
/* ------------------------------------------------------------------ */

type FeedbackState = 'up' | 'down' | null;

interface MessageActionsProps {
  text: string;
  showFeedback: boolean;
  chunkIds?: string[];
  queryText?: string;
}

function MessageActions({ text, showFeedback, chunkIds = [], queryText = '' }: MessageActionsProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [feedback, setFeedback] = useState<FeedbackState>(null);
  const [submitting, setSubmitting] = useState(false);

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
    if (feedback === type) {
      setFeedback(null);
      return;
    }
    if (chunkIds.length === 0 || !queryText) {
      setFeedback(type);
      return;
    }
    setSubmitting(true);
    try {
      const action = type === 'up' ? 'upvote' : 'downvote';
      await Promise.all(chunkIds.map((id) => api.addFeedback(id, queryText, action)));
      setFeedback(type);
    } catch {
      // Silently fail — feedback is best-effort
    } finally {
      setSubmitting(false);
    }
  }, [feedback, chunkIds, queryText]);

  const actionBtn =
    'p-1 rounded-md bg-surface-0/80 border border-border/50 text-text-tertiary hover:text-text-primary transition-colors cursor-pointer';

  return (
    <div className="absolute top-1.5 right-1.5 flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
      <button
        type="button"
        onClick={handleCopy}
        title={copied ? t('chat.copied') : t('chat.copyMessage')}
        className={actionBtn}
      >
        {copied ? (
          <Check className="h-3.5 w-3.5 text-green-500" />
        ) : (
          <Copy className="h-3.5 w-3.5" />
        )}
      </button>
      {showFeedback && (
        <>
          <button
            type="button"
            onClick={() => handleFeedback('up')}
            disabled={submitting}
            title={t('chat.feedbackGood')}
            className={`${actionBtn} ${feedback === 'up' ? 'text-success' : ''} ${submitting ? 'opacity-50 pointer-events-none' : ''}`}
          >
            <ThumbsUp className="h-3.5 w-3.5" fill={feedback === 'up' ? 'currentColor' : 'none'} />
          </button>
          <button
            type="button"
            onClick={() => handleFeedback('down')}
            disabled={submitting}
            title={t('chat.feedbackBad')}
            className={`${actionBtn} ${feedback === 'down' ? 'text-danger' : ''} ${submitting ? 'opacity-50 pointer-events-none' : ''}`}
          >
            <ThumbsDown className="h-3.5 w-3.5" fill={feedback === 'down' ? 'currentColor' : 'none'} />
          </button>
        </>
      )}
    </div>
  );
}

interface MessageBubbleProps {
  msg: ConversationMessage;
  chunkIds?: string[];
  queryText?: string;
}

function MessageBubble({ msg, chunkIds, queryText }: MessageBubbleProps) {
  const { t } = useTranslation();
  const isUser = msg.role === 'user';

  if (msg.role === 'tool' || msg.role === 'system') return null;

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className={`flex ${isUser ? 'justify-end' : 'justify-start'} mb-3`}
    >
      <div
        className={`group relative max-w-[80%] rounded-lg px-3.5 py-2.5 text-sm leading-relaxed
          ${isUser
            ? 'bg-accent/20 text-text-primary'
            : 'bg-surface-2 text-text-primary'
          }`}
      >
        <MessageActions text={msg.content} showFeedback={!isUser} chunkIds={chunkIds} queryText={queryText} />
        {msg.tokenCount > 0 && (
          <span
            className="absolute bottom-0.5 right-2 text-[9px] text-text-tertiary/0 group-hover:text-text-tertiary/60 transition-colors tabular-nums select-none"
            title={`${msg.tokenCount.toLocaleString()} tokens`}
          >
            {msg.tokenCount.toLocaleString()} {t('chat.tokensShort')}
          </span>
        )}
        {isUser ? (
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
          <div className="prose-chat">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
              {preprocessFilePaths(preprocessCitations(msg.content))}
            </ReactMarkdown>
          </div>
        )}
      </div>
    </motion.div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatMessages({ messages, streamText, thinkingText, isThinking, toolCalls, isStreaming }: ChatMessagesProps) {
  const { t } = useTranslation();
  const bottomRef = useRef<HTMLDivElement>(null);

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

  // Auto-scroll on new messages or streaming
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamText, toolCalls]);

  // Empty state
  if (messages.length === 0 && !isStreaming) {
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

  return (
    <div className="flex-1 overflow-y-auto px-4 py-4">
      <AnimatePresence initial={false}>
        {messages.map((msg, idx) => {
          // Skip tool result messages (rendered inline with tool calls)
          if (msg.role === 'tool' || msg.role === 'system') return null;

          // Resolve feedback context for assistant messages
          const queryText = msg.role === 'assistant'
            ? (messages.slice(0, idx).reverse().find((m) => m.role === 'user')?.content ?? '')
            : '';
          const chunkIds = chunkIdCacheRef.current.get(msg.id) ?? [];

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

              <MessageBubble msg={msg} chunkIds={chunkIds} queryText={queryText} />

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
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
                {preprocessFilePaths(preprocessCitations(displayedText))}
              </ReactMarkdown>
              {isRevealing && (
                <span className="inline-block w-1.5 h-4 bg-accent animate-pulse ml-0.5 align-text-bottom rounded-sm" />
              )}
            </div>
          </div>
        </motion.div>
      )}

      {/* Thinking indicator */}
      {isStreaming && !streamText && toolCalls.length === 0 && !isThinking && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="flex justify-start mb-3"
        >
          <div className="rounded-lg px-3.5 py-2.5 bg-surface-2">
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

      <div ref={bottomRef} />
    </div>
  );
}
