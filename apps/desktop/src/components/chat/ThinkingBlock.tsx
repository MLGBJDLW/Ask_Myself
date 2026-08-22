import { useState, useRef, useEffect, type ComponentPropsWithoutRef } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { ChevronRight, Brain } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import { useTranslation } from '../../i18n';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import { MermaidBlock, markdownRemarkPlugins, rehypePlugins } from './markdownComponents';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

export interface ThinkingSection {
  text: string;
  toolCallCards?: React.ReactNode;
  node?: React.ReactNode;
}

interface ThinkingBlockProps {
  content: string;
  sections?: ThinkingSection[];
  isStreaming?: boolean;
  defaultExpanded?: boolean;
  collapseOnFinish?: boolean;
  children?: React.ReactNode;
  elapsedLabel?: string | null;
}

const THINKING_MOODS = [
  '(｡•̀ᴗ-)✧',
  '(・・?)',
  '( •̀ ω •́ )',
  '(。-`ω´-)',
  '(๑>◡<๑)',
  '(づ｡◕‿‿◕｡)づ',
  '(っ˘ω˘ς )',
  '(๑˃ᴗ˂)ﻭ',
  '(´｡• ᵕ •｡`)',
  '(✿◠‿◠)',
  '(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧',
  '(๑•̀ㅂ•́)و✧',
];

function shuffledThinkingMoods(): string[] {
  const moods = [...THINKING_MOODS];
  for (let i = moods.length - 1; i > 0; i -= 1) {
    const j = Math.floor(Math.random() * (i + 1));
    [moods[i], moods[j]] = [moods[j], moods[i]];
  }
  return moods;
}

/* ------------------------------------------------------------------ */
/*  Minimal markdown overrides (muted style)                           */
/* ------------------------------------------------------------------ */

