import { useState, useRef, useEffect, useLayoutEffect, type ComponentPropsWithoutRef } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { ChevronRight, Brain } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useTranslation } from '../../i18n';
import { getSoftCollapseMotion } from '../../lib/uiMotion';

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
}: ThinkingBlockProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [expanded, setExpanded] = useState(defaultExpanded ?? isStreaming);
  const startTimeRef = useRef<number>(Date.now());
  const prevStreamingRef = useRef(isStreaming);
  const autoOpenedRef = useRef(false);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const userScrolledUpRef = useRef(false);
  const [elapsed, setElapsed] = useState(0);
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
      startTimeRef.current = Date.now();
      setElapsed(0);
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

  // Track elapsed thinking time
  useEffect(() => {
    if (!isStreaming) {
      // Capture final elapsed
      setElapsed(Math.round((Date.now() - startTimeRef.current) / 1000));
      return;
    }

    const interval = setInterval(() => {
      setElapsed(Math.round((Date.now() - startTimeRef.current) / 1000));
    }, 1000);

    return () => clearInterval(interval);
  }, [isStreaming]);

  // Auto-follow: keep the inner trace panel scrolled to the latest token
  // while streaming, unless the user has scrolled up away from the bottom.
  useLayoutEffect(() => {
    if (!isStreaming || !expanded) return;
    const el = scrollContainerRef.current;
    if (!el) return;
    if (userScrolledUpRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [combinedContent, isStreaming, expanded]);

  // Reset the "user scrolled up" guard whenever a new streaming phase starts.
  useEffect(() => {
    if (isStreaming) {
      userScrolledUpRef.current = false;
    }
  }, [isStreaming]);

  const summaryText = isStreaming
    ? t('chat.thinkingElapsed', { seconds: elapsed.toString() })
    : elapsed > 0
      ? t('chat.thoughtFor', { seconds: elapsed.toString() })
      : t('chat.thinkingCompleted');
  const traceActive = isStreaming && !shouldReduceMotion;
  const thinkingMood = moodOrder[Math.floor(elapsed / 6) % moodOrder.length] ?? THINKING_MOODS[0];

  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        aria-expanded={expanded}
        className="flex items-center gap-1.5 text-xs text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer group"
      >
        <ChevronRight
          size={12}
          className={`transition-transform duration-200 ${expanded ? 'rotate-90' : ''}`}
        />
        <span className="flex items-center gap-1.5">
          <Brain size={12} />
          <span>{summaryText}</span>
          {isStreaming && (
            <>
              <span
                aria-hidden="true"
                className="hidden min-w-[5.5rem] text-center font-mono text-[11px] text-accent/80 sm:inline-block"
              >
                {thinkingMood}
              </span>
              <span className="flex gap-0.5 ml-0.5">
                <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '0ms' }} />
                <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '150ms' }} />
                <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '300ms' }} />
              </span>
            </>
          )}
        </span>
      </button>

      <AnimatePresence initial={false}>
        {expanded && (combinedContent || effectiveSections || children) && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion)}
            className="overflow-hidden"
          >
            <div className={`mt-1 ml-4 border-l pl-3 ${traceActive ? 'border-accent/30' : 'border-border/35'}`}>
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
                      <div key={secIdx}>
                        {secIdx > 0 && <div className="my-1.5 border-t border-border/20" />}
                        {sec.text && (
                          <ReactMarkdown remarkPlugins={[remarkGfm]} components={thinkingMarkdownComponents}>
                            {sec.text}
                          </ReactMarkdown>
                        )}
                        {sec.node}
                        {sec.toolCallCards}
                      </div>
                    ))
                  ) : (
                    <ReactMarkdown remarkPlugins={[remarkGfm]} components={thinkingMarkdownComponents}>
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
              <div className="ml-4 mt-1 space-y-0.5 pb-0.5">
                {children}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