const thinkingMarkdownComponents: Record<string, React.ComponentType<ComponentPropsWithoutRef<any>>> = {
  p({ children, ...r }: ComponentPropsWithoutRef<'p'>) {
    return <p {...r} className="my-1 leading-relaxed">{children}</p>;
  },
  pre({ children, ...rest }: ComponentPropsWithoutRef<'pre'>) {
    const child = children as React.ReactElement<{ className?: string }> | undefined;
    if (child?.props?.className?.startsWith('language-')) {
      return <>{children}</>;
    }
    return (
      <pre
        {...rest}
        className="bg-surface-0/50 border border-border/50 rounded-md px-2.5 py-1.5 my-1.5 text-xs overflow-x-auto"
      >
        {children}
      </pre>
    );
  },
  code({ children, className, ...rest }: ComponentPropsWithoutRef<'code'> & { className?: string }) {
    const isBlock = className?.startsWith('language-');
    if (isBlock) {
      const language = className?.replace('language-', '') ?? '';
      const raw = typeof children === 'string'
        ? children
        : Array.isArray(children)
          ? children.join('')
          : String(children ?? '');
      const code = raw.replace(/\n$/, '');

      if (language.toLowerCase() === 'mermaid') {
        return <MermaidBlock chart={code} />;
      }

      return <code {...rest} className={className}>{children}</code>;
    }
    return (
      <code {...rest} className="bg-surface-0/50 border border-border/50 rounded px-1 py-0.5 text-xs">
        {children}
      </code>
    );
  },
  ul({ children, ...r }: ComponentPropsWithoutRef<'ul'>) {
    return <ul {...r} className="list-disc list-inside my-1 space-y-0.5">{children}</ul>;
  },
  ol({ children, ...r }: ComponentPropsWithoutRef<'ol'>) {
    return <ol {...r} className="list-decimal list-inside my-1 space-y-0.5">{children}</ol>;
  },
  blockquote({ children, ...r }: ComponentPropsWithoutRef<'blockquote'>) {
    return (
      <blockquote {...r} className="border-l-2 border-text-tertiary/30 pl-2.5 my-1.5 italic opacity-80">
        {children}
      </blockquote>
    );
  },
};

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ThinkingBlock({
  content,
  sections,
  isStreaming = false,
  defaultExpanded,
  collapseOnFinish = true,
  children,
  elapsedLabel,
}: ThinkingBlockProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [expanded, setExpanded] = useState(defaultExpanded ?? isStreaming);
  const prevStreamingRef = useRef(isStreaming);
  const autoOpenedRef = useRef(false);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const scrollFrameRef = useRef<number | null>(null);
  const userScrolledUpRef = useRef(false);
  const [moodOrder, setMoodOrder] = useState(() => shuffledThinkingMoods());

  const effectiveSections = sections && sections.length > 0 ? sections : null;
  const combinedContent = effectiveSections
    ? effectiveSections.map(s => s.text).filter(Boolean).join('\n')
    : content;
  const hasSectionCards = Boolean(effectiveSections?.some(section => section.toolCallCards || section.node));

  // Keep the live trace open while it is streaming, then collapse it once that phase ends.
  useEffect(() => {
    const hasContent = combinedContent.trim().length > 0 || hasSectionCards;
    if (!prevStreamingRef.current && isStreaming) {
      setMoodOrder(shuffledThinkingMoods());
    }
    if (isStreaming && hasContent) {
      setExpanded(true);
      autoOpenedRef.current = true;
    }
    if (collapseOnFinish && prevStreamingRef.current && !isStreaming && autoOpenedRef.current) {
      setExpanded(false);
      autoOpenedRef.current = false;
    }
    prevStreamingRef.current = isStreaming;
  }, [collapseOnFinish, combinedContent, hasSectionCards, isStreaming]);

  // Auto-follow: keep the inner trace panel scrolled to the latest token
  // while streaming, unless the user has scrolled up away from the bottom.
  useEffect(() => {
    if (!isStreaming || !expanded) return;
    const el = scrollContainerRef.current;
    if (!el) return;
    if (userScrolledUpRef.current) return;
    if (scrollFrameRef.current != null) {
      cancelAnimationFrame(scrollFrameRef.current);
    }
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = null;
      el.scrollTo({ top: el.scrollHeight, behavior: 'auto' });
    });
    return () => {
      if (scrollFrameRef.current != null) {
        cancelAnimationFrame(scrollFrameRef.current);
        scrollFrameRef.current = null;
      }
    };
  }, [combinedContent, isStreaming, expanded]);

  // Reset the "user scrolled up" guard whenever a new streaming phase starts.
  useEffect(() => {
    if (isStreaming) {
      userScrolledUpRef.current = false;
    }
  }, [isStreaming]);

  const summaryText = isStreaming
    ? elapsedLabel
      ? `${t('chat.thinking')} · ${elapsedLabel}`
      : t('chat.thinking')
    : elapsedLabel
      ? `${t('chat.thinkingCompleted')} · ${elapsedLabel}`
      : t('chat.thinkingCompleted');
  const traceActive = isStreaming && !shouldReduceMotion;
  const thinkingMood = moodOrder[0] ?? THINKING_MOODS[0];

  return (
    <div
      className="thinking-trace chat-thinking-text mb-2"
      data-trace-active={traceActive ? 'true' : 'false'}
    >
      <span className="thinking-trace-node" aria-hidden="true" />
      <button
        type="button"
        data-testid="thinking-trace-toggle"
        data-trace-state={isStreaming ? "active" : "complete"}
        onClick={() => setExpanded(!expanded)}
        aria-expanded={expanded}
        className="thinking-trace-header flex items-center gap-1.5 text-xs text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer group"
      >
        <ChevronRight
          size={12}
          className={`transition-transform duration-200 ${expanded ? 'rotate-90' : ''}`}
        />
        <span className="flex items-center gap-1.5">
          <Brain size={12} className={isStreaming ? 'text-accent/80' : ''} />
          <span
            className={`thinking-status-text ${isStreaming && !shouldReduceMotion ? 'thinking-status-text-active' : ''}`}
          >
            {summaryText}
          </span>
          {isStreaming && (
            <span
              aria-hidden="true"
              className="thinking-mood-badge hidden min-w-[5.5rem] text-center font-mono text-[11px] sm:inline-block"
            >
              {thinkingMood}
            </span>
          )}
        </span>
      </button>

      <AnimatePresence initial={false}>
        {expanded && (combinedContent || effectiveSections || children) && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion || isStreaming)}
            className="overflow-hidden"
          >
            <div className="thinking-trace-body mt-1 pl-1">
              <div
                ref={scrollContainerRef}
                onScroll={(e) => {
                  const el = e.currentTarget;
                  const distanceFromBottom =
                    el.scrollHeight - el.scrollTop - el.clientHeight;
                  userScrolledUpRef.current = distanceFromBottom > 40;
                }}
                className="relative max-h-[300px] overflow-y-auto py-1 pr-6 text-xs leading-relaxed text-text-secondary"
              >
                <div className="space-y-1">
                  {effectiveSections ? (
                    effectiveSections.map((sec, secIdx) => (
                      <div className="thinking-trace-section" key={secIdx}>
                        {secIdx > 0 && <div className="my-1.5 border-t border-border/20" />}
                        {sec.text && isStreaming ? (
                          <div className="whitespace-pre-wrap break-words">{sec.text}</div>
                        ) : sec.text ? (
                          <ReactMarkdown
                            remarkPlugins={markdownRemarkPlugins}
                            rehypePlugins={rehypePlugins}
                            components={thinkingMarkdownComponents}
                          >
                            {sec.text}
                          </ReactMarkdown>
                        ) : null}
                        {sec.node}
                        {sec.toolCallCards}
                      </div>
                    ))
                  ) : isStreaming ? (
                    <div className="whitespace-pre-wrap break-words">{content}</div>
                  ) : (
                    <ReactMarkdown
                      remarkPlugins={markdownRemarkPlugins}
                      rehypePlugins={rehypePlugins}
                      components={thinkingMarkdownComponents}
                    >
                      {content}
                    </ReactMarkdown>
                  )}
                </div>
                {isStreaming && (
                  <span className={`streaming-caret-overlay ${shouldReduceMotion ? '' : 'animate-pulse'}`} />
                )}
              </div>
            </div>
            {children && (
              <div className="thinking-trace-children mt-1 space-y-0.5 pb-0.5 pl-1">
                {children}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
